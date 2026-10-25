//! Go: diagnostics/loc_generated.go, and the localization part of
//! diagnostics/diagnostics.go (`(*Message).Localize`, `Localize`,
//! `getLocalizedMessages`, `Format`).
//!
//! PORT: the message catalogs are the Go files `diagnostics/loc/*.json.gz`,
//! copied unchanged to `crates/ts_goport/data/loc/`. The default build
//! embeds them with `include_bytes!` in place of Go `//go:embed`. A
//! `noembed` build (the release tsc) reads them from `<lib dir>/loc/` when a
//! run first needs one (only with a locale), so they are not in the binary
//! (1.1 MB). Go embeds them in its noembed build too.
//! PORT: Go `sync.OnceValue` is a `OnceLock` per catalog, and the Go
//! `sync.Map` cache is a `Mutex` around a map.

use crate::prelude::*;

use crate::diagnostics::Message;
use crate::frontend::json::json_unmarshal;
use crate::locale::{Locale, language};
use std::io::Read;
use std::sync::{LazyLock, Mutex, OnceLock, PoisonError};

// Go: diagnostics/diagnostics.go:44 Key
// PORT: a key is a `&str` (`Message::key`); catalogs map `String` keys.

/// A localized message catalog: message key to localized text.
pub type LocaleMessages = FxHashMap<String, String>;

// Go: diagnostics/loc_generated.go:15 matcher
static MATCHER: LazyLock<language::Matcher> = LazyLock::new(|| {
    language::new_matcher(&[
        language::english(),
        language::must_parse("zh-CN"),
        language::must_parse("zh-TW"),
        language::must_parse("cs-CZ"),
        language::must_parse("de-DE"),
        language::must_parse("es-ES"),
        language::must_parse("fr-FR"),
        language::must_parse("it-IT"),
        language::must_parse("ja-JP"),
        language::must_parse("ko-KR"),
        language::must_parse("pl-PL"),
        language::must_parse("pt-BR"),
        language::must_parse("ru-RU"),
        language::must_parse("tr-TR"),
    ])
});

// Go: diagnostics/loc_generated.go:32 localeFuncs
static LOCALE_FUNCS: [Option<fn() -> &'static LocaleMessages>; 14] = [
    None, // English (default)
    Some(zh_cn),
    Some(zh_tw),
    Some(cs_cz),
    Some(de_de),
    Some(es_es),
    Some(fr_fr),
    Some(it_it),
    Some(ja_jp),
    Some(ko_kr),
    Some(pl_pl),
    Some(pt_br),
    Some(ru_ru),
    Some(tr_tr),
];

// Go: diagnostics/loc_generated.go:49 loadLocaleData
/// PORT: Go creates the gzip reader first and panics if the header is bad.
/// `flate2` reads the header lazily, so a bad header panics with the first
/// message. Go `gzip.Reader` reads concatenated members, like
/// `MultiGzDecoder`. The embedded data is valid, so neither panic happens.
/// The error is Go's panic text; the embed build panics with it.
fn load_locale_data(data: &[u8]) -> Result<LocaleMessages, String> {
    let mut gr = flate2::read::MultiGzDecoder::new(data);
    let mut decoded = Vec::new();
    if let Err(err) = gr.read_to_end(&mut decoded) {
        return Err(format!("failed to create gzip reader: {err}"));
    }
    let mut result = LocaleMessages::default();
    if let Err(err) = json_unmarshal(&decoded, &mut result, &[]) {
        return Err(format!("failed to unmarshal locale data: {err}"));
    }
    Ok(result)
}

/// Go: one `//go:embed loc/<locale>.json.gz` variable and its
/// `sync.OnceValue` loader.
#[cfg(not(feature = "noembed"))]
macro_rules! locale_data {
    ($data:ident, $func:ident, $file:literal) => {
        static $data: &[u8] = include_bytes!(concat!("../data/loc/", $file));

        fn $func() -> &'static LocaleMessages {
            static MESSAGES: OnceLock<LocaleMessages> = OnceLock::new();
            MESSAGES.get_or_init(|| load_locale_data($data).unwrap_or_else(|err| panic!("{err}")))
        }
    };
}

/// noembed: the loader reads `<lib dir>/loc/<file>` (`load_locale_file`).
/// `$data` is not used.
#[cfg(feature = "noembed")]
macro_rules! locale_data {
    ($data:ident, $func:ident, $file:literal) => {
        fn $func() -> &'static LocaleMessages {
            static MESSAGES: OnceLock<LocaleMessages> = OnceLock::new();
            MESSAGES.get_or_init(|| load_locale_file($file))
        }
    };
}

/// noembed: the catalog `file` from `<lib dir>/loc/`, where
/// `scripts/copy-libs.sh` and the release packages put it. A file that is
/// missing or bad ends the run with a clear error, not a panic and not
/// English text.
// PORT: not in Go (Go embeds the catalogs in its noembed build too).
#[cfg(feature = "noembed")]
#[cold]
fn load_locale_file(file: &str) -> LocaleMessages {
    let path = locale_file_path(file);
    read_locale_file(&path).unwrap_or_else(|err| locale_file_exit(file, &err))
}

/// noembed: the path of catalog `file`.
#[cfg(feature = "noembed")]
fn locale_file_path(file: &str) -> String {
    format!("{}/loc/{file}", crate::frontend::bundled::lib_path())
}

/// noembed: ends the run when catalog `file` cannot be read (`err`).
#[cfg(feature = "noembed")]
#[cold]
fn locale_file_exit(file: &str, err: &str) -> ! {
    let locale = file.trim_end_matches(".json.gz");
    // The output so far comes first, as for a panic (bin/tsgo.rs).
    crate::execute::tsc::stdio::flush_cli_stdout_at_exit();
    let text = format!(
        "error: cannot read the {locale} messages: {err}\n\
         This tsc reads them from the loc directory next to it. Install it again.\n"
    );
    let _ = std::io::Write::write_all(&mut std::io::stderr(), text.as_bytes());
    // Go `ExitStatusDiagnosticsPresent_OutputsSkipped`, as for the other
    // setup errors (an invalid locale is TS6048).
    std::process::exit(1)
}

/// The catalog file of each `LOCALE_FUNCS` entry (the `locale_data!` lines
/// below), for `preload_localized_messages`.
// PORT: not in Go.
static LOCALE_FILES: [&str; 14] = [
    "",
    "zh-CN.json.gz",
    "zh-TW.json.gz",
    "cs-CZ.json.gz",
    "de-DE.json.gz",
    "es-ES.json.gz",
    "fr-FR.json.gz",
    "it-IT.json.gz",
    "ja-JP.json.gz",
    "ko-KR.json.gz",
    "pl-PL.json.gz",
    "pt-BR.json.gz",
    "ru-RU.json.gz",
    "tr-TR.json.gz",
];

/// In a noembed build, opens the catalog file of `locale`, so that a
/// missing file ends the run before any output (`locale_file_exit`), not
/// after a partial diagnostic line. The tsc command line calls it first.
/// The read and decode wait for the first message, as in Go (they cost
/// about 17M instructions). The default build does nothing here.
// PORT: not in Go.
pub fn preload_localized_messages(locale: &Locale) {
    #[cfg(feature = "noembed")]
    if locale.0 != language::Tag::UND {
        let (index, confidence) = MATCHER.match_(std::slice::from_ref(&locale.0));
        if confidence >= language::Confidence::Low
            && let Some(&file) = LOCALE_FILES.get(index).filter(|file| !file.is_empty())
        {
            let path = locale_file_path(file);
            if let Err(err) = std::fs::File::open(&path) {
                locale_file_exit(file, &format!("{path}: {err}"));
            }
        }
    }
}

/// Reads and decodes the catalog file at `path` (the noembed loader).
// PORT: not in Go.
fn read_locale_file(path: &str) -> Result<LocaleMessages, String> {
    let data = std::fs::read(path).map_err(|err| format!("{path}: {err}"))?;
    load_locale_data(&data).map_err(|err| format!("{path}: {err}"))
}

// Go: diagnostics/loc_generated.go:63 zhCNData, zhCN
locale_data!(ZH_CN_DATA, zh_cn, "zh-CN.json.gz");
// Go: diagnostics/loc_generated.go:70 zhTWData, zhTW
locale_data!(ZH_TW_DATA, zh_tw, "zh-TW.json.gz");
// Go: diagnostics/loc_generated.go:77 csCZData, csCZ
locale_data!(CS_CZ_DATA, cs_cz, "cs-CZ.json.gz");
// Go: diagnostics/loc_generated.go:84 deDEData, deDE
locale_data!(DE_DE_DATA, de_de, "de-DE.json.gz");
// Go: diagnostics/loc_generated.go:91 esESData, esES
locale_data!(ES_ES_DATA, es_es, "es-ES.json.gz");
// Go: diagnostics/loc_generated.go:98 frFRData, frFR
locale_data!(FR_FR_DATA, fr_fr, "fr-FR.json.gz");
// Go: diagnostics/loc_generated.go:105 itITData, itIT
locale_data!(IT_IT_DATA, it_it, "it-IT.json.gz");
// Go: diagnostics/loc_generated.go:112 jaJPData, jaJP
locale_data!(JA_JP_DATA, ja_jp, "ja-JP.json.gz");
// Go: diagnostics/loc_generated.go:119 koKRData, koKR
locale_data!(KO_KR_DATA, ko_kr, "ko-KR.json.gz");
// Go: diagnostics/loc_generated.go:126 plPLData, plPL
locale_data!(PL_PL_DATA, pl_pl, "pl-PL.json.gz");
// Go: diagnostics/loc_generated.go:133 ptBRData, ptBR
locale_data!(PT_BR_DATA, pt_br, "pt-BR.json.gz");
// Go: diagnostics/loc_generated.go:140 ruRUData, ruRU
locale_data!(RU_RU_DATA, ru_ru, "ru-RU.json.gz");
// Go: diagnostics/loc_generated.go:147 trTRData, trTR
locale_data!(TR_TR_DATA, tr_tr, "tr-TR.json.gz");

// Go: diagnostics/diagnostics.go:67 (*Message).Localize
/// PORT: Go takes `...any` and stringifies them (`StringifyArgs`); Rust
/// callers pass the strings.
pub fn message_localize(m: &'static Message, locale: &Locale, args: &[String]) -> String {
    localize(locale, Some(m), "", args)
}

// Go: diagnostics/diagnostics.go:83 Localize
/// PORT: Go `message *Message` is an `Option`; `None` looks the message up
/// by `key`.
pub fn localize(
    locale: &Locale,
    message: Option<&'static Message>,
    key: &str,
    args: &[String],
) -> String {
    let mut message = message;
    if message.is_none() {
        message = crate::diag::key_to_message(key);
    }
    let Some(message) = message else {
        crate::core::go_panic(format!("Unknown diagnostic message: {key}"));
    };

    let mut text = message.text();
    if let Some(localized) =
        get_localized_messages(&locale.0).and_then(|messages| messages.get(message.key()))
    {
        text = localized;
    }

    format(text, args)
}

// Go: diagnostics/diagnostics.go:99 localizedMessagesCache
static LOCALIZED_MESSAGES_CACHE: LazyLock<
    Mutex<FxHashMap<language::Tag, Option<&'static LocaleMessages>>>,
> = LazyLock::new(|| Mutex::new(FxHashMap::default()));

// Go: diagnostics/diagnostics.go:101 getLocalizedMessages
/// PORT: `pub` for the TestLocaleFiles port
/// (tests/go_baselines/units_platform/diagnostics_locale.rs); Go tests it
/// from inside the package.
pub fn get_localized_messages(loc: &language::Tag) -> Option<&'static LocaleMessages> {
    // PORT: the wasm build has no catalogs (1 MB; see npm/wasm/README.md),
    // so every locale gives English there. The link drops the catalogs.
    if cfg!(target_family = "wasm") || *loc == language::Tag::UND {
        return None;
    }

    // Check cache first
    if let Some(cached) = LOCALIZED_MESSAGES_CACHE
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(loc)
    {
        return *cached;
    }

    let mut messages = None;

    let (index, confidence) = MATCHER.match_(std::slice::from_ref(loc));
    if confidence >= language::Confidence::Low && index < LOCALE_FUNCS.len() {
        if let Some(f) = LOCALE_FUNCS[index] {
            messages = Some(f());
        }
    }

    LOCALIZED_MESSAGES_CACHE
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(loc.clone(), messages);
    messages
}

// Go: diagnostics/diagnostics.go:127 placeholderRegexp
// Go: diagnostics/diagnostics.go:129 Format
/// PORT: Go replaces `{(\d+)}` with `regexp.ReplaceAllStringFunc`. The loop
/// below finds the same leftmost, non-overlapping matches (`\d` is ASCII in
/// Go regexp).
pub fn format(text: &str, args: &[String]) -> String {
    if args.is_empty() {
        return text.to_string();
    }

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

    let bytes = text.as_bytes();
    let mut result = String::with_capacity(text.len());
    let mut last = 0;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' {
            let digits = bytes[i + 1..]
                .iter()
                .take_while(|c| c.is_ascii_digit())
                .count();
            if digits > 0 && bytes.get(i + 1 + digits) == Some(&b'}') {
                // Go panics when the index does not parse or has no arg,
                // for example on a `.tsbuildinfo` diagnostic with too few
                // `messageArgs`.
                let Some(arg) = text[i + 1..i + 1 + digits]
                    .parse::<i64>()
                    .ok()
                    .and_then(|index| usize::try_from(index).ok())
                    .and_then(|index| args.get(index))
                else {
                    crate::core::go_panic("Invalid formatting placeholder".to_string());
                };
                result.push_str(&text[last..i]);
                result.push_str(arg);
                i += digits + 2;
                last = i;
                continue;
            }
        }
        i += 1;
    }
    result.push_str(&text[last..]);
    result
}

// Go: diagnostics/diagnostics.go:148 StringifyArgs
// PORT: not ported; Rust callers pass `String` args (see `message_localize`).

#[cfg(test)]
mod tests {
    use super::*;

    fn locale(s: &str) -> Locale {
        let (l, ok) = crate::locale::parse(s);
        assert!(ok, "{s} should parse");
        l
    }

    #[test]
    fn parse_reports_invalid_tags() {
        assert!(!crate::locale::parse("").1);
        assert!(!crate::locale::parse("xx").1);
        assert!(!crate::locale::parse("de-AB").1);
        assert!(!crate::locale::parse("toolongtagname").1);
        assert!(crate::locale::parse("de").1);
        assert!(crate::locale::parse("ja-jp").1);
        assert!(crate::locale::parse("en_US").1);
    }

    #[test]
    fn locales_match_like_tsgo() {
        // (tag, index in LOCALE_FUNCS or None for English), checked against
        // x/text v0.38.0 and tsgo-oracle --locale.
        let cases: [(&str, Option<usize>); 16] = [
            ("en-US", None),
            ("de", Some(4)),
            ("de-AT", Some(4)),
            ("zh-cn", Some(1)),
            ("zh-HK", Some(2)),
            ("zh-Hant", Some(2)),
            ("pt-PT", Some(11)),
            ("es-419", Some(5)),
            ("gl", Some(5)),
            ("gsw", Some(4)),
            ("be", Some(12)),
            ("und-JP", Some(8)),
            ("uk", None),
            ("sr", None),
            ("yue", None),
            ("x-foo", None),
        ];
        for (tag, want) in cases {
            let messages = get_localized_messages(&locale(tag).0);
            let want = want
                .and_then(|i| LOCALE_FUNCS[i])
                .map(|f| f() as *const LocaleMessages);
            let got = messages.map(|m| m as *const LocaleMessages);
            assert_eq!(got, want, "{tag}");
        }
    }

    #[test]
    fn localizes_messages() {
        let m = diag::Type_0_is_not_assignable_to_type_1;
        let args = args!["string", "number"];
        assert_eq!(
            message_localize(m, &locale("de"), &args),
            "Der Typ \"string\" kann dem Typ \"number\" nicht zugewiesen werden."
        );
        assert_eq!(
            message_localize(m, &locale("zh-cn"), &args),
            "不能将类型“string”分配给类型“number”。"
        );
        assert_eq!(
            message_localize(m, &crate::locale::DEFAULT, &args),
            "Type 'string' is not assignable to type 'number'."
        );
    }

    #[test]
    fn every_catalog_loads() {
        for f in LOCALE_FUNCS.iter().flatten() {
            assert!(!f().is_empty());
        }
    }

    /// `LOCALE_FILES` names the catalog of each `LOCALE_FUNCS` entry.
    #[test]
    fn locale_files_name_the_catalogs() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/loc");
        for (file, f) in LOCALE_FILES.iter().zip(LOCALE_FUNCS) {
            match f {
                None => assert!(file.is_empty()),
                Some(f) => assert_eq!(&read_locale_file(&format!("{dir}/{file}")).unwrap(), f()),
            }
        }
    }

    /// The noembed loader: a catalog file gives the embedded messages, and
    /// a missing or bad file gives an error that names it.
    #[test]
    fn locale_files_read_like_the_embedded_catalogs() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/loc");
        let read = read_locale_file(&format!("{dir}/de-DE.json.gz")).unwrap();
        assert_eq!(&read, de_de());

        let missing = format!("{dir}/xx-XX.json.gz");
        let err = read_locale_file(&missing).unwrap_err();
        assert!(err.starts_with(&format!("{missing}: ")), "{err}");

        let bad = concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml");
        let err = read_locale_file(bad).unwrap_err();
        assert!(
            err.starts_with(&format!("{bad}: failed to create gzip reader")),
            "{err}"
        );
    }

    #[test]
    fn format_matches_go_regexp() {
        let args = args!["a", "b"];
        assert_eq!(format("{0}{1}", &args), "ab");
        assert_eq!(format("{{0}} {x} {} {1", &args), "{a} {x} {} {1");
        assert_eq!(format("{0}", &[]), "{0}");
        // Go `strings.ToValidUTF8`: a run of invalid bytes (port form units,
        // see `scanner_util::GO_STRING_MARKER`) becomes one U+FFFD, and a
        // real U+FDD0 stays.
        let args = args!["a\u{FDD0}\u{10F7FE}\u{FDD0}\u{10F7FF}b", "\u{FDD0}\u{FDD0}"];
        assert_eq!(format("{0} {1}", &args), "a\u{FFFD}b \u{FDD0}\u{FDD0}");
    }
}
