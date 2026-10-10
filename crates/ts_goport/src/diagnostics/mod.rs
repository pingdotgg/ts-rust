//! TypeScript diagnostic messages and rendered diagnostic instances. This
//! was the `ts_diagnostics` crate; `catalog.rs` comes from
//! `tools/ts_diagnostics_codegen`.

mod catalog;
mod effect_catalog;

use std::{error::Error, fmt};

pub use catalog::CATALOG;
pub use effect_catalog::EFFECT_CATALOG;

/// Go `diagnostics.Category`, an `int32`. The four named values are
/// TypeScript's diagnostic severity categories. A bad `.tsbuildinfo` can
/// give any other value. Go keeps it: it sorts as an int and is written to
/// the build info again as it was read. Go panics only when the diagnostic
/// is printed ("Unhandled diagnostic category").
// PORT: a newtype with associated consts, so `Category::Error` reads as the
// Go `diagnostics.CategoryError`.
#[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Category(pub i32);

#[allow(non_upper_case_globals)]
impl Category {
    pub const Warning: Self = Self(0);
    pub const Error: Self = Self(1);
    pub const Suggestion: Self = Self(2);
    pub const Message: Self = Self(3);

    // Go: diagnostics/diagnostics.go:26 (Category).Name
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Warning => "warning",
            Self::Error => "error",
            Self::Suggestion => "suggestion",
            Self::Message => "message",
            _ => crate::core::go_panic("Unhandled diagnostic category".to_string()),
        }
    }
}

/// Prints a named value as the old enum did (`Error`), and any other value
/// as `Category(999)`.
impl fmt::Debug for Category {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Warning => formatter.write_str("Warning"),
            Self::Error => formatter.write_str("Error"),
            Self::Suggestion => formatter.write_str("Suggestion"),
            Self::Message => formatter.write_str("Message"),
            Self(raw) => write!(formatter, "Category({raw})"),
        }
    }
}

impl fmt::Display for Category {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

/// A catalog entry from TypeScript's diagnosticMessages.json.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Message {
    code: u32,
    category: Category,
    /// "" in a catalog entry of the wasm build (`Message::catalog`).
    key: &'static str,
    /// "" in a catalog entry of the wasm build (`Message::catalog`).
    text: &'static str,
    reports_unnecessary: bool,
    elided_in_compatibility_pyramid: bool,
    reports_deprecated: bool,
}

impl Message {
    /// A message made outside the catalog: `ast::NIL_MESSAGE` and the ad
    /// hoc messages (code 0, key ""), and the messages that Go removed
    /// (`diag.rs`). It keeps its key and text on every target.
    #[doc(hidden)]
    #[must_use]
    pub const fn new(
        code: u32,
        category: Category,
        key: &'static str,
        text: &'static str,
        reports_unnecessary: bool,
        elided_in_compatibility_pyramid: bool,
        reports_deprecated: bool,
    ) -> Self {
        // The wasm build tells catalog entries by their code with no key.
        assert!(
            code == 0 || !key.is_empty(),
            "a message with a code has a key"
        );
        Self {
            code,
            category,
            key,
            text,
            reports_unnecessary,
            elided_in_compatibility_pyramid,
            reports_deprecated,
        }
    }

    /// An entry of the generated `CATALOG`. The wasm build leaves out its
    /// key (144 KB for all) and text (151 KB): `key` makes the key from the
    /// text, and `text` reads the texts that `parts/goport_util/build.rs`
    /// packed.
    #[doc(hidden)]
    #[must_use]
    pub const fn catalog(
        code: u32,
        category: Category,
        key: &'static str,
        text: &'static str,
        reports_unnecessary: bool,
        elided_in_compatibility_pyramid: bool,
        reports_deprecated: bool,
    ) -> Self {
        let packed = cfg!(target_family = "wasm");
        Self {
            code,
            category,
            key: if packed { "" } else { key },
            text: if packed { "" } else { text },
            reports_unnecessary,
            elided_in_compatibility_pyramid,
            reports_deprecated,
        }
    }

    /// wasm: true for a catalog entry, which has a code and no key.
    #[cfg(target_family = "wasm")]
    const fn packed(self) -> bool {
        self.code != 0 && self.key.is_empty()
    }

    #[must_use]
    pub const fn code(self) -> u32 {
        self.code
    }

    #[must_use]
    pub const fn category(self) -> Category {
        self.category
    }

    #[cfg(not(target_family = "wasm"))]
    #[must_use]
    pub const fn key(self) -> &'static str {
        self.key
    }

    /// wasm: the key. A catalog entry makes it from its text as Go's
    /// generator does (`message_key`), once.
    #[cfg(target_family = "wasm")]
    #[must_use]
    pub fn key(self) -> &'static str {
        use std::collections::HashMap;
        use std::sync::{Mutex, PoisonError};
        if !self.packed() {
            return self.key;
        }
        static KEYS: Mutex<Option<HashMap<u32, &'static str>>> = Mutex::new(None);
        let mut keys = KEYS.lock().unwrap_or_else(PoisonError::into_inner);
        keys.get_or_insert_with(HashMap::new)
            .entry(self.code)
            .or_insert_with(|| Box::leak(message_key(self.text(), self.code).into_boxed_str()))
    }

    #[cfg(not(target_family = "wasm"))]
    #[must_use]
    pub const fn text(self) -> &'static str {
        self.text
    }

    /// wasm: the text. A catalog entry reads it from the packed texts of
    /// the catalog, which the first read unpacks.
    #[cfg(target_family = "wasm")]
    #[must_use]
    pub fn text(self) -> &'static str {
        if !self.packed() {
            return self.text;
        }
        static TEXTS: std::sync::OnceLock<Vec<&'static str>> = std::sync::OnceLock::new();
        let texts = TEXTS.get_or_init(|| {
            let packed = include_bytes!(concat!(env!("OUT_DIR"), "/diagnostic_texts.lzma"));
            let texts: &'static str =
                Box::leak(crate::frontend::bundled::unpack(packed).into_boxed_str());
            let texts: Vec<&'static str> = texts.split_terminator('\0').collect();
            assert_eq!(
                texts.len(),
                CATALOG.len(),
                "a packed text for each catalog entry"
            );
            texts
        });
        let index = CATALOG
            .binary_search_by_key(&self.code, |message| message.code)
            .expect("a catalog message");
        texts[index]
    }

    #[must_use]
    pub const fn reports_unnecessary(self) -> bool {
        self.reports_unnecessary
    }

    #[must_use]
    pub const fn elided_in_compatibility_pyramid(self) -> bool {
        self.elided_in_compatibility_pyramid
    }

    #[must_use]
    pub const fn reports_deprecated(self) -> bool {
        self.reports_deprecated
    }

    /// Formats numbered placeholders such as {0} and {1}.
    ///
    /// # Errors
    ///
    /// Returns an error if the message references an argument that was not supplied.
    pub fn format(self, arguments: &[String]) -> Result<String, FormatError> {
        let text = self.text();
        if arguments.is_empty() {
            return Ok(text.to_owned());
        }

        let mut output = String::with_capacity(text.len());
        let mut remaining = text;
        while let Some(open) = remaining.find('{') {
            output.push_str(&remaining[..open]);
            remaining = &remaining[open + 1..];
            let digits = remaining.bytes().take_while(u8::is_ascii_digit).count();
            if digits == 0 || remaining.as_bytes().get(digits) != Some(&b'}') {
                output.push('{');
                continue;
            }

            let index_text = &remaining[..digits];
            let Ok(index) = index_text.parse::<usize>() else {
                output.push('{');
                continue;
            };
            let Some(argument) = arguments.get(index) else {
                return Err(FormatError {
                    code: self.code,
                    argument_index: index,
                });
            };
            output.push_str(argument);
            remaining = &remaining[digits + 1..];
        }
        output.push_str(remaining);
        Ok(output)
    }
}

/// One diagnostic occurrence with formatting arguments.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Diagnostic {
    pub message: &'static Message,
    pub arguments: Vec<String>,
    pub details: Vec<String>,
}

impl Diagnostic {
    #[must_use]
    pub const fn new(message: &'static Message) -> Self {
        Self {
            message,
            arguments: Vec::new(),
            details: Vec::new(),
        }
    }

    #[must_use]
    pub fn with_arguments(
        message: &'static Message,
        arguments: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            message,
            arguments: arguments.into_iter().map(Into::into).collect(),
            details: Vec::new(),
        }
    }

    #[must_use]
    pub fn with_details(mut self, details: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.details = details.into_iter().map(Into::into).collect();
        self
    }

    #[must_use]
    pub const fn code(&self) -> u32 {
        self.message.code()
    }

    #[must_use]
    pub const fn category(&self) -> Category {
        self.message.category()
    }

    /// Renders the diagnostic's English text.
    ///
    /// # Errors
    ///
    /// Returns an error when a required formatting argument is absent.
    pub fn render(&self) -> Result<String, FormatError> {
        let mut rendered = self.message.format(&self.arguments)?;
        for detail in &self.details {
            rendered.push('\n');
            rendered.push_str(detail);
        }
        Ok(rendered)
    }
}

/// A missing numbered diagnostic formatting argument.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FormatError {
    pub code: u32,
    pub argument_index: usize,
}

impl fmt::Display for FormatError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "diagnostic TS{} requires formatting argument {}",
            self.code, self.argument_index
        )
    }
}

impl Error for FormatError {}

/// Looks up a diagnostic message by its stable TypeScript code.
#[must_use]
pub fn message_by_code(code: u32) -> Option<&'static Message> {
    let table = if code >= EFFECT_CODE_START {
        EFFECT_CATALOG
    } else {
        CATALOG
    };
    table
        .binary_search_by_key(&code, |message| message.code())
        .ok()
        .map(|index| &table[index])
}

/// The first Effect diagnostic code. Effect messages (377000 to 377999) are
/// in `EFFECT_CATALOG`, not in the generated TypeScript catalog.
pub const EFFECT_CODE_START: u32 = 377_000;

/// Looks up a diagnostic message by its generated localization key.
#[must_use]
pub fn message_by_key(key: &str) -> Option<&'static Message> {
    #[cfg(not(target_family = "wasm"))]
    return CATALOG
        .iter()
        .chain(EFFECT_CATALOG)
        .find(|message| message.key() == key);
    // wasm makes keys on demand, so it finds the message by the code that
    // ends every key.
    #[cfg(target_family = "wasm")]
    key.rsplit_once('_')
        .and_then(|(_, code)| message_by_code(code.parse().ok()?))
        .filter(|message| message.key() == key)
}

/// The key of a message with `text` and `code`: Go's
/// `internal/diagnostics/generate.go` `convertPropertyName`. Each `*`,
/// `/` and `:` becomes a word, each other character that is not a letter
/// or digit becomes `_`; runs of `_` become one; leading `_` before a
/// non-digit and one trailing `_` go; the result is cut to 100 bytes and
/// gets `_<code>`.
// PORT: not in Go at run time. The wasm build makes its keys with it.
#[cfg(any(test, target_family = "wasm"))]
fn message_key(text: &str, code: u32) -> String {
    let mut name = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '*' => name.push_str("_Asterisk"),
            '/' => name.push_str("_Slash"),
            ':' => name.push_str("_Colon"),
            c if c.is_alphabetic() || c.is_numeric() => name.push(c),
            _ => name.push('_'),
        }
    }
    // `_+` -> `_`
    let mut collapsed = String::with_capacity(name.len());
    for c in name.chars() {
        if !(c == '_' && collapsed.ends_with('_')) {
            collapsed.push(c);
        }
    }
    // `^_+(\D)` -> `$1` (after the collapse there is at most one `_`)
    let mut key = match collapsed.strip_prefix('_') {
        Some(rest) if rest.chars().next().is_some_and(|c| !c.is_ascii_digit()) => rest.to_string(),
        _ => collapsed,
    };
    // `_$` -> ``
    if key.ends_with('_') {
        key.pop();
    }
    if key.len() > 100 {
        key.truncate(100);
    }
    format!("{key}_{code}")
}

/// The catalog text reader of the wasm build (`parts/goport_util/build.rs`),
/// for its test.
#[cfg(test)]
#[path = "../../parts/goport_util/catalog_texts.rs"]
mod catalog_texts;

#[cfg(test)]
mod tests {
    use super::{CATALOG, Category, Diagnostic, message_by_code, message_by_key, message_key};

    /// The wasm build packs the texts that `catalog_texts` reads from the
    /// catalog source; they must be the texts of the catalog.
    #[test]
    fn catalog_texts_reads_every_text() {
        let texts = super::catalog_texts::catalog_texts(include_str!("catalog.rs"));
        let texts: Vec<&str> = texts.split_terminator('\0').collect();
        let want: Vec<&str> = CATALOG.iter().map(|message| message.text()).collect();
        assert_eq!(texts, want);
    }

    /// The wasm build makes each key from its text (`message_key`); it
    /// must give the generated key of every message.
    #[test]
    fn message_key_makes_every_generated_key() {
        for message in CATALOG {
            assert_eq!(
                message_key(message.text(), message.code()),
                message.key(),
                "{}",
                message.code()
            );
        }
    }

    #[test]
    fn generated_catalog_is_complete_and_sorted() {
        assert_eq!(CATALOG.len(), 2_222);
        assert!(
            CATALOG
                .windows(2)
                .all(|pair| pair[0].code() < pair[1].code())
        );
    }

    #[test]
    fn representative_catalog_entries_match_typescript() {
        let unterminated = message_by_code(1002).unwrap();
        assert_eq!(unterminated.category(), Category::Error);
        assert_eq!(unterminated.text(), "Unterminated string literal.");
        assert_eq!(unterminated.key(), "Unterminated_string_literal_1002");

        let unused = message_by_code(6133).unwrap();
        assert_eq!(
            unused.text(),
            "'{0}' is declared but its value is never read."
        );
        assert!(unused.reports_unnecessary());

        let deprecated = message_by_code(6385).unwrap();
        assert_eq!(deprecated.category(), Category::Suggestion);
        assert!(deprecated.reports_deprecated());

        // Go #63987 moved the native-only messages from 100000 to TS codes.
        let native = message_by_code(6933).unwrap();
        assert_eq!(native.category(), Category::Message);
        assert_eq!(native.text(), "Do not print diagnostics.");
        assert!(message_by_code(100_000).is_none());
    }

    #[test]
    fn newly_exposed_checker_and_option_diagnostics_match_typescript() {
        let cases: &[(u32, &[&str], &str)] = &[
            (2349, &[], "This expression is not callable."),
            (
                5052,
                &["checkJs", "allowJs"],
                "Option 'checkJs' cannot be specified without specifying option 'allowJs'.",
            ),
            (
                6504,
                &["src/caf\u{00e9}.js"],
                "File 'src/caf\u{00e9}.js' is a JavaScript file. Did you mean to enable the 'allowJs' option?",
            ),
            (
                2874,
                &["React"],
                "This JSX tag requires 'React' to be in scope, but it could not be found.",
            ),
            (
                2875,
                &["react/jsx-runtime"],
                "This JSX tag requires the module path 'react/jsx-runtime' to exist, but none could be found. Make sure you have types for the appropriate package installed.",
            ),
        ];

        for &(code, arguments, expected) in cases {
            let message = message_by_code(code).expect("diagnostic exists in the pinned catalog");
            assert_eq!(message.category(), Category::Error);
            let diagnostic = Diagnostic::with_arguments(message, arguments.iter().copied());
            assert_eq!(diagnostic.render().unwrap(), expected, "TS{code}");
        }
    }

    #[test]
    fn ambient_declaration_diagnostics_match_pinned_typescript_records() {
        let cases: &[(u32, &str, &[&str], &str)] = &[
            (
                1039,
                "Initializers_are_not_allowed_in_ambient_contexts_1039",
                &[],
                "Initializers are not allowed in ambient contexts.",
            ),
            (
                1046,
                "Top_level_declarations_in_d_ts_files_must_start_with_either_a_declare_or_export_modifier_1046",
                &[],
                "Top-level declarations in .d.ts files must start with either a 'declare' or 'export' modifier.",
            ),
            (
                7010,
                "_0_which_lacks_return_type_annotation_implicitly_has_an_1_return_type_7010",
                &["foo", "any"],
                "'foo', which lacks return-type annotation, implicitly has an 'any' return type.",
            ),
        ];

        for &(code, key, arguments, expected) in cases {
            let message = message_by_code(code).expect("ambient diagnostic is in the catalog");
            assert_eq!(message.key(), key, "TS{code}");
            assert_eq!(message.category(), Category::Error, "TS{code}");
            assert_eq!(
                Diagnostic::with_arguments(message, arguments.iter().copied())
                    .render()
                    .unwrap(),
                expected,
                "TS{code}"
            );
        }
    }

    #[test]
    fn directive_and_related_diagnostics_keep_upstream_categories_and_flags() {
        let unused_directive = message_by_code(2578).unwrap();
        assert_eq!(unused_directive.category(), Category::Error);
        assert_eq!(
            unused_directive.text(),
            "Unused '@ts-expect-error' directive."
        );
        assert!(!unused_directive.reports_unnecessary());
        assert!(!unused_directive.reports_deprecated());

        let missing_argument = message_by_code(6210).unwrap();
        assert_eq!(missing_argument.category(), Category::Message);
        assert_eq!(
            Diagnostic::with_arguments(missing_argument, ["right"])
                .render()
                .unwrap(),
            "An argument for 'right' was not provided."
        );

        let property_origin = message_by_code(6500).unwrap();
        assert_eq!(property_origin.category(), Category::Message);
        assert_eq!(
            Diagnostic::with_arguments(property_origin, ["value", "Target"])
                .render()
                .unwrap(),
            "The expected type comes from property 'value' which is declared here on type 'Target'"
        );

        let elided = message_by_code(2202).unwrap();
        assert!(elided.elided_in_compatibility_pyramid());
        assert!(!missing_argument.elided_in_compatibility_pyramid());
    }

    #[test]
    fn diagnostics_render_numbered_arguments() {
        let message = message_by_code(1007).unwrap();
        let diagnostic = Diagnostic::with_arguments(message, ["{", "}"]);
        assert_eq!(
            diagnostic.render().unwrap(),
            "The parser expected to find a '}' to match the '{' token here."
        );
        assert_eq!(diagnostic.code(), 1007);
    }

    #[test]
    fn diagnostics_render_placeholders_inside_literal_braces() {
        let diagnostic =
            Diagnostic::with_arguments(message_by_code(2613).unwrap(), ["./module", "namedExport"]);
        assert_eq!(
            diagnostic.render().unwrap(),
            "Module './module' has no default export. Did you mean to use 'import { namedExport } from ./module' instead?"
        );
    }

    #[test]
    fn diagnostics_report_missing_numbered_arguments() {
        let diagnostic = Diagnostic::with_arguments(message_by_code(1007).unwrap(), ["{"]);
        let error = diagnostic.render().unwrap_err();
        assert_eq!(error.code, 1007);
        assert_eq!(error.argument_index, 1);
    }

    #[test]
    fn diagnostics_render_indented_message_details() {
        let message = message_by_code(2322).unwrap();
        let diagnostic = Diagnostic::with_arguments(message, ["source", "target"])
            .with_details(["  Types of parameters are incompatible."]);
        assert_eq!(
            diagnostic.render().unwrap(),
            concat!(
                "Type 'source' is not assignable to type 'target'.\n",
                "  Types of parameters are incompatible.",
            )
        );
    }

    #[test]
    fn lookup_by_key_uses_generated_key() {
        assert_eq!(
            message_by_key("Identifier_expected_1003").unwrap().code(),
            1003
        );
        assert!(message_by_code(u32::MAX).is_none());
    }
}
