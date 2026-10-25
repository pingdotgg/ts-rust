//! Go: locale/locale.go, with the part of `golang.org/x/text` v0.38.0 that
//! tsgo reaches through it and through `diagnostics/loc_generated.go`:
//! `language.Parse`, `language.MustParse`, `language.NewMatcher` and
//! `(*matcher).Match`, plus the `internal/language` and `internal/tag` code
//! under them.
//!
//! PORT: x/text is ported only as far as tsgo reaches it. tsgo parses a
//! `--locale` value with `language.Parse` and matches the result against its
//! fixed list of shipped locales. `gostd::collate` also needs `Compose`,
//! `Builder`, `TypeForKey`, `SetTypeForKey`, `Parent`, `Extensions`, `Base`
//! and the compact tags. No port code calls `gostd::collate` now: Go
//! organize imports has its own comparer from pin 52168999f3dc on, and Go N
//! imports no `x/text/collate`. Left out: the Accept-Language
//! parser, coverage, match options, and the part of `Match` that adjusts the
//! matched tag (tsgo drops the tag and reads only the index and the
//! confidence).
//!
//! PORT: Go `language.Tag` is a `compact.Tag`: common tags are stored as an
//! index, and other tags keep the full `internal/language.Tag`. The compact
//! form is ported (`compact`) but only `gostd::collate` uses it (through
//! `language::make_tag`); elsewhere `language::Tag` is the full tag. `compact.Make`
//! followed by `(*compact.Tag).Tag` gives back the same language, script,
//! region and variants, which is all that the matcher reads. A tag equals
//! Go `language.Und` exactly when it equals `Tag::UND`. Only the tag text can
//! differ: `compact.Make` drops a `-u-rg-` extension that names the tag's own
//! region (Go prints `es-US-u-rg-uszzzz` as `es-US`).
//!
//! PORT: the generated x/text tables are in the `tables` module at the end of
//! this file. They were printed from the pinned x/text sources by a Go
//! program that reads the Go tables, so the values are the Go values.

use crate::prelude::*;

use crate::gostd::context::{self, Context, ContextKey};

// Go: locale/locale.go:9 contextKey
static CONTEXT_KEY: ContextKey<Locale> = ContextKey::new("contextKey(0)");

// Go: locale/locale.go:22 WithLocale
pub fn with_locale(ctx: &Context, locale: Locale) -> Context {
    context::with_value(ctx, &CONTEXT_KEY, locale)
}

// Go: locale/locale.go:26 FromContext
pub fn from_context(ctx: &Context) -> Locale {
    match ctx.value(&CONTEXT_KEY) {
        Some(locale) => (*locale).clone(),
        None => Locale::default(),
    }
}

// Go: locale/locale.go:31 HasLocale (ts#64163)
pub fn has_locale(ctx: &Context) -> bool {
    ctx.value(&CONTEXT_KEY).is_some()
}

// Go: locale/locale.go:11 Locale
/// Go `locale.Locale` (`language.Tag`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Locale(pub language::Tag);

// Go: locale/locale.go:13 Default
// PORT: Go `var Default Locale` is never assigned, so it is the zero tag
// (`und`).
pub const DEFAULT: Locale = Locale(language::Tag::UND);

impl Locale {
    // Go: locale/locale.go:15 (Locale).String (tsgo#4712)
    /// The locale tag text, or "" for the default locale.
    #[must_use]
    pub fn string(&self) -> String {
        if *self == DEFAULT {
            return String::new();
        }
        self.0.string()
    }
}

// Go: locale/locale.go:36 Parse
pub fn parse(locale_str: &str) -> (Locale, bool) {
    // Parse gracefully fails.
    let (tag, err) = language::parse(locale_str);
    (Locale(tag), err.is_none())
}

/// Go: `golang.org/x/text/language` (language.go, parse.go, match.go).
pub mod language {
    use super::internal_language as il;
    use super::tables::*;
    use crate::prelude::FxHashMap;
    use std::sync::LazyLock;

    pub use il::{Error, Language, Region, Script, Tag};

    // Go: language/language.go:67 CanonType
    pub type CanonType = i32;

    // Go: language/language.go:69 (CanonType constants)
    /// Replace deprecated base languages with their preferred replacements.
    pub const DEPRECATED_BASE: CanonType = 1 << 0;
    /// Replace deprecated scripts with their preferred replacements.
    pub const DEPRECATED_SCRIPT: CanonType = 1 << 1;
    /// Replace deprecated regions with their preferred replacements.
    pub const DEPRECATED_REGION: CanonType = 1 << 2;
    /// Remove redundant scripts.
    pub const SUPPRESS_SCRIPT: CanonType = 1 << 3;
    /// Normalize legacy encodings. This includes legacy languages defined in
    /// CLDR as well as bibliographic codes defined in ISO-639.
    pub const LEGACY: CanonType = 1 << 4;
    /// Map the dominant language of a macro language group to the macro language
    /// subtag. For example cmn -> zh.
    pub const MACRO: CanonType = 1 << 5;
    /// The CLDR flag should be used if full compatibility with CLDR is required.
    /// There are a few cases where language.Tag may differ from CLDR. To follow all
    /// of CLDR's suggestions, use All|CLDR.
    pub const CLDR: CanonType = 1 << 6;

    /// Raw can be used to Compose or Parse without Canonicalization.
    pub const RAW: CanonType = 0;

    /// Replace all deprecated tags with their preferred replacements.
    pub const DEPRECATED: CanonType = DEPRECATED_BASE | DEPRECATED_SCRIPT | DEPRECATED_REGION;

    /// All canonicalizations recommended by BCP 47.
    pub const BCP47: CanonType = DEPRECATED | SUPPRESS_SCRIPT;

    /// All canonicalizations.
    pub const ALL: CanonType = BCP47 | LEGACY | MACRO;

    /// Default is the canonicalization used by Parse, Make and Compose. To
    /// preserve as much information as possible, canonicalizations that remove
    /// potentially valuable information are not included. The Matcher is
    /// designed to recognize similar tags that would be the same if
    /// they were canonicalized using All.
    pub const DEFAULT: CanonType = DEPRECATED | LEGACY;

    const CANON_LANG: CanonType = DEPRECATED_BASE | LEGACY | MACRO;

    // Go: language/language.go:24 makeTag
    // Go: language/language.go:28 (*Tag).tag
    /// Go `makeTag(t).tag()`: the full tag of the compact tag that Go keeps
    /// for t. It equals t except that a `-u-rg-` extension that names the
    /// tag's own region is dropped.
    /// PORT: most of the port keeps a public tag as its full tag and skips
    /// this step (see the module note). `gostd::collate` calls it where Go
    /// makes a tag, so that tag equality matches Go's compact `==`.
    pub fn make_tag(t: &Tag) -> Tag {
        super::compact::make(t).tag()
    }

    // Go: language/parse.go:107 update (the part types)
    /// PORT: Go `Compose` takes `...interface{}`. These are the part types
    /// that the port passes.
    pub enum ComposePart<'a> {
        Tag(&'a Tag),
        Base(Language),
        Script(Script),
        Region(Region),
        /// Go `[]Extension`
        Extensions(&'a [String]),
    }

    // Go: language/parse.go:89 (CanonType).Compose
    /// Compose creates a Tag from individual parts, which may be of type Tag, Base,
    /// Script, Region, Variant, []Variant, Extension, []Extension or error. If a
    /// Base, Script or Region or slice of type Variant or Extension is passed more
    /// than once, the latter will overwrite the former. Variants and Extensions are
    /// accumulated, but if two extensions of the same type are passed, the latter
    /// will replace the former. For -u extensions, though, the key-type pairs are
    /// added, where later values overwrite older ones. A Tag overwrites all former
    /// values and typically only makes sense as the first argument. The resulting
    /// tag is returned after canonicalizing using CanonType c. If one or more errors
    /// are encountered, one of the errors is returned.
    /// PORT: no part type the port passes gives an error, so none is returned.
    pub fn compose(c: CanonType, parts: &[ComposePart<'_>]) -> Tag {
        let mut b = il::Builder::default();
        // Go: language/parse.go:107 update
        for x in parts {
            match x {
                ComposePart::Tag(v) => b.set_tag(v),
                ComposePart::Base(v) => b.tag.lang_id = *v,
                ComposePart::Script(v) => b.tag.script_id = *v,
                // TODO: if range region is not a specific region (such as 001), use
                // the default region.
                ComposePart::Region(v) => b.tag.region_id = *v,
                ComposePart::Extensions(v) => {
                    b.clear_extensions();
                    for e in *v {
                        b.add_ext(e);
                    }
                }
            }
        }
        (b.tag, _) = canonicalize(c, b.tag.clone());
        make_tag(&b.make())
    }

    // Go: language/language.go:246 (Tag).Base
    /// Base returns the base language of the language tag. If the base language is
    /// unspecified, an attempt will be made to infer it from the context.
    /// It uses a variant of CLDR's Add Likely Subtags algorithm. This is subject to change.
    pub fn tag_base(t: &Tag) -> (Language, Confidence) {
        if t.lang_id.0 != 0 {
            return (t.lang_id, Confidence::Exact);
        }
        let mut c = Confidence::High;
        if t.script_id.0 == 0 && !t.region_id.is_country() {
            c = Confidence::Low;
        }
        let (tag, err) = t.maximize();
        if err.is_none() && tag.lang_id.0 != 0 {
            return (tag.lang_id, c);
        }
        (Language(0), Confidence::No)
    }

    // Go: language/language.go:343 (Tag).Parent
    /// Parent returns the CLDR parent of t. In CLDR, missing fields in data for a
    /// specific language are substituted with fields from the parent language.
    /// The parent for a language may change for newer versions of CLDR.
    ///
    /// Parent returns a tag for a less specific language that is mutually
    /// intelligible or Und if there is no such language. This may not be the same
    /// as simply stripping the last BCP 47 subtag. For instance, the parent of
    /// "zh-TW" is "zh-Hant", and the parent of "zh-Hant" is "und".
    pub fn tag_parent(t: &Tag) -> Tag {
        super::compact::make(t).parent().tag()
    }

    // Go: language/language.go:432 (Tag).SetTypeForKey
    /// SetTypeForKey returns a new Tag with the key set to type, where key and type
    /// are of the allowed values defined for the Unicode locale extension ('u') in
    /// https://www.unicode.org/reports/tr35/#Unicode_Language_and_Locale_Identifiers.
    /// An empty value removes an existing pair with the same key.
    pub fn set_type_for_key(t: &Tag, key: &str, value: &str) -> (Tag, Option<Error>) {
        let (tt, err) = t.set_type_for_key(key, value);
        (make_tag(&tt), err)
    }

    // Go: language/language.go:119 canonicalize
    /// canonicalize returns the canonicalized equivalent of the tag and
    /// whether there was any change.
    pub fn canonicalize(c: CanonType, mut t: Tag) -> (Tag, bool) {
        if c == RAW {
            return (t, false);
        }
        let mut changed = false;
        if c & SUPPRESS_SCRIPT != 0 {
            if t.lang_id.suppress_script() == t.script_id {
                t.script_id = Script(0);
                changed = true;
            }
        }
        if c & CANON_LANG != 0 {
            loop {
                let (l, alias_type) = t.lang_id.canonicalize();
                if l != t.lang_id {
                    match alias_type {
                        il::LEGACY => {
                            if c & LEGACY != 0 {
                                if t.lang_id.0 == _SH && t.script_id.0 == 0 {
                                    t.script_id = Script(_LATN);
                                }
                                t.lang_id = l;
                                changed = true;
                            }
                        }
                        il::MACRO => {
                            if c & MACRO != 0 {
                                // We deviate here from CLDR. The mapping "nb" -> "no"
                                // qualifies as a typical Macro language mapping.  However,
                                // for legacy reasons, CLDR maps "no", the macro language
                                // code for Norwegian, to the dominant variant "nb". This
                                // change is currently under consideration for CLDR as well.
                                // See https://unicode.org/cldr/trac/ticket/2698 and also
                                // https://unicode.org/cldr/trac/ticket/1790 for some of the
                                // practical implications. TODO: this check could be removed
                                // if CLDR adopts this change.
                                if c & CLDR == 0 || t.lang_id.0 != _NB {
                                    changed = true;
                                    t.lang_id = l;
                                }
                            }
                        }
                        il::DEPRECATED => {
                            if c & DEPRECATED_BASE != 0 {
                                if t.lang_id.0 == _MO && t.region_id.0 == 0 {
                                    t.region_id = Region(_MD);
                                }
                                t.lang_id = l;
                                changed = true;
                                // Other canonicalization types may still apply.
                                continue;
                            }
                        }
                        _ => {}
                    }
                } else if c & LEGACY != 0 && t.lang_id.0 == _NO && c & CLDR != 0 {
                    t.lang_id = Language(_NB);
                    changed = true;
                }
                break;
            }
        }
        if c & DEPRECATED_SCRIPT != 0 {
            if t.script_id.0 == _QAAI {
                changed = true;
                t.script_id = Script(_ZINH);
            }
        }
        if c & DEPRECATED_REGION != 0 {
            let r = t.region_id.canonicalize();
            if r != t.region_id {
                changed = true;
                t.region_id = r;
            }
        }
        (t, changed)
    }

    // Go: language/language.go:205 Confidence
    /// Confidence indicates the level of certainty for a given return value.
    /// For example, Serbian may be written in Cyrillic or Latin script.
    /// The confidence level indicates whether a value was explicitly specified,
    /// whether it is typically the only possible value, or whether there is
    /// an ambiguity.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub enum Confidence {
        /// full confidence that there was no match
        #[default]
        No = 0,
        /// most likely value picked out of a set of alternatives
        Low,
        /// value is generally assumed to be the correct match
        High,
        /// exact match or explicitly specified value
        Exact,
    }

    // Go: language/language.go:275 (Tag).Script
    /// Script infers the script for the language tag. If it was not explicitly given, it will infer
    /// a most likely candidate.
    /// PORT: Go returns a `Script` wrapper; this returns the script id.
    pub fn tag_script(t: &Tag) -> (Script, Confidence) {
        let scr = t.script_id;
        if scr.0 != 0 {
            return (scr, Confidence::Exact);
        }
        let mut tt = t.clone();
        let (mut sc, mut c) = (Script(_ZZZZ), Confidence::No);
        let scr = tt.lang_id.suppress_script();
        if scr.0 != 0 {
            // Note: it is not always the case that a language with a suppress
            // script value is only written in one script (e.g. kk, ms, pa).
            if tt.region_id.0 == 0 {
                return (scr, Confidence::High);
            }
            (sc, c) = (scr, Confidence::High);
        }
        let (tag, err) = tt.maximize();
        if err.is_none() {
            if tag.script_id != sc {
                (sc, c) = (tag.script_id, Confidence::Low);
            }
        } else {
            (tt, _) = canonicalize(DEPRECATED | MACRO, tt);
            let (tag, err) = tt.maximize();
            if err.is_none() && tag.script_id != sc {
                (sc, c) = (tag.script_id, Confidence::Low);
            }
        }
        (sc, c)
    }

    // Go: language/parse.go:33 Parse
    /// Parse parses the given BCP 47 string and returns a valid Tag. If parsing
    /// failed it returns an error and any part of the tag that could be parsed.
    /// If parsing succeeded but an unknown value was found, it returns
    /// ValueError. The Tag returned in this case is just stripped of the unknown
    /// value. All other values are preserved. It accepts tags in the BCP 47 format
    /// and extensions to this standard defined in
    /// https://www.unicode.org/reports/tr35/#Unicode_Language_and_Locale_Identifiers.
    /// The resulting tag is canonicalized using the default canonicalization type.
    pub fn parse(s: &str) -> (Tag, Option<Error>) {
        canon_type_parse(DEFAULT, s)
    }

    // Go: language/parse.go:45 (CanonType).Parse
    // PORT: Go also recovers from a panic here. The internal parser already
    // turns its reachable panics into `(Und, ErrSyntax)` (see
    // `il::parse`), and nothing below it can panic.
    pub fn canon_type_parse(c: CanonType, s: &str) -> (Tag, Option<Error>) {
        let (tt, err) = il::parse(s);
        if err.is_some() {
            return (tt, err);
        }
        let (mut tt, changed) = canonicalize(c, tt);
        if changed {
            tt.remake_string();
        }
        (tt, None)
    }

    // Go: language/tags.go:13 MustParse
    /// MustParse is like Parse, but panics if the given BCP 47 tag cannot be parsed.
    /// It simplifies safe initialization of Tag values.
    pub fn must_parse(s: &str) -> Tag {
        let (t, err) = parse(s);
        if let Some(err) = err {
            panic!("{}", err.error());
        }
        t
    }

    // Go: language/tags.go:78 English
    // PORT: Go `Tag(compact.English)`, whose full tag is the bare language.
    pub fn english() -> Tag {
        Tag {
            lang_id: Language(_EN),
            ..Tag::UND
        }
    }

    // Go: language/match.go:171 matcher
    /// matcher keeps a set of supported language tags, indexed by language.
    /// PORT: `passSettings` is left out; nothing sets it.
    #[derive(Debug)]
    pub struct Matcher {
        default_: HaveTag,
        supported: Vec<HaveTag>,
        index: FxHashMap<Language, MatchHeader>,
        prefer_same_script: bool,
    }

    // Go: language/match.go:181 matchHeader
    /// matchHeader has the lists of tags for exact matches and matches based on
    /// maximized and canonicalized tags for a given language.
    /// PORT: Go keeps `[]*haveTag`. Each list owns its entries (Go never shares
    /// an entry between lists), so they are values here.
    #[derive(Clone, Debug, Default)]
    struct MatchHeader {
        have_tags: Vec<HaveTag>,
        original: bool,
    }

    // Go: language/match.go:188 haveTag
    /// haveTag holds a supported Tag and its maximized script and region. The maximized
    /// or canonicalized language is not stored as it is not needed during matching.
    #[derive(Clone, Debug, Default)]
    struct HaveTag {
        tag: Tag,

        /// index of this tag in the original list of supported tags.
        index: usize,

        /// conf is the maximum confidence that can result from matching this haveTag.
        /// When conf < Exact this means it was inserted after applying a CLDR equivalence rule.
        conf: Confidence,

        /// Maximized region and script.
        max_region: Region,
        max_script: Script,

        /// altScript may be checked as an alternative match to maxScript. If altScript
        /// matches, the confidence level for this match is Low. Theoretically there
        /// could be multiple alternative scripts. This does not occur in practice.
        alt_script: Script,

        /// nextMax is the index of the next haveTag with the same maximized tags.
        next_max: u16,
    }

    // Go: language/match.go:210 makeHaveTag
    fn make_have_tag(tag: Tag, index: usize) -> (HaveTag, Language) {
        let mut max = tag.clone();
        if tag.lang_id.0 != 0 || tag.region_id.0 != 0 || tag.script_id.0 != 0 {
            (max, _) = canonicalize(ALL, max);
            (max, _) = max.maximize();
            max.remake_string();
        }
        let alt = alt_script(max.lang_id, max.script_id);
        (
            HaveTag {
                tag,
                index,
                conf: Confidence::Exact,
                max_region: max.region_id,
                max_script: max.script_id,
                alt_script: alt,
                next_max: 0,
            },
            max.lang_id,
        )
    }

    // Go: language/match.go:223 altScript
    /// altScript returns an alternative script that may match the given script with
    /// a low confidence.  At the moment, the langMatch data allows for at most one
    /// script to map to another and we rely on this to keep the code simple.
    fn alt_script(l: Language, s: Script) -> Script {
        for alt in &MATCH_SCRIPT {
            // TODO: also match cases where language is not the same.
            if (Language(alt.want_lang) == l || Language(alt.have_lang) == l)
                && Script(u16::from(alt.have_script)) == s
            {
                return Script(u16::from(alt.want_script));
            }
        }
        Script(0)
    }

    impl MatchHeader {
        // Go: language/match.go:236 (*matchHeader).addIfNew
        /// addIfNew adds a haveTag to the list of tags only if it is a unique tag.
        /// Tags that have the same maximized values are linked by index.
        fn add_if_new(&mut self, n: HaveTag, exact: bool) {
            self.original = self.original || exact;
            // Don't add new exact matches.
            for v in &self.have_tags {
                if equals_rest(&v.tag, &n.tag) {
                    return;
                }
            }
            // Allow duplicate maximized tags, but create a linked list to allow quickly
            // comparing the equivalents and bail out.
            for i in 0..self.have_tags.len() {
                let v = &self.have_tags[i];
                if v.max_script == n.max_script
                    && v.max_region == n.max_region
                    && v.tag.variant_or_private_use_tags() == n.tag.variant_or_private_use_tags()
                {
                    let mut i = i;
                    while self.have_tags[i].next_max != 0 {
                        i = usize::from(self.have_tags[i].next_max);
                    }
                    self.have_tags[i].next_max = self.have_tags.len() as u16;
                    break;
                }
            }
            self.have_tags.push(n);
        }
    }

    impl Matcher {
        // Go: language/match.go:260 (*matcher).header
        /// header returns the matchHeader for the given language. It creates one if
        /// it doesn't already exist.
        fn header(&mut self, l: Language) -> &mut MatchHeader {
            self.index.entry(l).or_default()
        }

        // Go: language/match.go:292 newMatcher (the `update` closure)
        /// update is used to add indexes in the map for equivalent languages.
        /// update will only add entries to original indexes, thus not computing any
        /// transitive relations.
        fn update(&mut self, want: u16, have: u16, conf: Confidence) {
            if let Some(hh) = self.index.get(&Language(have)) {
                if !hh.original {
                    return;
                }
                let hh_original = hh.original;
                // PORT: Go ranges over the slice value it read before the loop.
                let have_tags = hh.have_tags.clone();
                let hw = self.header(Language(want));
                for ht in &have_tags {
                    let mut v = ht.clone();
                    if conf < v.conf {
                        v.conf = conf;
                    }
                    v.next_max = 0; // this value needs to be recomputed
                    if v.alt_script.0 != 0 {
                        v.alt_script = alt_script(Language(want), v.max_script);
                    }
                    hw.add_if_new(v, conf == Confidence::Exact && hh_original);
                }
            }
        }
    }

    // Go: language/match.go:269 toConf
    fn to_conf(d: u8) -> Confidence {
        if d <= 10 {
            return Confidence::High;
        }
        if d < 30 {
            return Confidence::Low;
        }
        Confidence::No
    }

    // Go: language/match.go:62 NewMatcher
    /// NewMatcher returns a Matcher that matches an ordered list of preferred tags
    /// against a list of supported tags based on written intelligibility, closeness
    /// of dialect, equivalence of subtags and various other rules. It is initialized
    /// with the list of supported tags. The first element is used as the default
    /// value in case no match is found.
    /// PORT: match options are left out; tsgo passes none.
    pub fn new_matcher(t: &[Tag]) -> Matcher {
        new_matcher_inner(t)
    }

    // Go: language/match.go:282 newMatcher
    /// newMatcher builds an index for the given supported tags and returns it as
    /// a matcher. It also expands the index by considering various equivalence classes
    /// for a given tag.
    /// PORT: named `new_matcher_inner` because `NewMatcher` takes the snake name.
    fn new_matcher_inner(supported: &[Tag]) -> Matcher {
        let mut m = Matcher {
            default_: HaveTag::default(),
            supported: Vec::new(),
            index: FxHashMap::default(),
            prefer_same_script: true,
        };
        if supported.is_empty() {
            m.default_ = HaveTag::default();
            return m;
        }
        // Add supported languages to the index. Add exact matches first to give
        // them precedence.
        for (i, tag) in supported.iter().enumerate() {
            let tt = tag.clone();
            let (pair, _) = make_have_tag(tt.clone(), i);
            m.header(tt.lang_id).add_if_new(pair.clone(), true);
            m.supported.push(pair);
        }
        m.default_ = m.header(supported[0].lang_id).have_tags[0].clone();
        // Keep these in two different loops to support the case that two equivalent
        // languages are distinguished, such as iw and he.
        for (i, tag) in supported.iter().enumerate() {
            let tt = tag.clone();
            let (pair, max) = make_have_tag(tt.clone(), i);
            if max != tt.lang_id {
                m.header(max).add_if_new(pair, true);
            }
        }

        // Add entries for languages with mutual intelligibility as defined by CLDR's
        // languageMatch data.
        for ml in &MATCH_LANG {
            m.update(ml.want, ml.have, to_conf(ml.distance));
            if !ml.oneway {
                m.update(ml.have, ml.want, to_conf(ml.distance));
            }
        }

        // Add entries for possible canonicalizations. This is an optimization to
        // ensure that only one map lookup needs to be done at runtime per desired tag.
        // First we match deprecated equivalents. If they are perfect equivalents
        // (their canonicalization simply substitutes a different language code, but
        // nothing else), the match confidence is Exact, otherwise it is High.
        for (i, lm) in ALIAS_MAP.iter().enumerate() {
            // If deprecated codes match and there is no fiddling with the script
            // or region, we consider it an exact match.
            let mut conf = Confidence::Exact;
            if ALIAS_TYPES[i] != il::MACRO {
                if !is_exact_equivalent(Language(lm.from)) {
                    conf = Confidence::High;
                }
                m.update(lm.to, lm.from, conf);
            }
            m.update(lm.from, lm.to, conf);
        }
        m
    }

    impl Matcher {
        // Go: language/match.go:81 (*matcher).Match
        /// Match returns the best match for any of the given tags, along with
        /// a unique index associated with the returned tag and a confidence
        /// score.
        /// PORT: Go also returns the matched tag, with the desired region and
        /// extensions copied in. tsgo drops it, so only the index and the
        /// confidence are ported.
        pub fn match_(&self, want: &[Tag]) -> (usize, Confidence) {
            let mut index = 0;
            let (m, _w, c) = self.get_best(want);
            if let Some(m) = m {
                index = m.index;
            } else {
                // TODO: this should be an option
                if self.prefer_same_script {
                    'outer: for w in want {
                        let (script, _) = tag_script(w);
                        if script.0 == 0 {
                            // Don't do anything if there is no script, such as with
                            // private subtags.
                            continue;
                        }
                        for (i, h) in self.supported.iter().enumerate() {
                            if script == h.max_script {
                                index = i;
                                break 'outer;
                            }
                        }
                    }
                }
                // TODO: select first language tag based on script.
            }
            (index, c)
        }

        // Go: language/match.go:378 (*matcher).getBest
        /// getBest gets the best matching tag in m for any of the given tags, taking into
        /// account the order of preference of the given tags.
        fn get_best(&self, want: &[Tag]) -> (Option<HaveTag>, Tag, Confidence) {
            let mut best = BestMatch::default();
            for (i, ww) in want.iter().enumerate() {
                let mut w = ww.clone();
                let mut max: Tag;
                // Check for exact match first.
                let mut h = self.index.get(&w.lang_id);
                if w.lang_id.0 != 0 {
                    let Some(_) = h else {
                        continue;
                    };
                    // Base language is defined.
                    (max, _) = canonicalize(LEGACY | DEPRECATED | MACRO, w.clone());
                    // A region that is added through canonicalization is stronger than
                    // a maximized region: set it in the original (e.g. mo -> ro-MD).
                    if w.region_id != max.region_id {
                        w.region_id = max.region_id;
                    }
                    // TODO: should we do the same for scripts?
                    // See test case: en, sr, nl ; sh ; sr
                    (max, _) = max.maximize();
                } else {
                    // Base language is not defined.
                    if let Some(h) = h {
                        for have in &h.have_tags {
                            if equals_rest(&have.tag, &w) {
                                return (Some(have.clone()), w, Confidence::Exact);
                            }
                        }
                    }
                    if w.script_id.0 == 0 && w.region_id.0 == 0 {
                        // We skip all tags matching und for approximate matching, including
                        // private tags.
                        continue;
                    }
                    (max, _) = w.maximize();
                    h = self.index.get(&max.lang_id);
                    if h.is_none() {
                        continue;
                    }
                }
                let h = h.unwrap_or_else(|| unreachable!());
                let mut pin = true;
                for t in &want[i + 1..] {
                    if w.lang_id == t.lang_id {
                        pin = false;
                        break;
                    }
                }
                // Check for match based on maximized tag.
                for have in &h.have_tags {
                    let mut have = have;
                    best.update(have, &w, max.script_id, max.region_id, pin);
                    if best.conf == Confidence::Exact {
                        while have.next_max != 0 {
                            have = &h.have_tags[usize::from(have.next_max)];
                            best.update(have, &w, max.script_id, max.region_id, pin);
                        }
                        return (best.have, best.want, best.conf);
                    }
                }
            }
            if best.conf <= Confidence::No {
                if !want.is_empty() {
                    return (None, want[0].clone(), Confidence::No);
                }
                return (None, Tag::UND, Confidence::No);
            }
            (best.have, best.want, best.conf)
        }
    }

    // Go: language/match.go:458 bestMatch
    /// bestMatch accumulates the best match so far.
    #[derive(Default)]
    struct BestMatch {
        have: Option<HaveTag>,
        want: Tag,
        conf: Confidence,
        pinned_region: Region,
        pin_language: bool,
        same_region_group: bool,
        // Cached results from applying tie-breaking rules.
        orig_lang: bool,
        orig_reg: bool,
        paradigm_reg: bool,
        reg_group_dist: u8,
        orig_script: bool,
    }

    impl BestMatch {
        // Go: language/match.go:486 (*bestMatch).update
        /// update updates the existing best match if the new pair is considered to be a
        /// better match. To determine if the given pair is a better match, it first
        /// computes the rough confidence level. If this surpasses the current match, it
        /// will replace it and update the tie-breaker rule cache. If there is a tie, it
        /// proceeds with applying a series of tie-breaker rules. If there is no
        /// conclusive winner after applying the tie-breaker rules, it leaves the current
        /// match as the preferred match.
        ///
        /// If pin is true and have and tag are a strong match, it will henceforth only
        /// consider matches for this language. This corresponds to the idea that most
        /// users have a strong preference for the first defined language. A user can
        /// still prefer a second language over a dialect of the preferred language by
        /// explicitly specifying dialects, e.g. "en, nl, en-GB". In this case pin should
        /// be false.
        fn update(
            &mut self,
            have: &HaveTag,
            tag: &Tag,
            max_script: Script,
            max_region: Region,
            pin: bool,
        ) {
            // Bail if the maximum attainable confidence is below that of the current best match.
            let mut c = have.conf;
            if c < self.conf {
                return;
            }
            // Don't change the language once we already have found an exact match.
            if self.pin_language && tag.lang_id != self.want.lang_id {
                return;
            }
            // Pin the region group if we are comparing tags for the same language.
            if tag.lang_id == self.want.lang_id && self.same_region_group {
                let (_, same_group) = region_group_dist(
                    self.pinned_region,
                    have.max_region,
                    have.max_script,
                    self.want.lang_id,
                );
                if !same_group {
                    return;
                }
            }
            if c == Confidence::Exact && have.max_script == max_script {
                // If there is another language and then another entry of this language,
                // don't pin anything, otherwise pin the language.
                self.pin_language = pin;
            }
            if equals_rest(&have.tag, tag) {
            } else if have.max_script != max_script {
                // There is usually very little comprehension between different scripts.
                // In a few cases there may still be Low comprehension. This possibility
                // is pre-computed and stored in have.altScript.
                if Confidence::Low < self.conf || have.alt_script != max_script {
                    return;
                }
                c = Confidence::Low;
            } else if have.max_region != max_region {
                if Confidence::High < c {
                    // There is usually a small difference between languages across regions.
                    c = Confidence::High;
                }
            }

            // We store the results of the computations of the tie-breaker rules along
            // with the best match. There is no need to do the checks once we determine
            // we have a winner, but we do still need to do the tie-breaker computations.
            // We use "beaten" to keep track if we still need to do the checks.
            let mut beaten = false; // true if the new pair defeats the current one.
            if c != self.conf {
                if c < self.conf {
                    return;
                }
                beaten = true;
            }

            // Tie-breaker rules:
            // We prefer if the pre-maximized language was specified and identical.
            let orig_lang = have.tag.lang_id == tag.lang_id && tag.lang_id.0 != 0;
            if !beaten && self.orig_lang != orig_lang {
                if self.orig_lang {
                    return;
                }
                beaten = true;
            }

            // We prefer if the pre-maximized region was specified and identical.
            let orig_reg = have.tag.region_id == tag.region_id && tag.region_id.0 != 0;
            if !beaten && self.orig_reg != orig_reg {
                if self.orig_reg {
                    return;
                }
                beaten = true;
            }

            let (reg_group_dist, same_group) =
                region_group_dist(have.max_region, max_region, max_script, tag.lang_id);
            if !beaten && self.reg_group_dist != reg_group_dist {
                if reg_group_dist > self.reg_group_dist {
                    return;
                }
                beaten = true;
            }

            let paradigm_reg = is_paradigm_locale(tag.lang_id, have.max_region);
            if !beaten && self.paradigm_reg != paradigm_reg {
                if !paradigm_reg {
                    return;
                }
                beaten = true;
            }

            // Next we prefer if the pre-maximized script was specified and identical.
            let orig_script = have.tag.script_id == tag.script_id && tag.script_id.0 != 0;
            if !beaten && self.orig_script != orig_script {
                if self.orig_script {
                    return;
                }
                beaten = true;
            }

            // Update m to the newly found best match.
            if beaten {
                self.have = Some(have.clone());
                self.want = tag.clone();
                self.conf = c;
                self.pinned_region = max_region;
                self.same_region_group = same_group;
                self.orig_lang = orig_lang;
                self.orig_reg = orig_reg;
                self.paradigm_reg = paradigm_reg;
                self.orig_script = orig_script;
                self.reg_group_dist = reg_group_dist;
            }
        }
    }

    // Go: language/match.go:626 isParadigmLocale
    fn is_paradigm_locale(lang: Language, r: Region) -> bool {
        for e in PARADIGM_LOCALES.iter() {
            if Language(e[0]) == lang && (r == Region(e[1]) || r == Region(e[2])) {
                return true;
            }
        }
        false
    }

    // Go: language/match.go:635 regionGroupDist
    /// regionGroupDist computes the distance between two regions based on their
    /// CLDR grouping.
    /// PORT: Go `uint` is 64 bits on the supported platforms.
    fn region_group_dist(a: Region, b: Region, script: Script, lang: Language) -> (u8, bool) {
        const DEFAULT_DISTANCE: u8 = 4;

        let a_group = u64::from(REGION_TO_GROUPS[usize::from(a.0)]) << 1;
        let b_group = u64::from(REGION_TO_GROUPS[usize::from(b.0)]) << 1;
        for ri in &MATCH_REGION {
            if Language(ri.lang) == lang
                && (ri.script == 0 || Script(u16::from(ri.script)) == script)
            {
                let group = 1u64 << (ri.group & !0x80);
                if 0x80 & ri.group == 0 {
                    if a_group & b_group & group != 0 {
                        // Both regions are in the group.
                        return (ri.distance, ri.distance == DEFAULT_DISTANCE);
                    }
                } else if (a_group | b_group) & group == 0 {
                    // Both regions are not in the group.
                    return (ri.distance, ri.distance == DEFAULT_DISTANCE);
                }
            }
        }
        (DEFAULT_DISTANCE, true)
    }

    // Go: language/match.go:659 equalsRest
    /// equalsRest compares everything except the language.
    fn equals_rest(a: &Tag, b: &Tag) -> bool {
        // TODO: don't include extensions in this comparison. To do this efficiently,
        // though, we should handle private tags separately.
        a.script_id == b.script_id
            && a.region_id == b.region_id
            && a.variant_or_private_use_tags() == b.variant_or_private_use_tags()
    }

    // Go: language/match.go:667 isExactEquivalent
    /// isExactEquivalent returns true if canonicalizing the language will not alter
    /// the script or region of a tag.
    fn is_exact_equivalent(l: Language) -> bool {
        for o in NOT_EQUIVALENT.iter() {
            if *o == l {
                return false;
            }
        }
        true
    }

    // Go: language/match.go:676 notEquivalent, filled by init at 678
    // PORT: Go fills it in `init`; here it is computed on first use.
    static NOT_EQUIVALENT: LazyLock<Vec<Language>> = LazyLock::new(|| {
        let mut not_equivalent = Vec::new();
        // Create a list of all languages for which canonicalization may alter the
        // script or region.
        for lm in &ALIAS_MAP {
            let tag = Tag {
                lang_id: Language(lm.from),
                ..Tag::UND
            };
            let (tag, _) = canonicalize(ALL, tag);
            if tag.script_id.0 != 0 || tag.region_id.0 != 0 {
                not_equivalent.push(Language(lm.from));
            }
        }
        not_equivalent
    });

    // Go: language/tables.go:104 paradigmLocales, updated by init at 678
    // PORT: Go rewrites the table in `init`; here the rewritten table is
    // computed on first use from the generated one.
    static PARADIGM_LOCALES: LazyLock<[[u16; 3]; 3]> = LazyLock::new(|| {
        let mut paradigm_locales = PARADIGM_LOCALES_TABLE;
        // Maximize undefined regions of paradigm locales.
        for v in &mut paradigm_locales {
            let t = Tag {
                lang_id: Language(v[0]),
                ..Tag::UND
            };
            let (max, _) = t.maximize();
            if v[1] == 0 {
                v[1] = max.region_id.0;
            }
            if v[2] == 0 {
                v[2] = max.region_id.0;
            }
        }
        paradigm_locales
    });
}

/// Go: `golang.org/x/text/internal/tag`.
mod tag {
    // Go: internal/tag/tag.go:13 Index
    // PORT: Go `Index` is a string of 4-byte entries; here it is `&[u8]`.

    // Go: internal/tag/tag.go:16 (Index).Elem
    /// Elem returns the element data at the given index.
    pub fn elem(s: &[u8], x: usize) -> &[u8] {
        &s[x * 4..x * 4 + 4]
    }

    /// Go `sort.Search`.
    pub fn search(n: usize, f: impl Fn(usize) -> bool) -> usize {
        let (mut i, mut j) = (0, n);
        while i < j {
            let h = (i + j) / 2;
            if !f(h) {
                i = h + 1;
            } else {
                j = h;
            }
        }
        i
    }

    // Go: internal/tag/tag.go:23 (Index).Index
    /// Index reports the index of the given key or -1 if it could not be found.
    /// Only the first len(key) bytes from the start of the 4-byte entries will be
    /// considered for the search and the first match in Index will be returned.
    pub fn index(s: &[u8], key: &[u8]) -> i32 {
        let n = key.len();
        // search the index of the first entry with an equal or higher value than
        // key in s.
        let index = search(s.len() / 4, |i| cmp(&s[i * 4..i * 4 + n], key) != -1);
        let i = index * 4;
        if cmp(&s[i..i + key.len()], key) != 0 {
            return -1;
        }
        index as i32
    }

    // Go: internal/tag/tag.go:40 (Index).Next
    /// Next finds the next occurrence of key after index x, which must have been
    /// obtained from a call to Index using the same key. It returns x+1 or -1.
    pub fn next(s: &[u8], key: &[u8], x: i32) -> i32 {
        let x = x + 1;
        let p = x as usize * 4;
        if p < s.len() && cmp(&s[p..p + key.len()], key) == 0 {
            return x;
        }
        -1
    }

    // Go: internal/tag/tag.go:48 cmp
    /// cmp returns an integer comparing a and b lexicographically.
    fn cmp(a: &[u8], b: &[u8]) -> i32 {
        let n = a.len().min(b.len());
        for (i, &c) in b[..n].iter().enumerate() {
            if a[i] > c {
                return 1;
            } else if a[i] < c {
                return -1;
            }
        }
        if a.len() < b.len() {
            return -1;
        } else if a.len() > b.len() {
            return 1;
        }
        0
    }

    // Go: internal/tag/tag.go:71 Compare
    /// Compare returns an integer comparing a and b lexicographically.
    pub fn compare(a: &[u8], b: &[u8]) -> i32 {
        cmp(a, b)
    }

    // Go: internal/tag/tag.go:77 FixCase
    /// FixCase reformats b to the same pattern of cases as form.
    /// If returns false if string b is malformed.
    pub fn fix_case(form: &str, b: &mut [u8]) -> bool {
        let form = form.as_bytes();
        if form.len() != b.len() {
            return false;
        }
        for i in 0..b.len() {
            let mut c = b[i];
            if form[i] <= b'Z' {
                if c >= b'a' {
                    c = c.wrapping_sub(b'z' - b'Z');
                }
                if c < b'A' || b'Z' < c {
                    return false;
                }
            } else {
                if c <= b'Z' {
                    c = c.wrapping_add(b'z' - b'Z');
                }
                if c < b'a' || b'z' < c {
                    return false;
                }
            }
            b[i] = c;
        }
        true
    }
}

/// Go: `golang.org/x/text/internal/language` (language.go, lookup.go,
/// parse.go, match.go, common.go, tags.go).
pub mod internal_language {
    use super::tables::*;
    use super::tag;

    // Go: internal/language/language.go:18
    /// maxCoreSize is the maximum size of a BCP 47 tag without variants and
    /// extensions. Equals max lang (3) + script (4) + max reg (3) + 2 dashes.
    const MAX_CORE_SIZE: usize = 12;

    // Go: internal/language/lookup.go:356
    const MAX_ALT_TAGLEN: usize = "en-US-POSIX".len();
    const MAX_LEN: usize = MAX_ALT_TAGLEN;

    // Go: internal/language/common.go:8 AliasType
    /// AliasType is the type of an alias in AliasMap.
    pub type AliasType = i8;

    pub const DEPRECATED: AliasType = 0;
    pub const MACRO: AliasType = 1;
    pub const LEGACY: AliasType = 2;

    pub const ALIAS_TYPE_UNKNOWN: AliasType = -1;

    /// Go: the errors that the parser and `Maximize` return.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub enum Error {
        /// Go `ErrSyntax`: the input is not well-formed, according to BCP 47.
        Syntax,
        /// Go `ErrDuplicateKey`: different values for same key in -u extension.
        DuplicateKey,
        /// Go `ValueError`: the input is well-formed but the respective subtag
        /// is not recognized as a valid value.
        Value(ValueError),
        /// Go `ErrMissingLikelyTagsData`.
        MissingLikelyTagsData,
        /// Go `errPrivateUse` (`SetTypeForKey` on a private use tag).
        PrivateUse,
        /// Go `errInvalidArguments` (`SetTypeForKey`).
        InvalidArguments,
    }

    impl Error {
        /// Go `err.Error()`.
        pub fn error(&self) -> String {
            match self {
                Error::Syntax => "language: tag is not well-formed".to_string(),
                Error::DuplicateKey => {
                    "language: different values for same key in -u extension".to_string()
                }
                Error::Value(e) => e.error(),
                Error::MissingLikelyTagsData => "missing likely tags data".to_string(),
                Error::PrivateUse => "cannot set a key on a private use tag".to_string(),
                Error::InvalidArguments => "invalid key or type".to_string(),
            }
        }
    }

    /// Go: a panic inside the parser. Go `Parse` recovers from it and returns
    /// `(Und, ErrSyntax)`.
    /// PORT: Rust has no recover; the sites that panic in Go return this.
    #[derive(Clone, Copy, Debug)]
    pub struct GoPanic;

    // Go: internal/language/parse.go:44 ValueError
    /// ValueError is returned by any of the parsing functions when the
    /// input is well-formed but the respective subtag is not recognized
    /// as a valid value.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub struct ValueError {
        v: [u8; 8],
    }

    // Go: internal/language/parse.go:49 NewValueError
    /// NewValueError creates a new ValueError.
    pub fn new_value_error(tag: &[u8]) -> ValueError {
        let mut e = ValueError { v: [0; 8] };
        let n = tag.len().min(8);
        e.v[..n].copy_from_slice(&tag[..n]);
        e
    }

    impl ValueError {
        // Go: internal/language/parse.go:55 (ValueError).tag
        fn tag(&self) -> &[u8] {
            let n = self.v.iter().position(|&c| c == 0).unwrap_or(8);
            &self.v[..n]
        }

        // Go: internal/language/parse.go:64 (ValueError).Error
        /// Error implements the error interface.
        pub fn error(&self) -> String {
            format!(
                "language: subtag {:?} is well-formed but unknown",
                String::from_utf8_lossy(self.tag())
            )
        }
    }

    // Go: internal/language/match.go:5 scriptRegionFlags
    const IS_LIST: u8 = 1 << 0;
    const SCRIPT_IN_FROM: u8 = 1 << 1;
    const REGION_IN_FROM: u8 = 1 << 2;

    // Go: internal/language/lookup.go:34 Language
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct Language(pub u16);

    // Go: internal/language/lookup.go:183 Region
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct Region(pub u16);

    // Go: internal/language/lookup.go:308 Script
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct Script(pub u16);

    // Go: internal/language/language.go:36 Tag
    /// Tag represents a BCP 47 language tag. It is used to specify an instance of a
    /// specific language or locale. All language tag values are guaranteed to be
    /// well-formed. The zero value of Tag is Und.
    #[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
    pub struct Tag {
        pub lang_id: Language,
        pub region_id: Region,
        pub script_id: Script,
        /// offset in str, includes preceding '-'
        pub p_variant: u8,
        /// offset of first extension, includes preceding '-'
        pub p_ext: u16,

        /// str is the string representation of the Tag. It will only be used if the
        /// tag has variants or extensions.
        pub str: String,
    }

    impl Tag {
        // Go: internal/language/tags.go:47 Und
        /// Und is the root language.
        pub const UND: Tag = Tag {
            lang_id: Language(0),
            region_id: Region(0),
            script_id: Script(0),
            p_variant: 0,
            p_ext: 0,
            str: String::new(),
        };

        // Go: internal/language/language.go:78 (Tag).equalTags
        /// equalTags compares language, script and region subtags only.
        pub fn equal_tags(&self, a: &Tag) -> bool {
            self.lang_id == a.lang_id
                && self.script_id == a.script_id
                && self.region_id == a.region_id
        }

        // Go: internal/language/language.go:92 (Tag).IsPrivateUse
        /// IsPrivateUse reports whether the Tag consists solely of an IsPrivateUse use
        /// tag.
        pub fn is_private_use(&self) -> bool {
            !self.str.is_empty() && self.p_variant == 0
        }

        // Go: internal/language/language.go:99 (*Tag).RemakeString
        /// RemakeString is used to update t.str in case lang, script or region changed.
        /// It is assumed that pExt and pVariant still point to the start of the
        /// respective parts.
        pub fn remake_string(&mut self) {
            if self.str.is_empty() {
                return;
            }
            let mut extra = self.str[usize::from(self.p_variant)..].to_string();
            if self.p_variant > 0 {
                extra = extra[1..].to_string();
            }
            if self.equal_tags(&Tag::UND) && extra.starts_with("x-") {
                self.str = extra;
                self.p_variant = 0;
                self.p_ext = 0;
                return;
            }
            let mut b = self.gen_core_bytes();
            if !extra.is_empty() {
                let diff = b.len() as i32 - i32::from(self.p_variant);
                b.push(b'-');
                b.extend_from_slice(extra.as_bytes());
                self.p_variant = (i32::from(self.p_variant) + diff) as u8;
                self.p_ext = (i32::from(self.p_ext) + diff) as u16;
            } else {
                self.p_variant = b.len() as u8;
                self.p_ext = b.len() as u16;
            }
            self.str = String::from_utf8_lossy(&b).into_owned();
        }

        // Go: internal/language/language.go:127 (*Tag).genCoreBytes
        /// genCoreBytes writes a string for the base languages, script and region tags
        /// to the given buffer and returns the number of bytes written. It will never
        /// write more than maxCoreSize bytes.
        /// PORT: returns the bytes instead of writing into a caller buffer.
        fn gen_core_bytes(&self) -> Vec<u8> {
            let mut buf = Vec::with_capacity(MAX_CORE_SIZE);
            buf.extend_from_slice(self.lang_id.string().as_bytes());
            if self.script_id.0 != 0 {
                buf.push(b'-');
                buf.extend_from_slice(self.script_id.string().as_bytes());
            }
            if self.region_id.0 != 0 {
                buf.push(b'-');
                buf.extend_from_slice(self.region_id.string().as_bytes());
            }
            buf
        }

        // Go: internal/language/language.go:142 (Tag).String
        /// String returns the canonical string representation of the language tag.
        pub fn string(&self) -> String {
            if !self.str.is_empty() {
                return self.str.clone();
            }
            if self.script_id.0 == 0 && self.region_id.0 == 0 {
                return self.lang_id.string();
            }
            String::from_utf8_lossy(&self.gen_core_bytes()).into_owned()
        }

        // Go: internal/language/language.go:186 (Tag).VariantOrPrivateUseTags
        /// VariantOrPrivateUseTags returns variants or private use tags.
        pub fn variant_or_private_use_tags(&self) -> &str {
            if self.p_ext > 0 {
                return &self.str[usize::from(self.p_variant)..usize::from(self.p_ext)];
            }
            &self.str[usize::from(self.p_variant)..]
        }

        // Go: internal/language/match.go:17 (*Tag).setUndefinedLang
        fn set_undefined_lang(&mut self, id: Language) {
            if self.lang_id.0 == 0 {
                self.lang_id = id;
            }
        }

        // Go: internal/language/match.go:23 (*Tag).setUndefinedScript
        fn set_undefined_script(&mut self, id: Script) {
            if self.script_id.0 == 0 {
                self.script_id = id;
            }
        }

        // Go: internal/language/match.go:29 (*Tag).setUndefinedRegion
        fn set_undefined_region(&mut self, id: Region) {
            if self.region_id.0 == 0 || self.region_id.contains(id) {
                self.region_id = id;
            }
        }

        // Go: internal/language/match.go:68 (Tag).Maximize
        /// Maximize returns a new tag with missing tags filled in.
        pub fn maximize(&self) -> (Tag, Option<Error>) {
            add_tags(self.clone())
        }

        // Go: internal/language/language.go:84 (Tag).Raw
        /// Raw returns the raw base language, script and region, without making an
        /// attempt to infer their values.
        pub fn raw(&self) -> (Language, Script, Region) {
            (self.lang_id, self.script_id, self.region_id)
        }

        // Go: internal/language/language.go:174 (Tag).Variants
        /// Variants returns the part of the tag holding all variants or the empty string
        /// if there are no variants defined.
        pub fn variants(&self) -> &str {
            if self.p_variant == 0 {
                return "";
            }
            &self.str[usize::from(self.p_variant)..usize::from(self.p_ext)]
        }

        // Go: internal/language/language.go:191 (Tag).HasString
        /// HasString reports whether this tag defines more than just the raw
        /// components.
        pub fn has_string(&self) -> bool {
            !self.str.is_empty()
        }

        // Go: internal/language/language.go:198 (Tag).Parent
        /// Parent returns the CLDR parent of t. In CLDR, missing fields in data for a
        /// specific language are substituted with fields from the parent language.
        /// The parent for a language may change for newer versions of CLDR.
        pub fn parent(&self) -> Tag {
            if !self.str.is_empty() {
                // Strip the variants and extensions.
                let (b, s, r) = self.raw();
                let t = Tag {
                    lang_id: b,
                    script_id: s,
                    region_id: r,
                    ..Tag::UND
                };
                if t.region_id.0 == 0 && t.script_id.0 != 0 && t.lang_id.0 != 0 {
                    let (base, _) = add_tags(Tag {
                        lang_id: t.lang_id,
                        ..Tag::UND
                    });
                    if base.script_id == t.script_id {
                        return Tag {
                            lang_id: t.lang_id,
                            ..Tag::UND
                        };
                    }
                }
                return t;
            }
            if self.lang_id.0 != 0 {
                if self.region_id.0 != 0 {
                    let mut max_script = self.script_id;
                    if max_script.0 == 0 {
                        let (max, _) = add_tags(self.clone());
                        max_script = max.script_id;
                    }

                    for p in &PARENTS {
                        if Language(p.lang) == self.lang_id && Script(p.max_script) == max_script {
                            for &r in p.from_region {
                                if Region(r) == self.region_id {
                                    return Tag {
                                        lang_id: self.lang_id,
                                        script_id: Script(p.script),
                                        region_id: Region(p.to_region),
                                        ..Tag::UND
                                    };
                                }
                            }
                        }
                    }

                    // Strip the script if it is the default one.
                    let (base, _) = add_tags(Tag {
                        lang_id: self.lang_id,
                        ..Tag::UND
                    });
                    if base.script_id != max_script {
                        return Tag {
                            lang_id: self.lang_id,
                            script_id: max_script,
                            ..Tag::UND
                        };
                    }
                    return Tag {
                        lang_id: self.lang_id,
                        ..Tag::UND
                    };
                } else if self.script_id.0 != 0 {
                    // The parent for an base-script pair with a non-default script is
                    // "und" instead of the base language.
                    let (base, _) = add_tags(Tag {
                        lang_id: self.lang_id,
                        ..Tag::UND
                    });
                    if base.script_id != self.script_id {
                        return Tag::UND;
                    }
                    return Tag {
                        lang_id: self.lang_id,
                        ..Tag::UND
                    };
                }
            }
            Tag::UND
        }

        // Go: internal/language/language.go:275 (Tag).HasVariants
        /// HasVariants reports whether t has variants.
        pub fn has_variants(&self) -> bool {
            u16::from(self.p_variant) < self.p_ext
        }

        // Go: internal/language/language.go:280 (Tag).HasExtensions
        /// HasExtensions reports whether t has extensions.
        pub fn has_extensions(&self) -> bool {
            usize::from(self.p_ext) < self.str.len()
        }

        // Go: internal/language/language.go:287 (Tag).Extension
        /// Extension returns the extension of type x for tag t. It will return
        /// false for ok if t does not have the requested extension. The returned
        /// extension will be invalid in this case.
        pub fn extension(&self, x: u8) -> Option<&str> {
            let mut i = usize::from(self.p_ext);
            while i + 1 < self.str.len() {
                let ext;
                (i, ext) = get_extension(&self.str, i);
                if ext.as_bytes()[0] == x {
                    return Some(ext);
                }
            }
            None
        }

        // Go: internal/language/language.go:299 (Tag).Extensions
        /// Extensions returns all extensions of t.
        pub fn extensions(&self) -> Vec<String> {
            let mut e = Vec::new();
            let mut i = usize::from(self.p_ext);
            while i + 1 < self.str.len() {
                let ext;
                (i, ext) = get_extension(&self.str, i);
                e.push(ext.to_string());
            }
            e
        }

        // Go: internal/language/language.go:317 (Tag).TypeForKey
        /// TypeForKey returns the type associated with the given key, where key and type
        /// are of the allowed values defined for the Unicode locale extension ('u') in
        /// https://www.unicode.org/reports/tr35/#Unicode_Language_and_Locale_Identifiers.
        /// TypeForKey will traverse the inheritance chain to get the correct value.
        ///
        /// If there are multiple types associated with a key, only the first will be
        /// returned. If there is no type associated with a key, it returns the empty
        /// string.
        pub fn type_for_key(&self, key: &str) -> String {
            let (_, start, end, _) = self.find_type_for_key(key);
            if end != start {
                let mut s = &self.str[start..end];
                if let Some(p) = s.find('-') {
                    s = &s[..p];
                }
                return s.to_string();
            }
            String::new()
        }

        // Go: internal/language/language.go:337 (Tag).SetTypeForKey
        /// SetTypeForKey returns a new Tag with the key set to type, where key and type
        /// are of the allowed values defined for the Unicode locale extension ('u') in
        /// https://www.unicode.org/reports/tr35/#Unicode_Language_and_Locale_Identifiers.
        /// An empty value removes an existing pair with the same key.
        pub fn set_type_for_key(&self, key: &str, value: &str) -> (Tag, Option<Error>) {
            let mut t = self.clone();
            if t.is_private_use() {
                return (t, Some(Error::PrivateUse));
            }
            if key.len() != 2 {
                return (t, Some(Error::InvalidArguments));
            }

            // Remove the setting if value is "".
            if value.is_empty() {
                let (mut start, sep, end, _) = t.find_type_for_key(key);
                if start != sep {
                    // Remove a possible empty extension.
                    let sb = t.str.as_bytes();
                    if sb[start - 2] != b'-' {
                        // has previous elements.
                    } else if end == sb.len() // end of string
                        || (end + 2 < sb.len() && sb[end + 2] == b'-')
                    {
                        // end of extension
                        start -= 2;
                    }
                    if start == usize::from(t.p_variant) && end == t.str.len() {
                        t.str = String::new();
                        t.p_variant = 0;
                        t.p_ext = 0;
                    } else {
                        t.str = format!("{}{}", &t.str[..start], &t.str[end..]);
                    }
                }
                return (t, None);
            }

            if value.len() < 3 || value.len() > 8 {
                return (t, Some(Error::InvalidArguments));
            }

            // Generate the tag string if needed.
            // PORT: Go builds the string in a fixed buffer; `buf` is its used
            // part.
            let mut buf: Vec<u8> = Vec::new();
            let mut u_start = 0; // start of the -u extension.
            if t.str.is_empty() {
                buf = t.gen_core_bytes();
                u_start = buf.len();
                buf.push(b'-');
                u_start += 1;
            }

            // Create new key-type pair and parse it to verify.
            let mut b = b"u-".to_vec();
            b.extend_from_slice(key.as_bytes());
            b.push(b'-');
            b.extend_from_slice(value.as_bytes());
            let mut scan = make_scanner(b);
            match parse_extensions(&mut scan) {
                Ok(_) => {}
                // PORT: Go panics here and nothing recovers it; the key and
                // value are checked above, so the parser does not panic.
                Err(GoPanic) => panic!("language: SetTypeForKey parse panicked"),
            }
            if scan.err.is_some() {
                return (t, scan.err);
            }
            // PORT: Go reads `b` again, which shares its bytes with the
            // scanner; for one key-type pair `parseExtensions` keeps them in
            // place.
            let b = scan.buf[..scan.n].to_vec();

            // Assemble the replacement string.
            if t.str.is_empty() {
                t.p_variant = (u_start - 1) as u8;
                t.p_ext = (u_start - 1) as u16;
                buf.extend_from_slice(&b);
                t.str = String::from_utf8_lossy(&buf).into_owned();
            } else {
                let s = t.str.clone();
                let (start, sep, end, has_ext) = t.find_type_for_key(key);
                if start == sep {
                    let b = if has_ext { &b[2..] } else { &b[..] };
                    t.str = format!("{}-{}{}", &s[..sep], String::from_utf8_lossy(b), &s[end..]);
                } else {
                    t.str = format!("{}-{}{}", &s[..start + 3], value, &s[end..]);
                }
            }
            (t, None)
        }

        // Go: internal/language/language.go:417 (Tag).findTypeForKey
        /// findTypeForKey returns the start and end position for the type corresponding
        /// to key or the point at which to insert the key-value pair if the type
        /// wasn't found. The hasExt return value reports whether an -u extension was present.
        /// Note: the extensions are typically very small and are likely to contain
        /// only one key-type pair.
        fn find_type_for_key(&self, key: &str) -> (usize, usize, usize, bool) {
            let mut p = usize::from(self.p_ext);
            if key.len() != 2 || p == self.str.len() || p == 0 {
                return (p, p, p, false);
            }
            let s = self.str.as_bytes();

            // Find the correct extension.
            p += 1;
            while s[p] != b'u' {
                if s[p] > b'u' {
                    p -= 1;
                    return (p, p, p, false);
                }
                p = next_extension(&self.str, p);
                if p == s.len() {
                    return (s.len(), s.len(), s.len(), false);
                }
                p += 1;
            }
            // Proceed to the hyphen following the extension name.
            p += 1;

            // curKey is the key currently being processed.
            let mut cur_key: &[u8] = b"";
            let (mut start, mut sep) = (0, 0);

            // Iterate over keys until we get the end of a section.
            loop {
                let end = p;
                p += 1;
                while p < s.len() && s[p] != b'-' {
                    p += 1;
                }
                let n = p - end - 1;
                if n <= 2 && cur_key == key.as_bytes() {
                    if sep < end {
                        sep += 1;
                    }
                    return (start, sep, end, true);
                }
                match n {
                    // invalid string, next extension
                    0 | 1 => return (end, end, end, true),
                    2 => {
                        // next key
                        cur_key = &s[end + 1..p];
                        if cur_key > key.as_bytes() {
                            return (end, end, end, true);
                        }
                        start = end;
                        sep = p;
                    }
                    _ => {}
                }
            }
        }
    }

    // Go: internal/language/language.go:513 ParseRegion
    /// ParseRegion parses a 2- or 3-letter ISO 3166-1 or a UN M.49 code.
    /// It returns a ValueError if s is a well-formed but unknown region identifier
    /// or another error if another error occurred.
    pub fn parse_region(s: &str) -> (Region, Option<Error>) {
        let n = s.len();
        if !(2..=3).contains(&n) {
            return (Region(0), Some(Error::Syntax));
        }
        let mut buf = s.as_bytes().to_vec();
        match get_region_id(&mut buf) {
            Ok(r) => r,
            // Go recovers a panic here and returns ErrSyntax.
            Err(GoPanic) => (Region(0), Some(Error::Syntax)),
        }
    }

    impl Region {
        // Go: internal/language/language.go:530 (Region).IsCountry
        /// IsCountry returns whether this region is a country or autonomous area. This
        /// includes non-standard definitions from CLDR.
        pub fn is_country(self) -> bool {
            if self.0 == 0 || self.is_group() || (self.is_private_use() && self.0 != _XK) {
                return false;
            }
            true
        }

        // Go: internal/language/language.go:539 (Region).IsGroup
        /// IsGroup returns whether this region defines a collection of regions. This
        /// includes non-standard definitions from CLDR.
        pub fn is_group(self) -> bool {
            if self.0 == 0 {
                return false;
            }
            usize::from(REGION_INCLUSION[usize::from(self.0)]) < REGION_CONTAINMENT.len()
        }

        // Go: internal/language/lookup.go:327 (Region).IsPrivateUse
        /// IsPrivateUse reports whether r has the ISO 3166 User-assigned status. This
        /// may include private-use tags that are assigned by CLDR and used in this
        /// implementation. So IsPrivateUse and IsCountry can be simultaneously true.
        pub fn is_private_use(self) -> bool {
            // Go: lookup.go:278 iso3166UserAssigned = 1 << iota
            const ISO3166_USER_ASSIGNED: u8 = 1 << 0;
            REGION_TYPES[usize::from(self.0)] & ISO3166_USER_ASSIGNED != 0
        }
    }

    // Go: internal/language/parse.go:581 getExtension
    /// getExtension returns the name, body and end position of the extension.
    pub(super) fn get_extension(s: &str, mut p: usize) -> (usize, &str) {
        let b = s.as_bytes();
        if b[p] == b'-' {
            p += 1;
        }
        if b[p] == b'x' {
            return (s.len(), &s[p..]);
        }
        let end = next_extension(s, p);
        (end, &s[p..end])
    }

    // Go: internal/language/parse.go:596 nextExtension
    /// nextExtension finds the next extension within the string, searching
    /// for the -<char>- pattern from position p.
    /// In the fast majority of cases, language tags will have at most
    /// one extension and extensions tend to be small.
    pub(super) fn next_extension(s: &str, mut p: usize) -> usize {
        let b = s.as_bytes();
        let n = b.len().saturating_sub(3);
        while p < n {
            if b[p] == b'-' {
                if b[p + 2] == b'-' {
                    return p;
                }
                p += 3;
            } else {
                p += 1;
            }
        }
        s.len()
    }

    // Go: internal/language/compose.go:13 Builder
    /// A Builder allows constructing a Tag from individual components.
    /// Its main user is Compose in the top-level language package.
    #[derive(Clone, Debug, Default)]
    pub struct Builder {
        pub tag: Tag,

        /// the x extension
        private: String,
        variants: Vec<String>,
        extensions: Vec<String>,
    }

    impl Builder {
        // Go: internal/language/compose.go:23 (*Builder).Make
        /// Make returns a new Tag from the current settings.
        pub fn make(&mut self) -> Tag {
            let mut t = self.tag.clone();

            if !self.extensions.is_empty() || !self.variants.is_empty() {
                // Go: internal/language/compose.go:27 sort.Sort(sortVariants(b.variants))
                crate::gostd::slices::sort_slice(&mut self.variants, |a, b| {
                    variant_index(a) < variant_index(b)
                });
                // Go: internal/language/compose.go:28 sort.Strings(b.extensions)
                self.extensions.sort();

                if !self.private.is_empty() {
                    self.extensions.push(self.private.clone());
                }
                let mut buf = t.gen_core_bytes();
                let p = buf.len();
                t.p_variant = p as u8;
                append_tokens(&mut buf, &self.variants);
                t.p_ext = buf.len() as u16;
                append_tokens(&mut buf, &self.extensions);
                t.str = String::from_utf8_lossy(&buf).into_owned();
                // We may not always need to remake the string, but when or when not
                // to do so is rather tricky.
                let mut scan = make_scanner(buf);
                return match parse_inner(&mut scan, "") {
                    Ok((t, _)) => t,
                    // PORT: Go panics here; `Compose` recovers it and
                    // returns `(und, ErrSyntax)`. The tokens come from a
                    // parsed tag, so the parser does not panic.
                    Err(GoPanic) => panic!("language: Builder.Make parse panicked"),
                };
            } else if !self.private.is_empty() {
                t.str = self.private.clone();
                t.remake_string();
            }
            t
        }

        // Go: internal/language/compose.go:55 (*Builder).SetTag
        /// SetTag copies all the settings from a given Tag. Any previously set values
        /// are discarded.
        pub fn set_tag(&mut self, t: &Tag) {
            self.tag.lang_id = t.lang_id;
            self.tag.region_id = t.region_id;
            self.tag.script_id = t.script_id;
            // TODO: optimize
            self.variants.clear();
            let variants = t.variants();
            if !variants.is_empty() {
                for vr in variants[1..].split('-') {
                    self.variants.push(vr.to_string());
                }
            }
            self.extensions.clear();
            self.private.clear();
            for e in t.extensions() {
                self.add_ext(&e);
            }
        }

        // Go: internal/language/compose.go:75 (*Builder).AddExt
        /// AddExt adds extension e to the tag. e must be a valid extension as returned
        /// by Tag.Extension. If the extension already exists, it will be discarded,
        /// except for a -u extension, where non-existing key-type pairs will added.
        pub fn add_ext(&mut self, e: &str) {
            let e0 = e.as_bytes()[0];
            if e0 == b'x' {
                if self.private.is_empty() {
                    self.private = e.to_string();
                }
                return;
            }
            for s in &mut self.extensions {
                if s.as_bytes()[0] == e0 {
                    if e0 == b'u' {
                        s.push_str(&e[1..]);
                    }
                    return;
                }
            }
            self.extensions.push(e.to_string());
        }

        // Go: internal/language/compose.go:115 (*Builder).AddVariant
        /// AddVariant adds any number of variants.
        pub fn add_variant(&mut self, v: &[&str]) {
            for v in v {
                if !v.is_empty() {
                    self.variants.push((*v).to_string());
                }
            }
        }

        // Go: internal/language/compose.go:125 (*Builder).ClearVariants
        /// ClearVariants removes any variants previously added, including those
        /// copied from a Tag in SetTag.
        pub fn clear_variants(&mut self) {
            self.variants.clear();
        }

        // Go: internal/language/compose.go:131 (*Builder).ClearExtensions
        /// ClearExtensions removes any extensions previously added, including those
        /// copied from a Tag in SetTag.
        pub fn clear_extensions(&mut self) {
            self.private.clear();
            self.extensions.clear();
        }
    }

    // Go: internal/language/compose.go:144 appendTokens
    // PORT: appends to `buf` instead of writing into a sized buffer.
    fn append_tokens(buf: &mut Vec<u8>, token: &[String]) {
        for t in token {
            buf.push(b'-');
            buf.extend_from_slice(t.as_bytes());
        }
    }

    /// Go `variantIndex[s]` (the zero value for a missing key).
    fn variant_index(s: &str) -> u8 {
        VARIANT_INDEX
            .binary_search_by(|(key, _)| (*key).cmp(s))
            .map_or(0, |k| VARIANT_INDEX[k].1)
    }

    // Go: internal/language/compact.go:8 CompactCoreInfo
    /// CompactCoreInfo is a compact integer with the three core tags encoded.
    pub type CompactCoreInfo = u32;

    // Go: internal/language/compact.go:12 GetCompactCore
    /// GetCompactCore generates a uint32 value that is guaranteed to be unique for
    /// different language, region, and script values.
    pub fn get_compact_core(t: &Tag) -> Option<CompactCoreInfo> {
        if t.lang_id.0 > LANG_NO_INDEX_OFFSET {
            return None;
        }
        let mut cci = u32::from(t.lang_id.0) << (8 + 12);
        cci |= u32::from(t.script_id.0) << 12;
        cci |= u32::from(t.region_id.0);
        Some(cci)
    }

    // Go: internal/language/compact.go:24 (CompactCoreInfo).Tag
    /// Tag generates a tag from c.
    pub fn compact_core_tag(c: CompactCoreInfo) -> Tag {
        Tag {
            lang_id: Language((c >> 20) as u16),
            region_id: Region((c & 0x3ff) as u16),
            script_id: Script(((c >> 12) & 0xff) as u16),
            ..Tag::UND
        }
    }

    // Go: internal/language/language.go:55 MustParse
    /// MustParse is like Parse, but panics if the given BCP 47 tag cannot be
    /// parsed.
    pub fn must_parse(s: &str) -> Tag {
        let (t, err) = parse(s);
        if let Some(err) = err {
            panic!("{}", err.error());
        }
        t
    }

    // Go: internal/language/language.go:60 Make
    /// Make is a convenience wrapper for Parse that omits the error.
    /// In case of an error, a sensible default is returned.
    pub fn make(s: &str) -> Tag {
        let (t, _) = parse(s);
        t
    }

    // Go: internal/language/match.go:53 specializeRegion
    /// specializeRegion attempts to specialize a group region.
    fn specialize_region(t: &mut Tag) -> bool {
        let i = REGION_INCLUSION[usize::from(t.region_id.0)];
        if i < N_REGION_GROUPS {
            let x = LIKELY_REGION_GROUP[usize::from(i)];
            if Language(x.lang) == t.lang_id && Script(x.script) == t.script_id {
                t.region_id = Region(x.region);
            }
            return true;
        }
        false
    }

    // Go: internal/language/match.go:72 addTags
    fn add_tags(mut t: Tag) -> (Tag, Option<Error>) {
        // We leave private use identifiers alone.
        if t.is_private_use() {
            return (t, None);
        }
        if t.script_id.0 != 0 && t.region_id.0 != 0 {
            if t.lang_id.0 != 0 {
                // already fully specified
                specialize_region(&mut t);
                return (t, None);
            }
            // Search matches for und-script-region. Note that for these cases
            // region will never be a group so there is no need to check for this.
            let r = usize::from(t.region_id.0);
            let mut list: &[LikelyLangScript] = &LIKELY_REGION[r..r + 1];
            let x = list[0];
            if x.flags & IS_LIST != 0 {
                list = &LIKELY_REGION_LIST[usize::from(x.lang)..usize::from(x.lang + x.script)];
            }
            for x in list {
                // Deviating from the spec. See match_test.go for details.
                if Script(x.script) == t.script_id {
                    t.set_undefined_lang(Language(x.lang));
                    return (t, None);
                }
            }
        }
        if t.lang_id.0 != 0 {
            // Search matches for lang-script and lang-region, where lang != und.
            if t.lang_id.0 < LANG_NO_INDEX_OFFSET {
                let x = LIKELY_LANG[usize::from(t.lang_id.0)];
                if x.flags & IS_LIST != 0 {
                    let list =
                        &LIKELY_LANG_LIST[usize::from(x.region)..usize::from(x.region + x.script)];
                    if t.script_id.0 != 0 {
                        for x in list {
                            if Script(x.script) == t.script_id && x.flags & SCRIPT_IN_FROM != 0 {
                                t.set_undefined_region(Region(x.region));
                                return (t, None);
                            }
                        }
                    } else if t.region_id.0 != 0 {
                        let mut count = 0;
                        let mut good_script = true;
                        let mut tt = t.clone();
                        for x in list {
                            // We visit all entries for which the script was not
                            // defined, including the ones where the region was not
                            // defined. This allows for proper disambiguation within
                            // regions.
                            if x.flags & SCRIPT_IN_FROM == 0
                                && t.region_id.contains(Region(x.region))
                            {
                                tt.region_id = Region(x.region);
                                tt.set_undefined_script(Script(x.script));
                                good_script = good_script && tt.script_id == Script(x.script);
                                count += 1;
                            }
                        }
                        if count == 1 {
                            return (tt, None);
                        }
                        // Even if we fail to find a unique Region, we might have
                        // an unambiguous script.
                        if good_script {
                            t.script_id = tt.script_id;
                        }
                    }
                }
            }
        } else {
            // Search matches for und-script.
            if t.script_id.0 != 0 {
                let x = LIKELY_SCRIPT[usize::from(t.script_id.0)];
                if x.region != 0 {
                    t.set_undefined_region(Region(x.region));
                    t.set_undefined_lang(Language(x.lang));
                    return (t, None);
                }
            }
            // Search matches for und-region. If und-script-region exists, it would
            // have been found earlier.
            if t.region_id.0 != 0 {
                let i = REGION_INCLUSION[usize::from(t.region_id.0)];
                if i < N_REGION_GROUPS {
                    let x = LIKELY_REGION_GROUP[usize::from(i)];
                    if x.region != 0 {
                        t.set_undefined_lang(Language(x.lang));
                        t.set_undefined_script(Script(x.script));
                        t.region_id = Region(x.region);
                    }
                } else {
                    let mut x = LIKELY_REGION[usize::from(t.region_id.0)];
                    if x.flags & IS_LIST != 0 {
                        x = LIKELY_REGION_LIST[usize::from(x.lang)];
                    }
                    if x.script != 0 && x.flags != SCRIPT_IN_FROM {
                        t.set_undefined_lang(Language(x.lang));
                        t.set_undefined_script(Script(x.script));
                        return (t, None);
                    }
                }
            }
        }

        // Search matches for lang.
        if t.lang_id.0 < LANG_NO_INDEX_OFFSET {
            let mut x = LIKELY_LANG[usize::from(t.lang_id.0)];
            if x.flags & IS_LIST != 0 {
                x = LIKELY_LANG_LIST[usize::from(x.region)];
            }
            if x.region != 0 {
                t.set_undefined_script(Script(x.script));
                t.set_undefined_region(Region(x.region));
            }
            specialize_region(&mut t);
            if t.lang_id.0 == 0 {
                t.lang_id = Language(_EN); // default language
            }
            return (t, None);
        }
        (t, Some(Error::MissingLikelyTagsData))
    }

    // Go: internal/language/lookup.go:18 findIndex
    /// findIndex tries to find the given tag in idx and returns a standardized error
    /// if it could not be found.
    fn find_index(idx: &[u8], key: &mut [u8], form: &str) -> (i32, Option<Error>) {
        if !tag::fix_case(form, key) {
            return (0, Some(Error::Syntax));
        }
        let i = tag::index(idx, key);
        if i == -1 {
            return (0, Some(Error::Value(new_value_error(key))));
        }
        (i, None)
    }

    // Go: internal/language/lookup.go:30 searchUint
    // PORT: not ported; no reachable caller.

    // Go: internal/language/lookup.go:38 getLangID
    /// getLangID returns the langID of s if s is a canonical subtag
    /// or langUnknown if s is not a canonical subtag.
    fn get_lang_id(s: &mut [u8]) -> (Language, Option<Error>) {
        if s.len() == 2 {
            return get_lang_iso2(s);
        }
        get_lang_iso3(s)
    }

    impl Language {
        // Go: internal/language/lookup.go:49 (Language).Canonicalize
        pub fn canonicalize(self) -> (Language, AliasType) {
            norm_lang(self)
        }

        // Go: internal/language/lookup.go:132 (Language).StringToBuf
        // PORT: not ported; `gen_core_bytes` uses `string`, which writes the
        // same bytes.

        // Go: internal/language/lookup.go:150 (Language).String
        /// String returns the BCP 47 representation of the langID.
        /// Use b as variable name, instead of id, to ensure the variable
        /// used is consistent with that of Base in which this type is embedded.
        pub fn string(self) -> String {
            let mut b = self.0;
            if b == 0 {
                return "und".to_string();
            } else if b >= LANG_NO_INDEX_OFFSET {
                b -= LANG_NO_INDEX_OFFSET;
                let mut buf = [0u8; 3];
                int_to_str(u32::from(b), &mut buf);
                return String::from_utf8_lossy(&buf).into_owned();
            }
            let l = tag::elem(LANG, usize::from(b));
            if l[3] == 0 {
                return String::from_utf8_lossy(&l[..3]).into_owned();
            }
            String::from_utf8_lossy(&l[..2]).into_owned()
        }

        // Go: internal/language/lookup.go:186 (Language).SuppressScript
        /// SuppressScript returns the script marked as SuppressScript in the IANA
        /// language tag repository, or 0 if there is no such script.
        pub fn suppress_script(self) -> Script {
            if self.0 < LANG_NO_INDEX_OFFSET {
                return Script(u16::from(SUPPRESS_SCRIPT[usize::from(self.0)]));
            }
            Script(0)
        }
    }

    // Go: internal/language/lookup.go:54 normLang
    /// normLang returns the mapped langID of id according to mapping m.
    fn norm_lang(id: Language) -> (Language, AliasType) {
        let k = tag::search(ALIAS_MAP.len(), |i| ALIAS_MAP[i].from >= id.0);
        if k < ALIAS_MAP.len() && ALIAS_MAP[k].from == id.0 {
            return (Language(ALIAS_MAP[k].to), ALIAS_TYPES[k]);
        }
        (id, ALIAS_TYPE_UNKNOWN)
    }

    // Go: internal/language/lookup.go:66 getLangISO2
    /// getLangISO2 returns the langID for the given 2-letter ISO language code
    /// or unknownLang if this does not exist.
    fn get_lang_iso2(s: &mut [u8]) -> (Language, Option<Error>) {
        if !tag::fix_case("zz", s) {
            return (Language(0), Some(Error::Syntax));
        }
        let i = tag::index(LANG, s);
        if i != -1 && tag::elem(LANG, i as usize)[3] != 0 {
            return (Language(i as u16), None);
        }
        (Language(0), Some(Error::Value(new_value_error(s))))
    }

    // Go: internal/language/lookup.go:76 base
    const BASE: u32 = (b'z' - b'a' + 1) as u32;

    // Go: internal/language/lookup.go:78 strToInt
    fn str_to_int(s: &[u8]) -> u32 {
        let mut v = 0u32;
        for &c in s {
            v *= BASE;
            v += u32::from(c.wrapping_sub(b'a'));
        }
        v
    }

    // Go: internal/language/lookup.go:89 intToStr
    /// converts the given integer to the original ASCII string passed to strToInt.
    /// len(s) must match the number of characters obtained.
    fn int_to_str(mut v: u32, s: &mut [u8]) {
        for i in (0..s.len()).rev() {
            s[i] = (v % BASE) as u8 + b'a';
            v /= BASE;
        }
    }

    // Go: internal/language/lookup.go:98 getLangISO3
    /// getLangISO3 returns the langID for the given 3-letter ISO language code
    /// or unknownLang if this does not exist.
    fn get_lang_iso3(s: &mut [u8]) -> (Language, Option<Error>) {
        if tag::fix_case("und", s) {
            // first try to match canonical 3-letter entries
            let mut i = tag::index(LANG, &s[..2]);
            while i != -1 {
                let e = tag::elem(LANG, i as usize);
                if e[3] == 0 && e[2] == s[2] {
                    // We treat "und" as special and always translate it to "unspecified".
                    // Note that ZZ and Zzzz are private use and are not treated as
                    // unspecified by default.
                    let id = Language(i as u16);
                    if id.0 == NON_CANONICAL_UND {
                        return (Language(0), None);
                    }
                    return (id, None);
                }
                i = tag::next(LANG, &s[..2], i);
            }
            let i = tag::index(ALT_LANG_ISO3, s);
            if i != -1 {
                return (
                    Language(ALT_LANG_INDEX[usize::from(tag::elem(ALT_LANG_ISO3, i as usize)[3])]),
                    None,
                );
            }
            let n = str_to_int(s);
            if LANG_NO_INDEX[(n / 8) as usize] & (1 << (n % 8)) != 0 {
                return (Language(n as u16 + LANG_NO_INDEX_OFFSET), None);
            }
            // Check for non-canonical uses of ISO3.
            let mut i = tag::index(LANG, &s[..1]);
            while i != -1 {
                let e = tag::elem(LANG, i as usize);
                if e[2] == s[1] && e[3] == s[2] {
                    return (Language(i as u16), None);
                }
                i = tag::next(LANG, &s[..1], i);
            }
            return (Language(0), Some(Error::Value(new_value_error(s))));
        }
        (Language(0), Some(Error::Syntax))
    }

    // Go: internal/language/lookup.go:187 getRegionID
    /// getRegionID returns the region id for s if s is a valid 2-letter region code
    /// or unknownRegion.
    fn get_region_id(s: &mut [u8]) -> Result<(Region, Option<Error>), GoPanic> {
        if s.len() == 3 {
            if is_alpha(s[0]) {
                return Ok(get_region_iso3(s));
            }
            // Go: strconv.ParseUint(string(s), 10, 10)
            if s.iter().all(u8::is_ascii_digit) {
                let i = s.iter().fold(0u32, |v, &c| v * 10 + u32::from(c - b'0'));
                return get_region_m49(i as i32);
            }
        }
        Ok(get_region_iso2(s))
    }

    // Go: internal/language/lookup.go:201 getRegionISO2
    /// getRegionISO2 returns the regionID for the given 2-letter ISO country code
    /// or unknownRegion if this does not exist.
    fn get_region_iso2(s: &mut [u8]) -> (Region, Option<Error>) {
        let (i, err) = find_index(REGION_ISO, s, "ZZ");
        if err.is_some() {
            return (Region(0), err);
        }
        (Region(i as u16 + ISO_REGION_OFFSET), None)
    }

    // Go: internal/language/lookup.go:211 getRegionISO3
    /// getRegionISO3 returns the regionID for the given 3-letter ISO country code
    /// or unknownRegion if this does not exist.
    fn get_region_iso3(s: &mut [u8]) -> (Region, Option<Error>) {
        if tag::fix_case("ZZZ", s) {
            let mut i = tag::index(REGION_ISO, &s[..1]);
            while i != -1 {
                let e = tag::elem(REGION_ISO, i as usize);
                if e[2] == s[1] && e[3] == s[2] {
                    return (Region(i as u16 + ISO_REGION_OFFSET), None);
                }
                i = tag::next(REGION_ISO, &s[..1], i);
            }
            let mut i = 0;
            while i < ALT_REGION_ISO3.len() {
                if tag::compare(&ALT_REGION_ISO3[i..i + 3], s) == 0 {
                    return (Region(ALT_REGION_IDS[i / 3]), None);
                }
                i += 3;
            }
            return (Region(0), Some(Error::Value(new_value_error(s))));
        }
        (Region(0), Some(Error::Syntax))
    }

    // Go: internal/language/lookup.go:230 getRegionM49
    /// PORT: Go indexes `fromM49` one past its end for some codes above 895
    /// (for example 999) and panics; Go `Parse` recovers and returns
    /// `(Und, ErrSyntax)`. That index returns `GoPanic` here.
    fn get_region_m49(n: i32) -> Result<(Region, Option<Error>), GoPanic> {
        if 0 < n && n <= 999 {
            const SEARCH_BITS: i32 = 7;
            const REGION_BITS: u32 = 9;
            const REGION_MASK: u16 = (1 << REGION_BITS) - 1;
            let idx = (n >> SEARCH_BITS) as usize;
            let lo = M49_INDEX[idx] as usize;
            let hi = M49_INDEX[idx + 1] as usize;
            let buf = &FROM_M49[lo..hi];
            let val = (n as u16).wrapping_shl(REGION_BITS); // we rely on bits shifting out
            let i = tag::search(buf.len(), |i| buf[i] >= val);
            let Some(&r) = FROM_M49.get(lo + i) else {
                return Err(GoPanic);
            };
            if r & !REGION_MASK == val {
                return Ok((Region(r & REGION_MASK), None));
            }
        }
        // PORT: Go prints n into a copy of the error buffer, so the error
        // holds no subtag.
        Ok((Region(0), Some(Error::Value(ValueError { v: [0; 8] }))))
    }

    // Go: internal/language/lookup.go:251 normRegion
    /// normRegion returns a region if r is deprecated or 0 otherwise.
    /// TODO: consider supporting BYS (-> BLR), CSK (-> 200 or CZ), PHI (-> PHL) and AFI (-> DJ).
    /// TODO: consider mapping split up regions to new most populous one (like CLDR).
    fn norm_region(r: Region) -> Region {
        let m = &REGION_OLD_MAP;
        let k = tag::search(m.len(), |i| m[i].from >= r.0);
        if k < m.len() && m[k].from == r.0 {
            return Region(m[k].to);
        }
        Region(0)
    }

    impl Region {
        // Go: internal/language/lookup.go:275 (Region).String
        /// String returns the BCP 47 representation for the region.
        /// It returns "ZZ" for an unspecified region.
        pub fn string(self) -> String {
            let mut r = self.0;
            if r < ISO_REGION_OFFSET {
                if r == 0 {
                    return "ZZ".to_string();
                }
                return format!("{:03}", self.m49());
            }
            r -= ISO_REGION_OFFSET;
            String::from_utf8_lossy(&tag::elem(REGION_ISO, usize::from(r))[..2]).into_owned()
        }

        // Go: internal/language/lookup.go:305 (Region).M49
        /// M49 returns the UN M.49 encoding of r, or 0 if this encoding
        /// is not defined for r.
        pub fn m49(self) -> i32 {
            i32::from(M49[usize::from(self.0)])
        }

        // Go: internal/language/language.go:548 (Region).Contains
        /// Contains returns whether Region c is contained by Region r. It returns true
        /// if c == r.
        pub fn contains(self, c: Region) -> bool {
            if self == c {
                return true;
            }
            let g = REGION_INCLUSION[usize::from(self.0)];
            if g >= N_REGION_GROUPS {
                return false;
            }
            let m = REGION_CONTAINMENT[usize::from(g)];

            let d = REGION_INCLUSION[usize::from(c.0)];
            let b = REGION_INCLUSION_BITS[usize::from(d)];

            // A contained country may belong to multiple disjoint groups. Matching any
            // of these indicates containment. If the contained region is a group, it
            // must strictly be a subset.
            if d >= N_REGION_GROUPS {
                return b & m != 0;
            }
            b & !m == 0
        }

        // Go: internal/language/language.go:589 (Region).Canonicalize
        /// Canonicalize returns the region or a possible replacement if the region is
        /// deprecated. It will not return a replacement for deprecated regions that
        /// are split into multiple regions.
        pub fn canonicalize(self) -> Region {
            let cr = norm_region(self);
            if cr.0 != 0 {
                return cr;
            }
            self
        }
    }

    // Go: internal/language/lookup.go:312 getScriptID
    /// getScriptID returns the script id for string s. It assumes that s
    /// is of the format [A-Z][a-z]{3}.
    fn get_script_id(idx: &[u8], s: &mut [u8]) -> (Script, Option<Error>) {
        let (i, err) = find_index(idx, s, "Zzzz");
        (Script(i as u16), err)
    }

    impl Script {
        // Go: internal/language/lookup.go:319 (Script).String
        /// String returns the script code in title case.
        /// It returns "Zzzz" for an unspecified script.
        pub fn string(self) -> String {
            if self.0 == 0 {
                return "Zzzz".to_string();
            }
            String::from_utf8_lossy(tag::elem(SCRIPT, usize::from(self.0))).into_owned()
        }
    }

    // Go: internal/language/lookup.go:338 grandfatheredMap
    /// grandfatheredMap holds a mapping from legacy and grandfathered tags to
    /// their base language or index to more elaborate tag.
    /// PORT: Go keys are `[maxLen]byte` arrays padded with zero bytes.
    static GRANDFATHERED_MAP: [(&str, i16); 28] = [
        ("art-lojban", _JBO as i16), // art-lojban
        ("i-ami", _AMI as i16),      // i-ami
        ("i-bnn", _BNN as i16),      // i-bnn
        ("i-hak", _HAK as i16),      // i-hak
        ("i-klingon", _TLH as i16),  // i-klingon
        ("i-lux", _LB as i16),       // i-lux
        ("i-navajo", _NV as i16),    // i-navajo
        ("i-pwn", _PWN as i16),      // i-pwn
        ("i-tao", _TAO as i16),      // i-tao
        ("i-tay", _TAY as i16),      // i-tay
        ("i-tsu", _TSU as i16),      // i-tsu
        ("no-bok", _NB as i16),      // no-bok
        ("no-nyn", _NN as i16),      // no-nyn
        ("sgn-be-fr", _SFB as i16),  // sgn-BE-FR
        ("sgn-be-nl", _VGT as i16),  // sgn-BE-NL
        ("sgn-ch-de", _SGG as i16),  // sgn-CH-DE
        ("zh-guoyu", _CMN as i16),   // zh-guoyu
        ("zh-hakka", _HAK as i16),   // zh-hakka
        ("zh-min-nan", _NAN as i16), // zh-min-nan
        ("zh-xiang", _HSN as i16),   // zh-xiang
        // Grandfathered tags with no modern replacement will be converted as
        // follows:
        ("cel-gaulish", -1), // cel-gaulish
        ("en-gb-oed", -2),   // en-GB-oed
        ("i-default", -3),   // i-default
        ("i-enochian", -4),  // i-enochian
        ("i-mingo", -5),     // i-mingo
        ("zh-min", -6),      // zh-min
        // CLDR-specific tag.
        ("root", 0),         // root
        ("en-us-posix", -7), // en_US_POSIX"
    ];

    // Go: internal/language/lookup.go:398 grandfathered
    fn grandfathered(s: [u8; MAX_ALT_TAGLEN]) -> (Tag, bool) {
        for (key, v) in &GRANDFATHERED_MAP {
            let key = key.as_bytes();
            if s[..key.len()] == *key && s[key.len()..].iter().all(|&c| c == 0) {
                let v = *v;
                if v < 0 {
                    let lo = usize::from(ALT_TAG_INDEX[(-v - 1) as usize]);
                    let hi = usize::from(ALT_TAG_INDEX[(-v) as usize]);
                    return (make(&ALT_TAGS[lo..hi]), true);
                }
                let t = Tag {
                    lang_id: Language(v as u16),
                    ..Tag::UND
                };
                return (t, true);
            }
        }
        (Tag::UND, false)
    }

    // Go: internal/language/parse.go:17 isAlpha
    /// isAlpha returns true if the byte is not a digit.
    /// b must be an ASCII letter or digit.
    fn is_alpha(b: u8) -> bool {
        b > b'9'
    }

    // Go: internal/language/parse.go:22 isAlphaNum
    /// isAlphaNum returns true if the string contains only ASCII letters or digits.
    fn is_alpha_num(s: &[u8]) -> bool {
        s.iter().all(u8::is_ascii_alphanumeric)
    }

    // Go: internal/language/parse.go:71 scanner
    /// scanner is used to scan BCP 47 tokens, which are separated by _ or -.
    /// PORT: Go `b` is a slice over a backing array that the scanner shrinks
    /// and regrows in place, and `token` is a slice of the same array that is
    /// not updated when bytes move. `buf` is the backing array (it only
    /// grows), `n` is `len(b)` and `token` is `(position, length)` in `buf`,
    /// so a token read after the bytes moved sees the same bytes as in Go.
    /// Go only moves `b` to a new array in `replace` (always followed by
    /// `scan`, which resets the token) and at the end of `parseExtensions`.
    struct Scanner {
        buf: Vec<u8>,
        n: usize,
        tok_start: usize,
        /// Go `len(token)`; 0 is a nil token.
        tok_len: usize,
        /// start position of the current token
        start: usize,
        /// end position of the current token
        end: usize,
        /// next point for scan
        next: usize,
        err: Option<Error>,
        done: bool,
    }

    // Go: internal/language/parse.go:98 makeScanner
    /// makeScanner returns a scanner using b as the input buffer.
    /// b is not copied and may be modified by the scanner routines.
    fn make_scanner(b: Vec<u8>) -> Scanner {
        let n = b.len();
        let mut scan = Scanner {
            buf: b,
            n,
            tok_start: 0,
            tok_len: 0,
            start: 0,
            end: 0,
            next: 0,
            err: None,
            done: false,
        };
        scan.init();
        scan
    }

    // Go: internal/language/parse.go:83 makeScannerString
    fn make_scanner_string(s: &str) -> Scanner {
        let mut scan = Scanner {
            buf: s.as_bytes().to_vec(),
            n: s.len(),
            tok_start: 0,
            tok_len: 0,
            start: 0,
            end: 0,
            next: 0,
            err: None,
            done: false,
        };
        scan.init();
        scan
    }

    impl Scanner {
        fn token(&self) -> &[u8] {
            &self.buf[self.tok_start..self.tok_start + self.tok_len]
        }

        fn token_mut(&mut self) -> &mut [u8] {
            &mut self.buf[self.tok_start..self.tok_start + self.tok_len]
        }

        /// Go `scan.b = scan.b[:n]` (and appends): sets `len(b)`.
        fn set_len(&mut self, n: usize) {
            if n > self.buf.len() {
                self.buf.resize(n, 0);
            }
            self.n = n;
        }

        // Go: internal/language/parse.go:102 (*scanner).init
        fn init(&mut self) {
            for i in 0..self.n {
                if self.buf[i] == b'_' {
                    self.buf[i] = b'-';
                }
            }
            self.scan();
        }

        // Go: internal/language/parse.go:112 (*scanner).toLower
        /// restToLower converts the string between start and end to lower case.
        fn to_lower(&mut self, start: usize, end: usize) -> Result<(), GoPanic> {
            for i in start..end {
                if i >= self.n {
                    return Err(GoPanic);
                }
                let c = self.buf[i];
                if c.is_ascii_uppercase() {
                    self.buf[i] += b'a' - b'A';
                }
            }
            Ok(())
        }

        // Go: internal/language/parse.go:121 (*scanner).setError
        fn set_error(&mut self, e: Option<Error>) {
            if self.err.is_none() || (e == Some(Error::Syntax) && self.err != Some(Error::Syntax)) {
                self.err = e;
            }
        }

        // Go: internal/language/parse.go:130 (*scanner).resizeRange
        /// resizeRange shrinks or grows the array at position oldStart such that
        /// a new string of size newSize can fit between oldStart and oldEnd.
        /// Sets the scan point to after the resized range.
        /// PORT: Go moves to a new array when the result does not fit the
        /// capacity; `buf` grows in place (see `Scanner`).
        fn resize_range(&mut self, old_start: usize, old_end: usize, new_size: usize) {
            self.start = old_start;
            let end = old_start + new_size;
            if end != old_end {
                let diff = end as isize - old_end as isize;
                let old_n = self.n;
                let n = (old_n as isize + diff) as usize;
                if n > self.buf.len() {
                    self.buf.resize(n, 0);
                }
                self.buf.copy_within(old_end..old_n, end);
                self.n = n;
                self.next = (end as isize + (self.next as isize - self.end as isize)) as usize;
                self.end = end;
            }
        }

        // Go: internal/language/parse.go:150 (*scanner).replace
        /// replace replaces the current token with repl.
        fn replace(&mut self, repl: &[u8]) {
            self.resize_range(self.start, self.end, repl.len());
            self.buf[self.start..self.start + repl.len()].copy_from_slice(repl);
        }

        // Go: internal/language/parse.go:157 (*scanner).gobble
        /// gobble removes the current token from the input.
        /// Caller must call scan after calling gobble.
        fn gobble(&mut self, e: Option<Error>) {
            self.set_error(e);
            if self.start == 0 {
                let count = self.n - self.next;
                self.buf.copy_within(self.next..self.n, 0);
                self.n = count;
                self.end = 0;
            } else {
                let count = (self.n - (self.start - 1)).min(self.n - self.end);
                self.buf
                    .copy_within(self.end..self.end + count, self.start - 1);
                self.n = self.start - 1 + count;
                self.end = self.start - 1;
            }
            self.next = self.start;
        }

        // Go: internal/language/parse.go:170 (*scanner).deleteRange
        /// deleteRange removes the given range from s.b before the current token.
        fn delete_range(&mut self, start: usize, end: usize) {
            let count = self.n - end;
            self.buf.copy_within(end..self.n, start);
            self.n = start + count;
            let diff = end - start;
            self.next -= diff;
            self.start -= diff;
            self.end -= diff;
        }

        // Go: internal/language/parse.go:182 (*scanner).scan
        /// scan parses the next token of a BCP 47 string.  Tokens that are larger
        /// than 8 characters or include non-alphanumeric characters result in an error
        /// and are gobbled and removed from the output.
        /// It returns the end position of the last token consumed.
        fn scan(&mut self) -> usize {
            let end = self.end;
            self.tok_len = 0;
            self.start = self.next;
            while self.next < self.n {
                let i = match self.buf[self.next..self.n].iter().position(|&c| c == b'-') {
                    None => {
                        self.end = self.n;
                        self.next = self.n;
                        self.end - self.start
                    }
                    Some(i) => {
                        self.end = self.next + i;
                        self.next = self.end + 1;
                        i
                    }
                };
                if i < 1 || i > 8 || !is_alpha_num(&self.buf[self.start..self.end]) {
                    self.gobble(Some(Error::Syntax));
                    continue;
                }
                self.tok_start = self.start;
                self.tok_len = self.end - self.start;
                return end;
            }
            if self.n > 0 && self.buf[self.n - 1] == b'-' {
                self.set_error(Some(Error::Syntax));
                self.n -= 1;
            }
            self.done = true;
            end
        }

        // Go: internal/language/parse.go:212 (*scanner).acceptMinSize
        /// acceptMinSize parses multiple tokens of the given size or greater.
        /// It returns the end position of the last token consumed.
        fn accept_min_size(&mut self, min: usize) -> usize {
            let mut end = self.end;
            self.scan();
            while self.tok_len >= min {
                end = self.end;
                self.scan();
            }
            end
        }
    }

    // Go: internal/language/parse.go:229 Parse
    /// Parse parses the given BCP 47 string and returns a valid Tag. If parsing
    /// failed it returns an error and any part of the tag that could be parsed.
    /// If parsing succeeded but an unknown value was found, it returns
    /// ValueError. The Tag returned in this case is just stripped of the unknown
    /// value. All other values are preserved. It accepts tags in the BCP 47 format
    /// and extensions to this standard defined in
    /// https://www.unicode.org/reports/tr35/#Unicode_Language_and_Locale_Identifiers.
    /// PORT: Go recovers from a panic and returns `(Und, ErrSyntax)`. The
    /// parser returns `GoPanic` from the sites that panic in Go.
    pub fn parse(s: &str) -> (Tag, Option<Error>) {
        // TODO: consider supporting old-style locale key-value pairs.
        if s.is_empty() {
            return (Tag::UND, Some(Error::Syntax));
        }
        if s.len() <= MAX_ALT_TAGLEN {
            let mut b = [0u8; MAX_ALT_TAGLEN];
            for (i, c) in s.char_indices() {
                // Generating invalid UTF-8 is okay as it won't match.
                let mut c = c as u32;
                if u32::from(b'A') <= c && c <= u32::from(b'Z') {
                    c += u32::from(b'a' - b'A');
                } else if c == u32::from(b'_') {
                    c = u32::from(b'-');
                }
                b[i] = c as u8;
            }
            let (t, ok) = grandfathered(b);
            if ok {
                return (t, None);
            }
        }
        let mut scan = make_scanner_string(s);
        match parse_inner(&mut scan, s) {
            Ok(r) => r,
            Err(GoPanic) => (Tag::UND, Some(Error::Syntax)),
        }
    }

    // Go: internal/language/parse.go:257 parse
    // PORT: named `parse_inner` because `Parse` takes the snake name.
    // PORT: Go stores `s[:end]` when it equals `scan.b`, else `scan.b`; both
    // are the same bytes, so `scan.b` is stored.
    fn parse_inner(scan: &mut Scanner, _s: &str) -> Result<(Tag, Option<Error>), GoPanic> {
        let mut t = Tag::UND;
        let mut end;
        let n = scan.tok_len;
        if n <= 1 {
            scan.to_lower(0, scan.n)?;
            if n == 0 || scan.token()[0] != b'x' {
                return Ok((t, Some(Error::Syntax)));
            }
            end = parse_extensions(scan)?;
        } else if n >= 4 {
            return Ok((Tag::UND, Some(Error::Syntax)));
        } else {
            // the usual case
            (t, end) = parse_tag(scan, true)?;
            let n = scan.tok_len;
            if n == 1 {
                t.p_ext = end as u16;
                end = parse_extensions(scan)?;
            } else if end < scan.n {
                scan.set_error(Some(Error::Syntax));
                scan.set_len(end);
            }
        }
        if usize::from(t.p_variant) < scan.n {
            let _ = end;
            t.str = String::from_utf8_lossy(&scan.buf[..scan.n]).into_owned();
        } else {
            t.p_variant = 0;
            t.p_ext = 0;
        }
        Ok((t, scan.err))
    }

    // Go: internal/language/parse.go:293 parseTag
    /// parseTag parses language, script, region and variants.
    /// It returns a Tag and the end position in the input that was parsed.
    /// If doNorm is true, then <lang>-<extlang> will be normalized to <extlang>.
    fn parse_tag(scan: &mut Scanner, do_norm: bool) -> Result<(Tag, usize), GoPanic> {
        let mut t = Tag::UND;
        let mut e;
        // TODO: set an error if an unknown lang, script or region is encountered.
        (t.lang_id, e) = get_lang_id(scan.token_mut());
        scan.set_error(e);
        scan.replace(t.lang_id.string().as_bytes());
        let lang_start = scan.start;
        let mut end = scan.scan();
        while scan.tok_len == 3 && is_alpha(scan.token()[0]) {
            // From http://tools.ietf.org/html/bcp47, <lang>-<extlang> tags are equivalent
            // to a tag of the form <extlang>.
            if do_norm {
                let (lang, e) = get_lang_id(scan.token_mut());
                if lang.0 != 0 {
                    t.lang_id = lang;
                    let lang_str = lang.string();
                    let lang_str = lang_str.as_bytes();
                    let m = lang_str.len().min(scan.n.saturating_sub(lang_start));
                    scan.buf[lang_start..lang_start + m].copy_from_slice(&lang_str[..m]);
                    if lang_start + lang_str.len() >= scan.n {
                        return Err(GoPanic);
                    }
                    scan.buf[lang_start + lang_str.len()] = b'-';
                    scan.start = lang_start + lang_str.len() + 1;
                }
                scan.gobble(e);
            }
            end = scan.scan();
        }
        if scan.tok_len == 4 && is_alpha(scan.token()[0]) {
            (t.script_id, e) = get_script_id(SCRIPT, scan.token_mut());
            if t.script_id.0 == 0 {
                scan.gobble(e);
            }
            end = scan.scan();
        }
        let n = scan.tok_len;
        if (2..=3).contains(&n) {
            (t.region_id, e) = get_region_id(scan.token_mut())?;
            if t.region_id.0 == 0 {
                scan.gobble(e);
            } else {
                scan.replace(t.region_id.string().as_bytes());
            }
            end = scan.scan();
        }
        scan.to_lower(scan.start, scan.n)?;
        t.p_variant = end as u8;
        end = parse_variants(scan, end, &t);
        t.p_ext = end as u16;
        Ok((t, end))
    }

    // Go: internal/language/parse.go:348 parseVariants
    /// parseVariants scans tokens as long as each token is a valid variant string.
    /// Duplicate variants are removed.
    /// PORT: Go keeps the variants as slices of the scan buffer; they are
    /// copied here. Nothing writes to those bytes before Go joins them.
    fn parse_variants(scan: &mut Scanner, mut end: usize, _t: &Tag) -> usize {
        let start = scan.start;
        let mut var_id: Vec<u8> = Vec::with_capacity(4);
        let mut variant: Vec<Vec<u8>> = Vec::with_capacity(4);
        let mut last: i32 = -1;
        let mut need_sort = false;
        while scan.tok_len >= 4 {
            // TODO: measure the impact of needing this conversion and redesign
            // the data structure if there is an issue.
            let token = scan.token().to_vec();
            let Ok(k) = VARIANT_INDEX.binary_search_by(|(key, _)| key.as_bytes().cmp(&token))
            else {
                // unknown variant
                // TODO: allow user-defined variants?
                scan.gobble(Some(Error::Value(new_value_error(&token))));
                scan.scan();
                continue;
            };
            let v = VARIANT_INDEX[k].1;
            var_id.push(v);
            variant.push(token);
            if !need_sort {
                if last < i32::from(v) {
                    last = i32::from(v);
                } else {
                    need_sort = true;
                    // There is no legal combinations of more than 7 variants
                    // (and this is by no means a useful sequence).
                    const MAX_VARIANTS: usize = 8;
                    if var_id.len() > MAX_VARIANTS {
                        break;
                    }
                }
            }
            end = scan.end;
            scan.scan();
        }
        if need_sort {
            // Go: internal/language/parse.go:387 sort.Sort(variantsSort{varID, variant})
            // PORT: the two Go slices are one slice of pairs here.
            let mut pairs: Vec<(u8, Vec<u8>)> = var_id.into_iter().zip(variant).collect();
            crate::gostd::slices::sort_slice(&mut pairs, |a, b| a.0 < b.0);
            let mut k = 0;
            let mut l: i32 = -1;
            for i in 0..pairs.len() {
                let w = i32::from(pairs[i].0);
                if l == w {
                    // Remove duplicates.
                    continue;
                }
                pairs.swap(k, i);
                k += 1;
                l = w;
            }
            let str = pairs[..k]
                .iter()
                .map(|p| p.1.as_slice())
                .collect::<Vec<_>>()
                .join(&b'-');
            if str.is_empty() {
                end = start - 1;
            } else {
                scan.resize_range(start, end, str.len());
                scan.buf[scan.start..scan.start + str.len()].copy_from_slice(&str);
                end = scan.end;
            }
        }
        end
    }

    // Go: internal/language/parse.go:452 parseExtensions
    /// parseExtensions parses and normalizes the extensions in the buffer.
    /// It returns the last position of scan.b that is part of any extension.
    /// It also trims scan.b to remove excess parts accordingly.
    /// PORT: Go keeps the extensions as slices of the scan buffer; they are
    /// copied here. Go `sort.Sort` compares only the first byte and is not
    /// stable for more than 12 extensions. `sort_slice` is Go's pdqsort, so
    /// equal extensions keep Go's order. The order only changes the tag text,
    /// never its language, script, region or variants.
    fn parse_extensions(scan: &mut Scanner) -> Result<usize, GoPanic> {
        let start = scan.start;
        let mut exts: Vec<Vec<u8>> = Vec::new();
        let mut private: Vec<u8> = Vec::new();
        let mut end = scan.end;
        while scan.tok_len == 1 {
            let ext_start = scan.start;
            let ext = scan.token()[0];
            end = parse_extension(scan)?;
            if ext_start > end || end > scan.buf.len() {
                return Err(GoPanic);
            }
            let extension = scan.buf[ext_start..end].to_vec();
            if extension.len() < 3 || (ext != b'x' && extension.len() < 4) {
                scan.set_error(Some(Error::Syntax));
                end = ext_start;
                continue;
            } else if start == ext_start && (ext == b'x' || scan.start == scan.n) {
                scan.set_len(end);
                return Ok(end);
            } else if ext == b'x' {
                private = extension;
                break;
            }
            exts.push(extension);
        }
        // Go: internal/language/parse.go:478 sort.Sort(bytesSort{exts, 1})
        crate::gostd::slices::sort_slice(&mut exts, |a, b| a[..1] < b[..1]);
        if !private.is_empty() {
            exts.push(private);
        }
        scan.set_len(start);
        if !exts.is_empty() {
            let joined = exts.join(&b'-');
            let n = start + joined.len();
            scan.set_len(n);
            scan.buf[start..n].copy_from_slice(&joined);
        } else if start > 0 {
            // Strip trailing '-'.
            scan.set_len(start - 1);
        }
        Ok(end)
    }

    // Go: internal/language/parse.go:494 parseExtension
    /// parseExtension parses a single extension and returns the position of
    /// the extension end.
    /// PORT: Go keeps attributes, keys and the previous key as slices of the
    /// scan buffer; they are copied here. Nothing writes to those bytes
    /// before Go reads them. Go `sort.Sort` of the attributes compares only
    /// the first 3 bytes and is not stable for more than 12 attributes.
    /// `sort_slice` is Go's pdqsort, so equal attributes keep Go's order.
    fn parse_extension(scan: &mut Scanner) -> Result<usize, GoPanic> {
        let (start, mut end) = (scan.start, scan.end);
        match scan.token()[0] {
            b'u' => {
                // https://www.ietf.org/rfc/rfc6067.txt
                let attr_start = end;
                scan.scan();
                let mut last: Vec<u8> = Vec::new();
                while scan.tok_len > 2 {
                    if scan.token().cmp(last.as_slice()) != std::cmp::Ordering::Less {
                        // Attributes are unsorted. Start over from scratch.
                        let p = attr_start + 1;
                        scan.next = p;
                        let mut attrs: Vec<Vec<u8>> = Vec::new();
                        scan.scan();
                        while scan.tok_len > 2 {
                            attrs.push(scan.token().to_vec());
                            end = scan.end;
                            scan.scan();
                        }
                        // Go: internal/language/parse.go:510 sort.Sort(bytesSort{attrs, 3})
                        crate::gostd::slices::sort_slice(&mut attrs, |a, b| a[..3] < b[..3]);
                        let joined = attrs.join(&b'-');
                        copy_into(scan, p, &joined)?;
                        break;
                    }
                    last = scan.token().to_vec();
                    end = scan.end;
                    scan.scan();
                }
                // Scan key-type sequences. A key is of length 2 and may be followed
                // by 0 or more "type" subtags from 3 to the maximum of 8 letters.
                let mut last: Vec<u8> = Vec::new();
                let attr_end = end;
                while scan.tok_len == 2 {
                    let key = scan.token().to_vec();
                    end = scan.end;
                    scan.scan();
                    while end < scan.end && scan.tok_len > 2 {
                        end = scan.end;
                        scan.scan();
                    }
                    // TODO: check key value validity
                    if key.as_slice().cmp(last.as_slice()) != std::cmp::Ordering::Greater
                        || scan.err.is_some()
                    {
                        // We have an invalid key or the keys are not sorted.
                        // Start scanning keys from scratch and reorder.
                        let p = attr_end + 1;
                        scan.next = p;
                        let mut keys: Vec<Vec<u8>> = Vec::new();
                        scan.scan();
                        while scan.tok_len == 2 {
                            let key_start = scan.start;
                            end = scan.end;
                            scan.scan();
                            while end < scan.end && scan.tok_len > 2 {
                                end = scan.end;
                                scan.scan();
                            }
                            if key_start > end || end > scan.buf.len() {
                                return Err(GoPanic);
                            }
                            keys.push(scan.buf[key_start..end].to_vec());
                        }
                        // Go: internal/language/parse.go:541 sort.Stable(bytesSort{keys, 2})
                        crate::gostd::slices::sort_stable_func(&mut keys, |a, b| {
                            a[..2].cmp(&b[..2]) as i32
                        });
                        let n = keys.len();
                        if n > 0 {
                            let mut k = 0;
                            for i in 1..n {
                                if keys[k][..2] != keys[i][..2] {
                                    k += 1;
                                    keys[k] = keys[i].clone();
                                } else if keys[k] != keys[i] {
                                    scan.set_error(Some(Error::DuplicateKey));
                                }
                            }
                            keys.truncate(k + 1);
                        }
                        let reordered = keys.join(&b'-');
                        let e = p + reordered.len();
                        if e < end {
                            scan.delete_range(e, end);
                            end = e;
                        }
                        copy_into(scan, p, &reordered)?;
                        break;
                    }
                    last = key;
                }
            }
            b't' => {
                // https://www.ietf.org/rfc/rfc6497.txt
                scan.scan();
                let n = scan.tok_len;
                if (2..=3).contains(&n) && is_alpha(scan.token()[1]) {
                    (_, end) = parse_tag(scan, false)?;
                    scan.to_lower(start, end)?;
                }
                while scan.tok_len == 2 && !is_alpha(scan.token()[1]) {
                    end = scan.accept_min_size(3);
                }
            }
            b'x' => {
                end = scan.accept_min_size(1);
            }
            _ => {
                end = scan.accept_min_size(2);
            }
        }
        Ok(end)
    }

    /// Go `copy(scan.b[p:], src)`: copies up to `len(b)-p` bytes.
    fn copy_into(scan: &mut Scanner, p: usize, src: &[u8]) -> Result<(), GoPanic> {
        if p > scan.n {
            return Err(GoPanic);
        }
        let m = src.len().min(scan.n - p);
        scan.buf[p..p + m].copy_from_slice(&src[..m]);
        Ok(())
    }
}

/// Go: `golang.org/x/text/internal/language/compact` (compact.go,
/// language.go): the compact form of a `language.Tag`.
/// PORT: the rest of the port keeps a public `language.Tag` as its full
/// `internal/language.Tag` (see the module note). `gostd::collate` needs the
/// compact form where Go's result depends on it: tag equality (`==` on
/// compact tags) and `Tag.Parent`, which maps a tag with a compact index to
/// the nearest compact tag.
pub mod compact {
    use super::internal_language::{self as il, Tag};
    use super::tables::{CORE_TAGS, SPECIAL_TAGS_STR};
    use std::sync::LazyLock;

    // Go: internal/language/compact/compact.go:23 ID
    /// ID is an integer identifying a single tag.
    pub type Id = u16;

    // Go: internal/language/compact/language.go:21 Tag
    /// Tag represents a BCP 47 language tag. It is used to specify an instance of a
    /// specific language or locale. All language tag values are guaranteed to be
    /// well-formed.
    /// PORT: Go `full fullTag` (an interface that always holds a
    /// `language.Tag`) is `Option<Tag>`. Go compares the interface by the
    /// dynamic value, as `PartialEq` does here.
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct CompactTag {
        language: Id,
        locale: Id,
        full: Option<Tag>,
    }

    // Go: internal/language/compact/language.go:29 _und
    const UND: Id = 0;

    // Go: internal/language/compact/compact.go:53 specialTags
    static SPECIAL_TAGS: LazyLock<Vec<Tag>> =
        LazyLock::new(|| SPECIAL_TAGS_STR.split(' ').map(il::must_parse).collect());

    // Go: internal/language/compact/compact.go:26 getCoreIndex
    fn get_core_index(t: &Tag) -> Option<Id> {
        let cci = il::get_compact_core(t)?;
        let i = CORE_TAGS.partition_point(|&c| c < cci);
        if i == CORE_TAGS.len() || CORE_TAGS[i] != cci {
            return None;
        }
        Some(i as Id)
    }

    // Go: internal/language/compact/compact.go:46 (ID).Tag
    /// Tag converts id to an internal language Tag.
    fn id_tag(id: Id) -> Tag {
        let id = usize::from(id);
        if id >= CORE_TAGS.len() {
            return SPECIAL_TAGS[id - CORE_TAGS.len()].clone();
        }
        il::compact_core_tag(CORE_TAGS[id])
    }

    // Go: internal/language/compact/language.go:37 Make
    /// Make a compact Tag from a fully specified internal language Tag.
    pub fn make(t: &Tag) -> CompactTag {
        let region = t.type_for_key("rg");
        if region.len() == 6 && &region[2..] == "zzzz" {
            let (r, err) = il::parse_region(&region[..2]);
            if err.is_none() {
                let t_full = t.clone();
                let (mut t, _) = t.set_type_for_key("rg", "");
                // TODO: should we not consider "va" for the language tag?
                let (language, exact1) = from_tag(&t);
                t.region_id = r;
                let (locale, exact2) = from_tag(&t);
                return CompactTag {
                    language,
                    locale,
                    full: (!exact1 || !exact2).then_some(t_full),
                };
            }
        }
        let (lang, ok) = from_tag(t);
        CompactTag {
            language: lang,
            locale: lang,
            full: (!ok).then(|| t.clone()),
        }
    }

    impl CompactTag {
        // Go: internal/language/compact/language.go:63 (Tag).Tag
        /// Tag returns an internal language Tag version of this tag.
        pub fn tag(&self) -> Tag {
            if let Some(full) = &self.full {
                return full.clone();
            }
            let mut tag = id_tag(self.language);
            if self.language != self.locale {
                let loc = id_tag(self.locale);
                (tag, _) =
                    tag.set_type_for_key("rg", &(loc.region_id.string().to_lowercase() + "zzzz"));
            }
            tag
        }

        // Go: internal/language/compact/language.go:105 (Tag).Parent
        /// Parent returns the CLDR parent of t. In CLDR, missing fields in data for a
        /// specific language are substituted with fields from the parent language.
        /// The parent for a language may change for newer versions of CLDR.
        pub fn parent(&self) -> CompactTag {
            if let Some(full) = &self.full {
                return make(&full.parent());
            }
            if self.language != self.locale {
                // Simulate stripping -u-rg-xxxxxx
                return CompactTag {
                    language: self.language,
                    locale: self.language,
                    full: None,
                };
            }
            // TODO: use parent lookup table once cycle from internal package is
            // removed. Probably by internalizing the table and declaring this fast
            // enough.
            // lang := compactID(internal.Parent(uint16(t.language)))
            let (lang, _) = from_tag(&id_tag(self.language).parent());
            CompactTag {
                language: lang,
                locale: lang,
                full: None,
            }
        }
    }

    // Go: internal/language/compact/language.go:192 FromTag
    /// FromTag reports closest matching ID for an internal language Tag.
    pub fn from_tag(t: &Tag) -> (Id, bool) {
        // TODO: perhaps give more frequent tags a lower index.
        // TODO: we could make the indexes stable. This will excluded some
        //       possibilities for optimization, so don't do this quite yet.
        let mut exact = true;

        let (b, s, r) = t.raw();
        let mut t = t.clone();
        if t.has_string() {
            if t.is_private_use() {
                // We have no entries for user-defined tags.
                return (0, false);
            }
            let mut has_extra = false;
            if t.has_variants() {
                if t.has_extensions() {
                    let mut build = il::Builder::default();
                    build.set_tag(&Tag {
                        lang_id: b,
                        script_id: s,
                        region_id: r,
                        ..Tag::UND
                    });
                    build.add_variant(&[t.variants()]);
                    exact = false;
                    t = build.make();
                }
                has_extra = true;
            } else if t.extension(b'u').is_some() {
                // TODO: va may mean something else. Consider not considering it.
                // Strip all but the 'va' entry.
                let old = t.clone();
                let variant = t.type_for_key("va");
                t = Tag {
                    lang_id: b,
                    script_id: s,
                    region_id: r,
                    ..Tag::UND
                };
                if !variant.is_empty() {
                    (t, _) = t.set_type_for_key("va", &variant);
                    has_extra = true;
                }
                exact = old == t;
            } else {
                exact = false;
            }
            if has_extra {
                // We have some variants.
                for (i, s) in SPECIAL_TAGS.iter().enumerate() {
                    if *s == t {
                        return ((i + CORE_TAGS.len()) as Id, exact);
                    }
                }
                exact = false;
            }
        }
        if let Some(x) = get_core_index(&t) {
            return (x, exact);
        }
        exact = false;
        if r.0 != 0 && s.0 == 0 {
            // Deal with cases where an extra script is inserted for the region.
            let (t, _) = t.maximize();
            if let Some(x) = get_core_index(&t) {
                return (x, exact);
            }
        }
        t = t.parent();
        while t != Tag::UND {
            // No variants specified: just compare core components.
            // The key has the form lllssrrr, where l, s, and r are nibbles for
            // respectively the langID, scriptID, and regionID.
            if let Some(x) = get_core_index(&t) {
                return (x, exact);
            }
            t = t.parent();
        }
        (UND, exact)
    }
}

/// Go: the generated tables of `golang.org/x/text/internal/language`
/// (tables.go) and `golang.org/x/text/language` (tables.go), CLDR 32.
/// PORT: printed from the pinned Go tables by a Go program. Go maps become
/// sorted slices, and Go struct literals become calls to the short
/// constructors below. Only the tables that the ported code reads are here.
#[allow(clippy::unreadable_literal, clippy::too_many_lines)]
mod tables {
    // Go: internal/language/tables.go:16 FromTo
    #[derive(Clone, Copy, Debug)]
    pub(super) struct FromTo {
        pub(super) from: u16,
        pub(super) to: u16,
    }

    const fn ft(from: u16, to: u16) -> FromTo {
        FromTo { from, to }
    }

    // Go: internal/language/tables.go:1416 likelyLangRegion
    #[derive(Clone, Copy, Debug)]
    pub(super) struct LikelyLangRegion {
        pub(super) lang: u16,
        pub(super) region: u16,
    }

    const fn llr(lang: u16, region: u16) -> LikelyLangRegion {
        LikelyLangRegion { lang, region }
    }

    // Go: internal/language/tables.go:1570 likelyScriptRegion
    #[derive(Clone, Copy, Debug)]
    pub(super) struct LikelyScriptRegion {
        pub(super) region: u16,
        pub(super) script: u16,
        pub(super) flags: u8,
    }

    const fn lsr(region: u16, script: u16, flags: u8) -> LikelyScriptRegion {
        LikelyScriptRegion {
            region,
            script,
            flags,
        }
    }

    // Go: internal/language/tables.go:3001 likelyLangScript
    #[derive(Clone, Copy, Debug)]
    pub(super) struct LikelyLangScript {
        pub(super) lang: u16,
        pub(super) script: u16,
        pub(super) flags: u8,
    }

    const fn lls(lang: u16, script: u16, flags: u8) -> LikelyLangScript {
        LikelyLangScript {
            lang,
            script,
            flags,
        }
    }

    // Go: internal/language/tables.go:3315 likelyTag
    #[derive(Clone, Copy, Debug)]
    pub(super) struct LikelyTag {
        pub(super) lang: u16,
        pub(super) region: u16,
        pub(super) script: u16,
    }

    const fn lt(lang: u16, region: u16, script: u16) -> LikelyTag {
        LikelyTag {
            lang,
            region,
            script,
        }
    }

    // Go: language/tables.go:110 mutualIntelligibility
    #[derive(Clone, Copy, Debug)]
    pub(super) struct MutualIntelligibility {
        pub(super) want: u16,
        pub(super) have: u16,
        pub(super) distance: u8,
        pub(super) oneway: bool,
    }

    const fn mi(want: u16, have: u16, distance: u8, oneway: bool) -> MutualIntelligibility {
        MutualIntelligibility {
            want,
            have,
            distance,
            oneway,
        }
    }

    // Go: language/tables.go:116 scriptIntelligibility
    #[derive(Clone, Copy, Debug)]
    pub(super) struct ScriptIntelligibility {
        pub(super) want_lang: u16,
        pub(super) have_lang: u16,
        pub(super) want_script: u8,
        pub(super) have_script: u8,
        pub(super) distance: u8,
    }

    const fn si(
        want_lang: u16,
        have_lang: u16,
        want_script: u8,
        have_script: u8,
        distance: u8,
    ) -> ScriptIntelligibility {
        ScriptIntelligibility {
            want_lang,
            have_lang,
            want_script,
            have_script,
            distance,
        }
    }

    // Go: language/tables.go:123 regionIntelligibility
    #[derive(Clone, Copy, Debug)]
    pub(super) struct RegionIntelligibility {
        pub(super) lang: u16,
        pub(super) script: u8,
        pub(super) group: u8,
        pub(super) distance: u8,
    }

    const fn ri(lang: u16, script: u8, group: u8, distance: u8) -> RegionIntelligibility {
        RegionIntelligibility {
            lang,
            script,
            group,
            distance,
        }
    }

    // Go: language/tables.go:104 paradigmLocales, before `init` in match.go
    // fills the zero regions.
    pub(super) const PARADIGM_LOCALES_TABLE: [[u16; 3]; 3] =
        [[0x139, 0x0, 0x7c], [0x13e, 0x0, 0x1f], [0x3c0, 0x41, 0xef]];

    // Go: internal/language/tables.go
    pub(super) const NUM_LANGUAGES: usize = 8798;
    pub(super) const NON_CANONICAL_UND: u16 = 1201;
    pub(super) const LANG_NO_INDEX_OFFSET: u16 = 1330;
    pub(super) const LANG_PRIVATE_START: u16 = 0x2f72;
    pub(super) const LANG_PRIVATE_END: u16 = 0x3179;
    pub(super) const ISO_REGION_OFFSET: u16 = 32;
    pub(super) const N_REGION_GROUPS: u8 = 33;
    pub(super) const _EN: u16 = 313;
    pub(super) const _MO: u16 = 784;
    pub(super) const _NB: u16 = 839;
    pub(super) const _NO: u16 = 879;
    pub(super) const _SH: u16 = 1031;
    pub(super) const _JBO: u16 = 515;
    pub(super) const _AMI: u16 = 1650;
    pub(super) const _BNN: u16 = 2357;
    pub(super) const _HAK: u16 = 438;
    pub(super) const _TLH: u16 = 14467;
    pub(super) const _LB: u16 = 661;
    pub(super) const _NV: u16 = 899;
    pub(super) const _PWN: u16 = 12055;
    pub(super) const _TAO: u16 = 14188;
    pub(super) const _TAY: u16 = 14198;
    pub(super) const _TSU: u16 = 14662;
    pub(super) const _NN: u16 = 874;
    pub(super) const _SFB: u16 = 13629;
    pub(super) const _VGT: u16 = 15701;
    pub(super) const _SGG: u16 = 13660;
    pub(super) const _CMN: u16 = 3007;
    pub(super) const _NAN: u16 = 835;
    pub(super) const _HSN: u16 = 467;
    pub(super) const _LATN: u16 = 91;
    pub(super) const _QAAA: u16 = 149;
    pub(super) const _QAAI: u16 = 157;
    pub(super) const _QABX: u16 = 198;
    pub(super) const _ZINH: u16 = 255;
    pub(super) const _ZZZZ: u16 = 261;
    pub(super) const _MD: u16 = 189;
    pub(super) const LANG: &[u8] = b"\
    ---\x00aaaraai\x00aak\x00aau\x00abbkabi\x00abq\x00abr\x00abt\x00aby\x00acd\x00ace\x00ach\x00ada\x00ade\x00\
    adj\x00ady\x00adz\x00aeveaeb\x00aey\x00affragc\x00agd\x00agg\x00agm\x00ago\x00agq\x00aha\x00ahl\x00aho\x00\
    ajg\x00akkaakk\x00ala\x00ali\x00aln\x00alt\x00ammhamm\x00amn\x00amo\x00amp\x00anrganc\x00ank\x00ann\x00\
    any\x00aoj\x00aom\x00aoz\x00apc\x00apd\x00ape\x00apr\x00aps\x00apz\x00arraarc\x00arh\x00arn\x00aro\x00arq\x00\
    ars\x00ary\x00arz\x00assmasa\x00ase\x00asg\x00aso\x00ast\x00ata\x00atg\x00atj\x00auy\x00avvaavl\x00avn\x00\
    avt\x00avu\x00awa\x00awb\x00awo\x00awx\x00ayymayb\x00azzebaakbal\x00ban\x00bap\x00bar\x00bas\x00bav\x00\
    bax\x00bba\x00bbb\x00bbc\x00bbd\x00bbj\x00bbp\x00bbr\x00bcf\x00bch\x00bci\x00bcm\x00bcn\x00bco\x00bcq\x00bcu\x00\
    bdd\x00beelbef\x00beh\x00bej\x00bem\x00bet\x00bew\x00bex\x00bez\x00bfd\x00bfq\x00bft\x00bfy\x00bgulbgc\x00\
    bgn\x00bgx\x00bhihbhb\x00bhg\x00bhi\x00bhk\x00bhl\x00bho\x00bhy\x00biisbib\x00big\x00bik\x00bim\x00bin\x00\
    bio\x00biq\x00bjh\x00bji\x00bjj\x00bjn\x00bjo\x00bjr\x00bjt\x00bjz\x00bkc\x00bkm\x00bkq\x00bku\x00bkv\x00blt\x00\
    bmambmh\x00bmk\x00bmq\x00bmu\x00bnenbng\x00bnm\x00bnp\x00boodboj\x00bom\x00bon\x00bpy\x00bqc\x00bqi\x00\
    bqp\x00bqv\x00brrebra\x00brh\x00brx\x00brz\x00bsosbsj\x00bsq\x00bss\x00bst\x00bto\x00btt\x00btv\x00bua\x00\
    buc\x00bud\x00bug\x00buk\x00bum\x00buo\x00bus\x00buu\x00bvb\x00bwd\x00bwr\x00bxh\x00bye\x00byn\x00byr\x00bys\x00\
    byv\x00byx\x00bza\x00bze\x00bzf\x00bzh\x00bzw\x00caatcan\x00cbj\x00cch\x00ccp\x00ceheceb\x00cfa\x00cgg\x00\
    chhachk\x00chm\x00cho\x00chp\x00chr\x00cja\x00cjm\x00cjv\x00ckb\x00ckl\x00cko\x00cky\x00cla\x00cme\x00cmg\x00\
    cooscop\x00cps\x00crrecrh\x00crj\x00crk\x00crl\x00crm\x00crs\x00csescsb\x00csw\x00ctd\x00cuhucvhv\
    cyymdaandad\x00daf\x00dag\x00dah\x00dak\x00dar\x00dav\x00dbd\x00dbq\x00dcc\x00ddn\x00deeuded\x00den\x00\
    dga\x00dgh\x00dgi\x00dgl\x00dgr\x00dgz\x00dia\x00dje\x00dnj\x00dob\x00doi\x00dop\x00dow\x00dri\x00drs\x00dsb\x00\
    dtm\x00dtp\x00dts\x00dty\x00dua\x00duc\x00dud\x00dug\x00dvivdva\x00dww\x00dyo\x00dyu\x00dzzodzg\x00ebu\x00\
    eeweefi\x00egl\x00egy\x00eka\x00eky\x00elllema\x00emi\x00enngenn\x00enq\x00eopoeri\x00es\x00\x05esu\x00\
    etstetr\x00ett\x00etu\x00etx\x00euusewo\x00ext\x00faasfaa\x00fab\x00fag\x00fai\x00fan\x00ffulffi\x00\
    ffm\x00fiinfia\x00fil\x00fit\x00fjijflr\x00fmp\x00foaofod\x00fon\x00for\x00fpe\x00fqs\x00frrafrc\x00\
    frp\x00frr\x00frs\x00fub\x00fud\x00fue\x00fuf\x00fuh\x00fuq\x00fur\x00fuv\x00fuy\x00fvr\x00fyrygalegaa\x00\
    gaf\x00gag\x00gah\x00gaj\x00gam\x00gan\x00gaw\x00gay\x00gba\x00gbf\x00gbm\x00gby\x00gbz\x00gcr\x00gdlagde\x00\
    gdn\x00gdr\x00geb\x00gej\x00gel\x00gez\x00gfk\x00ggn\x00ghs\x00gil\x00gim\x00gjk\x00gjn\x00gju\x00gkn\x00gkp\x00\
    gllgglk\x00gmm\x00gmv\x00gnrngnd\x00gng\x00god\x00gof\x00goi\x00gom\x00gon\x00gor\x00gos\x00got\x00grb\x00\
    grc\x00grt\x00grw\x00gsw\x00guujgub\x00guc\x00gud\x00gur\x00guw\x00gux\x00guz\x00gvlvgvf\x00gvr\x00gvs\x00\
    gwc\x00gwi\x00gwt\x00gyi\x00haauhag\x00hak\x00ham\x00haw\x00haz\x00hbb\x00hdy\x00heebhhy\x00hiinhia\x00\
    hif\x00hig\x00hih\x00hil\x00hla\x00hlu\x00hmd\x00hmt\x00hnd\x00hne\x00hnj\x00hnn\x00hno\x00homohoc\x00hoj\x00\
    hot\x00hrrvhsb\x00hsn\x00htathuunhui\x00hyyehzerianaian\x00iar\x00iba\x00ibb\x00iby\x00ica\x00\
    ich\x00idndidd\x00idi\x00idu\x00ieleife\x00igboigb\x00ige\x00iiiiijj\x00ikpkikk\x00ikt\x00ikw\x00\
    ikx\x00ilo\x00imo\x00inndinh\x00iodoiou\x00iri\x00isslittaiukuiw\x00\x03iwm\x00iws\x00izh\x00izi\x00\
    japnjab\x00jam\x00jbo\x00jbu\x00jen\x00jgk\x00jgo\x00ji\x00\x06jib\x00jmc\x00jml\x00jra\x00jut\x00jvavjwav\
    kaatkaa\x00kab\x00kac\x00kad\x00kai\x00kaj\x00kam\x00kao\x00kbd\x00kbm\x00kbp\x00kbq\x00kbx\x00kby\x00kcg\x00\
    kck\x00kcl\x00kct\x00kde\x00kdh\x00kdl\x00kdt\x00kea\x00ken\x00kez\x00kfo\x00kfr\x00kfy\x00kgonkge\x00kgf\x00\
    kgp\x00kha\x00khb\x00khn\x00khq\x00khs\x00kht\x00khw\x00khz\x00kiikkij\x00kiu\x00kiw\x00kjuakjd\x00kjg\x00\
    kjs\x00kjy\x00kkazkkc\x00kkj\x00klalkln\x00klq\x00klt\x00klx\x00kmhmkmb\x00kmh\x00kmo\x00kms\x00kmu\x00\
    kmw\x00knanknf\x00knp\x00koorkoi\x00kok\x00kol\x00kos\x00koz\x00kpe\x00kpf\x00kpo\x00kpr\x00kpx\x00kqb\x00\
    kqf\x00kqs\x00kqy\x00kraukrc\x00kri\x00krj\x00krl\x00krs\x00kru\x00ksasksb\x00ksd\x00ksf\x00ksh\x00ksj\x00\
    ksr\x00ktb\x00ktm\x00kto\x00kuurkub\x00kud\x00kue\x00kuj\x00kum\x00kun\x00kup\x00kus\x00kvomkvg\x00kvr\x00\
    kvx\x00kw\x00\x01kwj\x00kwo\x00kxa\x00kxc\x00kxm\x00kxp\x00kxw\x00kxz\x00kyirkye\x00kyx\x00kzr\x00laatlab\x00\
    lad\x00lag\x00lah\x00laj\x00las\x00lbtzlbe\x00lbu\x00lbw\x00lcm\x00lcp\x00ldb\x00led\x00lee\x00lem\x00lep\x00\
    leq\x00leu\x00lez\x00lguglgg\x00liimlia\x00lid\x00lif\x00lig\x00lih\x00lij\x00lis\x00ljp\x00lki\x00lkt\x00\
    lle\x00lln\x00lmn\x00lmo\x00lmp\x00lninlns\x00lnu\x00loaoloj\x00lok\x00lol\x00lor\x00los\x00loz\x00lrc\x00\
    ltitltg\x00luublua\x00luo\x00luy\x00luz\x00lvavlwl\x00lzh\x00lzz\x00mad\x00maf\x00mag\x00mai\x00mak\x00\
    man\x00mas\x00maw\x00maz\x00mbh\x00mbo\x00mbq\x00mbu\x00mbw\x00mci\x00mcp\x00mcq\x00mcr\x00mcu\x00mda\x00mde\x00\
    mdf\x00mdh\x00mdj\x00mdr\x00mdx\x00med\x00mee\x00mek\x00men\x00mer\x00met\x00meu\x00mfa\x00mfe\x00mfn\x00mfo\x00\
    mfq\x00mglgmgh\x00mgl\x00mgo\x00mgp\x00mgy\x00mhahmhi\x00mhl\x00mirimif\x00min\x00mis\x00miw\x00mkkd\
    mki\x00mkl\x00mkp\x00mkw\x00mlalmle\x00mlp\x00mls\x00mmo\x00mmu\x00mmx\x00mnonmna\x00mnf\x00mni\x00mnw\x00\
    moolmoa\x00moe\x00moh\x00mos\x00mox\x00mpp\x00mps\x00mpt\x00mpx\x00mql\x00mrarmrd\x00mrj\x00mro\x00mssa\
    mtltmtc\x00mtf\x00mti\x00mtr\x00mua\x00mul\x00mur\x00mus\x00mva\x00mvn\x00mvy\x00mwk\x00mwr\x00mwv\x00mxc\x00\
    mxm\x00myyamyk\x00mym\x00myv\x00myw\x00myx\x00myz\x00mzk\x00mzm\x00mzn\x00mzp\x00mzw\x00mzz\x00naaunac\x00\
    naf\x00nah\x00nak\x00nan\x00nap\x00naq\x00nas\x00nbobnca\x00nce\x00ncf\x00nch\x00nco\x00ncu\x00nddendc\x00\
    nds\x00neepneb\x00new\x00nex\x00nfr\x00ngdonga\x00ngb\x00ngl\x00nhb\x00nhe\x00nhw\x00nif\x00nii\x00nij\x00\
    nin\x00niu\x00niy\x00niz\x00njo\x00nkg\x00nko\x00nlldnmg\x00nmz\x00nnnonnf\x00nnh\x00nnk\x00nnm\x00noor\
    nod\x00noe\x00non\x00nop\x00nou\x00nqo\x00nrblnrb\x00nsk\x00nsn\x00nso\x00nss\x00ntm\x00ntr\x00nui\x00nup\x00\
    nus\x00nuv\x00nux\x00nvavnwb\x00nxq\x00nxr\x00nyyanym\x00nyn\x00nzi\x00occiogc\x00ojjiokr\x00okv\x00\
    omrmong\x00onn\x00ons\x00opm\x00orrioro\x00oru\x00osssosa\x00ota\x00otk\x00ozm\x00paanpag\x00pal\x00\
    pam\x00pap\x00pau\x00pbi\x00pcd\x00pcm\x00pdc\x00pdt\x00ped\x00peo\x00pex\x00pfl\x00phl\x00phn\x00pilipil\x00\
    pip\x00pka\x00pko\x00plolpla\x00pms\x00png\x00pnn\x00pnt\x00pon\x00ppo\x00pra\x00prd\x00prg\x00psuspss\x00\
    ptorptp\x00puu\x00pwa\x00quuequc\x00qug\x00rai\x00raj\x00rao\x00rcf\x00rej\x00rel\x00res\x00rgn\x00rhg\x00\
    ria\x00rif\x00rjs\x00rkt\x00rmohrmf\x00rmo\x00rmt\x00rmu\x00rnunrna\x00rng\x00roonrob\x00rof\x00roo\x00\
    rro\x00rtm\x00ruusrue\x00rug\x00rw\x00\x04rwk\x00rwo\x00ryu\x00saansaf\x00sah\x00saq\x00sas\x00sat\x00sav\x00\
    saz\x00sba\x00sbe\x00sbp\x00scrdsck\x00scl\x00scn\x00sco\x00scs\x00sdndsdc\x00sdh\x00semesef\x00seh\x00\
    sei\x00ses\x00sgagsga\x00sgs\x00sgw\x00sgz\x00sh\x00\x02shi\x00shk\x00shn\x00shu\x00siinsid\x00sig\x00sil\x00\
    sim\x00sjr\x00sklkskc\x00skr\x00sks\x00sllvsld\x00sli\x00sll\x00sly\x00smmosma\x00smi\x00smj\x00smn\x00\
    smp\x00smq\x00sms\x00snnasnc\x00snk\x00snp\x00snx\x00sny\x00soomsok\x00soq\x00sou\x00soy\x00spd\x00spl\x00\
    sps\x00sqqisrrpsrb\x00srn\x00srr\x00srx\x00ssswssd\x00ssg\x00ssy\x00stotstk\x00stq\x00suunsua\x00\
    sue\x00suk\x00sur\x00sus\x00svweswwaswb\x00swc\x00swg\x00swp\x00swv\x00sxn\x00sxw\x00syl\x00syr\x00szl\x00\
    taamtaj\x00tal\x00tan\x00taq\x00tbc\x00tbd\x00tbf\x00tbg\x00tbo\x00tbw\x00tbz\x00tci\x00tcy\x00tdd\x00tdg\x00\
    tdh\x00teelted\x00tem\x00teo\x00tet\x00tfi\x00tggktgc\x00tgo\x00tgu\x00thhathl\x00thq\x00thr\x00tiir\
    tif\x00tig\x00tik\x00tim\x00tio\x00tiv\x00tkuktkl\x00tkr\x00tkt\x00tlgltlf\x00tlx\x00tly\x00tmh\x00tmy\x00\
    tnsntnh\x00toontof\x00tog\x00toq\x00tpi\x00tpm\x00tpz\x00tqo\x00trurtru\x00trv\x00trw\x00tssotsd\x00\
    tsf\x00tsg\x00tsj\x00tsw\x00ttatttd\x00tte\x00ttj\x00ttr\x00tts\x00ttt\x00tuh\x00tul\x00tum\x00tuq\x00tvd\x00\
    tvl\x00tvu\x00twwitwh\x00twq\x00txg\x00tyahtya\x00tyv\x00tzm\x00ubu\x00udm\x00ugiguga\x00ukkruli\x00\
    umb\x00und\x00unr\x00unx\x00urrduri\x00urt\x00urw\x00usa\x00utr\x00uvh\x00uvl\x00uzzbvag\x00vai\x00van\x00\
    veenvec\x00vep\x00viievic\x00viv\x00vls\x00vmf\x00vmw\x00voolvot\x00vro\x00vun\x00vut\x00walnwae\x00\
    waj\x00wal\x00wan\x00war\x00wbp\x00wbq\x00wbr\x00wci\x00wer\x00wgi\x00whg\x00wib\x00wiu\x00wiv\x00wja\x00wji\x00\
    wls\x00wmo\x00wnc\x00wni\x00wnu\x00woolwob\x00wos\x00wrs\x00wsk\x00wtm\x00wuu\x00wuv\x00wwa\x00xav\x00xbi\x00\
    xcr\x00xes\x00xhhoxla\x00xlc\x00xld\x00xmf\x00xmn\x00xmr\x00xna\x00xnr\x00xog\x00xon\x00xpr\x00xrb\x00xsa\x00\
    xsi\x00xsm\x00xsr\x00xwe\x00yam\x00yao\x00yap\x00yas\x00yat\x00yav\x00yay\x00yaz\x00yba\x00ybb\x00yby\x00yer\x00\
    ygr\x00ygw\x00yiidyko\x00yle\x00ylg\x00yll\x00yml\x00yooryon\x00yrb\x00yre\x00yrl\x00yss\x00yua\x00yue\x00\
    yuj\x00yut\x00yuw\x00zahazag\x00zbl\x00zdj\x00zea\x00zgh\x00zhhozhx\x00zia\x00zlm\x00zmi\x00zne\x00zuul\
    zxx\x00zza\x00\xff\xff\xff\xff";
    pub(super) static LANG_NO_INDEX: [u8; 2197] = [
        0xff, 0xf8, 0xed, 0xfe, 0xeb, 0xd3, 0x3b, 0xd2, 0xfb, 0xbf, 0x7a, 0xfa, 0x37, 0x1d, 0x3c,
        0x57, 0x6e, 0x97, 0x73, 0x38, 0xfb, 0xea, 0xbf, 0x70, 0xad, 0x03, 0xff, 0xff, 0xcf, 0x05,
        0x84, 0x72, 0xe9, 0xbf, 0xfd, 0xbf, 0xbf, 0xf7, 0xfd, 0x77, 0x0f, 0xff, 0xef, 0x6f, 0xff,
        0xfb, 0xdf, 0xe2, 0xc9, 0xf8, 0x7f, 0x7e, 0x4d, 0xbc, 0x0a, 0x6a, 0x7c, 0xea, 0xe3, 0xfa,
        0x7a, 0xbf, 0x67, 0xff, 0xff, 0xff, 0xff, 0xdf, 0x2a, 0x54, 0x91, 0xc0, 0x5d, 0xe3, 0x97,
        0x14, 0x07, 0x20, 0xdd, 0xed, 0x9f, 0x3f, 0xc9, 0x21, 0xf8, 0x3f, 0x94, 0x35, 0x7c, 0x5f,
        0xff, 0x5f, 0x8e, 0x6e, 0xdf, 0xff, 0xff, 0xff, 0x55, 0x7c, 0xd3, 0xfd, 0xbf, 0xb5, 0x7b,
        0xdf, 0x7f, 0xf7, 0xca, 0xfe, 0xdb, 0xa3, 0xa8, 0xff, 0x1f, 0x67, 0x7d, 0xeb, 0xef, 0xce,
        0xff, 0xff, 0x9f, 0xff, 0xb7, 0xef, 0xfe, 0xcf, 0xdb, 0xff, 0xf3, 0xcd, 0xfb, 0x7f, 0xff,
        0xff, 0xbb, 0xee, 0xf7, 0xbd, 0xdb, 0xff, 0x5f, 0xf7, 0xfd, 0xf2, 0xfd, 0xff, 0x5e, 0x2f,
        0x3b, 0xba, 0x7e, 0xff, 0xff, 0xfe, 0xf7, 0xff, 0xdd, 0xff, 0xfd, 0xdf, 0xfb, 0xfe, 0x9d,
        0xb4, 0xd3, 0xff, 0xef, 0xff, 0xdf, 0xf7, 0x7f, 0xb7, 0xfd, 0xd5, 0xa5, 0x77, 0x40, 0xff,
        0x9c, 0xc1, 0x41, 0x2c, 0x08, 0x21, 0x41, 0x00, 0x50, 0x40, 0x00, 0x80, 0xfb, 0x4a, 0xf2,
        0x9f, 0xb4, 0x42, 0x41, 0x96, 0x1b, 0x14, 0x08, 0xf3, 0x2b, 0xe7, 0x17, 0x56, 0x05, 0x7d,
        0x0e, 0x1c, 0x37, 0x7f, 0xf3, 0xef, 0x97, 0xff, 0x5d, 0x38, 0x64, 0x08, 0x00, 0x10, 0xbc,
        0x85, 0xaf, 0xdf, 0xff, 0xff, 0x7b, 0x35, 0x3e, 0xc7, 0xc7, 0xdf, 0xff, 0x01, 0x81, 0x00,
        0xb0, 0x05, 0x80, 0x00, 0x20, 0x00, 0x00, 0x03, 0x40, 0x00, 0x40, 0x92, 0x21, 0x50, 0xb1,
        0x5d, 0xfd, 0xdc, 0xbe, 0x5e, 0x00, 0x00, 0x02, 0x64, 0x0d, 0x19, 0x41, 0xdf, 0x79, 0x22,
        0x00, 0x00, 0x00, 0x5e, 0x64, 0xdc, 0x24, 0xe5, 0xd9, 0xe3, 0xfe, 0xff, 0xfd, 0xcb, 0x9f,
        0x14, 0x41, 0x0c, 0x86, 0x00, 0xd1, 0x00, 0xf0, 0xc7, 0x67, 0x5f, 0x56, 0x99, 0x5e, 0xb5,
        0x6c, 0xaf, 0x03, 0x00, 0x02, 0x00, 0x00, 0x00, 0xc0, 0x37, 0xda, 0x56, 0x90, 0x6d, 0x01,
        0x2e, 0x96, 0x69, 0x20, 0xfb, 0xff, 0x3f, 0x00, 0x00, 0x00, 0x01, 0x0c, 0x16, 0x03, 0x00,
        0x00, 0xb0, 0x14, 0x23, 0x50, 0x06, 0x0a, 0x00, 0x01, 0x00, 0x00, 0x10, 0x11, 0x09, 0x00,
        0x00, 0x60, 0x10, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x44, 0x00, 0x00, 0x10, 0x00, 0x05,
        0x08, 0x00, 0x00, 0x05, 0x00, 0x80, 0x28, 0x04, 0x00, 0x00, 0x40, 0xd5, 0x2d, 0x00, 0x64,
        0x35, 0x24, 0x52, 0xf4, 0xd5, 0xbf, 0x62, 0xc9, 0x03, 0x00, 0x80, 0x00, 0x40, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x04, 0x13, 0x39, 0x01, 0xdd, 0x57, 0x98, 0x21, 0x18, 0x81, 0x08, 0x00,
        0x01, 0x40, 0x82, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x40, 0x00, 0x44,
        0x00, 0x00, 0x80, 0xea, 0xa9, 0x39, 0x00, 0x02, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x20, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0x03,
        0x28, 0x05, 0x00, 0x00, 0x00, 0x00, 0x04, 0x20, 0x04, 0xa6, 0x00, 0x04, 0x00, 0x00, 0x81,
        0x50, 0x00, 0x00, 0x00, 0x11, 0x84, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x06, 0x55,
        0x02, 0x10, 0x08, 0x04, 0x00, 0x00, 0x00, 0x40, 0x30, 0x83, 0x01, 0x00, 0x00, 0x00, 0x11,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x1e, 0xcd, 0xbf,
        0x7a, 0xbf, 0xdf, 0xc3, 0x83, 0x82, 0xc0, 0xfb, 0x57, 0x27, 0xed, 0x55, 0xe7, 0x01, 0x00,
        0x20, 0xb2, 0xc5, 0xa4, 0x45, 0x25, 0x9b, 0x02, 0xdf, 0xe1, 0xdf, 0x03, 0x44, 0x08, 0x90,
        0x01, 0x04, 0x81, 0xe3, 0x92, 0x54, 0xdb, 0x28, 0xd3, 0x5f, 0xfe, 0x6d, 0x79, 0xed, 0x1c,
        0x7f, 0x04, 0x08, 0x00, 0x01, 0x21, 0x12, 0x64, 0x5f, 0xdd, 0x0e, 0x85, 0x4f, 0x40, 0x40,
        0x00, 0x04, 0xf1, 0xfd, 0x3d, 0x54, 0xe8, 0x03, 0xb4, 0x27, 0x23, 0x0d, 0x00, 0x00, 0x20,
        0x7b, 0x78, 0x02, 0x07, 0x84, 0x00, 0xf0, 0xbb, 0x7e, 0x5a, 0x00, 0x18, 0x04, 0x81, 0x00,
        0x00, 0x00, 0x80, 0x10, 0x90, 0x1c, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x40, 0x00,
        0x04, 0x08, 0xa0, 0x70, 0xa5, 0x0c, 0x40, 0x00, 0x00, 0x91, 0x24, 0x04, 0x68, 0x00, 0x20,
        0x70, 0xff, 0x7b, 0x7f, 0x70, 0x00, 0x05, 0x9b, 0xdd, 0x66, 0x03, 0x00, 0x11, 0x00, 0x00,
        0x00, 0x40, 0x05, 0xb5, 0xb6, 0x80, 0x08, 0x04, 0x00, 0x04, 0x51, 0xe2, 0xef, 0xfd, 0x3f,
        0x05, 0x09, 0x08, 0x05, 0x40, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00, 0x0c, 0x00, 0x00,
        0x00, 0x00, 0x81, 0x00, 0x60, 0xe7, 0x48, 0x00, 0x81, 0x20, 0xc0, 0x05, 0x80, 0x03, 0x00,
        0x00, 0x00, 0x8c, 0x50, 0x40, 0x04, 0x84, 0x47, 0x84, 0x40, 0x20, 0x10, 0x00, 0x20, 0x02,
        0x50, 0x80, 0x11, 0x00, 0x99, 0x6c, 0xe2, 0x50, 0x27, 0x1d, 0x11, 0x29, 0x0e, 0x59, 0xe9,
        0x33, 0x08, 0x00, 0x20, 0x04, 0x40, 0x10, 0x00, 0x00, 0x00, 0x50, 0x44, 0x92, 0x49, 0xd6,
        0x5d, 0xa7, 0x81, 0x47, 0x97, 0xfb, 0x00, 0x10, 0x00, 0x08, 0x00, 0x80, 0x00, 0x40, 0x04,
        0x00, 0x01, 0x02, 0x00, 0x01, 0x40, 0x80, 0x00, 0x40, 0x08, 0xd8, 0xeb, 0xf6, 0x39, 0xc4,
        0x8d, 0x12, 0x00, 0x00, 0x0c, 0x04, 0x01, 0x20, 0x20, 0xdd, 0xa0, 0x01, 0x00, 0x00, 0x00,
        0x12, 0x00, 0x00, 0x00, 0x04, 0x10, 0xd0, 0x9d, 0x95, 0x13, 0x04, 0x80, 0x00, 0x01, 0xd0,
        0x16, 0x40, 0x00, 0x10, 0xb0, 0x10, 0x62, 0x4c, 0xd2, 0x02, 0x01, 0x4a, 0x00, 0x46, 0x04,
        0x00, 0x08, 0x02, 0x00, 0x20, 0x80, 0x00, 0x80, 0x06, 0x00, 0x08, 0x00, 0x00, 0x00, 0x00,
        0xf0, 0xd8, 0x6f, 0x15, 0x02, 0x08, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x10, 0x01,
        0x00, 0x10, 0x00, 0x00, 0x00, 0xf0, 0x84, 0xe3, 0xdd, 0xbf, 0xf9, 0xf9, 0x3b, 0x7f, 0x7f,
        0xdb, 0xfd, 0xfc, 0xfe, 0xdf, 0xff, 0xfd, 0xff, 0xf6, 0xfb, 0xfc, 0xf7, 0x1f, 0xff, 0xb3,
        0x6c, 0xff, 0xd9, 0xad, 0xdf, 0xfe, 0xef, 0xba, 0xdf, 0xff, 0xff, 0xff, 0xb7, 0xdd, 0x7d,
        0xbf, 0xab, 0x7f, 0xfd, 0xfd, 0xdf, 0x2f, 0x9c, 0xdf, 0xf3, 0x6f, 0xdf, 0xdd, 0xff, 0xfb,
        0xee, 0xd2, 0xab, 0x5f, 0xd5, 0xdf, 0x7f, 0xff, 0xeb, 0xff, 0xe4, 0x4d, 0xf9, 0xff, 0xfe,
        0xf7, 0xfd, 0xdf, 0xfb, 0xbf, 0xee, 0xdb, 0x6f, 0xef, 0xff, 0x7f, 0xff, 0xff, 0xf7, 0x5f,
        0xd3, 0x3b, 0xfd, 0xd9, 0xdf, 0xeb, 0xbc, 0x08, 0x05, 0x24, 0xff, 0x07, 0x70, 0xfe, 0xe6,
        0x5e, 0x00, 0x08, 0x00, 0x83, 0x7d, 0x1f, 0x06, 0xe6, 0x72, 0x60, 0xd1, 0x3c, 0x7f, 0x44,
        0x02, 0x30, 0x9f, 0x7a, 0x16, 0xbd, 0x7f, 0x57, 0xf2, 0xff, 0x31, 0xff, 0xf2, 0x1e, 0x90,
        0xf7, 0xf1, 0xf9, 0x45, 0x80, 0x01, 0x02, 0x00, 0x20, 0x40, 0x54, 0x9f, 0x8a, 0xdf, 0xf9,
        0x6e, 0x11, 0x86, 0x51, 0xc0, 0xf3, 0xfb, 0x47, 0x40, 0x03, 0x05, 0xd1, 0x50, 0x5c, 0x00,
        0x40, 0x00, 0x10, 0x04, 0x02, 0x00, 0x00, 0x0a, 0x00, 0x17, 0xd2, 0xb9, 0xfd, 0xfc, 0xba,
        0xfe, 0xef, 0xc7, 0xbe, 0x53, 0x6f, 0xdf, 0xe7, 0xdb, 0x65, 0xbb, 0x7f, 0xfa, 0xff, 0x77,
        0xf3, 0xef, 0xbf, 0xfd, 0xf7, 0xdf, 0xdf, 0x9b, 0x7f, 0xff, 0xff, 0x7f, 0x6f, 0xf7, 0xfb,
        0xeb, 0xdf, 0xbc, 0xff, 0xbf, 0x6b, 0x7b, 0xfb, 0xff, 0xce, 0x76, 0xbd, 0xf7, 0xf7, 0xdf,
        0xdc, 0xf7, 0xf7, 0xff, 0xdf, 0xf3, 0xfe, 0xef, 0xff, 0xff, 0xff, 0xb6, 0x7f, 0x7f, 0xde,
        0xf7, 0xb9, 0xeb, 0x77, 0xff, 0xfb, 0xbf, 0xdf, 0xfd, 0xfe, 0xfb, 0xff, 0xfe, 0xeb, 0x1f,
        0x7d, 0x2f, 0xfd, 0xb6, 0xb5, 0xa5, 0xfc, 0xff, 0xfd, 0x7f, 0x4e, 0xbf, 0x8f, 0xae, 0xff,
        0xee, 0xdf, 0x7f, 0xf7, 0x73, 0x02, 0x02, 0x04, 0xfc, 0xf7, 0xff, 0xb7, 0xd7, 0xef, 0xfe,
        0xcd, 0xf5, 0xce, 0xe2, 0x8e, 0xe7, 0xbf, 0xb7, 0xff, 0x56, 0xfd, 0xcd, 0xff, 0xfb, 0xff,
        0xdf, 0xd7, 0xea, 0xff, 0xe5, 0x5f, 0x6d, 0x0f, 0xa7, 0x51, 0x06, 0xc4, 0x93, 0x50, 0x5d,
        0xaf, 0xa6, 0xff, 0x99, 0xfb, 0x63, 0x1d, 0x53, 0xff, 0xef, 0xb7, 0x35, 0x20, 0x14, 0x00,
        0x55, 0x51, 0xc2, 0x65, 0xf5, 0x41, 0xe2, 0xff, 0xfc, 0xdf, 0x02, 0x85, 0xc5, 0x05, 0x00,
        0x22, 0x00, 0x74, 0x69, 0x10, 0x08, 0x05, 0x41, 0x00, 0x01, 0x06, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x51, 0x20, 0x05, 0x04, 0x01, 0x00, 0x00, 0x06, 0x11, 0x20, 0x00, 0x18, 0x01, 0x92,
        0xf1, 0xfd, 0x47, 0x69, 0x06, 0x95, 0x06, 0x57, 0xed, 0xfb, 0x4d, 0x1c, 0x6b, 0x83, 0x04,
        0x62, 0x40, 0x00, 0x11, 0x42, 0x00, 0x00, 0x00, 0x54, 0x83, 0xb8, 0x4f, 0x10, 0x8e, 0x89,
        0x46, 0xde, 0xf7, 0x13, 0x31, 0x00, 0x20, 0x00, 0x00, 0x00, 0x90, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x0a, 0x10, 0x00, 0x01, 0x00, 0x00, 0xf0, 0x5b, 0xf4, 0xbe, 0x3d, 0xbe, 0xcf, 0xf7,
        0xaf, 0x42, 0x04, 0x84, 0x41, 0x30, 0xff, 0x79, 0x72, 0x04, 0x00, 0x00, 0x49, 0x2d, 0x14,
        0x27, 0x5f, 0xed, 0xf1, 0x3f, 0xe7, 0x3f, 0x00, 0x00, 0x02, 0xc6, 0xa0, 0x1e, 0xf8, 0xbb,
        0xff, 0xfd, 0xfb, 0xb7, 0xfd, 0xe7, 0xf7, 0xfd, 0xfc, 0xd5, 0xed, 0x47, 0xf4, 0x7e, 0x10,
        0x01, 0x01, 0x84, 0x6d, 0xff, 0xf7, 0xdd, 0xf9, 0x5b, 0x05, 0x86, 0xed, 0xf5, 0x77, 0xbd,
        0x3c, 0x00, 0x00, 0x00, 0x42, 0x71, 0x42, 0x00, 0x40, 0x00, 0x00, 0x01, 0x43, 0x19, 0x24,
        0x08, 0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
        0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
        0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
        0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
        0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xab, 0xbd, 0xe7, 0x57, 0xee, 0x13, 0x5d, 0x09,
        0xc1, 0x40, 0x21, 0xfa, 0x17, 0x01, 0x80, 0x00, 0x00, 0x00, 0x00, 0xf0, 0xce, 0xfb, 0xbf,
        0x00, 0x23, 0x00, 0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0x30, 0x15, 0xa3, 0x10, 0x00, 0x00,
        0x00, 0x11, 0x04, 0x16, 0x00, 0x00, 0x02, 0x20, 0x81, 0xa3, 0x01, 0x50, 0x00, 0x00, 0x83,
        0x11, 0x40, 0x00, 0x00, 0x00, 0xf0, 0xdd, 0x7b, 0xbe, 0x02, 0xaa, 0x10, 0x5d, 0x98, 0x52,
        0x00, 0x80, 0x20, 0x00, 0x00, 0x00, 0x00, 0x40, 0x00, 0x02, 0x02, 0x3d, 0x40, 0x10, 0x02,
        0x10, 0x61, 0x5a, 0x9d, 0x31, 0x00, 0x00, 0x00, 0x01, 0x18, 0x02, 0x20, 0x00, 0x00, 0x01,
        0x00, 0x42, 0x00, 0x20, 0x00, 0x00, 0x1f, 0xdf, 0xd2, 0xb9, 0xff, 0xfd, 0x3f, 0x1f, 0x98,
        0xcf, 0x9c, 0xff, 0xaf, 0x5f, 0xfe, 0x7b, 0x4b, 0x40, 0x10, 0xe1, 0xfd, 0xaf, 0xd9, 0xb7,
        0xf6, 0xfb, 0xb3, 0xc7, 0xff, 0x6f, 0xf1, 0x73, 0xb1, 0x7f, 0x9f, 0x7f, 0xbd, 0xfc, 0xb7,
        0xee, 0x1c, 0xfa, 0xcb, 0xef, 0xdd, 0xf9, 0xbd, 0x6e, 0xae, 0x55, 0xfd, 0x6e, 0x81, 0x76,
        0x9f, 0xd4, 0x77, 0xf5, 0x7d, 0xfb, 0xff, 0xeb, 0xfe, 0xbe, 0x5f, 0x46, 0x5b, 0xe9, 0x5f,
        0x50, 0x18, 0x02, 0xfa, 0xf7, 0x9d, 0x15, 0x97, 0x05, 0x0f, 0x75, 0xc4, 0x7d, 0x81, 0x92,
        0xf5, 0x57, 0x6c, 0xff, 0xe4, 0xef, 0x6f, 0xff, 0xfc, 0xdd, 0xde, 0xfc, 0xfd, 0x76, 0x5f,
        0x7a, 0x3f, 0x00, 0x98, 0x02, 0xfb, 0xa3, 0xef, 0xf3, 0xd6, 0xf2, 0xff, 0xb9, 0xda, 0x7d,
        0xd0, 0x3e, 0x15, 0x7b, 0xb4, 0xf5, 0x3e, 0xff, 0xff, 0xf1, 0xf7, 0xff, 0xe7, 0x5f, 0xff,
        0xff, 0x9e, 0xdf, 0xf6, 0xd7, 0xb9, 0xef, 0x27, 0x80, 0xbb, 0xc5, 0xff, 0xff, 0xe3, 0x97,
        0x9d, 0xbf, 0x9f, 0xf7, 0xc7, 0xfd, 0x37, 0xce, 0x7f, 0x44, 0x1d, 0x73, 0x7f, 0xf8, 0xda,
        0x5d, 0xce, 0x7d, 0x06, 0xb9, 0xea, 0x79, 0xa0, 0x1a, 0x20, 0x00, 0x30, 0x02, 0x04, 0x24,
        0x08, 0x04, 0x00, 0x00, 0x40, 0xd4, 0x02, 0x04, 0x00, 0x00, 0x04, 0x00, 0x04, 0x00, 0x20,
        0x09, 0x06, 0x50, 0x00, 0x08, 0x00, 0x00, 0x00, 0x24, 0x00, 0x04, 0x00, 0x10, 0xdc, 0x58,
        0xd7, 0x0d, 0x0f, 0x54, 0x4d, 0xf1, 0x16, 0x44, 0xd5, 0x42, 0x08, 0x40, 0x02, 0x00, 0x40,
        0x00, 0x08, 0x00, 0x00, 0x00, 0xdc, 0xfb, 0xcb, 0x0e, 0x58, 0x48, 0x41, 0x24, 0x20, 0x04,
        0x00, 0x30, 0x12, 0x40, 0x00, 0x00, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00,
        0x00, 0x00, 0x80, 0x10, 0x10, 0xab, 0x6d, 0x93, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x80, 0x80, 0x25, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x0a, 0x00, 0x00, 0x00,
        0x80, 0x86, 0xc2, 0x00, 0x00, 0x01, 0x00, 0x01, 0xff, 0x18, 0x02, 0x00, 0x02, 0xf0, 0xfd,
        0x79, 0x3b, 0x00, 0x25, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x40,
        0x00, 0x00, 0x03, 0x00, 0x09, 0x20, 0x00, 0x00, 0x01, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00,
        0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xef,
        0xd5, 0xfd, 0xcf, 0x7e, 0xb0, 0x11, 0x00, 0x00, 0x00, 0x92, 0x01, 0x46, 0xcd, 0xf9, 0x5c,
        0x00, 0x01, 0x00, 0x30, 0x04, 0x04, 0x55, 0x00, 0x01, 0x04, 0xf4, 0x3f, 0x4a, 0x01, 0x00,
        0x00, 0xb0, 0x80, 0x20, 0x55, 0x75, 0x97, 0x7c, 0xdf, 0x31, 0xcc, 0x68, 0xd1, 0x03, 0xd5,
        0x57, 0x27, 0x14, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x2c, 0xf7, 0xcb, 0x1f, 0x14, 0x60,
        0x83, 0x68, 0x01, 0x10, 0x8b, 0x38, 0x8a, 0x01, 0x00, 0x00, 0x20, 0x00, 0x24, 0x44, 0x00,
        0x00, 0x10, 0x03, 0x31, 0x02, 0x01, 0x00, 0x00, 0xf0, 0xf5, 0xff, 0xd5, 0x97, 0xbc, 0x70,
        0xd6, 0x78, 0x78, 0x15, 0x50, 0x05, 0xa4, 0x84, 0xa9, 0x41, 0x00, 0x00, 0x00, 0x6b, 0x39,
        0x52, 0x74, 0x40, 0xe8, 0x30, 0x90, 0x6a, 0x92, 0x00, 0x00, 0x02, 0xff, 0xef, 0xff, 0x4b,
        0x85, 0x53, 0xf4, 0xed, 0xdd, 0xbf, 0xf2, 0x5d, 0xc7, 0x0c, 0xd5, 0x42, 0xfc, 0xff, 0xf7,
        0x1f, 0x00, 0x80, 0x40, 0x56, 0xcc, 0x16, 0x9e, 0xea, 0x35, 0x7d, 0xef, 0xff, 0xbd, 0xa4,
        0xaf, 0x01, 0x44, 0x18, 0x01, 0x4d, 0x4e, 0x4a, 0x08, 0x50, 0x28, 0x30, 0xe0, 0x80, 0x10,
        0x20, 0x24, 0x00, 0xff, 0x2f, 0xd3, 0x60, 0xfe, 0x01, 0x02, 0x88, 0x2a, 0x40, 0x16, 0x01,
        0x01, 0x15, 0x2b, 0x3c, 0x01, 0x00, 0x00, 0x10, 0x90, 0x49, 0x41, 0x02, 0x02, 0x01, 0xe1,
        0xbf, 0xbf, 0x03, 0x00, 0x00, 0x10, 0xdc, 0xa3, 0xd1, 0x40, 0x9c, 0x44, 0xdf, 0xf5, 0x8f,
        0x66, 0xb3, 0x55, 0x20, 0xd4, 0xc1, 0xd8, 0x30, 0x3d, 0x80, 0x00, 0x00, 0x00, 0x04, 0xd4,
        0x11, 0xc5, 0x84, 0x2f, 0x50, 0x00, 0x22, 0x50, 0x6e, 0xbd, 0x93, 0x07, 0x00, 0x20, 0x10,
        0x84, 0xb2, 0x45, 0x10, 0x06, 0x44, 0x00, 0x00, 0x12, 0x02, 0x11, 0x00, 0xf0, 0xfb, 0xfd,
        0x7f, 0x05, 0x00, 0x16, 0x89, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x0c, 0x03, 0x00, 0x00,
        0x00, 0x00, 0x03, 0x30, 0x02, 0x28, 0x84, 0x00, 0x21, 0xc0, 0x23, 0x24, 0x00, 0x00, 0x00,
        0xcb, 0xe4, 0x3a, 0x46, 0x88, 0x54, 0xf1, 0xef, 0xff, 0x7f, 0x12, 0x01, 0x01, 0x84, 0x50,
        0x07, 0xfc, 0xff, 0xff, 0x0f, 0x01, 0x00, 0x40, 0x10, 0x38, 0x01, 0x01, 0x1c, 0x12, 0x40,
        0xe1, 0x76, 0x16, 0x08, 0x03, 0x10, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x20, 0x24, 0x0a, 0x00, 0x80, 0x00, 0x00,
    ];
    pub(super) const ALT_LANG_ISO3: &[u8] = b"\
    ---\x00cor\x00hbs\x01heb\x02kin\x03spa\x04yid\x05\xff\xff\xff\xff";
    pub(super) static ALT_LANG_INDEX: [u16; 6] = [0x0281, 0x0407, 0x01fb, 0x03e5, 0x013e, 0x0208];
    pub(super) static ALIAS_MAP: [FromTo; 193] = [
        ft(0x82, 0x88),
        ft(0x187, 0x1ae),
        ft(0x1f3, 0x1e1),
        ft(0x1fb, 0x1bc),
        ft(0x208, 0x512),
        ft(0x20f, 0x20e),
        ft(0x310, 0x3dc),
        ft(0x347, 0x36f),
        ft(0x407, 0x432),
        ft(0x47a, 0x153),
        ft(0x490, 0x451),
        ft(0x4a2, 0x21),
        ft(0x53e, 0x544),
        ft(0x58f, 0x12d),
        ft(0x62b, 0x34),
        ft(0x62f, 0x14),
        ft(0x630, 0x1eb1),
        ft(0x651, 0x431),
        ft(0x662, 0x431),
        ft(0x6ed, 0x3a),
        ft(0x6f8, 0x1d7),
        ft(0x709, 0x3625),
        ft(0x73e, 0x21a1),
        ft(0x7b3, 0x56),
        ft(0x7b9, 0x299b),
        ft(0x7c5, 0x58),
        ft(0x7e6, 0x145),
        ft(0x80c, 0x5a),
        ft(0x815, 0x8d),
        ft(0x87e, 0x810),
        ft(0x8a8, 0x8b7),
        ft(0x8c3, 0xee3),
        ft(0x8fa, 0x1dc),
        ft(0x9ef, 0x331),
        ft(0xa36, 0x2c5),
        ft(0xa3d, 0xbf),
        ft(0xabe, 0x3322),
        ft(0xb38, 0x529),
        ft(0xb75, 0x265a),
        ft(0xb7e, 0xbc3),
        ft(0xb9b, 0x44e),
        ft(0xbbc, 0x4229),
        ft(0xbbf, 0x529),
        ft(0xbfe, 0x2da7),
        ft(0xc2e, 0x3181),
        ft(0xcb9, 0xf3),
        ft(0xd08, 0xfa),
        ft(0xdc8, 0x11a),
        ft(0xdd7, 0x32d),
        ft(0xdf8, 0xdfb),
        ft(0xdfe, 0x531),
        ft(0xe01, 0xdf3),
        ft(0xedf, 0x205a),
        ft(0xee9, 0x222e),
        ft(0xeee, 0x2e9a),
        ft(0xf39, 0x367),
        ft(0x10d0, 0x140),
        ft(0x1104, 0x2d0),
        ft(0x11a0, 0x1ec),
        ft(0x1279, 0x21),
        ft(0x1424, 0x15e),
        ft(0x1470, 0x14e),
        ft(0x151f, 0xd9b),
        ft(0x1523, 0x390),
        ft(0x1532, 0x19f),
        ft(0x1580, 0x210),
        ft(0x1583, 0x10d),
        ft(0x15a3, 0x3caf),
        ft(0x1630, 0x222e),
        ft(0x166a, 0x19b),
        ft(0x16c8, 0x136),
        ft(0x1700, 0x29f8),
        ft(0x1718, 0x194),
        ft(0x1727, 0xf3f),
        ft(0x177a, 0x178),
        ft(0x1809, 0x17b6),
        ft(0x1816, 0x18f3),
        ft(0x188a, 0x436),
        ft(0x1979, 0x1d01),
        ft(0x1a74, 0x2bb0),
        ft(0x1a8a, 0x1f8),
        ft(0x1b5a, 0x1fa),
        ft(0x1b86, 0x1515),
        ft(0x1d64, 0x2c9b),
        ft(0x2038, 0x37b1),
        ft(0x203d, 0x20dd),
        ft(0x2042, 0x2e00),
        ft(0x205a, 0x30b),
        ft(0x20e3, 0x274),
        ft(0x20ee, 0x263),
        ft(0x20f2, 0x22d),
        ft(0x20f9, 0x256),
        ft(0x210f, 0x21eb),
        ft(0x2135, 0x27d),
        ft(0x2160, 0x913),
        ft(0x2199, 0x121),
        ft(0x21ce, 0x1561),
        ft(0x21e6, 0x504),
        ft(0x21f4, 0x49f),
        ft(0x21fb, 0x269),
        ft(0x222d, 0x121),
        ft(0x2237, 0x121),
        ft(0x2248, 0x217d),
        ft(0x2262, 0x92a),
        ft(0x2316, 0x3226),
        ft(0x236a, 0x2835),
        ft(0x2382, 0x3365),
        ft(0x2472, 0x2c7),
        ft(0x24e4, 0x2ff),
        ft(0x24f0, 0x2fa),
        ft(0x24fa, 0x31f),
        ft(0x2550, 0xb5b),
        ft(0x25a9, 0xe2),
        ft(0x263e, 0x2d0),
        ft(0x26c9, 0x26b4),
        ft(0x26f9, 0x3c8),
        ft(0x2727, 0x3caf),
        ft(0x2755, 0x6a4),
        ft(0x2765, 0x26b4),
        ft(0x2789, 0x4358),
        ft(0x27c9, 0x2001),
        ft(0x28ea, 0x27b1),
        ft(0x28ef, 0x2837),
        ft(0x28fe, 0xaa5),
        ft(0x2914, 0x351),
        ft(0x2986, 0x2da7),
        ft(0x29f0, 0x96b),
        ft(0x2b1a, 0x38d),
        ft(0x2bfc, 0x395),
        ft(0x2c3f, 0x3caf),
        ft(0x2ce1, 0x2201),
        ft(0x2cfc, 0x3be),
        ft(0x2d13, 0x597),
        ft(0x2d47, 0x148),
        ft(0x2d48, 0x148),
        ft(0x2dff, 0x2f1),
        ft(0x2e08, 0x19cc),
        ft(0x2e10, 0xc45),
        ft(0x2e1a, 0x2d95),
        ft(0x2e21, 0x292),
        ft(0x2e54, 0x7d),
        ft(0x2e65, 0x2282),
        ft(0x2e97, 0x1a4),
        ft(0x2ea0, 0x2e9b),
        ft(0x2eef, 0x2ed7),
        ft(0x3193, 0x3c4),
        ft(0x3366, 0x338e),
        ft(0x342a, 0x3dc),
        ft(0x34ee, 0x18d0),
        ft(0x35c8, 0x2c9b),
        ft(0x35e6, 0x412),
        ft(0x35f5, 0x24b),
        ft(0x360d, 0x1dc),
        ft(0x3658, 0x246),
        ft(0x3676, 0x3f4),
        ft(0x36fd, 0x445),
        ft(0x3747, 0x3b42),
        ft(0x37c0, 0x121),
        ft(0x3816, 0x38f2),
        ft(0x382a, 0x2b48),
        ft(0x382b, 0x2c9b),
        ft(0x382f, 0xa9),
        ft(0x3832, 0x3228),
        ft(0x386c, 0x39a6),
        ft(0x3892, 0x3fc0),
        ft(0x38a0, 0x45f),
        ft(0x38a5, 0x39d7),
        ft(0x38b4, 0x1fa4),
        ft(0x38b5, 0x2e9a),
        ft(0x38fa, 0x38f1),
        ft(0x395c, 0x47e),
        ft(0x3b4e, 0xd91),
        ft(0x3b78, 0x137),
        ft(0x3c99, 0x4bc),
        ft(0x3fbd, 0x100),
        ft(0x4208, 0xa91),
        ft(0x42be, 0x573),
        ft(0x42f9, 0x3f60),
        ft(0x4378, 0x25a),
        ft(0x43b8, 0xe6c),
        ft(0x43cd, 0x10f),
        ft(0x43d4, 0x4848),
        ft(0x44af, 0x3322),
        ft(0x44e3, 0x512),
        ft(0x45ca, 0x2409),
        ft(0x45dd, 0x26dc),
        ft(0x4610, 0x48ae),
        ft(0x46ae, 0x46a0),
        ft(0x473e, 0x4745),
        ft(0x4817, 0x3503),
        ft(0x483b, 0x208b),
        ft(0x4916, 0x31f),
        ft(0x49a7, 0x523),
    ];
    pub(super) static ALIAS_TYPES: [i8; 193] = [
        1, 0, 0, 0, 0, 0, 0, 1, 2, 2, 0, 1, 0, 0, 0, 0, 1, 2, 1, 1, 2, 0, 0, 1, 0, 1, 2, 1, 1, 0,
        0, 0, 0, 2, 1, 1, 0, 2, 0, 0, 1, 0, 1, 0, 0, 1, 2, 1, 1, 1, 1, 0, 0, 0, 0, 2, 1, 1, 1, 1,
        2, 1, 0, 1, 1, 2, 2, 0, 0, 1, 2, 0, 1, 0, 1, 1, 1, 1, 0, 0, 2, 1, 0, 0, 0, 0, 0, 1, 1, 1,
        1, 1, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 1, 2, 2, 2, 0, 1, 1, 0, 1, 0, 0, 0, 0,
        0, 0, 0, 0, 1, 0, 0, 1, 1, 0, 0, 1, 0, 2, 1, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 1, 1, 2, 0, 0,
        2, 0, 0, 1, 1, 1, 0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 0, 1, 2, 0, 0, 0, 1, 0,
        1, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 1, 1,
    ];
    pub(super) static SUPPRESS_SCRIPT: [u8; 1330] = [
        0x00, 0x00, 0x00, 0x00, 0x00, 0x20, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x5b, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x2c, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x05, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x0e, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x5b, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x20, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x20, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x0e, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x5b, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x5b, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x5b,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x5b, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x5b, 0x5b, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x5b,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x5b, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xed, 0x00, 0x00, 0x00,
        0x00, 0xef, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x34, 0x00, 0x00, 0x5b, 0x00,
        0x00, 0x5b, 0x00, 0x5b, 0x00, 0x5b, 0x00, 0x00, 0x00, 0x00, 0x5b, 0x00, 0x00, 0x05, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x5b, 0x00, 0x00, 0x00, 0x5b, 0x00, 0x00, 0x5b,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x5b, 0x00, 0x00, 0x5b, 0x5b, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x5b, 0x5b, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x5b, 0x00, 0x00, 0x00, 0x5b,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x5b,
        0x35, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x5b, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x3e, 0x00, 0x22, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x5b, 0x5b, 0x00, 0x5b, 0x5b, 0x00, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x5b, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x5b, 0x00, 0x00, 0x00, 0x00, 0x5b, 0x5b, 0x00, 0x3e, 0x00, 0x00,
        0x00, 0x00, 0x49, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x2e, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x20, 0x00, 0x00, 0x5b, 0x00, 0x00, 0x00,
        0x00, 0x4f, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x53, 0x00, 0x00, 0x54, 0x00, 0x22, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x5b, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x5b, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x5b, 0x00, 0x00, 0x58, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x5b,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x5b, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x22, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x5b, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x5b, 0x00, 0x00, 0x00, 0x00, 0x00, 0x5b, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x20, 0x00, 0x00, 0x00, 0x00, 0x6f, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x5b, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x22, 0x00, 0x00, 0x00, 0x5b, 0x5b, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x76, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x5b, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x5b,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x5b, 0x00, 0x5b, 0x22, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x5b, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x5b, 0x00, 0x00, 0x5b, 0x00, 0x00, 0x00, 0x00, 0x5b, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x7e, 0x5b, 0x00, 0x00, 0x00, 0x5b, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x5b, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x5b, 0x00, 0x00,
        0x00, 0x00, 0x83, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x36, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x5b, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x05, 0x00,
        0x5b, 0x00, 0x00, 0x00, 0x5b, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x5b, 0x00, 0x00, 0x00, 0x00, 0x5b, 0x00, 0x00, 0x5b, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x20, 0x00, 0x00, 0x5b, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x5b, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0xd6, 0x00, 0x00, 0x00, 0x00, 0x00, 0x5b, 0x00, 0x00, 0x00, 0x5b, 0x00, 0x00, 0x00,
        0x00, 0x5b, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x5b, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x5b, 0x00, 0x00, 0x00, 0x00, 0x00, 0x5b,
        0x00, 0x00, 0x00, 0x5b, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x5b, 0x5b, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xe6, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xe9, 0x00, 0x5b, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xee, 0x00, 0x00, 0x00, 0x2c, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x5b, 0x00, 0x00, 0x5b, 0x00, 0x00, 0x00, 0x5b, 0x00, 0x5b, 0x00, 0x5b,
        0x00, 0x00, 0x00, 0x5b, 0x00, 0x00, 0x00, 0x5b, 0x00, 0x00, 0x00, 0x5b, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x5b,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x20, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x05, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x5b, 0x00, 0x00, 0x5b, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x5b, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x3e, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x5b, 0x00, 0x00,
    ];
    pub(super) static ALT_TAG_INDEX: [u8; 8] = [0, 17, 31, 45, 61, 74, 86, 102];
    pub(super) const ALT_TAGS: &str = "xtg-x-cel-gaulishen-GB-oxendicten-x-i-defaultund-x-i-enochiansee-x-i-mingonan-x-zh-minen-US-u-va-posix";
    pub(super) const SCRIPT: &[u8] = b"\
    ----AdlmAfakAghbAhomArabAranArmiArmnAvstBaliBamuBassBatkBengBhks\
    BlisBopoBrahBraiBugiBuhdCakmCansCariChamCherChrsCirtCoptCpmnCprt\
    CyrlCyrsDevaDiakDogrDsrtDuplEgydEgyhEgypElbaElymEthiGeokGeorGlag\
    GongGonmGothGranGrekGujrGuruHanbHangHaniHanoHansHantHatrHebrHira\
    HluwHmngHmnpHrktHungIndsItalJamoJavaJpanJurcKaliKanaKawiKharKhmr\
    KhojKitlKitsKndaKoreKpelKthiLanaLaooLatfLatgLatnLekeLepcLimbLina\
    LinbLisuLomaLyciLydiMahjMakaMandManiMarcMayaMedfMendMercMeroMlym\
    ModiMongMoonMrooMteiMultMymrNagmNandNarbNbatNewaNkdbNkgbNkooNshu\
    OgamOlckOrkhOryaOsgeOsmaOugrPalmPaucPcunPelmPermPhagPhliPhlpPhlv\
    PhnxPiqdPlrdPrtiPsinQaaaQaabQaacQaadQaaeQaafQaagQaahQaaiQaajQaak\
    QaalQaamQaanQaaoQaapQaaqQaarQaasQaatQaauQaavQaawQaaxQaayQaazQaba\
    QabbQabcQabdQabeQabfQabgQabhQabiQabjQabkQablQabmQabnQaboQabpQabq\
    QabrQabsQabtQabuQabvQabwQabxRanjRjngRohgRoroRunrSamrSaraSarbSaur\
    SgnwShawShrdShuiSiddSindSinhSogdSogoSoraSoyoSundSunuSyloSyrcSyre\
    SyrjSyrnTagbTakrTaleTaluTamlTangTavtTeluTengTfngTglgThaaThaiTibt\
    TirhTnsaTotoUgarVaiiVispVithWaraWchoWoleXpeoXsuxYeziYiiiZanbZinh\
    ZmthZsyeZsymZxxxZyyyZzzz\xff\xff\xff\xff";
    pub(super) const REGION_ISO: &[u8] = b"\
    AAAAACSCADNDAEREAFFGAGTGAIIAALLBAMRMANNTAOGOAQTAARRGASSMATUTAUUS\
    AWBWAXLAAZZEBAIHBBRBBDGDBEELBFFABGGRBHHRBIDIBJENBLLMBMMUBNRNBOOL\
    BQESBRRABSHSBTTNBUURBVVTBWWABYLRBZLZCAANCCCKCDODCFAFCGOGCHHECIIV\
    CKOKCLHLCMMRCNHNCOOLCPPTCQ\x20\x20CRRICS\x00\x00CTTECUUBCVPVCWUWCXXRCYYPCZZE\
    DDDRDEEUDGGADJJIDKNKDMMADOOMDYHYDZZAEA\x20\x20ECCUEESTEGGYEHSHERRIESSP\
    ETTHEU\x00\x03EZ\x20\x20FIINFJJIFKLKFMSMFOROFQ\x00\x18FRRAFXXXGAABGBBRGDRDGEEOGFUF\
    GGGYGHHAGIIBGLRLGMMBGNINGPLPGQNQGRRCGS\x00\x06GTTMGUUMGWNBGYUYHKKGHMMD\
    HNNDHRRVHTTIHUUNHVVOIC\x20\x20IDDNIERLILSRIMMNINNDIOOTIQRQIRRNISSLITTA\
    JEEYJMAMJOORJPPNJTTNKEENKGGZKHHMKIIRKM\x00\x09KNNAKP\x00\x0cKRORKWWTKY\x00\x0fKZAZ\
    LAAOLBBNLCCALIIELKKALRBRLSSOLTTULUUXLVVALYBYMAARMCCOMDDAMENEMFAF\
    MGDGMHHLMIIDMKKDMLLIMMMRMNNGMOACMPNPMQTQMRRTMSSRMTLTMUUSMVDVMWWI\
    MXEXMYYSMZOZNAAMNCCLNEERNFFKNGGANHHBNIICNLLDNOORNPPLNQ\x00\x1eNRRUNTTZ\
    NUIUNZZLOMMNPAANPCCIPEERPFYFPGNGPHHLPKAKPLOLPM\x00\x12PNCNPRRIPSSEPTRT\
    PUUSPWLWPYRYPZCZQAATQMMMQNNNQOOOQPPPQQQQQRRRQSSSQTTTQU\x00\x03QVVVQWWW\
    QXXXQYYYQZZZREEURHHOROOURS\x00\x15RUUSRWWASAAUSBLBSCYCSDDNSEWESGGPSHHN\
    SIVNSJJMSKVKSLLESMMRSNENSOOMSRURSSSDSTTPSUUNSVLVSXXMSYYRSZWZTAAA\
    TCCATDCDTF\x00\x18TGGOTHHATJJKTKKLTLLSTMKMTNUNTOONTPMPTRURTTTOTVUVTWWN\
    TZZAUAKRUGGAUK\x20\x20UMMIUN\x20\x20USSAUYRYUZZBVAATVCCTVDDRVEENVGGBVIIRVNNM\
    VUUTWFLFWKAKWSSMXAAAXBBBXCCCXDDDXEEEXFFFXGGGXHHHXIIIXJJJXKKKXLLL\
    XMMMXNNNXOOOXPPPXQQQXRRRXSSSXTTTXUUUXVVVXWWWXXXXXYYYXZZZYDMDYEEM\
    YT\x00\x1bYUUGZAAFZMMBZRARZWWEZZZZ\xff\xff\xff\xff";
    pub(super) const ALT_REGION_ISO3: &[u8] = b"\
    SCGQUUSGSCOMPRKCYMSPMSRBATFMYTATN";
    pub(super) static ALT_REGION_IDS: [u16; 11] = [
        0x0058, 0x0071, 0x0089, 0x00a9, 0x00ab, 0x00ae, 0x00eb, 0x0106, 0x0122, 0x0160, 0x00dd,
    ];
    pub(super) static REGION_OLD_MAP: [FromTo; 20] = [
        ft(0x44, 0xc5),
        ft(0x59, 0xa8),
        ft(0x60, 0x61),
        ft(0x67, 0x3b),
        ft(0x7a, 0x79),
        ft(0x94, 0x37),
        ft(0xa4, 0x134),
        ft(0xc2, 0x134),
        ft(0xd8, 0x140),
        ft(0xdd, 0x2b),
        ft(0xf0, 0x134),
        ft(0xf3, 0xe3),
        ft(0xfd, 0x71),
        ft(0x104, 0x165),
        ft(0x12b, 0x127),
        ft(0x133, 0x7c),
        ft(0x13b, 0x13f),
        ft(0x142, 0x134),
        ft(0x15e, 0x15f),
        ft(0x164, 0x4b),
    ];
    pub(super) static M49: [i16; 359] = [
        0, 1, 2, 3, 5, 9, 11, 13, 14, 15, 17, 18, 19, 21, 29, 30, 34, 35, 39, 53, 54, 57, 61, 142,
        143, 145, 150, 151, 154, 155, 202, 419, 958, 0, 20, 784, 4, 28, 660, 8, 51, 530, 24, 10,
        32, 16, 40, 36, 533, 248, 31, 70, 52, 50, 56, 854, 100, 48, 108, 204, 652, 60, 96, 68, 535,
        76, 44, 64, 104, 74, 72, 112, 84, 124, 166, 180, 140, 178, 756, 384, 184, 152, 120, 156,
        170, 0, 0, 188, 891, 296, 192, 132, 531, 162, 196, 203, 278, 276, 0, 262, 208, 212, 214,
        204, 12, 0, 218, 233, 818, 732, 232, 724, 231, 967, 0, 246, 242, 238, 583, 234, 0, 250,
        249, 266, 826, 308, 268, 254, 831, 288, 292, 304, 270, 324, 312, 226, 300, 239, 320, 316,
        624, 328, 344, 334, 340, 191, 332, 348, 854, 0, 360, 372, 376, 833, 356, 86, 368, 364, 352,
        380, 832, 388, 400, 392, 581, 404, 417, 116, 296, 174, 659, 408, 410, 414, 136, 398, 418,
        422, 662, 438, 144, 430, 426, 440, 442, 428, 434, 504, 492, 498, 499, 663, 450, 584, 581,
        807, 466, 104, 496, 446, 580, 474, 478, 500, 470, 480, 462, 454, 484, 458, 508, 516, 540,
        562, 574, 566, 548, 558, 528, 578, 524, 10, 520, 536, 570, 554, 512, 591, 0, 604, 258, 598,
        608, 586, 616, 666, 612, 630, 275, 620, 581, 585, 600, 591, 634, 959, 960, 961, 962, 963,
        964, 965, 966, 967, 968, 969, 970, 971, 972, 638, 716, 642, 688, 643, 646, 682, 90, 690,
        729, 752, 702, 654, 705, 744, 703, 694, 674, 686, 706, 740, 728, 678, 810, 222, 534, 760,
        748, 0, 796, 148, 260, 768, 764, 762, 772, 626, 795, 788, 776, 626, 792, 780, 798, 158,
        834, 804, 800, 826, 581, 0, 840, 858, 860, 336, 670, 704, 862, 92, 850, 704, 548, 876, 581,
        882, 973, 974, 975, 976, 977, 978, 979, 980, 981, 982, 983, 984, 985, 986, 987, 988, 989,
        990, 991, 992, 993, 994, 995, 996, 997, 998, 720, 887, 175, 891, 710, 894, 180, 716, 999,
    ];
    pub(super) static M49_INDEX: [i16; 9] = [0, 59, 108, 143, 181, 220, 259, 291, 333];
    pub(super) static FROM_M49: [u16; 333] = [
        0x0201, 0x0402, 0x0603, 0x0824, 0x0a04, 0x1027, 0x1205, 0x142b, 0x1606, 0x1868, 0x1a07,
        0x1c08, 0x1e09, 0x202d, 0x220a, 0x240b, 0x260c, 0x2822, 0x2a0d, 0x302a, 0x3825, 0x3a0e,
        0x3c0f, 0x3e32, 0x402c, 0x4410, 0x4611, 0x482f, 0x4e12, 0x502e, 0x5842, 0x6039, 0x6435,
        0x6628, 0x6834, 0x6a13, 0x6c14, 0x7036, 0x7215, 0x783d, 0x7a16, 0x8043, 0x883f, 0x8c33,
        0x9046, 0x9445, 0x9841, 0xa848, 0xac9b, 0xb50a, 0xb93d, 0xc03e, 0xc838, 0xd0c5, 0xd83a,
        0xe047, 0xe8a7, 0xf052, 0xf849, 0x085b, 0x10ae, 0x184c, 0x1c17, 0x1e18, 0x20b4, 0x2219,
        0x2921, 0x2c1a, 0x2e1b, 0x3051, 0x341c, 0x361d, 0x3853, 0x3d2f, 0x445d, 0x4c4a, 0x5454,
        0x5ca9, 0x5f60, 0x644d, 0x684b, 0x7050, 0x7857, 0x7e91, 0x805a, 0x885e, 0x941e, 0x965f,
        0x983b, 0xa064, 0xa865, 0xac66, 0xb46a, 0xbd1b, 0xc487, 0xcc70, 0xce70, 0xd06e, 0xd26b,
        0xd477, 0xdc75, 0xde89, 0xe474, 0xec73, 0xf031, 0xf27a, 0xf479, 0xfc7f, 0x04e6, 0x0922,
        0x0c63, 0x147b, 0x187e, 0x1c84, 0x26ee, 0x2861, 0x2c60, 0x3061, 0x4081, 0x4882, 0x50a8,
        0x5888, 0x6083, 0x687d, 0x7086, 0x788b, 0x808a, 0x8885, 0x908d, 0x9892, 0x9c8f, 0xa139,
        0xa890, 0xb08e, 0xb893, 0xc09e, 0xc89a, 0xd096, 0xd89d, 0xe09c, 0xe897, 0xf098, 0xf89f,
        0x004f, 0x08a1, 0x10a3, 0x1caf, 0x20a2, 0x28a5, 0x30ab, 0x34ac, 0x3cad, 0x42a6, 0x44b0,
        0x461f, 0x4cb1, 0x54b6, 0x58b9, 0x5cb5, 0x64ba, 0x6cb3, 0x70b7, 0x74b8, 0x7cc7, 0x84c0,
        0x8ccf, 0x94d1, 0x9cce, 0xa4c4, 0xaccc, 0xb4c9, 0xbcca, 0xc0cd, 0xc8d0, 0xd8bc, 0xe0c6,
        0xe4bd, 0xe6be, 0xe8cb, 0xf0bb, 0xf8d2, 0x00e2, 0x08d3, 0x10de, 0x18dc, 0x20da, 0x2429,
        0x265c, 0x2a30, 0x2d1c, 0x2e40, 0x30df, 0x38d4, 0x4940, 0x54e1, 0x5cd9, 0x64d5, 0x6cd7,
        0x74e0, 0x7cd6, 0x84db, 0x88c8, 0x8b34, 0x8e76, 0x90c1, 0x92f1, 0x94e9, 0x9ee3, 0xace7,
        0xb0f2, 0xb8e5, 0xc0e8, 0xc8ec, 0xd0ea, 0xd8ef, 0xe08c, 0xe527, 0xeced, 0xf4f4, 0xfd03,
        0x0505, 0x0707, 0x0d08, 0x183c, 0x1d0f, 0x26aa, 0x2826, 0x2cb2, 0x2ebf, 0x34eb, 0x3d3a,
        0x4514, 0x4d19, 0x5509, 0x5d15, 0x6106, 0x650b, 0x6d13, 0x7d0e, 0x7f12, 0x813f, 0x8310,
        0x8516, 0x8d62, 0x9965, 0xa15e, 0xa86f, 0xb118, 0xb30c, 0xb86d, 0xc10c, 0xc917, 0xd111,
        0xd91e, 0xe10d, 0xe84e, 0xf11d, 0xf525, 0xf924, 0x0123, 0x0926, 0x112a, 0x192d, 0x2023,
        0x2929, 0x312c, 0x3728, 0x3920, 0x3d2e, 0x4132, 0x4931, 0x4ec3, 0x551a, 0x646c, 0x747c,
        0x7e80, 0x80a0, 0x8299, 0x8530, 0x9136, 0xa53e, 0xac37, 0xb537, 0xb938, 0xbd3c, 0xd941,
        0xe543, 0xed5f, 0xef5f, 0xf658, 0xfd63, 0x7c20, 0x7ef5, 0x80f6, 0x82f7, 0x84f8, 0x86f9,
        0x88fa, 0x8afb, 0x8cfc, 0x8e71, 0x90fe, 0x92ff, 0x9500, 0x9701, 0x9902, 0x9b44, 0x9d45,
        0x9f46, 0xa147, 0xa348, 0xa549, 0xa74a, 0xa94b, 0xab4c, 0xad4d, 0xaf4e, 0xb14f, 0xb350,
        0xb551, 0xb752, 0xb953, 0xbb54, 0xbd55, 0xbf56, 0xc157, 0xc358, 0xc559, 0xc75a, 0xc95b,
        0xcb5c, 0xcd5d, 0xcf66,
    ];
    /// Go `variantIndex` (a map), sorted by key.
    pub(super) static VARIANT_INDEX: [(&str, u8); 112] = [
        ("1606nict", 0x0),
        ("1694acad", 0x1),
        ("1901", 0x2),
        ("1959acad", 0x3),
        ("1994", 0x67),
        ("1996", 0x4),
        ("abl1943", 0x5),
        ("akuapem", 0x6),
        ("alalc97", 0x69),
        ("aluku", 0x7),
        ("ao1990", 0x8),
        ("aranes", 0x9),
        ("arevela", 0xa),
        ("arevmda", 0xb),
        ("arkaika", 0xc),
        ("asante", 0xd),
        ("auvern", 0xe),
        ("baku1926", 0xf),
        ("balanka", 0x10),
        ("barla", 0x11),
        ("basiceng", 0x12),
        ("bauddha", 0x13),
        ("bciav", 0x14),
        ("bcizbl", 0x15),
        ("biscayan", 0x16),
        ("biske", 0x62),
        ("bohoric", 0x17),
        ("boont", 0x18),
        ("bornholm", 0x19),
        ("cisaup", 0x1a),
        ("colb1945", 0x1b),
        ("cornu", 0x1c),
        ("creiss", 0x1d),
        ("dajnko", 0x1e),
        ("ekavsk", 0x1f),
        ("emodeng", 0x20),
        ("fonipa", 0x6a),
        ("fonkirsh", 0x6b),
        ("fonnapa", 0x6c),
        ("fonupa", 0x6d),
        ("fonxsamp", 0x6e),
        ("gallo", 0x21),
        ("gascon", 0x22),
        ("grclass", 0x23),
        ("grital", 0x24),
        ("grmistr", 0x25),
        ("hepburn", 0x26),
        ("heploc", 0x68),
        ("hognorsk", 0x27),
        ("hsistemo", 0x28),
        ("ijekavsk", 0x29),
        ("itihasa", 0x2a),
        ("ivanchov", 0x2b),
        ("jauer", 0x2c),
        ("jyutping", 0x2d),
        ("kkcor", 0x2e),
        ("kociewie", 0x2f),
        ("kscor", 0x30),
        ("laukika", 0x31),
        ("lemosin", 0x32),
        ("lengadoc", 0x33),
        ("lipaw", 0x63),
        ("ltg1929", 0x34),
        ("ltg2007", 0x35),
        ("luna1918", 0x36),
        ("metelko", 0x37),
        ("monoton", 0x38),
        ("ndyuka", 0x39),
        ("nedis", 0x3a),
        ("newfound", 0x3b),
        ("nicard", 0x3c),
        ("njiva", 0x64),
        ("nulik", 0x3d),
        ("osojs", 0x65),
        ("oxendict", 0x3e),
        ("pahawh2", 0x3f),
        ("pahawh3", 0x40),
        ("pahawh4", 0x41),
        ("pamaka", 0x42),
        ("peano", 0x43),
        ("petr1708", 0x44),
        ("pinyin", 0x45),
        ("polyton", 0x46),
        ("provenc", 0x47),
        ("puter", 0x48),
        ("rigik", 0x49),
        ("rozaj", 0x4a),
        ("rumgr", 0x4b),
        ("scotland", 0x4c),
        ("scouse", 0x4d),
        ("simple", 0x6f),
        ("solba", 0x66),
        ("sotav", 0x4e),
        ("spanglis", 0x4f),
        ("surmiran", 0x50),
        ("sursilv", 0x51),
        ("sutsilv", 0x52),
        ("synnejyl", 0x53),
        ("tarask", 0x54),
        ("tongyong", 0x55),
        ("tunumiit", 0x56),
        ("uccor", 0x57),
        ("ucrcor", 0x58),
        ("ulster", 0x59),
        ("unifon", 0x5a),
        ("vaidika", 0x5b),
        ("valencia", 0x5c),
        ("vallader", 0x5d),
        ("vecdruka", 0x5e),
        ("vivaraup", 0x5f),
        ("wadegile", 0x60),
        ("xsistemo", 0x61),
    ];
    pub(super) static LIKELY_SCRIPT: [LikelyLangRegion; 263] = [
        llr(0x0, 0x0),
        llr(0x14e, 0x85),
        llr(0x0, 0x0),
        llr(0x2a2, 0x107),
        llr(0x1f, 0x9a),
        llr(0x3a, 0x6c),
        llr(0x0, 0x0),
        llr(0x3b, 0x9d),
        llr(0x1d7, 0x28),
        llr(0x13, 0x9d),
        llr(0x5b, 0x96),
        llr(0x60, 0x52),
        llr(0xb9, 0xb5),
        llr(0x63, 0x96),
        llr(0xa5, 0x35),
        llr(0x3e9, 0x9a),
        llr(0x0, 0x0),
        llr(0x529, 0x12f),
        llr(0x3b1, 0x9a),
        llr(0x15e, 0x79),
        llr(0xc2, 0x96),
        llr(0x9d, 0xe8),
        llr(0xdb, 0x35),
        llr(0xf3, 0x49),
        llr(0x4f0, 0x12c),
        llr(0xe7, 0x13f),
        llr(0xe5, 0x136),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
        llr(0xf1, 0x6c),
        llr(0x0, 0x0),
        llr(0x1a0, 0x5e),
        llr(0x3e2, 0x107),
        llr(0x0, 0x0),
        llr(0x1be, 0x9a),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
        llr(0x15e, 0x79),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
        llr(0x133, 0x6c),
        llr(0x431, 0x27),
        llr(0x0, 0x0),
        llr(0x27, 0x70),
        llr(0x0, 0x0),
        llr(0x210, 0x7e),
        llr(0xfe, 0x38),
        llr(0x0, 0x0),
        llr(0x19b, 0x9a),
        llr(0x19e, 0x131),
        llr(0x3e9, 0x9a),
        llr(0x136, 0x88),
        llr(0x1a4, 0x9a),
        llr(0x39d, 0x9a),
        llr(0x529, 0x12f),
        llr(0x254, 0xac),
        llr(0x529, 0x53),
        llr(0x1cb, 0xe8),
        llr(0x529, 0x53),
        llr(0x529, 0x12f),
        llr(0x2fd, 0x9c),
        llr(0x1bc, 0x98),
        llr(0x200, 0xa3),
        llr(0x1c5, 0x12c),
        llr(0x1ca, 0xb0),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
        llr(0x1d5, 0x93),
        llr(0x0, 0x0),
        llr(0x142, 0x9f),
        llr(0x254, 0xac),
        llr(0x20e, 0x96),
        llr(0x200, 0xa3),
        llr(0x0, 0x0),
        llr(0x135, 0xc5),
        llr(0x200, 0xa3),
        llr(0x0, 0x0),
        llr(0x3bb, 0xe9),
        llr(0x24a, 0xa7),
        llr(0x3fa, 0x9a),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
        llr(0x251, 0x9a),
        llr(0x254, 0xac),
        llr(0x0, 0x0),
        llr(0x88, 0x9a),
        llr(0x370, 0x124),
        llr(0x2b8, 0xb0),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
        llr(0x29f, 0x9a),
        llr(0x2a8, 0x9a),
        llr(0x28f, 0x88),
        llr(0x1a0, 0x88),
        llr(0x2ac, 0x53),
        llr(0x0, 0x0),
        llr(0x4f4, 0x12c),
        llr(0x4f5, 0x12c),
        llr(0x1be, 0x9a),
        llr(0x0, 0x0),
        llr(0x337, 0x9d),
        llr(0x4f7, 0x53),
        llr(0xa9, 0x53),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
        llr(0x2e8, 0x113),
        llr(0x4f8, 0x10c),
        llr(0x4f8, 0x10c),
        llr(0x304, 0x9a),
        llr(0x31b, 0x9a),
        llr(0x30b, 0x53),
        llr(0x0, 0x0),
        llr(0x31e, 0x35),
        llr(0x30e, 0x9a),
        llr(0x414, 0xe9),
        llr(0x331, 0xc5),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
        llr(0x4f9, 0x109),
        llr(0x3b, 0xa2),
        llr(0x353, 0xdc),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
        llr(0x2d0, 0x85),
        llr(0x52a, 0x53),
        llr(0x403, 0x97),
        llr(0x3ee, 0x9a),
        llr(0x39b, 0xc6),
        llr(0x395, 0x9a),
        llr(0x399, 0x136),
        llr(0x429, 0x116),
        llr(0x0, 0x0),
        llr(0x3b, 0x11d),
        llr(0xfd, 0xc5),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
        llr(0x27d, 0x107),
        llr(0x2c9, 0x53),
        llr(0x39f, 0x9d),
        llr(0x39f, 0x53),
        llr(0x0, 0x0),
        llr(0x3ad, 0xb1),
        llr(0x0, 0x0),
        llr(0x1c6, 0x53),
        llr(0x4fd, 0x9d),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
        llr(0x3cb, 0x96),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
        llr(0x372, 0x10d),
        llr(0x420, 0x98),
        llr(0x0, 0x0),
        llr(0x4ff, 0x15f),
        llr(0x3f0, 0x9a),
        llr(0x45, 0x136),
        llr(0x139, 0x7c),
        llr(0x3e9, 0x9a),
        llr(0x0, 0x0),
        llr(0x3e9, 0x9a),
        llr(0x3fa, 0x9a),
        llr(0x40c, 0xb4),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
        llr(0x433, 0x9a),
        llr(0xef, 0xc6),
        llr(0x43e, 0x96),
        llr(0x0, 0x0),
        llr(0x44d, 0x35),
        llr(0x44e, 0x9c),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
        llr(0x45a, 0xe8),
        llr(0x11a, 0x9a),
        llr(0x45e, 0x53),
        llr(0x232, 0x53),
        llr(0x450, 0x9a),
        llr(0x4a5, 0x53),
        llr(0x9f, 0x13f),
        llr(0x461, 0x9a),
        llr(0x0, 0x0),
        llr(0x528, 0xbb),
        llr(0x153, 0xe8),
        llr(0x128, 0xce),
        llr(0x46b, 0x124),
        llr(0xa9, 0x53),
        llr(0x2ce, 0x9a),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
        llr(0x4ad, 0x11d),
        llr(0x4be, 0xb5),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
        llr(0x1ce, 0x9a),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
        llr(0x3a9, 0x9d),
        llr(0x22, 0x9c),
        llr(0x0, 0x0),
        llr(0x1ea, 0x53),
        llr(0xef, 0xc6),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
        llr(0x0, 0x0),
    ];
    pub(super) static LIKELY_LANG: [LikelyScriptRegion; 1330] = [
        lsr(0x136, 0x5b, 0x0),
        lsr(0x70, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x7e, 0x20, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x20, 0x0),
        lsr(0x81, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x96, 0x5b, 0x0),
        lsr(0x132, 0x5b, 0x0),
        lsr(0x81, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x107, 0x20, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x9d, 0x9, 0x0),
        lsr(0x129, 0x5, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x162, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x52, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x9a, 0x4, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x81, 0x5b, 0x0),
        lsr(0x9c, 0xfb, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x14e, 0x5b, 0x0),
        lsr(0x107, 0x20, 0x0),
        lsr(0x70, 0x2c, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xd7, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x0, 0x0, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x96, 0x5b, 0x0),
        lsr(0x166, 0x5, 0x0),
        lsr(0x123, 0x5, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x6c, 0x5, 0x0),
        lsr(0x0, 0x3, 0x1),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x51, 0x5b, 0x0),
        lsr(0x3f, 0x5b, 0x0),
        lsr(0x68, 0x5, 0x0),
        lsr(0x0, 0x0, 0x0),
        lsr(0xbb, 0x5, 0x0),
        lsr(0x6c, 0x5, 0x0),
        lsr(0x9a, 0xe, 0x0),
        lsr(0x130, 0x5b, 0x0),
        lsr(0x136, 0xd0, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x6f, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x49, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x107, 0x20, 0x0),
        lsr(0x166, 0x5, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x9a, 0x22, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x3f, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x3, 0x5, 0x1),
        lsr(0x107, 0x20, 0x0),
        lsr(0xe9, 0x5, 0x0),
        lsr(0x96, 0x5b, 0x0),
        lsr(0xdc, 0x22, 0x0),
        lsr(0x2e, 0x5b, 0x0),
        lsr(0x52, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x52, 0xb, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x96, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x52, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x4f, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x2c, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x47, 0x20, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x10c, 0x5, 0x0),
        lsr(0x163, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x96, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x130, 0x5b, 0x0),
        lsr(0x52, 0x5b, 0x0),
        lsr(0x9a, 0xe6, 0x0),
        lsr(0xe9, 0x5, 0x0),
        lsr(0x9a, 0x22, 0x0),
        lsr(0x38, 0x20, 0x0),
        lsr(0x9a, 0x22, 0x0),
        lsr(0xe9, 0x5, 0x0),
        lsr(0x12c, 0x34, 0x0),
        lsr(0x0, 0x0, 0x0),
        lsr(0x9a, 0x22, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x9a, 0x22, 0x0),
        lsr(0xe8, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x9a, 0x22, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x140, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xe8, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xd7, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x2c, 0x0),
        lsr(0x9a, 0x22, 0x0),
        lsr(0x96, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x115, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x52, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xe8, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x13f, 0xe8, 0x0),
        lsr(0xc4, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xc4, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x35, 0xe, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x53, 0xef, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x9a, 0xe, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x9d, 0x5, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x4f, 0x5b, 0x0),
        lsr(0x79, 0x5b, 0x0),
        lsr(0x9a, 0x22, 0x0),
        lsr(0xe9, 0x5, 0x0),
        lsr(0x9a, 0x22, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x33, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xb5, 0xc, 0x0),
        lsr(0x52, 0x5b, 0x0),
        lsr(0x166, 0x2c, 0x0),
        lsr(0xe8, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xe9, 0x22, 0x0),
        lsr(0x107, 0x20, 0x0),
        lsr(0x160, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x96, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x52, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x87, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x6e, 0x2c, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x52, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xc4, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x6f, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xd7, 0x5b, 0x0),
        lsr(0x35, 0x16, 0x0),
        lsr(0x107, 0x20, 0x0),
        lsr(0xe8, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x132, 0x5b, 0x0),
        lsr(0x8b, 0x5b, 0x0),
        lsr(0x76, 0x5b, 0x0),
        lsr(0x107, 0x20, 0x0),
        lsr(0x136, 0x5b, 0x0),
        lsr(0x49, 0x5b, 0x0),
        lsr(0x136, 0x1a, 0x0),
        lsr(0xa7, 0x5, 0x0),
        lsr(0x13f, 0x19, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x9c, 0x5, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xc6, 0xda, 0x0),
        lsr(0x79, 0x5b, 0x0),
        lsr(0x6c, 0x1d, 0x0),
        lsr(0xe8, 0x5b, 0x0),
        lsr(0x49, 0x17, 0x0),
        lsr(0x131, 0x20, 0x0),
        lsr(0x49, 0x17, 0x0),
        lsr(0x49, 0x17, 0x0),
        lsr(0x49, 0x17, 0x0),
        lsr(0x49, 0x17, 0x0),
        lsr(0x10b, 0x5b, 0x0),
        lsr(0x5f, 0x5b, 0x0),
        lsr(0xea, 0x5b, 0x0),
        lsr(0x49, 0x17, 0x0),
        lsr(0xc5, 0x88, 0x0),
        lsr(0x8, 0x2, 0x1),
        lsr(0x107, 0x20, 0x0),
        lsr(0x7c, 0x5b, 0x0),
        lsr(0x64, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x136, 0x5b, 0x0),
        lsr(0x107, 0x20, 0x0),
        lsr(0xa5, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x9a, 0x5, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x61, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x49, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5, 0x0),
        lsr(0x49, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xd5, 0x5b, 0x0),
        lsr(0x4f, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x9a, 0x5, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x2c, 0x0),
        lsr(0x61, 0x5b, 0x0),
        lsr(0xc4, 0x5b, 0x0),
        lsr(0xd1, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xdc, 0x22, 0x0),
        lsr(0x52, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xce, 0xed, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x115, 0x5b, 0x0),
        lsr(0x37, 0x5b, 0x0),
        lsr(0x43, 0xef, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xa5, 0x5b, 0x0),
        lsr(0x81, 0x5b, 0x0),
        lsr(0xd7, 0x5b, 0x0),
        lsr(0x9f, 0x5b, 0x0),
        lsr(0x6c, 0x29, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xc5, 0x4b, 0x0),
        lsr(0x88, 0x34, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xa, 0x2, 0x1),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x1, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x6f, 0x5b, 0x0),
        lsr(0x136, 0x5b, 0x0),
        lsr(0x6b, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x9f, 0x46, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x6f, 0x5b, 0x0),
        lsr(0x52, 0x5b, 0x0),
        lsr(0x6f, 0x5b, 0x0),
        lsr(0x9d, 0x5, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x87, 0x5b, 0x0),
        lsr(0xc, 0x2, 0x1),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xc4, 0x5b, 0x0),
        lsr(0x73, 0x5b, 0x0),
        lsr(0x10c, 0x5, 0x0),
        lsr(0xe8, 0x5b, 0x0),
        lsr(0x10d, 0x5b, 0x0),
        lsr(0x74, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x77, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x3b, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x79, 0x5b, 0x0),
        lsr(0x136, 0x5b, 0x0),
        lsr(0x79, 0x5b, 0x0),
        lsr(0x61, 0x5b, 0x0),
        lsr(0x61, 0x5b, 0x0),
        lsr(0x52, 0x5, 0x0),
        lsr(0x141, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x85, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xd5, 0x5b, 0x0),
        lsr(0x9f, 0x5b, 0x0),
        lsr(0xd7, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x10c, 0x5b, 0x0),
        lsr(0xda, 0x5b, 0x0),
        lsr(0x97, 0x5b, 0x0),
        lsr(0x81, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xbd, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x53, 0x3b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x96, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x9a, 0x22, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x9d, 0x5, 0x0),
        lsr(0x7f, 0x5b, 0x0),
        lsr(0x7c, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x70, 0x2c, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xdc, 0x22, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xa8, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xe9, 0x5, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xe9, 0x5, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x6f, 0x5b, 0x0),
        lsr(0x9d, 0x5, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x2c, 0x0),
        lsr(0xf2, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x2c, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x9a, 0x22, 0x0),
        lsr(0x9a, 0xe9, 0x0),
        lsr(0x96, 0x5b, 0x0),
        lsr(0xda, 0x5b, 0x0),
        lsr(0x131, 0x32, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xe, 0x2, 0x1),
        lsr(0x9a, 0xe, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x4e, 0x5b, 0x0),
        lsr(0x9a, 0x35, 0x0),
        lsr(0x41, 0x5b, 0x0),
        lsr(0x54, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x81, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xa5, 0x5b, 0x0),
        lsr(0x99, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xdc, 0x22, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5, 0x0),
        lsr(0x49, 0x5b, 0x0),
        lsr(0x166, 0x5, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x10, 0x3, 0x1),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x53, 0x3b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x136, 0x5b, 0x0),
        lsr(0x24, 0x5, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x2c, 0x0),
        lsr(0x98, 0x3e, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x9a, 0x22, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x74, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xe8, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x12c, 0x40, 0x0),
        lsr(0x53, 0x92, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xe9, 0x5, 0x0),
        lsr(0x9a, 0x22, 0x0),
        lsr(0xb0, 0x41, 0x0),
        lsr(0xe8, 0x5b, 0x0),
        lsr(0xe9, 0x5, 0x0),
        lsr(0xe7, 0x5b, 0x0),
        lsr(0x9a, 0x22, 0x0),
        lsr(0x9a, 0x22, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x91, 0x5b, 0x0),
        lsr(0x61, 0x5b, 0x0),
        lsr(0x53, 0x3b, 0x0),
        lsr(0x92, 0x5b, 0x0),
        lsr(0x93, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x28, 0x8, 0x0),
        lsr(0xd3, 0x5b, 0x0),
        lsr(0x79, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xd1, 0x5b, 0x0),
        lsr(0xd7, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x96, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x0, 0x0, 0x0),
        lsr(0x123, 0x5b, 0x0),
        lsr(0xd7, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x53, 0xfd, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x136, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x49, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xe8, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x96, 0x5b, 0x0),
        lsr(0x107, 0x20, 0x0),
        lsr(0x1, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x9e, 0x5b, 0x0),
        lsr(0x9f, 0x5b, 0x0),
        lsr(0x49, 0x17, 0x0),
        lsr(0x98, 0x3e, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x107, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xa3, 0x49, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xa1, 0x5b, 0x0),
        lsr(0x1, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x52, 0x5b, 0x0),
        lsr(0x131, 0x3e, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x130, 0x5b, 0x0),
        lsr(0xdc, 0x22, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x64, 0x5b, 0x0),
        lsr(0x96, 0x5b, 0x0),
        lsr(0x96, 0x5b, 0x0),
        lsr(0x7e, 0x2e, 0x0),
        lsr(0x138, 0x20, 0x0),
        lsr(0x68, 0x5b, 0x0),
        lsr(0xc5, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xd7, 0x5b, 0x0),
        lsr(0xa5, 0x5b, 0x0),
        lsr(0xc4, 0x5b, 0x0),
        lsr(0x107, 0x20, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xd5, 0x5, 0x0),
        lsr(0xd7, 0x5b, 0x0),
        lsr(0x165, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x130, 0x5b, 0x0),
        lsr(0x123, 0x5, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x124, 0xee, 0x0),
        lsr(0x5b, 0x5b, 0x0),
        lsr(0x52, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x4f, 0x5b, 0x0),
        lsr(0x9a, 0x22, 0x0),
        lsr(0x9a, 0x22, 0x0),
        lsr(0x4b, 0x5b, 0x0),
        lsr(0x96, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x41, 0x5b, 0x0),
        lsr(0x9a, 0x5b, 0x0),
        lsr(0x53, 0xe5, 0x0),
        lsr(0x9a, 0x22, 0x0),
        lsr(0xc4, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x9a, 0x76, 0x0),
        lsr(0xe9, 0x5, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xa5, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x12c, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xd3, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xb0, 0x58, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x13, 0x6, 0x1),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x52, 0x5b, 0x0),
        lsr(0x83, 0x5b, 0x0),
        lsr(0xa5, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xa7, 0x4f, 0x0),
        lsr(0x2a, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x9a, 0x53, 0x0),
        lsr(0x8c, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xac, 0x54, 0x0),
        lsr(0x107, 0x20, 0x0),
        lsr(0x9a, 0x22, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x76, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xb5, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x2c, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x107, 0x20, 0x0),
        lsr(0x113, 0x5b, 0x0),
        lsr(0xe8, 0x5b, 0x0),
        lsr(0x107, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x9a, 0x22, 0x0),
        lsr(0x9a, 0x5, 0x0),
        lsr(0x130, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x52, 0x5b, 0x0),
        lsr(0x61, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x2c, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x19, 0x3, 0x1),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x107, 0x20, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x107, 0x20, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x96, 0x5b, 0x0),
        lsr(0xe9, 0x5, 0x0),
        lsr(0x7c, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x2c, 0x0),
        lsr(0x124, 0xee, 0x0),
        lsr(0xe9, 0x5, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x1c, 0x5, 0x1),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x139, 0x5b, 0x0),
        lsr(0x88, 0x5f, 0x0),
        lsr(0x98, 0x3e, 0x0),
        lsr(0x130, 0x5b, 0x0),
        lsr(0xe9, 0x5, 0x0),
        lsr(0x132, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xb8, 0x5b, 0x0),
        lsr(0x107, 0x20, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x96, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x53, 0xee, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x9a, 0x5d, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x107, 0x20, 0x0),
        lsr(0x132, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xda, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x21, 0x2, 0x1),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x9f, 0x5b, 0x0),
        lsr(0x53, 0x61, 0x0),
        lsr(0x96, 0x5b, 0x0),
        lsr(0x9d, 0x5, 0x0),
        lsr(0x136, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x9a, 0xe9, 0x0),
        lsr(0x9f, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x4b, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xb0, 0x58, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x4b, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x163, 0x5b, 0x0),
        lsr(0x9d, 0x5, 0x0),
        lsr(0xb7, 0x5b, 0x0),
        lsr(0xb9, 0x5b, 0x0),
        lsr(0x4b, 0x5b, 0x0),
        lsr(0x4b, 0x5b, 0x0),
        lsr(0xa5, 0x5b, 0x0),
        lsr(0xa5, 0x5b, 0x0),
        lsr(0x9d, 0x5, 0x0),
        lsr(0xb9, 0x5b, 0x0),
        lsr(0x124, 0xee, 0x0),
        lsr(0x53, 0x3b, 0x0),
        lsr(0x12c, 0x5b, 0x0),
        lsr(0x96, 0x5b, 0x0),
        lsr(0x52, 0x5b, 0x0),
        lsr(0x9a, 0x22, 0x0),
        lsr(0x9a, 0x22, 0x0),
        lsr(0x96, 0x5b, 0x0),
        lsr(0x23, 0x3, 0x1),
        lsr(0xa5, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xd0, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5, 0x0),
        lsr(0x107, 0x20, 0x0),
        lsr(0xe8, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x96, 0x5b, 0x0),
        lsr(0x166, 0x2c, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x113, 0x5b, 0x0),
        lsr(0xa5, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x124, 0x5, 0x0),
        lsr(0xcd, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xc0, 0x5b, 0x0),
        lsr(0xd2, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x52, 0x5b, 0x0),
        lsr(0xdc, 0x22, 0x0),
        lsr(0x130, 0x5b, 0x0),
        lsr(0xc1, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xe1, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x96, 0x5b, 0x0),
        lsr(0x9c, 0x3d, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xc3, 0x20, 0x0),
        lsr(0x166, 0x5, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x9a, 0x6f, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x10c, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x26, 0x3, 0x1),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x9a, 0xe, 0x0),
        lsr(0xc5, 0x76, 0x0),
        lsr(0x0, 0x0, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x49, 0x5b, 0x0),
        lsr(0x49, 0x5b, 0x0),
        lsr(0x37, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x9a, 0x22, 0x0),
        lsr(0xdc, 0x22, 0x0),
        lsr(0x107, 0x20, 0x0),
        lsr(0x35, 0x73, 0x0),
        lsr(0x29, 0x3, 0x1),
        lsr(0xcc, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x9a, 0x22, 0x0),
        lsr(0x52, 0x5b, 0x0),
        lsr(0x0, 0x0, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x136, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xe9, 0x5, 0x0),
        lsr(0xc4, 0x5b, 0x0),
        lsr(0x9a, 0x22, 0x0),
        lsr(0x96, 0x5b, 0x0),
        lsr(0x165, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xc5, 0x76, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x2c, 0x0),
        lsr(0x107, 0x20, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x132, 0x5b, 0x0),
        lsr(0x9d, 0x67, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x9d, 0x5, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xde, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x0, 0x0, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x53, 0x3b, 0x0),
        lsr(0x9f, 0x5b, 0x0),
        lsr(0xd3, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xdb, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xd0, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x165, 0x5b, 0x0),
        lsr(0xd2, 0x5b, 0x0),
        lsr(0x61, 0x5b, 0x0),
        lsr(0xdc, 0x22, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xdc, 0x22, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xd3, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xd2, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xd0, 0x5b, 0x0),
        lsr(0xd0, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x96, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xe0, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x9a, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xda, 0x5b, 0x0),
        lsr(0x52, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xdb, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x52, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xdb, 0x5b, 0x0),
        lsr(0x124, 0x57, 0x0),
        lsr(0x9a, 0x22, 0x0),
        lsr(0x10d, 0xcb, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x85, 0x7e, 0x0),
        lsr(0x162, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x49, 0x17, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x162, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x118, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x136, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x53, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xcf, 0x5b, 0x0),
        lsr(0x130, 0x5b, 0x0),
        lsr(0x132, 0x5b, 0x0),
        lsr(0x81, 0x5b, 0x0),
        lsr(0x79, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x0, 0x0, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x70, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x9a, 0x83, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5, 0x0),
        lsr(0x7e, 0x20, 0x0),
        lsr(0x136, 0x84, 0x0),
        lsr(0x166, 0x5, 0x0),
        lsr(0xc6, 0x82, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x2c, 0x3, 0x1),
        lsr(0xe8, 0x5b, 0x0),
        lsr(0x2f, 0x2, 0x1),
        lsr(0xe8, 0x5b, 0x0),
        lsr(0x30, 0x5b, 0x0),
        lsr(0xf1, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x79, 0x5b, 0x0),
        lsr(0xd7, 0x5b, 0x0),
        lsr(0x136, 0x5b, 0x0),
        lsr(0x49, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x9d, 0xfa, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x61, 0x5b, 0x0),
        lsr(0x166, 0x5, 0x0),
        lsr(0xb1, 0x90, 0x0),
        lsr(0x0, 0x0, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x9a, 0x12, 0x0),
        lsr(0xa5, 0x5b, 0x0),
        lsr(0xea, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x9f, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x88, 0x34, 0x0),
        lsr(0x76, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xe9, 0x4e, 0x0),
        lsr(0x9d, 0x5, 0x0),
        lsr(0x1, 0x5b, 0x0),
        lsr(0x24, 0x5, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x41, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x7b, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xe5, 0x5b, 0x0),
        lsr(0x8a, 0x5b, 0x0),
        lsr(0x6a, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x9a, 0x22, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x103, 0x5b, 0x0),
        lsr(0x96, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x9f, 0x5b, 0x0),
        lsr(0x166, 0x5, 0x0),
        lsr(0x9a, 0x5b, 0x0),
        lsr(0x31, 0x2, 0x1),
        lsr(0xdc, 0x22, 0x0),
        lsr(0x35, 0xe, 0x0),
        lsr(0x4e, 0x5b, 0x0),
        lsr(0x73, 0x5b, 0x0),
        lsr(0x4e, 0x5b, 0x0),
        lsr(0x9d, 0x5, 0x0),
        lsr(0x10d, 0x5b, 0x0),
        lsr(0x3a, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xd2, 0x5b, 0x0),
        lsr(0x105, 0x5b, 0x0),
        lsr(0x96, 0x5b, 0x0),
        lsr(0x130, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x74, 0x5b, 0x0),
        lsr(0x107, 0x20, 0x0),
        lsr(0x131, 0x20, 0x0),
        lsr(0x10a, 0x5b, 0x0),
        lsr(0x108, 0x5b, 0x0),
        lsr(0x130, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xa3, 0x4c, 0x0),
        lsr(0x9a, 0x22, 0x0),
        lsr(0x81, 0x5b, 0x0),
        lsr(0x107, 0x20, 0x0),
        lsr(0xa5, 0x5b, 0x0),
        lsr(0x96, 0x5b, 0x0),
        lsr(0x9a, 0x5b, 0x0),
        lsr(0x115, 0x5b, 0x0),
        lsr(0x9a, 0xcf, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x130, 0x5b, 0x0),
        lsr(0x9f, 0x5b, 0x0),
        lsr(0x9a, 0x22, 0x0),
        lsr(0x166, 0x5, 0x0),
        lsr(0x9f, 0x5b, 0x0),
        lsr(0x7c, 0x5b, 0x0),
        lsr(0x49, 0x5b, 0x0),
        lsr(0x33, 0x4, 0x1),
        lsr(0x9f, 0x5b, 0x0),
        lsr(0x9d, 0x5, 0x0),
        lsr(0xdb, 0x5b, 0x0),
        lsr(0x4f, 0x5b, 0x0),
        lsr(0xd2, 0x5b, 0x0),
        lsr(0xd0, 0x5b, 0x0),
        lsr(0xc4, 0x5b, 0x0),
        lsr(0x4c, 0x5b, 0x0),
        lsr(0x97, 0x80, 0x0),
        lsr(0xb7, 0x5b, 0x0),
        lsr(0x166, 0x2c, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x0, 0x0, 0x0),
        lsr(0xbb, 0xeb, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xc5, 0x76, 0x0),
        lsr(0x166, 0x5, 0x0),
        lsr(0xb4, 0xd6, 0x0),
        lsr(0x70, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x112, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xe9, 0x5, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x110, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xea, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x96, 0x5b, 0x0),
        lsr(0x143, 0x5b, 0x0),
        lsr(0x10d, 0x5b, 0x0),
        lsr(0x0, 0x0, 0x0),
        lsr(0x10d, 0x5b, 0x0),
        lsr(0x73, 0x5b, 0x0),
        lsr(0x98, 0xcc, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x73, 0x5b, 0x0),
        lsr(0x165, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xc4, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x116, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x124, 0xee, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x27, 0x5b, 0x0),
        lsr(0x37, 0x5, 0x1),
        lsr(0x9a, 0xd9, 0x0),
        lsr(0x117, 0x5b, 0x0),
        lsr(0x115, 0x5b, 0x0),
        lsr(0x9a, 0x22, 0x0),
        lsr(0x162, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x6e, 0x5b, 0x0),
        lsr(0x162, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x61, 0x5b, 0x0),
        lsr(0x96, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x130, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x85, 0x5b, 0x0),
        lsr(0x10d, 0x5b, 0x0),
        lsr(0x130, 0x5b, 0x0),
        lsr(0x160, 0x5, 0x0),
        lsr(0x4b, 0x5b, 0x0),
        lsr(0x61, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x9a, 0x22, 0x0),
        lsr(0x96, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x35, 0xe, 0x0),
        lsr(0x9c, 0xde, 0x0),
        lsr(0xea, 0x5b, 0x0),
        lsr(0x9a, 0xe6, 0x0),
        lsr(0xdc, 0x22, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xe8, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x9a, 0x53, 0x0),
        lsr(0x53, 0xe4, 0x0),
        lsr(0xdc, 0x22, 0x0),
        lsr(0xdc, 0x22, 0x0),
        lsr(0x9a, 0xe9, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x113, 0x5b, 0x0),
        lsr(0x132, 0x5b, 0x0),
        lsr(0x127, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x3c, 0x3, 0x1),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x124, 0xee, 0x0),
        lsr(0xdc, 0x22, 0x0),
        lsr(0xdc, 0x22, 0x0),
        lsr(0xdc, 0x22, 0x0),
        lsr(0x70, 0x2c, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x6e, 0x2c, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xd7, 0x5b, 0x0),
        lsr(0x128, 0x5b, 0x0),
        lsr(0x126, 0x5b, 0x0),
        lsr(0x32, 0x5b, 0x0),
        lsr(0xdc, 0x22, 0x0),
        lsr(0xe8, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x32, 0x5b, 0x0),
        lsr(0xd5, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x162, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x12a, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xcf, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xe7, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x12c, 0x5b, 0x0),
        lsr(0x12c, 0x5b, 0x0),
        lsr(0x12f, 0x5b, 0x0),
        lsr(0x166, 0x5, 0x0),
        lsr(0x162, 0x5b, 0x0),
        lsr(0x88, 0x34, 0x0),
        lsr(0xdc, 0x22, 0x0),
        lsr(0xe8, 0x5b, 0x0),
        lsr(0x43, 0xef, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x107, 0x20, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x132, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x124, 0xee, 0x0),
        lsr(0x32, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xcf, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x12e, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x0, 0x0, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xd5, 0x5b, 0x0),
        lsr(0x53, 0xe7, 0x0),
        lsr(0xe6, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x107, 0x20, 0x0),
        lsr(0xbb, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x107, 0x20, 0x0),
        lsr(0x3f, 0x4, 0x1),
        lsr(0x11d, 0xf3, 0x0),
        lsr(0x131, 0x20, 0x0),
        lsr(0x76, 0x5b, 0x0),
        lsr(0x2a, 0x5b, 0x0),
        lsr(0x0, 0x0, 0x0),
        lsr(0x43, 0x3, 0x1),
        lsr(0x9a, 0xe, 0x0),
        lsr(0xe9, 0x5, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x46, 0x4, 0x1),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xb5, 0xf4, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x162, 0x5b, 0x0),
        lsr(0x9f, 0x5b, 0x0),
        lsr(0x107, 0x5b, 0x0),
        lsr(0x13f, 0x5b, 0x0),
        lsr(0x11c, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x36, 0x5b, 0x0),
        lsr(0x61, 0x5b, 0x0),
        lsr(0xd2, 0x5b, 0x0),
        lsr(0x1, 0x5b, 0x0),
        lsr(0x107, 0x5b, 0x0),
        lsr(0x6b, 0x5b, 0x0),
        lsr(0x130, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x36, 0x5b, 0x0),
        lsr(0x4e, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x70, 0x2c, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xe8, 0x5b, 0x0),
        lsr(0x2f, 0x5b, 0x0),
        lsr(0x9a, 0xe9, 0x0),
        lsr(0x9a, 0x22, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x141, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xa9, 0x5, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x115, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x9a, 0x22, 0x0),
        lsr(0x53, 0x3b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x41, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x12c, 0x18, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x162, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x12c, 0x63, 0x0),
        lsr(0x12c, 0x64, 0x0),
        lsr(0x7e, 0x2e, 0x0),
        lsr(0x53, 0x68, 0x0),
        lsr(0x10c, 0x6d, 0x0),
        lsr(0x109, 0x79, 0x0),
        lsr(0x9a, 0x22, 0x0),
        lsr(0x132, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x9d, 0x93, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x15f, 0xce, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xdc, 0x22, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xd2, 0x5b, 0x0),
        lsr(0x76, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x52, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x52, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x1, 0x3e, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xd7, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x41, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0xd0, 0x5b, 0x0),
        lsr(0x4a, 0x3, 0x1),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x53, 0x5b, 0x0),
        lsr(0x10c, 0x5b, 0x0),
        lsr(0x0, 0x0, 0x0),
        lsr(0xa9, 0x5, 0x0),
        lsr(0xda, 0x5b, 0x0),
        lsr(0xbb, 0xeb, 0x0),
        lsr(0x4d, 0x14, 0x1),
        lsr(0x53, 0x7f, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x123, 0x5b, 0x0),
        lsr(0xd1, 0x5b, 0x0),
        lsr(0x166, 0x5b, 0x0),
        lsr(0x162, 0x5b, 0x0),
        lsr(0x0, 0x0, 0x0),
        lsr(0x12c, 0x5b, 0x0),
    ];
    pub(super) static LIKELY_LANG_LIST: [LikelyScriptRegion; 97] = [
        lsr(0x9d, 0x7, 0x0),
        lsr(0xa2, 0x7a, 0x2),
        lsr(0x11d, 0x87, 0x2),
        lsr(0x32, 0x5b, 0x0),
        lsr(0x9c, 0x5, 0x4),
        lsr(0x9d, 0x5, 0x4),
        lsr(0x107, 0x20, 0x4),
        lsr(0x9d, 0x5, 0x2),
        lsr(0x107, 0x20, 0x0),
        lsr(0x38, 0x2f, 0x2),
        lsr(0x136, 0x5b, 0x0),
        lsr(0x7c, 0xd1, 0x2),
        lsr(0x115, 0x5b, 0x0),
        lsr(0x85, 0x1, 0x2),
        lsr(0x5e, 0x1f, 0x0),
        lsr(0x88, 0x60, 0x2),
        lsr(0xd7, 0x5b, 0x0),
        lsr(0x52, 0x5, 0x4),
        lsr(0x10c, 0x5, 0x4),
        lsr(0xaf, 0x20, 0x0),
        lsr(0x24, 0x5, 0x4),
        lsr(0x53, 0x5, 0x4),
        lsr(0x9d, 0x5, 0x4),
        lsr(0xc6, 0x5, 0x4),
        lsr(0x53, 0x5, 0x2),
        lsr(0x12c, 0x5b, 0x0),
        lsr(0xb1, 0x5, 0x4),
        lsr(0x9c, 0x5, 0x2),
        lsr(0xa6, 0x20, 0x0),
        lsr(0x53, 0x5, 0x4),
        lsr(0x12c, 0x5b, 0x4),
        lsr(0x53, 0x5, 0x2),
        lsr(0x12c, 0x5b, 0x2),
        lsr(0xdc, 0x22, 0x0),
        lsr(0x9a, 0x5e, 0x2),
        lsr(0x84, 0x5b, 0x0),
        lsr(0x85, 0x7e, 0x4),
        lsr(0x85, 0x7e, 0x2),
        lsr(0xc6, 0x20, 0x0),
        lsr(0x53, 0x71, 0x4),
        lsr(0x53, 0x71, 0x2),
        lsr(0xd1, 0x5b, 0x0),
        lsr(0x4a, 0x5, 0x4),
        lsr(0x96, 0x5, 0x4),
        lsr(0x9a, 0x36, 0x0),
        lsr(0xe9, 0x5, 0x4),
        lsr(0xe9, 0x5, 0x2),
        lsr(0x9d, 0x8d, 0x0),
        lsr(0x53, 0x8e, 0x2),
        lsr(0xbb, 0xeb, 0x0),
        lsr(0xda, 0x5b, 0x4),
        lsr(0xe9, 0x5, 0x0),
        lsr(0x9a, 0x22, 0x2),
        lsr(0x9a, 0x50, 0x2),
        lsr(0x9a, 0xd5, 0x2),
        lsr(0x106, 0x20, 0x0),
        lsr(0xbe, 0x5b, 0x4),
        lsr(0x105, 0x5b, 0x4),
        lsr(0x107, 0x5b, 0x4),
        lsr(0x12c, 0x5b, 0x4),
        lsr(0x125, 0x20, 0x0),
        lsr(0xe9, 0x5, 0x4),
        lsr(0xe9, 0x5, 0x2),
        lsr(0x53, 0x5, 0x0),
        lsr(0xaf, 0x20, 0x4),
        lsr(0xc6, 0x20, 0x4),
        lsr(0xaf, 0x20, 0x2),
        lsr(0x9a, 0xe, 0x0),
        lsr(0xdc, 0x22, 0x4),
        lsr(0xdc, 0x22, 0x2),
        lsr(0x138, 0x5b, 0x0),
        lsr(0x24, 0x5, 0x4),
        lsr(0x53, 0x20, 0x4),
        lsr(0x24, 0x5, 0x2),
        lsr(0x8e, 0x3c, 0x0),
        lsr(0x53, 0x3b, 0x4),
        lsr(0x53, 0x3b, 0x2),
        lsr(0x53, 0x3b, 0x0),
        lsr(0x2f, 0x3c, 0x4),
        lsr(0x3e, 0x3c, 0x4),
        lsr(0x7c, 0x3c, 0x4),
        lsr(0x7f, 0x3c, 0x4),
        lsr(0x8e, 0x3c, 0x4),
        lsr(0x96, 0x3c, 0x4),
        lsr(0xc7, 0x3c, 0x4),
        lsr(0xd1, 0x3c, 0x4),
        lsr(0xe3, 0x3c, 0x4),
        lsr(0xe6, 0x3c, 0x4),
        lsr(0xe8, 0x3c, 0x4),
        lsr(0x117, 0x3c, 0x4),
        lsr(0x124, 0x3c, 0x4),
        lsr(0x12f, 0x3c, 0x4),
        lsr(0x136, 0x3c, 0x4),
        lsr(0x13f, 0x3c, 0x4),
        lsr(0x12f, 0x11, 0x2),
        lsr(0x12f, 0x37, 0x2),
        lsr(0x12f, 0x3c, 0x2),
    ];
    pub(super) static LIKELY_REGION: [LikelyLangScript; 359] = [
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0xd7, 0x5b, 0x0),
        lls(0x3a, 0x5, 0x0),
        lls(0x0, 0x2, 0x1),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x2, 0x2, 0x1),
        lls(0x4, 0x2, 0x1),
        lls(0x0, 0x0, 0x0),
        lls(0x3c0, 0x5b, 0x0),
        lls(0x0, 0x5b, 0x0),
        lls(0x13e, 0x5b, 0x0),
        lls(0x41b, 0x5b, 0x0),
        lls(0x10d, 0x5b, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x367, 0x5b, 0x0),
        lls(0x444, 0x5b, 0x0),
        lls(0x58, 0x5b, 0x0),
        lls(0x6, 0x2, 0x1),
        lls(0x0, 0x0, 0x0),
        lls(0xa5, 0xe, 0x0),
        lls(0x367, 0x5b, 0x0),
        lls(0x15e, 0x5b, 0x0),
        lls(0x7e, 0x20, 0x0),
        lls(0x3a, 0x5, 0x0),
        lls(0x3d9, 0x5b, 0x0),
        lls(0x15e, 0x5b, 0x0),
        lls(0x15e, 0x5b, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x31f, 0x5b, 0x0),
        lls(0x13e, 0x5b, 0x0),
        lls(0x3a1, 0x5b, 0x0),
        lls(0x3c0, 0x5b, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x8, 0x2, 0x1),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x5b, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x71, 0x20, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x512, 0x3e, 0x2),
        lls(0x31f, 0x5, 0x2),
        lls(0x445, 0x5b, 0x0),
        lls(0x15e, 0x5b, 0x0),
        lls(0x15e, 0x5b, 0x0),
        lls(0x10d, 0x5b, 0x0),
        lls(0x15e, 0x5b, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x13e, 0x5b, 0x0),
        lls(0x15e, 0x5b, 0x0),
        lls(0xa, 0x4, 0x1),
        lls(0x13e, 0x5b, 0x0),
        lls(0x0, 0x5b, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x13e, 0x5b, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x13e, 0x5b, 0x0),
        lls(0x3c0, 0x5b, 0x0),
        lls(0x3a1, 0x5b, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0xe, 0x2, 0x1),
        lls(0xfa, 0x5b, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x10d, 0x5b, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x1, 0x5b, 0x0),
        lls(0x101, 0x5b, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x13e, 0x5b, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x10, 0x2, 0x1),
        lls(0x13e, 0x5b, 0x0),
        lls(0x13e, 0x5b, 0x0),
        lls(0x140, 0x5b, 0x0),
        lls(0x3a, 0x5, 0x0),
        lls(0x3a, 0x5, 0x0),
        lls(0x46f, 0x2c, 0x0),
        lls(0x13e, 0x5b, 0x0),
        lls(0x12, 0x2, 0x1),
        lls(0x0, 0x0, 0x0),
        lls(0x10d, 0x5b, 0x0),
        lls(0x151, 0x5b, 0x0),
        lls(0x1c0, 0x22, 0x2),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x158, 0x5b, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x15e, 0x5b, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x15e, 0x5b, 0x0),
        lls(0x14, 0x2, 0x1),
        lls(0x0, 0x0, 0x0),
        lls(0x16, 0x3, 0x1),
        lls(0x15e, 0x5b, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x21, 0x5b, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x245, 0x5b, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x15e, 0x5b, 0x0),
        lls(0x15e, 0x5b, 0x0),
        lls(0x13e, 0x5b, 0x0),
        lls(0x19, 0x2, 0x1),
        lls(0x0, 0x5b, 0x0),
        lls(0x13e, 0x5b, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x3c0, 0x5b, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x529, 0x3c, 0x0),
        lls(0x0, 0x5b, 0x0),
        lls(0x13e, 0x5b, 0x0),
        lls(0x1d1, 0x5b, 0x0),
        lls(0x1d4, 0x5b, 0x0),
        lls(0x1d5, 0x5b, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x13e, 0x5b, 0x0),
        lls(0x1b, 0x2, 0x1),
        lls(0x0, 0x0, 0x0),
        lls(0x1bc, 0x3e, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x1d, 0x3, 0x1),
        lls(0x0, 0x0, 0x0),
        lls(0x3a, 0x5, 0x0),
        lls(0x20, 0x2, 0x1),
        lls(0x1f8, 0x5b, 0x0),
        lls(0x1f9, 0x5b, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x3a, 0x5, 0x0),
        lls(0x200, 0x49, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x445, 0x5b, 0x0),
        lls(0x28a, 0x20, 0x0),
        lls(0x22, 0x3, 0x1),
        lls(0x0, 0x0, 0x0),
        lls(0x25, 0x2, 0x1),
        lls(0x0, 0x0, 0x0),
        lls(0x254, 0x54, 0x0),
        lls(0x254, 0x54, 0x0),
        lls(0x3a, 0x5, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x3e2, 0x20, 0x0),
        lls(0x27, 0x2, 0x1),
        lls(0x3a, 0x5, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x10d, 0x5b, 0x0),
        lls(0x40c, 0xd6, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x43b, 0x5b, 0x0),
        lls(0x2c0, 0x5b, 0x0),
        lls(0x15e, 0x5b, 0x0),
        lls(0x2c7, 0x5b, 0x0),
        lls(0x3a, 0x5, 0x0),
        lls(0x29, 0x2, 0x1),
        lls(0x15e, 0x5b, 0x0),
        lls(0x2b, 0x2, 0x1),
        lls(0x432, 0x5b, 0x0),
        lls(0x15e, 0x5b, 0x0),
        lls(0x2f1, 0x5b, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x2d, 0x2, 0x1),
        lls(0xa0, 0x5b, 0x0),
        lls(0x2f, 0x2, 0x1),
        lls(0x31, 0x2, 0x1),
        lls(0x33, 0x2, 0x1),
        lls(0x0, 0x0, 0x0),
        lls(0x15e, 0x5b, 0x0),
        lls(0x35, 0x2, 0x1),
        lls(0x0, 0x0, 0x0),
        lls(0x320, 0x5b, 0x0),
        lls(0x37, 0x3, 0x1),
        lls(0x128, 0xed, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x13e, 0x5b, 0x0),
        lls(0x31f, 0x5b, 0x0),
        lls(0x3c0, 0x5b, 0x0),
        lls(0x16, 0x5b, 0x0),
        lls(0x15e, 0x5b, 0x0),
        lls(0x1b4, 0x5b, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x1b4, 0x5, 0x2),
        lls(0x0, 0x0, 0x0),
        lls(0x13e, 0x5b, 0x0),
        lls(0x367, 0x5b, 0x0),
        lls(0x347, 0x5b, 0x0),
        lls(0x351, 0x22, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x3a, 0x5, 0x0),
        lls(0x13e, 0x5b, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x13e, 0x5b, 0x0),
        lls(0x15e, 0x5b, 0x0),
        lls(0x486, 0x5b, 0x0),
        lls(0x153, 0x5b, 0x0),
        lls(0x3a, 0x3, 0x1),
        lls(0x3b3, 0x5b, 0x0),
        lls(0x15e, 0x5b, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x13e, 0x5b, 0x0),
        lls(0x3a, 0x5, 0x0),
        lls(0x3c0, 0x5b, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x3a2, 0x5b, 0x0),
        lls(0x194, 0x5b, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x3a, 0x5, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x15e, 0x5b, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x3d, 0x2, 0x1),
        lls(0x432, 0x20, 0x0),
        lls(0x3f, 0x2, 0x1),
        lls(0x3e5, 0x5b, 0x0),
        lls(0x3a, 0x5, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x15e, 0x5b, 0x0),
        lls(0x3a, 0x5, 0x0),
        lls(0x41, 0x2, 0x1),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x416, 0x5b, 0x0),
        lls(0x347, 0x5b, 0x0),
        lls(0x43, 0x2, 0x1),
        lls(0x0, 0x0, 0x0),
        lls(0x1f9, 0x5b, 0x0),
        lls(0x15e, 0x5b, 0x0),
        lls(0x429, 0x5b, 0x0),
        lls(0x367, 0x5b, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x3c0, 0x5b, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x13e, 0x5b, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x45, 0x2, 0x1),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x15e, 0x5b, 0x0),
        lls(0x15e, 0x5b, 0x0),
        lls(0x47, 0x2, 0x1),
        lls(0x49, 0x3, 0x1),
        lls(0x4c, 0x2, 0x1),
        lls(0x477, 0x5b, 0x0),
        lls(0x3c0, 0x5b, 0x0),
        lls(0x476, 0x5b, 0x0),
        lls(0x4e, 0x2, 0x1),
        lls(0x482, 0x5b, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x50, 0x4, 0x1),
        lls(0x0, 0x0, 0x0),
        lls(0x4a0, 0x5b, 0x0),
        lls(0x54, 0x2, 0x1),
        lls(0x445, 0x5b, 0x0),
        lls(0x56, 0x3, 0x1),
        lls(0x445, 0x5b, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x512, 0x3e, 0x2),
        lls(0x13e, 0x5b, 0x0),
        lls(0x4bc, 0x5b, 0x0),
        lls(0x1f9, 0x5b, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x13e, 0x5b, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x4c3, 0x5b, 0x0),
        lls(0x8a, 0x5b, 0x0),
        lls(0x15e, 0x5b, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x41b, 0x5b, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x59, 0x2, 0x1),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x3a, 0x5, 0x0),
        lls(0x5b, 0x2, 0x1),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x0, 0x0, 0x0),
        lls(0x423, 0x5b, 0x0),
        lls(0x0, 0x0, 0x0),
    ];
    pub(super) static LIKELY_REGION_LIST: [LikelyLangScript; 93] = [
        lls(0x148, 0x5, 0x0),
        lls(0x476, 0x5b, 0x0),
        lls(0x431, 0x5b, 0x0),
        lls(0x2ff, 0x20, 0x0),
        lls(0x1d7, 0x8, 0x0),
        lls(0x274, 0x5b, 0x0),
        lls(0xb7, 0x5b, 0x0),
        lls(0x432, 0x20, 0x0),
        lls(0x12d, 0xef, 0x0),
        lls(0x351, 0x22, 0x0),
        lls(0x529, 0x3b, 0x0),
        lls(0x4ac, 0x5, 0x0),
        lls(0x523, 0x5b, 0x0),
        lls(0x29a, 0xee, 0x0),
        lls(0x136, 0x34, 0x0),
        lls(0x48a, 0x5b, 0x0),
        lls(0x3a, 0x5, 0x0),
        lls(0x15e, 0x5b, 0x0),
        lls(0x27, 0x2c, 0x0),
        lls(0x139, 0x5b, 0x0),
        lls(0x26a, 0x5, 0x2),
        lls(0x512, 0x3e, 0x2),
        lls(0x210, 0x2e, 0x0),
        lls(0x5, 0x20, 0x0),
        lls(0x274, 0x5b, 0x0),
        lls(0x136, 0x34, 0x0),
        lls(0x2ff, 0x20, 0x0),
        lls(0x1e1, 0x5b, 0x0),
        lls(0x31f, 0x5, 0x0),
        lls(0x1be, 0x22, 0x0),
        lls(0x4b4, 0x5, 0x0),
        lls(0x236, 0x76, 0x0),
        lls(0x148, 0x5, 0x0),
        lls(0x476, 0x5b, 0x0),
        lls(0x24a, 0x4f, 0x0),
        lls(0xe6, 0x5, 0x0),
        lls(0x226, 0xee, 0x0),
        lls(0x3a, 0x5, 0x0),
        lls(0x15e, 0x5b, 0x0),
        lls(0x2b8, 0x58, 0x0),
        lls(0x226, 0xee, 0x0),
        lls(0x3a, 0x5, 0x0),
        lls(0x15e, 0x5b, 0x0),
        lls(0x3dc, 0x5b, 0x0),
        lls(0x4ae, 0x20, 0x0),
        lls(0x2ff, 0x20, 0x0),
        lls(0x431, 0x5b, 0x0),
        lls(0x331, 0x76, 0x0),
        lls(0x213, 0x5b, 0x0),
        lls(0x30b, 0x20, 0x0),
        lls(0x242, 0x5, 0x0),
        lls(0x529, 0x3c, 0x0),
        lls(0x3c0, 0x5b, 0x0),
        lls(0x3a, 0x5, 0x0),
        lls(0x15e, 0x5b, 0x0),
        lls(0x2ed, 0x5b, 0x0),
        lls(0x4b4, 0x5, 0x0),
        lls(0x88, 0x22, 0x0),
        lls(0x4b4, 0x5, 0x0),
        lls(0x4b4, 0x5, 0x0),
        lls(0xbe, 0x22, 0x0),
        lls(0x3dc, 0x5b, 0x0),
        lls(0x7e, 0x20, 0x0),
        lls(0x3e2, 0x20, 0x0),
        lls(0x267, 0x5b, 0x0),
        lls(0x444, 0x5b, 0x0),
        lls(0x512, 0x3e, 0x0),
        lls(0x412, 0x5b, 0x0),
        lls(0x4ae, 0x20, 0x0),
        lls(0x3a, 0x5, 0x0),
        lls(0x15e, 0x5b, 0x0),
        lls(0x15e, 0x5b, 0x0),
        lls(0x35, 0x5, 0x0),
        lls(0x46b, 0xee, 0x0),
        lls(0x2ec, 0x5, 0x0),
        lls(0x30f, 0x76, 0x0),
        lls(0x467, 0x20, 0x0),
        lls(0x148, 0x5, 0x0),
        lls(0x3a, 0x5, 0x0),
        lls(0x15e, 0x5b, 0x0),
        lls(0x48a, 0x5b, 0x0),
        lls(0x58, 0x5, 0x0),
        lls(0x219, 0x20, 0x0),
        lls(0x81, 0x34, 0x0),
        lls(0x529, 0x3c, 0x0),
        lls(0x48c, 0x5b, 0x0),
        lls(0x4ae, 0x20, 0x0),
        lls(0x512, 0x3e, 0x0),
        lls(0x3b3, 0x5b, 0x0),
        lls(0x431, 0x5b, 0x0),
        lls(0x432, 0x20, 0x0),
        lls(0x15e, 0x5b, 0x0),
        lls(0x446, 0x5, 0x0),
    ];
    pub(super) static LIKELY_REGION_GROUP: [LikelyTag; 33] = [
        lt(0x0, 0x0, 0x0),
        lt(0x139, 0xd7, 0x5b),
        lt(0x139, 0x136, 0x5b),
        lt(0x3c0, 0x41, 0x5b),
        lt(0x139, 0x2f, 0x5b),
        lt(0x139, 0xd7, 0x5b),
        lt(0x13e, 0xd0, 0x5b),
        lt(0x445, 0x130, 0x5b),
        lt(0x3a, 0x6c, 0x5),
        lt(0x445, 0x4b, 0x5b),
        lt(0x139, 0x162, 0x5b),
        lt(0x139, 0x136, 0x5b),
        lt(0x139, 0x136, 0x5b),
        lt(0x13e, 0x5a, 0x5b),
        lt(0x529, 0x53, 0x3b),
        lt(0x1be, 0x9a, 0x22),
        lt(0x1e1, 0x96, 0x5b),
        lt(0x1f9, 0x9f, 0x5b),
        lt(0x139, 0x2f, 0x5b),
        lt(0x139, 0xe7, 0x5b),
        lt(0x139, 0x8b, 0x5b),
        lt(0x41b, 0x143, 0x5b),
        lt(0x529, 0x53, 0x3b),
        lt(0x4bc, 0x138, 0x5b),
        lt(0x3a, 0x109, 0x5),
        lt(0x3e2, 0x107, 0x20),
        lt(0x3e2, 0x107, 0x20),
        lt(0x139, 0x7c, 0x5b),
        lt(0x10d, 0x61, 0x5b),
        lt(0x139, 0xd7, 0x5b),
        lt(0x13e, 0x1f, 0x5b),
        lt(0x139, 0x9b, 0x5b),
        lt(0x139, 0x7c, 0x5b),
    ];
    pub(super) static REGION_CONTAINMENT: [u64; 33] = [
        0x00000001ffffffff,
        0x00000000200007a2,
        0x0000000000003044,
        0x0000000000000008,
        0x00000000803c0010,
        0x0000000000000020,
        0x0000000000000040,
        0x0000000000000080,
        0x0000000000000100,
        0x0000000000000200,
        0x0000000000000400,
        0x000000004000384c,
        0x0000000000001000,
        0x0000000000002000,
        0x0000000000004000,
        0x0000000000008000,
        0x0000000000010000,
        0x0000000000020000,
        0x0000000000040000,
        0x0000000000080000,
        0x0000000000100000,
        0x0000000000200000,
        0x0000000001c1c000,
        0x0000000000800000,
        0x0000000001000000,
        0x000000001e020000,
        0x0000000004000000,
        0x0000000008000000,
        0x0000000010000000,
        0x00000000200006a0,
        0x0000000040002048,
        0x0000000080000000,
        0x0000000100000000,
    ];
    pub(super) static REGION_INCLUSION: [u8; 359] = [
        0x00, 0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d,
        0x0e, 0x0f, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c,
        0x1d, 0x1e, 0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x26, 0x23, 0x24, 0x26, 0x27, 0x22, 0x28,
        0x29, 0x2a, 0x2b, 0x26, 0x2c, 0x24, 0x23, 0x26, 0x25, 0x2a, 0x2d, 0x2e, 0x24, 0x2f, 0x2d,
        0x26, 0x30, 0x31, 0x28, 0x26, 0x28, 0x26, 0x25, 0x31, 0x22, 0x32, 0x33, 0x34, 0x30, 0x22,
        0x27, 0x27, 0x27, 0x35, 0x2d, 0x29, 0x28, 0x27, 0x36, 0x28, 0x22, 0x21, 0x34, 0x23, 0x21,
        0x26, 0x2d, 0x26, 0x22, 0x37, 0x2e, 0x35, 0x2a, 0x22, 0x2f, 0x38, 0x26, 0x26, 0x21, 0x39,
        0x39, 0x28, 0x38, 0x39, 0x39, 0x2f, 0x3a, 0x2f, 0x20, 0x21, 0x38, 0x3b, 0x28, 0x3c, 0x2c,
        0x21, 0x2a, 0x35, 0x27, 0x38, 0x26, 0x24, 0x28, 0x2c, 0x2d, 0x23, 0x30, 0x2d, 0x2d, 0x26,
        0x27, 0x3a, 0x22, 0x34, 0x3c, 0x2d, 0x28, 0x36, 0x22, 0x34, 0x3a, 0x26, 0x2e, 0x21, 0x39,
        0x31, 0x38, 0x24, 0x2c, 0x25, 0x22, 0x24, 0x25, 0x2c, 0x3a, 0x2c, 0x26, 0x24, 0x36, 0x21,
        0x2f, 0x3d, 0x31, 0x3c, 0x2f, 0x26, 0x36, 0x36, 0x24, 0x26, 0x3d, 0x31, 0x24, 0x26, 0x35,
        0x25, 0x2d, 0x32, 0x38, 0x2a, 0x38, 0x39, 0x39, 0x35, 0x33, 0x23, 0x26, 0x2f, 0x3c, 0x21,
        0x23, 0x2d, 0x31, 0x36, 0x36, 0x3c, 0x26, 0x2d, 0x26, 0x3a, 0x2f, 0x25, 0x2f, 0x34, 0x31,
        0x2f, 0x32, 0x3b, 0x2d, 0x2b, 0x2d, 0x21, 0x34, 0x2a, 0x2c, 0x25, 0x21, 0x3c, 0x24, 0x29,
        0x2b, 0x24, 0x34, 0x21, 0x28, 0x29, 0x3b, 0x31, 0x25, 0x2e, 0x30, 0x29, 0x26, 0x24, 0x3a,
        0x21, 0x3c, 0x28, 0x21, 0x24, 0x21, 0x21, 0x1f, 0x21, 0x21, 0x21, 0x21, 0x21, 0x21, 0x21,
        0x21, 0x21, 0x21, 0x21, 0x2f, 0x21, 0x2e, 0x23, 0x33, 0x2f, 0x24, 0x3b, 0x2f, 0x39, 0x38,
        0x31, 0x2d, 0x3a, 0x2c, 0x2e, 0x2d, 0x23, 0x2d, 0x2f, 0x28, 0x2f, 0x27, 0x33, 0x34, 0x26,
        0x24, 0x32, 0x22, 0x26, 0x27, 0x22, 0x2d, 0x31, 0x3d, 0x29, 0x31, 0x3d, 0x39, 0x29, 0x31,
        0x24, 0x26, 0x29, 0x36, 0x2f, 0x33, 0x2f, 0x21, 0x22, 0x21, 0x30, 0x28, 0x3d, 0x23, 0x26,
        0x21, 0x28, 0x26, 0x26, 0x31, 0x3b, 0x29, 0x21, 0x29, 0x21, 0x21, 0x21, 0x21, 0x21, 0x21,
        0x21, 0x21, 0x21, 0x21, 0x23, 0x21, 0x21, 0x21, 0x21, 0x21, 0x21, 0x21, 0x21, 0x21, 0x21,
        0x21, 0x21, 0x21, 0x21, 0x21, 0x24, 0x24, 0x2f, 0x23, 0x32, 0x2f, 0x27, 0x2f, 0x21,
    ];
    pub(super) static REGION_INCLUSION_BITS: [u64; 73] = [
        0x0000000102400813,
        0x00000000200007a3,
        0x0000000000003844,
        0x0000000040000808,
        0x00000000803c0011,
        0x0000000020000022,
        0x0000000040000844,
        0x0000000020000082,
        0x0000000000000102,
        0x0000000020000202,
        0x0000000020000402,
        0x000000004000384d,
        0x0000000000001804,
        0x0000000040002804,
        0x0000000000404000,
        0x0000000000408000,
        0x0000000000410000,
        0x0000000002020000,
        0x0000000000040010,
        0x0000000000080010,
        0x0000000000100010,
        0x0000000000200010,
        0x0000000001c1c001,
        0x0000000000c00000,
        0x0000000001400000,
        0x000000001e020001,
        0x0000000006000000,
        0x000000000a000000,
        0x0000000012000000,
        0x00000000200006a2,
        0x0000000040002848,
        0x0000000080000010,
        0x0000000100000001,
        0x0000000000000001,
        0x0000000080000000,
        0x0000000000020000,
        0x0000000001000000,
        0x0000000000008000,
        0x0000000000002000,
        0x0000000000000200,
        0x0000000000000008,
        0x0000000000200000,
        0x0000000110000000,
        0x0000000000040000,
        0x0000000008000000,
        0x0000000000000020,
        0x0000000104000000,
        0x0000000000000080,
        0x0000000000001000,
        0x0000000000010000,
        0x0000000000000400,
        0x0000000004000000,
        0x0000000000000040,
        0x0000000010000000,
        0x0000000000004000,
        0x0000000101000000,
        0x0000000108000000,
        0x0000000000000100,
        0x0000000100020000,
        0x0000000000080000,
        0x0000000000100000,
        0x0000000000800000,
        0x00000001ffffffff,
        0x0000000122400fb3,
        0x00000001827c0813,
        0x000000014240385f,
        0x0000000103c1c813,
        0x000000011e420813,
        0x0000000112000001,
        0x0000000106000001,
        0x0000000101400001,
        0x000000010a000001,
        0x0000000102020001,
    ];

    // Go: language/tables.go
    pub(super) static REGION_TO_GROUPS: [u8; 359] = [
        0x00, 0x00, 0x00, 0x04, 0x04, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x00, 0x04, 0x04, 0x04,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x04, 0x04, 0x00, 0x00, 0x04, 0x00, 0x00, 0x04,
        0x01, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x04, 0x04, 0x00, 0x04, 0x04, 0x04, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x04, 0x04, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x04, 0x00, 0x00, 0x04, 0x00, 0x00,
        0x04, 0x00, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x04, 0x04, 0x00, 0x08,
        0x00, 0x04, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x04, 0x00, 0x04, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x04,
        0x00, 0x00, 0x00, 0x04, 0x01, 0x00, 0x04, 0x02, 0x00, 0x04, 0x00, 0x04, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x04, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x08, 0x08, 0x00, 0x00, 0x00, 0x04, 0x00, 0x01, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x02, 0x01, 0x04, 0x08, 0x04, 0x00, 0x00, 0x00, 0x00, 0x04, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x04, 0x00, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x04, 0x00, 0x05, 0x00, 0x00,
        0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x04, 0x04,
        0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x08, 0x00, 0x00,
        0x00, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x05, 0x04, 0x00, 0x00, 0x04,
        0x00, 0x04, 0x04, 0x05, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    ];
    pub(super) static MATCH_LANG: [MutualIntelligibility; 113] = [
        mi(0x1d1, 0xb7, 0x4, false),
        mi(0x407, 0xb7, 0x4, false),
        mi(0x407, 0x1d1, 0x4, false),
        mi(0x407, 0x432, 0x4, false),
        mi(0x43a, 0x1, 0x4, false),
        mi(0x1a3, 0x10d, 0x4, true),
        mi(0x295, 0x10d, 0x4, true),
        mi(0x101, 0x36f, 0x8, false),
        mi(0x101, 0x347, 0x8, false),
        mi(0x5, 0x3e2, 0xa, true),
        mi(0xd, 0x139, 0xa, true),
        mi(0x16, 0x367, 0xa, true),
        mi(0x21, 0x139, 0xa, true),
        mi(0x56, 0x13e, 0xa, true),
        mi(0x58, 0x3e2, 0xa, true),
        mi(0x71, 0x3e2, 0xa, true),
        mi(0x75, 0x139, 0xa, true),
        mi(0x82, 0x1be, 0xa, true),
        mi(0xa5, 0x139, 0xa, true),
        mi(0xb2, 0x15e, 0xa, true),
        mi(0xdd, 0x153, 0xa, true),
        mi(0xe5, 0x139, 0xa, true),
        mi(0xe9, 0x3a, 0xa, true),
        mi(0xf0, 0x15e, 0xa, true),
        mi(0xf9, 0x15e, 0xa, true),
        mi(0x100, 0x139, 0xa, true),
        mi(0x130, 0x139, 0xa, true),
        mi(0x13c, 0x139, 0xa, true),
        mi(0x140, 0x151, 0xa, true),
        mi(0x145, 0x13e, 0xa, true),
        mi(0x158, 0x101, 0xa, true),
        mi(0x16d, 0x367, 0xa, true),
        mi(0x16e, 0x139, 0xa, true),
        mi(0x16f, 0x139, 0xa, true),
        mi(0x17e, 0x139, 0xa, true),
        mi(0x190, 0x13e, 0xa, true),
        mi(0x194, 0x13e, 0xa, true),
        mi(0x1a4, 0x1be, 0xa, true),
        mi(0x1b4, 0x139, 0xa, true),
        mi(0x1b8, 0x139, 0xa, true),
        mi(0x1d4, 0x15e, 0xa, true),
        mi(0x1d7, 0x3e2, 0xa, true),
        mi(0x1d9, 0x139, 0xa, true),
        mi(0x1e7, 0x139, 0xa, true),
        mi(0x1f8, 0x139, 0xa, true),
        mi(0x20e, 0x1e1, 0xa, true),
        mi(0x210, 0x139, 0xa, true),
        mi(0x22d, 0x15e, 0xa, true),
        mi(0x242, 0x3e2, 0xa, true),
        mi(0x24a, 0x139, 0xa, true),
        mi(0x251, 0x139, 0xa, true),
        mi(0x265, 0x139, 0xa, true),
        mi(0x274, 0x48a, 0xa, true),
        mi(0x28a, 0x3e2, 0xa, true),
        mi(0x28e, 0x1f9, 0xa, true),
        mi(0x2a3, 0x139, 0xa, true),
        mi(0x2b5, 0x15e, 0xa, true),
        mi(0x2b8, 0x139, 0xa, true),
        mi(0x2be, 0x139, 0xa, true),
        mi(0x2c3, 0x15e, 0xa, true),
        mi(0x2ed, 0x139, 0xa, true),
        mi(0x2f1, 0x15e, 0xa, true),
        mi(0x2fa, 0x139, 0xa, true),
        mi(0x2ff, 0x7e, 0xa, true),
        mi(0x304, 0x139, 0xa, true),
        mi(0x30b, 0x3e2, 0xa, true),
        mi(0x31b, 0x1be, 0xa, true),
        mi(0x31f, 0x1e1, 0xa, true),
        mi(0x320, 0x139, 0xa, true),
        mi(0x331, 0x139, 0xa, true),
        mi(0x351, 0x139, 0xa, true),
        mi(0x36a, 0x347, 0xa, false),
        mi(0x36a, 0x36f, 0xa, true),
        mi(0x37a, 0x139, 0xa, true),
        mi(0x387, 0x139, 0xa, true),
        mi(0x389, 0x139, 0xa, true),
        mi(0x38b, 0x15e, 0xa, true),
        mi(0x390, 0x139, 0xa, true),
        mi(0x395, 0x139, 0xa, true),
        mi(0x39d, 0x139, 0xa, true),
        mi(0x3a5, 0x139, 0xa, true),
        mi(0x3be, 0x139, 0xa, true),
        mi(0x3c4, 0x13e, 0xa, true),
        mi(0x3d4, 0x10d, 0xa, true),
        mi(0x3d9, 0x139, 0xa, true),
        mi(0x3e5, 0x15e, 0xa, true),
        mi(0x3e9, 0x1be, 0xa, true),
        mi(0x3fa, 0x139, 0xa, true),
        mi(0x40c, 0x139, 0xa, true),
        mi(0x423, 0x139, 0xa, true),
        mi(0x429, 0x139, 0xa, true),
        mi(0x431, 0x139, 0xa, true),
        mi(0x43b, 0x139, 0xa, true),
        mi(0x43e, 0x1e1, 0xa, true),
        mi(0x445, 0x139, 0xa, true),
        mi(0x450, 0x139, 0xa, true),
        mi(0x461, 0x139, 0xa, true),
        mi(0x467, 0x3e2, 0xa, true),
        mi(0x46f, 0x139, 0xa, true),
        mi(0x476, 0x3e2, 0xa, true),
        mi(0x3883, 0x139, 0xa, true),
        mi(0x480, 0x139, 0xa, true),
        mi(0x482, 0x139, 0xa, true),
        mi(0x494, 0x3e2, 0xa, true),
        mi(0x49d, 0x139, 0xa, true),
        mi(0x4ac, 0x529, 0xa, true),
        mi(0x4b4, 0x139, 0xa, true),
        mi(0x4bc, 0x3e2, 0xa, true),
        mi(0x4e5, 0x15e, 0xa, true),
        mi(0x4f2, 0x139, 0xa, true),
        mi(0x512, 0x139, 0xa, true),
        mi(0x518, 0x139, 0xa, true),
        mi(0x52f, 0x139, 0xa, true),
    ];
    pub(super) static MATCH_SCRIPT: [ScriptIntelligibility; 26] = [
        si(0x432, 0x432, 0x5b, 0x20, 0x5),
        si(0x432, 0x432, 0x20, 0x5b, 0x5),
        si(0x58, 0x3e2, 0x5b, 0x20, 0xa),
        si(0xa5, 0x139, 0xe, 0x5b, 0xa),
        si(0x1d7, 0x3e2, 0x8, 0x20, 0xa),
        si(0x210, 0x139, 0x2e, 0x5b, 0xa),
        si(0x24a, 0x139, 0x4f, 0x5b, 0xa),
        si(0x251, 0x139, 0x53, 0x5b, 0xa),
        si(0x2b8, 0x139, 0x58, 0x5b, 0xa),
        si(0x304, 0x139, 0x6f, 0x5b, 0xa),
        si(0x331, 0x139, 0x76, 0x5b, 0xa),
        si(0x351, 0x139, 0x22, 0x5b, 0xa),
        si(0x395, 0x139, 0x83, 0x5b, 0xa),
        si(0x39d, 0x139, 0x36, 0x5b, 0xa),
        si(0x3be, 0x139, 0x5, 0x5b, 0xa),
        si(0x3fa, 0x139, 0x5, 0x5b, 0xa),
        si(0x40c, 0x139, 0xd6, 0x5b, 0xa),
        si(0x450, 0x139, 0xe6, 0x5b, 0xa),
        si(0x461, 0x139, 0xe9, 0x5b, 0xa),
        si(0x46f, 0x139, 0x2c, 0x5b, 0xa),
        si(0x476, 0x3e2, 0x5b, 0x20, 0xa),
        si(0x4b4, 0x139, 0x5, 0x5b, 0xa),
        si(0x4bc, 0x3e2, 0x5b, 0x20, 0xa),
        si(0x512, 0x139, 0x3e, 0x5b, 0xa),
        si(0x529, 0x529, 0x3b, 0x3c, 0xf),
        si(0x529, 0x529, 0x3c, 0x3b, 0x13),
    ];
    pub(super) static MATCH_REGION: [RegionIntelligibility; 15] = [
        ri(0x3a, 0x0, 0x4, 0x4),
        ri(0x3a, 0x0, 0x84, 0x4),
        ri(0x139, 0x0, 0x1, 0x4),
        ri(0x139, 0x0, 0x81, 0x4),
        ri(0x13e, 0x0, 0x3, 0x4),
        ri(0x13e, 0x0, 0x83, 0x4),
        ri(0x3c0, 0x0, 0x3, 0x4),
        ri(0x3c0, 0x0, 0x83, 0x4),
        ri(0x529, 0x3c, 0x2, 0x4),
        ri(0x529, 0x3c, 0x82, 0x4),
        ri(0x3a, 0x0, 0x80, 0x5),
        ri(0x139, 0x0, 0x80, 0x5),
        ri(0x13e, 0x0, 0x80, 0x5),
        ri(0x3c0, 0x0, 0x80, 0x5),
        ri(0x529, 0x3c, 0x80, 0x5),
    ];

    // Go: internal/language/tables.go:3477 parentRel
    #[derive(Clone, Copy, Debug)]
    pub(super) struct ParentRel {
        pub(super) lang: u16,
        pub(super) script: u16,
        pub(super) max_script: u16,
        pub(super) to_region: u16,
        pub(super) from_region: &'static [u16],
    }

    // The tables below were printed by
    // target/continuation-r97-goport/complete/gen/collate/gen.sh
    // (zz_dump_test.go in internal/language and internal/language/compact).
    // Go: internal/language/tables.go:1044 _XK
    pub(super) const _XK: u16 = 334;
    // Go: internal/language/tables.go:1054 regionTypes
    pub(super) static REGION_TYPES: [u8; 359] = [
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x05, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06,
        0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06,
        0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x04, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06,
        0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x04, 0x04, 0x06, 0x04, 0x00,
        0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x04, 0x06, 0x04, 0x06, 0x06, 0x06, 0x06, 0x00, 0x06,
        0x04, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x04, 0x06, 0x06, 0x06, 0x06, 0x06,
        0x00, 0x06, 0x04, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06,
        0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x00, 0x04,
        0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x00,
        0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06,
        0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x00,
        0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06,
        0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x00, 0x06, 0x06, 0x06, 0x06, 0x00, 0x06, 0x04, 0x06,
        0x06, 0x06, 0x06, 0x00, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06,
        0x00, 0x06, 0x06, 0x00, 0x06, 0x05, 0x05, 0x05, 0x05, 0x05, 0x05, 0x05, 0x05, 0x05, 0x05,
        0x05, 0x05, 0x05, 0x05, 0x06, 0x00, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06,
        0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06,
        0x06, 0x06, 0x04, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06,
        0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x02, 0x06, 0x04, 0x06, 0x06, 0x06, 0x06, 0x06,
        0x00, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x00, 0x06, 0x05, 0x05, 0x05, 0x05, 0x05, 0x05,
        0x05, 0x05, 0x05, 0x05, 0x05, 0x05, 0x05, 0x05, 0x05, 0x05, 0x05, 0x05, 0x05, 0x05, 0x05,
        0x05, 0x05, 0x05, 0x05, 0x05, 0x04, 0x06, 0x06, 0x04, 0x06, 0x06, 0x04, 0x06, 0x05,
    ];
    // Go: internal/language/tables.go:3486 parents
    pub(super) static PARENTS: [ParentRel; 5] = [
        ParentRel {
            lang: 0x139,
            script: 0x0,
            max_script: 0x5b,
            to_region: 0x1,
            from_region: &[
                0x1a, 0x25, 0x26, 0x2f, 0x34, 0x36, 0x3d, 0x42, 0x46, 0x48, 0x49, 0x4a, 0x50, 0x52,
                0x5d, 0x5e, 0x62, 0x65, 0x6e, 0x74, 0x75, 0x76, 0x7c, 0x7d, 0x80, 0x81, 0x82, 0x84,
                0x8d, 0x8e, 0x97, 0x98, 0x99, 0x9a, 0x9b, 0xa0, 0xa1, 0xa5, 0xa8, 0xaa, 0xae, 0xb2,
                0xb5, 0xb6, 0xc0, 0xc7, 0xcb, 0xcc, 0xcd, 0xcf, 0xd1, 0xd3, 0xd6, 0xd7, 0xde, 0xe0,
                0xe1, 0xe7, 0xe8, 0xe9, 0xec, 0xf1, 0x108, 0x10a, 0x10b, 0x10c, 0x10e, 0x10f,
                0x113, 0x118, 0x11c, 0x11e, 0x120, 0x126, 0x12a, 0x12d, 0x12e, 0x130, 0x132, 0x13a,
                0x13d, 0x140, 0x143, 0x162, 0x163, 0x165,
            ],
        },
        ParentRel {
            lang: 0x139,
            script: 0x0,
            max_script: 0x5b,
            to_region: 0x1a,
            from_region: &[0x2e, 0x4e, 0x61, 0x64, 0x73, 0xda, 0x10d, 0x110],
        },
        ParentRel {
            lang: 0x13e,
            script: 0x0,
            max_script: 0x5b,
            to_region: 0x1f,
            from_region: &[
                0x2c, 0x3f, 0x41, 0x48, 0x51, 0x54, 0x57, 0x5a, 0x66, 0x6a, 0x8a, 0x90, 0xd0, 0xd9,
                0xe3, 0xe5, 0xed, 0xf2, 0x11b, 0x136, 0x137, 0x13c,
            ],
        },
        ParentRel {
            lang: 0x3c0,
            script: 0x0,
            max_script: 0x5b,
            to_region: 0xef,
            from_region: &[0x2a, 0x4e, 0x5b, 0x87, 0x8c, 0xb8, 0xc7, 0xd2, 0x119, 0x127],
        },
        ParentRel {
            lang: 0x529,
            script: 0x3c,
            max_script: 0x3c,
            to_region: 0x8e,
            from_region: &[0xc7],
        },
    ];
    // Go: internal/language/compact/tables.go:791 coreTags
    pub(super) static CORE_TAGS: [u32; 773] = [
        0x00000000, 0x01600000, 0x016000d3, 0x01600162, 0x01c00000, 0x01c00052, 0x02100000,
        0x02100081, 0x02700000, 0x02700070, 0x03a00000, 0x03a00001, 0x03a00023, 0x03a00039,
        0x03a00063, 0x03a00068, 0x03a0006c, 0x03a0006d, 0x03a0006e, 0x03a00098, 0x03a0009c,
        0x03a000a2, 0x03a000a9, 0x03a000ad, 0x03a000b1, 0x03a000ba, 0x03a000bb, 0x03a000ca,
        0x03a000e2, 0x03a000ee, 0x03a000f4, 0x03a00109, 0x03a0010c, 0x03a00116, 0x03a00118,
        0x03a0011d, 0x03a00121, 0x03a00129, 0x03a0015f, 0x04000000, 0x04300000, 0x0430009a,
        0x04400000, 0x04400130, 0x04800000, 0x0480006f, 0x05800000, 0x05820000, 0x05820032,
        0x0585b000, 0x0585b032, 0x05e00000, 0x05e00052, 0x07100000, 0x07100047, 0x07500000,
        0x07500163, 0x07900000, 0x07900130, 0x07e00000, 0x07e00038, 0x08200000, 0x0a000000,
        0x0a0000c4, 0x0a500000, 0x0a500035, 0x0a50009a, 0x0a900000, 0x0a900053, 0x0a90009a,
        0x0b200000, 0x0b200079, 0x0b500000, 0x0b50009a, 0x0b700000, 0x0b720000, 0x0b720033,
        0x0b75b000, 0x0b75b033, 0x0d700000, 0x0d700022, 0x0d70006f, 0x0d700079, 0x0d70009f,
        0x0db00000, 0x0db00035, 0x0db0009a, 0x0dc00000, 0x0dc00107, 0x0df00000, 0x0df00132,
        0x0e500000, 0x0e500136, 0x0e900000, 0x0e90009c, 0x0e90009d, 0x0fa00000, 0x0fa0005f,
        0x0fe00000, 0x0fe00107, 0x10000000, 0x1000007c, 0x10100000, 0x10100064, 0x10100083,
        0x10800000, 0x108000a5, 0x10d00000, 0x10d0002e, 0x10d00036, 0x10d0004e, 0x10d00061,
        0x10d0009f, 0x10d000b3, 0x10d000b8, 0x11700000, 0x117000d5, 0x11f00000, 0x11f00061,
        0x12400000, 0x12400052, 0x12800000, 0x12b00000, 0x12b00115, 0x12d00000, 0x12d00043,
        0x12f00000, 0x12f000a5, 0x13000000, 0x13000081, 0x13000123, 0x13600000, 0x1360005e,
        0x13600088, 0x13900000, 0x13900001, 0x1390001a, 0x13900025, 0x13900026, 0x1390002d,
        0x1390002e, 0x1390002f, 0x13900034, 0x13900036, 0x1390003a, 0x1390003d, 0x13900042,
        0x13900046, 0x13900048, 0x13900049, 0x1390004a, 0x1390004e, 0x13900050, 0x13900052,
        0x1390005d, 0x1390005e, 0x13900061, 0x13900062, 0x13900064, 0x13900065, 0x1390006e,
        0x13900073, 0x13900074, 0x13900075, 0x13900076, 0x1390007c, 0x1390007d, 0x13900080,
        0x13900081, 0x13900082, 0x13900084, 0x1390008b, 0x1390008d, 0x1390008e, 0x13900097,
        0x13900098, 0x13900099, 0x1390009a, 0x1390009b, 0x139000a0, 0x139000a1, 0x139000a5,
        0x139000a8, 0x139000aa, 0x139000ae, 0x139000b2, 0x139000b5, 0x139000b6, 0x139000c0,
        0x139000c1, 0x139000c7, 0x139000c8, 0x139000cb, 0x139000cc, 0x139000cd, 0x139000cf,
        0x139000d1, 0x139000d3, 0x139000d6, 0x139000d7, 0x139000da, 0x139000de, 0x139000e0,
        0x139000e1, 0x139000e7, 0x139000e8, 0x139000e9, 0x139000ec, 0x139000ed, 0x139000f1,
        0x13900108, 0x1390010a, 0x1390010b, 0x1390010c, 0x1390010d, 0x1390010e, 0x1390010f,
        0x13900110, 0x13900113, 0x13900118, 0x1390011c, 0x1390011e, 0x13900120, 0x13900126,
        0x1390012a, 0x1390012d, 0x1390012e, 0x13900130, 0x13900132, 0x13900134, 0x13900136,
        0x1390013a, 0x1390013d, 0x1390013e, 0x13900140, 0x13900143, 0x13900162, 0x13900163,
        0x13900165, 0x13c00000, 0x13c00001, 0x13e00000, 0x13e0001f, 0x13e0002c, 0x13e0003f,
        0x13e00041, 0x13e00048, 0x13e00051, 0x13e00054, 0x13e00057, 0x13e0005a, 0x13e00066,
        0x13e00069, 0x13e0006a, 0x13e0006f, 0x13e00087, 0x13e0008a, 0x13e00090, 0x13e00095,
        0x13e000d0, 0x13e000d9, 0x13e000e3, 0x13e000e5, 0x13e000e8, 0x13e000ed, 0x13e000f2,
        0x13e0011b, 0x13e00136, 0x13e00137, 0x13e0013c, 0x14000000, 0x1400006b, 0x14500000,
        0x1450006f, 0x14600000, 0x14600052, 0x14800000, 0x14800024, 0x1480009d, 0x14e00000,
        0x14e00052, 0x14e00085, 0x14e000ca, 0x14e00115, 0x15100000, 0x15100073, 0x15300000,
        0x153000e8, 0x15800000, 0x15800064, 0x15800077, 0x15e00000, 0x15e00036, 0x15e00037,
        0x15e0003a, 0x15e0003b, 0x15e0003c, 0x15e00049, 0x15e0004b, 0x15e0004c, 0x15e0004d,
        0x15e0004e, 0x15e0004f, 0x15e00052, 0x15e00063, 0x15e00068, 0x15e00079, 0x15e0007b,
        0x15e0007f, 0x15e00085, 0x15e00086, 0x15e00087, 0x15e00092, 0x15e000a9, 0x15e000b8,
        0x15e000bb, 0x15e000bc, 0x15e000bf, 0x15e000c0, 0x15e000c4, 0x15e000c9, 0x15e000ca,
        0x15e000cd, 0x15e000d4, 0x15e000d5, 0x15e000e6, 0x15e000eb, 0x15e00103, 0x15e00108,
        0x15e0010b, 0x15e00115, 0x15e0011d, 0x15e00121, 0x15e00123, 0x15e00129, 0x15e00140,
        0x15e00141, 0x15e00160, 0x16900000, 0x1690009f, 0x16d00000, 0x16d000da, 0x16e00000,
        0x16e00097, 0x17e00000, 0x17e0007c, 0x19000000, 0x1900006f, 0x1a300000, 0x1a30004e,
        0x1a300079, 0x1a3000b3, 0x1a400000, 0x1a40009a, 0x1a900000, 0x1ab00000, 0x1ab000a5,
        0x1ac00000, 0x1ac00099, 0x1b400000, 0x1b400081, 0x1b4000d5, 0x1b4000d7, 0x1b800000,
        0x1b800136, 0x1bc00000, 0x1bc00098, 0x1be00000, 0x1be0009a, 0x1d100000, 0x1d100033,
        0x1d100091, 0x1d200000, 0x1d200061, 0x1d500000, 0x1d500093, 0x1d700000, 0x1d700028,
        0x1e100000, 0x1e100096, 0x1e700000, 0x1e7000d7, 0x1ea00000, 0x1ea00053, 0x1f300000,
        0x1f500000, 0x1f800000, 0x1f80009e, 0x1f900000, 0x1f90004e, 0x1f90009f, 0x1f900114,
        0x1f900139, 0x1fa00000, 0x1fb00000, 0x20000000, 0x200000a3, 0x20300000, 0x20700000,
        0x20700052, 0x20800000, 0x20a00000, 0x20a00130, 0x20e00000, 0x20f00000, 0x21000000,
        0x2100007e, 0x21200000, 0x21200068, 0x21600000, 0x21700000, 0x217000a5, 0x21f00000,
        0x22300000, 0x22300130, 0x22700000, 0x2270005b, 0x23400000, 0x234000c4, 0x23900000,
        0x239000a5, 0x24200000, 0x242000af, 0x24400000, 0x24400052, 0x24500000, 0x24500083,
        0x24600000, 0x246000a5, 0x24a00000, 0x24a000a7, 0x25100000, 0x2510009a, 0x25400000,
        0x254000ab, 0x254000ac, 0x25600000, 0x2560009a, 0x26a00000, 0x26a0009a, 0x26b00000,
        0x26b00130, 0x26d00000, 0x26d00052, 0x26e00000, 0x26e00061, 0x27400000, 0x28100000,
        0x2810007c, 0x28a00000, 0x28a000a6, 0x29100000, 0x29100130, 0x29500000, 0x295000b8,
        0x2a300000, 0x2a300132, 0x2af00000, 0x2af00136, 0x2b500000, 0x2b50002a, 0x2b50004b,
        0x2b50004c, 0x2b50004d, 0x2b800000, 0x2b8000b0, 0x2bf00000, 0x2bf0009c, 0x2bf0009d,
        0x2c000000, 0x2c0000b7, 0x2c200000, 0x2c20004b, 0x2c400000, 0x2c4000a5, 0x2c500000,
        0x2c5000a5, 0x2c700000, 0x2c7000b9, 0x2d100000, 0x2d1000a5, 0x2d100130, 0x2e900000,
        0x2e9000a5, 0x2ed00000, 0x2ed000cd, 0x2f100000, 0x2f1000c0, 0x2f200000, 0x2f2000d2,
        0x2f400000, 0x2f400052, 0x2ff00000, 0x2ff000c3, 0x30400000, 0x3040009a, 0x30b00000,
        0x30b000c6, 0x31000000, 0x31b00000, 0x31b0009a, 0x31f00000, 0x31f0003e, 0x31f000d1,
        0x31f0010e, 0x32000000, 0x320000cc, 0x32500000, 0x32500052, 0x33100000, 0x331000c5,
        0x33a00000, 0x33a0009d, 0x34100000, 0x34500000, 0x345000d3, 0x34700000, 0x347000db,
        0x34700111, 0x34e00000, 0x34e00165, 0x35000000, 0x35000061, 0x350000da, 0x35100000,
        0x3510009a, 0x351000dc, 0x36700000, 0x36700030, 0x36700036, 0x36700040, 0x3670005c,
        0x367000da, 0x36700117, 0x3670011c, 0x36800000, 0x36800052, 0x36a00000, 0x36a000db,
        0x36c00000, 0x36c00052, 0x36f00000, 0x37500000, 0x37600000, 0x37a00000, 0x38000000,
        0x38000118, 0x38700000, 0x38900000, 0x38900132, 0x39000000, 0x39000070, 0x390000a5,
        0x39500000, 0x3950009a, 0x39800000, 0x3980007e, 0x39800107, 0x39d00000, 0x39d05000,
        0x39d050e9, 0x39d36000, 0x39d3609a, 0x3a100000, 0x3b300000, 0x3b3000ea, 0x3bd00000,
        0x3bd00001, 0x3be00000, 0x3be00024, 0x3c000000, 0x3c00002a, 0x3c000041, 0x3c00004e,
        0x3c00005b, 0x3c000087, 0x3c00008c, 0x3c0000b8, 0x3c0000c7, 0x3c0000d2, 0x3c0000ef,
        0x3c000119, 0x3c000127, 0x3c400000, 0x3c40003f, 0x3c40006a, 0x3c4000e5, 0x3d400000,
        0x3d40004e, 0x3d900000, 0x3d90003a, 0x3dc00000, 0x3dc000bd, 0x3dc00105, 0x3de00000,
        0x3de00130, 0x3e200000, 0x3e200047, 0x3e2000a6, 0x3e2000af, 0x3e2000bd, 0x3e200107,
        0x3e200131, 0x3e500000, 0x3e500108, 0x3e600000, 0x3e600130, 0x3eb00000, 0x3eb00107,
        0x3ec00000, 0x3ec000a5, 0x3f300000, 0x3f300130, 0x3fa00000, 0x3fa000e9, 0x3fc00000,
        0x3fd00000, 0x3fd00073, 0x3fd000db, 0x3fd0010d, 0x3ff00000, 0x3ff000d2, 0x40100000,
        0x401000c4, 0x40200000, 0x4020004c, 0x40700000, 0x40800000, 0x4085b000, 0x4085b0bb,
        0x408eb000, 0x408eb0bb, 0x40c00000, 0x40c000b4, 0x41200000, 0x41200112, 0x41600000,
        0x41600110, 0x41c00000, 0x41d00000, 0x41e00000, 0x41f00000, 0x41f00073, 0x42200000,
        0x42300000, 0x42300165, 0x42900000, 0x42900063, 0x42900070, 0x429000a5, 0x42900116,
        0x43100000, 0x43100027, 0x431000c3, 0x4310014e, 0x43200000, 0x43220000, 0x43220033,
        0x432200be, 0x43220106, 0x4322014e, 0x4325b000, 0x4325b033, 0x4325b0be, 0x4325b106,
        0x4325b14e, 0x43700000, 0x43a00000, 0x43b00000, 0x44400000, 0x44400031, 0x44400073,
        0x4440010d, 0x44500000, 0x4450004b, 0x445000a5, 0x44500130, 0x44500132, 0x44e00000,
        0x45000000, 0x4500009a, 0x450000b4, 0x450000d1, 0x4500010e, 0x46100000, 0x4610009a,
        0x46400000, 0x464000a5, 0x46400132, 0x46700000, 0x46700125, 0x46b00000, 0x46b00124,
        0x46f00000, 0x46f0006e, 0x46f00070, 0x47100000, 0x47600000, 0x47600128, 0x47a00000,
        0x48000000, 0x48200000, 0x4820012a, 0x48a00000, 0x48a0005e, 0x48a0012c, 0x48e00000,
        0x49400000, 0x49400107, 0x4a400000, 0x4a4000d5, 0x4a900000, 0x4a9000bb, 0x4ac00000,
        0x4ac00053, 0x4ae00000, 0x4ae00131, 0x4b400000, 0x4b40009a, 0x4b4000e9, 0x4bc00000,
        0x4bc05000, 0x4bc05024, 0x4bc20000, 0x4bc20138, 0x4bc5b000, 0x4bc5b138, 0x4be00000,
        0x4be5b000, 0x4be5b0b5, 0x4bef4000, 0x4bef40b5, 0x4c000000, 0x4c300000, 0x4c30013f,
        0x4c900000, 0x4c900001, 0x4cc00000, 0x4cc00130, 0x4ce00000, 0x4cf00000, 0x4cf0004e,
        0x4e500000, 0x4e500115, 0x4f200000, 0x4fb00000, 0x4fb00132, 0x50900000, 0x50900052,
        0x51200000, 0x51200001, 0x51800000, 0x5180003b, 0x518000d7, 0x51f00000, 0x51f3b000,
        0x51f3b053, 0x51f3c000, 0x51f3c08e, 0x52800000, 0x528000bb, 0x52900000, 0x5293b000,
        0x5293b053, 0x5293b08e, 0x5293b0c7, 0x5293b10e, 0x5293c000, 0x5293c08e, 0x5293c0c7,
        0x5293c12f, 0x52f00000, 0x52f00162,
    ];
    // Go: internal/language/compact/tables.go:1013 specialTagsStr
    pub(super) const SPECIAL_TAGS_STR: &str = "ca-ES-valencia en-US-u-va-posix";
}
