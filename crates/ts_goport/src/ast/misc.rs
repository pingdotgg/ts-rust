//! Go `ast/symbol.go`, `ast/diagnostic.go`, `ast/precedence.go`,
//! `ast/modifierflags.go`, `ast/flow.go`, `ast/checkflags.go`,
//! `ast/symbolflags.go`, `ast/functionflags.go`, `ast/positionmap.go` and
//! `ast/ids.go` (functions and non-flag types only; the flag consts live in
//! `crate::flags`).

use crate::prelude::*;

// ---------------------------------------------------------------------------
// symbol.go
// ---------------------------------------------------------------------------

// Go: ast/symbol.go:28 GetSourceFileOfSymbol (ts#64518)
// GetSourceFileOfSymbol returns the owning file of a published binder symbol, or
// nil for a non-file-owned symbol, even if it borrows declarations from a file.
// Ownership recovery walks only the first declaration's AST parents.
// PORT: Go reads `symbol.Parent` directly; here the arena that owns the
// symbol is passed in. The result is the SourceFile node (`Node::NIL` for
// Go nil).
#[must_use]
pub fn get_source_file_of_symbol(symbols: &SymbolArena, symbol: SymbolId) -> Node {
    go_assert!(symbol.is_some(), "Expected a symbol");
    let mut s = symbols.sym(symbol);
    if s.flags.intersects(SymbolFlags::TRANSIENT) {
        return Node::NIL;
    }
    if s.declarations.is_empty() {
        // A class's implicit prototype has no declaration of its own.
        go_assert!(
            s.flags.intersects(SymbolFlags::PROTOTYPE),
            "File-bound symbol has no declarations"
        );
        go_assert!(
            s.parent.is_some() && symbols.sym(s.parent).flags.intersects(SymbolFlags::CLASS),
            "Prototype has no declaring class"
        );
        s = symbols.sym(s.parent);
        go_assert!(
            !s.flags.intersects(SymbolFlags::TRANSIENT),
            "Prototype parent is not file-bound"
        );
        go_assert!(
            !s.declarations.is_empty(),
            "Prototype parent has no declarations"
        );
    }
    let file = get_source_file_of_node(s.declarations[0]);
    go_assert!(file.is_some(), "File-bound declaration has no source file");
    file
}

impl Symbol {
    // Go: ast/symbol.go:23 IsExternalModule
    #[must_use]
    pub fn is_external_module(&self) -> bool {
        self.flags.intersects(SymbolFlags::MODULE) && is_ambient_module_symbol_name(&self.name)
    }

    // Go: ast/symbol.go:27 IsStatic
    #[must_use]
    pub fn is_static(&self) -> bool {
        if self.value_declaration.is_nil() {
            return false;
        }
        let modifier_flags = self.value_declaration.modifier_flags();
        modifier_flags.intersects(ModifierFlags::STATIC)
    }

    // Go: ast/symbol.go:36 CombinedLocalAndExportSymbolFlags
    // See comment on `declareModuleMember` in `binder.go`.
    // PORT: Go follows `s.ExportSymbol` directly; here the arena that owns the
    // export symbol is passed in.
    #[must_use]
    pub fn combined_local_and_export_symbol_flags(&self, symbols: &SymbolArena) -> SymbolFlags {
        if self.export_symbol.is_some() {
            return self.flags | symbols.sym(self.export_symbol).flags;
        }
        self.flags
    }
}

// PORT: Go uses the byte 0xFE, which is invalid UTF-8. A Rust `String` holds
// it in the port form of Go strings (see `scanner_util::GO_STRING_MARKER`):
// U+FDD0 + U+10F7FE, the invalid byte unit for 0xFE. Source text with the
// byte 0xFE has the same port form, so it names the same internal symbols as
// in Go, and a real U+FFFE char stays an ordinary char. Go byte checks such as
// `name[0] == '\xFE'` port to `name.starts_with(INTERNAL_SYMBOL_NAME_PREFIX)`.
pub const INTERNAL_SYMBOL_NAME_PREFIX: &str = "\u{FDD0}\u{10F7FE}"; // Invalid as IdentifierName

pub const INTERNAL_SYMBOL_NAME_CALL: &str = "\u{FDD0}\u{10F7FE}call"; // Call signatures
pub const INTERNAL_SYMBOL_NAME_CONSTRUCTOR: &str = "\u{FDD0}\u{10F7FE}constructor"; // Constructor implementations
pub const INTERNAL_SYMBOL_NAME_NEW: &str = "\u{FDD0}\u{10F7FE}new"; // Constructor signatures
pub const INTERNAL_SYMBOL_NAME_INDEX: &str = "\u{FDD0}\u{10F7FE}index"; // Index signatures
pub const INTERNAL_SYMBOL_NAME_EXPORT_STAR: &str = "\u{FDD0}\u{10F7FE}export"; // Module export * declarations
pub const INTERNAL_SYMBOL_NAME_GLOBAL: &str = "\u{FDD0}\u{10F7FE}global"; // Global self-reference
pub const INTERNAL_SYMBOL_NAME_MISSING: &str = "\u{FDD0}\u{10F7FE}missing"; // Indicates missing symbol
pub const INTERNAL_SYMBOL_NAME_TYPE: &str = "\u{FDD0}\u{10F7FE}type"; // Anonymous type literal symbol
pub const INTERNAL_SYMBOL_NAME_OBJECT: &str = "\u{FDD0}\u{10F7FE}object"; // Anonymous object literal declaration
pub const INTERNAL_SYMBOL_NAME_JSX_ATTRIBUTES: &str = "\u{FDD0}\u{10F7FE}jsxAttributes"; // Anonymous JSX attributes object literal declaration
pub const INTERNAL_SYMBOL_NAME_CLASS: &str = "\u{FDD0}\u{10F7FE}class"; // Unnamed class expression
pub const INTERNAL_SYMBOL_NAME_FUNCTION: &str = "\u{FDD0}\u{10F7FE}function"; // Unnamed function expression
pub const INTERNAL_SYMBOL_NAME_COMPUTED: &str = "\u{FDD0}\u{10F7FE}computed"; // Computed property name declaration with dynamic name
pub const INTERNAL_SYMBOL_NAME_ASSIGNMENT_DECLARATION: &str = "\u{FDD0}\u{10F7FE}assignment"; // Assignment declarations
pub const INTERNAL_SYMBOL_NAME_INSTANTIATION_EXPRESSION: &str =
    "\u{FDD0}\u{10F7FE}instantiationExpression"; // Instantiation expressions
pub const INTERNAL_SYMBOL_NAME_IMPORT_ATTRIBUTES: &str = "\u{FDD0}\u{10F7FE}importAttributes";
pub const INTERNAL_SYMBOL_NAME_EXPORT_EQUALS: &str = "export="; // Export assignment symbol
pub const INTERNAL_SYMBOL_NAME_DEFAULT: &str = "default"; // Default export symbol (technically not wholly internal, but included here for usability)
pub const INTERNAL_SYMBOL_NAME_THIS: &str = "this";
pub const INTERNAL_SYMBOL_NAME_MODULE_EXPORTS: &str = "module.exports";

// Go: ast/symbol.go:72 SymbolName
#[must_use]
pub fn symbol_name(symbols: &SymbolArena, symbol: SymbolId) -> String {
    let s = symbols.sym(symbol);
    if s.value_declaration.is_some()
        && is_private_identifier_class_element_declaration(s.value_declaration)
    {
        return s.value_declaration.name().text().to_string();
    }
    s.name.to_string()
}

// Go: ast/symbol.go:80 EscapeAllInternalSymbolNames
// EscapeAllInternalSymbolNames replaces internal symbol name markers ("\xFE") with "__".
// PORT: each byte 0xFE is the unit INTERNAL_SYMBOL_NAME_PREFIX in the port
// form. A plain text search could also match a U+FDD0 unit (U+FDD0 twice)
// followed by a real U+10F7FE char, so this reads the units.
#[must_use]
pub fn escape_all_internal_symbol_names(name: &str) -> String {
    if !contains_go_string_marker(name) {
        return name.to_string();
    }
    let mut out = String::with_capacity(name.len());
    let mut i = 0usize;
    while i < name.len() {
        let (unit, size) = go_unit_at(name, i);
        if unit == GoUnit::InvalidByte(0xFE) {
            out.push_str("__");
        } else {
            out.push_str(&name[i..i + size]);
        }
        i += size;
    }
    out
}

// Go: ast/symbol.go:84 EscapeInternalSymbolName
// PORT: Go `strings.CutPrefix(name, "\xFE")` reads the first Go byte. The
// byte 0xFE is the unit `INTERNAL_SYMBOL_NAME_PREFIX` in the port form.
#[must_use]
pub fn escape_internal_symbol_name(name: &str) -> String {
    if let Some(rest) = name.strip_prefix(INTERNAL_SYMBOL_NAME_PREFIX) {
        return format!("__{rest}");
    }
    name.to_string()
}

// Go: ast/symbol.go:95 EscapeSymbolName
// EscapeSymbolName converts a binder symbol name into its escaped "__String"
// form. Internal names (prefixed with the "\xFE" sentinel) become "__"-prefixed,
// and user names that already begin with "__" gain an extra leading underscore
// so they can be distinguished from internal names.
#[must_use]
pub fn escape_symbol_name(name: &str) -> String {
    if let Some(rest) = name.strip_prefix(INTERNAL_SYMBOL_NAME_PREFIX) {
        return format!("__{rest}");
    }
    if name.starts_with("__") {
        return format!("_{name}");
    }
    name.to_string()
}

// ---------------------------------------------------------------------------
// diagnostic.go
// ---------------------------------------------------------------------------

/// Go `ast.RepopulateDiagnosticKind`: the kind of repopulation for a
/// diagnostic chain entry.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RepopulateDiagnosticKind(pub i32);

impl RepopulateDiagnosticKind {
    pub const MODE_MISMATCH: Self = Self(1); // RepopulateModeMismatch
    pub const MODULE_NOT_FOUND: Self = Self(2); // RepopulateModuleNotFound
}

/// Go `ast.RepopulateDiagnosticInfo`. It stores information needed to
/// recompute a diagnostic chain entry during incremental builds when the
/// program state may have changed.
/// PORT: Go `core.ResolutionMode` is an alias of `core.ModuleKind`.
#[derive(Clone, Debug, Default)]
pub struct RepopulateDiagnosticInfo {
    pub kind: RepopulateDiagnosticKind,
    pub module_reference: String,
    pub mode: ModuleKind,
    pub package_name: String,
}

// PORT: `core::Diagnostic` has no `messageKey` field. The message key is
// always `message.key()`.
impl Diagnostic {
    // Go: ast/diagnostic.go:58 File
    #[must_use]
    pub fn file(&self) -> Node {
        self.file
    }

    // Go: ast/diagnostic.go:59 Pos
    #[must_use]
    pub fn pos(&self) -> i32 {
        self.pos
    }

    // Go: ast/diagnostic.go:60 End
    #[must_use]
    pub fn end(&self) -> i32 {
        self.end
    }

    // Go: ast/diagnostic.go:61 Len
    #[must_use]
    pub fn len(&self) -> i32 {
        self.end - self.pos
    }

    // Go: ast/diagnostic.go:62 Loc
    #[must_use]
    pub fn loc(&self) -> TextRange {
        TextRange::new(self.pos, self.end)
    }

    // Go: ast/diagnostic.go:63 Code
    #[must_use]
    pub fn code(&self) -> i32 {
        self.code
    }

    // Go: ast/diagnostic.go:64 Category
    #[must_use]
    pub fn category(&self) -> crate::diagnostics::Category {
        self.category
    }

    // Go: ast/diagnostic.go:65 Source (tsgo#4712)
    #[must_use]
    pub fn source(&self) -> &str {
        &self.source
    }

    // Go: ast/diagnostic.go:66 MessageText (tsgo#4712)
    #[must_use]
    pub fn message_text(&self) -> &str {
        &self.message_text
    }

    // Go: ast/diagnostic.go:67 MessageKey
    // PORT: a diagnostic with the Go nil message (`NIL_MESSAGE`) has the key
    // "", as in Go.
    #[must_use]
    pub fn message_key(&self) -> &'static str {
        self.message.key()
    }

    // Go: ast/diagnostic.go:68 MessageArgs
    #[must_use]
    pub fn message_args(&self) -> &[String] {
        &self.message_args
    }

    // Go: ast/diagnostic.go:69 MessageChain
    #[must_use]
    pub fn message_chain(&self) -> &[Diagnostic] {
        &self.message_chain
    }

    // Go: ast/diagnostic.go:70 RelatedInformation
    #[must_use]
    pub fn related_information(&self) -> &[Diagnostic] {
        &self.related_information
    }

    // Go: ast/diagnostic.go:71 ReportsUnnecessary
    #[must_use]
    pub fn reports_unnecessary(&self) -> bool {
        self.reports_unnecessary
    }

    // Go: ast/diagnostic.go:72 ReportsDeprecated
    #[must_use]
    pub fn reports_deprecated(&self) -> bool {
        self.reports_deprecated
    }

    // Go: ast/diagnostic.go:73 SkippedOnNoEmit
    #[must_use]
    pub fn skipped_on_no_emit(&self) -> bool {
        self.skipped_on_no_emit
    }

    // Go: ast/diagnostic.go:74 RepopulateInfo
    #[must_use]
    pub fn repopulate_info(&self) -> Option<std::sync::Arc<RepopulateDiagnosticInfo>> {
        self.repopulate_info.clone()
    }

    // Go: ast/diagnostic.go:76 SetFile
    pub fn set_file(&mut self, file: Node) {
        self.file = file;
    }

    // Go: ast/diagnostic.go:77 SetLocation
    pub fn set_location(&mut self, loc: TextRange) {
        self.pos = loc.pos();
        self.end = loc.end();
    }

    // Go: ast/diagnostic.go:78 SetCategory
    pub fn set_category(&mut self, category: crate::diagnostics::Category) {
        self.category = category;
    }

    // Go: ast/diagnostic.go:79 SetSkippedOnNoEmit
    pub fn set_skipped_on_no_emit(&mut self) {
        self.skipped_on_no_emit = true;
    }

    // Go: ast/diagnostic.go:80 SetRepopulateInfo
    pub fn set_repopulate_info(&mut self, info: Option<std::sync::Arc<RepopulateDiagnosticInfo>>) {
        self.repopulate_info = info;
    }

    // Go: ast/diagnostic.go:82 SetExternalData (tsgo#4712)
    pub fn set_external_data(&mut self, source: &str, message_text: &str) -> &mut Self {
        self.source = source.to_string();
        self.message_text = message_text.to_string();
        self
    }

    // Go: ast/diagnostic.go:88 SetMessageChain
    // PORT: Go returns the receiver for chaining; this returns `&mut Self`.
    pub fn set_message_chain(&mut self, message_chain: Vec<Diagnostic>) -> &mut Self {
        self.message_chain = message_chain;
        self
    }

    // Go: ast/diagnostic.go:93 AddMessageChain
    pub fn add_message_chain(&mut self, message_chain: Option<Diagnostic>) -> &mut Self {
        if let Some(message_chain) = message_chain {
            self.message_chain.push(message_chain);
        }
        self
    }

    // Go: ast/diagnostic.go:100 SetRelatedInfo
    pub fn set_related_info(&mut self, related_information: Vec<Diagnostic>) -> &mut Self {
        self.related_information = related_information;
        self
    }

    // Go: ast/diagnostic.go:105 AddRelatedInfo
    pub fn add_related_info(&mut self, related_information: Option<Diagnostic>) -> &mut Self {
        if let Some(related_information) = related_information {
            self.related_information.push(related_information);
        }
        self
    }

    // Go: ast/diagnostic.go:112 Clone
    // PORT: Go `d.Clone()` is a shallow copy; use the derived `Clone::clone`.
    // Go shares the slices between the copies, but no Go caller mutates them
    // in place after cloning, so a deep clone behaves the same.

    // Go: ast/diagnostic.go:117 Localize
    // PORT: the port resolves the message when it makes the diagnostic (see
    // NewDiagnosticFromSerialized below), so the Go `d.messageKey` is read
    // only for the Go nil message (`NIL_MESSAGE` with the key "", or an
    // unknown key), and Go panics there as it does here.
    // PORT: Go returns the message text when `d.message == nil`. Go sets the
    // text only on a diagnostic with a nil message: `NewExternalDiagnostic`
    // and `SetExternalData` on a serialized diagnostic. A serialized port
    // diagnostic has its resolved message, so the text alone decides.
    #[must_use]
    pub fn localize(&self, locale: &crate::locale::Locale) -> String {
        if !self.message_text.is_empty() {
            return self.message_text.clone();
        }
        let message = (!is_nil_message(self.message)).then_some(self.message);
        crate::diagnostics_loc::localize(
            locale,
            message,
            self.message_key(),
            &self.display_message_args(),
        )
    }

    // Go: ast/diagnostic.go:125 String
    // For debugging only.
    // PORT: as `localize`, the text alone decides. For the Go nil message
    // with no text, Go panics on the unknown key ""; this returns "".
    #[must_use]
    pub fn string(&self) -> String {
        if !self.message_text.is_empty() {
            return self.message_text.clone();
        }
        format_message(self.message, &self.display_message_args())
    }

    // Go: ast/diagnostic.go:134 displayMessageArgs (tsgo#4712)
    // displayMessageArgs substitutes the original text for a complete alias span when a diagnostic argument
    // exactly matches the virtual alias. Stored arguments remain unchanged for code fixes and serialization.
    // PORT: returns the stored arguments borrowed, or a changed copy. Go
    // slices the texts at any byte; a range that does not fall on char
    // boundaries here (the port form of a Go string) cannot equal an
    // argument, so it gives the stored arguments.
    fn display_message_args(&self) -> std::borrow::Cow<'_, [String]> {
        use std::borrow::Cow;
        if self.file.is_nil() || !self.source.is_empty() {
            return Cow::Borrowed(self.message_args.as_slice());
        }
        let (segment, ok) = crate::spanmap::SpanMap::alias_for_virtual_span(
            source_file_span_map(self.file),
            self.loc(),
        );
        if !ok {
            return Cow::Borrowed(self.message_args.as_slice());
        }
        let virtual_text = source_file_text(self.file);
        let original_text = source_file_original_text(self.file);
        if segment.virtual_start < 0
            || segment.virtual_end > virtual_text.len() as i32
            || segment.original_start < 0
            || segment.original_end > original_text.len() as i32
        {
            return Cow::Borrowed(self.message_args.as_slice());
        }
        let (Some(virtual_name), Some(original_name)) = (
            virtual_text.get(segment.virtual_start as usize..segment.virtual_end as usize),
            original_text.get(segment.original_start as usize..segment.original_end as usize),
        ) else {
            return Cow::Borrowed(self.message_args.as_slice());
        };
        let mut result: Option<Vec<String>> = None;
        for (i, arg) in self.message_args.iter().enumerate() {
            if arg != virtual_name {
                continue;
            }
            result.get_or_insert_with(|| self.message_args.clone())[i] = original_name.to_string();
        }
        match result {
            Some(result) => Cow::Owned(result),
            None => Cow::Borrowed(self.message_args.as_slice()),
        }
    }
}

/// The Go nil `*diagnostics.Message` of a diagnostic: `NewExternalDiagnostic`
/// makes one (tsgo#4712), and so does `NewDiagnosticFromSerialized` with the
/// key "" of a serialized external diagnostic.
// PORT: `core::Diagnostic.message` is `&'static Message`, so the nil message
// is this empty one. Its key is "" and its code 0; the diagnostic keeps its
// own code and category.
pub static NIL_MESSAGE: &crate::diagnostics::Message = &crate::diagnostics::Message::new(
    0,
    crate::diagnostics::Category::Error,
    "",
    "",
    false,
    false,
    false,
);

/// True for the Go nil message: `NIL_MESSAGE`, or the message of an
/// unknown key (`unknown_key_message`).
#[must_use]
pub fn is_nil_message(message: &'static crate::diagnostics::Message) -> bool {
    std::ptr::eq(message, NIL_MESSAGE) || (message.code() == 0 && !message.key().is_empty())
}

/// The Go nil message of a serialized diagnostic whose key is not in the
/// catalog: code 0 and the key, which no other message has (catalog
/// messages have a code, and the nil and ad hoc messages the key "").
// PORT: the message is leaked, as the ad hoc messages are. Only a bad
// `.tsbuildinfo` has such a key.
fn unknown_key_message(key: &str) -> &'static crate::diagnostics::Message {
    let key: &'static str = Box::leak(key.to_string().into_boxed_str());
    Box::leak(Box::new(crate::diagnostics::Message::new(
        0,
        crate::diagnostics::Category::Error,
        key,
        "",
        false,
        false,
        false,
    )))
}

// Go: diagnostics/diagnostics.go:129 Format
// PORT: also Go `diagnostics.Localize` with the default locale; the port has
// only the English messages. `Message::format` replaces the placeholders,
// and Go panics on a bad placeholder.
pub fn format_message(message: &'static crate::diagnostics::Message, args: &[String]) -> String {
    // Replace invalid UTF-8 with Unicode replacement character
    // PORT: each arg is the port form of a Go string (see
    // `scanner_util::GO_STRING_MARKER`), so only an arg with a marker can
    // hold invalid bytes.
    let valid: Vec<String>;
    let args = if args.iter().any(|arg| contains_go_string_marker(arg)) {
        valid = args
            .iter()
            .map(|arg| go_to_valid_utf8(arg).into_owned())
            .collect();
        &valid[..]
    } else {
        args
    };
    match message.format(args) {
        Ok(text) => text,
        Err(_) => crate::core::go_panic("Invalid formatting placeholder".to_string()),
    }
}

// Go: ast/diagnostic.go:166 NewDiagnosticFromSerialized
// PORT: Go keeps `message` nil and resolves the key lazily in `Localize`
// (panicking on an unknown key). `core::Diagnostic` needs the message, so a
// known key is resolved here. The key "" of a serialized external
// diagnostic (tsgo#4712: Go `SetExternalData` gives it its text next) is the
// Go nil message, and `localize` panics on it without a text, as Go does.
// An unknown key (a bad `.tsbuildinfo`) is a nil message that keeps the key
// (`unknown_key_message`), so `localize` panics when Go does, with Go's
// text, and the build info keeps the key.
#[allow(clippy::too_many_arguments)]
#[must_use]
pub fn new_diagnostic_from_serialized(
    file: Node,
    loc: TextRange,
    code: i32,
    category: crate::diagnostics::Category,
    message_key: &str,
    message_args: Vec<String>,
    message_chain: Vec<Diagnostic>,
    related_information: Vec<Diagnostic>,
    reports_unnecessary: bool,
    reports_deprecated: bool,
    skipped_on_no_emit: bool,
) -> Diagnostic {
    // Go `Localize` resolves the key with the generated `keyToMessage`, which
    // also knows the messages that are local to this crate.
    let message = match crate::diag::key_to_message(message_key) {
        Some(message) => message,
        None if message_key.is_empty() => NIL_MESSAGE,
        None => unknown_key_message(message_key),
    };
    Diagnostic {
        file,
        pos: loc.pos(),
        end: loc.end(),
        code,
        category,
        source: String::new(),
        message,
        message_text: String::new(),
        message_args,
        message_chain,
        related_information,
        reports_unnecessary,
        reports_deprecated,
        skipped_on_no_emit,
        repopulate_info: None,
    }
}

// Go: ast/diagnostic.go:194 NewDiagnosticFromText (ts#63950)
// PORT: Go `file *SourceFile` is the SourceFile node (nil is `Node::NIL`).
// Go sets `message` to `diagnostics.NewAdHocMessage(text)` and leaves
// `messageKey` empty. The port reads the key from the message
// (`message_key`), so the ad hoc message has the key "" here, not the Go
// "-1": `MessageKey()` is "" as in Go, and no locale has either key. The
// message code is 0, not the Go -1 (`Message` codes are `u32`); the
// diagnostic keeps its own code, and nothing reads the message code.
#[allow(clippy::too_many_arguments)]
#[must_use]
pub fn new_diagnostic_from_text(
    file: Node,
    loc: TextRange,
    code: i32,
    category: crate::diagnostics::Category,
    text: &str,
    message_chain: Vec<Diagnostic>,
    related_information: Vec<Diagnostic>,
    reports_unnecessary: bool,
    reports_deprecated: bool,
) -> Diagnostic {
    Diagnostic {
        file,
        pos: loc.pos(),
        end: loc.end(),
        code,
        category,
        source: String::new(),
        message: new_ad_hoc_message(text),
        message_text: String::new(),
        message_args: Vec::new(),
        message_chain,
        related_information,
        reports_unnecessary,
        reports_deprecated,
        skipped_on_no_emit: false,
        repopulate_info: None,
    }
}

// Go: diagnostics/diagnostics.go:164 NewAdHocMessage
// PORT: a diagnostic holds a `&'static Message`, so each distinct text is
// leaked once and shared (Go allocates a new message per call). The key is
// "" (see `new_diagnostic_from_text`, the only user).
fn new_ad_hoc_message(message: &str) -> &'static crate::diagnostics::Message {
    static AD_HOC_MESSAGES: std::sync::LazyLock<
        std::sync::Mutex<FxHashMap<String, &'static crate::diagnostics::Message>>,
    > = std::sync::LazyLock::new(|| std::sync::Mutex::new(FxHashMap::default()));
    let mut messages = AD_HOC_MESSAGES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(existing) = messages.get(message) {
        return *existing;
    }
    let text: &'static str = Box::leak(message.to_string().into_boxed_str());
    let leaked: &'static crate::diagnostics::Message =
        Box::leak(Box::new(crate::diagnostics::Message::new(
            0,
            crate::diagnostics::Category::Error,
            "",
            text,
            false,
            false,
            false,
        )));
    messages.insert(message.to_string(), leaked);
    leaked
}

// Go: ast/diagnostic.go:218 NewDiagnostic
#[must_use]
pub fn new_diagnostic(
    file: Node,
    loc: TextRange,
    message: &'static crate::diagnostics::Message,
    args: Vec<String>,
) -> Diagnostic {
    Diagnostic {
        file,
        pos: loc.pos(),
        end: loc.end(),
        code: message.code() as i32,
        category: message.category(),
        source: String::new(),
        message,
        message_text: String::new(),
        message_args: args,
        message_chain: Vec::new(),
        related_information: Vec::new(),
        reports_unnecessary: message.reports_unnecessary(),
        reports_deprecated: message.reports_deprecated(),
        skipped_on_no_emit: false,
        repopulate_info: None,
    }
}

// Go: ast/diagnostic.go:232 NewDiagnosticChain
#[must_use]
pub fn new_diagnostic_chain(
    chain: Option<Diagnostic>,
    message: &'static crate::diagnostics::Message,
    args: Vec<String>,
) -> Diagnostic {
    if let Some(chain) = chain {
        let related_information = chain.related_information.clone();
        let mut result = new_diagnostic(chain.file, chain.loc(), message, args);
        result
            .add_message_chain(Some(chain))
            .set_related_info(related_information);
        return result;
    }
    new_diagnostic(Node::NIL, TextRange::new(0, 0), message, args)
}

// Go: ast/diagnostic.go:239 NewCompilerDiagnostic
// PORT: Go `core.UndefinedTextRange()` is `TextRange{-1, -1}`.
#[must_use]
pub fn new_compiler_diagnostic(
    message: &'static crate::diagnostics::Message,
    args: Vec<String>,
) -> Diagnostic {
    new_diagnostic(Node::NIL, TextRange::new(-1, -1), message, args)
}

// Go: ast/diagnostic.go:247 NewExternalDiagnostic (tsgo#4712)
// NewExternalDiagnostic creates a diagnostic reported by an external source such as a content mapper.
// The message text is already localized (the external source owns localization) and the code is shown
// with the given source prefix (e.g. "vue") instead of "TS". The location refers to the file's original,
// untransformed content.
// PORT: Go `file *SourceFile` is the SourceFile node (nil is `Node::NIL`).
// The Go nil message is `NIL_MESSAGE`.
#[must_use]
pub fn new_external_diagnostic(
    file: Node,
    loc: TextRange,
    source: &str,
    category: crate::diagnostics::Category,
    code: i32,
    message_text: &str,
) -> Diagnostic {
    Diagnostic {
        file,
        pos: loc.pos(),
        end: loc.end(),
        code,
        category,
        source: source.to_string(),
        message: NIL_MESSAGE,
        message_text: message_text.to_string(),
        message_args: Vec::new(),
        message_chain: Vec::new(),
        related_information: Vec::new(),
        reports_unnecessary: false,
        reports_deprecated: false,
        skipped_on_no_emit: false,
        repopulate_info: None,
    }
}

/// Go `ast.DiagnosticsCollection`. The mutex is dropped (single thread).
/// PORT: Go keeps `*Diagnostic` pointers in the file lists and the location
/// index, and `Add` and `Lookup` return the stored pointer, which callers
/// change later (for example, they add related information). Here the
/// collection owns each stored diagnostic in `diagnostics`, the lists and the
/// index hold its position there, and `add` and `lookup` return
/// `&mut Diagnostic`.
/// PORT: `file_diagnostics` is an `IndexMap` so `get_diagnostics` sees a
/// deterministic order before its sort. Go map order is random there.
/// Go keys the file lists and the location index by `file.Path()`
/// (tsgo#4901), so a replaced SourceFile of the same path shares its list.
/// Here the key is the file name: one program has one file name per path, so
/// the lists are the same.
/// PORT (diagfix1): Go's stored `*Diagnostic` keeps its `*SourceFile` alive,
/// so `Add` and the sorts can read the file name of a stored diagnostic at
/// any time. Here a freeable file version can die while a diagnostic of it
/// is stored (an API checker reads a symbol of a version that another
/// snapshot holds, and that snapshot is released), and a read of its name
/// then panics. So `add` keeps the name of each file that the stored
/// diagnostic and its related information point at (`stored_file_names`),
/// and the compares of stored diagnostics read the name there
/// (`stored_diagnostic_path`).
/// Related information that a caller adds to a stored diagnostic after
/// `add` is not in `stored_file_names`, so a compare reads its name from
/// its file. Only four callers can put it in another file:
/// `add_duplicate_declaration_error` (the other declarations), the
/// assertion call (the declarations of `get_type_of_dotted_name`), the
/// circular constraint (the node that the checker is at) and
/// `invocation_error_recovery` (the import, for `invocation_error` and the
/// decorator call). `error_and_maybe_suggest_await` puts it in the
/// diagnostic's own file, the regular expression errors give it no file,
/// and `add_related_info_to_reported_diagnostic` runs only in deferred
/// diagnostics. No path reads such a name after its file dies
/// (followups31):
/// - Go `Add` (ast/diagnostic.go:269) compares a new diagnostic of any
///   caller with each stored one of the same path, location and code.
///   `EqualDiagnostics` reads the related information only when all else
///   is equal, the message chain too, and the two lists have the same
///   length (`slices.EqualFunc`). The new diagnostic has only the related
///   information that its caller added before `add`, and with an equal
///   message and chain, that is the part that the stored diagnostic had at
///   its own `add`: no caller gives the duplicate declaration codes, the
///   assertion code or the circular constraint code related information
///   before `add`, and an invocation error gets the semicolon note of its
///   node and the await note of its apparent type, whose text is in the
///   chain. So when the stored diagnostic has later information, the
///   lengths are not equal, and `add` reads none of it.
/// - `lookup` and the sorts compare stored diagnostics, and two of them
///   can differ only in their later information (a second duplicate
///   declaration error at a node does not equal the first one when the
///   first one has information). They run only in a check of a file
///   (`check_source_file`), in `get_diagnostics` and in
///   `get_global_diagnostics`, on a checker whose program holds every file
///   of its diagnostics.
/// - The API's persistent checker, the only one that gets symbols of other
///   file versions (`api::checker_symbol`), runs no `lookup` and no sort.
///
/// A change to any of these must also keep the names of the later
/// related information.
#[derive(Clone, Debug, Default)]
pub struct DiagnosticsCollection {
    pub count: i32,
    diagnostics: Vec<Diagnostic>,
    file_diagnostics: IndexMap<&'static str, Vec<usize>>,
    file_diagnostics_sorted: FxHashSet<&'static str>,
    non_file_diagnostics: Vec<usize>,
    non_file_diagnostics_sorted: bool,
    // #4825: the stored diagnostics by location, for the `add` dedup.
    diagnostic_index: FxHashMap<DiagnosticLocationKey, usize>,
    diagnostic_collisions: FxHashMap<DiagnosticLocationKey, Vec<usize>>,
    // diagfix1: the file name of each source file node that a stored
    // diagnostic or its related information points at. Empty in a process
    // that frees no file version (`any_freeable_published`). Not the
    // `file_names` method, which gives the keys of `file_diagnostics`.
    stored_file_names: FxHashMap<Node, &'static str>,
}

impl DiagnosticsCollection {
    // Go: ast/diagnostic.go:269 Add
    // #4825: returns the stored diagnostic: an equal one that is already
    // stored, or `diagnostic`.
    pub fn add(&mut self, diagnostic: Diagnostic) -> &mut Diagnostic {
        let key = get_diagnostic_location_key(&diagnostic);
        if let Some(existing) = self.find_equal(key, &diagnostic) {
            return &mut self.diagnostics[existing];
        }
        let id = self.diagnostics.len();
        match self.diagnostic_index.entry(key) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(id);
            }
            std::collections::hash_map::Entry::Occupied(_) => {
                self.diagnostic_collisions.entry(key).or_default().push(id);
            }
        }

        self.count += 1;

        if diagnostic.file().is_some() {
            // The key path is the file name (`get_diagnostic_path`).
            let path = key.path;
            self.file_diagnostics.entry(path).or_default().push(id);
            self.file_diagnostics_sorted.remove(path);
        } else {
            self.non_file_diagnostics.push(id);
            self.non_file_diagnostics_sorted = false;
        }
        if super::file_version::any_freeable_published() {
            note_file_names(&mut self.stored_file_names, &diagnostic);
        }
        self.diagnostics.push(diagnostic);
        &mut self.diagnostics[id]
    }

    // The equal-diagnostic search at the start of Go `Add` (#4825).
    // PORT: split out so `add` can return the stored entry after the search
    // borrows end.
    fn find_equal(&self, key: DiagnosticLocationKey, diagnostic: &Diagnostic) -> Option<usize> {
        let path = |d: &Diagnostic| stored_diagnostic_path(&self.stored_file_names, d);
        let existing = *self.diagnostic_index.get(&key)?;
        if equal_diagnostics_by(&self.diagnostics[existing], diagnostic, &path) {
            return Some(existing);
        }
        self.diagnostic_collisions
            .get(&key)?
            .iter()
            .copied()
            .find(|&collision| {
                equal_diagnostics_by(&self.diagnostics[collision], diagnostic, &path)
            })
    }

    // Go: ast/diagnostic.go:330 Lookup
    // PORT: returns the stored diagnostic (Go returns the pointer).
    // `diagnostic` can be a copy of a stored one, so its path is read as a
    // stored one's.
    pub fn lookup(&mut self, diagnostic: &Diagnostic) -> Option<&mut Diagnostic> {
        let diagnostics = if diagnostic.file().is_some() {
            self.get_diagnostics_for_file_locked(stored_diagnostic_path(
                &self.stored_file_names,
                diagnostic,
            ))
        } else {
            self.get_global_diagnostics_locked()
        };
        let path = |d: &Diagnostic| stored_diagnostic_path(&self.stored_file_names, d);
        // Go slices.BinarySearchFunc: the first index where cmp >= 0.
        let i = diagnostics.partition_point(|&d| {
            compare_diagnostics_by(&self.diagnostics[d], diagnostic, &path) < 0
        });
        if i < diagnostics.len()
            && compare_diagnostics_by(&self.diagnostics[diagnostics[i]], diagnostic, &path) == 0
        {
            return Some(&mut self.diagnostics[diagnostics[i]]);
        }
        None
    }

    // Go: ast/diagnostic.go:346 GetGlobalDiagnostics
    pub fn get_global_diagnostics(&mut self) -> Vec<Diagnostic> {
        let ids = self.get_global_diagnostics_locked();
        ids.iter().map(|&id| self.diagnostics[id].clone()).collect()
    }

    // Go: ast/diagnostic.go:353 getGlobalDiagnosticsLocked
    // PORT: returns positions in `diagnostics` (Go returns the pointers).
    fn get_global_diagnostics_locked(&mut self) -> Vec<usize> {
        if !self.non_file_diagnostics_sorted {
            // Go: ast/diagnostic.go:331 slices.SortStableFunc(c.nonFileDiagnostics, CompareDiagnostics)
            sort_diagnostic_ids(
                &mut self.non_file_diagnostics,
                &self.diagnostics,
                &self.stored_file_names,
            );
            self.non_file_diagnostics_sorted = true;
        }
        self.non_file_diagnostics.clone()
    }

    // Go: ast/diagnostic.go:361 GetDiagnosticsForFile
    // #4825: takes the source file, not its name.
    pub fn get_diagnostics_for_file(&mut self, file: Node) -> Vec<Diagnostic> {
        self.get_diagnostics_for_file_name(source_file_file_name(file))
    }

    /// `get_diagnostics_for_file` for the file named `name` (see
    /// `file_names`).
    pub fn get_diagnostics_for_file_name(&mut self, name: &'static str) -> Vec<Diagnostic> {
        let ids = self.get_diagnostics_for_file_locked(name);
        ids.iter().map(|&id| self.diagnostics[id].clone()).collect()
    }

    /// The names of the files that have diagnostics (Go `fileDiagnostics`
    /// keys), in the order of their first diagnostic.
    pub fn file_names(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.file_diagnostics.keys().copied()
    }

    // Go: ast/diagnostic.go:368 getDiagnosticsForFileLocked
    // PORT: returns positions in `diagnostics` (Go returns the pointers).
    // Takes the file name, the key of the lists.
    fn get_diagnostics_for_file_locked(&mut self, path: &'static str) -> Vec<usize> {
        if !self.file_diagnostics_sorted.contains(path) {
            if let Some(ids) = self.file_diagnostics.get_mut(path) {
                // Go: ast/diagnostic.go:347 slices.SortStableFunc(c.fileDiagnostics[path], CompareDiagnostics)
                sort_diagnostic_ids(ids, &self.diagnostics, &self.stored_file_names);
            }
            self.file_diagnostics_sorted.insert(path);
        }
        self.file_diagnostics.get(path).cloned().unwrap_or_default()
    }

    // Go: ast/diagnostic.go:377 GetDiagnostics
    #[must_use]
    pub fn get_diagnostics(&self) -> Vec<Diagnostic> {
        let mut diagnostics: Vec<Diagnostic> = Vec::with_capacity(self.count as usize);
        let lists =
            std::iter::once(&self.non_file_diagnostics).chain(self.file_diagnostics.values());
        for ids in lists {
            diagnostics.extend(ids.iter().map(|&id| self.diagnostics[id].clone()));
        }
        // Go: ast/diagnostic.go:362 slices.SortFunc(diagnostics, CompareDiagnostics)
        let path = |d: &Diagnostic| stored_diagnostic_path(&self.stored_file_names, d);
        crate::gostd::slices::sort_func(&mut diagnostics, |a, b| {
            compare_diagnostics_by(a, b, &path)
        });
        diagnostics
    }
}

// Go `slices.SortStableFunc(list, CompareDiagnostics)` on a list of
// positions in `diagnostics`, with the paths of `stored_file_names`
// (`stored_diagnostic_path`).
fn sort_diagnostic_ids(
    ids: &mut [usize],
    diagnostics: &[Diagnostic],
    stored_file_names: &FxHashMap<Node, &'static str>,
) {
    let path = |d: &Diagnostic| stored_diagnostic_path(stored_file_names, d);
    crate::gostd::slices::sort_stable_func(ids, |&a, &b| {
        compare_diagnostics_by(&diagnostics[a], &diagnostics[b], &path)
    });
}

/// diagfix1: adds the file names of `diagnostic` and of its related
/// information (the files whose names `equal_diagnostics` and
/// `compare_diagnostics` read) to `stored_file_names`. Called when
/// `diagnostic` is stored, while its files are alive. A factory-made
/// source file is left out: its name is read from it, as before.
fn note_file_names(stored_file_names: &mut FxHashMap<Node, &'static str>, diagnostic: &Diagnostic) {
    let file = diagnostic.file();
    if file.is_some() && !super::is_synthetic_node(file) {
        stored_file_names
            .entry(file)
            .or_insert_with(|| source_file_file_name(file));
    }
    for related in diagnostic.related_information() {
        note_file_names(stored_file_names, related);
    }
}

/// Go `getDiagnosticPath` of a stored diagnostic, or of a copy of one:
/// the name that `add` kept for its file (`note_file_names`), so it does
/// not read a file version that died after `add`. Else the name of the
/// file.
fn stored_diagnostic_path(
    stored_file_names: &FxHashMap<Node, &'static str>,
    d: &Diagnostic,
) -> &'static str {
    if !stored_file_names.is_empty()
        && let Some(&name) = stored_file_names.get(&d.file())
    {
        return name;
    }
    get_diagnostic_path(d)
}

// Go: ast/diagnostic.go:312 diagnosticLocationKey (#4825)
// PORT: `path` is the file name (see `DiagnosticsCollection`). Go `loc` is
// `pos` and `end`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct DiagnosticLocationKey {
    path: &'static str,
    pos: i32,
    end: i32,
    code: i32,
}

// Go: ast/diagnostic.go:318 getDiagnosticLocationKey (#4825)
// PORT: `get_diagnostic_path` is the Go path part: the file name, or "" when
// there is no file.
fn get_diagnostic_location_key(diagnostic: &Diagnostic) -> DiagnosticLocationKey {
    DiagnosticLocationKey {
        path: get_diagnostic_path(diagnostic),
        pos: diagnostic.pos(),
        end: diagnostic.end(),
        code: diagnostic.code(),
    }
}

// Go: ast/diagnostic.go:390 getDiagnosticPath
// PORT: returns `&'static str` (file names live for the program).
#[must_use]
pub fn get_diagnostic_path(d: &Diagnostic) -> &'static str {
    if d.file().is_some() {
        return source_file_file_name(d.file());
    }
    ""
}

// Go: ast/diagnostic.go:397 EqualDiagnostics
#[must_use]
pub fn equal_diagnostics(d1: &Diagnostic, d2: &Diagnostic) -> bool {
    equal_diagnostics_by(d1, d2, &get_diagnostic_path)
}

/// `equal_diagnostics` with Go `getDiagnosticPath` read by `path`
/// (`stored_diagnostic_path` in `DiagnosticsCollection`).
fn equal_diagnostics_by<P: Fn(&Diagnostic) -> &'static str>(
    d1: &Diagnostic,
    d2: &Diagnostic,
    path: &P,
) -> bool {
    if std::ptr::eq(d1, d2) {
        return true;
    }
    equal_diagnostics_no_related_info_by(d1, d2, path)
        && d1.related_information().len() == d2.related_information().len()
        && d1
            .related_information()
            .iter()
            .zip(d2.related_information())
            .all(|(a, b)| equal_diagnostics_by(a, b, path))
}

// Go: ast/diagnostic.go:405 EqualDiagnosticsNoRelatedInfo
#[must_use]
pub fn equal_diagnostics_no_related_info(d1: &Diagnostic, d2: &Diagnostic) -> bool {
    equal_diagnostics_no_related_info_by(d1, d2, &get_diagnostic_path)
}

/// `equal_diagnostics_no_related_info` with Go `getDiagnosticPath` read by
/// `path`.
fn equal_diagnostics_no_related_info_by<P: Fn(&Diagnostic) -> &'static str>(
    d1: &Diagnostic,
    d2: &Diagnostic,
    path: &P,
) -> bool {
    if std::ptr::eq(d1, d2) {
        return true;
    }
    path(d1) == path(d2) && equal_diagnostics_no_related_info_after_path(d1, d2)
}

/// `equal_diagnostics_no_related_info` of two diagnostics whose paths
/// (`get_diagnostic_path`) are equal: the checks after the path.
/// `program::sort_and_deduplicate_diagnostics` reads each file name once.
#[must_use]
pub fn equal_diagnostics_no_related_info_after_path(d1: &Diagnostic, d2: &Diagnostic) -> bool {
    d1.pos() == d2.pos()
        && d1.end() == d2.end()
        && d1.code() == d2.code()
        // tsgo#4712
        && d1.category() == d2.category()
        && d1.source() == d2.source()
        // #4825
        && get_diagnostic_message_identity(d1) == get_diagnostic_message_identity(d2)
        && d1.message_args() == d2.message_args()
        && d1.message_chain().len() == d2.message_chain().len()
        && d1
            .message_chain()
            .iter()
            .zip(d2.message_chain())
            .all(|(a, b)| equal_message_chain(a, b))
}

// Go: ast/diagnostic.go:419 getDiagnosticMessageIdentity (#4825, tsgo#4712)
// PORT: a port diagnostic has its message, or `NIL_MESSAGE` for the Go nil
// message, whose text and key are both "" (see
// `new_diagnostic_from_serialized`). Go `message.String()` is the message
// text.
fn get_diagnostic_message_identity(diagnostic: &Diagnostic) -> &str {
    if !diagnostic.message_text().is_empty() {
        return diagnostic.message_text();
    }
    if diagnostic.code() == -1 {
        return diagnostic.message.text();
    }
    diagnostic.message_key()
}

// Go: ast/diagnostic.go:429 equalMessageChain
fn equal_message_chain(c1: &Diagnostic, c2: &Diagnostic) -> bool {
    if std::ptr::eq(c1, c2) {
        return true;
    }
    c1.code() == c2.code()
        && c1.message_args() == c2.message_args()
        && c1.message_chain().len() == c2.message_chain().len()
        && c1
            .message_chain()
            .iter()
            .zip(c2.message_chain())
            .all(|(a, b)| equal_message_chain(a, b))
}

// Go `slices.Compare` / `strings.Compare` as -1, 0, 1.
fn ordering_to_int(ordering: std::cmp::Ordering) -> i32 {
    match ordering {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    }
}

// Go: ast/diagnostic.go:438 compareMessageChainSize
fn compare_message_chain_size(c1: &[Diagnostic], c2: &[Diagnostic]) -> i32 {
    let mut c = c2.len() as i32 - c1.len() as i32;
    if c != 0 {
        return c;
    }
    for i in 0..c1.len() {
        c = compare_message_chain_size(c1[i].message_chain(), c2[i].message_chain());
        if c != 0 {
            return c;
        }
    }
    0
}

// Go: ast/diagnostic.go:452 compareMessageChainContent
fn compare_message_chain_content(c1: &[Diagnostic], c2: &[Diagnostic]) -> i32 {
    for i in 0..c1.len() {
        let mut c = ordering_to_int(compare_go_bytes_slices(
            c1[i].message_args(),
            c2[i].message_args(),
        ));
        if c != 0 {
            return c;
        }
        // PORT: Go checks `!= nil`; an empty chain recurses over nothing and
        // returns 0, so `!is_empty()` is equivalent.
        if !c1[i].message_chain().is_empty() {
            c = compare_message_chain_content(c1[i].message_chain(), c2[i].message_chain());
            if c != 0 {
                return c;
            }
        }
    }
    0
}

// Go: ast/diagnostic.go:468 compareRelatedInfo
// PORT: Go `getDiagnosticPath` is read by `path` (`compare_diagnostics_by`).
fn compare_related_info<P: Fn(&Diagnostic) -> &'static str>(
    r1: &[Diagnostic],
    r2: &[Diagnostic],
    path: &P,
) -> i32 {
    let mut c = r2.len() as i32 - r1.len() as i32;
    if c != 0 {
        return c;
    }
    for i in 0..r1.len() {
        c = compare_diagnostics_by(&r1[i], &r2[i], path);
        if c != 0 {
            return c;
        }
    }
    0
}

// Go: ast/diagnostic.go:482 CompareDiagnostics
#[must_use]
pub fn compare_diagnostics(d1: &Diagnostic, d2: &Diagnostic) -> i32 {
    compare_diagnostics_by(d1, d2, &get_diagnostic_path)
}

/// `compare_diagnostics` with Go `getDiagnosticPath` read by `path`
/// (`stored_diagnostic_path` in `DiagnosticsCollection`).
fn compare_diagnostics_by<P: Fn(&Diagnostic) -> &'static str>(
    d1: &Diagnostic,
    d2: &Diagnostic,
    path: &P,
) -> i32 {
    if std::ptr::eq(d1, d2) {
        return 0;
    }
    // PORT: Go compares the bytes of the strings, which are port forms here
    // (see `scanner_util::compare_go_bytes`).
    let c = ordering_to_int(compare_go_bytes(path(d1), path(d2)));
    if c != 0 {
        return c;
    }
    compare_diagnostics_after_path_by(d1, d2, path)
}

/// `compare_diagnostics` of two diagnostics whose paths
/// (`get_diagnostic_path`) compare equal: the compares after the path.
/// `program::sort_and_deduplicate_diagnostics` ranks each file name once.
#[must_use]
pub fn compare_diagnostics_after_path(d1: &Diagnostic, d2: &Diagnostic) -> i32 {
    compare_diagnostics_after_path_by(d1, d2, &get_diagnostic_path)
}

/// `compare_diagnostics_by` after the path compare.
fn compare_diagnostics_after_path_by<P: Fn(&Diagnostic) -> &'static str>(
    d1: &Diagnostic,
    d2: &Diagnostic,
    path: &P,
) -> i32 {
    // PORT: Go subtracts these int32 values as Go ints (64 bits), so the
    // sign is their true order. An `i32` difference can wrap on the values
    // of a bad `.tsbuildinfo` (category -2147483648 and 1), so the port
    // compares them.
    let mut c = ordering_to_int(d1.pos().cmp(&d2.pos()));
    if c != 0 {
        return c;
    }
    c = ordering_to_int(d1.end().cmp(&d2.end()));
    if c != 0 {
        return c;
    }
    c = ordering_to_int(d1.code().cmp(&d2.code()));
    if c != 0 {
        return c;
    }
    // tsgo#4712
    c = ordering_to_int(d1.category().cmp(&d2.category()));
    if c != 0 {
        return c;
    }
    // PORT: sources, keys and message texts are plain UTF-8 (not port
    // forms: external sources and texts come from JSON, which Go decodes to
    // valid UTF-8), so the `str` order is the Go byte order.
    c = ordering_to_int(d1.source().cmp(d2.source()));
    if c != 0 {
        return c;
    }
    // #4825
    c = ordering_to_int(
        get_diagnostic_message_identity(d1).cmp(get_diagnostic_message_identity(d2)),
    );
    if c != 0 {
        return c;
    }
    c = ordering_to_int(compare_go_bytes_slices(
        d1.message_args(),
        d2.message_args(),
    ));
    if c != 0 {
        return c;
    }
    c = compare_message_chain_size(d1.message_chain(), d2.message_chain());
    if c != 0 {
        return c;
    }
    c = compare_message_chain_content(d1.message_chain(), d2.message_chain());
    if c != 0 {
        return c;
    }
    compare_related_info(d1.related_information(), d2.related_information(), path)
}

// ---------------------------------------------------------------------------
// precedence.go (OperatorPrecedence, OperatorPrecedenceFlags and
// TypePrecedence consts are in `crate::flags`)
// ---------------------------------------------------------------------------

// Go: ast/precedence.go:189 getOperator
fn get_operator(expression: Node) -> SyntaxKind {
    match expression.kind() {
        SyntaxKind::BinaryExpression => expression.operator_token().kind(),
        SyntaxKind::PrefixUnaryExpression => expression.operator(),
        SyntaxKind::PostfixUnaryExpression => expression.operator(),
        _ => expression.kind(),
    }
}

// Go: ast/precedence.go:203 GetExpressionPrecedence
// Gets the precedence of an expression
#[must_use]
pub fn get_expression_precedence(expression: Node) -> OperatorPrecedence {
    let operator = get_operator(expression);
    let mut flags = OperatorPrecedenceFlags::NONE;
    if expression.kind() == SyntaxKind::NewExpression && expression.argument_list().is_nil() {
        flags = OperatorPrecedenceFlags::NEW_WITHOUT_ARGUMENTS;
    } else if is_optional_chain(expression) {
        flags = OperatorPrecedenceFlags::OPTIONAL_CHAIN;
    }
    get_operator_precedence(expression.kind(), operator, flags)
}

// Go: ast/precedence.go:223 GetOperatorPrecedence
// Gets the precedence of an operator
#[must_use]
pub fn get_operator_precedence(
    node_kind: SyntaxKind,
    operator_kind: SyntaxKind,
    flags: OperatorPrecedenceFlags,
) -> OperatorPrecedence {
    match node_kind {
        SyntaxKind::SpreadElement => OperatorPrecedence::SPREAD,
        SyntaxKind::YieldExpression => OperatorPrecedence::YIELD,
        // !!! By necessity, this differs from the old compiler to better align with ParenthesizerRules. consider backporting
        SyntaxKind::ArrowFunction => OperatorPrecedence::ASSIGNMENT,
        SyntaxKind::ConditionalExpression => OperatorPrecedence::CONDITIONAL,
        SyntaxKind::BinaryExpression => match operator_kind {
            SyntaxKind::CommaToken => OperatorPrecedence::COMMA,

            SyntaxKind::EqualsToken
            | SyntaxKind::PlusEqualsToken
            | SyntaxKind::MinusEqualsToken
            | SyntaxKind::AsteriskAsteriskEqualsToken
            | SyntaxKind::AsteriskEqualsToken
            | SyntaxKind::SlashEqualsToken
            | SyntaxKind::PercentEqualsToken
            | SyntaxKind::LessThanLessThanEqualsToken
            | SyntaxKind::GreaterThanGreaterThanEqualsToken
            | SyntaxKind::GreaterThanGreaterThanGreaterThanEqualsToken
            | SyntaxKind::AmpersandEqualsToken
            | SyntaxKind::CaretEqualsToken
            | SyntaxKind::BarEqualsToken
            | SyntaxKind::BarBarEqualsToken
            | SyntaxKind::AmpersandAmpersandEqualsToken
            | SyntaxKind::QuestionQuestionEqualsToken => OperatorPrecedence::ASSIGNMENT,

            _ => get_binary_operator_precedence(operator_kind),
        },
        // TODO: Should prefix `++` and `--` be moved to the `Update` precedence?
        SyntaxKind::TypeAssertionExpression
        | SyntaxKind::NonNullExpression
        | SyntaxKind::PrefixUnaryExpression
        | SyntaxKind::TypeOfExpression
        | SyntaxKind::VoidExpression
        | SyntaxKind::DeleteExpression
        | SyntaxKind::AwaitExpression => OperatorPrecedence::UNARY,

        SyntaxKind::PostfixUnaryExpression => OperatorPrecedence::UPDATE,

        // !!! By necessity, this differs from the old compiler to better align with ParenthesizerRules. consider backporting
        SyntaxKind::PropertyAccessExpression | SyntaxKind::ElementAccessExpression => {
            if flags.intersects(OperatorPrecedenceFlags::OPTIONAL_CHAIN) {
                return OperatorPrecedence::OPTIONAL_CHAIN;
            }
            OperatorPrecedence::MEMBER
        }

        SyntaxKind::CallExpression => {
            if flags.intersects(OperatorPrecedenceFlags::OPTIONAL_CHAIN) {
                return OperatorPrecedence::OPTIONAL_CHAIN;
            }
            OperatorPrecedence::MEMBER
        }

        // !!! By necessity, this differs from the old compiler to better align with ParenthesizerRules. consider backporting
        SyntaxKind::NewExpression => {
            if flags.intersects(OperatorPrecedenceFlags::NEW_WITHOUT_ARGUMENTS) {
                return OperatorPrecedence::LEFT_HAND_SIDE;
            }
            OperatorPrecedence::MEMBER
        }

        // !!! By necessity, this differs from the old compiler to better align with ParenthesizerRules. consider backporting
        SyntaxKind::TaggedTemplateExpression
        | SyntaxKind::MetaProperty
        | SyntaxKind::ExpressionWithTypeArguments => OperatorPrecedence::MEMBER,

        SyntaxKind::AsExpression | SyntaxKind::SatisfiesExpression => {
            OperatorPrecedence::RELATIONAL
        }

        SyntaxKind::ThisKeyword
        | SyntaxKind::SuperKeyword
        | SyntaxKind::ImportKeyword
        | SyntaxKind::Identifier
        | SyntaxKind::PrivateIdentifier
        | SyntaxKind::NullKeyword
        | SyntaxKind::TrueKeyword
        | SyntaxKind::FalseKeyword
        | SyntaxKind::NumericLiteral
        | SyntaxKind::BigIntLiteral
        | SyntaxKind::StringLiteral
        | SyntaxKind::ArrayLiteralExpression
        | SyntaxKind::ObjectLiteralExpression
        | SyntaxKind::FunctionExpression
        | SyntaxKind::ClassExpression
        | SyntaxKind::RegularExpressionLiteral
        | SyntaxKind::NoSubstitutionTemplateLiteral
        | SyntaxKind::TemplateExpression
        | SyntaxKind::OmittedExpression
        | SyntaxKind::JsxElement
        | SyntaxKind::JsxSelfClosingElement
        | SyntaxKind::JsxFragment
        | SyntaxKind::MissingDeclaration => OperatorPrecedence::PRIMARY,

        // !!! By necessity, this differs from the old compiler to support emit. consider backporting
        SyntaxKind::ParenthesizedExpression => OperatorPrecedence::PARENTHESES,

        _ => OperatorPrecedence::INVALID,
    }
}

// Go: ast/precedence.go:336 GetBinaryOperatorPrecedence
// Gets the precedence of a binary operator
#[must_use]
pub fn get_binary_operator_precedence(operator_kind: SyntaxKind) -> OperatorPrecedence {
    match operator_kind {
        SyntaxKind::QuestionQuestionToken => return OperatorPrecedence::COALESCE,
        SyntaxKind::BarBarToken => return OperatorPrecedence::LOGICAL_OR,
        SyntaxKind::AmpersandAmpersandToken => return OperatorPrecedence::LOGICAL_AND,
        SyntaxKind::BarToken => return OperatorPrecedence::BITWISE_OR,
        SyntaxKind::CaretToken => return OperatorPrecedence::BITWISE_XOR,
        SyntaxKind::AmpersandToken => return OperatorPrecedence::BITWISE_AND,
        SyntaxKind::EqualsEqualsToken
        | SyntaxKind::ExclamationEqualsToken
        | SyntaxKind::EqualsEqualsEqualsToken
        | SyntaxKind::ExclamationEqualsEqualsToken => return OperatorPrecedence::EQUALITY,
        SyntaxKind::LessThanToken
        | SyntaxKind::GreaterThanToken
        | SyntaxKind::LessThanEqualsToken
        | SyntaxKind::GreaterThanEqualsToken
        | SyntaxKind::InstanceOfKeyword
        | SyntaxKind::InKeyword
        | SyntaxKind::AsKeyword
        | SyntaxKind::SatisfiesKeyword => return OperatorPrecedence::RELATIONAL,
        SyntaxKind::LessThanLessThanToken
        | SyntaxKind::GreaterThanGreaterThanToken
        | SyntaxKind::GreaterThanGreaterThanGreaterThanToken => return OperatorPrecedence::SHIFT,
        SyntaxKind::PlusToken | SyntaxKind::MinusToken => return OperatorPrecedence::ADDITIVE,
        SyntaxKind::AsteriskToken | SyntaxKind::SlashToken | SyntaxKind::PercentToken => {
            return OperatorPrecedence::MULTIPLICATIVE;
        }
        SyntaxKind::AsteriskAsteriskToken => return OperatorPrecedence::EXPONENTIATION,
        _ => {}
    }
    // -1 is lower than all other precedences.  Returning it will cause binary expression
    // parsing to stop.
    OperatorPrecedence::INVALID
}

// Go: ast/precedence.go:370 GetLeftmostExpression
// Gets the leftmost expression of an expression, e.g. `a` in `a.b`, `a[b]`, `a++`, `a+b`, `a?b:c`, `a as B`, etc.
#[must_use]
pub fn get_leftmost_expression(mut node: Node, stop_at_call_expressions: bool) -> Node {
    loop {
        match node.kind() {
            SyntaxKind::PostfixUnaryExpression => {
                node = node.operand();
                continue;
            }
            SyntaxKind::BinaryExpression => {
                node = node.left();
                continue;
            }
            SyntaxKind::ConditionalExpression => {
                node = node.condition();
                continue;
            }
            SyntaxKind::TaggedTemplateExpression => {
                node = node.tag();
                continue;
            }
            SyntaxKind::CallExpression
            | SyntaxKind::AsExpression
            | SyntaxKind::ElementAccessExpression
            | SyntaxKind::PropertyAccessExpression
            | SyntaxKind::NonNullExpression
            | SyntaxKind::PartiallyEmittedExpression
            | SyntaxKind::SatisfiesExpression => {
                // Go: `case KindCallExpression: if stopAtCallExpressions { return node }; fallthrough`
                if node.kind() == SyntaxKind::CallExpression && stop_at_call_expressions {
                    return node;
                }
                node = node.expression();
                continue;
            }
            _ => {}
        }
        return node;
    }
}

// Go: ast/precedence.go:655 GetTypeNodePrecedence
// Gets the precedence of a TypeNode
#[must_use]
pub fn get_type_node_precedence(n: Node) -> TypePrecedence {
    match n.kind() {
        SyntaxKind::ConditionalType => TypePrecedence::CONDITIONAL,
        SyntaxKind::JsDocOptionalType | SyntaxKind::JsDocVariadicType => TypePrecedence::JS_DOC,
        SyntaxKind::FunctionType | SyntaxKind::ConstructorType => TypePrecedence::FUNCTION,
        SyntaxKind::UnionType => TypePrecedence::UNION,
        SyntaxKind::IntersectionType => TypePrecedence::INTERSECTION,
        SyntaxKind::TypeOperator => TypePrecedence::TYPE_OPERATOR,
        SyntaxKind::InferType => {
            if n.type_parameter().constraint().is_some() {
                // `infer T extends U` must be treated as FunctionTypeNode precedence as the `extends` clause eagerly consumes
                // TypeNode
                return TypePrecedence::FUNCTION;
            }
            TypePrecedence::TYPE_OPERATOR
        }
        SyntaxKind::IndexedAccessType | SyntaxKind::ArrayType | SyntaxKind::OptionalType => TypePrecedence::POSTFIX,
        SyntaxKind::TypeQuery => {
            // TypeQueryNode is actually a NonArrayType, but we treat it as TypeOperatorNode
            // precedence so that it is parenthesized when used in a PostfixType
            // context (e.g., `(typeof C)[]` instead of `typeof C[]`)
            TypePrecedence::TYPE_OPERATOR
        }
        SyntaxKind::AnyKeyword
        | SyntaxKind::UnknownKeyword
        | SyntaxKind::StringKeyword
        | SyntaxKind::NumberKeyword
        | SyntaxKind::BigIntKeyword
        | SyntaxKind::SymbolKeyword
        | SyntaxKind::BooleanKeyword
        | SyntaxKind::UndefinedKeyword
        | SyntaxKind::NeverKeyword
        | SyntaxKind::ObjectKeyword
        | SyntaxKind::IntrinsicKeyword
        | SyntaxKind::VoidKeyword
        | SyntaxKind::JsDocAllType
        | SyntaxKind::JsDocNullableType
        | SyntaxKind::JsDocNonNullableType
        | SyntaxKind::LiteralType
        | SyntaxKind::TypePredicate
        | SyntaxKind::TypeReference
        | SyntaxKind::TypeLiteral
        | SyntaxKind::TupleType
        | SyntaxKind::RestType
        | SyntaxKind::ParenthesizedType
        | SyntaxKind::ThisType
        | SyntaxKind::MappedType
        | SyntaxKind::NamedTupleMember
        | SyntaxKind::TemplateLiteralType
        | SyntaxKind::ImportType
        // These occur in pseudo-types like `f<T>.C`, where `f` is a generic function and `C` is a local type
        | SyntaxKind::PropertyAccessExpression
        | SyntaxKind::ExpressionWithTypeArguments => TypePrecedence::NON_ARRAY,
        // Go `%v` of a Kind is `Kind.String()`.
        kind => panic!(
            "unhandled TypeNode: {}",
            crate::gostd::debug::kind_string(kind)
        ),
    }
}

// ---------------------------------------------------------------------------
// modifierflags.go, checkflags.go, symbolflags.go: consts only (in
// `crate::flags`).
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// flow.go (FlowFlags consts are in `crate::flags`; `FlowNode` is in
// `crate::core`, with `antecedents: Vec<FlowNodeId>` in place of Go `FlowList`)
// ---------------------------------------------------------------------------

/// Go `ast.FlowSwitchClauseData` (synthetic AST node for
/// `FlowFlags::SWITCH_CLAUSE`).
/// PORT: Go wraps this in a synthetic `*Node` of `KindUnknown` stored in
/// `FlowNode.Node`. `core::FlowNode.node` is a handle into parsed files and
/// cannot hold a synthetic node, so this is a plain value. The owner of the
/// flow node storage decides where it lives.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FlowSwitchClauseData {
    pub switch_statement: Node,
    pub clause_start: i32, // Start index of case/default clause range
    pub clause_end: i32,   // End index of case/default clause range
}

// Go: ast/flow.go:50 NewFlowSwitchClauseData
// PORT: returns the data value instead of a synthetic `*Node` (see the struct).
#[must_use]
pub fn new_flow_switch_clause_data(
    switch_statement: Node,
    clause_start: i32,
    clause_end: i32,
) -> FlowSwitchClauseData {
    FlowSwitchClauseData {
        switch_statement,
        clause_start,
        clause_end,
    }
}

impl FlowSwitchClauseData {
    // Go: ast/flow.go:58 IsEmpty
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.clause_start == self.clause_end
    }
}

/// Go `ast.FlowReduceLabelData` (synthetic AST node for
/// `FlowFlags::REDUCE_LABEL`).
/// PORT: plain value, like `FlowSwitchClauseData`. Go `*FlowList` becomes
/// `Vec<FlowNodeId>` in the same order.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FlowReduceLabelData {
    pub target: FlowNodeId,           // Target label
    pub antecedents: Vec<FlowNodeId>, // Temporary antecedent list
}

// Go: ast/flow.go:70 NewFlowReduceLabelData
// PORT: returns the data value instead of a synthetic `*Node` (see the struct).
#[must_use]
pub fn new_flow_reduce_label_data(
    target: FlowNodeId,
    antecedents: Vec<FlowNodeId>,
) -> FlowReduceLabelData {
    FlowReduceLabelData {
        target,
        antecedents,
    }
}

// ---------------------------------------------------------------------------
// functionflags.go (FunctionFlags consts are in `crate::flags`)
// ---------------------------------------------------------------------------

// Go: ast/functionflags.go:13 GetFunctionFlags
#[must_use]
pub fn get_function_flags(node: Node) -> FunctionFlags {
    if node.is_nil() {
        return FunctionFlags::INVALID;
    }
    // PORT: Go `node.BodyData()` is non-nil exactly for the kinds that embed
    // `BodyBase` (FunctionDeclaration, MethodDeclaration, Constructor,
    // Get/SetAccessor, FunctionExpression, ArrowFunction, ModuleDeclaration).
    // `data.AsteriskToken` / `data.Body` are read with the field accessors.
    let has_body_data = matches!(
        node.kind(),
        SyntaxKind::FunctionDeclaration
            | SyntaxKind::MethodDeclaration
            | SyntaxKind::Constructor
            | SyntaxKind::GetAccessor
            | SyntaxKind::SetAccessor
            | SyntaxKind::FunctionExpression
            | SyntaxKind::ArrowFunction
            | SyntaxKind::ModuleDeclaration
    );
    if !has_body_data {
        return FunctionFlags::INVALID;
    }
    let mut flags = FunctionFlags::NORMAL;
    match node.kind() {
        SyntaxKind::FunctionDeclaration
        | SyntaxKind::FunctionExpression
        | SyntaxKind::MethodDeclaration
        | SyntaxKind::ArrowFunction => {
            // Go: generator check, then `fallthrough` to the ArrowFunction case.
            if node.kind() != SyntaxKind::ArrowFunction && node.asterisk_token().is_some() {
                flags |= FunctionFlags::GENERATOR;
            }
            if has_syntactic_modifier(node, ModifierFlags::ASYNC) {
                flags |= FunctionFlags::ASYNC;
            }
        }
        _ => {}
    }
    if node.body().is_nil() {
        flags |= FunctionFlags::INVALID;
    }
    flags
}

// ---------------------------------------------------------------------------
// positionmap.go
// ---------------------------------------------------------------------------

/// Go `ast.PositionMap`: bidirectional mapping between UTF-8 byte offsets
/// (used by Go) and UTF-16 code unit offsets (used by JavaScript/TypeScript).
///
/// For ASCII-only text, the two are identical. For text containing non-ASCII
/// characters, the offsets diverge because multi-byte UTF-8 sequences map to
/// different numbers of UTF-16 code units:
///   - U+0000..U+007F:   1 byte  in UTF-8, 1 code unit  in UTF-16
///   - U+0080..U+07FF:   2 bytes in UTF-8, 1 code unit  in UTF-16
///   - U+0800..U+FFFF:   3 bytes in UTF-8, 1 code unit  in UTF-16
///   - U+10000..U+10FFFF: 4 bytes in UTF-8, 2 code units in UTF-16 (surrogate pair)
#[derive(Clone, Debug, Default)]
pub struct PositionMap {
    /// True if the text contains only ASCII characters, meaning UTF-8 byte
    /// offsets and UTF-16 code unit offsets are identical.
    pub ascii_only: bool,
    /// For each multi-byte character: the UTF-8 byte offset after it and the
    /// cumulative delta (utf8Offset - utf16Offset) through it. This allows
    /// O(log n) conversion in either direction.
    pub entries: Vec<PositionMapEntry>,
    /// PORT: the port offset of each rune whose Go bytes are in more than
    /// one unit of the port form (`SPLIT_RUNE_LEN`). Usually empty.
    split_runes: Vec<i32>,
}

/// PORT: the port size of the only Go rune that is more than one unit of the
/// port form: a WTF-8 lone surrogate in source text, which is 3 invalid byte
/// units of 7 bytes (see `scanner_util::GO_STRING_MARKER`). Every other
/// rune is at most one unit of at most 7 bytes. Go reads its 3 bytes as one
/// rune (`DecodeJSStringRune`), and a position k bytes into it is the end of
/// its k-th unit here.
const SPLIT_RUNE_UNIT_LEN: i32 = 7;
const SPLIT_RUNE_LEN: i32 = 3 * SPLIT_RUNE_UNIT_LEN;

/// Go `ast.positionMapEntry`.
#[derive(Clone, Copy, Debug, Default)]
pub struct PositionMapEntry {
    pub utf8_pos: i32, // UTF-8 byte offset AFTER this multi-byte character
    pub delta: i32,    // cumulative (utf8 - utf16) offset difference after this character
}

// Go: ast/positionmap.go:40 ComputePositionMap
// ComputePositionMap builds a PositionMap for the given text.
// PORT: `text` is the port form of the Go text (see
// `scanner_util::GO_STRING_MARKER`), and the "UTF-8" offsets are offsets in
// it. `decode_go_js_string_rune` reads one Go `DecodeJSStringRune` rune and
// gives its size in the port form, so each entry holds a port offset and the
// UTF-16 length of the Go rune. An invalid byte and a WTF-8 lone surrogate
// are each one UTF-16 unit, as in Go. A WTF-8 lone surrogate is 3 units
// here; `split_runes` keeps its offset for the positions inside it.
#[must_use]
pub fn compute_position_map(text: &str) -> PositionMap {
    let mut pm = PositionMap::default();
    let mut delta: i32 = 0;
    let bytes = text.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] < 0x80 {
            i += 1;
            continue;
        }
        let (r, size, _) = decode_go_js_string_rune(&text[i..]);
        let utf16_size: i32 = if r >= 0x10000 { 2 } else { 1 };
        if size as i32 > SPLIT_RUNE_UNIT_LEN {
            pm.split_runes.push(i as i32);
        }
        delta += size as i32 - utf16_size;
        pm.entries.push(PositionMapEntry {
            utf8_pos: (i + size) as i32,
            delta,
        });
        i += size;
    }
    pm.ascii_only = pm.entries.is_empty();
    pm
}

impl PositionMap {
    // Go: ast/positionmap.go:64 IsAsciiOnly
    // IsAsciiOnly returns true if the text is ASCII-only,
    // meaning UTF-8 and UTF-16 offsets are identical.
    #[must_use]
    pub fn is_ascii_only(&self) -> bool {
        self.ascii_only
    }

    // Go: ast/positionmap.go:69 UTF8ToUTF16
    // UTF8ToUTF16 converts a UTF-8 byte offset to a UTF-16 code unit offset.
    #[must_use]
    pub fn utf8_to_utf16(&self, utf8_offset: i32) -> i32 {
        if self.ascii_only {
            return utf8_offset;
        }
        // Binary search: find the last entry where utf8Pos <= utf8Offset
        let (mut lo, mut hi) = (0usize, self.entries.len());
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if self.entries[mid].utf8_pos <= utf8_offset {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        if lo == 0 {
            // Before any multi-byte character
            return self.split_rune_utf16(utf8_offset, 0);
        }
        self.split_rune_utf16(utf8_offset, self.entries[lo - 1].delta)
    }

    /// `utf8_offset - delta`, the Go result for an offset after the last
    /// multi-byte character before it, whose cumulative delta is `delta`.
    // PORT: an offset inside a split rune (`SPLIT_RUNE_LEN`) is k units into
    // it, and Go's offset is k bytes into it: Go's result is the UTF-16
    // offset of the rune plus k.
    #[inline]
    fn split_rune_utf16(&self, utf8_offset: i32, delta: i32) -> i32 {
        if !self.split_runes.is_empty() {
            let i = self
                .split_runes
                .partition_point(|&start| start < utf8_offset);
            if let Some(&start) = i.checked_sub(1).and_then(|i| self.split_runes.get(i))
                && utf8_offset - start < SPLIT_RUNE_LEN
            {
                return start - delta + (utf8_offset - start) / SPLIT_RUNE_UNIT_LEN;
            }
        }
        utf8_offset - delta
    }

    // Go: ast/positionmap.go:91 UTF16ToUTF8
    // UTF16ToUTF8 converts a UTF-16 code unit offset to a UTF-8 byte offset.
    #[must_use]
    pub fn utf16_to_utf8(&self, utf16_offset: i32) -> i32 {
        if self.ascii_only {
            return utf16_offset;
        }
        // We need the last entry where (utf8Pos - delta) <= utf16Offset.
        // (utf8Pos - delta) is the UTF-16 offset of that entry's character.
        let (mut lo, mut hi) = (0usize, self.entries.len());
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let utf16_pos = self.entries[mid].utf8_pos - self.entries[mid].delta;
            if utf16_pos <= utf16_offset {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        if lo == 0 {
            return utf16_offset;
        }
        utf16_offset + self.entries[lo - 1].delta
    }
}

// ---------------------------------------------------------------------------
// ids.go
// ---------------------------------------------------------------------------

// PORT: Go `ast.NodeId` and `ast.SymbolId` are plain `uint64` ids. The port
// uses the `core::Node` and `core::SymbolId` handles instead, so there is
// nothing to define here.

#[cfg(test)]
mod tests {
    use super::*;
    use std::panic::AssertUnwindSafe;

    // Go prints `%v` of the Kind in this panic (ast/precedence.go:715
    // `panic(fmt.Sprintf("unhandled TypeNode: %v", n.Kind))`), which is
    // `Kind.String()`. The API's printNode reaches it (gaps2a skeptic repro
    // r-typenode).
    #[test]
    fn unhandled_type_node_panic_names_the_go_kind() {
        let f = NodeFactory::new();
        let name = f.new_identifier("x");
        let payload = std::panic::catch_unwind(AssertUnwindSafe(|| get_type_node_precedence(name)))
            .expect_err("no panic");
        assert_eq!(
            payload.downcast_ref::<String>().map(String::as_str),
            Some("unhandled TypeNode: KindIdentifier")
        );
    }
}
