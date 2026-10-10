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

/// Leaks a declaration so it has the Go pointer lifetime.
fn opt(o: CommandLineOption) -> &'static CommandLineOption {
    Box::leak(Box::new(o))
}

// CommandLineOption.Elements()
// Go: tsoptions/declarations_generated.go:984 commandLineOptionElements
// PORT: Go package-level vars are `LazyLock` statics.
pub static COMMAND_LINE_OPTION_ELEMENTS: LazyLock<
    FxHashMap<&'static str, &'static CommandLineOption>,
> = LazyLock::new(|| {
    [
        (
            "lib",
            opt(CommandLineOption {
                name: "lib",
                kind: CommandLineOptionKind::ENUM, // libMap,
                default_value_description: CompilerOptionsValue::Tristate(Tristate::Unknown),
                ..Default::default()
            }),
        ),
        (
            "rootDirs",
            opt(CommandLineOption {
                name: "rootDirs",
                kind: CommandLineOptionKind::STRING,
                is_file_path: true,
                ..Default::default()
            }),
        ),
        (
            "typeRoots",
            opt(CommandLineOption {
                name: "typeRoots",
                kind: CommandLineOptionKind::STRING,
                is_file_path: true,
                ..Default::default()
            }),
        ),
        (
            "types",
            opt(CommandLineOption {
                name: "types",
                kind: CommandLineOptionKind::STRING,
                ..Default::default()
            }),
        ),
        (
            "moduleSuffixes",
            opt(CommandLineOption {
                name: "moduleSuffixes",
                kind: CommandLineOptionKind::STRING,
                ..Default::default()
            }),
        ),
        (
            "customConditions",
            opt(CommandLineOption {
                name: "condition",
                kind: CommandLineOptionKind::STRING,
                ..Default::default()
            }),
        ),
        (
            "plugins",
            opt(CommandLineOption {
                name: "plugin",
                kind: CommandLineOptionKind::OBJECT,
                ..Default::default()
            }),
        ),
        // For tsconfig root options
        (
            "references",
            opt(CommandLineOption {
                name: "references",
                kind: CommandLineOptionKind::OBJECT,
                ..Default::default()
            }),
        ),
        // tsgo#4712
        (
            "contentMappers",
            opt(CommandLineOption {
                name: "contentMappers",
                kind: CommandLineOptionKind::OBJECT,
                ..Default::default()
            }),
        ),
        (
            "files",
            opt(CommandLineOption {
                name: "files",
                kind: CommandLineOptionKind::STRING,
                ..Default::default()
            }),
        ),
        (
            "include",
            opt(CommandLineOption {
                name: "include",
                kind: CommandLineOptionKind::STRING,
                ..Default::default()
            }),
        ),
        (
            "exclude",
            opt(CommandLineOption {
                name: "exclude",
                kind: CommandLineOptionKind::STRING,
                ..Default::default()
            }),
        ),
        (
            "extends",
            opt(CommandLineOption {
                name: "extends",
                kind: CommandLineOptionKind::STRING,
                ..Default::default()
            }),
        ),
        // Test infra options
        (
            "libFiles",
            opt(CommandLineOption {
                name: "libFiles",
                kind: CommandLineOptionKind::STRING,
                ..Default::default()
            }),
        ),
    ]
    .into_iter()
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
