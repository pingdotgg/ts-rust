use crate::contentmapper::Mapper;
use crate::frontend::prelude::*;
use crate::frontend::stringutil_ls::equate_string_case_insensitive;
use std::cell::OnceCell;
use std::rc::Weak;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

// This file ports tsoptions/parsedcommandline.go. `ParsedOptions` is in
// `parsed_options.rs` (tsgo#4712 moves it from core to tsoptions).
// PORT: Go `sync.Once` plus a cached field is a single-threaded `OnceCell`.
// PORT: Go methods that check `p == nil` take `&self`, which is never nil.
// A Go nil `*ParsedCommandLine` is `Option<ParsedCommandLine>` at the caller.
// PORT: Go `iter.Seq` results are eager `Vec`s. Every Go caller reads the
// whole sequence.

// Go: tsoptions/parsedcommandline.go:24 fileGlobPattern
const FILE_GLOB_PATTERN: &str = "*.{js,jsx,mjs,cjs,ts,tsx,mts,cts,json}";
// Go: tsoptions/parsedcommandline.go:25 recursiveFileGlobPattern
const RECURSIVE_FILE_GLOB_PATTERN: &str = "**/*.{js,jsx,mjs,cjs,ts,tsx,mts,cts,json}";

impl ParsedCommandLine {
    // Go: tsoptions/parsedcommandline.go:31 (*ParsedCommandLine).fileGlobPatterns (tsgo#4712)
    // fileGlobPatterns returns the include file glob patterns for this command line, augmenting the
    // built-in patterns with the extensions registered by its content mappers so that created
    // content-mapped files are recognized as possible root files.
    fn file_glob_patterns(&self) -> (String, String) {
        let mapper_extensions = self.content_mapper_extensions();
        if mapper_extensions.is_empty() {
            return (
                FILE_GLOB_PATTERN.to_string(),
                RECURSIVE_FILE_GLOB_PATTERN.to_string(),
            );
        }
        let mut extensions: Vec<&str> = Vec::with_capacity(9 + mapper_extensions.len());
        extensions.extend(["js", "jsx", "mjs", "cjs", "ts", "tsx", "mts", "cts", "json"]);
        for extension in &mapper_extensions {
            extensions.push(extension.strip_prefix('.').unwrap_or(extension));
        }
        let file_glob = format!("*.{{{}}}", extensions.join(","));
        let recursive_file_glob = format!("**/{file_glob}");
        (file_glob, recursive_file_glob)
    }
}

// ---------------------------------------------------------------------------
// Go: glob/glob.go
// PORT: `internal/glob` has one compiler caller,
// `ParsedCommandLine.WildcardDirectoryGlobs`, so it is ported in this file.
// Package-level names get a `glob_` prefix (like `json_marshal`) so the glob
// export stays unambiguous. `Parse` returns the Go error text as `Err`.
// ---------------------------------------------------------------------------

// Go: glob/glob.go:41 Glob
/// A Glob is an LSP-compliant glob pattern, as defined by the spec:
/// https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#documentFilter
///
/// Glob patterns can have the following syntax:
///   - `*` to match one or more characters in a path segment
///   - `?` to match on one character in a path segment
///   - `**` to match any number of path segments, including none
///   - `{}` to group sub patterns into an OR expression. (e.g. `**/*.{ts,js}`
///     matches all TypeScript and JavaScript files)
///   - `[]` to declare a range of characters to match in a path segment
///     (e.g., `example.[0-9]` to match on `example.0`, `example.1`, …)
///   - `[!...]` to negate a range of characters to match in a path segment
///     (e.g., `example.[!0-9]` to match on `example.a`, `example.b`, but
///     not `example.0`)
///
/// Expanding on this:
///   - '/' matches one or more literal slashes.
///   - any other character matches itself literally.
#[derive(Clone, Debug, Default)]
pub struct Glob {
    elems: Vec<GlobElement>, // pattern elements
}

// Go: glob/glob.go:181 element and :184 element types
/// element holds a glob pattern element.
// PORT: the Go `element` interface and its seven types are one enum.
#[derive(Clone, Debug)]
pub enum GlobElement {
    /// One or more '/' separators
    Slash,
    /// string literal, not containing /, *, ?, {}, or []
    Literal(String),
    /// *
    Star,
    /// ?
    AnyChar,
    /// **
    StarStar,
    /// {foo, bar, ...} grouping
    Group(Vec<Glob>),
    /// [a-z] character range
    CharRange { negate: bool, low: char, high: char },
}

// Go: glob/glob.go:47 Parse
/// Parse builds a Glob for the given pattern, returning an error if the pattern
/// is invalid.
pub fn glob_parse(pattern: &str) -> Result<Glob, String> {
    let (g, _) = glob_parse_nested(pattern, false)?;
    Ok(g)
}

// Go: glob/glob.go:52 parse
// PORT: Go indexes `pattern[0]` as a byte. Every byte it tests is ASCII, so
// the `&str` slices below stay on character boundaries.
fn glob_parse_nested(mut pattern: &str, nested: bool) -> Result<(Glob, &str), String> {
    let mut g = Glob::default();
    while !pattern.is_empty() {
        match pattern.as_bytes()[0] {
            b'/' => {
                pattern = &pattern[1..];
                g.elems.push(GlobElement::Slash);
            }

            b'*' => {
                let bytes = pattern.as_bytes();
                if bytes.len() > 1 && bytes[1] == b'*' {
                    if (!g.elems.is_empty()
                        && !matches!(g.elems[g.elems.len() - 1], GlobElement::Slash))
                        || (bytes.len() > 2 && bytes[2] != b'/')
                    {
                        return Err("** may only be adjacent to '/'".to_string());
                    }
                    pattern = &pattern[2..];
                    g.elems.push(GlobElement::StarStar);
                    continue;
                }
                pattern = &pattern[1..];
                g.elems.push(GlobElement::Star);
            }

            b'?' => {
                pattern = &pattern[1..];
                g.elems.push(GlobElement::AnyChar);
            }

            b'{' => {
                let mut gs: Vec<Glob> = Vec::new();
                while pattern.as_bytes()[0] != b'}' {
                    pattern = &pattern[1..];
                    let (group_g, pat) = glob_parse_nested(pattern, true)?;
                    if pat.is_empty() {
                        return Err("unmatched '{'".to_string());
                    }
                    pattern = pat;
                    gs.push(group_g);
                }
                pattern = &pattern[1..];
                g.elems.push(GlobElement::Group(gs));
            }

            b'}' | b',' => {
                if nested {
                    return Ok((g, pattern));
                }
                pattern = g.parse_literal(pattern, false);
            }

            b'[' => {
                pattern = &pattern[1..];
                if pattern.is_empty() {
                    return Err(GLOB_ERR_BAD_RANGE.to_string());
                }
                let mut negate = false;
                if pattern.as_bytes()[0] == b'!' {
                    pattern = &pattern[1..];
                    negate = true;
                }
                let (low, sz) = glob_read_range_rune(pattern)?;
                pattern = &pattern[sz..];
                if pattern.is_empty() || pattern.as_bytes()[0] != b'-' {
                    return Err(GLOB_ERR_BAD_RANGE.to_string());
                }
                pattern = &pattern[1..];
                let (high, sz) = glob_read_range_rune(pattern)?;
                pattern = &pattern[sz..];
                if pattern.is_empty() || pattern.as_bytes()[0] != b']' {
                    return Err(GLOB_ERR_BAD_RANGE.to_string());
                }
                pattern = &pattern[1..];
                g.elems.push(GlobElement::CharRange { negate, low, high });
            }

            _ => {
                pattern = g.parse_literal(pattern, nested);
            }
        }
    }
    Ok((g, ""))
}

// Go: glob/glob.go:137 readRangeRune
/// helper for decoding a rune in range elements, e.g. [a-z]
// PORT: a Rust `&str` is valid UTF-8, so Go's `errInvalidUTF8` case (a
// one-byte `RuneError`) cannot happen. An encoded U+FFFD is a valid rune in
// both.
fn glob_read_range_rune(input: &str) -> Result<(char, usize), String> {
    match input.chars().next() {
        Some(r) => Ok((r, r.len_utf8())),
        None => Err(GLOB_ERR_BAD_RANGE.to_string()),
    }
}

// Go: glob/glob.go:153 errBadRange
const GLOB_ERR_BAD_RANGE: &str = "'[' patterns must be of the form [x-y]";

impl Glob {
    // Go: glob/glob.go:157 (*Glob).parseLiteral
    fn parse_literal<'a>(&mut self, pattern: &'a str, nested: bool) -> &'a str {
        let special_chars: &[char] = if nested {
            &['*', '?', '{', '[', '/', '}', ',']
        } else {
            &['*', '?', '{', '[', '/']
        };
        let end = pattern.find(special_chars).unwrap_or(pattern.len());
        self.elems
            .push(GlobElement::Literal(pattern[..end].to_string()));
        &pattern[end..]
    }

    // Go: glob/glob.go:215 (*Glob).Match
    /// Match reports whether the input string matches the glob pattern.
    // PORT: Go matches byte by byte (`?` skips one byte), so the input is
    // matched as bytes.
    pub fn match_(&self, input: &str) -> bool {
        let elems: Vec<&GlobElement> = self.elems.iter().collect();
        glob_match(&elems, input.as_bytes())
    }
}

// Go: glob/glob.go:172 (*Glob).String
// PORT: Go `fmt.Stringer` is `Display`.
impl std::fmt::Display for Glob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for e in &self.elems {
            write!(f, "{e}")?;
        }
        Ok(())
    }
}

// Go: glob/glob.go:197-213 element String methods
// PORT: like Go, a negated range prints without the `!`.
impl std::fmt::Display for GlobElement {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GlobElement::Slash => f.write_str("/"),
            GlobElement::Literal(l) => f.write_str(l),
            GlobElement::Star => f.write_str("*"),
            GlobElement::AnyChar => f.write_str("?"),
            GlobElement::StarStar => f.write_str("**"),
            GlobElement::Group(g) => {
                let parts: Vec<String> = g.iter().map(ToString::to_string).collect();
                write!(f, "{{{}}}", parts.join(","))
            }
            GlobElement::CharRange { low, high, .. } => write!(f, "[{low}-{high}]"),
        }
    }
}

// Go: glob/glob.go:219 match
// PORT: Go slices `[]element`; a group branch appends the rest of the
// pattern to a member's elements. Elements are borrowed here so a branch is a
// list of references. Go ignores `charRange.negate` when matching, and so
// does this port. Like Go, a `/` element that consumes the rest of the input
// indexes past the end and panics.
fn glob_match(mut elems: &[&GlobElement], mut input: &[u8]) -> bool {
    while !elems.is_empty() {
        let elem = elems[0];
        elems = &elems[1..];
        match elem {
            GlobElement::Slash => {
                if input.is_empty() || input[0] != b'/' {
                    return false;
                }
                while input[0] == b'/' {
                    input = &input[1..];
                }
            }

            GlobElement::StarStar => {
                // Special cases:
                //  - **/a matches "a"
                //  - **/ matches everything
                //
                // Note that if ** is followed by anything, it must be '/' (this is
                // enforced by Parse).
                if !elems.is_empty() {
                    elems = &elems[1..];
                }

                // A trailing ** matches anything.
                if elems.is_empty() {
                    return true;
                }

                // Backtracking: advance pattern segments until the remaining pattern
                // elements match.
                while !input.is_empty() {
                    if glob_match(elems, input) {
                        return true;
                    }
                    input = glob_split(input).1;
                }
                return false;
            }

            GlobElement::Literal(literal) => {
                if !input.starts_with(literal.as_bytes()) {
                    return false;
                }
                input = &input[literal.len()..];
            }

            GlobElement::Star => {
                let (seg_input, rest) = glob_split(input);
                input = rest;

                let elem_end = elems
                    .iter()
                    .position(|e| matches!(e, GlobElement::Slash))
                    .unwrap_or(elems.len());
                let seg_elems = &elems[..elem_end];
                elems = &elems[elem_end..];

                // A trailing * matches the entire segment.
                if seg_elems.is_empty() {
                    continue;
                }

                // Backtracking: advance characters until remaining subpattern elements
                // match.
                let mut matched = false;
                for i in 0..seg_input.len() {
                    if glob_match(seg_elems, &seg_input[i..]) {
                        matched = true;
                        break;
                    }
                }
                if !matched {
                    return false;
                }
            }

            GlobElement::AnyChar => {
                if input.is_empty() || input[0] == b'/' {
                    return false;
                }
                input = &input[1..];
            }

            GlobElement::Group(group) => {
                // Append remaining pattern elements to each group member looking for a
                // match.
                let mut branch: Vec<&GlobElement> = Vec::new();
                for m in group {
                    branch.clear();
                    branch.extend(m.elems.iter());
                    branch.extend_from_slice(elems);
                    if glob_match(&branch, input) {
                        return true;
                    }
                }
                return false;
            }

            GlobElement::CharRange { low, high, .. } => {
                if input.is_empty() || input[0] == b'/' {
                    return false;
                }
                let (c, sz) = glob_decode_rune(input);
                if c < *low || c > *high {
                    return false;
                }
                input = &input[sz..];
            }
        }
    }

    input.is_empty()
}

// Go `utf8.DecodeRuneInString` for a non-empty input: an invalid encoding
// gives `(RuneError, 1)`.
// PORT: Go `match` can split a rune (`?` skips one byte), so the input can
// start mid-rune. Rust and Go accept the same UTF-8 encodings.
fn glob_decode_rune(input: &[u8]) -> (char, usize) {
    let head = &input[..input.len().min(4)];
    let valid = match std::str::from_utf8(head) {
        Ok(s) => s,
        Err(e) => std::str::from_utf8(&head[..e.valid_up_to()]).unwrap_or_default(),
    };
    match valid.chars().next() {
        Some(c) => (c, c.len_utf8()),
        None => (char::REPLACEMENT_CHARACTER, 1),
    }
}

// Go: glob/glob.go:337 split
/// split returns the portion before and after the first slash
/// (or sequence of consecutive slashes). If there is no slash
/// it returns (input, nil).
fn glob_split(input: &[u8]) -> (&[u8], &[u8]) {
    let Some(i) = input.iter().position(|&c| c == b'/') else {
        return (input, &[]);
    };
    let first = &input[..i];
    for j in i..input.len() {
        if input[j] != b'/' {
            return (first, &input[j..]);
        }
    }
    (first, &[])
}

// PORT: the two Go maps that `ParseInputOutputNames` fills in one `Once`.
#[derive(Debug, Default)]
pub struct SourceAndOutputMaps {
    pub source_to_project_reference: FxHashMap<Path, Rc<SourceOutputAndProjectReference>>,
    pub output_dts_to_project_reference: FxHashMap<Path, Rc<SourceOutputAndProjectReference>>,
}

// Go: tsoptions/parsedcommandline.go:45 ParsedCommandLine
// PORT: all fields are `pub`, because Go code in the same package (the
// tsconfig parser) writes the unexported fields.
// PORT: Go `ConfigFile *TsConfigSourceFile` is `Option<Rc<...>>`, because
// `ReloadFileNamesOfParsedCommandLine` shares it with the new value.
// PORT: Go `locale` and `localeOnce` are one `OnceCell` (`locale`).
#[derive(Debug, Default)]
pub struct ParsedCommandLine {
    pub parsed_config: ParsedOptions,

    /// TsConfigSourceFile, used in Program and ExecuteCommandLine
    pub config_file: Option<Rc<TsConfigSourceFile>>,
    pub errors: Vec<Diagnostic>,
    pub raw: CompilerOptionsValue,
    pub compile_on_save: Option<bool>,

    pub compare_paths_options: ComparePathsOptions,
    pub wildcard_directories: OnceCell<FxHashMap<String, bool>>,
    pub include_globs: OnceCell<Vec<Glob>>,

    pub source_and_output_maps: OnceCell<SourceAndOutputMaps>,

    pub common_source_directory: OnceCell<String>,
    /// Go appends these to `Errors` from `CommonSourceDirectory`. Readers of
    /// Go `Errors` read `errors` and then these
    /// (`errors_with_common_source_directory_errors`).
    pub common_source_directory_errors: RefCell<Vec<Diagnostic>>,
    /// Set when a checker thread read `CommonSourceDirectory` of this
    /// command line through its thread-safe copy
    /// (`program::ResolvedProjectReference`). Go records the TS6059 errors
    /// on that thread; the copy cannot reach this command line there, so
    /// the next reader of the errors records them
    /// (`errors_with_common_source_directory_errors`).
    pub common_source_directory_read: Arc<AtomicBool>,

    pub resolved_project_reference_paths: OnceCell<Vec<String>>,

    // PORT: Go `int` is `i32`.
    pub literal_file_names_len: i32,
    /// ts#64159: the root file names as the command line gave them, for the
    /// root file diagnostics (`root_file_name_for_diagnostic`). Empty for a
    /// config file.
    pub root_file_names_for_diagnostics: Vec<String>,
    /// maps file names to their paths, used for quick lookups
    pub file_names_by_path: OnceCell<FxHashMap<Path, String>>,

    pub locale: OnceCell<crate::locale::Locale>,
}

// Go: tsoptions/parsedcommandline.go:80 NewParsedCommandLine
// PORT: Go returns a new pointer; this returns the value. A nil
// `projectReferences` slice is `None`.
pub fn new_parsed_command_line(
    compiler_options: Rc<CompilerOptions>,
    root_file_names: Vec<String>,
    project_references: Option<Vec<ProjectReference>>,
    compare_paths_options: ComparePathsOptions,
) -> ParsedCommandLine {
    ParsedCommandLine {
        parsed_config: ParsedOptions {
            compiler_options,
            file_names: root_file_names,
            project_references,
            ..Default::default()
        },
        compare_paths_options,
        ..Default::default()
    }
}

impl ParsedCommandLine {
    // Go: tsoptions/parsedcommandline.go:98 (*ParsedCommandLine).WithFileNames (tsgo#4712)
    // PORT: Go returns a new pointer; this returns the value. Go copies the
    // cached `wildcardDirectories` map and `includeGlobs` slice but not their
    // `sync.Once`; this clones both cache cells, as
    // `reload_file_names_of_parsed_command_line` does. Go `p.Errors` holds
    // the common source directory errors so far, so the copy's `errors` do.
    #[must_use]
    pub fn with_file_names(&self, file_names: Vec<String>) -> ParsedCommandLine {
        let mut parsed_config = self.parsed_config.clone();
        parsed_config.file_names = file_names;
        ParsedCommandLine {
            parsed_config,
            config_file: self.config_file.clone(),
            errors: self.errors_with_common_source_directory_errors(),
            raw: self.raw.clone(),
            compile_on_save: self.compile_on_save,
            compare_paths_options: self.compare_paths_options.clone(),
            wildcard_directories: self.wildcard_directories.clone(),
            include_globs: self.include_globs.clone(),
            literal_file_names_len: self.literal_file_names_len,
            root_file_names_for_diagnostics: self.root_file_names_for_diagnostics.clone(),
            ..Default::default()
        }
    }
}

// Go: tsoptions/parsedcommandline.go:127 SourceOutputAndProjectReference
// PORT: Go `Resolved *ParsedCommandLine` points back at the command line
// that owns the map. An `Rc` would make a reference cycle, so this is a
// `Weak`. `ParsedCommandLine::parse_input_output_names` takes `&Rc<Self>`.
#[derive(Debug)]
pub struct SourceOutputAndProjectReference {
    pub source: String,
    pub output_dts: String,
    pub resolved: Weak<ParsedCommandLine>,
}

// Go: tsoptions/parsedcommandline.go:80 interface assertions
// PORT: `module.ResolvedProjectReference` and `outputpaths.OutputPathsHost`
// are satisfied by the methods below; there is no Rust trait to assert.

impl ParsedCommandLine {
    // Go: tsoptions/parsedcommandline.go:140 (*ParsedCommandLine).ConfigName
    pub fn config_name(&self) -> &'static str {
        let Some(config_file) = &self.config_file else {
            return "";
        };
        source_file_file_name(config_file.source_file)
    }

    // Go: tsoptions/parsedcommandline.go:154 (*ParsedCommandLine).SourceToProjectReference
    // PORT: Go returns a nil map before `ParseInputOutputNames`; that is `None`.
    pub fn source_to_project_reference(
        &self,
    ) -> Option<&FxHashMap<Path, Rc<SourceOutputAndProjectReference>>> {
        self.source_and_output_maps
            .get()
            .map(|m| &m.source_to_project_reference)
    }

    // Go: tsoptions/parsedcommandline.go:158 (*ParsedCommandLine).OutputDtsToProjectReference
    // PORT: Go returns a nil map before `ParseInputOutputNames`; that is `None`.
    pub fn output_dts_to_project_reference(
        &self,
    ) -> Option<&FxHashMap<Path, Rc<SourceOutputAndProjectReference>>> {
        self.source_and_output_maps
            .get()
            .map(|m| &m.output_dts_to_project_reference)
    }

    // Go: tsoptions/parsedcommandline.go:162 (*ParsedCommandLine).ParseInputOutputNames
    pub fn parse_input_output_names(self: &Rc<Self>) {
        self.source_and_output_maps.get_or_init(|| {
            let mut source_to_output: FxHashMap<Path, Rc<SourceOutputAndProjectReference>> =
                FxHashMap::default();
            let mut output_dts_to_source: FxHashMap<Path, Rc<SourceOutputAndProjectReference>> =
                FxHashMap::default();

            for (output_dts, source) in self.get_output_declaration_and_source_file_names() {
                let path = to_path(
                    &source,
                    self.get_current_directory(),
                    self.use_case_sensitive_file_names(),
                );
                let project_reference = Rc::new(SourceOutputAndProjectReference {
                    source,
                    output_dts: output_dts.clone(),
                    resolved: Rc::downgrade(self),
                });
                if !output_dts.is_empty() {
                    output_dts_to_source.insert(
                        to_path(
                            &output_dts,
                            self.get_current_directory(),
                            self.use_case_sensitive_file_names(),
                        ),
                        project_reference.clone(),
                    );
                }
                source_to_output.insert(path, project_reference);
            }
            SourceAndOutputMaps {
                output_dts_to_project_reference: output_dts_to_source,
                source_to_project_reference: source_to_output,
            }
        });
    }

    // Go: tsoptions/parsedcommandline.go:185 (*ParsedCommandLine).CommonSourceDirectory
    // PORT: Go passes `checkSourceFilesBelongToPath`, which appends to
    // `Errors`. This method takes `&self`, so those diagnostics go to
    // `common_source_directory_errors` instead, and the readers of Go
    // `Errors` read both (`errors_with_common_source_directory_errors`).
    // Call it only where Go calls `CommonSourceDirectory`: an extra call
    // adds TS6059 errors that Go does not report
    // (`common_source_directory_unchecked` reads the value without them).
    pub fn common_source_directory(&self) -> &str {
        self.common_source_directory.get_or_init(|| {
            let mut check = |source_files: &[String], root_directory: &str| -> bool {
                self.check_source_files_belong_to_path(source_files, root_directory)
            };
            get_common_source_directory(
                &self.parsed_config.compiler_options,
                || self.common_source_directory_files(),
                self.get_current_directory(),
                self.use_case_sensitive_file_names(),
                Some(&mut check),
            )
        })
    }

    /// The value of `common_source_directory` without its TS6059 check, so
    /// no error is recorded. Not in Go: the port reads it where it copies a
    /// referenced project for the checker threads
    /// (`ProjectReferenceCopies::resolved`), which Go does not do. The check
    /// does not change the value. A checker's read of the copy records the
    /// errors later (`common_source_directory_read`).
    pub fn common_source_directory_unchecked(&self) -> String {
        if let Some(common_source_directory) = self.common_source_directory.get() {
            return common_source_directory.clone();
        }
        get_common_source_directory(
            &self.parsed_config.compiler_options,
            || self.common_source_directory_files(),
            self.get_current_directory(),
            self.use_case_sensitive_file_names(),
            None,
        )
    }

    // The `files` closure of Go CommonSourceDirectory
    // (tsoptions/parsedcommandline.go:159).
    fn common_source_directory_files(&self) -> Vec<String> {
        self.parsed_config
            .file_names
            .iter()
            .filter(|file| {
                !(self
                    .parsed_config
                    .compiler_options
                    .no_emit_for_js_files
                    .is_true()
                    && has_js_file_extension(file))
                    && !is_declaration_file_name(file)
            })
            .cloned()
            .collect()
    }

    // Go: tsoptions/parsedcommandline.go:204 (*ParsedCommandLine).checkSourceFilesBelongToPath
    pub fn check_source_files_belong_to_path(
        &self,
        source_files: &[String],
        root_directory: &str,
    ) -> bool {
        let mut all_files_belong_to_path = true;
        for file in source_files {
            let absolute_source_file_path = get_canonical_file_name(
                &get_normalized_absolute_path(file, self.get_current_directory()),
                self.use_case_sensitive_file_names(),
            );
            if !contains_path(root_directory, file, &self.compare_paths_options) {
                self.common_source_directory_errors.borrow_mut().push(new_compiler_diagnostic(
                    diag::File_0_is_not_under_rootDir_1_rootDir_is_expected_to_contain_all_source_files,
                    args![absolute_source_file_path, root_directory],
                ));
                all_files_belong_to_path = false;
            }
        }

        all_files_belong_to_path
    }

    // Go: tsoptions/parsedcommandline.go:189 (*ParsedCommandLine).GetCurrentDirectory (at 673a5f17d713;
    // ts#64159 makes it BaseDirectory, tsoptions/parsedcommandline.go:216)
    pub fn get_current_directory(&self) -> &str {
        &self.compare_paths_options.current_directory
    }

    // Go: tsoptions/parsedcommandline.go:193 (*ParsedCommandLine).UseCaseSensitiveFileNames (at 673a5f17d713;
    // ts#64159 makes it CaseSensitivity, tsoptions/parsedcommandline.go:220)
    pub fn use_case_sensitive_file_names(&self) -> bool {
        self.compare_paths_options.use_case_sensitive_file_names
    }

    // Go: tsoptions/parsedcommandline.go:224 (*ParsedCommandLine).getOutputDeclarationAndSourceFileNames
    // PORT: Go `iter.Seq2[dtsName, inputName]` is an eager `Vec` of pairs.
    pub fn get_output_declaration_and_source_file_names(&self) -> Vec<(String, String)> {
        let mut result = Vec::new();
        for file_name in &self.parsed_config.file_names {
            let mut output_dts = String::new();
            if !is_declaration_file_name(file_name) && !file_extension_is(file_name, EXTENSION_JSON)
            {
                output_dts = get_output_declaration_file_name_worker(
                    file_name,
                    &self.parsed_config.compiler_options,
                    self,
                );
            }
            result.push((output_dts, file_name.clone()));
        }
        result
    }

    // Go: tsoptions/parsedcommandline.go:238 (*ParsedCommandLine).GetOutputFileNames
    // PORT: Go `iter.Seq[string]` is a lazy iterator, and so is this: each
    // name is made when the caller asks for it. A caller that stops early
    // (the `-b` up-to-date check, `FirstOrNilSeq`) makes no later name, so
    // it does not call `CommonSourceDirectory` (and record its TS6059 errors)
    // where Go does not.
    pub fn get_output_file_names(&self) -> impl Iterator<Item = String> + '_ {
        let opts = &self.parsed_config.compiler_options;
        self.parsed_config
            .file_names
            .iter()
            .filter(|file_name| !is_declaration_file_name(file_name))
            .flat_map(move |file_name| {
                let is_json = file_extension_is(file_name, EXTENSION_JSON);
                let js =
                    std::iter::once_with(move || get_output_js_file_name(file_name, opts, self))
                        .filter(|js_file_name| !js_file_name.is_empty())
                        .flat_map(move |js_file_name| {
                            let source_map = if is_json {
                                String::new()
                            } else {
                                get_source_map_file_path(&js_file_name, opts)
                            };
                            std::iter::once(js_file_name)
                                .chain((!source_map.is_empty()).then_some(source_map))
                        });
                let dts = std::iter::once_with(move || {
                    if is_json || !opts.get_emit_declarations() {
                        return String::new();
                    }
                    get_output_declaration_file_name_worker(file_name, opts, self)
                })
                .filter(|dts_file_name| !dts_file_name.is_empty())
                .flat_map(move |dts_file_name| {
                    // tsgo#4712: no declaration map for a content-mapped file.
                    let declaration_map =
                        (self.get_content_mapper_for_file_name(file_name).is_none()
                            && opts.get_are_declaration_maps_enabled())
                        .then(|| format!("{dts_file_name}.map"));
                    std::iter::once(dts_file_name).chain(declaration_map)
                });
                js.chain(dts)
            })
    }

    // Go: tsoptions/parsedcommandline.go:278 (*ParsedCommandLine).GetBuildInfoFileName
    pub fn get_build_info_file_name(&self) -> String {
        get_build_info_file_name(
            &self.parsed_config.compiler_options,
            &self.compare_paths_options,
        )
    }

    // Go: tsoptions/parsedcommandline.go:283 (*ParsedCommandLine).WildcardDirectories
    /// Returns the cached wildcard directories, initializing them if needed.
    /// PORT: Go dereferences `ConfigFile.configFileSpecs` and panics when
    /// either is nil; `expect` does the same.
    pub fn wildcard_directories(&self) -> &FxHashMap<String, bool> {
        self.wildcard_directories.get_or_init(|| {
            let specs = self
                .config_file
                .as_ref()
                .and_then(|c| c.config_file_specs.as_ref())
                .expect("nil pointer dereference: ConfigFile.configFileSpecs");
            get_wildcard_directories(
                &specs.validated_include_specs,
                &specs.validated_exclude_specs,
                &self.compare_paths_options,
            )
        })
    }

    // Go: tsoptions/parsedcommandline.go:303 (*ParsedCommandLine).WildcardDirectoryGlobs
    // PORT: Go returns nil when `WildcardDirectories` is a nil map. The Rust
    // map is empty there, which gives an empty list; the caller only tests
    // the length and the members. Go builds the list in map order, which
    // only changes which glob matches first.
    pub fn wildcard_directory_globs(&self) -> &[Glob] {
        let wildcard_directories = self.wildcard_directories();
        self.include_globs.get_or_init(|| {
            // tsgo#4712
            let (file_glob, recursive_file_glob) = self.file_glob_patterns();
            let mut globs = Vec::with_capacity(wildcard_directories.len());
            for (dir, recursive) in wildcard_directories {
                let pattern = if *recursive {
                    &recursive_file_glob
                } else {
                    &file_glob
                };
                if let Ok(parsed) = glob_parse(&format!("{}/{}", normalize_path(dir), pattern)) {
                    globs.push(parsed);
                }
            }
            globs
        })
    }

    // Go: tsoptions/parsedcommandline.go:327 (*ParsedCommandLine).LiteralFileNames
    /// Normalized file names explicitly specified in `files`
    /// PORT: Go returns nil without a config file; that is an empty slice.
    pub fn literal_file_names(&self) -> &[String] {
        if self.config_file.is_some() {
            return &self.file_names()[0..self.literal_file_names_len as usize];
        }
        &[]
    }

    // Go: tsoptions/parsedcommandline.go:334 (*ParsedCommandLine).SetParsedOptions
    pub fn set_parsed_options(&mut self, o: ParsedOptions) {
        self.parsed_config = o;
    }

    // Go: tsoptions/parsedcommandline.go:338 (*ParsedCommandLine).SetCompilerOptions
    pub fn set_compiler_options(&mut self, o: Rc<CompilerOptions>) {
        self.parsed_config.compiler_options = o;
    }

    // Go: tsoptions/parsedcommandline.go:342 (*ParsedCommandLine).CompilerOptions
    pub fn compiler_options(&self) -> &Rc<CompilerOptions> {
        &self.parsed_config.compiler_options
    }

    // Go: tsoptions/parsedcommandline.go:349 (*ParsedCommandLine).SetTypeAcquisition
    pub fn set_type_acquisition(&mut self, o: Option<TypeAcquisition>) {
        self.parsed_config.type_acquisition = o;
    }

    // Go: tsoptions/parsedcommandline.go:353 (*ParsedCommandLine).TypeAcquisition
    pub fn type_acquisition(&self) -> Option<&TypeAcquisition> {
        self.parsed_config.type_acquisition.as_ref()
    }

    // Go: tsoptions/parsedcommandline.go:358 (*ParsedCommandLine).FileNames
    /// All file names matched by files, include, and exclude patterns
    pub fn file_names(&self) -> &[String] {
        &self.parsed_config.file_names
    }

    // Go: tsoptions/parsedcommandline.go:362 (*ParsedCommandLine).RootFileNameForDiagnostic (ts#64159)
    /// The text of root file `index` in the root file diagnostics: the name
    /// as the command line gave it, else the rooted file name.
    pub fn root_file_name_for_diagnostic(&self, index: usize) -> &str {
        match self.root_file_names_for_diagnostics.get(index) {
            Some(name) => name,
            None => &self.parsed_config.file_names[index],
        }
    }

    // Go: tsoptions/parsedcommandline.go:334 (*ParsedCommandLine).FileNamesByPath (at 673a5f17d713;
    // ts#64159 makes it FilePaths, tsoptions/parsedcommandline.go:369)
    pub fn file_names_by_path(&self) -> &FxHashMap<Path, String> {
        self.file_names_by_path.get_or_init(|| {
            let mut file_names_by_path = FxHashMap::with_capacity_and_hasher(
                self.parsed_config.file_names.len(),
                Default::default(),
            );
            for file_name in &self.parsed_config.file_names {
                let path = to_path(
                    file_name,
                    self.get_current_directory(),
                    self.use_case_sensitive_file_names(),
                );
                file_names_by_path.insert(path, file_name.clone());
            }
            file_names_by_path
        })
    }

    // Go: tsoptions/parsedcommandline.go:380 (*ParsedCommandLine).ProjectReferences
    /// PORT: a nil Go slice is an empty slice here. Use
    /// `has_project_references` for Go `ProjectReferences() != nil`.
    pub fn project_references(&self) -> &[ProjectReference] {
        self.parsed_config
            .project_references
            .as_deref()
            .unwrap_or_default()
    }

    /// Go `ProjectReferences() != nil`: the config has a `references` list,
    /// which can be empty.
    pub fn has_project_references(&self) -> bool {
        self.parsed_config.project_references.is_some()
    }

    // Go: tsoptions/parsedcommandline.go:384 (*ParsedCommandLine).ContentMappers (tsgo#4712)
    pub fn content_mappers(&self) -> &[Rc<Mapper>] {
        &self.parsed_config.content_mappers
    }

    // Go: tsoptions/parsedcommandline.go:393 (*ParsedCommandLine).ContentMapperExtensions (tsgo#4712)
    // ContentMapperExtensions returns the flattened list of file extensions registered by the
    // config's content mappers.
    // PORT: Go builds a new slice on each call (`core.FlatMap`).
    pub fn content_mapper_extensions(&self) -> Vec<String> {
        self.content_mappers()
            .iter()
            .flat_map(|m| m.definition.extensions.iter().cloned())
            .collect()
    }

    // Go: tsoptions/parsedcommandline.go:401 (*ParsedCommandLine).GetContentMapperForFileName (tsgo#4712)
    // GetContentMapperForFileName returns the configured content mapper whose extensions include fileName,
    // or nil if no content mapper is registered for the file's extension.
    // PORT: Go nil is `None`.
    pub fn get_content_mapper_for_file_name(&self, file_name: &str) -> Option<Rc<Mapper>> {
        let ignore_case = !self.use_case_sensitive_file_names();
        let extension = get_longest_extension_from_path(
            file_name,
            &self.content_mapper_extensions(),
            ignore_case,
        );
        for mapper in self.content_mappers() {
            if mapper.definition.extensions.iter().any(|mapper_extension| {
                extension == *mapper_extension
                    || ignore_case && equate_string_case_insensitive(&extension, mapper_extension)
            }) {
                return Some(mapper.clone());
            }
        }
        None
    }

    // Go: tsoptions/parsedcommandline.go:414 (*ParsedCommandLine).ResolvedProjectReferencePaths
    pub fn resolved_project_reference_paths(&self) -> &[String] {
        self.resolved_project_reference_paths.get_or_init(|| {
            self.project_references()
                .iter()
                .map(resolve_project_reference_path)
                .collect()
        })
    }

    // Go: tsoptions/parsedcommandline.go:421 (*ParsedCommandLine).ExtendedSourceFiles
    /// PORT: Go returns nil without a config file; that is an empty slice.
    pub fn extended_source_files(&self) -> &[String] {
        match &self.config_file {
            None => &[],
            Some(config_file) => &config_file.extended_source_files,
        }
    }

    // Go: tsoptions/parsedcommandline.go:428 (*ParsedCommandLine).GetConfigFileParsingDiagnostics
    // PORT: Go `p.Errors` is `errors_with_common_source_directory_errors`.
    pub fn get_config_file_parsing_diagnostics(&self) -> Vec<Diagnostic> {
        if let Some(config_file) = &self.config_file {
            // todo: !!! should be ConfigFile.ParseDiagnostics, check if they are the same
            let mut result = parsed_source_file_diagnostics(config_file.source_file).to_vec();
            result.extend(self.errors_with_common_source_directory_errors());
            return result;
        }
        self.errors_with_common_source_directory_errors()
    }

    /// Go `p.Errors` as it is now: `errors`, then the TS6059 errors that
    /// `CommonSourceDirectory` appended (Go appends them to `Errors`, so they
    /// come last). Go reads `p.Errors` when it is called, so a reader after
    /// `CommonSourceDirectory` sees them and a reader before it does not.
    /// A checker's call through a copy (`common_source_directory_read`)
    /// counts as a call: its errors are recorded here first.
    pub fn errors_with_common_source_directory_errors(&self) -> Vec<Diagnostic> {
        if self.common_source_directory_read.load(Ordering::Acquire) {
            self.common_source_directory();
        }
        let mut errors = self.errors.clone();
        errors.extend(self.common_source_directory_errors.borrow().iter().cloned());
        errors
    }

    // Go: tsoptions/parsedcommandline.go:438 (*ParsedCommandLine).PossiblyMatchesFileName
    /// A fast check to see if a file is currently included by a config
    /// or would be included if the file were to be created. It may return false positives.
    pub fn possibly_matches_file_name(&self, file_name: &str) -> bool {
        let path = to_path(
            file_name,
            self.get_current_directory(),
            self.use_case_sensitive_file_names(),
        );
        if self.file_names_by_path().contains_key(&path) {
            return true;
        }

        let specs = self
            .config_file
            .as_ref()
            .and_then(|c| c.config_file_specs.as_ref())
            .expect("nil pointer dereference: ConfigFile.configFileSpecs");
        for include in &specs.validated_include_specs {
            if !include.contains(['*', '?']) && !is_implicit_glob(include) {
                let include_path = to_path(
                    include,
                    self.get_current_directory(),
                    self.use_case_sensitive_file_names(),
                );
                if include_path == path {
                    return true;
                }
            }
        }
        // tsgo#4712
        if self.get_content_mapper_for_file_name(file_name).is_some() {
            let directory_path = path.get_directory_path();
            if self.possibly_matches_directory_name(&directory_path) {
                return true;
            }
        }
        let wildcard_directory_globs = self.wildcard_directory_globs();
        if !wildcard_directory_globs.is_empty() {
            for glob in wildcard_directory_globs {
                if glob.match_(file_name) {
                    return true;
                }
            }
        }
        false
    }

    // Go: tsoptions/parsedcommandline.go:470 (*ParsedCommandLine).PossiblyMatchesDirectoryName
    pub fn possibly_matches_directory_name(&self, directory_path: &Path) -> bool {
        for (wildcard_dir, recursive) in self.wildcard_directories() {
            let wildcard_dir_path = to_path(
                wildcard_dir,
                self.get_current_directory(),
                self.use_case_sensitive_file_names(),
            );
            if *recursive {
                if wildcard_dir_path.contains_path(directory_path) {
                    return true;
                }
            } else if wildcard_dir_path == *directory_path {
                return true;
            }
        }
        false
    }

    // Go: tsoptions/parsedcommandline.go:486 (*ParsedCommandLine).GetMatchedFileSpec
    pub fn get_matched_file_spec(&self, file_name: &str) -> String {
        self.config_file_specs()
            .get_matched_file_spec(file_name, &self.compare_paths_options)
    }

    // Go: tsoptions/parsedcommandline.go:490 (*ParsedCommandLine).GetMatchedIncludeSpec
    pub fn get_matched_include_spec(&self, file_name: &str) -> (String, bool) {
        let specs = self.config_file_specs();
        if specs.validated_include_specs.is_empty() {
            return (String::new(), false);
        }

        if specs.is_default_include_spec {
            return (specs.validated_include_specs[0].clone(), true);
        }

        (
            specs.get_matched_include_spec(file_name, &self.compare_paths_options),
            false,
        )
    }

    // PORT: Go reads `p.ConfigFile.configFileSpecs` and panics when either
    // pointer is nil. This helper does that read once.
    fn config_file_specs(&self) -> &ConfigFileSpecs {
        self.config_file
            .as_ref()
            .and_then(|c| c.config_file_specs.as_ref())
            .expect("nil pointer dereference: ConfigFile.configFileSpecs")
    }

    // Go: tsoptions/parsedcommandline.go:503 (*ParsedCommandLine).ReloadFileNamesOfParsedCommandLine
    // PORT: Go copies the cached `wildcardDirectories` map pointer and the
    // `includeGlobs` slice; this clones both cache cells. Go `p.Errors`
    // holds the common source directory errors so far, so the copy's
    // `errors` do.
    pub fn reload_file_names_of_parsed_command_line(&self, fs: &dyn Fs) -> ParsedCommandLine {
        let mut parsed_config = self.parsed_config.clone();
        let (file_names, literal_file_names_len) = get_file_names_from_config_specs(
            self.config_file_specs(),
            self.get_current_directory(),
            Some(&**self.compiler_options()),
            fs,
            &self.content_mapper_extensions(),
        );
        parsed_config.file_names = file_names;
        ParsedCommandLine {
            parsed_config,
            config_file: self.config_file.clone(),
            errors: self.errors_with_common_source_directory_errors(),
            raw: self.raw.clone(),
            compile_on_save: self.compile_on_save,
            compare_paths_options: self.compare_paths_options.clone(),
            wildcard_directories: self.wildcard_directories.clone(),
            include_globs: self.include_globs.clone(),
            literal_file_names_len,
            root_file_names_for_diagnostics: self.root_file_names_for_diagnostics.clone(),
            ..Default::default()
        }
    }

    // Go: tsoptions/parsedcommandline.go:530 (*ParsedCommandLine).Locale
    // PORT: Go returns the `Locale` value; this returns a clone of the
    // cached value.
    pub fn locale(&self) -> crate::locale::Locale {
        self.locale
            .get_or_init(|| {
                let (locale, _) = crate::locale::parse(&self.compiler_options().locale);
                locale
            })
            .clone()
    }
}

// Go: `*ParsedCommandLine` implements `module.ResolvedProjectReference`.
impl ModuleResolvedProjectReference for ParsedCommandLine {
    fn config_name(&self) -> &str {
        ParsedCommandLine::config_name(self)
    }
    fn compiler_options(&self) -> Option<Rc<CompilerOptions>> {
        Some(ParsedCommandLine::compiler_options(self).clone())
    }
}

// Go: `*ParsedCommandLine` implements `outputpaths.OutputPathsHost`.
impl OutputPathsHost for ParsedCommandLine {
    fn common_source_directory(&self) -> String {
        ParsedCommandLine::common_source_directory(self).to_string()
    }
    fn get_current_directory(&self) -> String {
        ParsedCommandLine::get_current_directory(self).to_string()
    }
    fn use_case_sensitive_file_names(&self) -> bool {
        ParsedCommandLine::use_case_sensitive_file_names(self)
    }
    // tsgo#4712
    fn content_mapper_extensions(&self) -> Vec<String> {
        ParsedCommandLine::content_mapper_extensions(self)
    }
}

#[cfg(test)]
mod glob_tests {
    use super::*;

    // Expected values come from the pinned Go `internal/glob`.
    #[test]
    fn wildcard_directory_patterns_match_like_go() {
        let flat = glob_parse(&format!("/a/b/{FILE_GLOB_PATTERN}")).unwrap();
        let deep = glob_parse(&format!("/a/b/{RECURSIVE_FILE_GLOB_PATTERN}")).unwrap();
        assert_eq!(
            flat.to_string(),
            "/a/b/*.{js,jsx,mjs,cjs,ts,tsx,mts,cts,json}"
        );
        let cases = [
            ("/a/b/c.ts", true, true),
            ("/a/b/c/d.ts", false, true),
            ("/a/b/c.d.ts", true, true),
            ("/a/b/c.tsx", true, true),
            ("/a/b/c.css", false, false),
            ("/a//b/c.ts", true, true),
            ("/a/b/.ts", true, true),
            ("/a/b/c/d/e.json", false, true),
        ];
        for (input, in_flat, in_deep) in cases {
            assert_eq!(flat.match_(input), in_flat, "{input}");
            assert_eq!(deep.match_(input), in_deep, "{input}");
        }
    }

    #[test]
    fn parse_errors_and_elements_match_go() {
        assert_eq!(
            glob_parse("a**").unwrap_err(),
            "** may only be adjacent to '/'"
        );
        assert_eq!(glob_parse("{a").unwrap_err(), "unmatched '{'");
        assert_eq!(glob_parse("[a").unwrap_err(), GLOB_ERR_BAD_RANGE);
        let m = |pattern: &str, input: &str| glob_parse(pattern).unwrap().match_(input);
        assert!(m("a/[0-9]x", "a/5x"));
        assert!(!m("a/[0-9]x", "a/xx"));
        assert!(m("a/?", "a/b"));
        assert!(m("{a,b/c}d", "ad"));
        assert!(m("{a,b/c}d", "b/cd"));
        assert!(m("**/x", "x"));
        assert!(m("**/x", "q/r/x"));
        assert!(m("a/**", "a/b/c"));
        assert!(!m("*.ts", "a/b"));
        // Go ignores the negation when matching.
        assert!(m("[!a-c]", "b"));
        assert!(!m("[!a-c]", "é"));
    }
}

#[cfg(test)]
mod common_source_directory_tests {
    use super::*;

    fn command_line(options: CompilerOptions) -> ParsedCommandLine {
        new_parsed_command_line(
            Rc::new(options),
            vec!["/p/src/a.ts".to_string(), "/p/other/b.ts".to_string()],
            None,
            ComparePathsOptions {
                use_case_sensitive_file_names: true,
                current_directory: "/p".to_string(),
            },
        )
    }

    fn codes(command_line: &ParsedCommandLine) -> Vec<i32> {
        command_line
            .get_config_file_parsing_diagnostics()
            .iter()
            .map(Diagnostic::code)
            .collect()
    }

    // Go: tsoptions/parsedcommandline.go:204 checkSourceFilesBelongToPath
    // appends TS6059 to `p.Errors`, which GetConfigFileParsingDiagnostics
    // (:393) and WithFileNames (:93) read. GetOutputFileNames (:211) is
    // lazy: only a name under outDir or declarationDir reads
    // CommonSourceDirectory (outputpaths.go:166).
    #[test]
    fn common_source_directory_errors_are_config_errors_like_go() {
        let out_dir = command_line(CompilerOptions {
            root_dir: "/p/src".to_string(),
            out_dir: "/p/out".to_string(),
            ..Default::default()
        });
        assert!(codes(&out_dir).is_empty());
        assert_eq!(
            out_dir.get_output_file_names().next().as_deref(),
            Some("/p/out/a.js")
        );
        assert_eq!(codes(&out_dir), [6059]);
        assert_eq!(codes(&out_dir.with_file_names(Vec::new())), [6059]);

        // The first name (a.js next to a.ts) does not read the directory.
        let declaration_dir = command_line(CompilerOptions {
            root_dir: "/p/src".to_string(),
            declaration: Tristate::True,
            declaration_dir: "/p/types".to_string(),
            ..Default::default()
        });
        assert_eq!(
            declaration_dir.get_output_file_names().next().as_deref(),
            Some("/p/src/a.js")
        );
        assert!(codes(&declaration_dir).is_empty());
        assert_eq!(
            declaration_dir.get_output_file_names().collect::<Vec<_>>(),
            [
                "/p/src/a.js",
                "/p/types/a.d.ts",
                "/p/other/b.js",
                "/p/other/b.d.ts"
            ]
        );
        assert_eq!(codes(&declaration_dir), [6059]);
    }
}
