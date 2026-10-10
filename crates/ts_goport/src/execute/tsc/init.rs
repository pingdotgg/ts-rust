//! Go: execute/tsc/init.go (`tsc --init`).

use crate::prelude::*;

use super::compile::{System, write_str};
use super::diagnostics::DiagnosticReporter;
use super::help::{format_value_v, get_header};
use crate::diagnostics::Message;
use crate::diagnostics_loc::message_localize;
use crate::execute::incremental::build_info::marshal_any;
use crate::frontend::tsoptions::{
    CommandLineOption, CommandLineOptionEnumMap, CompilerOptionsValue, OPTIONS_DECLARATIONS,
};
use crate::frontend::tspath::{combine_paths, normalize_path};
use crate::locale::Locale;

// Go: execute/tsc/init.go:18 WriteConfigFile
// PORT: Go `options` is the command line `Raw` map
// (`*collections.OrderedMap[string, any]`); here it is the `IndexMap` in
// `CompilerOptionsValue::Map`.
pub fn write_config_file(
    sys: &dyn System,
    locale: &Locale,
    report_diagnostic: &DiagnosticReporter,
    options: &IndexMap<String, CompilerOptionsValue>,
) {
    let get_current_directory = sys.get_current_directory();
    let file = normalize_path(&combine_paths(&get_current_directory, &["tsconfig.json"]));
    if sys.fs().file_exists(&file) {
        report_diagnostic(&new_compiler_diagnostic(
            diag::A_tsconfig_json_file_is_already_defined_at_Colon_0,
            args![file],
        ));
    } else {
        let _ = sys
            .fs()
            .write_file(&file, &generate_ts_config(options, locale));
        let mut output = vec!["\n".to_string()];
        output.extend(get_header(sys, "Created a new tsconfig.json"));
        output.extend([
            "You can learn more at https://aka.ms/tsconfig".to_string(),
            "\n".to_string(),
        ]);
        write_str(&sys.writer(), &output.join(""));
    }
}

// Go: execute/tsc/init.go:33 tab
const TAB: &str = "  ";

// Go: execute/tsc/init.go:106 commented
// commentedNever': Never comment this out
// commentedAlways': Always comment this out, even if it's on commandline
// commentedOptional': Comment out unless it's on commandline
// PORT: a Go `int` with three constants is a Rust enum, so the Go
// "invalid `commented`" panic in `emitOption` cannot happen.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Commented {
    Never,
    Always,
    Optional,
}

// PORT: Go `generateTSConfig` keeps `result` and `allSetOptions` in local
// variables and changes them from closures (`emitHeader`, `newline`,
// `push`, `emitOption`). Here they are fields of this struct, and the
// closures are its methods.
struct TsConfigWriter<'a> {
    options: &'a IndexMap<String, CompilerOptionsValue>,
    locale: &'a Locale,
    result: Vec<String>,
    all_set_options: Vec<&'a str>,
}

impl TsConfigWriter<'_> {
    // Go: execute/tsc/init.go:43 emitHeader
    fn emit_header(&mut self, header: &'static Message) {
        self.result.push(format!(
            "{TAB}{TAB}// {}",
            message_localize(header, self.locale, &[])
        ));
    }

    // Go: execute/tsc/init.go:46 newline
    fn newline(&mut self) {
        self.result.push(String::new());
    }

    // Go: execute/tsc/init.go:49 push
    fn push(&mut self, line: String) {
        self.result.push(line);
    }

    // Go: execute/tsc/init.go:112 emitOption
    fn emit_option(
        &mut self,
        setting: &str,
        default_value: CompilerOptionsValue,
        commented: Commented,
    ) {
        if let Some(existing_option_index) = self
            .all_set_options
            .iter()
            .position(|option| *option == setting)
        {
            self.all_set_options.remove(existing_option_index);
        }

        let comment = match commented {
            Commented::Always => true,
            Commented::Never => false,
            Commented::Optional => !self.options.contains_key(setting),
        };

        let value = match self.options.get(setting) {
            Some(value) => value.clone(),
            None => default_value,
        };

        if comment {
            self.push(format!(
                "{TAB}{TAB}// \"{setting}\": {},",
                format_value_or_array(setting, &value)
            ));
        } else {
            self.push(format!(
                "{TAB}{TAB}\"{setting}\": {},",
                format_value_or_array(setting, &value)
            ));
        }
    }
}

// Go: execute/tsc/init.go:32 generateTSConfig
fn generate_ts_config(options: &IndexMap<String, CompilerOptionsValue>, locale: &Locale) -> String {
    let mut all_set_options: Vec<&str> = Vec::with_capacity(options.len());
    for k in options.keys() {
        if k != "init" && k != "help" && k != "watch" {
            all_set_options.push(k);
        }
    }

    let mut w = TsConfigWriter {
        options,
        locale,
        result: Vec::new(),
        all_set_options,
    };

    w.push("{".to_string());
    w.push(format!(
        "{TAB}// {}",
        message_localize(
            diag::Visit_https_Colon_Slash_Slashaka_ms_Slashtsconfig_to_read_more_about_this_file,
            locale,
            &[]
        )
    ));
    w.push(format!("{TAB}\"compilerOptions\": {{"));

    w.emit_header(diag::File_Layout);
    w.emit_option(
        "rootDir",
        CompilerOptionsValue::String("./src".to_string()),
        Commented::Optional,
    );
    w.emit_option(
        "outDir",
        CompilerOptionsValue::String("./dist".to_string()),
        Commented::Optional,
    );

    w.newline();

    w.emit_header(diag::Environment_Settings);
    w.emit_header(diag::See_also_https_Colon_Slash_Slashaka_ms_Slashtsconfig_Slashmodule);
    w.emit_option(
        "module",
        CompilerOptionsValue::ModuleKind(ModuleKind::NODE_NEXT),
        Commented::Never,
    );
    w.emit_option(
        "target",
        CompilerOptionsValue::ScriptTarget(ScriptTarget::ES_NEXT),
        Commented::Never,
    );
    w.emit_option(
        "types",
        CompilerOptionsValue::List(Vec::new()),
        Commented::Never,
    );
    if let Some(lib) = options.get("lib") {
        w.emit_option("lib", lib.clone(), Commented::Never);
    }
    w.emit_header(diag::For_nodejs_Colon);
    w.push(format!("{TAB}{TAB}// \"lib\": [\"esnext\"],"));
    w.push(format!("{TAB}{TAB}// \"types\": [\"node\"],"));
    w.emit_header(diag::X_and_npm_install_D_types_Slashnode);

    w.newline();

    w.emit_header(diag::Other_Outputs);
    w.emit_option(
        "sourceMap",
        CompilerOptionsValue::Bool(true),
        Commented::Never,
    );
    w.emit_option(
        "declaration",
        CompilerOptionsValue::Bool(true),
        Commented::Never,
    );
    w.emit_option(
        "declarationMap",
        CompilerOptionsValue::Bool(true),
        Commented::Never,
    );

    w.newline();

    w.emit_header(diag::Stricter_Typechecking_Options);
    w.emit_option(
        "noUncheckedIndexedAccess",
        CompilerOptionsValue::Bool(true),
        Commented::Never,
    );
    w.emit_option(
        "exactOptionalPropertyTypes",
        CompilerOptionsValue::Bool(true),
        Commented::Never,
    );

    w.newline();

    w.emit_header(diag::Style_Options);
    w.emit_option(
        "noImplicitReturns",
        CompilerOptionsValue::Bool(true),
        Commented::Optional,
    );
    w.emit_option(
        "noImplicitOverride",
        CompilerOptionsValue::Bool(true),
        Commented::Optional,
    );
    w.emit_option(
        "noUnusedLocals",
        CompilerOptionsValue::Bool(true),
        Commented::Optional,
    );
    w.emit_option(
        "noUnusedParameters",
        CompilerOptionsValue::Bool(true),
        Commented::Optional,
    );
    w.emit_option(
        "noFallthroughCasesInSwitch",
        CompilerOptionsValue::Bool(true),
        Commented::Optional,
    );
    w.emit_option(
        "noPropertyAccessFromIndexSignature",
        CompilerOptionsValue::Bool(true),
        Commented::Optional,
    );

    w.newline();

    w.emit_header(diag::Recommended_Options);
    w.emit_option("strict", CompilerOptionsValue::Bool(true), Commented::Never);
    w.emit_option(
        "jsx",
        CompilerOptionsValue::JsxEmit(JsxEmit::REACT_JSX),
        Commented::Never,
    );
    w.emit_option(
        "verbatimModuleSyntax",
        CompilerOptionsValue::Bool(true),
        Commented::Never,
    );
    w.emit_option(
        "isolatedModules",
        CompilerOptionsValue::Bool(true),
        Commented::Never,
    );
    w.emit_option(
        "noUncheckedSideEffectImports",
        CompilerOptionsValue::Bool(true),
        Commented::Never,
    );
    w.emit_option(
        "moduleDetection",
        CompilerOptionsValue::ModuleDetectionKind(ModuleDetectionKind::FORCE),
        Commented::Never,
    );
    w.emit_option(
        "skipLibCheck",
        CompilerOptionsValue::Bool(true),
        Commented::Never,
    );

    // Write any user-provided options we haven't already
    if !w.all_set_options.is_empty() {
        w.newline();
        while !w.all_set_options.is_empty() {
            let setting = w.all_set_options[0];
            // Go `options.GetOrZero`.
            let value = options.get(setting).cloned().unwrap_or_default();
            w.emit_option(setting, value, Commented::Never);
        }
    }

    w.push(format!("{TAB}}}"));
    w.push("}".to_string());
    w.push(String::new());

    w.result.join("\n")
}

// Go: execute/tsc/init.go:53 formatSingleValue
// PORT: Go `value == v` compares two `any` values by dynamic type and
// value. `CompilerOptionsValue` equality compares the variant and the value,
// and the command line parser stores the enum map value itself
// (`convert_json_option_of_enum_type`), so the same keys match.
fn format_single_value(
    value: &CompilerOptionsValue,
    enum_map: Option<&CommandLineOptionEnumMap>,
) -> String {
    let mut value = value.clone();
    if let Some(enum_map) = enum_map {
        let mut found = false;
        for (k, v) in enum_map {
            if value == *v {
                value = CompilerOptionsValue::String(k.clone());
                found = true;
                break;
            }
        }
        if !found {
            crate::core::go_panic(format!("No matching value of {}", format_value_v(&value)));
        }
    }

    // PORT: Go `json.MarshalIndent(value, "", "")` with an empty prefix and
    // indent is plain `json.Marshal` (internal/json/json.go). `marshal_any`
    // is the Go `any` marshaler.
    let mut b = String::new();
    if let Err(err) = marshal_any(&mut b, &value) {
        crate::core::go_panic(format!("should not happen: {err}"));
    }
    b
}

// Go: execute/tsc/init.go:75 formatValueOrArray
// PORT: Go tests `reflect.Kind() == reflect.Slice`. The slice values here
// are `List` and `NilList` (Go `[]any`: a parsed list option or the `types`
// default) and `StringList` (Go `[]string`: the parser's value for a list
// option with no argument).
fn format_value_or_array(setting_name: &str, value: &CompilerOptionsValue) -> String {
    // PORT: the Go loop has no break, so the last declaration with the name
    // wins.
    let mut option: Option<&'static CommandLineOption> = None;
    for &decl in OPTIONS_DECLARATIONS.iter() {
        if decl.name == setting_name {
            option = Some(decl);
        }
    }
    let Some(option) = option else {
        crate::core::go_panic(format!("No option named {setting_name}"));
    };

    let elems: Vec<CompilerOptionsValue> = match value {
        CompilerOptionsValue::List(values) => values.clone(),
        CompilerOptionsValue::NilList => Vec::new(),
        CompilerOptionsValue::StringList(values) => values
            .iter()
            .map(|value| CompilerOptionsValue::String(value.clone()))
            .collect(),
        _ => return format_single_value(value, option.enum_map()),
    };
    let enum_map = option
        .elements()
        .and_then(|elem_option| elem_option.enum_map());
    let elems: Vec<String> = elems
        .iter()
        .map(|elem| format_single_value(elem, enum_map))
        .collect();
    format!("[{}]", elems.join(", "))
}
