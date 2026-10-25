use crate::diagnostics::Message;
use crate::frontend::prelude::*;
use std::sync::LazyLock;

// Go: tsoptions/commandlineoption.go:10 CommandLineOptionKind
// PORT: Go `CommandLineOptionKind` is a string type. It stays a string
// newtype. Go `CommandLineOptionTypeX` constants are the associated
// constants `CommandLineOptionKind::X`.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct CommandLineOptionKind(pub &'static str);

impl CommandLineOptionKind {
    // Go: tsoptions/commandlineoption.go:13 CommandLineOptionTypeString
    pub const STRING: Self = Self("string");
    // Go: tsoptions/commandlineoption.go:14 CommandLineOptionTypeNumber
    pub const NUMBER: Self = Self("number");
    // Go: tsoptions/commandlineoption.go:15 CommandLineOptionTypeBoolean
    pub const BOOLEAN: Self = Self("boolean");
    // Go: tsoptions/commandlineoption.go:16 CommandLineOptionTypeObject
    pub const OBJECT: Self = Self("object");
    // Go: tsoptions/commandlineoption.go:17 CommandLineOptionTypeList
    pub const LIST: Self = Self("list");
    // Go: tsoptions/commandlineoption.go:18 CommandLineOptionTypeListOrElement
    pub const LIST_OR_ELEMENT: Self = Self("listOrElement");
    // Go: tsoptions/commandlineoption.go:19 CommandLineOptionTypeEnum
    pub const ENUM: Self = Self("enum"); // map
}

/// Go `any` in option code (`CompilerOptionsValue`, enum map values,
/// `DefaultValueDescription`, and values read from tsconfig JSON).
// Go: tsoptions/commandlineoption.go:145 CompilerOptionsValue
// PORT: Go `any` becomes a closed enum with one variant per dynamic type
// that tsoptions stores in an `any`. `Nil` is Go untyped nil. `Int` is a Go
// `int`, `Number` a JSON `float64`. `List` is a non-nil `[]any`, `NilList`
// a nil `[]any`, and `Map` is `*collections.OrderedMap[string, any]`.
// `StringList`, `Paths` and `IntPtr` are the typed `[]string`,
// `*OrderedMap[string, []string]` and `*int` values of `core.CompilerOptions`
// fields (a typed nil pointer is not Go untyped nil, so these keep their own
// `None`). Go `int` is 64-bit, so `Int` and `IntPtr` hold an `i64`.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum CompilerOptionsValue {
    #[default]
    Nil,
    Bool(bool),
    Int(i64),
    Number(f64),
    String(String),
    Message(&'static Message),
    Tristate(Tristate),
    ScriptTarget(ScriptTarget),
    ModuleKind(ModuleKind),
    ModuleResolutionKind(ModuleResolutionKind),
    ModuleDetectionKind(ModuleDetectionKind),
    JsxEmit(JsxEmit),
    NewLineKind(NewLineKind),
    List(Vec<CompilerOptionsValue>),
    /// Go `[]any(nil)` in an `any`, which is not Go nil. It is a slice to
    /// `reflect` and `.([]any)` with no elements, and it stays nil through
    /// `core.MapIndex`, `core.Filter`, `core.Map` and `ParseStringArray`, so
    /// the option is unset. JSON conversion makes it for an array whose
    /// elements are all null, `convertJsonOptionOfListType` for a value that
    /// is not an array, and `ParseListTypeOption` for a list that filters to
    /// nothing.
    NilList,
    Map(IndexMap<String, CompilerOptionsValue>),
    /// Go `map[string]any`: what a Go JSON decode into an `any` makes for an
    /// object (the values of the API's `RawCompilerOptions`, ts#64159). Go
    /// code that asserts `*collections.OrderedMap[string, any]` (`Map`) does
    /// not match it, so a raw API "plugins" entry gets no name (Go N'
    /// tsoptions/options_generated.go:154-161).
    /// PORT: the Rust map keeps the input order; a Go map has no order.
    GoMap(IndexMap<String, CompilerOptionsValue>),
    StringList(Vec<String>),
    Paths(Option<IndexMap<String, Option<Vec<String>>>>),
    IntPtr(Option<i64>),
    /// Go `struct{}{}`, returned by `convertToJson` for a missing root.
    EmptyStruct,
}

impl CompilerOptionsValue {
    /// Go `value == nil` on an `any`.
    #[must_use]
    pub fn is_nil(&self) -> bool {
        matches!(self, CompilerOptionsValue::Nil)
    }

    /// Go `value.([]any)`: the elements of a `List` or a `NilList`.
    #[must_use]
    pub fn as_any_slice(&self) -> Option<&[CompilerOptionsValue]> {
        match self {
            CompilerOptionsValue::List(list) => Some(list),
            CompilerOptionsValue::NilList => Some(&[]),
            _ => None,
        }
    }
}

// Go: tsoptions/commandlineoption.go:85 CommandLineOption
// PORT: Go `string` fields of the static declarations are `&'static str`.
// Go `*diagnostics.Message` fields are `Option<&'static Message>`. Go
// unexported fields are plain `pub` fields. Declarations are leaked
// `&'static CommandLineOption` values, so Go pointer identity is
// `std::ptr::eq`.
#[derive(Clone, Debug, Default)]
pub struct CommandLineOption {
    pub name: &'static str,
    pub short_name: &'static str,
    pub kind: CommandLineOptionKind,

    // used in parsing
    pub is_file_path: bool,
    pub is_ts_config_only: bool,
    pub is_command_line_only: bool,

    // used in output
    pub description: Option<&'static Message>,
    pub default_value_description: CompilerOptionsValue,
    pub show_in_simplified_help_view: bool,

    // used in output in serializing and generate tsconfig
    pub category: Option<&'static Message>,

    // What kind of extra validation `validateJsonOptionValue` should do
    pub extra_validation: ExtraValidation,

    // checks that option with number type has value >= minValue
    pub min_value: i32,

    // true or undefined
    // used for configDirTemplateSubstitutionOptions
    pub allow_config_dir_template_substitution: bool,

    // used for filter in compilerrunner
    pub affects_declaration_path: bool,
    pub affects_program_structure: bool,
    pub affects_semantic_diagnostics: bool,
    pub affects_build_info: bool,
    pub affects_bind_diagnostics: bool,
    pub affects_source_file: bool,
    pub affects_module_resolution: bool,
    pub affects_emit: bool,

    pub allow_js_flag: bool,
    pub strict_flag: bool,

    // used in transpileoptions worker
    // todo: revisit to see if this can be reduced to boolean
    pub transpile_option_value: Tristate,

    // used for CommandLineOptionTypeList
    pub list_preserve_falsy_values: bool,
    // used for compilerOptionsDeclaration
    pub element_options: CommandLineOptionNameMap,
}

// Go: tsoptions/commandlineoption.go:115 extraValidation
// PORT: Go string type kept as a string newtype. ts#64457 removed
// extraValidationNone and extraValidationSpec (only the removed watch options
// used "spec"); `NONE` stays as the Rust name of the zero value.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct ExtraValidation(pub &'static str);

impl ExtraValidation {
    pub const NONE: Self = Self("");
    // Go: tsoptions/commandlineoption.go:117 extraValidationLocale
    pub const LOCALE: Self = Self("locale");
}

impl CommandLineOption {
    // Go: tsoptions/commandlineoption.go:119 DeprecatedKeys
    #[must_use]
    pub fn deprecated_keys(&self) -> Option<&'static FxHashSet<String>> {
        if self.kind != CommandLineOptionKind::ENUM {
            return None;
        }
        COMMAND_LINE_OPTION_DEPRECATED.get(self.name)
    }

    // Go: tsoptions/commandlineoption.go:126 EnumMap
    #[must_use]
    pub fn enum_map(&self) -> Option<&'static CommandLineOptionEnumMap> {
        if self.kind != CommandLineOptionKind::ENUM {
            return None;
        }
        COMMAND_LINE_OPTION_ENUM_MAP.get(self.name).copied()
    }

    // Go: tsoptions/commandlineoption.go:133 Elements
    #[must_use]
    pub fn elements(&self) -> Option<&'static CommandLineOption> {
        if self.kind != CommandLineOptionKind::LIST
            && self.kind != CommandLineOptionKind::LIST_OR_ELEMENT
        {
            return None;
        }
        COMMAND_LINE_OPTION_ELEMENTS.get(self.name).copied()
    }

    // Go: tsoptions/commandlineoption.go:140 DisallowNullOrUndefined
    #[must_use]
    pub fn disallow_null_or_undefined(&self) -> bool {
        self.name == "extends"
    }
}

/// One declaration of a static option list (here and in decls.rs) as const
/// data: the CommandLineOption fields that the lists set, with a Copy default
/// value. `leak` makes the CommandLineOption.
// PORT: Go declares the lists as `*CommandLineOption` literals. Built in
// code, each literal was a few hundred bytes of machine code; as static data
// each list is one table, and its LazyLock leaks one CommandLineOption per
// entry, so an option has one address in every list (Go pointer identity).
#[derive(Clone, Copy)]
pub(crate) struct OptionDecl {
    pub name: &'static str,
    pub short_name: &'static str,
    pub kind: CommandLineOptionKind,
    pub is_file_path: bool,
    pub is_ts_config_only: bool,
    pub is_command_line_only: bool,
    pub description: Option<&'static Message>,
    pub default_value_description: DefaultValue,
    pub show_in_simplified_help_view: bool,
    pub category: Option<&'static Message>,
    pub extra_validation: ExtraValidation,
    pub min_value: i32,
    pub allow_config_dir_template_substitution: bool,
    pub affects_declaration_path: bool,
    pub affects_program_structure: bool,
    pub affects_semantic_diagnostics: bool,
    pub affects_build_info: bool,
    pub affects_bind_diagnostics: bool,
    pub affects_source_file: bool,
    pub affects_module_resolution: bool,
    pub affects_emit: bool,
    pub allow_js_flag: bool,
    pub strict_flag: bool,
    pub transpile_option_value: Tristate,
    pub list_preserve_falsy_values: bool,
}

/// The `DefaultValueDescription` values of the static declarations, as
/// CompilerOptionsValue variants that are Copy (`String` is a static text).
#[derive(Clone, Copy)]
pub(crate) enum DefaultValue {
    Nil,
    Bool(bool),
    Int(i64),
    String(&'static str),
    Message(&'static Message),
    Tristate(Tristate),
    ScriptTarget(ScriptTarget),
}

impl DefaultValue {
    fn value(self) -> CompilerOptionsValue {
        match self {
            DefaultValue::Nil => CompilerOptionsValue::Nil,
            DefaultValue::Bool(b) => CompilerOptionsValue::Bool(b),
            DefaultValue::Int(i) => CompilerOptionsValue::Int(i),
            DefaultValue::String(s) => CompilerOptionsValue::String(s.to_string()),
            DefaultValue::Message(m) => CompilerOptionsValue::Message(m),
            DefaultValue::Tristate(t) => CompilerOptionsValue::Tristate(t),
            DefaultValue::ScriptTarget(t) => CompilerOptionsValue::ScriptTarget(t),
        }
    }
}

impl OptionDecl {
    /// The zero value of every field, as `CommandLineOption::default()`.
    pub const DEFAULT: OptionDecl = OptionDecl {
        name: "",
        short_name: "",
        kind: CommandLineOptionKind(""),
        is_file_path: false,
        is_ts_config_only: false,
        is_command_line_only: false,
        description: None,
        default_value_description: DefaultValue::Nil,
        show_in_simplified_help_view: false,
        category: None,
        extra_validation: ExtraValidation::NONE,
        min_value: 0,
        allow_config_dir_template_substitution: false,
        affects_declaration_path: false,
        affects_program_structure: false,
        affects_semantic_diagnostics: false,
        affects_build_info: false,
        affects_bind_diagnostics: false,
        affects_source_file: false,
        affects_module_resolution: false,
        affects_emit: false,
        allow_js_flag: false,
        strict_flag: false,
        transpile_option_value: Tristate::Unknown,
        list_preserve_falsy_values: false,
    };

    /// Makes the declaration's CommandLineOption and leaks it so it has the
    /// Go pointer lifetime.
    pub(crate) fn leak(&self) -> &'static CommandLineOption {
        Box::leak(Box::new(CommandLineOption {
            name: self.name,
            short_name: self.short_name,
            kind: self.kind,
            is_file_path: self.is_file_path,
            is_ts_config_only: self.is_ts_config_only,
            is_command_line_only: self.is_command_line_only,
            description: self.description,
            default_value_description: self.default_value_description.value(),
            show_in_simplified_help_view: self.show_in_simplified_help_view,
            category: self.category,
            extra_validation: self.extra_validation,
            min_value: self.min_value,
            allow_config_dir_template_substitution: self.allow_config_dir_template_substitution,
            affects_declaration_path: self.affects_declaration_path,
            affects_program_structure: self.affects_program_structure,
            affects_semantic_diagnostics: self.affects_semantic_diagnostics,
            affects_build_info: self.affects_build_info,
            affects_bind_diagnostics: self.affects_bind_diagnostics,
            affects_source_file: self.affects_source_file,
            affects_module_resolution: self.affects_module_resolution,
            affects_emit: self.affects_emit,
            allow_js_flag: self.allow_js_flag,
            strict_flag: self.strict_flag,
            transpile_option_value: self.transpile_option_value,
            list_preserve_falsy_values: self.list_preserve_falsy_values,
            element_options: CommandLineOptionNameMap::default(),
        }))
    }
}

// CommandLineOption.Elements()
// Go: tsoptions/declarations_generated.go:984 commandLineOptionElements
// PORT: Go package-level vars are `LazyLock` statics.
pub static COMMAND_LINE_OPTION_ELEMENTS: LazyLock<
    FxHashMap<&'static str, &'static CommandLineOption>,
> = LazyLock::new(|| {
    static DECLS: &[(&str, OptionDecl)] = &[
        (
            "lib",
            OptionDecl {
                name: "lib",
                kind: CommandLineOptionKind::ENUM, // libMap,
                default_value_description: DefaultValue::Tristate(Tristate::Unknown),
                ..OptionDecl::DEFAULT
            },
        ),
        (
            "rootDirs",
            OptionDecl {
                name: "rootDirs",
                kind: CommandLineOptionKind::STRING,
                is_file_path: true,
                ..OptionDecl::DEFAULT
            },
        ),
        (
            "typeRoots",
            OptionDecl {
                name: "typeRoots",
                kind: CommandLineOptionKind::STRING,
                is_file_path: true,
                ..OptionDecl::DEFAULT
            },
        ),
        (
            "types",
            OptionDecl {
                name: "types",
                kind: CommandLineOptionKind::STRING,
                ..OptionDecl::DEFAULT
            },
        ),
        (
            "moduleSuffixes",
            OptionDecl {
                name: "moduleSuffixes",
                kind: CommandLineOptionKind::STRING,
                ..OptionDecl::DEFAULT
            },
        ),
        (
            "customConditions",
            OptionDecl {
                name: "condition",
                kind: CommandLineOptionKind::STRING,
                ..OptionDecl::DEFAULT
            },
        ),
        (
            "plugins",
            OptionDecl {
                name: "plugin",
                kind: CommandLineOptionKind::OBJECT,
                ..OptionDecl::DEFAULT
            },
        ),
        // For tsconfig root options
        (
            "references",
            OptionDecl {
                name: "references",
                kind: CommandLineOptionKind::OBJECT,
                ..OptionDecl::DEFAULT
            },
        ),
        // tsgo#4712
        (
            "contentMappers",
            OptionDecl {
                name: "contentMappers",
                kind: CommandLineOptionKind::OBJECT,
                ..OptionDecl::DEFAULT
            },
        ),
        (
            "files",
            OptionDecl {
                name: "files",
                kind: CommandLineOptionKind::STRING,
                ..OptionDecl::DEFAULT
            },
        ),
        (
            "include",
            OptionDecl {
                name: "include",
                kind: CommandLineOptionKind::STRING,
                ..OptionDecl::DEFAULT
            },
        ),
        (
            "exclude",
            OptionDecl {
                name: "exclude",
                kind: CommandLineOptionKind::STRING,
                ..OptionDecl::DEFAULT
            },
        ),
        (
            "extends",
            OptionDecl {
                name: "extends",
                kind: CommandLineOptionKind::STRING,
                ..OptionDecl::DEFAULT
            },
        ),
        // Test infra options
        (
            "libFiles",
            OptionDecl {
                name: "libFiles",
                kind: CommandLineOptionKind::STRING,
                ..OptionDecl::DEFAULT
            },
        ),
    ];
    DECLS
        .iter()
        .map(|(name, decl)| (*name, decl.leak()))
        .collect()
});

// CommandLineOption.EnumMap()
// Go: tsoptions/declarations_generated.go:1354 commandLineOptionEnumMap
pub static COMMAND_LINE_OPTION_ENUM_MAP: LazyLock<
    FxHashMap<&'static str, &'static CommandLineOptionEnumMap>,
> = LazyLock::new(|| {
    let entries: [(&'static str, &'static CommandLineOptionEnumMap); 7] = [
        ("lib", &LIB_MAP),
        ("moduleResolution", &MODULE_RESOLUTION_OPTION_MAP),
        ("module", &MODULE_OPTION_MAP),
        ("target", &TARGET_OPTION_MAP),
        ("moduleDetection", &MODULE_DETECTION_OPTION_MAP),
        ("jsx", &JSX_OPTION_MAP),
        ("newLine", &NEW_LINE_OPTION_MAP),
    ];
    entries.into_iter().collect()
});

// CommandLineOption.DeprecatedKeys()
// Go: tsoptions/declarations_generated.go:1364 commandLineOptionDeprecated
pub static COMMAND_LINE_OPTION_DEPRECATED: LazyLock<FxHashMap<&'static str, FxHashSet<String>>> =
    LazyLock::new(|| {
        let set = |items: &[&str]| {
            items
                .iter()
                .map(|s| s.to_string())
                .collect::<FxHashSet<String>>()
        };
        let mut m = FxHashMap::default();
        m.insert("module", set(&["none", "amd", "system", "umd"]));
        m.insert("moduleResolution", set(&["node", "classic", "node10"]));
        m.insert("target", set(&["es5"]));
        m
    });
