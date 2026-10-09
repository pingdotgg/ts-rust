use crate::ls::lsutil::prelude::*;

use crate::flags_macros::go_enum;
use crate::frontend::json::{
    JsonDecoder, JsonError, MarshalerTo, UnmarshalerFrom, json_unmarshal_decode,
};
use crate::modulespecifiers::{
    ImportModuleSpecifierEndingPreference, ImportModuleSpecifierPreference,
};
use std::borrow::Cow;
use std::sync::LazyLock;

// Port of Go `ls/lsutil/userpreferences.go`.
//
// PORT: Go walks the preference structs with `reflect`. Here a static field
// table (`USER_PREFERENCES_FIELDS`) lists every tagged field in Go field
// order, with the Go `raw` and `config` tag strings and a getter and setter.
// `collectFieldInfos` parses those tags as Go does. The table order decides
// the `withConfig` order and the `MarshalJSONTo` input order, as the Go field
// order does. Go `any` is `LspAny` and Go `map[string]any` is
// `IndexMap<String, LspAny>`. Go `nil` is `LspAny::Null`.
//
// PORT: Go `[]string` fields tell a nil slice from an empty one. Only
// `MarshalJSONTo` (nil is skipped) sees the difference. A Rust `Vec` has no
// nil, so an empty `Vec` counts as nil there.

// Go: ls/lsutil/userpreferences.go:16 NewDefaultUserPreferences
pub fn new_default_user_preferences() -> UserPreferences {
    UserPreferences {
        format_code_settings: get_default_format_code_settings(),

        include_completions_for_module_exports: Tristate::True,
        include_completions_for_import_statements: Tristate::True,
        enable_auto_closing_tags: Tristate::True,
        enable_js_doc_completions: Tristate::True,
        generate_return_in_doc_template: Tristate::True,

        allow_rename_of_import_path: Tristate::True,
        provide_refactor_not_applicable_reason: Tristate::True,
        enable_formatting: Tristate::True,
        enable_validation: Tristate::True,
        display_parts_for_js_doc: Tristate::True,
        disable_line_text_in_references: Tristate::True,
        report_style_checks_as_warnings: Tristate::True,

        exclude_library_symbols_in_nav_to: Tristate::True,
        workspace_symbols_scope: WorkspaceSymbolsScope::ALL_OPEN_PROJECTS,
        ..Default::default()
    }
}

// Go: ls/lsutil/userpreferences.go:47 UserPreferences (at 673a5f17d713; ts#64554 generates it: ls/lsutil/userpreferences_generated.go:89 UserPreferences)
// UserPreferences represents TypeScript language service preferences.
//
// Fields are populated using two tags:
//   - `raw:"name"` or `raw:"name,invert"` - TypeScript/raw name for unstable section lookup
//   - `config:"path.to.setting"` or `config:"path.to.setting,invert"` - VS Code nested config path
//
// At least one tag must be present on each preference field.
// The `,invert` modifier inverts boolean values (e.g., VS Code's "suppress" -> our "include").
// PORT: the tags are in `USER_PREFERENCES_FIELDS`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UserPreferences {
    pub format_code_settings: FormatCodeSettings,

    pub quote_preference: QuotePreference,
    pub lazy_configured_projects_from_external_project: Tristate, // !!!

    // A positive integer indicating the maximum length of a hover text before it is truncated.
    //
    // Default: `500`
    pub maximum_hover_length: i32, // !!!

    // ------- Completions -------

    // If enabled, TypeScript will search through all external modules' exports and add them to the completions list.
    // This affects lone identifier completions but not completions on the right hand side of `obj.`.
    pub include_completions_for_module_exports: Tristate,
    // Enables auto-import-style completions on partially-typed import statements. E.g., allows
    // `import write|` to be completed to `import { writeFile } from "fs"`.
    pub include_completions_for_import_statements: Tristate,
    // Unless this option is `false`,  member completion lists triggered with `.` will include entries
    // on potentially-null and potentially-undefined values, with insertion text to replace
    // preceding `.` tokens with `?.`.
    pub include_automatic_optional_chain_completions: Tristate,
    // If enabled, completions for class members (e.g. methods and properties) will include
    // a whole declaration for the member.
    // E.g., `class A { f| }` could be completed to `class A { foo(): number {} }`, instead of
    // `class A { foo }`.
    pub include_completions_with_class_member_snippets: Tristate,
    // If enabled, object literal methods will have a method declaration completion entry in addition
    // to the regular completion entry containing just the method name.
    // E.g., `const objectLiteral: T = { f| }` could be completed to `const objectLiteral: T = { foo(): void {} }`,
    // in addition to `const objectLiteral: T = { foo }`.
    pub include_completions_with_object_literal_method_snippets: Tristate,
    pub jsx_attribute_completion_style: JsxAttributeCompletionStyle,
    pub enable_auto_closing_tags: Tristate,
    pub enable_js_doc_completions: Tristate,
    pub generate_return_in_doc_template: Tristate,

    // ------- AutoImports --------
    pub import_module_specifier_preference: ImportModuleSpecifierPreference, // !!!
    // Determines whether we import `foo/index.ts` as "foo", "foo/index", or "foo/index.js"
    pub import_module_specifier_ending: ImportModuleSpecifierEndingPreference, // !!!
    pub auto_import_specifier_exclude_regexes: Vec<String>,                    // !!!
    pub auto_import_file_exclude_patterns: Vec<String>,
    pub auto_import_entrypoint_directory_search: Tristate,
    pub prefer_type_only_auto_imports: Tristate,

    // ------- OrganizeImports -------

    // Indicates which deterministic preset should be used to sort imports.
    // "auto" detects the existing ordinal case sensitivity where possible.
    pub organize_imports_sort: OrganizeImportsSort, // !!!
    // Indicates whether imports should be organized in a case-insensitive manner.
    //
    // Default: TSUnknown ("auto" in strada), will perform detection
    pub organize_imports_ignore_case: Tristate, // !!!
    // Indicates whether imports should be organized via an "ordinal" (binary) comparison using the numeric value of their
    // code points, or via "unicode" natural sorting. This implementation is locale-agnostic and approximates the practical
    // import-sorting behavior rather than the full Unicode Collation Algorithm.
    //
    // Default: Ordinal
    pub organize_imports_collation: OrganizeImportsCollation, // !!!
    // Indicates the locale to use for "unicode" collation in legacy clients. This is accepted for compatibility, but
    // currently ignored because organize-import sorting is deterministic and locale-agnostic.
    //
    // This preference is ignored if organizeImportsCollation is not `unicode`.
    //
    // Default: `"en"`
    pub organize_imports_locale: String, // !!!
    // Indicates whether numeric collation should be used for digit sequences in strings. When `true`, will collate
    // strings such that `a1z < a2z < a100z`. When `false`, will collate strings such that `a1z < a100z < a2z`.
    //
    // This preference is ignored if organizeImportsCollation is not `unicode`.
    //
    // Default: `false`
    pub organize_imports_numeric_collation: Tristate, // !!!
    // Indicates whether accents and other diacritic marks are considered unequal for the purpose of sorting.
    //
    // This preference is ignored if organizeImportsCollation is not `unicode`.
    //
    // Default: `true`
    pub organize_imports_accent_collation: Tristate, // !!!
    // Indicates whether upper case or lower case should sort first.
    //
    // This permission is ignored if:
    //	- organizeImportsCollation is not `unicode`
    //	- organizeImportsIgnoreCase is `true`
    //	- organizeImportsIgnoreCase is `auto` and the auto-detected case sensitivity is case-insensitive.
    //
    // Default: `false`
    pub organize_imports_case_first: OrganizeImportsCaseFirst, // !!!
    // Indicates where named type-only imports should sort. "inline" sorts named imports without regard to if the import is type-only.
    //
    // Default: `auto`, which defaults to `last`
    pub organize_imports_type_order: OrganizeImportsTypeOrder, // !!!

    // ------- MoveToFile -------
    pub allow_text_changes_in_new_files: Tristate, // !!!

    // ------- Rename -------
    pub use_aliases_for_rename: Tristate,
    pub allow_rename_of_import_path: Tristate,

    // ------- CodeFixes/Refactors -------
    pub provide_refactor_not_applicable_reason: Tristate, // !!!

    // ------- InlayHints -------
    pub inlay_hints: InlayHintsPreferences,

    // ------- CodeLens -------
    pub code_lens: CodeLensUserPreferences,

    // ------- Definition -------
    pub prefer_go_to_source_definition: bool,

    // ------- Symbols -------
    pub exclude_library_symbols_in_nav_to: Tristate,
    pub workspace_symbols_scope: WorkspaceSymbolsScope,

    // ------- Misc -------
    pub enable_formatting: Tristate,
    pub enable_validation: Tristate,
    pub disable_suggestions: Tristate,             // !!!
    pub disable_line_text_in_references: Tristate, // !!!
    pub display_parts_for_js_doc: Tristate,        // !!!
    pub report_style_checks_as_warnings: Tristate,
    pub locale: String,

    // ------- ATA -------

    // DisableAutomaticTypeAcquisition is the deprecated setting from typescript.disableAutomaticTypeAcquisition.
    pub disable_automatic_type_acquisition: Tristate,
    // AutomaticTypeAcquisitionEnabled is the unified setting from tsserver.automaticTypeAcquisition.enabled under the js/ts section.
    // When set, it takes precedence over DisableAutomaticTypeAcquisition.
    pub automatic_type_acquisition_enabled: Tristate,
    // TODO: add tsserver.web.typeAcquisition.enabled under the js/ts section for the web variant when web support is implemented.

    // ------- Project Configuration -------

    // CustomConfigFileName specifies a custom config file name to use before defaulting to tsconfig.json/jsconfig.json.
    pub custom_config_file_name: String,
}

impl UserPreferences {
    // Go: ls/lsutil/userpreferences.go:196 (UserPreferences).IsATADisabled
    // IsATADisabled returns whether Automatic Type Acquisition is disabled based on user preferences.
    // It checks the unified setting (tsserver.automaticTypeAcquisition.enabled) first,
    // then falls back to the deprecated setting (disableAutomaticTypeAcquisition).
    pub fn is_ata_disabled(&self) -> bool {
        if !self.automatic_type_acquisition_enabled.is_unknown() {
            return !self.automatic_type_acquisition_enabled.is_true();
        }
        self.disable_automatic_type_acquisition.is_true()
    }
}

// Go: ls/lsutil/userpreferences.go:209 InlayHintsPreferences (at 673a5f17d713; ts#64554 generates it: ls/lsutil/userpreferences_generated.go:77 InlayHintsPreferences)
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InlayHintsPreferences {
    pub include_inlay_parameter_name_hints: IncludeInlayParameterNameHints,
    pub include_inlay_parameter_name_hints_when_argument_matches_name: Tristate,
    pub include_inlay_function_parameter_type_hints: Tristate,
    pub include_inlay_variable_type_hints: Tristate,
    pub include_inlay_variable_type_hints_when_type_matches_name: Tristate,
    pub include_inlay_property_declaration_type_hints: Tristate,
    pub include_inlay_function_like_return_type_hints: Tristate,
    pub include_inlay_enum_member_value_hints: Tristate,
}

// Go: ls/lsutil/userpreferences.go:220 CodeLensUserPreferences (at 673a5f17d713; ts#64554 generates it: ls/lsutil/userpreferences_generated.go:14 CodeLensUserPreferences)
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CodeLensUserPreferences {
    pub references_code_lens_enabled: Tristate,
    pub implementations_code_lens_enabled: Tristate,
    pub references_code_lens_show_on_all_functions: Tristate,
    pub implementations_code_lens_show_on_interface_methods: Tristate,
    pub implementations_code_lens_show_on_all_class_methods: Tristate,
}

// --- Enum Types ---

// Go: ls/lsutil/userpreferences.go:40 QuotePreference
// PORT: a Go string type. The value is always one of the Go constants, so a
// `&'static str` holds it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct QuotePreference(pub &'static str);

impl QuotePreference {
    pub const UNKNOWN: QuotePreference = QuotePreference("");
    pub const AUTO: QuotePreference = QuotePreference("auto");
    pub const DOUBLE: QuotePreference = QuotePreference("double");
    pub const SINGLE: QuotePreference = QuotePreference("single");
}

// Go: ls/lsutil/userpreferences.go:49 WorkspaceSymbolsScope
// PORT: a Go string type. Since ts#64554 it has a parser
// (`parsePreferenceWorkspaceSymbolsScope`), so the value is one of the
// constants or "" (an unknown value). It stays a `Cow`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct WorkspaceSymbolsScope(pub Cow<'static, str>);

impl WorkspaceSymbolsScope {
    pub const ALL_OPEN_PROJECTS: WorkspaceSymbolsScope =
        WorkspaceSymbolsScope(Cow::Borrowed("allOpenProjects"));
    pub const CURRENT_PROJECT: WorkspaceSymbolsScope =
        WorkspaceSymbolsScope(Cow::Borrowed("currentProject"));
}

// Go: ls/lsutil/userpreferences.go:56 JsxAttributeCompletionStyle
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct JsxAttributeCompletionStyle(pub &'static str);

impl JsxAttributeCompletionStyle {
    pub const UNKNOWN: JsxAttributeCompletionStyle = JsxAttributeCompletionStyle("");
    pub const AUTO: JsxAttributeCompletionStyle = JsxAttributeCompletionStyle("auto");
    pub const BRACES: JsxAttributeCompletionStyle = JsxAttributeCompletionStyle("braces");
    pub const NONE: JsxAttributeCompletionStyle = JsxAttributeCompletionStyle("none");
}

// Go: ls/lsutil/userpreferences.go:65 IncludeInlayParameterNameHints
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct IncludeInlayParameterNameHints(pub &'static str);

impl IncludeInlayParameterNameHints {
    pub const NONE: IncludeInlayParameterNameHints = IncludeInlayParameterNameHints("");
    pub const ALL: IncludeInlayParameterNameHints = IncludeInlayParameterNameHints("all");
    pub const LITERALS: IncludeInlayParameterNameHints = IncludeInlayParameterNameHints("literals");
}

// Go: ls/lsutil/userpreferences.go:263 OrganizeImportsSort
go_enum!(OrganizeImportsSort, i32 {
    AUTO = 0;
    ORDINAL = 1;
    ORDINAL_IGNORE_CASE = 2;
    NATURAL = 3;
    NATURAL_IGNORE_CASE = 4;
});

// Go: ls/lsutil/userpreferences.go:83 OrganizeImportsCollation
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct OrganizeImportsCollation(pub bool);

impl OrganizeImportsCollation {
    pub const ORDINAL: OrganizeImportsCollation = OrganizeImportsCollation(false);
    pub const UNICODE: OrganizeImportsCollation = OrganizeImportsCollation(true);
}

// Go: ls/lsutil/userpreferences.go:280 OrganizeImportsCaseFirst
go_enum!(OrganizeImportsCaseFirst, i32 {
    FALSE = 0;
    LOWER = 1;
    UPPER = 2;
});

// Go: ls/lsutil/userpreferences.go:288 OrganizeImportsTypeOrder
go_enum!(OrganizeImportsTypeOrder, i32 {
    AUTO = 0;
    LAST = 1;
    INLINE = 2;
    FIRST = 3;
});

// --- Reflection-based parsing infrastructure ---

// PORT: Go `reflect.Type` of a preference field. It is the key of
// `typeParsers` and `typeSerializers`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FieldType {
    Tristate,
    IndentStyle,
    SemicolonPreference,
    QuotePreference,
    JsxAttributeCompletionStyle,
    IncludeInlayParameterNameHints,
    OrganizeImportsSort,
    OrganizeImportsCollation,
    OrganizeImportsCaseFirst,
    OrganizeImportsTypeOrder,
    ImportModuleSpecifierPreference,
    ImportModuleSpecifierEndingPreference,
    WorkspaceSymbolsScope,
    Int,
    String,
    Bool,
    StringSlice,
}

// PORT: Go `reflect.Kind` of the preference field types.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FieldKind {
    Bool,
    Int,
    Uint8,
    String,
    Slice,
}

impl FieldType {
    // PORT: Go `reflect.Type.Kind` (`core.Tristate` is a `byte`).
    fn kind(self) -> FieldKind {
        match self {
            FieldType::Tristate => FieldKind::Uint8,
            FieldType::IndentStyle
            | FieldType::OrganizeImportsSort
            | FieldType::OrganizeImportsCaseFirst
            | FieldType::OrganizeImportsTypeOrder
            | FieldType::Int => FieldKind::Int,
            FieldType::OrganizeImportsCollation | FieldType::Bool => FieldKind::Bool,
            FieldType::SemicolonPreference
            | FieldType::QuotePreference
            | FieldType::JsxAttributeCompletionStyle
            | FieldType::IncludeInlayParameterNameHints
            | FieldType::ImportModuleSpecifierPreference
            | FieldType::ImportModuleSpecifierEndingPreference
            | FieldType::WorkspaceSymbolsScope
            | FieldType::String => FieldKind::String,
            FieldType::StringSlice => FieldKind::Slice,
        }
    }
}

// PORT: a Go `reflect.Value` of a preference field, held by value. The
// variant is the Go field type.
#[derive(Clone, Debug, PartialEq)]
enum FieldValue {
    Tristate(Tristate),
    IndentStyle(IndentStyle),
    SemicolonPreference(SemicolonPreference),
    QuotePreference(QuotePreference),
    JsxAttributeCompletionStyle(JsxAttributeCompletionStyle),
    IncludeInlayParameterNameHints(IncludeInlayParameterNameHints),
    OrganizeImportsSort(OrganizeImportsSort),
    OrganizeImportsCollation(OrganizeImportsCollation),
    OrganizeImportsCaseFirst(OrganizeImportsCaseFirst),
    OrganizeImportsTypeOrder(OrganizeImportsTypeOrder),
    ImportModuleSpecifierPreference(ImportModuleSpecifierPreference),
    ImportModuleSpecifierEndingPreference(ImportModuleSpecifierEndingPreference),
    WorkspaceSymbolsScope(WorkspaceSymbolsScope),
    Int(i32),
    String(String),
    Bool(bool),
    StringSlice(Vec<String>),
}

impl FieldValue {
    // PORT: Go `reflect.Value.Type`.
    fn type_(&self) -> FieldType {
        match self {
            FieldValue::Tristate(_) => FieldType::Tristate,
            FieldValue::IndentStyle(_) => FieldType::IndentStyle,
            FieldValue::SemicolonPreference(_) => FieldType::SemicolonPreference,
            FieldValue::QuotePreference(_) => FieldType::QuotePreference,
            FieldValue::JsxAttributeCompletionStyle(_) => FieldType::JsxAttributeCompletionStyle,
            FieldValue::IncludeInlayParameterNameHints(_) => {
                FieldType::IncludeInlayParameterNameHints
            }
            FieldValue::OrganizeImportsSort(_) => FieldType::OrganizeImportsSort,
            FieldValue::OrganizeImportsCollation(_) => FieldType::OrganizeImportsCollation,
            FieldValue::OrganizeImportsCaseFirst(_) => FieldType::OrganizeImportsCaseFirst,
            FieldValue::OrganizeImportsTypeOrder(_) => FieldType::OrganizeImportsTypeOrder,
            FieldValue::ImportModuleSpecifierPreference(_) => {
                FieldType::ImportModuleSpecifierPreference
            }
            FieldValue::ImportModuleSpecifierEndingPreference(_) => {
                FieldType::ImportModuleSpecifierEndingPreference
            }
            FieldValue::WorkspaceSymbolsScope(_) => FieldType::WorkspaceSymbolsScope,
            FieldValue::Int(_) => FieldType::Int,
            FieldValue::String(_) => FieldType::String,
            FieldValue::Bool(_) => FieldType::Bool,
            FieldValue::StringSlice(_) => FieldType::StringSlice,
        }
    }

    // PORT: Go `reflect.Value.Bool` (Bool kinds).
    fn bool_(&self) -> bool {
        match self {
            FieldValue::OrganizeImportsCollation(v) => v.0,
            FieldValue::Bool(v) => *v,
            _ => panic!(
                "reflect: call of reflect.Value.Bool on {:?} Value",
                self.type_()
            ),
        }
    }

    // PORT: Go `reflect.Value.Int` (Int kinds).
    fn int(&self) -> i64 {
        match self {
            FieldValue::IndentStyle(v) => i64::from(v.0),
            FieldValue::OrganizeImportsSort(v) => i64::from(v.0),
            FieldValue::OrganizeImportsCaseFirst(v) => i64::from(v.0),
            FieldValue::OrganizeImportsTypeOrder(v) => i64::from(v.0),
            FieldValue::Int(v) => i64::from(*v),
            _ => panic!(
                "reflect: call of reflect.Value.Int on {:?} Value",
                self.type_()
            ),
        }
    }

    // PORT: Go `reflect.Value.String` (String kinds). The two
    // modulespecifiers enums are Go string types; these are their Go values.
    fn string(&self) -> String {
        match self {
            FieldValue::SemicolonPreference(v) => v.0.to_string(),
            FieldValue::QuotePreference(v) => v.0.to_string(),
            FieldValue::JsxAttributeCompletionStyle(v) => v.0.to_string(),
            FieldValue::IncludeInlayParameterNameHints(v) => v.0.to_string(),
            FieldValue::ImportModuleSpecifierPreference(v) => match v {
                ImportModuleSpecifierPreference::None => "",
                ImportModuleSpecifierPreference::Shortest => "shortest",
                ImportModuleSpecifierPreference::ProjectRelative => "project-relative",
                ImportModuleSpecifierPreference::Relative => "relative",
                ImportModuleSpecifierPreference::NonRelative => "non-relative",
            }
            .to_string(),
            FieldValue::ImportModuleSpecifierEndingPreference(v) => match v {
                ImportModuleSpecifierEndingPreference::None => "",
                ImportModuleSpecifierEndingPreference::Auto => "auto",
                ImportModuleSpecifierEndingPreference::Minimal => "minimal",
                ImportModuleSpecifierEndingPreference::Index => "index",
                ImportModuleSpecifierEndingPreference::Js => "js",
            }
            .to_string(),
            FieldValue::WorkspaceSymbolsScope(v) => v.0.to_string(),
            FieldValue::String(v) => v.clone(),
            _ => format!("<{:?} Value>", self.type_()),
        }
    }

    // PORT: Go `reflect.Value.IsZero`. An empty `Vec` stands for a nil slice
    // (see the file note).
    fn is_zero(&self) -> bool {
        match self {
            FieldValue::Tristate(v) => *v == Tristate::Unknown,
            FieldValue::IndentStyle(v) => v.0 == 0,
            FieldValue::SemicolonPreference(v) => v.0.is_empty(),
            FieldValue::QuotePreference(v) => v.0.is_empty(),
            FieldValue::JsxAttributeCompletionStyle(v) => v.0.is_empty(),
            FieldValue::IncludeInlayParameterNameHints(v) => v.0.is_empty(),
            FieldValue::OrganizeImportsSort(v) => v.0 == 0,
            FieldValue::OrganizeImportsCollation(v) => !v.0,
            FieldValue::OrganizeImportsCaseFirst(v) => v.0 == 0,
            FieldValue::OrganizeImportsTypeOrder(v) => v.0 == 0,
            FieldValue::ImportModuleSpecifierPreference(v) => {
                *v == ImportModuleSpecifierPreference::None
            }
            FieldValue::ImportModuleSpecifierEndingPreference(v) => {
                *v == ImportModuleSpecifierEndingPreference::None
            }
            FieldValue::WorkspaceSymbolsScope(v) => v.0.is_empty(),
            FieldValue::Int(v) => *v == 0,
            FieldValue::String(v) => v.is_empty(),
            FieldValue::Bool(v) => !*v,
            FieldValue::StringSlice(v) => v.is_empty(),
        }
    }
}

// Go: ls/lsutil/userpreferences.go:300 typeParsers (at 673a5f17d713; removed by ts#64554, which generates the per-field code in ls/lsutil/userpreferences_generated.go)
// typeParsers maps reflect.Type to a function that parses a value into that type.
// PORT: the Go map is a match on the field type.
fn type_parsers(t: FieldType) -> Option<fn(&LspAny) -> FieldValue> {
    match t {
        FieldType::Tristate => Some(|val: &LspAny| -> FieldValue {
            if let LspAny::Bool(b) = val {
                if *b {
                    return FieldValue::Tristate(Tristate::True);
                }
                return FieldValue::Tristate(Tristate::False);
            }
            FieldValue::Tristate(Tristate::Unknown)
        }),
        FieldType::IndentStyle => {
            Some(|val: &LspAny| -> FieldValue { FieldValue::IndentStyle(parse_indent_style(val)) })
        }
        FieldType::SemicolonPreference => Some(|val: &LspAny| -> FieldValue {
            FieldValue::SemicolonPreference(parse_semicolon_preference(val))
        }),
        FieldType::QuotePreference => Some(|val: &LspAny| -> FieldValue {
            if let LspAny::String(s) = val {
                match strings_to_lower(s).as_str() {
                    "auto" => return FieldValue::QuotePreference(QuotePreference::AUTO),
                    "double" => return FieldValue::QuotePreference(QuotePreference::DOUBLE),
                    "single" => return FieldValue::QuotePreference(QuotePreference::SINGLE),
                    _ => {}
                }
            }
            FieldValue::QuotePreference(QuotePreference::UNKNOWN)
        }),
        // Go: ls/lsutil/userpreferences_generated.go:371 parsePreferenceWorkspaceSymbolsScope
        // (ts#64554): case-insensitive, and any other value is "".
        FieldType::WorkspaceSymbolsScope => Some(|val: &LspAny| -> FieldValue {
            if let LspAny::String(s) = val {
                match strings_to_lower(s).as_str() {
                    "allopenprojects" => {
                        return FieldValue::WorkspaceSymbolsScope(
                            WorkspaceSymbolsScope::ALL_OPEN_PROJECTS,
                        );
                    }
                    "currentproject" => {
                        return FieldValue::WorkspaceSymbolsScope(
                            WorkspaceSymbolsScope::CURRENT_PROJECT,
                        );
                    }
                    _ => {}
                }
            }
            FieldValue::WorkspaceSymbolsScope(WorkspaceSymbolsScope(Cow::Borrowed("")))
        }),
        FieldType::JsxAttributeCompletionStyle => Some(|val: &LspAny| -> FieldValue {
            if let LspAny::String(s) = val {
                match strings_to_lower(s).as_str() {
                    "braces" => {
                        return FieldValue::JsxAttributeCompletionStyle(
                            JsxAttributeCompletionStyle::BRACES,
                        );
                    }
                    "none" => {
                        return FieldValue::JsxAttributeCompletionStyle(
                            JsxAttributeCompletionStyle::NONE,
                        );
                    }
                    _ => {}
                }
            }
            FieldValue::JsxAttributeCompletionStyle(JsxAttributeCompletionStyle::AUTO)
        }),
        // Go: ls/lsutil/userpreferences_generated.go:179 parsePreferenceIncludeInlayParameterNameHints
        // (ts#64554): case-insensitive.
        FieldType::IncludeInlayParameterNameHints => Some(|val: &LspAny| -> FieldValue {
            if let LspAny::String(s) = val {
                match strings_to_lower(s).as_str() {
                    "all" => {
                        return FieldValue::IncludeInlayParameterNameHints(
                            IncludeInlayParameterNameHints::ALL,
                        );
                    }
                    "literals" => {
                        return FieldValue::IncludeInlayParameterNameHints(
                            IncludeInlayParameterNameHints::LITERALS,
                        );
                    }
                    _ => {}
                }
            }
            FieldValue::IncludeInlayParameterNameHints(IncludeInlayParameterNameHints::NONE)
        }),
        FieldType::OrganizeImportsSort => Some(|val: &LspAny| -> FieldValue {
            if let LspAny::String(s) = val {
                match strings_to_lower(s).as_str() {
                    "ordinal" => {
                        return FieldValue::OrganizeImportsSort(OrganizeImportsSort::ORDINAL);
                    }
                    "ordinalignorecase" => {
                        return FieldValue::OrganizeImportsSort(
                            OrganizeImportsSort::ORDINAL_IGNORE_CASE,
                        );
                    }
                    "natural" => {
                        return FieldValue::OrganizeImportsSort(OrganizeImportsSort::NATURAL);
                    }
                    "naturalignorecase" => {
                        return FieldValue::OrganizeImportsSort(
                            OrganizeImportsSort::NATURAL_IGNORE_CASE,
                        );
                    }
                    _ => {}
                }
            }
            FieldValue::OrganizeImportsSort(OrganizeImportsSort::AUTO)
        }),
        FieldType::OrganizeImportsCollation => Some(|val: &LspAny| -> FieldValue {
            if let LspAny::String(s) = val
                && strings_to_lower(s) == "unicode"
            {
                return FieldValue::OrganizeImportsCollation(OrganizeImportsCollation::UNICODE);
            }
            FieldValue::OrganizeImportsCollation(OrganizeImportsCollation::ORDINAL)
        }),
        // Go: ls/lsutil/userpreferences_generated.go:227 parsePreferenceOrganizeImportsCaseFirst
        // (ts#64554): case-insensitive.
        FieldType::OrganizeImportsCaseFirst => Some(|val: &LspAny| -> FieldValue {
            if let LspAny::String(s) = val {
                match strings_to_lower(s).as_str() {
                    "lower" => {
                        return FieldValue::OrganizeImportsCaseFirst(
                            OrganizeImportsCaseFirst::LOWER,
                        );
                    }
                    "upper" => {
                        return FieldValue::OrganizeImportsCaseFirst(
                            OrganizeImportsCaseFirst::UPPER,
                        );
                    }
                    _ => {}
                }
            }
            FieldValue::OrganizeImportsCaseFirst(OrganizeImportsCaseFirst::FALSE)
        }),
        // Go: ls/lsutil/userpreferences_generated.go:312 parsePreferenceOrganizeImportsTypeOrder
        // (ts#64554): case-insensitive.
        FieldType::OrganizeImportsTypeOrder => Some(|val: &LspAny| -> FieldValue {
            if let LspAny::String(s) = val {
                match strings_to_lower(s).as_str() {
                    "last" => {
                        return FieldValue::OrganizeImportsTypeOrder(
                            OrganizeImportsTypeOrder::LAST,
                        );
                    }
                    "inline" => {
                        return FieldValue::OrganizeImportsTypeOrder(
                            OrganizeImportsTypeOrder::INLINE,
                        );
                    }
                    "first" => {
                        return FieldValue::OrganizeImportsTypeOrder(
                            OrganizeImportsTypeOrder::FIRST,
                        );
                    }
                    _ => {}
                }
            }
            FieldValue::OrganizeImportsTypeOrder(OrganizeImportsTypeOrder::AUTO)
        }),
        FieldType::ImportModuleSpecifierPreference => Some(|val: &LspAny| -> FieldValue {
            if let LspAny::String(s) = val {
                match strings_to_lower(s).as_str() {
                    "project-relative" => {
                        return FieldValue::ImportModuleSpecifierPreference(
                            ImportModuleSpecifierPreference::ProjectRelative,
                        );
                    }
                    "relative" => {
                        return FieldValue::ImportModuleSpecifierPreference(
                            ImportModuleSpecifierPreference::Relative,
                        );
                    }
                    "non-relative" => {
                        return FieldValue::ImportModuleSpecifierPreference(
                            ImportModuleSpecifierPreference::NonRelative,
                        );
                    }
                    _ => {}
                }
            }
            FieldValue::ImportModuleSpecifierPreference(ImportModuleSpecifierPreference::Shortest)
        }),
        FieldType::ImportModuleSpecifierEndingPreference => Some(|val: &LspAny| -> FieldValue {
            if let LspAny::String(s) = val {
                match strings_to_lower(s).as_str() {
                    "minimal" => {
                        return FieldValue::ImportModuleSpecifierEndingPreference(
                            ImportModuleSpecifierEndingPreference::Minimal,
                        );
                    }
                    "index" => {
                        return FieldValue::ImportModuleSpecifierEndingPreference(
                            ImportModuleSpecifierEndingPreference::Index,
                        );
                    }
                    "js" => {
                        return FieldValue::ImportModuleSpecifierEndingPreference(
                            ImportModuleSpecifierEndingPreference::Js,
                        );
                    }
                    _ => {}
                }
            }
            FieldValue::ImportModuleSpecifierEndingPreference(
                ImportModuleSpecifierEndingPreference::Auto,
            )
        }),
        _ => None,
    }
}

// Go: ls/lsutil/userpreferences.go:426 typeSerializers (at 673a5f17d713; removed by ts#64554, which generates the per-field code in ls/lsutil/userpreferences_generated.go)
// typeSerializers maps reflect.Type to a function that serializes a value of that type.
// For types which do not serialize as-is (tristate, enums, etc).
// PORT: the Go map is a match on the field type. The Go type assertion
// `val.(T)` panics on another type, as the `let .. else` does here.
fn type_serializers(t: FieldType) -> Option<fn(&FieldValue) -> LspAny> {
    match t {
        FieldType::Tristate => Some(|val: &FieldValue| -> LspAny {
            let FieldValue::Tristate(v) = val else {
                panic!("interface conversion: interface {{}} is not core.Tristate")
            };
            match *v {
                Tristate::True => LspAny::Bool(true),
                Tristate::False => LspAny::Bool(false),
                _ => LspAny::Null,
            }
        }),
        FieldType::OrganizeImportsSort => Some(|val: &FieldValue| -> LspAny {
            let FieldValue::OrganizeImportsSort(v) = val else {
                panic!("interface conversion: interface {{}} is not lsutil.OrganizeImportsSort")
            };
            match *v {
                OrganizeImportsSort::ORDINAL => LspAny::String("ordinal".to_string()),
                OrganizeImportsSort::ORDINAL_IGNORE_CASE => {
                    LspAny::String("ordinalIgnoreCase".to_string())
                }
                OrganizeImportsSort::NATURAL => LspAny::String("natural".to_string()),
                OrganizeImportsSort::NATURAL_IGNORE_CASE => {
                    LspAny::String("naturalIgnoreCase".to_string())
                }
                _ => LspAny::String("auto".to_string()),
            }
        }),
        FieldType::OrganizeImportsCollation => Some(|val: &FieldValue| -> LspAny {
            let FieldValue::OrganizeImportsCollation(v) = val else {
                panic!(
                    "interface conversion: interface {{}} is not lsutil.OrganizeImportsCollation"
                )
            };
            if *v == OrganizeImportsCollation::UNICODE {
                return LspAny::String("unicode".to_string());
            }
            LspAny::String("ordinal".to_string())
        }),
        FieldType::OrganizeImportsCaseFirst => Some(|val: &FieldValue| -> LspAny {
            let FieldValue::OrganizeImportsCaseFirst(v) = val else {
                panic!(
                    "interface conversion: interface {{}} is not lsutil.OrganizeImportsCaseFirst"
                )
            };
            match *v {
                OrganizeImportsCaseFirst::LOWER => LspAny::String("lower".to_string()),
                OrganizeImportsCaseFirst::UPPER => LspAny::String("upper".to_string()),
                _ => LspAny::String("default".to_string()),
            }
        }),
        FieldType::OrganizeImportsTypeOrder => Some(|val: &FieldValue| -> LspAny {
            let FieldValue::OrganizeImportsTypeOrder(v) = val else {
                panic!(
                    "interface conversion: interface {{}} is not lsutil.OrganizeImportsTypeOrder"
                )
            };
            match *v {
                OrganizeImportsTypeOrder::LAST => LspAny::String("last".to_string()),
                OrganizeImportsTypeOrder::INLINE => LspAny::String("inline".to_string()),
                OrganizeImportsTypeOrder::FIRST => LspAny::String("first".to_string()),
                _ => LspAny::String("auto".to_string()),
            }
        }),
        // These enums distinguish an unset zero value (e.g. "") from their effective
        // default (e.g. "auto"): the parser promotes unset/unknown input to the
        // non-zero default. Plain string serialization would therefore write "" for
        // an unset field and the parser would read it back as the non-zero default,
        // breaking round-tripping. Mirror the core.Tristate serializer above and omit
        // the unset value (return nil) so it decodes back to the zero value. (Enums
        // whose default already is their zero value, like the OrganizeImports* ones,
        // round-trip without this.)
        //
        // TODO: These three are the only parsers whose fallback is a non-zero value;
        // every other parser returns its zero value as the fallback. They should be
        // made consistent: change the parser fallback to return the zero value and
        // remove this serializer (relying on the default string serialization, which
        // already omits ""). The consumer must then treat the zero value as the
        // effective default. The two module-specifier enums are safe to convert (all
        // read sites already treat the "" zero identically to the promoted default).
        FieldType::JsxAttributeCompletionStyle => Some(|val: &FieldValue| -> LspAny {
            // TODO: make consistent with other enums (see note above). Unlike the
            // module-specifier enums, the consumer in completions.go distinguishes
            // JsxAttributeCompletionStyleUnknown from ...Auto, so converting this one
            // requires updating that consumer to treat the zero value as "auto".
            let FieldValue::JsxAttributeCompletionStyle(v) = val else {
                panic!(
                    "interface conversion: interface {{}} is not lsutil.JsxAttributeCompletionStyle"
                )
            };
            if *v != JsxAttributeCompletionStyle::UNKNOWN {
                return LspAny::String(v.0.to_string());
            }
            LspAny::Null
        }),
        FieldType::ImportModuleSpecifierPreference => Some(|val: &FieldValue| -> LspAny {
            // TODO: make consistent with other enums (see note above): have the parser
            // return the zero value (None) as its fallback and drop this serializer.
            let FieldValue::ImportModuleSpecifierPreference(_) = val else {
                panic!(
                    "interface conversion: interface {{}} is not modulespecifiers.ImportModuleSpecifierPreference"
                )
            };
            let v = val.string();
            if !v.is_empty() {
                return LspAny::String(v);
            }
            LspAny::Null
        }),
        FieldType::ImportModuleSpecifierEndingPreference => Some(|val: &FieldValue| -> LspAny {
            // TODO: make consistent with other enums (see note above): have the parser
            // return the zero value (None) as its fallback and drop this serializer.
            let FieldValue::ImportModuleSpecifierEndingPreference(_) = val else {
                panic!(
                    "interface conversion: interface {{}} is not modulespecifiers.ImportModuleSpecifierEndingPreference"
                )
            };
            let v = val.string();
            if !v.is_empty() {
                return LspAny::String(v);
            }
            LspAny::Null
        }),
        _ => None,
    }
}

// Go: ls/lsutil/userpreferences.go:525 configPathParsers (at 673a5f17d713; removed by ts#64554, which generates the per-field code in ls/lsutil/userpreferences_generated.go)
// configPathParsers provides field-specific config value parsers that override the default
// type-based parser when the VS Code config value format differs from the Go field type.
// PORT: the Go map is a match on the config path.
fn config_path_parsers(path: &str) -> Option<fn(&LspAny) -> FieldValue> {
    match path {
        // VS Code sends caseSensitivity as a string ("auto"/"caseSensitive"/"caseInsensitive"),
        // but OrganizeImportsIgnoreCase is a core.Tristate.
        "preferences.organizeImports.caseSensitivity" => Some(|val: &LspAny| -> FieldValue {
            if let LspAny::String(s) = val {
                match strings_to_lower(s).as_str() {
                    "caseinsensitive" => return FieldValue::Tristate(Tristate::True),
                    "casesensitive" => return FieldValue::Tristate(Tristate::False),
                    _ => {}
                }
            }
            if let LspAny::Bool(b) = val {
                if *b {
                    return FieldValue::Tristate(Tristate::True);
                }
                return FieldValue::Tristate(Tristate::False);
            }
            FieldValue::Tristate(Tristate::Unknown)
        }),
        _ => None,
    }
}

// PORT: one Go struct field as `reflect` sees it: the Go field name, the Go
// `raw`, `config` and `fallbackConfig` tags, and accessors that stand for the
// Go field index path.
struct StructField {
    name: &'static str,
    raw_tag: &'static str,
    config_tag: &'static str,
    fallback_config_tag: &'static str,
    get: fn(&UserPreferences) -> FieldValue,
    set: fn(&mut UserPreferences, FieldValue),
}

// PORT: builds one `StructField`. `$variant` is the Go field type and the
// path is the field inside `UserPreferences`. `set` panics like Go
// `reflect.Value.Set` when the value has another type. The form with
// `fallback:` sets the Go `fallbackConfig` tag.
macro_rules! pref_field {
    ($name:literal, $raw:literal, $config:literal, $variant:ident, $($path:ident).+) => {
        pref_field!($name, $raw, $config, fallback: "", $variant, $($path).+)
    };
    ($name:literal, $raw:literal, $config:literal, fallback: $fallback:literal, $variant:ident, $($path:ident).+) => {
        StructField {
            name: $name,
            raw_tag: $raw,
            config_tag: $config,
            fallback_config_tag: $fallback,
            get: |p| FieldValue::$variant(p.$($path).+.clone()),
            set: |p, v| match v {
                FieldValue::$variant(x) => p.$($path).+ = x,
                other => panic!(
                    "reflect.Set: value of type {:?} is not assignable to type {}",
                    other.type_(),
                    stringify!($variant)
                ),
            },
        }
    };
}

// PORT: the fields of `UserPreferences` in Go field order. Go
// `collectFieldInfos` recurses into the untagged struct fields
// (`FormatCodeSettings`, its embedded `EditorSettings`, `InlayHints`,
// `CodeLens`) at their place in the order; they are listed inline here.
static USER_PREFERENCES_FIELDS: &[StructField] = &[
    // FormatCodeSettings.EditorSettings
    pref_field!(
        "BaseIndentSize",
        "baseIndentSize",
        "format.baseIndentSize",
        Int,
        format_code_settings.editor_settings.base_indent_size
    ),
    pref_field!(
        "IndentSize",
        "indentSize",
        "format.indentSize",
        Int,
        format_code_settings.editor_settings.indent_size
    ),
    pref_field!(
        "TabSize",
        "tabSize",
        "format.tabSize",
        Int,
        format_code_settings.editor_settings.tab_size
    ),
    pref_field!(
        "NewLineCharacter",
        "newLineCharacter",
        "format.newLineCharacter",
        String,
        format_code_settings.editor_settings.new_line_character
    ),
    pref_field!(
        "ConvertTabsToSpaces",
        "convertTabsToSpaces",
        "format.convertTabsToSpaces",
        Tristate,
        format_code_settings.editor_settings.convert_tabs_to_spaces
    ),
    pref_field!(
        "IndentStyle",
        "indentStyle",
        "format.indentStyle",
        IndentStyle,
        format_code_settings.editor_settings.indent_style
    ),
    pref_field!(
        "TrimTrailingWhitespace",
        "trimTrailingWhitespace",
        "format.trimTrailingWhitespace",
        Tristate,
        format_code_settings
            .editor_settings
            .trim_trailing_whitespace
    ),
    // FormatCodeSettings
    pref_field!(
        "InsertSpaceAfterCommaDelimiter",
        "insertSpaceAfterCommaDelimiter",
        "format.insertSpaceAfterCommaDelimiter",
        Tristate,
        format_code_settings.insert_space_after_comma_delimiter
    ),
    pref_field!(
        "InsertSpaceAfterSemicolonInForStatements",
        "insertSpaceAfterSemicolonInForStatements",
        "format.insertSpaceAfterSemicolonInForStatements",
        Tristate,
        format_code_settings.insert_space_after_semicolon_in_for_statements
    ),
    pref_field!(
        "InsertSpaceBeforeAndAfterBinaryOperators",
        "insertSpaceBeforeAndAfterBinaryOperators",
        "format.insertSpaceBeforeAndAfterBinaryOperators",
        Tristate,
        format_code_settings.insert_space_before_and_after_binary_operators
    ),
    pref_field!(
        "InsertSpaceAfterConstructor",
        "insertSpaceAfterConstructor",
        "format.insertSpaceAfterConstructor",
        Tristate,
        format_code_settings.insert_space_after_constructor
    ),
    pref_field!(
        "InsertSpaceAfterKeywordsInControlFlowStatements",
        "insertSpaceAfterKeywordsInControlFlowStatements",
        "format.insertSpaceAfterKeywordsInControlFlowStatements",
        Tristate,
        format_code_settings.insert_space_after_keywords_in_control_flow_statements
    ),
    pref_field!(
        "InsertSpaceAfterFunctionKeywordForAnonymousFunctions",
        "insertSpaceAfterFunctionKeywordForAnonymousFunctions",
        "format.insertSpaceAfterFunctionKeywordForAnonymousFunctions",
        Tristate,
        format_code_settings.insert_space_after_function_keyword_for_anonymous_functions
    ),
    pref_field!(
        "InsertSpaceAfterOpeningAndBeforeClosingNonemptyParenthesis",
        "insertSpaceAfterOpeningAndBeforeClosingNonemptyParenthesis",
        "format.insertSpaceAfterOpeningAndBeforeClosingNonemptyParenthesis",
        Tristate,
        format_code_settings.insert_space_after_opening_and_before_closing_nonempty_parenthesis
    ),
    pref_field!(
        "InsertSpaceAfterOpeningAndBeforeClosingNonemptyBrackets",
        "insertSpaceAfterOpeningAndBeforeClosingNonemptyBrackets",
        "format.insertSpaceAfterOpeningAndBeforeClosingNonemptyBrackets",
        Tristate,
        format_code_settings.insert_space_after_opening_and_before_closing_nonempty_brackets
    ),
    pref_field!(
        "InsertSpaceAfterOpeningAndBeforeClosingNonemptyBraces",
        "insertSpaceAfterOpeningAndBeforeClosingNonemptyBraces",
        "format.insertSpaceAfterOpeningAndBeforeClosingNonemptyBraces",
        Tristate,
        format_code_settings.insert_space_after_opening_and_before_closing_nonempty_braces
    ),
    pref_field!(
        "InsertSpaceAfterOpeningAndBeforeClosingEmptyBraces",
        "insertSpaceAfterOpeningAndBeforeClosingEmptyBraces",
        "format.insertSpaceAfterOpeningAndBeforeClosingEmptyBraces",
        Tristate,
        format_code_settings.insert_space_after_opening_and_before_closing_empty_braces
    ),
    pref_field!(
        "InsertSpaceAfterOpeningAndBeforeClosingTemplateStringBraces",
        "insertSpaceAfterOpeningAndBeforeClosingTemplateStringBraces",
        "format.insertSpaceAfterOpeningAndBeforeClosingTemplateStringBraces",
        Tristate,
        format_code_settings.insert_space_after_opening_and_before_closing_template_string_braces
    ),
    pref_field!(
        "InsertSpaceAfterOpeningAndBeforeClosingJsxExpressionBraces",
        "insertSpaceAfterOpeningAndBeforeClosingJsxExpressionBraces",
        "format.insertSpaceAfterOpeningAndBeforeClosingJsxExpressionBraces",
        Tristate,
        format_code_settings.insert_space_after_opening_and_before_closing_jsx_expression_braces
    ),
    pref_field!(
        "InsertSpaceAfterTypeAssertion",
        "insertSpaceAfterTypeAssertion",
        "format.insertSpaceAfterTypeAssertion",
        Tristate,
        format_code_settings.insert_space_after_type_assertion
    ),
    pref_field!(
        "InsertSpaceBeforeFunctionParenthesis",
        "insertSpaceBeforeFunctionParenthesis",
        "format.insertSpaceBeforeFunctionParenthesis",
        Tristate,
        format_code_settings.insert_space_before_function_parenthesis
    ),
    pref_field!(
        "PlaceOpenBraceOnNewLineForFunctions",
        "placeOpenBraceOnNewLineForFunctions",
        "format.placeOpenBraceOnNewLineForFunctions",
        Tristate,
        format_code_settings.place_open_brace_on_new_line_for_functions
    ),
    pref_field!(
        "PlaceOpenBraceOnNewLineForControlBlocks",
        "placeOpenBraceOnNewLineForControlBlocks",
        "format.placeOpenBraceOnNewLineForControlBlocks",
        Tristate,
        format_code_settings.place_open_brace_on_new_line_for_control_blocks
    ),
    pref_field!(
        "InsertSpaceBeforeTypeAnnotation",
        "insertSpaceBeforeTypeAnnotation",
        "format.insertSpaceBeforeTypeAnnotation",
        Tristate,
        format_code_settings.insert_space_before_type_annotation
    ),
    pref_field!(
        "IndentMultiLineObjectLiteralBeginningOnBlankLine",
        "indentMultiLineObjectLiteralBeginningOnBlankLine",
        "format.indentMultiLineObjectLiteralBeginningOnBlankLine",
        Tristate,
        format_code_settings.indent_multi_line_object_literal_beginning_on_blank_line
    ),
    pref_field!(
        "Semicolons",
        "semicolons",
        "format.semicolons",
        SemicolonPreference,
        format_code_settings.semicolons
    ),
    pref_field!(
        "IndentSwitchCase",
        "indentSwitchCase",
        "format.indentSwitchCase",
        Tristate,
        format_code_settings.indent_switch_case
    ),
    // UserPreferences
    pref_field!(
        "QuotePreference",
        "quotePreference",
        "preferences.quoteStyle",
        QuotePreference,
        quote_preference
    ),
    pref_field!(
        "LazyConfiguredProjectsFromExternalProject",
        "lazyConfiguredProjectsFromExternalProject",
        "",
        Tristate,
        lazy_configured_projects_from_external_project
    ),
    pref_field!(
        "MaximumHoverLength",
        "maximumHoverLength",
        "",
        Int,
        maximum_hover_length
    ),
    pref_field!(
        "IncludeCompletionsForModuleExports",
        "includeCompletionsForModuleExports",
        "suggest.autoImports",
        Tristate,
        include_completions_for_module_exports
    ),
    pref_field!(
        "IncludeCompletionsForImportStatements",
        "includeCompletionsForImportStatements",
        "suggest.includeCompletionsForImportStatements",
        Tristate,
        include_completions_for_import_statements
    ),
    pref_field!(
        "IncludeAutomaticOptionalChainCompletions",
        "includeAutomaticOptionalChainCompletions",
        "suggest.includeAutomaticOptionalChainCompletions",
        Tristate,
        include_automatic_optional_chain_completions
    ),
    pref_field!(
        "IncludeCompletionsWithClassMemberSnippets",
        "includeCompletionsWithClassMemberSnippets",
        "suggest.classMemberSnippets.enabled",
        Tristate,
        include_completions_with_class_member_snippets
    ),
    pref_field!(
        "IncludeCompletionsWithObjectLiteralMethodSnippets",
        "includeCompletionsWithObjectLiteralMethodSnippets",
        "suggest.objectLiteralMethodSnippets.enabled",
        Tristate,
        include_completions_with_object_literal_method_snippets
    ),
    pref_field!(
        "JsxAttributeCompletionStyle",
        "jsxAttributeCompletionStyle",
        "preferences.jsxAttributeCompletionStyle",
        JsxAttributeCompletionStyle,
        jsx_attribute_completion_style
    ),
    pref_field!(
        "EnableAutoClosingTags",
        "autoClosingTags",
        "autoClosingTags.enabled",
        fallback: "autoClosingTags",
        Tristate,
        enable_auto_closing_tags
    ),
    pref_field!(
        "EnableJSDocCompletions",
        "completeJSDocs",
        "suggest.jsdoc.enabled",
        fallback: "suggest.completeJSDocs",
        Tristate,
        enable_js_doc_completions
    ),
    pref_field!(
        "GenerateReturnInDocTemplate",
        "generateReturnInDocTemplate",
        "suggest.jsdoc.generateReturns",
        Tristate,
        generate_return_in_doc_template
    ),
    pref_field!(
        "ImportModuleSpecifierPreference",
        "importModuleSpecifierPreference",
        "preferences.importModuleSpecifier",
        ImportModuleSpecifierPreference,
        import_module_specifier_preference
    ),
    pref_field!(
        "ImportModuleSpecifierEnding",
        "importModuleSpecifierEnding",
        "preferences.importModuleSpecifierEnding",
        ImportModuleSpecifierEndingPreference,
        import_module_specifier_ending
    ),
    pref_field!(
        "AutoImportSpecifierExcludeRegexes",
        "autoImportSpecifierExcludeRegexes",
        "preferences.autoImportSpecifierExcludeRegexes",
        StringSlice,
        auto_import_specifier_exclude_regexes
    ),
    pref_field!(
        "AutoImportFileExcludePatterns",
        "autoImportFileExcludePatterns",
        "preferences.autoImportFileExcludePatterns",
        StringSlice,
        auto_import_file_exclude_patterns
    ),
    pref_field!(
        "AutoImportEntrypointDirectorySearch",
        "autoImportEntrypointDirectorySearch",
        "preferences.autoImportEntrypointDirectorySearch",
        Tristate,
        auto_import_entrypoint_directory_search
    ),
    pref_field!(
        "PreferTypeOnlyAutoImports",
        "preferTypeOnlyAutoImports",
        "preferences.preferTypeOnlyAutoImports",
        Tristate,
        prefer_type_only_auto_imports
    ),
    pref_field!(
        "OrganizeImportsSort",
        "organizeImportsSort",
        "preferences.organizeImports.sort",
        OrganizeImportsSort,
        organize_imports_sort
    ),
    pref_field!(
        "OrganizeImportsIgnoreCase",
        "organizeImportsIgnoreCase",
        "preferences.organizeImports.caseSensitivity",
        Tristate,
        organize_imports_ignore_case
    ),
    pref_field!(
        "OrganizeImportsCollation",
        "organizeImportsCollation",
        "preferences.organizeImports.unicodeCollation",
        OrganizeImportsCollation,
        organize_imports_collation
    ),
    pref_field!(
        "OrganizeImportsLocale",
        "organizeImportsLocale",
        "preferences.organizeImports.locale",
        String,
        organize_imports_locale
    ),
    pref_field!(
        "OrganizeImportsNumericCollation",
        "organizeImportsNumericCollation",
        "preferences.organizeImports.numericCollation",
        Tristate,
        organize_imports_numeric_collation
    ),
    pref_field!(
        "OrganizeImportsAccentCollation",
        "organizeImportsAccentCollation",
        "preferences.organizeImports.accentCollation",
        Tristate,
        organize_imports_accent_collation
    ),
    pref_field!(
        "OrganizeImportsCaseFirst",
        "organizeImportsCaseFirst",
        "preferences.organizeImports.caseFirst",
        OrganizeImportsCaseFirst,
        organize_imports_case_first
    ),
    pref_field!(
        "OrganizeImportsTypeOrder",
        "organizeImportsTypeOrder",
        "preferences.organizeImports.typeOrder",
        OrganizeImportsTypeOrder,
        organize_imports_type_order
    ),
    pref_field!(
        "AllowTextChangesInNewFiles",
        "allowTextChangesInNewFiles",
        "",
        Tristate,
        allow_text_changes_in_new_files
    ),
    pref_field!(
        "UseAliasesForRename",
        "providePrefixAndSuffixTextForRename",
        "preferences.useAliasesForRenames",
        Tristate,
        use_aliases_for_rename
    ),
    pref_field!(
        "AllowRenameOfImportPath",
        "allowRenameOfImportPath",
        "",
        Tristate,
        allow_rename_of_import_path
    ),
    pref_field!(
        "ProvideRefactorNotApplicableReason",
        "provideRefactorNotApplicableReason",
        "",
        Tristate,
        provide_refactor_not_applicable_reason
    ),
    // InlayHints
    pref_field!(
        "IncludeInlayParameterNameHints",
        "includeInlayParameterNameHints",
        "inlayHints.parameterNames.enabled",
        IncludeInlayParameterNameHints,
        inlay_hints.include_inlay_parameter_name_hints
    ),
    pref_field!(
        "IncludeInlayParameterNameHintsWhenArgumentMatchesName",
        "includeInlayParameterNameHintsWhenArgumentMatchesName",
        "inlayHints.parameterNames.suppressWhenArgumentMatchesName,invert",
        Tristate,
        inlay_hints.include_inlay_parameter_name_hints_when_argument_matches_name
    ),
    pref_field!(
        "IncludeInlayFunctionParameterTypeHints",
        "includeInlayFunctionParameterTypeHints",
        "inlayHints.parameterTypes.enabled",
        Tristate,
        inlay_hints.include_inlay_function_parameter_type_hints
    ),
    pref_field!(
        "IncludeInlayVariableTypeHints",
        "includeInlayVariableTypeHints",
        "inlayHints.variableTypes.enabled",
        Tristate,
        inlay_hints.include_inlay_variable_type_hints
    ),
    pref_field!(
        "IncludeInlayVariableTypeHintsWhenTypeMatchesName",
        "includeInlayVariableTypeHintsWhenTypeMatchesName",
        "inlayHints.variableTypes.suppressWhenTypeMatchesName,invert",
        Tristate,
        inlay_hints.include_inlay_variable_type_hints_when_type_matches_name
    ),
    pref_field!(
        "IncludeInlayPropertyDeclarationTypeHints",
        "includeInlayPropertyDeclarationTypeHints",
        "inlayHints.propertyDeclarationTypes.enabled",
        Tristate,
        inlay_hints.include_inlay_property_declaration_type_hints
    ),
    pref_field!(
        "IncludeInlayFunctionLikeReturnTypeHints",
        "includeInlayFunctionLikeReturnTypeHints",
        "inlayHints.functionLikeReturnTypes.enabled",
        Tristate,
        inlay_hints.include_inlay_function_like_return_type_hints
    ),
    pref_field!(
        "IncludeInlayEnumMemberValueHints",
        "includeInlayEnumMemberValueHints",
        "inlayHints.enumMemberValues.enabled",
        Tristate,
        inlay_hints.include_inlay_enum_member_value_hints
    ),
    // CodeLens
    pref_field!(
        "ReferencesCodeLensEnabled",
        "referencesCodeLensEnabled",
        "referencesCodeLens.enabled",
        Tristate,
        code_lens.references_code_lens_enabled
    ),
    pref_field!(
        "ImplementationsCodeLensEnabled",
        "implementationsCodeLensEnabled",
        "implementationsCodeLens.enabled",
        Tristate,
        code_lens.implementations_code_lens_enabled
    ),
    pref_field!(
        "ReferencesCodeLensShowOnAllFunctions",
        "referencesCodeLensShowOnAllFunctions",
        "referencesCodeLens.showOnAllFunctions",
        Tristate,
        code_lens.references_code_lens_show_on_all_functions
    ),
    pref_field!(
        "ImplementationsCodeLensShowOnInterfaceMethods",
        "implementationsCodeLensShowOnInterfaceMethods",
        "implementationsCodeLens.showOnInterfaceMethods",
        Tristate,
        code_lens.implementations_code_lens_show_on_interface_methods
    ),
    pref_field!(
        "ImplementationsCodeLensShowOnAllClassMethods",
        "implementationsCodeLensShowOnAllClassMethods",
        "implementationsCodeLens.showOnAllClassMethods",
        Tristate,
        code_lens.implementations_code_lens_show_on_all_class_methods
    ),
    // UserPreferences
    pref_field!(
        "PreferGoToSourceDefinition",
        "preferGoToSourceDefinition",
        "",
        Bool,
        prefer_go_to_source_definition
    ),
    pref_field!(
        "ExcludeLibrarySymbolsInNavTo",
        "excludeLibrarySymbolsInNavTo",
        "workspaceSymbols.excludeLibrarySymbols",
        Tristate,
        exclude_library_symbols_in_nav_to
    ),
    // ts#64554: the raw name `workspaceSymbolsScope` is new.
    pref_field!(
        "WorkspaceSymbolsScope",
        "workspaceSymbolsScope",
        "workspaceSymbols.scope",
        WorkspaceSymbolsScope,
        workspace_symbols_scope
    ),
    pref_field!(
        "EnableFormatting",
        "formatEnabled",
        "format.enabled",
        fallback: "format.enable",
        Tristate,
        enable_formatting
    ),
    pref_field!(
        "EnableValidation",
        "validateEnabled",
        "validate.enabled",
        fallback: "validate.enable",
        Tristate,
        enable_validation
    ),
    pref_field!(
        "DisableSuggestions",
        "disableSuggestions",
        "",
        Tristate,
        disable_suggestions
    ),
    pref_field!(
        "DisableLineTextInReferences",
        "disableLineTextInReferences",
        "",
        Tristate,
        disable_line_text_in_references
    ),
    pref_field!(
        "DisplayPartsForJSDoc",
        "displayPartsForJSDoc",
        "",
        Tristate,
        display_parts_for_js_doc
    ),
    pref_field!(
        "ReportStyleChecksAsWarnings",
        "reportStyleChecksAsWarnings",
        "reportStyleChecksAsWarnings",
        Tristate,
        report_style_checks_as_warnings
    ),
    // ts#64554: the raw name `locale` is new.
    pref_field!("Locale", "locale", "locale", String, locale),
    pref_field!(
        "DisableAutomaticTypeAcquisition",
        "disableAutomaticTypeAcquisition",
        "disableAutomaticTypeAcquisition",
        Tristate,
        disable_automatic_type_acquisition
    ),
    pref_field!(
        "AutomaticTypeAcquisitionEnabled",
        "automaticTypeAcquisitionEnabled",
        "tsserver.automaticTypeAcquisition.enabled",
        Tristate,
        automatic_type_acquisition_enabled
    ),
    pref_field!(
        "CustomConfigFileName",
        "customConfigFileName",
        "customConfigFileName",
        String,
        custom_config_file_name
    ),
];

// Go: ls/lsutil/userpreferences.go:547 fieldInfo (at 673a5f17d713; removed by ts#64554, which generates the per-field code in ls/lsutil/userpreferences_generated.go)
#[derive(Clone)]
struct FieldInfo {
    raw_name: &'static str, // raw name for unstable section lookup (e.g., "quotePreference")
    config_path: &'static str, // dotted path for config (e.g., "preferences.quoteStyle")
    fallback_config_paths: Vec<ConfigPathInfo>,
    // PORT: Go `fieldPath []int` is the index path to the field in the
    // struct. The field accessors stand for it.
    get: fn(&UserPreferences) -> FieldValue,
    set: fn(&mut UserPreferences, FieldValue),
    raw_invert: bool,    // whether to invert boolean values for raw name
    config_invert: bool, // whether to invert boolean values for config path
}

// Go: ls/lsutil/userpreferences.go:556 configPathInfo (at 673a5f17d713; removed by ts#64554, which generates the per-field code in ls/lsutil/userpreferences_generated.go)
#[derive(Clone, Copy)]
struct ConfigPathInfo {
    path: &'static str,
    invert: bool,
}

// Go: ls/lsutil/userpreferences.go:561 fieldInfoCache (at 673a5f17d713; removed by ts#64554, which generates the per-field code in ls/lsutil/userpreferences_generated.go)
static FIELD_INFO_CACHE: LazyLock<Vec<FieldInfo>> =
    LazyLock::new(|| collect_field_infos(USER_PREFERENCES_FIELDS));

// Go: ls/lsutil/userpreferences.go:566 unstableNameIndex (at 673a5f17d713; removed by ts#64554, which generates the per-field code in ls/lsutil/userpreferences_generated.go)
// unstableNameIndex maps raw names to fieldInfo index for unstable section lookup.
static UNSTABLE_NAME_INDEX: LazyLock<FxHashMap<&'static str, usize>> = LazyLock::new(|| {
    let infos = &*FIELD_INFO_CACHE;
    let mut index = FxHashMap::with_capacity_and_hasher(infos.len(), Default::default());
    for (i, info) in infos.iter().enumerate() {
        if !info.raw_name.is_empty() {
            index.insert(info.raw_name, i);
        }
    }
    index
});

// Go: ls/lsutil/userpreferences.go:577 collectFieldInfos (at 673a5f17d713; removed by ts#64554, which generates the per-field code in ls/lsutil/userpreferences_generated.go)
// PORT: Go takes a `reflect.Type` and an index path and recurses into
// untagged struct fields. The field table is already flat, so every entry
// is a tagged field; the Go panic for an untagged non-struct field stays.
fn collect_field_infos(fields: &'static [StructField]) -> Vec<FieldInfo> {
    let mut infos = Vec::new();
    for field in fields {
        let raw_tag = field.raw_tag;
        let config_tag = field.config_tag;

        if raw_tag.is_empty() && config_tag.is_empty() {
            crate::core::go_panic(format!(
                "raw or config tag required for field {}",
                field.name
            ));
        }

        let fallback_config_tag = field.fallback_config_tag;

        let mut info = FieldInfo {
            raw_name: "",
            config_path: "",
            fallback_config_paths: Vec::new(),
            get: field.get,
            set: field.set,
            raw_invert: false,
            config_invert: false,
        };

        // Parse raw tag: "name" or "name,invert"
        if !raw_tag.is_empty() {
            let parts: Vec<&'static str> = raw_tag.split(',').collect();
            info.raw_name = parts[0];
            for part in &parts[1..] {
                if *part == "invert" {
                    info.raw_invert = true;
                }
            }
        }

        // Parse config tag: "path.to.setting" or "path.to.setting,invert"
        if !config_tag.is_empty() {
            let config_path = parse_config_path_tag(config_tag);
            info.config_path = config_path.path;
            info.config_invert = config_path.invert;
        }
        if !fallback_config_tag.is_empty() {
            for tag in fallback_config_tag.split(';') {
                info.fallback_config_paths.push(parse_config_path_tag(tag));
            }
        }

        infos.push(info);
    }
    infos
}

// Go: ls/lsutil/userpreferences.go:628 parseConfigPathTag (at 673a5f17d713; removed by ts#64554, which generates the per-field code in ls/lsutil/userpreferences_generated.go)
fn parse_config_path_tag(tag: &'static str) -> ConfigPathInfo {
    let mut parts = tag.split(',');
    let mut info = ConfigPathInfo {
        path: parts.next().unwrap_or(""),
        invert: false,
    };
    for part in parts {
        if part == "invert" {
            info.invert = true;
        }
    }
    info
}

// Go: ls/lsutil/userpreferences.go:147 getNestedValue
// PORT: Go starts from `any(config)`. `current` is `None` only for that root
// map, before the first part is read.
fn get_nested_value(config: &IndexMap<String, LspAny>, path: &str) -> (LspAny, bool) {
    let parts = path.split('.');
    let mut current: Option<&LspAny> = None;
    for part in parts {
        let m = match current {
            None => config,
            Some(LspAny::Object(m)) => m,
            Some(_) => return (LspAny::Null, false),
        };
        match m.get(part) {
            Some(v) => current = Some(v),
            None => return (LspAny::Null, false),
        }
    }
    (current.cloned().unwrap_or(LspAny::Null), true)
}

// Go: ls/lsutil/userpreferences.go:162 setNestedValue
fn set_nested_value(config: &mut IndexMap<String, LspAny>, path: &str, value: LspAny) {
    let parts: Vec<&str> = path.split('.').collect();
    let mut current = config;
    for part in &parts[..parts.len() - 1] {
        if !matches!(current.get(*part), Some(LspAny::Object(_))) {
            current.insert(part.to_string(), LspAny::Object(IndexMap::new()));
        }
        let Some(LspAny::Object(next)) = current.get_mut(*part) else {
            unreachable!("the map was just set")
        };
        current = next;
    }
    current.insert(parts[parts.len() - 1].to_string(), value);
}

// Go: ls/lsutil/userpreferences.go:669 setRawFieldsFromConfig (at 673a5f17d713; removed by ts#64554, which generates the per-field code in ls/lsutil/userpreferences_generated.go)
// PORT: Go takes the struct as a `reflect.Value`; here it is `p`.
fn set_raw_fields_from_config(
    p: &mut UserPreferences,
    infos: &[FieldInfo],
    settings: &IndexMap<String, LspAny>,
) {
    let index = &*UNSTABLE_NAME_INDEX;
    // PORT: Go ranges over a map in random order. Each raw name sets its own
    // field, so the order does not change the result.
    for (name, value) in settings {
        if let Some(&idx) = index.get(name.as_str()) {
            let info = &infos[idx];
            let mut value = value.clone();
            if info.raw_invert
                && let LspAny::Bool(b) = value
            {
                value = LspAny::Bool(!b);
            }
            set_field_from_value(p, info, &value);
        }
    }
}

impl UserPreferences {
    // Go: ls/lsutil/userpreferences.go:674 (UserPreferences).withConfig (at 673a5f17d713; ts#64554 generates it: ls/lsutil/userpreferences_generated.go:637 UserPreferences.withConfig)
    // PORT: Go has a value receiver; this works on a copy of `self`.
    pub(crate) fn with_config(&self, config: &IndexMap<String, LspAny>) -> UserPreferences {
        let mut p = self.clone();
        let infos = &*FIELD_INFO_CACHE;

        // Raw UserPreferences can be provided directly, notably via LSP initializationOptions.
        set_raw_fields_from_config(&mut p, infos, config);

        // Process "unstable" section first - allows any field to be set by raw name.
        // This mirrors VS Code's behavior: { ...config.get('unstable'), ...stableOptions }
        // where stable options are spread after and take precedence.
        if let Some(LspAny::Object(unstable)) = config.get("unstable") {
            set_raw_fields_from_config(&mut p, infos, unstable);
        }

        // Process path-based config (VS Code style nested paths).
        // These run after unstable, so stable config values take precedence.
        for info in infos {
            if info.config_path.is_empty() {
                continue;
            }
            let mut config_path = ConfigPathInfo {
                path: info.config_path,
                invert: info.config_invert,
            };
            let (mut val, mut ok) = get_nested_value(config, config_path.path);
            if !ok {
                for fallback_config_path in &info.fallback_config_paths {
                    (val, ok) = get_nested_value(config, fallback_config_path.path);
                    if ok {
                        config_path = *fallback_config_path;
                        break;
                    }
                }
            }
            if !ok {
                continue;
            }

            if config_path.invert
                && let LspAny::Bool(b) = val
            {
                val = LspAny::Bool(!b);
            }
            if let Some(parser) = config_path_parsers(config_path.path) {
                (info.set)(&mut p, parser(&val));
                continue;
            }
            set_field_from_value(&mut p, info, &val);
        }

        // Validate CustomConfigFileName for path traversal
        if !p.custom_config_file_name.is_empty() {
            // PORT: Go `strings.TrimSpace` trims Unicode White_Space, as
            // `str::trim` does.
            let name = p.custom_config_file_name.trim().to_string();
            if name.contains(['/', '\\']) || name == ".." || name == "." {
                p.custom_config_file_name = String::new();
            } else {
                p.custom_config_file_name = name;
            }
        }

        p
    }
}

// Go: ls/lsutil/userpreferences.go:746 getFieldByPath (at 673a5f17d713; removed by ts#64554, which generates the per-field code in ls/lsutil/userpreferences_generated.go)
// PORT: not ported. `FieldInfo.get` and `FieldInfo.set` stand for the field
// that Go finds by its index path.

// Go: ls/lsutil/userpreferences.go:753 setFieldFromValue (at 673a5f17d713; removed by ts#64554, which generates the per-field code in ls/lsutil/userpreferences_generated.go)
// PORT: Go takes the field as a `reflect.Value`. Here the field is `info` on
// `p`. Go also accepts a Go `int` for an int field; an `LspAny` number is
// always the Go `float64` case. Go `int64(float64)` keeps 64 bits; the Rust
// field is `i32` (PORTING `int` -> `i32`), so a value outside the `i32` range
// differs from Go.
fn set_field_from_value(p: &mut UserPreferences, info: &FieldInfo, val: &LspAny) {
    if matches!(val, LspAny::Null) {
        return;
    }
    let field_type = (info.get)(p).type_();

    // Check custom parsers first (for types like Tristate, enums, etc.)
    if let Some(parser) = type_parsers(field_type) {
        (info.set)(p, parser(val));
        return;
    }

    match field_type.kind() {
        FieldKind::Bool => {
            if let LspAny::Bool(b) = val {
                (info.set)(p, FieldValue::Bool(*b));
            }
        }
        FieldKind::Int => {
            if let LspAny::Number(v) = val {
                (info.set)(p, FieldValue::Int(*v as i64 as i32));
            }
        }
        FieldKind::String => {
            if let LspAny::String(s) = val {
                // PORT: only plain `string` fields reach this point; every
                // string type has a parser.
                (info.set)(p, FieldValue::String(s.clone()));
            }
        }
        FieldKind::Slice => {
            if let LspAny::Array(arr) = val {
                let mut result: Vec<String> = Vec::with_capacity(arr.len());
                for item in arr {
                    if let LspAny::String(s) = item {
                        result.push(s.clone());
                    }
                }
                (info.set)(p, FieldValue::StringSlice(result));
            }
        }
        FieldKind::Uint8 => {}
    }
}

// Go: ls/lsutil/userpreferences.go:656 (*UserPreferences).MarshalJSONTo (at 673a5f17d713; ts#64554 generates it: ls/lsutil/userpreferences_generated.go:1053 UserPreferences.MarshalJSONTo)
impl MarshalerTo for UserPreferences {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        let mut config: IndexMap<String, LspAny> = IndexMap::new();

        for info in FIELD_INFO_CACHE.iter() {
            let field = (info.get)(self);

            let mut val = serialize_field(&field);
            if matches!(val, LspAny::Null) {
                continue;
            }

            // Prefer config path if available, otherwise use unstable section
            if !info.config_path.is_empty() {
                if info.config_invert
                    && let LspAny::Bool(b) = val
                {
                    val = LspAny::Bool(!b);
                }
                set_nested_value(&mut config, info.config_path, val);
            } else if !info.raw_name.is_empty() {
                if info.raw_invert
                    && let LspAny::Bool(b) = val
                {
                    val = LspAny::Bool(!b);
                }
                set_nested_value(&mut config, &format!("unstable.{}", info.raw_name), val);
            }
        }

        // PORT: Go marshals with `json.Deterministic(true)`, which writes the
        // keys of every `map[string]any` in `slices.Sort` order (bytewise).
        sort_keys_deterministic(&mut config);
        config.marshal_json_to(enc)
    }
}

// PORT: the Go `json.Deterministic(true)` map key order, applied to every
// nested object. `String` `Ord` is the bytewise Go `slices.Sort` order.
fn sort_keys_deterministic(m: &mut IndexMap<String, LspAny>) {
    m.sort_keys();
    for v in m.values_mut() {
        sort_any_keys_deterministic(v);
    }
}

fn sort_any_keys_deterministic(v: &mut LspAny) {
    match v {
        LspAny::Object(m) => sort_keys_deterministic(m),
        LspAny::Array(items) => {
            for item in items {
                sort_any_keys_deterministic(item);
            }
        }
        _ => {}
    }
}

// Go: ls/lsutil/userpreferences.go:826 serializeField (at 673a5f17d713; removed by ts#64554, which generates the per-field code in ls/lsutil/userpreferences_generated.go)
// PORT: Go `nil` is `LspAny::Null`. A Go `int` is written as a JSON number;
// `LspAny::Number` is an `f64`, which holds every `i32` exactly.
fn serialize_field(field: &FieldValue) -> LspAny {
    // Check custom serializers first (for types like Tristate, enums, etc.)
    if let Some(serializer) = type_serializers(field.type_()) {
        return serializer(field);
    }

    match field.type_().kind() {
        FieldKind::Bool => LspAny::Bool(field.bool_()),
        FieldKind::Int => {
            // Zero means "unset" for these preference fields. Omit it so a partial
            // config does not clobber defaults with zeros when round-tripped through
            // withConfig.
            let i = field.int();
            if i == 0 {
                return LspAny::Null;
            }
            LspAny::Number(i as f64)
        }
        FieldKind::String => {
            // Zero ("") means "unset"; omit it for the same reason as int above.
            let s = field.string();
            if s.is_empty() {
                return LspAny::Null;
            }
            LspAny::String(s)
        }
        FieldKind::Slice => {
            let FieldValue::StringSlice(v) = field else {
                unreachable!("the only slice field type is []string")
            };
            // PORT: an empty `Vec` stands for a nil slice (see the file note).
            if v.is_empty() {
                return LspAny::Null;
            }
            let mut result: Vec<LspAny> = Vec::with_capacity(v.len());
            for s in v {
                result.push(LspAny::String(s.clone()));
            }
            LspAny::Array(result)
        }
        // PORT: Go returns `field.Interface()`. Every field type of this kind
        // (`core.Tristate`) has a serializer, so this is not reached.
        FieldKind::Uint8 => unreachable!("core.Tristate has a type serializer"),
    }
}

// Go: ls/lsutil/userpreferences.go:184 (*UserPreferences).UnmarshalJSONFrom
impl UnmarshalerFrom for UserPreferences {
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        let mut config: IndexMap<String, LspAny> = IndexMap::new();
        json_unmarshal_decode(dec, &mut config)?;
        // Start with defaults, then overlay parsed values
        *self = new_default_user_preferences().with_config(&config);
        Ok(())
    }
}

// --- Helper methods ---

impl UserPreferences {
    // Go: ls/lsutil/userpreferences.go:203 (UserPreferences).ModuleSpecifierPreferences
    pub fn module_specifier_preferences(&self) -> crate::modulespecifiers::UserPreferences {
        crate::modulespecifiers::UserPreferences {
            import_module_specifier_preference: self.import_module_specifier_preference,
            import_module_specifier_ending: self.import_module_specifier_ending,
            auto_import_specifier_exclude_regexes: self
                .auto_import_specifier_exclude_regexes
                .clone(),
        }
    }

    // Go: ls/lsutil/userpreferences.go:211 (UserPreferences).ParsedAutoImportFileExcludePatterns
    // PORT: Go returns a nil `*SpecMatcher` for no patterns; that is `None`.
    pub fn parsed_auto_import_file_exclude_patterns(
        &self,
        use_case_sensitive_file_names: bool,
    ) -> Option<crate::frontend::vfs::vfsmatch::SpecMatcher> {
        crate::frontend::vfs::vfsmatch::new_spec_matcher(
            &self.auto_import_file_exclude_patterns,
            "",
            crate::frontend::vfs::vfsmatch::Usage::Exclude,
            use_case_sensitive_file_names,
        )
    }

    // Go: ls/lsutil/userpreferences.go:215 (UserPreferences).IsModuleSpecifierExcluded
    pub fn is_module_specifier_excluded(&self, module_specifier: &str) -> bool {
        crate::modulespecifiers::is_excluded_by_regex(
            module_specifier,
            &self.auto_import_specifier_exclude_regexes,
        )
    }
}

// Go: ls/lsutil/userpreferences.go:219 ParseUserPreferences
pub fn parse_user_preferences(items: &IndexMap<String, LspAny>) -> UserPreferences {
    let mut prefs = new_default_user_preferences();
    // Apply editor settings first (tabSize, indentSize, etc.) as raw-name defaults,
    // then overlay language-specific settings with increasing precedence:
    // editor < javascript < typescript < js/ts
    if let Some(editor_item) = items.get("editor")
        && !matches!(editor_item, LspAny::Null)
        && let LspAny::Object(editor_settings) = editor_item
    {
        let mut normalized_settings = editor_settings.clone();
        if let Some(tab_size) = normalized_settings.get("tabSize").cloned() {
            if !normalized_settings.contains_key("indentSize") {
                normalized_settings.insert("indentSize".to_string(), tab_size);
            }
        }
        if let Some(insert_spaces) = normalized_settings.get("insertSpaces").cloned() {
            if !normalized_settings.contains_key("convertTabsToSpaces") {
                normalized_settings.insert("convertTabsToSpaces".to_string(), insert_spaces);
            }
        }
        let mut config: IndexMap<String, LspAny> = IndexMap::new();
        config.insert("unstable".to_string(), LspAny::Object(normalized_settings));
        prefs = prefs.with_config(&config);
    }
    // Apply javascript, then typescript, then js/ts (highest precedence).
    for section in ["javascript", "typescript", "js/ts"] {
        if let Some(item) = items.get(section)
            && !matches!(item, LspAny::Null)
            && let LspAny::Object(settings) = item
        {
            prefs = prefs.with_config(settings);
        }
    }
    prefs
}

// PORT: Go `strings.ToLower` maps each rune with the simple (one rune)
// `unicode.ToLower`. The first char of Rust `char::to_lowercase` is that
// mapping (U+0130 is the only multi-char case, and it starts with 'i'). Rust
// `str::to_lowercase` also applies the final-sigma rule, which Go does not.
fn strings_to_lower(s: &str) -> String {
    s.chars()
        .map(|c| c.to_lowercase().next().unwrap_or(c))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ls::lsutil::IndentStyle;

    fn config(entries: Vec<(&str, LspAny)>) -> IndexMap<String, LspAny> {
        entries
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect()
    }

    fn object(entries: Vec<(&str, LspAny)>) -> LspAny {
        LspAny::Object(config(entries))
    }

    fn string(s: &str) -> LspAny {
        LspAny::String(s.to_string())
    }

    // Go: ls/lsutil/userpreferences_test.go:13 TestUserPreferencesParsingEdgeCases (ts#64554)
    // PORT: "empty array is not nil" has no Rust subject (a Vec has no nil);
    // the case is kept and checks the empty Vec.
    #[test]
    fn test_user_preferences_parsing_edge_cases() {
        type Expected = fn(&mut UserPreferences);
        let tests: Vec<(&str, IndexMap<String, LspAny>, Expected)> = vec![
            (
                "null raw values leave preferences unchanged",
                config(vec![
                    ("quotePreference", LspAny::Null),
                    ("maximumHoverLength", LspAny::Null),
                    ("includeCompletionsForModuleExports", LspAny::Null),
                ]),
                |_| {},
            ),
            (
                "invalid boolean becomes unknown",
                config(vec![(
                    "includeCompletionsForModuleExports",
                    string("invalid"),
                )]),
                |p| p.include_completions_for_module_exports = Tristate::Unknown,
            ),
            (
                "case-insensitive enums",
                config(vec![
                    ("quotePreference", string("SINGLE")),
                    ("jsxAttributeCompletionStyle", string("BRACES")),
                    ("organizeImportsCaseFirst", string("LOWER")),
                    ("includeInlayParameterNameHints", string("ALL")),
                    ("organizeImportsTypeOrder", string("FIRST")),
                    ("workspaceSymbolsScope", string("CURRENTPROJECT")),
                ]),
                |p| {
                    p.quote_preference = QuotePreference::SINGLE;
                    p.jsx_attribute_completion_style = JsxAttributeCompletionStyle::BRACES;
                    p.organize_imports_case_first = OrganizeImportsCaseFirst::LOWER;
                    p.inlay_hints.include_inlay_parameter_name_hints =
                        IncludeInlayParameterNameHints::ALL;
                    p.organize_imports_type_order = OrganizeImportsTypeOrder::FIRST;
                    p.workspace_symbols_scope = WorkspaceSymbolsScope::CURRENT_PROJECT;
                },
            ),
            (
                "present null primary path prevents fallback",
                config(vec![(
                    "suggest",
                    object(vec![
                        ("jsdoc", object(vec![("enabled", LspAny::Null)])),
                        ("completeJSDocs", LspAny::Bool(false)),
                    ]),
                )]),
                |_| {},
            ),
            (
                "null case sensitivity becomes unknown",
                config(vec![(
                    "preferences",
                    object(vec![(
                        "organizeImports",
                        object(vec![("caseSensitivity", LspAny::Null)]),
                    )]),
                )]),
                |p| p.organize_imports_ignore_case = Tristate::Unknown,
            ),
            (
                "numeric conversion and array filtering",
                config(vec![
                    ("maximumHoverLength", LspAny::Number(9.8)),
                    ("indentStyle", LspAny::Number(1.7)),
                    (
                        "autoImportFileExcludePatterns",
                        LspAny::Array(vec![
                            string("first"),
                            LspAny::Bool(false),
                            LspAny::Number(3.0),
                            string("second"),
                        ]),
                    ),
                ]),
                |p| {
                    p.maximum_hover_length = 9;
                    p.format_code_settings.editor_settings.indent_style = IndentStyle::BLOCK;
                    p.auto_import_file_exclude_patterns =
                        vec!["first".to_string(), "second".to_string()];
                },
            ),
            (
                "empty array is not nil",
                config(vec![(
                    "autoImportFileExcludePatterns",
                    LspAny::Array(vec![]),
                )]),
                |p| p.auto_import_file_exclude_patterns = Vec::new(),
            ),
            (
                "invalid module specifier preference uses its default",
                config(vec![(
                    "importModuleSpecifierPreference",
                    LspAny::Bool(true),
                )]),
                |p| {
                    p.import_module_specifier_preference =
                        ImportModuleSpecifierPreference::Shortest;
                },
            ),
        ];
        for (name, config, expected) in tests {
            let mut base = new_default_user_preferences();
            base.quote_preference = QuotePreference::DOUBLE;
            base.maximum_hover_length = 17;
            base.organize_imports_ignore_case = Tristate::True;
            let mut want = base.clone();
            expected(&mut want);
            assert_eq!(base.with_config(&config), want, "{name}");
        }
    }
}
