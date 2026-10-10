//! Port of `scanner/scanner.go` lines 1 to 1236: the `Scanner` struct and
//! state, `Scan`, comment directives and the rescans for `<`, `>`, `/`, `*=`
//! and templates.
//!
//! Go `rune` ports to `i32` (Go `char()` returns -1 at the end of the text).
//! The Go `stringutil` predicates take `char`; `rune_to_char` maps a rune to
//! a `char` (invalid values become U+FFFD, which none of them accept).
//!
//! Go indexes strings by byte. This port reads `text.as_bytes()` and decodes
//! runes with `utf8_decode_rune_in_string`, which follows Go
//! `utf8.DecodeRuneInString` also at a non-boundary position.

use crate::flags_macros::{go_enum, go_flags};
use crate::frontend::prelude::*;

use super::regexp::{RegExpParser, RegularExpressionFlags, char_code_to_reg_exp_flag};

// Go: scanner/scanner.go:21 identifierVariant (ts#63996)
go_enum!(IdentifierVariant, i32 {
    STANDARD = 0; // identifierVariantStandard
    JSX = 1; // identifierVariantJSX
    REG_EXP_GROUP_NAME = 2; // identifierVariantRegExpGroupName
});

go_flags!(EscapeSequenceScanningFlags, i32 {
    STRING = 1 << 0; // EscapeSequenceScanningFlagsString
    REPORT_ERRORS = 1 << 1; // EscapeSequenceScanningFlagsReportErrors
    REGULAR_EXPRESSION = 1 << 2; // EscapeSequenceScanningFlagsRegularExpression
    ANNEX_B = 1 << 3; // EscapeSequenceScanningFlagsAnnexB
    ANY_UNICODE_MODE = 1 << 4; // EscapeSequenceScanningFlagsAnyUnicodeMode
    ATOM_ESCAPE = 1 << 5; // EscapeSequenceScanningFlagsAtomEscape
    REPORT_INVALID_ESCAPE_ERRORS = (1 << 2) | (1 << 1); // EscapeSequenceScanningFlagsReportInvalidEscapeErrors
    ALLOW_EXTENDED_UNICODE_ESCAPE = (1 << 0) | (1 << 4); // EscapeSequenceScanningFlagsAllowExtendedUnicodeEscape
});

/// Go `ErrorCallback func(diagnostic *diagnostics.Message, start, length int, args ...any)`.
pub type ErrorCallback =
    Box<dyn FnMut(&'static crate::diagnostics::Message, i32, i32, Vec<String>)>;

// ---------------------------------------------------------------------------
// Go `unicode/utf8` helpers.
// ---------------------------------------------------------------------------

/// Go `utf8.RuneError`.
pub(crate) const RUNE_ERROR: i32 = 0xFFFD;
/// Go `utf8.RuneSelf`.
pub(crate) const RUNE_SELF: i32 = 0x80;

/// Go `utf8.DecodeRuneInString(text[pos:])`. Returns `(RuneError, 0)` at the
/// end and `(RuneError, 1)` for an invalid sequence (for example `pos` inside
/// a multi-byte character).
// PORT: `text` is the port form of the Go text (see
// `scanner_util::GO_STRING_MARKER`). A unit decodes as one rune with its
// size in `text`: a real U+FDD0 as `(0xFDD0, 6)`, an invalid byte as
// `(RuneError, 7)` where Go gives `(RuneError, 1)`, and a lone surrogate
// (only in values, never in source text) as `(RuneError, 7)`.
pub(crate) fn utf8_decode_rune_in_string(text: &str, pos: usize) -> (i32, i32) {
    let bytes = text.as_bytes();
    if pos >= bytes.len() {
        return (RUNE_ERROR, 0);
    }
    let b = bytes[pos];
    if i32::from(b) < RUNE_SELF {
        return (i32::from(b), 1);
    }
    if b == GO_STRING_MARKER_LEAD {
        return go_unit_rune(go_unit_at(text, pos));
    }
    decode_rune_in_bytes(&bytes[pos..])
}

/// Go `utf8.DecodeLastRuneInString(text[:end])`.
// PORT: a unit of the port form decodes as in `utf8_decode_rune_in_string`.
pub(crate) fn utf8_decode_last_rune_in_string(text: &str, end: usize) -> (i32, i32) {
    let bytes = text.as_bytes();
    if end == 0 {
        return (RUNE_ERROR, 0);
    }
    let b = bytes[end - 1];
    if i32::from(b) < RUNE_SELF {
        return (i32::from(b), 1);
    }
    if text.is_char_boundary(end) {
        return go_unit_rune(go_unit_before(text, end));
    }
    let lim = end.saturating_sub(4);
    let mut start = end - 1;
    while start > lim && bytes[start] & 0xC0 == 0x80 {
        start -= 1;
    }
    // Decode only inside text[:end].
    let (r, size) = decode_rune_in_bytes(&bytes[start..end]);
    if start + size as usize != end {
        return (RUNE_ERROR, 1);
    }
    (r, size)
}

/// First UTF-8 byte of `GO_STRING_MARKER` (U+FDD0 is EF B7 90).
const GO_STRING_MARKER_LEAD: u8 = 0xEF;

/// The Go rune of a port form unit and its size in the text (see
/// `utf8_decode_rune_in_string`).
fn go_unit_rune((unit, size): (GoUnit, usize)) -> (i32, i32) {
    match unit {
        GoUnit::Char(ch) => (ch as i32, size as i32),
        GoUnit::Surrogate(_) | GoUnit::InvalidByte(_) => (RUNE_ERROR, size as i32),
    }
}

/// Go `utf8.DecodeRune(b)` on a byte slice.
fn decode_rune_in_bytes(bytes: &[u8]) -> (i32, i32) {
    if bytes.is_empty() {
        return (RUNE_ERROR, 0);
    }
    let b = bytes[0];
    if i32::from(b) < RUNE_SELF {
        return (i32::from(b), 1);
    }
    let width = if b & 0xE0 == 0xC0 {
        2
    } else if b & 0xF0 == 0xE0 {
        3
    } else if b & 0xF8 == 0xF0 {
        4
    } else {
        return (RUNE_ERROR, 1);
    };
    if width > bytes.len() {
        return (RUNE_ERROR, 1);
    }
    match std::str::from_utf8(&bytes[..width])
        .ok()
        .and_then(|s| s.chars().next())
    {
        Some(ch) => (ch as i32, width as i32),
        None => (RUNE_ERROR, 1),
    }
}

/// Go `utf8.DecodeRuneInString(s)`.
pub(crate) fn utf8_decode_rune(s: &str) -> (i32, i32) {
    utf8_decode_rune_in_string(s, 0)
}

/// Go `utf8.DecodeLastRuneInString(s)`.
pub(crate) fn utf8_decode_last_rune(s: &str) -> (i32, i32) {
    utf8_decode_last_rune_in_string(s, s.len())
}

/// Maps a Go rune to a `char` for the `stringutil` predicates.
// PORT: Go passes the rune through. A value that is not a Unicode scalar
// (-1, a surrogate) becomes U+FFFD, which no predicate accepts, as Go.
pub(crate) fn rune_to_char(r: i32) -> char {
    u32::try_from(r)
        .ok()
        .and_then(char::from_u32)
        .unwrap_or(char::REPLACEMENT_CHARACTER)
}

/// Go `string(r)` for a rune: invalid runes become "�".
pub(crate) fn rune_to_string(r: i32) -> String {
    rune_to_char(r).to_string()
}

/// Interns a token value as `&'static str`.
// PORT: Go `ScannerState` holds `tokenValue string`, and Go strings are
// shared immutable values, so `Mark`/`Rewind` copy it for free. The contract
// makes `ScannerState` `Copy`, so each distinct value is leaked once and
// shared. The set of distinct token values is bounded by the source texts.
pub(crate) fn intern_token_value(value: &str) -> &'static str {
    thread_local! {
        static TOKEN_VALUES: RefCell<FxHashSet<&'static str>> =
            const { RefCell::new(FxHashSet::with_hasher(rustc_hash::FxBuildHasher)) };
    }
    if value.is_empty() {
        return "";
    }
    TOKEN_VALUES.with(|values| {
        let mut values = values.borrow_mut();
        if let Some(existing) = values.get(value) {
            return *existing;
        }
        let leaked: &'static str = Box::leak(value.to_string().into_boxed_str());
        values.insert(leaked);
        leaked
    })
}

// ---------------------------------------------------------------------------
// Keyword and token tables.
// ---------------------------------------------------------------------------

/// Go `textToKeyword` map, in Go source order.
pub(crate) static TEXT_TO_KEYWORD: &[(&str, SyntaxKind)] = &[
    ("abstract", SyntaxKind::AbstractKeyword),
    ("accessor", SyntaxKind::AccessorKeyword),
    ("any", SyntaxKind::AnyKeyword),
    ("as", SyntaxKind::AsKeyword),
    ("asserts", SyntaxKind::AssertsKeyword),
    ("assert", SyntaxKind::AssertKeyword),
    ("bigint", SyntaxKind::BigIntKeyword),
    ("boolean", SyntaxKind::BooleanKeyword),
    ("break", SyntaxKind::BreakKeyword),
    ("case", SyntaxKind::CaseKeyword),
    ("catch", SyntaxKind::CatchKeyword),
    ("class", SyntaxKind::ClassKeyword),
    ("continue", SyntaxKind::ContinueKeyword),
    ("const", SyntaxKind::ConstKeyword),
    ("constructor", SyntaxKind::ConstructorKeyword),
    ("debugger", SyntaxKind::DebuggerKeyword),
    ("declare", SyntaxKind::DeclareKeyword),
    ("default", SyntaxKind::DefaultKeyword),
    ("defer", SyntaxKind::DeferKeyword),
    ("delete", SyntaxKind::DeleteKeyword),
    ("do", SyntaxKind::DoKeyword),
    ("else", SyntaxKind::ElseKeyword),
    ("enum", SyntaxKind::EnumKeyword),
    ("export", SyntaxKind::ExportKeyword),
    ("extends", SyntaxKind::ExtendsKeyword),
    ("false", SyntaxKind::FalseKeyword),
    ("finally", SyntaxKind::FinallyKeyword),
    ("for", SyntaxKind::ForKeyword),
    ("from", SyntaxKind::FromKeyword),
    ("function", SyntaxKind::FunctionKeyword),
    ("get", SyntaxKind::GetKeyword),
    ("if", SyntaxKind::IfKeyword),
    ("immediate", SyntaxKind::ImmediateKeyword),
    ("implements", SyntaxKind::ImplementsKeyword),
    ("import", SyntaxKind::ImportKeyword),
    ("in", SyntaxKind::InKeyword),
    ("infer", SyntaxKind::InferKeyword),
    ("instanceof", SyntaxKind::InstanceOfKeyword),
    ("interface", SyntaxKind::InterfaceKeyword),
    ("intrinsic", SyntaxKind::IntrinsicKeyword),
    ("is", SyntaxKind::IsKeyword),
    ("keyof", SyntaxKind::KeyOfKeyword),
    ("let", SyntaxKind::LetKeyword),
    ("module", SyntaxKind::ModuleKeyword),
    ("namespace", SyntaxKind::NamespaceKeyword),
    ("never", SyntaxKind::NeverKeyword),
    ("new", SyntaxKind::NewKeyword),
    ("null", SyntaxKind::NullKeyword),
    ("number", SyntaxKind::NumberKeyword),
    ("object", SyntaxKind::ObjectKeyword),
    ("package", SyntaxKind::PackageKeyword),
    ("private", SyntaxKind::PrivateKeyword),
    ("protected", SyntaxKind::ProtectedKeyword),
    ("public", SyntaxKind::PublicKeyword),
    ("override", SyntaxKind::OverrideKeyword),
    ("out", SyntaxKind::OutKeyword),
    ("readonly", SyntaxKind::ReadonlyKeyword),
    ("require", SyntaxKind::RequireKeyword),
    ("global", SyntaxKind::GlobalKeyword),
    ("return", SyntaxKind::ReturnKeyword),
    ("satisfies", SyntaxKind::SatisfiesKeyword),
    ("set", SyntaxKind::SetKeyword),
    ("source", SyntaxKind::SourceKeyword),
    ("static", SyntaxKind::StaticKeyword),
    ("string", SyntaxKind::StringKeyword),
    ("super", SyntaxKind::SuperKeyword),
    ("switch", SyntaxKind::SwitchKeyword),
    ("symbol", SyntaxKind::SymbolKeyword),
    ("this", SyntaxKind::ThisKeyword),
    ("throw", SyntaxKind::ThrowKeyword),
    ("true", SyntaxKind::TrueKeyword),
    ("try", SyntaxKind::TryKeyword),
    ("type", SyntaxKind::TypeKeyword),
    ("typeof", SyntaxKind::TypeOfKeyword),
    ("undefined", SyntaxKind::UndefinedKeyword),
    ("unique", SyntaxKind::UniqueKeyword),
    ("unknown", SyntaxKind::UnknownKeyword),
    ("using", SyntaxKind::UsingKeyword),
    ("var", SyntaxKind::VarKeyword),
    ("void", SyntaxKind::VoidKeyword),
    ("while", SyntaxKind::WhileKeyword),
    ("with", SyntaxKind::WithKeyword),
    ("yield", SyntaxKind::YieldKeyword),
    ("async", SyntaxKind::AsyncKeyword),
    ("await", SyntaxKind::AwaitKeyword),
    ("of", SyntaxKind::OfKeyword),
];

/// Go `textToToken` punctuation entries. Go copies `textToKeyword` into the
/// same map; `text_to_token` checks both tables.
pub(crate) static TEXT_TO_PUNCTUATION: &[(&str, SyntaxKind)] = &[
    ("{", SyntaxKind::OpenBraceToken),
    ("}", SyntaxKind::CloseBraceToken),
    ("(", SyntaxKind::OpenParenToken),
    (")", SyntaxKind::CloseParenToken),
    ("[", SyntaxKind::OpenBracketToken),
    ("]", SyntaxKind::CloseBracketToken),
    (".", SyntaxKind::DotToken),
    ("...", SyntaxKind::DotDotDotToken),
    (";", SyntaxKind::SemicolonToken),
    (",", SyntaxKind::CommaToken),
    ("<", SyntaxKind::LessThanToken),
    (">", SyntaxKind::GreaterThanToken),
    ("<=", SyntaxKind::LessThanEqualsToken),
    (">=", SyntaxKind::GreaterThanEqualsToken),
    ("==", SyntaxKind::EqualsEqualsToken),
    ("!=", SyntaxKind::ExclamationEqualsToken),
    ("===", SyntaxKind::EqualsEqualsEqualsToken),
    ("!==", SyntaxKind::ExclamationEqualsEqualsToken),
    ("=>", SyntaxKind::EqualsGreaterThanToken),
    ("+", SyntaxKind::PlusToken),
    ("-", SyntaxKind::MinusToken),
    ("**", SyntaxKind::AsteriskAsteriskToken),
    ("*", SyntaxKind::AsteriskToken),
    ("/", SyntaxKind::SlashToken),
    ("%", SyntaxKind::PercentToken),
    ("++", SyntaxKind::PlusPlusToken),
    ("--", SyntaxKind::MinusMinusToken),
    ("<<", SyntaxKind::LessThanLessThanToken),
    ("</", SyntaxKind::LessThanSlashToken),
    (">>", SyntaxKind::GreaterThanGreaterThanToken),
    (">>>", SyntaxKind::GreaterThanGreaterThanGreaterThanToken),
    ("&", SyntaxKind::AmpersandToken),
    ("|", SyntaxKind::BarToken),
    ("^", SyntaxKind::CaretToken),
    ("!", SyntaxKind::ExclamationToken),
    ("~", SyntaxKind::TildeToken),
    ("&&", SyntaxKind::AmpersandAmpersandToken),
    ("||", SyntaxKind::BarBarToken),
    ("?", SyntaxKind::QuestionToken),
    ("??", SyntaxKind::QuestionQuestionToken),
    ("?.", SyntaxKind::QuestionDotToken),
    (":", SyntaxKind::ColonToken),
    ("=", SyntaxKind::EqualsToken),
    ("+=", SyntaxKind::PlusEqualsToken),
    ("-=", SyntaxKind::MinusEqualsToken),
    ("*=", SyntaxKind::AsteriskEqualsToken),
    ("**=", SyntaxKind::AsteriskAsteriskEqualsToken),
    ("/=", SyntaxKind::SlashEqualsToken),
    ("%=", SyntaxKind::PercentEqualsToken),
    ("<<=", SyntaxKind::LessThanLessThanEqualsToken),
    (">>=", SyntaxKind::GreaterThanGreaterThanEqualsToken),
    (
        ">>>=",
        SyntaxKind::GreaterThanGreaterThanGreaterThanEqualsToken,
    ),
    ("&=", SyntaxKind::AmpersandEqualsToken),
    ("|=", SyntaxKind::BarEqualsToken),
    ("^=", SyntaxKind::CaretEqualsToken),
    ("||=", SyntaxKind::BarBarEqualsToken),
    ("&&=", SyntaxKind::AmpersandAmpersandEqualsToken),
    ("??=", SyntaxKind::QuestionQuestionEqualsToken),
    ("@", SyntaxKind::AtToken),
    ("#", SyntaxKind::HashToken),
    ("`", SyntaxKind::BacktickToken),
];

/// Go `textToKeyword[text]`. Returns `SyntaxKind::Unknown` (the Go zero
/// value) on a miss.
// PERF: a match on the length, then on the bytes, so a lookup does not hash
// the text. It holds the same entries as `TEXT_TO_KEYWORD`
// (`tests::text_to_keyword_matches_table` checks this). `crate::scanner_util`
// uses it too.
pub(crate) fn text_to_keyword(text: &str) -> SyntaxKind {
    use crate::astdata::SyntaxKind as K;
    let b = text.as_bytes();
    match b.len() {
        2 => match b {
            b"as" => K::AsKeyword,
            b"do" => K::DoKeyword,
            b"if" => K::IfKeyword,
            b"in" => K::InKeyword,
            b"is" => K::IsKeyword,
            b"of" => K::OfKeyword,
            _ => K::Unknown,
        },
        3 => match b {
            b"any" => K::AnyKeyword,
            b"for" => K::ForKeyword,
            b"get" => K::GetKeyword,
            b"let" => K::LetKeyword,
            b"new" => K::NewKeyword,
            b"out" => K::OutKeyword,
            b"set" => K::SetKeyword,
            b"try" => K::TryKeyword,
            b"var" => K::VarKeyword,
            _ => K::Unknown,
        },
        4 => match b {
            b"case" => K::CaseKeyword,
            b"else" => K::ElseKeyword,
            b"enum" => K::EnumKeyword,
            b"from" => K::FromKeyword,
            b"null" => K::NullKeyword,
            b"this" => K::ThisKeyword,
            b"true" => K::TrueKeyword,
            b"type" => K::TypeKeyword,
            b"void" => K::VoidKeyword,
            b"with" => K::WithKeyword,
            _ => K::Unknown,
        },
        5 => match b {
            b"async" => K::AsyncKeyword,
            b"await" => K::AwaitKeyword,
            b"break" => K::BreakKeyword,
            b"catch" => K::CatchKeyword,
            b"class" => K::ClassKeyword,
            b"const" => K::ConstKeyword,
            b"defer" => K::DeferKeyword,
            b"false" => K::FalseKeyword,
            b"infer" => K::InferKeyword,
            b"keyof" => K::KeyOfKeyword,
            b"never" => K::NeverKeyword,
            b"super" => K::SuperKeyword,
            b"throw" => K::ThrowKeyword,
            b"using" => K::UsingKeyword,
            b"while" => K::WhileKeyword,
            b"yield" => K::YieldKeyword,
            _ => K::Unknown,
        },
        6 => match b {
            b"assert" => K::AssertKeyword,
            b"bigint" => K::BigIntKeyword,
            b"delete" => K::DeleteKeyword,
            b"export" => K::ExportKeyword,
            b"global" => K::GlobalKeyword,
            b"import" => K::ImportKeyword,
            b"module" => K::ModuleKeyword,
            b"number" => K::NumberKeyword,
            b"object" => K::ObjectKeyword,
            b"public" => K::PublicKeyword,
            b"return" => K::ReturnKeyword,
            b"source" => K::SourceKeyword,
            b"static" => K::StaticKeyword,
            b"string" => K::StringKeyword,
            b"switch" => K::SwitchKeyword,
            b"symbol" => K::SymbolKeyword,
            b"typeof" => K::TypeOfKeyword,
            b"unique" => K::UniqueKeyword,
            _ => K::Unknown,
        },
        7 => match b {
            b"asserts" => K::AssertsKeyword,
            b"boolean" => K::BooleanKeyword,
            b"declare" => K::DeclareKeyword,
            b"default" => K::DefaultKeyword,
            b"extends" => K::ExtendsKeyword,
            b"finally" => K::FinallyKeyword,
            b"package" => K::PackageKeyword,
            b"private" => K::PrivateKeyword,
            b"require" => K::RequireKeyword,
            b"unknown" => K::UnknownKeyword,
            _ => K::Unknown,
        },
        8 => match b {
            b"abstract" => K::AbstractKeyword,
            b"accessor" => K::AccessorKeyword,
            b"continue" => K::ContinueKeyword,
            b"debugger" => K::DebuggerKeyword,
            b"function" => K::FunctionKeyword,
            b"override" => K::OverrideKeyword,
            b"readonly" => K::ReadonlyKeyword,
            _ => K::Unknown,
        },
        9 => match b {
            b"immediate" => K::ImmediateKeyword,
            b"interface" => K::InterfaceKeyword,
            b"intrinsic" => K::IntrinsicKeyword,
            b"namespace" => K::NamespaceKeyword,
            b"protected" => K::ProtectedKeyword,
            b"satisfies" => K::SatisfiesKeyword,
            b"undefined" => K::UndefinedKeyword,
            _ => K::Unknown,
        },
        10 => match b {
            b"implements" => K::ImplementsKeyword,
            b"instanceof" => K::InstanceOfKeyword,
            _ => K::Unknown,
        },
        11 => match b {
            b"constructor" => K::ConstructorKeyword,
            _ => K::Unknown,
        },
        _ => K::Unknown,
    }
}

/// Go `kind, ok := textToToken[text]`.
pub(crate) fn text_to_token(text: &str) -> Option<SyntaxKind> {
    static MAP: std::sync::OnceLock<FxHashMap<&'static str, SyntaxKind>> =
        std::sync::OnceLock::new();
    MAP.get_or_init(|| {
        let mut m: FxHashMap<&'static str, SyntaxKind> =
            TEXT_TO_PUNCTUATION.iter().copied().collect();
        // Go: maps.Copy(m, textToKeyword)
        m.extend(TEXT_TO_KEYWORD.iter().copied());
        m
    })
    .get(text)
    .copied()
}

// ---------------------------------------------------------------------------
// Scanner
// ---------------------------------------------------------------------------

/// Go `scanner.ScannerState`.
// PORT: `commentDirectives` is a Go slice header. Go `Rewind` restores its
// length, and later appends overwrite the shared backing array after that
// length. Here the backing array is `Scanner.comment_directives` and the
// state keeps only the length (`comment_directives_len`). `tokenValue` is a
// slice of the scanned text or an interned `&'static str` (see
// `intern_token_value`), so the state is `Copy`.
#[derive(Clone, Copy, Debug)]
pub struct ScannerState<'a> {
    /// Current position in text (and ending position of current token)
    pub pos: i32,
    /// Starting position of current token including preceding whitespace
    pub full_start_pos: i32,
    /// Starting position of non-whitespace part of current token
    pub token_start: i32,
    /// Kind of current token
    pub token: SyntaxKind,
    /// Parsed value of current token
    pub token_value: &'a str,
    /// Flags for current token
    pub token_flags: TokenFlags,
    pub comment_directives_len: usize,
    /// Leading asterisks to skip when scanning types inside JSDoc. Should be 0 outside JSDoc
    pub skip_js_doc_leading_asterisks: i32,
}

impl Default for ScannerState<'_> {
    fn default() -> Self {
        ScannerState {
            pos: 0,
            full_start_pos: 0,
            token_start: 0,
            token: SyntaxKind::Unknown,
            token_value: "",
            token_flags: TokenFlags::NONE,
            comment_directives_len: 0,
            skip_js_doc_leading_asterisks: 0,
        }
    }
}

/// Go `scanner.Scanner`.
// PORT: the Go embedded `ScannerState` is the field `scanner_state`.
// `comment_directives` is the backing array of the Go `commentDirectives`
// slice (see `ScannerState`). The scanner borrows its text (`'a`): a parse
// borrows the file text for the parse (textleak1).
pub struct Scanner<'a> {
    pub(crate) text: &'a str,
    pub(crate) end: i32,
    pub(crate) language_variant: LanguageVariant,
    pub(crate) script_target: ScriptTarget,
    pub(crate) on_error: Option<ErrorCallback>,
    pub(crate) skip_trivia: bool,
    pub(crate) scanner_state: ScannerState<'a>,

    // PORT: maps a token value to its interned `jsnum` string, so a hit
    // needs no second intern lookup.
    pub(crate) number_cache: FxHashMap<&'a str, &'static str>,
    // PORT: values (and `hex_number_cache` keys) are interned `&'static str`,
    // like Go strings, so a cache hit needs no allocation or clone. The
    // `hex_digit_cache` key is looked up by `&str` and owned only on a miss.
    pub(crate) hex_number_cache: FxHashMap<&'static str, &'a str>,
    pub(crate) hex_digit_cache: FxHashMap<Box<str>, &'static str>,

    pub(crate) comment_directives: Vec<CommentDirective>,
}

// Go: scanner/scanner.go:225 defaultScanner
pub(crate) fn default_scanner<'a>() -> Scanner<'a> {
    // Using a function rather than a global is intentional; this function is
    // inlined as pure code (zeroing + moves), whereas a global requires write
    // barriers since the memory is mutable.
    Scanner {
        text: "",
        end: 0,
        language_variant: LanguageVariant::STANDARD,
        script_target: ScriptTarget::NONE,
        on_error: None,
        skip_trivia: true,
        scanner_state: ScannerState::default(),
        number_cache: FxHashMap::default(),
        hex_number_cache: FxHashMap::default(),
        hex_digit_cache: FxHashMap::default(),
        comment_directives: Vec::new(),
    }
}

// Go: scanner/scanner.go:232 NewScanner
pub fn new_scanner<'a>() -> Scanner<'a> {
    default_scanner()
}

impl<'a> Scanner<'a> {
    // Go: scanner/scanner.go:237 Reset
    pub fn reset(&mut self) {
        let number_cache = cleared(std::mem::take(&mut self.number_cache));
        let hex_number_cache = cleared(std::mem::take(&mut self.hex_number_cache));
        let hex_digit_cache = cleared(std::mem::take(&mut self.hex_digit_cache));
        *self = default_scanner();
        self.number_cache = number_cache;
        self.hex_number_cache = hex_number_cache;
        self.hex_digit_cache = hex_digit_cache;
    }
}

// Go: scanner/scanner.go:247 cleared
pub(crate) fn cleared<K, V>(mut m: FxHashMap<K, V>) -> FxHashMap<K, V> {
    m.clear();
    m
}

impl<'a> Scanner<'a> {
    // Go: scanner/scanner.go:252 Text
    pub fn text(&self) -> &'a str {
        self.text
    }

    // Go: scanner/scanner.go:256 Token
    pub fn token(&self) -> SyntaxKind {
        self.scanner_state.token
    }

    // Go: scanner/scanner.go:260 TokenFlags
    pub fn token_flags(&self) -> TokenFlags {
        self.scanner_state.token_flags
    }

    // Go: scanner/scanner.go:264 TokenFullStart
    pub fn token_full_start(&self) -> i32 {
        self.scanner_state.full_start_pos
    }

    // Go: scanner/scanner.go:268 TokenStart
    pub fn token_start(&self) -> i32 {
        self.scanner_state.token_start
    }

    // Go: scanner/scanner.go:272 TokenEnd
    pub fn token_end(&self) -> i32 {
        self.scanner_state.pos
    }

    // Go: scanner/scanner.go:276 TokenText
    pub fn token_text(&self) -> &str {
        &self.text[self.scanner_state.token_start as usize..self.scanner_state.pos as usize]
    }

    // Go: scanner/scanner.go:280 TokenValue
    pub fn token_value(&self) -> &'a str {
        self.scanner_state.token_value
    }

    /// Go `s.text[start:end]` as a token value. A slice of the text needs
    /// no interning: like a Go substring, it shares the text.
    #[inline]
    pub(crate) fn text_token_value(&self, start: usize, end: usize) -> &'a str {
        &self.text[start..end]
    }

    /// Go `s.tokenValue = value`.
    // PORT: interns the value; see `intern_token_value`.
    pub(crate) fn set_token_value(&mut self, value: &str) {
        self.scanner_state.token_value = intern_token_value(value);
    }

    // Go: scanner/scanner.go:284 TokenRange
    pub fn token_range(&self) -> TextRange {
        TextRange::new(self.scanner_state.token_start, self.scanner_state.pos)
    }

    // Go: scanner/scanner.go:288 CommentDirectives
    pub fn comment_directives(&self) -> &[CommentDirective] {
        &self.comment_directives[..self.scanner_state.comment_directives_len]
    }

    // Go: scanner/scanner.go:292 Mark
    pub fn mark(&self) -> ScannerState<'a> {
        self.scanner_state
    }

    // Go: scanner/scanner.go:296 Rewind
    pub fn rewind(&mut self, state: ScannerState<'a>) {
        self.scanner_state = state;
    }

    // Go: scanner/scanner.go:300 ResetPos
    pub fn reset_pos(&mut self, pos: i32) {
        if pos < 0 {
            panic!("Cannot reset token state to negative position");
        }
        self.scanner_state.pos = pos;
        self.scanner_state.full_start_pos = pos;
        self.scanner_state.token_start = pos;
    }

    // Go: scanner/scanner.go:309 ResetTokenState
    pub fn reset_token_state(&mut self, pos: i32) {
        self.reset_pos(pos);
        self.scanner_state.token = SyntaxKind::Unknown;
        self.scanner_state.token_value = "";
        self.scanner_state.token_flags = TokenFlags::NONE;
    }

    // Go: scanner/scanner.go:316 SetSkipJSDocLeadingAsterisks
    pub fn set_skip_js_doc_leading_asterisks(&mut self, skip: bool) {
        if skip {
            self.scanner_state.skip_js_doc_leading_asterisks += 1;
        } else {
            self.scanner_state.skip_js_doc_leading_asterisks += -1;
        }
    }

    // Go: scanner/scanner.go:324 SetSkipTrivia
    pub fn set_skip_trivia(&mut self, skip: bool) {
        self.skip_trivia = skip;
    }

    // Go: scanner/scanner.go:328 HasUnicodeEscape
    pub fn has_unicode_escape(&self) -> bool {
        self.scanner_state
            .token_flags
            .intersects(TokenFlags::UNICODE_ESCAPE)
    }

    // Go: scanner/scanner.go:332 HasExtendedUnicodeEscape
    pub fn has_extended_unicode_escape(&self) -> bool {
        self.scanner_state
            .token_flags
            .intersects(TokenFlags::EXTENDED_UNICODE_ESCAPE)
    }

    // Go: scanner/scanner.go:336 HasPrecedingLineBreak
    pub fn has_preceding_line_break(&self) -> bool {
        self.scanner_state
            .token_flags
            .intersects(TokenFlags::PRECEDING_LINE_BREAK)
    }

    // Go: scanner/scanner.go:340 HasPrecedingJSDocComment
    pub fn has_preceding_js_doc_comment(&self) -> bool {
        self.scanner_state
            .token_flags
            .intersects(TokenFlags::PRECEDING_JS_DOC_COMMENT)
    }

    // Go: scanner/scanner.go:344 HasPrecedingJSDocLeadingAsterisks
    pub fn has_preceding_js_doc_leading_asterisks(&self) -> bool {
        self.scanner_state
            .token_flags
            .intersects(TokenFlags::PRECEDING_JS_DOC_LEADING_ASTERISKS)
    }

    // Go: scanner/scanner.go:348 HasPrecedingJSDocWithDeprecatedTag
    pub fn has_preceding_js_doc_with_deprecated_tag(&self) -> bool {
        self.scanner_state
            .token_flags
            .intersects(TokenFlags::PRECEDING_JS_DOC_WITH_DEPRECATED)
    }

    // Go: scanner/scanner.go:352 HasPrecedingJSDocWithSeeOrLink
    pub fn has_preceding_js_doc_with_see_or_link(&self) -> bool {
        self.scanner_state
            .token_flags
            .intersects(TokenFlags::PRECEDING_JS_DOC_WITH_SEE_OR_LINK)
    }

    // Go: scanner/scanner.go:358 scanJSDocCommentForTags
    // scanJSDocCommentForTags scans a JSDoc comment for @deprecated, @see, and @link tags,
    // setting the appropriate token flags. Called during scanning when a JSDoc comment is detected.
    // PORT: Go takes `commentText string` (a slice of `s.text`). This takes the
    // slice bounds so the text can be read while the flags change.
    pub(crate) fn scan_js_doc_comment_for_tags(&mut self, comment_start: i32, comment_end: i32) {
        let mut comment_text: &str = &self.text[comment_start as usize..comment_end as usize];
        loop {
            // Go `strings.IndexByte`: memchr does the same vectorized byte scan.
            let Some(i) = memchr::memchr(b'@', comment_text.as_bytes()) else {
                return;
            };
            comment_text = &comment_text[i + 1..];
            let flags = &mut self.scanner_state.token_flags;
            if !flags.intersects(TokenFlags::PRECEDING_JS_DOC_WITH_DEPRECATED)
                && has_js_doc_tag(comment_text, &["deprecated"])
            {
                *flags |= TokenFlags::PRECEDING_JS_DOC_WITH_DEPRECATED;
            }
            if !flags.intersects(TokenFlags::PRECEDING_JS_DOC_WITH_SEE_OR_LINK)
                && has_js_doc_tag(comment_text, &["see", "link", "linkcode", "linkplain"])
            {
                *flags |= TokenFlags::PRECEDING_JS_DOC_WITH_SEE_OR_LINK;
            }
            if (*flags
                & (TokenFlags::PRECEDING_JS_DOC_WITH_DEPRECATED
                    | TokenFlags::PRECEDING_JS_DOC_WITH_SEE_OR_LINK))
                == (TokenFlags::PRECEDING_JS_DOC_WITH_DEPRECATED
                    | TokenFlags::PRECEDING_JS_DOC_WITH_SEE_OR_LINK)
            {
                return;
            }
        }
    }
}

// Go: scanner/scanner.go:380 hasJSDocTag
// hasJSDocTag reports whether text starts with one of the given tag names followed
// by a valid JSDoc tag terminator (whitespace, '}', '*', or end-of-string).
pub(crate) fn has_js_doc_tag(text: &str, tags: &[&str]) -> bool {
    for tag in tags {
        if !text.starts_with(tag) {
            continue;
        }
        if text.len() == tag.len() {
            return true;
        }
        let ch = text.as_bytes()[tag.len()];
        if ch == b' ' || ch == b'\t' || ch == b'\n' || ch == b'\r' || ch == b'}' || ch == b'*' {
            return true;
        }
    }
    false
}

impl<'a> Scanner<'a> {
    // Go: scanner/scanner.go:396 SetText
    pub fn set_text(&mut self, text: &'a str) {
        self.text = text;
        self.end = self.text.len() as i32;
        self.scanner_state = ScannerState::default();
    }

    // Go: scanner/scanner.go:402 SetOnError
    pub fn set_on_error(&mut self, error_callback: Option<ErrorCallback>) {
        self.on_error = error_callback;
    }

    // Go: scanner/scanner.go:406 SetLanguageVariant
    pub fn set_language_variant(&mut self, language_variant: LanguageVariant) {
        self.language_variant = language_variant;
    }

    // Go: scanner/scanner.go:410 SetScriptTarget
    pub fn set_script_target(&mut self, script_target: ScriptTarget) {
        self.script_target = script_target;
    }

    // Go: scanner/scanner.go:414 languageVersion
    pub(crate) fn language_version(&self) -> ScriptTarget {
        if self.script_target == ScriptTarget::NONE {
            return ScriptTarget::LATEST;
        }
        self.script_target
    }

    // Go: scanner/scanner.go:421 error
    pub(crate) fn error(&mut self, diagnostic: &'static crate::diagnostics::Message) {
        self.error_at(diagnostic, self.scanner_state.pos, 0, Vec::new());
    }

    // Go: scanner/scanner.go:425 errorAt
    pub(crate) fn error_at(
        &mut self,
        diagnostic: &'static crate::diagnostics::Message,
        pos: i32,
        length: i32,
        args: Vec<String>,
    ) {
        if let Some(on_error) = self.on_error.as_mut() {
            on_error(diagnostic, pos, length, args);
        }
    }

    // Go: scanner/scanner.go:434 char
    // NOTE: even though this returns a rune, it only decodes the current byte.
    // It must be checked against utf8.RuneSelf to verify that a call to charAndSize
    // is not needed.
    // PORT: `end <= text.len()` always, so `get` fails only where Go would
    // also be past the end; it drops the panic path from the hot loop.
    #[inline]
    pub(crate) fn char(&self) -> i32 {
        if self.scanner_state.pos < self.end
            && let Some(&b) = self.text.as_bytes().get(self.scanner_state.pos as usize)
        {
            return i32::from(b);
        }
        -1
    }

    // Go: scanner/scanner.go:442 charAt
    // NOTE: this returns a rune, but only decodes the byte at the offset.
    #[inline]
    pub(crate) fn char_at(&self, offset: i32) -> i32 {
        let i = self.scanner_state.pos + offset;
        if i < self.end
            && let Some(&b) = self.text.as_bytes().get(i as usize)
        {
            return i32::from(b);
        }
        -1
    }

    // Go: scanner/scanner.go:449 charAndSize
    pub(crate) fn char_and_size(&mut self) -> (i32, i32) {
        // Fast path: a single ASCII byte. The vast majority of source bytes are
        // ASCII; handling them here avoids constructing a string slice header and
        // calling the non-inlined utf8.DecodeRuneInString on every byte.
        if self.scanner_state.pos < self.end {
            let b = self.text.as_bytes()[self.scanner_state.pos as usize];
            if i32::from(b) < RUNE_SELF {
                return (i32::from(b), 1);
            }
        }
        utf8_decode_rune_in_string(&self.text, self.scanner_state.pos as usize)
    }

    /// The position of the first `*/` at or after `pos`, and the position
    /// after the last line break before it, when the text up to it is
    /// ASCII. `None` sends the multi-line comment scan to its byte loop.
    fn ascii_comment_end(&self) -> Option<(i32, Option<i32>)> {
        let start = self.scanner_state.pos as usize;
        let bytes = &self.text.as_bytes()[start..self.end as usize];
        let close = memchr::memmem::find(bytes, b"*/")?;
        let body = &bytes[..close];
        if !body.is_ascii() {
            return None;
        }
        let line_start = memchr::memrchr2(b'\n', b'\r', body).map(|i| (start + i + 1) as i32);
        Some(((start + close) as i32, line_start))
    }

    /// The length of the single-line comment body at `pos`, up to the next
    /// `\n` or `\r` or the end, when that body is ASCII. `None` sends the
    /// single-line comment scan to its byte loop.
    fn ascii_line_comment_len(&self) -> Option<i32> {
        let rest = &self.text.as_bytes()[self.scanner_state.pos as usize..self.end as usize];
        let body = &rest[..memchr::memchr2(b'\n', b'\r', rest).unwrap_or(rest.len())];
        body.is_ascii().then_some(body.len() as i32)
    }

    // Go: scanner/scanner.go:464 scanASCIIWhile
    // scanASCIIWhile advances s.pos over the longest run of ASCII bytes for which
    // pred returns true. It stops at end-of-text, the first non-ASCII byte, or the
    // first byte where pred is false.
    pub(crate) fn scan_ascii_while(&mut self, pred: impl Fn(u8) -> bool) {
        let text = &self.text.as_bytes()[self.scanner_state.pos as usize..self.end as usize];
        let mut i = 0usize;
        while i < text.len() {
            let b = text[i];
            if i32::from(b) >= RUNE_SELF || !pred(b) {
                break;
            }
            i += 1;
        }
        self.scanner_state.pos += i as i32;
    }

    /// Go `s.errorAt` passed as the `reportError` callback of
    /// `scanConflictMarkerTrivia`.
    // PORT: split borrow of `text` and `on_error`, so the Go method value can
    // be passed while the text is read.
    pub(crate) fn scan_conflict_marker_trivia_at_pos(&mut self) -> i32 {
        let text: &str = &self.text;
        let on_error = &mut self.on_error;
        let mut report =
            |diagnostic: &'static crate::diagnostics::Message, pos: i32, length: i32| {
                if let Some(on_error) = on_error.as_mut() {
                    on_error(diagnostic, pos, length, Vec::new());
                }
            };
        scan_conflict_marker_trivia(text, self.scanner_state.pos as usize, Some(&mut report)) as i32
    }

    // Go: scanner/scanner.go:477 Scan
    pub fn scan(&mut self) -> SyntaxKind {
        self.scanner_state.full_start_pos = self.scanner_state.pos;
        self.scanner_state.token_flags = TokenFlags::NONE;
        // PORT: label for `continue` through the `'sw` block.
        'scan: loop {
            let ch = self.char();
            self.scanner_state.token_start = self.scanner_state.pos;

            // PORT: Go `switch ch` with `break` leaving the switch is the
            // labeled block `'sw`. `char()` returns a byte or -1, so the
            // cases match on `u8`; -1 goes to the default case.
            'sw: {
                match u8::try_from(ch).ok() {
                    Some(b'\t' | 0x0B | 0x0C | b' ') => {
                        self.scanner_state.pos += 1;
                        if self.skip_trivia {
                            // PORT: skips the rest of the run here instead of
                            // one byte per `'scan` pass; the result is the same.
                            self.scan_ascii_while(|b| matches!(b, b'\t' | 0x0B | 0x0C | b' '));
                            continue 'scan;
                        }
                        loop {
                            let (ch, size) = self.char_and_size();
                            if !is_white_space_single_line(rune_to_char(ch)) {
                                break;
                            }
                            self.scanner_state.pos += size;
                        }
                        self.scanner_state.token = SyntaxKind::WhitespaceTrivia;
                    }
                    Some(b'\n' | b'\r') => {
                        self.scanner_state.token_flags |= TokenFlags::PRECEDING_LINE_BREAK;
                        if self.skip_trivia {
                            self.scanner_state.pos += 1;
                            self.scan_ascii_while(|b| b == b' ' || (b'\t'..=b'\r').contains(&b));
                            continue 'scan;
                        }
                        if ch == i32::from(b'\r') && self.char_at(1) == i32::from(b'\n') {
                            self.scanner_state.pos += 2;
                        } else {
                            self.scanner_state.pos += 1;
                        }
                        self.scanner_state.token = SyntaxKind::NewLineTrivia;
                    }
                    Some(b'!') => {
                        if self.char_at(1) == i32::from(b'=') {
                            if self.char_at(2) == i32::from(b'=') {
                                self.scanner_state.pos += 3;
                                self.scanner_state.token = SyntaxKind::ExclamationEqualsEqualsToken;
                            } else {
                                self.scanner_state.pos += 2;
                                self.scanner_state.token = SyntaxKind::ExclamationEqualsToken;
                            }
                        } else {
                            self.scanner_state.pos += 1;
                            self.scanner_state.token = SyntaxKind::ExclamationToken;
                        }
                    }
                    Some(b'"' | b'\'') => {
                        self.scanner_state.token_value =
                            self.scan_string(false /*jsxAttributeString*/);
                        self.scanner_state.token = SyntaxKind::StringLiteral;
                    }
                    Some(b'`') => {
                        self.scanner_state.token = self.scan_template_and_set_token_value(
                            false, /*shouldEmitInvalidEscapeError*/
                        );
                    }
                    Some(b'%') => {
                        if self.char_at(1) == i32::from(b'=') {
                            self.scanner_state.pos += 2;
                            self.scanner_state.token = SyntaxKind::PercentEqualsToken;
                        } else {
                            self.scanner_state.pos += 1;
                            self.scanner_state.token = SyntaxKind::PercentToken;
                        }
                    }
                    Some(b'&') => {
                        let next = self.char_at(1);
                        if next == i32::from(b'&') {
                            if self.char_at(2) == i32::from(b'=') {
                                self.scanner_state.pos += 3;
                                self.scanner_state.token =
                                    SyntaxKind::AmpersandAmpersandEqualsToken;
                            } else {
                                self.scanner_state.pos += 2;
                                self.scanner_state.token = SyntaxKind::AmpersandAmpersandToken;
                            }
                        } else if next == i32::from(b'=') {
                            self.scanner_state.pos += 2;
                            self.scanner_state.token = SyntaxKind::AmpersandEqualsToken;
                        } else {
                            self.scanner_state.pos += 1;
                            self.scanner_state.token = SyntaxKind::AmpersandToken;
                        }
                    }
                    Some(b'(') => {
                        self.scanner_state.pos += 1;
                        self.scanner_state.token = SyntaxKind::OpenParenToken;
                    }
                    Some(b')') => {
                        self.scanner_state.pos += 1;
                        self.scanner_state.token = SyntaxKind::CloseParenToken;
                    }
                    Some(b'*') => {
                        let next = self.char_at(1);
                        if next == i32::from(b'=') {
                            self.scanner_state.pos += 2;
                            self.scanner_state.token = SyntaxKind::AsteriskEqualsToken;
                        } else if next == i32::from(b'*') {
                            if self.char_at(2) == i32::from(b'=') {
                                self.scanner_state.pos += 3;
                                self.scanner_state.token = SyntaxKind::AsteriskAsteriskEqualsToken;
                            } else {
                                self.scanner_state.pos += 2;
                                self.scanner_state.token = SyntaxKind::AsteriskAsteriskToken;
                            }
                        } else {
                            self.scanner_state.pos += 1;
                            if self.scanner_state.skip_js_doc_leading_asterisks != 0
                                && !self
                                    .scanner_state
                                    .token_flags
                                    .intersects(TokenFlags::PRECEDING_JS_DOC_LEADING_ASTERISKS)
                                && self
                                    .scanner_state
                                    .token_flags
                                    .intersects(TokenFlags::PRECEDING_LINE_BREAK)
                            {
                                self.scanner_state.token_flags |=
                                    TokenFlags::PRECEDING_JS_DOC_LEADING_ASTERISKS;
                                continue 'scan;
                            }
                            self.scanner_state.token = SyntaxKind::AsteriskToken;
                        }
                    }
                    Some(b'+') => {
                        let next = self.char_at(1);
                        if next == i32::from(b'=') {
                            self.scanner_state.pos += 2;
                            self.scanner_state.token = SyntaxKind::PlusEqualsToken;
                        } else if next == i32::from(b'+') {
                            self.scanner_state.pos += 2;
                            self.scanner_state.token = SyntaxKind::PlusPlusToken;
                        } else {
                            self.scanner_state.pos += 1;
                            self.scanner_state.token = SyntaxKind::PlusToken;
                        }
                    }
                    Some(b',') => {
                        self.scanner_state.pos += 1;
                        self.scanner_state.token = SyntaxKind::CommaToken;
                    }
                    Some(b'-') => {
                        let next = self.char_at(1);
                        if next == i32::from(b'=') {
                            self.scanner_state.pos += 2;
                            self.scanner_state.token = SyntaxKind::MinusEqualsToken;
                        } else if next == i32::from(b'-') {
                            self.scanner_state.pos += 2;
                            self.scanner_state.token = SyntaxKind::MinusMinusToken;
                        } else {
                            self.scanner_state.pos += 1;
                            self.scanner_state.token = SyntaxKind::MinusToken;
                        }
                    }
                    Some(b'.') => {
                        let next = self.char_at(1);
                        if is_digit(rune_to_char(next)) {
                            self.scanner_state.token = self.scan_number();
                        } else if next == i32::from(b'.') && self.char_at(2) == i32::from(b'.') {
                            self.scanner_state.pos += 3;
                            self.scanner_state.token = SyntaxKind::DotDotDotToken;
                        } else {
                            self.scanner_state.pos += 1;
                            self.scanner_state.token = SyntaxKind::DotToken;
                        }
                    }
                    Some(b'/') => {
                        // Single-line comment
                        if self.char_at(1) == i32::from(b'/') {
                            self.scanner_state.pos += 2;

                            // PORT: fast path for an ASCII comment; the loop
                            // below gives the same result byte by byte.
                            if let Some(len) = self.ascii_line_comment_len() {
                                self.scanner_state.pos += len;
                            }
                            loop {
                                self.scan_ascii_while(|b| b != b'\n' && b != b'\r');
                                let (ch1, size) = self.char_and_size();
                                if size == 0 || is_line_break(rune_to_char(ch1)) {
                                    break;
                                }
                                self.scanner_state.pos += size;
                            }

                            self.process_comment_directive(
                                self.scanner_state.token_start,
                                self.scanner_state.pos,
                                false,
                            );

                            if self.skip_trivia {
                                continue 'scan;
                            }
                            self.scanner_state.token = SyntaxKind::SingleLineCommentTrivia;
                            return self.scanner_state.token;
                        }
                        // Multi-line comment
                        if self.char_at(1) == i32::from(b'*') {
                            self.scanner_state.pos += 2;
                            let is_js_doc = self.char() == i32::from(b'*')
                                && self.char_at(1) != i32::from(b'/');

                            let mut comment_closed = false;
                            let mut last_line_start = self.scanner_state.token_start;
                            // PORT: fast path for an ASCII comment; the loop
                            // below gives the same result byte by byte.
                            if let Some((close, line_start)) = self.ascii_comment_end() {
                                if let Some(line_start) = line_start {
                                    last_line_start = line_start;
                                    self.scanner_state.token_flags |=
                                        TokenFlags::PRECEDING_LINE_BREAK;
                                }
                                self.scanner_state.pos = close + 2;
                                comment_closed = true;
                            }
                            while !comment_closed {
                                self.scan_ascii_while(|b| b != b'*' && b != b'\n' && b != b'\r');
                                let (ch1, size) = self.char_and_size();
                                if size == 0 {
                                    break;
                                }

                                if ch1 == i32::from(b'*') && self.char_at(1) == i32::from(b'/') {
                                    self.scanner_state.pos += 2;
                                    comment_closed = true;
                                    break;
                                }

                                self.scanner_state.pos += size;

                                if is_line_break(rune_to_char(ch1)) {
                                    last_line_start = self.scanner_state.pos;
                                    self.scanner_state.token_flags |=
                                        TokenFlags::PRECEDING_LINE_BREAK;
                                }
                            }

                            if is_js_doc {
                                self.scanner_state.token_flags |=
                                    TokenFlags::PRECEDING_JS_DOC_COMMENT;
                                self.scan_js_doc_comment_for_tags(
                                    self.scanner_state.token_start,
                                    self.scanner_state.pos,
                                );
                            }

                            self.process_comment_directive(
                                last_line_start,
                                self.scanner_state.pos,
                                true,
                            );

                            if !comment_closed {
                                self.error(diag::Asterisk_Slash_expected);
                            }

                            if self.skip_trivia {
                                continue 'scan;
                            }

                            if !comment_closed {
                                self.scanner_state.token_flags |= TokenFlags::UNTERMINATED;
                            }
                            self.scanner_state.token = SyntaxKind::MultiLineCommentTrivia;
                            return self.scanner_state.token;
                        }
                        if self.char_at(1) == i32::from(b'=') {
                            self.scanner_state.pos += 2;
                            self.scanner_state.token = SyntaxKind::SlashEqualsToken;
                        } else {
                            self.scanner_state.pos += 1;
                            self.scanner_state.token = SyntaxKind::SlashToken;
                        }
                    }
                    Some(b'0'..=b'9') => {
                        if ch == i32::from(b'0') {
                            if self.char_at(1) == i32::from(b'X')
                                || self.char_at(1) == i32::from(b'x')
                            {
                                let start = self.scanner_state.pos;
                                self.scanner_state.pos += 2;
                                let mut digits = self.scan_hex_digits(1, true, true);
                                if digits.is_empty() {
                                    self.error(diag::Hexadecimal_digit_expected);
                                    digits = "0";
                                }
                                if let Some(&cached_value) = self.hex_number_cache.get(digits) {
                                    self.scanner_state.token_value = cached_value;
                                } else {
                                    let (start, end) =
                                        (start as usize, self.scanner_state.pos as usize);
                                    let raw_text = &self.text[start..end];
                                    let value =
                                        if raw_text.starts_with("0x") && &raw_text[2..] == digits {
                                            // Go `s.tokenValue = rawText` shares the text.
                                            self.text_token_value(start, end)
                                        } else {
                                            intern_token_value(&format!("0x{digits}"))
                                        };
                                    self.scanner_state.token_value = value;
                                    self.hex_number_cache.insert(digits, value);
                                }
                                self.scanner_state.token_flags |= TokenFlags::HEX_SPECIFIER;
                                self.scanner_state.token = self.scan_big_int_suffix();
                                break 'sw;
                            }
                            if self.char_at(1) == i32::from(b'B')
                                || self.char_at(1) == i32::from(b'b')
                            {
                                self.scanner_state.pos += 2;
                                let mut digits = self.scan_binary_or_octal_digits(2);
                                if digits.is_empty() {
                                    self.error(diag::Binary_digit_expected);
                                    digits = "0".to_string();
                                }
                                self.set_token_value(&format!("0b{digits}"));
                                self.scanner_state.token_flags |= TokenFlags::BINARY_SPECIFIER;
                                self.scanner_state.token = self.scan_big_int_suffix();
                                break 'sw;
                            }
                            if self.char_at(1) == i32::from(b'O')
                                || self.char_at(1) == i32::from(b'o')
                            {
                                self.scanner_state.pos += 2;
                                let mut digits = self.scan_binary_or_octal_digits(8);
                                if digits.is_empty() {
                                    self.error(diag::Octal_digit_expected);
                                    digits = "0".to_string();
                                }
                                self.set_token_value(&format!("0o{digits}"));
                                self.scanner_state.token_flags |= TokenFlags::OCTAL_SPECIFIER;
                                self.scanner_state.token = self.scan_big_int_suffix();
                                break 'sw;
                            }
                            // Go: fallthrough
                        }
                        self.scanner_state.token = self.scan_number();
                    }
                    Some(b':') => {
                        self.scanner_state.pos += 1;
                        self.scanner_state.token = SyntaxKind::ColonToken;
                    }
                    Some(b';') => {
                        self.scanner_state.pos += 1;
                        self.scanner_state.token = SyntaxKind::SemicolonToken;
                    }
                    Some(b'<') => {
                        if self.char_at(1) == i32::from(b'<')
                            && is_conflict_marker_trivia(
                                &self.text,
                                self.scanner_state.pos as usize,
                            )
                        {
                            self.scanner_state.pos = self.scan_conflict_marker_trivia_at_pos();
                            if self.skip_trivia {
                                continue 'scan;
                            } else {
                                self.scanner_state.token = SyntaxKind::ConflictMarkerTrivia;
                                return self.scanner_state.token;
                            }
                        }
                        if self.char_at(1) == i32::from(b'<') {
                            if self.char_at(2) == i32::from(b'=') {
                                self.scanner_state.pos += 3;
                                self.scanner_state.token = SyntaxKind::LessThanLessThanEqualsToken;
                            } else {
                                self.scanner_state.pos += 2;
                                self.scanner_state.token = SyntaxKind::LessThanLessThanToken;
                            }
                        } else if self.char_at(1) == i32::from(b'=') {
                            self.scanner_state.pos += 2;
                            self.scanner_state.token = SyntaxKind::LessThanEqualsToken;
                        } else if self.language_variant == LanguageVariant::JSX
                            && self.char_at(1) == i32::from(b'/')
                            && self.char_at(2) != i32::from(b'*')
                        {
                            self.scanner_state.pos += 2;
                            self.scanner_state.token = SyntaxKind::LessThanSlashToken;
                        } else {
                            self.scanner_state.pos += 1;
                            self.scanner_state.token = SyntaxKind::LessThanToken;
                        }
                    }
                    Some(b'=') => {
                        if self.char_at(1) == i32::from(b'=')
                            && is_conflict_marker_trivia(
                                &self.text,
                                self.scanner_state.pos as usize,
                            )
                        {
                            self.scanner_state.pos = self.scan_conflict_marker_trivia_at_pos();
                            if self.skip_trivia {
                                continue 'scan;
                            } else {
                                self.scanner_state.token = SyntaxKind::ConflictMarkerTrivia;
                                return self.scanner_state.token;
                            }
                        }
                        if self.char_at(1) == i32::from(b'=') {
                            if self.char_at(2) == i32::from(b'=') {
                                self.scanner_state.pos += 3;
                                self.scanner_state.token = SyntaxKind::EqualsEqualsEqualsToken;
                            } else {
                                self.scanner_state.pos += 2;
                                self.scanner_state.token = SyntaxKind::EqualsEqualsToken;
                            }
                        } else if self.char_at(1) == i32::from(b'>') {
                            self.scanner_state.pos += 2;
                            self.scanner_state.token = SyntaxKind::EqualsGreaterThanToken;
                        } else {
                            self.scanner_state.pos += 1;
                            self.scanner_state.token = SyntaxKind::EqualsToken;
                        }
                    }
                    Some(b'>') => {
                        if self.char_at(1) == i32::from(b'>')
                            && is_conflict_marker_trivia(
                                &self.text,
                                self.scanner_state.pos as usize,
                            )
                        {
                            self.scanner_state.pos = self.scan_conflict_marker_trivia_at_pos();
                            if self.skip_trivia {
                                continue 'scan;
                            } else {
                                self.scanner_state.token = SyntaxKind::ConflictMarkerTrivia;
                                return self.scanner_state.token;
                            }
                        }
                        self.scanner_state.pos += 1;
                        self.scanner_state.token = SyntaxKind::GreaterThanToken;
                    }
                    Some(b'?') => {
                        if self.char_at(1) == i32::from(b'.')
                            && !is_digit(rune_to_char(self.char_at(2)))
                        {
                            self.scanner_state.pos += 2;
                            self.scanner_state.token = SyntaxKind::QuestionDotToken;
                        } else if self.char_at(1) == i32::from(b'?') {
                            if self.char_at(2) == i32::from(b'=') {
                                self.scanner_state.pos += 3;
                                self.scanner_state.token = SyntaxKind::QuestionQuestionEqualsToken;
                            } else {
                                self.scanner_state.pos += 2;
                                self.scanner_state.token = SyntaxKind::QuestionQuestionToken;
                            }
                        } else {
                            self.scanner_state.pos += 1;
                            self.scanner_state.token = SyntaxKind::QuestionToken;
                        }
                    }
                    Some(b'[') => {
                        self.scanner_state.pos += 1;
                        self.scanner_state.token = SyntaxKind::OpenBracketToken;
                    }
                    Some(b']') => {
                        self.scanner_state.pos += 1;
                        self.scanner_state.token = SyntaxKind::CloseBracketToken;
                    }
                    Some(b'^') => {
                        if self.char_at(1) == i32::from(b'=') {
                            self.scanner_state.pos += 2;
                            self.scanner_state.token = SyntaxKind::CaretEqualsToken;
                        } else {
                            self.scanner_state.pos += 1;
                            self.scanner_state.token = SyntaxKind::CaretToken;
                        }
                    }
                    Some(b'{') => {
                        self.scanner_state.pos += 1;
                        self.scanner_state.token = SyntaxKind::OpenBraceToken;
                    }
                    Some(b'|') => {
                        if self.char_at(1) == i32::from(b'|')
                            && is_conflict_marker_trivia(
                                &self.text,
                                self.scanner_state.pos as usize,
                            )
                        {
                            self.scanner_state.pos = self.scan_conflict_marker_trivia_at_pos();
                            if self.skip_trivia {
                                continue 'scan;
                            } else {
                                self.scanner_state.token = SyntaxKind::ConflictMarkerTrivia;
                                return self.scanner_state.token;
                            }
                        }
                        if self.char_at(1) == i32::from(b'|') {
                            if self.char_at(2) == i32::from(b'=') {
                                self.scanner_state.pos += 3;
                                self.scanner_state.token = SyntaxKind::BarBarEqualsToken;
                            } else {
                                self.scanner_state.pos += 2;
                                self.scanner_state.token = SyntaxKind::BarBarToken;
                            }
                        } else if self.char_at(1) == i32::from(b'=') {
                            self.scanner_state.pos += 2;
                            self.scanner_state.token = SyntaxKind::BarEqualsToken;
                        } else {
                            self.scanner_state.pos += 1;
                            self.scanner_state.token = SyntaxKind::BarToken;
                        }
                    }
                    Some(b'}') => {
                        self.scanner_state.pos += 1;
                        self.scanner_state.token = SyntaxKind::CloseBraceToken;
                    }
                    Some(b'~') => {
                        self.scanner_state.pos += 1;
                        self.scanner_state.token = SyntaxKind::TildeToken;
                    }
                    Some(b'@') => {
                        self.scanner_state.pos += 1;
                        self.scanner_state.token = SyntaxKind::AtToken;
                    }
                    Some(b'\\') => {
                        if self.scan_identifier(0, IdentifierVariant::STANDARD) {
                            self.scanner_state.token =
                                get_identifier_token(self.scanner_state.token_value);
                        } else {
                            self.scan_invalid_character();
                        }
                    }
                    Some(b'#') => {
                        if self.char_at(1) == i32::from(b'!') {
                            if self.scanner_state.pos == 0 {
                                self.scanner_state.pos += 2;
                                let (mut ch, mut size) = self.char_and_size();
                                while size > 0 && !is_line_break(rune_to_char(ch)) {
                                    self.scanner_state.pos += size;
                                    (ch, size) = self.char_and_size();
                                }
                                continue 'scan;
                            }
                            self.error_at(
                                diag::X_can_only_be_used_at_the_start_of_a_file,
                                self.scanner_state.pos,
                                2,
                                Vec::new(),
                            );
                            self.scanner_state.pos += 2;
                            self.scanner_state.token = SyntaxKind::Unknown;
                            break 'sw;
                        }
                        if !self.scan_identifier(1, IdentifierVariant::STANDARD) {
                            self.error_at(
                                diag::Invalid_character,
                                self.scanner_state.pos - 1,
                                1,
                                Vec::new(),
                            );
                            self.scanner_state.token_value = "#";
                        }
                        self.scanner_state.token = SyntaxKind::PrivateIdentifier;
                    }
                    _ => {
                        if ch < 0 {
                            self.scanner_state.token = SyntaxKind::EndOfFile;
                            break 'sw;
                        }
                        if self.scan_identifier(0, IdentifierVariant::STANDARD) {
                            self.scanner_state.token =
                                get_identifier_token(self.scanner_state.token_value);
                            break 'sw;
                        }
                        let (mut ch, mut size) = self.char_and_size();
                        if ch == RUNE_ERROR {
                            self.error_at(diag::File_appears_to_be_binary, 0, 0, Vec::new());
                            self.scanner_state.pos = self.text.len() as i32;
                            self.scanner_state.token = SyntaxKind::NonTextFileMarkerTrivia;
                            break 'sw;
                        }
                        if is_white_space_single_line(rune_to_char(ch)) {
                            self.scanner_state.pos += size;

                            // If we get here and it's not 0x0085 (nextLine), then we're handling non-ASCII whitespace.
                            // Handle skipTrivia like we do in the space case above.
                            if ch == 0x0085 || self.skip_trivia {
                                continue 'scan;
                            }

                            loop {
                                (ch, size) = self.char_and_size();
                                if !is_white_space_single_line(rune_to_char(ch)) {
                                    break;
                                }
                                self.scanner_state.pos += size;
                            }
                            self.scanner_state.token = SyntaxKind::WhitespaceTrivia;
                            return self.scanner_state.token;
                        }
                        if is_line_break(rune_to_char(ch)) {
                            self.scanner_state.token_flags |= TokenFlags::PRECEDING_LINE_BREAK;
                            self.scanner_state.pos += size;
                            continue 'scan;
                        }
                        self.scan_invalid_character();
                    }
                }
            }
            return self.scanner_state.token;
        }
    }

    // Go: scanner/scanner.go:968 processCommentDirective
    pub(crate) fn process_comment_directive(&mut self, start: i32, end: i32, multiline: bool) {
        let text = self.text.as_bytes();
        let start_u = start as usize;
        let end_u = end as usize;
        // Skip starting slashes and whitespace
        let mut pos = start_u;
        if multiline {
            // Skip whitespace
            while pos < end_u && (text[pos] == b' ' || text[pos] == b'\t') {
                pos += 1;
            }
            // Skip combinations of / and *
            while pos < end_u && (text[pos] == b'/' || text[pos] == b'*') {
                pos += 1;
            }
        } else {
            // Skip opening //
            pos += 2;
            // Skip another / if present
            while pos < end_u && text[pos] == b'/' {
                pos += 1;
            }
        }
        // Skip whitespace
        while pos < end_u && (text[pos] == b' ' || text[pos] == b'\t') {
            pos += 1;
        }
        // Directive must start with '@'
        if !(pos < end_u && text[pos] == b'@') {
            return;
        }
        pos += 1;
        let kind;
        if text[pos..].starts_with(b"ts-expect-error") {
            kind = CommentDirectiveKind::EXPECT_ERROR;
        } else if text[pos..].starts_with(b"ts-ignore") {
            kind = CommentDirectiveKind::IGNORE;
        } else {
            return;
        }
        // Go: append(s.commentDirectives, ...) on the state's slice header.
        let len = self.scanner_state.comment_directives_len;
        self.comment_directives.truncate(len);
        self.comment_directives.push(CommentDirective {
            loc: TextRange::new(start, end),
            kind,
        });
        self.scanner_state.comment_directives_len = len + 1;
    }

    // Go: scanner/scanner.go:1009 ReScanLessThanToken
    pub fn re_scan_less_than_token(&mut self) -> SyntaxKind {
        if self.scanner_state.token == SyntaxKind::LessThanLessThanToken {
            self.scanner_state.pos = self.scanner_state.token_start + 1;
            self.scanner_state.token = SyntaxKind::LessThanToken;
        }
        self.scanner_state.token
    }

    // Go: scanner/scanner.go:1017 ReScanGreaterThanToken
    pub fn re_scan_greater_than_token(&mut self) -> SyntaxKind {
        if self.scanner_state.token == SyntaxKind::GreaterThanToken {
            self.re_scan_greater_than_token_inner();
        }
        self.scanner_state.token
    }

    // Go: scanner/scanner.go:1024 reScanGreaterThanTokenInner
    pub(crate) fn re_scan_greater_than_token_inner(&mut self) {
        self.scanner_state.pos = self.scanner_state.token_start + 1;
        if self.char() == i32::from(b'>') {
            if self.char_at(1) == i32::from(b'>') {
                if self.char_at(2) == i32::from(b'=') {
                    self.scanner_state.pos += 3;
                    self.scanner_state.token =
                        SyntaxKind::GreaterThanGreaterThanGreaterThanEqualsToken;
                } else {
                    self.scanner_state.pos += 2;
                    self.scanner_state.token = SyntaxKind::GreaterThanGreaterThanGreaterThanToken;
                }
            } else if self.char_at(1) == i32::from(b'=') {
                self.scanner_state.pos += 2;
                self.scanner_state.token = SyntaxKind::GreaterThanGreaterThanEqualsToken;
            } else {
                self.scanner_state.pos += 1;
                self.scanner_state.token = SyntaxKind::GreaterThanGreaterThanToken;
            }
        } else if self.char() == i32::from(b'=') {
            self.scanner_state.pos += 1;
            self.scanner_state.token = SyntaxKind::GreaterThanEqualsToken;
        }
    }

    // Go: scanner/scanner.go:1048 ReScanTemplateToken
    pub fn re_scan_template_token(&mut self, is_tagged_template: bool) -> SyntaxKind {
        self.scanner_state.pos = self.scanner_state.token_start;
        self.scanner_state.token = self.scan_template_and_set_token_value(!is_tagged_template);
        self.scanner_state.token
    }

    // Go: scanner/scanner.go:1054 ReScanAsteriskEqualsToken
    pub fn re_scan_asterisk_equals_token(&mut self) -> SyntaxKind {
        if self.scanner_state.token != SyntaxKind::AsteriskEqualsToken {
            panic!("'ReScanAsteriskEqualsToken' should only be called on a '*='");
        }
        self.scanner_state.pos = self.scanner_state.token_start + 1;
        self.scanner_state.token = SyntaxKind::EqualsToken;
        self.scanner_state.token
    }

    // Go: scanner/scanner.go:1063 ReScanSlashToken
    // PORT: Go takes `reportErrors ...bool` and reads only the first value.
    pub fn re_scan_slash_token(&mut self, report_errors: bool) -> SyntaxKind {
        let should_report_errors = report_errors;
        if self.scanner_state.token == SyntaxKind::SlashToken
            || self.scanner_state.token == SyntaxKind::SlashEqualsToken
        {
            // Quickly get to the end of regex such that we know the flags
            let start_of_reg_exp_body = self.scanner_state.token_start + 1;
            let mut p = start_of_reg_exp_body;
            let mut in_escape = false;
            let mut named_capture_groups = false;
            // Although nested character classes are allowed in Unicode Sets mode,
            // an unescaped slash is nevertheless invalid even in a character class in any Unicode mode.
            // This is indicated by Section 12.9.5 Regular Expression Literals of the specification,
            // where nested character classes are not considered at all. (A `[` RegularExpressionClassChar
            // does nothing in a RegularExpressionClass, and a `]` always closes the class.)
            // Additionally, parsing nested character classes will misinterpret regexes like `/[[]/`
            // as unterminated, consuming characters beyond the slash. (This even applies to `/[[]/v`,
            // which should be parsed as a well-terminated regex with an incomplete character class.)
            // Thus we must not handle nested character classes in the first pass.
            let mut in_character_class = false;
            {
                let text = self.text.as_bytes();
                let end = self.end;
                loop {
                    // If we reach the end of a file, or hit a newline, then this is an unterminated
                    // regex. Report error and return what we have so far.
                    if p >= end {
                        self.scanner_state.token_flags |= TokenFlags::UNTERMINATED;
                        break;
                    }
                    let ch = text[p as usize];
                    if is_line_break(char::from(ch)) {
                        self.scanner_state.token_flags |= TokenFlags::UNTERMINATED;
                        break;
                    } else if in_escape {
                        // Parsing an escape character;
                        // reset the flag and just advance to the next char.
                        in_escape = false;
                    } else if ch == b'/' && !in_character_class {
                        // A slash within a character class is permissible,
                        // but in general it signals the end of the regexp literal.
                        break;
                    } else if ch == b'[' {
                        in_character_class = true;
                    } else if ch == b'\\' {
                        in_escape = true;
                    } else if ch == b']' {
                        in_character_class = false;
                    } else if !in_character_class
                        && ch == b'('
                        && p + 1 < end
                        && text[(p + 1) as usize] == b'?'
                        && p + 2 < end
                        && text[(p + 2) as usize] == b'<'
                        && (p + 3 >= end
                            || (text[(p + 3) as usize] != b'=' && text[(p + 3) as usize] != b'!'))
                    {
                        named_capture_groups = true;
                    }
                    p += 1;
                }
            }

            let end_of_reg_exp_body = p;
            if self
                .scanner_state
                .token_flags
                .intersects(TokenFlags::UNTERMINATED)
            {
                // Search for the nearest unbalanced bracket for better recovery. Since the expression is
                // invalid anyways, we take nested square brackets into consideration for the best guess.
                p = start_of_reg_exp_body;
                in_escape = false;
                let mut character_class_depth = 0;
                let mut in_decimal_quantifier = false;
                let mut group_depth = 0;
                {
                    let text = self.text.as_bytes();
                    while p < end_of_reg_exp_body {
                        let ch = text[p as usize];
                        if in_escape {
                            in_escape = false;
                        } else if ch == b'\\' {
                            in_escape = true;
                        } else if ch == b'[' {
                            character_class_depth += 1;
                        } else if ch == b']' && character_class_depth != 0 {
                            character_class_depth -= 1;
                        } else if character_class_depth == 0 {
                            if ch == b'{' {
                                in_decimal_quantifier = true;
                            } else if ch == b'}' && in_decimal_quantifier {
                                in_decimal_quantifier = false;
                            } else if !in_decimal_quantifier {
                                if ch == b'(' {
                                    group_depth += 1;
                                } else if ch == b')' && group_depth != 0 {
                                    group_depth -= 1;
                                } else if ch == b')' || ch == b']' || ch == b'}' {
                                    // We encountered an unbalanced bracket outside a character class. Treat this position as the end of regex.
                                    break;
                                }
                            }
                        }
                        p += 1;
                    }
                }
                // Whitespaces and semicolons at the end are not likely to be part of the regex
                while p > start_of_reg_exp_body {
                    let (ch, size) = utf8_decode_last_rune_in_string(&self.text, p as usize);
                    if is_white_space_like(rune_to_char(ch)) || ch == i32::from(b';') {
                        p -= size;
                    } else {
                        break;
                    }
                }
                self.error_at(
                    diag::Unterminated_regular_expression_literal,
                    self.scanner_state.token_start,
                    p - self.scanner_state.token_start,
                    Vec::new(),
                );
            } else {
                // Consume the slash character
                p += 1;
                let mut reg_exp_flags = RegularExpressionFlags::NONE;
                while p < self.end {
                    let (ch, size) = utf8_decode_rune_in_string(&self.text, p as usize);
                    if ch == RUNE_ERROR || !is_identifier_part(rune_to_char(ch)) {
                        break;
                    }
                    if should_report_errors {
                        match char_code_to_reg_exp_flag(ch) {
                            None => {
                                self.error_at(
                                    diag::Unknown_regular_expression_flag,
                                    p,
                                    size,
                                    Vec::new(),
                                );
                            }
                            Some(flag) => {
                                if reg_exp_flags.intersects(flag) {
                                    self.error_at(
                                        diag::Duplicate_regular_expression_flag,
                                        p,
                                        size,
                                        Vec::new(),
                                    );
                                } else if (reg_exp_flags | flag)
                                    .contains(RegularExpressionFlags::ANY_UNICODE_MODE)
                                {
                                    self.error_at(
                                        diag::The_Unicode_u_flag_and_the_Unicode_Sets_v_flag_cannot_be_set_simultaneously,
                                        p,
                                        size,
                                        Vec::new(),
                                    );
                                } else {
                                    reg_exp_flags |= flag;
                                    self.check_regular_expression_flag_availability(flag, p, size);
                                }
                            }
                        }
                    }
                    p += size;
                }
                if should_report_errors {
                    self.scanner_state.pos = start_of_reg_exp_body;
                    let save_end = self.end;
                    let save_token_pos = self.scanner_state.token_start;
                    let save_token_flags = self.scanner_state.token_flags;
                    self.end = end_of_reg_exp_body;
                    let mut parser = RegExpParser {
                        scanner: &mut *self,
                        end: end_of_reg_exp_body,
                        reg_exp_flags,
                        any_unicode_mode: reg_exp_flags
                            .intersects(RegularExpressionFlags::ANY_UNICODE_MODE),
                        unicode_sets_mode: reg_exp_flags
                            .intersects(RegularExpressionFlags::UNICODE_SETS),
                        annex_b: true,
                        any_unicode_mode_or_non_annex_b: false,
                        named_capture_groups,
                        may_contain_strings: false,
                        number_of_capturing_groups: 0,
                        group_specifiers: FxHashMap::default(),
                        group_name_references: Vec::new(),
                        decimal_escapes: Vec::new(),
                        named_capturing_groups: Vec::new(),
                        pending_low_surrogate: 0,
                    };
                    parser.run();
                    self.end = save_end;
                    self.scanner_state.pos = p;
                    self.scanner_state.token_start = save_token_pos;
                    self.scanner_state.token_flags = save_token_flags;
                } else {
                    self.scanner_state.pos = p;
                }
            }

            self.scanner_state.pos = p;
            let value = self.text_token_value(
                self.scanner_state.token_start as usize,
                self.scanner_state.pos as usize,
            );
            self.scanner_state.token_value = value;
            self.scanner_state.token = SyntaxKind::RegularExpressionLiteral;
        }
        self.scanner_state.token
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Go `textToKeyword[text]` read from the table.
    fn table_keyword(text: &str) -> SyntaxKind {
        TEXT_TO_KEYWORD
            .iter()
            .find(|&&(t, _)| t == text)
            .map_or(SyntaxKind::Unknown, |&(_, kind)| kind)
    }

    // `text_to_keyword` is a hand-written match. Every keyword and some near
    // misses (longer, shorter, other case, other last byte) must give the
    // table result.
    #[test]
    fn text_to_keyword_matches_table() {
        for &(text, kind) in TEXT_TO_KEYWORD {
            assert_eq!(text_to_keyword(text), kind, "{text}");
            let last = text.len() - 1;
            let near = [
                format!("{text}s"),
                text[..last].to_string(),
                text.to_uppercase(),
                format!("{}z", &text[..last]),
            ];
            for miss in &near {
                assert_eq!(text_to_keyword(miss), table_keyword(miss), "{miss}");
            }
        }
        assert_eq!(text_to_keyword(""), SyntaxKind::Unknown);
        assert_eq!(text_to_keyword("{"), SyntaxKind::Unknown);
    }
}
