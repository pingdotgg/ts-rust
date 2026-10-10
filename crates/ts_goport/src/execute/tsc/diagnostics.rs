//! Go: execute/tsc/diagnostics.go (diagnostic, error summary and status
//! reporters), with the `diagnosticwriter` pieces they call. The help and
//! version printers (execute/tsc/help.go) are in help.rs.
//!
//! PORT: Go `diagnosticwriter.Diagnostic` is an interface. Every Go caller
//! of these writers passes an `*ast.Diagnostic` wrapped in
//! `diagnosticwriter.ASTDiagnostic` (`WrapASTDiagnostic`,
//! `FromASTDiagnostics`). The writers here take the `Diagnostic` and wrap it
//! themselves (`AstDiagnostic`), so a content-mapped file (tsgo#4712) is
//! shown as Go shows it.

use crate::prelude::*;

use std::borrow::Cow;
use std::hash::{Hash, Hasher};
use std::sync::OnceLock;
use std::time::SystemTime;

use super::compile::{System, Writer, write_str};
// PORT: testing (the status reporters)
use super::compile::CommandLineTesting;
use crate::diagnostics::Category;
use crate::diagnostics_loc::message_localize;
use crate::frontend::tspath::{ComparePathsOptions, convert_to_relative_path, path_is_absolute};
use crate::locale::Locale;

// Go: diagnosticwriter/diagnosticwriter.go:203 FormattingOptions
#[derive(Clone, Debug, Default)]
pub struct FormattingOptions {
    pub new_line: String,
    pub compare_paths_options: ComparePathsOptions,
    pub locale: Locale,
}

// Go: execute/tsc/diagnostics.go:14 getFormatOptsOfSys
fn get_format_opts_of_sys(sys: &dyn System, locale: &Locale) -> FormattingOptions {
    FormattingOptions {
        new_line: "\n".to_string(),
        compare_paths_options: ComparePathsOptions {
            current_directory: sys.get_current_directory(),
            use_case_sensitive_file_names: sys.fs().use_case_sensitive_file_names(),
        },
        locale: locale.clone(),
    }
}

// Go: execute/tsc/diagnostics.go:23 DiagnosticReporter
pub type DiagnosticReporter = Rc<dyn Fn(&Diagnostic)>;

// Go: execute/tsc/diagnostics.go:25 QuietDiagnosticReporter
pub fn quiet_diagnostic_reporter() -> DiagnosticReporter {
    Rc::new(|_diagnostic: &Diagnostic| {})
}

// Go: execute/tsc/diagnostics.go:27 CreateDiagnosticReporter
pub fn create_diagnostic_reporter(
    sys: &dyn System,
    w: Writer,
    locale: &Locale,
    options: &CompilerOptions,
) -> DiagnosticReporter {
    if options.quiet.is_true() {
        return quiet_diagnostic_reporter();
    }
    let format_opts = get_format_opts_of_sys(sys, locale);
    if should_be_pretty(sys, Some(options)) {
        return Rc::new(move |diagnostic: &Diagnostic| {
            format_diagnostic_with_color_and_context(&w, diagnostic, &format_opts);
            write_str(&w, &format_opts.new_line);
        });
    }
    Rc::new(move |diagnostic: &Diagnostic| {
        write_format_diagnostic(&w, diagnostic, &format_opts);
    })
}

// Go: execute/tsc/diagnostics.go:43 defaultIsPretty (ts#63941)
fn default_is_pretty(sys: &dyn System) -> bool {
    let (force_color, ok) = sys.get_environment_variable("FORCE_COLOR");
    if ok {
        return matches!(force_color.as_str(), "" | "1" | "2" | "3" | "true");
    }
    let (no_color, _) = sys.get_environment_variable("NO_COLOR");
    if !no_color.is_empty() {
        return false;
    }
    let (term, _) = sys.get_environment_variable("TERM");
    if term == "dumb" {
        return false;
    }
    sys.write_output_is_tty()
}

// Go: execute/tsc/diagnostics.go:61 shouldBePretty
pub fn should_be_pretty(sys: &dyn System, options: Option<&CompilerOptions>) -> bool {
    match options {
        Some(options) if !options.pretty.is_unknown() => options.pretty.is_true(),
        _ => default_is_pretty(sys),
    }
}

// Go: execute/tsc/diagnostics.go:68 colors
#[derive(Clone, Debug, Default)]
pub struct Colors {
    show_colors: bool,

    is_windows: bool,
    is_windows_terminal: bool,
    is_vs_code: bool,
    supports_richer_colors: bool,
}

// Go: execute/tsc/diagnostics.go:77 createColors (ts#63941)
pub fn create_colors(sys: &dyn System) -> Colors {
    if !default_is_pretty(sys) {
        return Colors {
            show_colors: false,
            ..Colors::default()
        };
    }

    let (os, _) = sys.get_environment_variable("OS");
    let is_windows = os.to_lowercase().contains("windows");
    let (wt_session, _) = sys.get_environment_variable("WT_SESSION");
    let (term_program, _) = sys.get_environment_variable("TERM_PROGRAM");
    let (color_term, _) = sys.get_environment_variable("COLORTERM");
    let (term, _) = sys.get_environment_variable("TERM");

    Colors {
        show_colors: true,
        is_windows,
        is_windows_terminal: !wt_session.is_empty(),
        is_vs_code: term_program == "vscode",
        supports_richer_colors: color_term == "truecolor" || term == "xterm-256color",
    }
}

impl Colors {
    // Go: execute/tsc/diagnostics.go:98 (*colors).bold
    pub fn bold(&self, str: &str) -> String {
        if !self.show_colors {
            return str.to_string();
        }
        format!("\x1b[1m{str}\x1b[22m")
    }

    // Go: execute/tsc/diagnostics.go:105 (*colors).blue
    pub fn blue(&self, str: &str) -> String {
        if !self.show_colors {
            return str.to_string();
        }

        // Effectively Powershell and Command prompt users use cyan instead
        // of blue because the default theme doesn't show blue with enough contrast.
        if self.is_windows && !self.is_windows_terminal && !self.is_vs_code {
            return self.bright_white(str);
        }
        format!("\x1b[94m{str}\x1b[39m")
    }

    // Go: execute/tsc/diagnostics.go:118 (*colors).blueBackground
    pub fn blue_background(&self, str: &str) -> String {
        if !self.show_colors {
            return str.to_string();
        }
        if self.supports_richer_colors {
            format!("\x1B[48;5;68m{str}\x1B[39;49m")
        } else {
            format!("\x1b[44m{str}\x1B[39;49m")
        }
    }

    // Go: execute/tsc/diagnostics.go:129 (*colors).brightWhite
    pub fn bright_white(&self, str: &str) -> String {
        if !self.show_colors {
            return str.to_string();
        }
        format!("\x1b[97m{str}\x1b[39m")
    }
}

// Go: execute/tsc/diagnostics.go:136 DiagnosticsReporter
pub type DiagnosticsReporter = Rc<dyn Fn(&[Diagnostic])>;

// Go: execute/tsc/diagnostics.go:138 QuietDiagnosticsReporter
pub fn quiet_diagnostics_reporter() -> DiagnosticsReporter {
    Rc::new(|_diagnostics: &[Diagnostic]| {})
}

// Go: execute/tsc/diagnostics.go:140 CreateReportErrorSummary
// PORT: Go reads `sys.Writer()` on each report. The reporter cannot keep
// `sys`, so it reads the writer when it is made. A system's writer does
// not change after the system is made.
pub fn create_report_error_summary(
    sys: &dyn System,
    locale: &Locale,
    options: Option<&CompilerOptions>,
) -> DiagnosticsReporter {
    if should_be_pretty(sys, options) {
        let format_opts = get_format_opts_of_sys(sys, locale);
        let writer = sys.writer();
        return Rc::new(move |diagnostics: &[Diagnostic]| {
            write_error_summary_text(&writer, diagnostics, &format_opts);
        });
    }
    quiet_diagnostics_reporter()
}

// Go: execute/tsc/diagnostics.go:150 CreateBuilderStatusReporter
// PORT: Go `options` can be nil only through `shouldBePretty`; the quiet
// check reads it, so it is required here.
pub fn create_builder_status_reporter(
    sys: Rc<dyn System>,
    w: Writer,
    locale: &Locale,
    options: &CompilerOptions,
    testing: Option<Rc<dyn CommandLineTesting>>,
) -> DiagnosticReporter {
    if options.quiet.is_true() {
        return quiet_diagnostic_reporter();
    }

    let format_opts = get_format_opts_of_sys(sys.as_ref(), locale);
    let write_status: fn(&Writer, &str, &Diagnostic, &FormattingOptions) =
        if should_be_pretty(sys.as_ref(), Some(options)) {
            format_diagnostics_status_with_color_and_time
        } else {
            format_diagnostics_status_and_time
        };
    Rc::new(move |diagnostic: &Diagnostic| {
        // PORT: testing. Go `defer testing.OnBuildStatusReportEnd(w)`.
        if let Some(testing) = &testing {
            testing.on_build_status_report_start(&w);
        }
        write_status(&w, &format_status_time(sys.now()), diagnostic, &format_opts);
        write_str(
            &w,
            &format!("{}{}", format_opts.new_line, format_opts.new_line),
        );
        if let Some(testing) = &testing {
            testing.on_build_status_report_end(&w);
        }
    })
}

// Go: execute/tsc/diagnostics.go:168 CreateWatchStatusReporter
pub fn create_watch_status_reporter(
    sys: Rc<dyn System>,
    locale: &Locale,
    options: Rc<CompilerOptions>,
    testing: Option<Rc<dyn CommandLineTesting>>,
) -> DiagnosticReporter {
    let format_opts = get_format_opts_of_sys(sys.as_ref(), locale);
    let write_status: fn(&Writer, &str, &Diagnostic, &FormattingOptions) =
        if should_be_pretty(sys.as_ref(), Some(&options)) {
            format_diagnostics_status_with_color_and_time
        } else {
            format_diagnostics_status_and_time
        };
    Rc::new(move |diagnostic: &Diagnostic| {
        let writer = sys.writer();
        // PORT: testing. Go `defer testing.OnWatchStatusReportEnd()`.
        if let Some(testing) = &testing {
            testing.on_watch_status_report_start();
        }
        try_clear_screen(&writer, diagnostic, &options);
        write_status(
            &writer,
            &format_status_time(sys.now()),
            diagnostic,
            &format_opts,
        );
        write_str(
            &writer,
            &format!("{}{}", format_opts.new_line, format_opts.new_line),
        );
        if let Some(testing) = &testing {
            testing.on_watch_status_report_end();
        }
    })
}

/// Go `sys.Now().Format("03:04:05 PM")`. Go `time.Now()` is in `time.Local`.
// Go (go1.27.1, the oracle toolchain): time/format.go:667
// (Time).appendFormat, the stdZeroHour12 (:756), stdZeroMinute (:765),
// stdZeroSecond (:769) and stdPM (:771) cases.
// PORT: jiff converts the time to the zone's civil time (Go
// `Time.locabs`). A time outside the jiff range (years -9999 to 9999) is
// not ported.
pub fn format_status_time(now: SystemTime) -> String {
    let Ok(timestamp) = jiff::Timestamp::try_from(now) else {
        unported!("Time.Format of a time outside years -9999 to 9999");
    };
    let datetime = local_location().to_datetime(timestamp);
    let hour = datetime.hour();
    // Noon is 12PM, midnight is 12AM.
    let mut hr = hour % 12;
    if hr == 0 {
        hr = 12;
    }
    let pm = if hour >= 12 { "PM" } else { "AM" };
    format!(
        "{hr:02}:{:02}:{:02} {pm}",
        datetime.minute(),
        datetime.second()
    )
}

// ---------------------------------------------------------------------------
// Go time.Local (go1.27.1 time/zoneinfo_unix.go and zoneinfo_windows.go),
// for the status clock
// ---------------------------------------------------------------------------
// PORT: Go `time.Location` is a `jiff::tz::TimeZone`. On Unix, Go parses
// zone files with `LoadLocationFromTZData`; jiff parses the same TZif data
// (the transitions and the POSIX TZ footer) with `TimeZone::tzif`. Windows
// has its own `init_local` (below).

// Go: time/zoneinfo.go:88 localLoc, :89 localOnce, :91 (*Location).get
static LOCAL_LOC: OnceLock<jiff::tz::TimeZone> = OnceLock::new();

/// Go `time.Local`.
pub(crate) fn local_location() -> &'static jiff::tz::TimeZone {
    LOCAL_LOC.get_or_init(init_local)
}

// Go: time/zoneinfo_unix.go:21 platformZoneSources
// Many systems use /usr/share/zoneinfo, Solaris 2 has
// /usr/share/lib/zoneinfo, IRIX 6 has /usr/lib/locale/TZ,
// NixOS has /etc/zoneinfo.
const PLATFORM_ZONE_SOURCES: &[&str] = &[
    "/usr/share/zoneinfo/",
    "/usr/share/lib/zoneinfo/",
    "/usr/lib/locale/TZ/",
    "/etc/zoneinfo",
];

// Go: time/zoneinfo_unix.go:28 initLocal
// PORT: Go `syscall.Getenv` gives the raw bytes of the value; so does
// `as_encoded_bytes` on Unix.
#[cfg(not(windows))]
fn init_local() -> jiff::tz::TimeZone {
    // consult $TZ to find the time zone to use.
    // no $TZ means use the system default /etc/localtime.
    // $TZ="" means use UTC.
    // $TZ="foo" or $TZ=":foo" if foo is an absolute path, then the file pointed
    // by foo will be used to initialize timezone; otherwise, file
    // /usr/share/zoneinfo/foo will be used.

    let tz = std::env::var_os("TZ");
    match tz.as_ref().map(|tz| tz.as_encoded_bytes()) {
        None => {
            if let Some(z) = load_location(b"localtime", &["/etc"]) {
                return z;
            }
        }
        Some(tz) if !tz.is_empty() => {
            let tz = tz.strip_prefix(b":").unwrap_or(tz);
            if tz.first() == Some(&b'/') {
                if let Some(z) = load_location(tz, &[""]) {
                    return z;
                }
            } else if !tz.is_empty() && tz != b"UTC" {
                if let Some(z) = load_location(tz, PLATFORM_ZONE_SOURCES) {
                    return z;
                }
            }
        }
        Some(_) => {}
    }

    // Fall back to UTC.
    jiff::tz::TimeZone::UTC
}

// Go: time/zoneinfo_windows.go:230 initLocal
// PORT: Go calls `GetTimeZoneInformation` and makes the zone from the
// Windows standard and daylight rules. jiff asks Windows for the zone key
// name (`GetDynamicTimeZoneInformation`), maps it to an IANA name (CLDR
// windowsZones) and takes that zone from the zone data in the binary. For a
// mapped zone, both give the same civil time now. On failure both fall back
// to UTC.
// DIVERGES: jiff reads `TZ` first; Go on Windows does not read it. The
// logger test `log_time_is_local_time` sets `TZ`, so on Windows it needs
// this. A Windows zone with no IANA name in jiff's table gives UTC, and jiff
// does not read the Windows setting that turns off daylight saving time.
// Go's exact path needs `unsafe` FFI, which the workspace forbids.
#[cfg(windows)]
fn init_local() -> jiff::tz::TimeZone {
    jiff::tz::TimeZone::try_system().unwrap_or(jiff::tz::TimeZone::UTC)
}

// Go: time/zoneinfo_read.go:531 loadLocation
// PORT: `initLocal` reads only whether it failed, so the Go first-error
// bookkeeping is dropped. After the sources Go tries the embedded
// `time/tzdata`, which tsgo does not import, and
// `runtime.GOROOT()/lib/time/zoneinfo.zip`. The port has no Go root, so
// that zip is not ported. It changes the result only for a zone name that
// no system source has.
fn load_location(name: &[u8], sources: &[&str]) -> Option<jiff::tz::TimeZone> {
    for source in sources {
        if let Some(zone_data) = load_tzinfo(name, source) {
            // Go: time/zoneinfo_read.go:118 LoadLocationFromTZData
            if let Ok(z) = jiff::tz::TimeZone::tzif(&String::from_utf8_lossy(name), &zone_data) {
                return Some(z);
            }
        }
    }
    None
}

// Go: time/zoneinfo_read.go:520 loadTzinfo
// Go: time/zoneinfo_read.go:367 loadTzinfoFromDirOrZip
// PORT: Go reads a source that ends in "tzdata" (Android) or ".zip" as an
// archive. No source in `initLocal` does, so only the directory case is
// ported.
fn load_tzinfo(name: &[u8], source: &str) -> Option<Vec<u8>> {
    let mut path = Vec::new();
    if !source.is_empty() {
        path.extend_from_slice(source.as_bytes());
        path.push(b'/');
    }
    path.extend_from_slice(name);
    read_file(&path)
}

// Go: time/zoneinfo_read.go:37 maxFileSize
const MAX_FILE_SIZE: usize = 10 << 20;

// Go: time/zoneinfo_read.go:575 readFile
// PORT: the path is raw bytes, as in Go. Go stops reading past
// `maxFileSize` and fails; this reads the file and then checks the size.
// Off unix a path must be UTF-8; another path fails, as a missing file.
#[cfg(not(target_family = "wasm"))]
fn read_file(name: &[u8]) -> Option<Vec<u8>> {
    #[cfg(unix)]
    let path = {
        use std::os::unix::ffi::OsStrExt;
        std::ffi::OsStr::from_bytes(name)
    };
    #[cfg(not(unix))]
    let path = std::str::from_utf8(name).ok()?;
    let data = std::fs::read(path).ok()?;
    if data.len() > MAX_FILE_SIZE {
        return None;
    }
    Some(data)
}

/// The wasm build reads no zone file: its hosts (npm/wasm/core.js) give the
/// module no preopened directory (`fd_prestat_get`), so `std::fs` cannot
/// open one, and `time.Local` is UTC, as in Go when no zone file is found.
/// This leaves the zone file parser out of the wasm module.
// PORT: not in Go.
#[cfg(target_family = "wasm")]
fn read_file(_name: &[u8]) -> Option<Vec<u8>> {
    None
}

// ---------------------------------------------------------------------------
// Go diagnosticwriter/diagnosticwriter.go (the pieces the reporters call)
// ---------------------------------------------------------------------------

// Go: diagnosticwriter/diagnosticwriter.go:21 FileLike
/// Go `diagnosticwriter.FileLike`: the file whose text a diagnostic is shown
/// against.
// PORT: the Go interface has three implementations here: the
// `*ast.SourceFile` (a source file node), the `originalTextFile` of a
// content-mapped file (tsgo#4712) and the `renamedFile` of a supplemental
// file (ts#63936). Go compares the interface values as pointers, so two
// values are equal when they are the same node or the same `Rc`.
#[derive(Clone)]
pub enum FileLike {
    Source(Node),
    Original(Rc<OriginalTextFile>),
    Renamed(Rc<RenamedFile>),
}

impl FileLike {
    /// Go `FileLike.FileName`.
    #[must_use]
    pub fn file_name(&self) -> &str {
        match self {
            FileLike::Source(file) => source_file_file_name(*file),
            FileLike::Original(file) => file.file_name,
            FileLike::Renamed(file) => file.file_name,
        }
    }

    /// Go `FileLike.Text`.
    #[must_use]
    pub fn text(&self) -> FileText {
        match self {
            FileLike::Source(file) => source_file_text(*file),
            FileLike::Original(file) => file.text.clone(),
            FileLike::Renamed(file) => source_file_text(file.file),
        }
    }

    /// Go `FileLike.ECMALineMap`.
    #[must_use]
    pub fn ecma_line_map(&self) -> LineMapRef<'_> {
        match self {
            FileLike::Source(file) => LineMapRef::File(get_ecma_line_starts(*file)),
            FileLike::Original(file) => LineMapRef::Original(&file.line_map),
            FileLike::Renamed(file) => LineMapRef::File(get_ecma_line_starts(file.file)),
        }
    }
}

/// The line map of a `FileLike`: the guard of a source file's map (it pins a
/// freeable file version, lsshells M3b) or the map of an original text file.
pub enum LineMapRef<'a> {
    File(FileRef<[i32]>),
    Original(&'a [i32]),
}

impl std::ops::Deref for LineMapRef<'_> {
    type Target = [i32];

    fn deref(&self) -> &[i32] {
        match self {
            LineMapRef::File(map) => map,
            LineMapRef::Original(map) => map,
        }
    }
}

impl From<Node> for FileLike {
    fn from(file: Node) -> Self {
        FileLike::Source(file)
    }
}

impl PartialEq for FileLike {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (FileLike::Source(a), FileLike::Source(b)) => a == b,
            (FileLike::Original(a), FileLike::Original(b)) => Rc::ptr_eq(a, b),
            (FileLike::Renamed(a), FileLike::Renamed(b)) => Rc::ptr_eq(a, b),
            _ => false,
        }
    }
}

impl Eq for FileLike {}

impl Hash for FileLike {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self {
            FileLike::Source(file) => file.hash(state),
            FileLike::Original(file) => Rc::as_ptr(file).hash(state),
            FileLike::Renamed(file) => Rc::as_ptr(file).hash(state),
        }
    }
}

// Go: diagnosticwriter/diagnosticwriter.go:51 ASTDiagnostic
// ASTDiagnostic wraps ast.Diagnostic to implement the Diagnostic interface
// PORT: Go reads `Code`, `Category` and `Localize` through the embedded
// `*ast.Diagnostic`; the port reads them from `.0`. The api lane calls it
// (ts#63935 `NewDiagnosticResponse`).
#[derive(Clone, Copy)]
pub struct AstDiagnostic<'a>(pub &'a Diagnostic);

impl<'a> AstDiagnostic<'a> {
    // Go: diagnosticwriter/diagnosticwriter.go:55 (*ASTDiagnostic).RelatedInformation
    pub fn related_information(self) -> impl Iterator<Item = AstDiagnostic<'a>> {
        self.0.related_information.iter().map(AstDiagnostic)
    }

    // Go: diagnosticwriter/diagnosticwriter.go:64 (*ASTDiagnostic).File (tsgo#4712, ts#63936)
    pub fn file(self) -> Option<FileLike> {
        let file = self.0.file;
        if file.is_nil() {
            return None;
        }
        let mut file_name = source_file_file_name(file);
        let canonical = source_file_canonical_source_file(file);
        if canonical.is_some() {
            file_name = source_file_file_name(canonical);
        }
        if self.resolve().use_original {
            // The mapper's own diagnostics (Source != "") already carry original ranges; compiler
            // diagnostics have their transformed ranges mapped back. Both render against the original,
            // untransformed text. Diagnostics in synthesized code (see resolve) keep the virtual text.
            return Some(FileLike::Original(new_original_text_file(file, file_name)));
        }
        if file_name != source_file_file_name(file) {
            return Some(FileLike::Renamed(Rc::new(RenamedFile { file, file_name })));
        }
        Some(FileLike::Source(file))
    }

    // Go: diagnosticwriter/diagnosticwriter.go:85 (*ASTDiagnostic).Source (tsgo#4712)
    pub fn source(self) -> &'a str {
        self.0.source()
    }

    // Go: diagnosticwriter/diagnosticwriter.go:89 (*ASTDiagnostic).Pos (tsgo#4712)
    pub fn pos(self) -> i32 {
        self.resolve().loc.pos()
    }

    // Go: diagnosticwriter/diagnosticwriter.go:90 (*ASTDiagnostic).End (ts#63935)
    pub fn end(self) -> i32 {
        self.resolve().loc.end()
    }

    // Go: diagnosticwriter/diagnosticwriter.go:91 (*ASTDiagnostic).Len (tsgo#4712)
    // PORT: Go `TextRange.Len` subtracts the int32 ends, which wraps for a
    // bad range from a `.tsbuildinfo` (core/text.go:30).
    fn len(self) -> i32 {
        let loc = self.resolve().loc;
        loc.end().wrapping_sub(loc.pos())
    }

    // Go: diagnosticwriter/diagnosticwriter.go:104 (*ASTDiagnostic).resolve (tsgo#4712)
    // resolve determines where and against which text a diagnostic should be reported. A content mapper's
    // own diagnostics already carry original ranges. A compiler diagnostic on a content-mapped file has its
    // virtual range mapped back to the original; if it falls entirely within synthesized code, there is no
    // original location, so it is shown against the virtual text and flagged as synthesized.
    fn resolve(self) -> ResolvedLocation {
        let loc = self.0.loc();
        let file = self.0.file;
        let resolved = ResolvedLocation {
            loc,
            use_original: false,
            synthesized: false,
        };
        if file.is_nil() {
            return resolved;
        }
        if !self.0.source().is_empty() {
            return ResolvedLocation {
                use_original: true,
                ..resolved
            };
        }
        match source_file_span_map(file) {
            Some(span_map) => {
                let (mapped, fidelity) =
                    crate::spanmap::SpanMap::virtual_to_original_span(Some(span_map), loc);
                if fidelity == crate::spanmap::Fidelity::NONE {
                    return ResolvedLocation {
                        synthesized: true,
                        ..resolved
                    };
                }
                ResolvedLocation {
                    loc: mapped,
                    use_original: true,
                    synthesized: false,
                }
            }
            None => resolved,
        }
    }

    // Go: diagnosticwriter/diagnosticwriter.go:153 (*ASTDiagnostic).MessageChain (tsgo#4712)
    // PORT: the chain entries are borrowed; the note is a new diagnostic.
    pub fn message_chain(self) -> Vec<Cow<'a, Diagnostic>> {
        let mut result: Vec<Cow<'a, Diagnostic>> =
            self.0.message_chain.iter().map(Cow::Borrowed).collect();
        if self.resolve().synthesized {
            // The diagnostic points into synthesized virtual code; make clear the shown location is not in the
            // original file, and which content mapper produced it.
            let note = new_compiler_diagnostic(
                diag::This_location_is_in_virtual_code_produced_by_the_content_mapper_0_and_has_no_corresponding_location_in_the_original_file,
                args![source_file_content_mapper(self.0.file)],
            );
            result.push(Cow::Owned(note));
        }
        result
    }
}

// Go: diagnosticwriter/diagnosticwriter.go:171 WrapASTDiagnostic (ts#63935)
pub fn wrap_ast_diagnostic(d: &Diagnostic) -> AstDiagnostic<'_> {
    AstDiagnostic(d)
}

// Go: diagnosticwriter/diagnosticwriter.go:94 resolvedLocation (tsgo#4712)
// resolvedLocation describes how a diagnostic on a content-mapped file should be reported.
#[derive(Clone, Copy)]
struct ResolvedLocation {
    loc: TextRange,
    use_original: bool, // render against the file's original, untransformed text
    synthesized: bool,  // the range is in virtual code with no corresponding original location
}

// Go: diagnosticwriter/diagnosticwriter.go:125 originalTextFile (tsgo#4712)
/// Go `originalTextFile`: a content-mapped source file's original
/// (untransformed) text as a `FileLike`, so that diagnostics whose ranges
/// point into that text render at the correct locations.
pub struct OriginalTextFile {
    file_name: &'static str,
    text: FileText,
    line_map: Vec<i32>,
}

// Go: diagnosticwriter/diagnosticwriter.go:131 newOriginalTextFile (tsgo#4712, ts#63936)
fn new_original_text_file(file: Node, file_name: &'static str) -> Rc<OriginalTextFile> {
    let text = source_file_original_text(file);
    Rc::new(OriginalTextFile {
        file_name,
        line_map: compute_ecma_line_starts(&text),
        text,
    })
}

// Go: diagnosticwriter/diagnosticwriter.go:144 renamedFile (ts#63936)
/// Go `renamedFile`: a source file shown under another name (the name of
/// its canonical source file); its text and line map are the file's own.
pub struct RenamedFile {
    file: Node,
    file_name: &'static str,
}

// Go: scanner/scanner.go:2677 GetECMALineOfPosition
// PORT: Go takes an `ast.SourceFileLike`. The Rust scanner function takes a
// file node, so this is its code on a `FileLike`.
fn get_ecma_line_of_file_position(file: &FileLike, pos: i32) -> i32 {
    compute_line_of_position(&file.ecma_line_map(), pos)
}

// Go: scanner/scanner.go:2685 GetECMALineAndUTF16CharacterOfPosition
// PORT: Go takes an `ast.SourceFileLike`. The code is
// `scanner_util::ecma_line_and_utf16_character_of_text_position` (a `pos`
// inside a char, and the Go panics for a `pos` out of the text) on a
// `FileLike`.
pub fn get_ecma_line_and_utf16_character_of_file_position(file: &FileLike, pos: i32) -> (i32, i32) {
    crate::scanner_util::ecma_line_and_utf16_character_of_text_position(
        &file.ecma_line_map(),
        &file.text(),
        pos,
    )
}

// Go: diagnosticwriter/diagnosticwriter.go:211 foregroundColorEscapeGrey
const FOREGROUND_COLOR_ESCAPE_GREY: &str = "\u{1b}[90m";
// Go: diagnosticwriter/diagnosticwriter.go:212 foregroundColorEscapeRed
const FOREGROUND_COLOR_ESCAPE_RED: &str = "\u{1b}[91m";
// Go: diagnosticwriter/diagnosticwriter.go:213 foregroundColorEscapeYellow
const FOREGROUND_COLOR_ESCAPE_YELLOW: &str = "\u{1b}[93m";
// Go: diagnosticwriter/diagnosticwriter.go:214 foregroundColorEscapeBlue
const FOREGROUND_COLOR_ESCAPE_BLUE: &str = "\u{1b}[94m";
// Go: diagnosticwriter/diagnosticwriter.go:215 foregroundColorEscapeCyan
const FOREGROUND_COLOR_ESCAPE_CYAN: &str = "\u{1b}[96m";

// Go: diagnosticwriter/diagnosticwriter.go:219 gutterStyleSequence
const GUTTER_STYLE_SEQUENCE: &str = "\u{1b}[7m";
// Go: diagnosticwriter/diagnosticwriter.go:220 gutterSeparator
const GUTTER_SEPARATOR: &str = " ";
// Go: diagnosticwriter/diagnosticwriter.go:221 resetEscapeSequence
const RESET_ESCAPE_SEQUENCE: &str = "\u{1b}[0m";
// Go: diagnosticwriter/diagnosticwriter.go:222 ellipsis
const ELLIPSIS: &str = "...";

// Go: diagnosticwriter/diagnosticwriter.go:237 FormatDiagnosticWithColorAndContext
pub fn format_diagnostic_with_color_and_context(
    output: &Writer,
    diagnostic: &Diagnostic,
    format_opts: &FormattingOptions,
) {
    let diagnostic = AstDiagnostic(diagnostic);
    let file = diagnostic.file();
    if let Some(file) = &file {
        let pos = diagnostic.pos();
        write_location(
            output,
            file.clone(),
            pos,
            Some(format_opts),
            write_with_style_and_reset,
        );
        write_str(output, " - ");
    }

    write_with_style_and_reset(
        output,
        diagnostic.0.category.name(),
        get_category_format(diagnostic.0.category),
    );
    write_str(
        output,
        &format!(
            "{FOREGROUND_COLOR_ESCAPE_GREY} {}{}: {RESET_ESCAPE_SEQUENCE}",
            diagnostic_prefix(diagnostic),
            diagnostic.0.code
        ),
    );
    write_flattened_diagnostic_message(
        output,
        diagnostic.0,
        &format_opts.new_line,
        &format_opts.locale,
    );

    let snippet_file = file
        .as_ref()
        .filter(|_| diagnostic.0.code != diag::File_appears_to_be_binary.code() as i32);
    if let Some(file) = snippet_file {
        write_str(output, &format_opts.new_line);
        write_code_snippet(
            output,
            file,
            diagnostic.pos(),
            diagnostic.len(),
            get_category_format(diagnostic.0.category),
            "",
            format_opts,
        );
        write_str(output, &format_opts.new_line);
    }

    for related_information in diagnostic.related_information() {
        if let Some(file) = related_information.file() {
            write_str(output, &format_opts.new_line);
            write_str(output, "  ");
            let pos = related_information.pos();
            write_location(
                output,
                file.clone(),
                pos,
                Some(format_opts),
                write_with_style_and_reset,
            );
            write_str(output, " - ");
            write_flattened_diagnostic_message(
                output,
                related_information.0,
                &format_opts.new_line,
                &format_opts.locale,
            );
            write_code_snippet(
                output,
                &file,
                pos,
                related_information.len(),
                FOREGROUND_COLOR_ESCAPE_CYAN,
                "    ",
                format_opts,
            );
        }
        write_str(output, &format_opts.new_line);
    }
}

// Go: diagnosticwriter/diagnosticwriter.go:272 writeCodeSnippet
fn write_code_snippet(
    writer: &Writer,
    source_file: &FileLike,
    start: i32,
    length: i32,
    squiggle_color: &str,
    indent: &str,
    format_opts: &FormattingOptions,
) {
    let (first_line, first_line_char) =
        get_ecma_line_and_utf16_character_of_file_position(source_file, start);
    // Go `start+length` is a Go int. `length` is the wrapped int32 length
    // of a bad range from a `.tsbuildinfo` (`Len`), so the sum can pass the
    // int32 range: then Go panics in `text[lineMap[line]:pos]`.
    // PORT: `start` and `length` are port offsets (see `go_byte_offset`).
    // When their sum stays in the int32 range, so does Go's, and both are
    // the end of the range. Else Go's sum is taken on the Go offsets of the
    // ends: `start` is in the text (the first call above did not panic), so
    // a wrapped Go sum is past it.
    let end = i64::from(start) + i64::from(length);
    let (last_line, mut last_line_char) = match i32::try_from(end) {
        Ok(end) => get_ecma_line_and_utf16_character_of_file_position(source_file, end),
        Err(_) => {
            let text = source_file.text();
            let end = start.wrapping_add(length);
            let go_start = crate::scanner_util::go_byte_offset(&text, start);
            let go_end = crate::scanner_util::go_byte_offset(&text, end);
            let go_sum = i64::from(go_start) + i64::from(go_end.wrapping_sub(go_start));
            if go_sum == i64::from(go_end) {
                get_ecma_line_and_utf16_character_of_file_position(source_file, end)
            } else {
                crate::scanner_util::panic_past_text(&text, go_sum)
            }
        }
    };
    if length == 0 {
        last_line_char += 1; // When length is zero, squiggle the character right after the start position.
    }

    let text = source_file.text();
    let last_line_of_file = get_ecma_line_of_file_position(source_file, text.len() as i32);

    let has_more_than_five_lines = last_line - first_line >= 4;
    let mut gutter_width = (last_line + 1).to_string().len() as i32;
    if has_more_than_five_lines {
        gutter_width = (ELLIPSIS.len() as i32).max(gutter_width);
    }

    let mut i = first_line;
    while i <= last_line {
        write_str(writer, &format_opts.new_line);

        // If the error spans over 5 lines, we'll only show the first 2 and last 2 lines,
        // so we'll skip ahead to the second-to-last line.
        if has_more_than_five_lines && first_line + 1 < i && i < last_line - 1 {
            write_str(writer, indent);
            write_str(writer, GUTTER_STYLE_SEQUENCE);
            write_str(writer, &go_pad(ELLIPSIS, gutter_width, false));
            write_str(writer, RESET_ESCAPE_SEQUENCE);
            write_str(writer, GUTTER_SEPARATOR);
            write_str(writer, &format_opts.new_line);
            i = last_line - 1;
        }

        // Go scanner.GetECMAPositionOfLineAndByteOffset
        let line_starts = &*source_file.ecma_line_map();
        let line_start = compute_position_of_line_and_byte_offset(line_starts, i, 0);
        let line_end = if i < last_line_of_file {
            compute_position_of_line_and_byte_offset(line_starts, i + 1, 0)
        } else {
            text.len() as i32
        };

        // Go `unicode.IsSpace` is the Unicode White_Space property, like
        // `char::is_whitespace`.
        let line_content =
            text[line_start as usize..line_end as usize].trim_end_matches(char::is_whitespace); // trim from end
        let line_content = line_content.replace('\t', " "); // convert tabs to single spaces

        // Output the gutter and the actual contents of the line.
        write_str(writer, indent);
        write_str(writer, GUTTER_STYLE_SEQUENCE);
        write_str(writer, &go_pad(&(i + 1).to_string(), gutter_width, false));
        write_str(writer, RESET_ESCAPE_SEQUENCE);
        write_str(writer, GUTTER_SEPARATOR);
        write_str(writer, &line_content);
        write_str(writer, &format_opts.new_line);

        // Output the gutter and the error span for the line using tildes.
        write_str(writer, indent);
        write_str(writer, GUTTER_STYLE_SEQUENCE);
        write_str(writer, &go_pad("", gutter_width, false));
        write_str(writer, RESET_ESCAPE_SEQUENCE);
        write_str(writer, GUTTER_SEPARATOR);
        write_str(writer, squiggle_color);
        if i == first_line {
            // If we're on the last line, then limit it to the last character of the last line.
            // Otherwise, we'll just squiggle the rest of the line, giving 'slice' no end position.
            let last_char_for_line = if i == last_line {
                last_line_char
            } else {
                utf16_len(&line_content)
            };

            // Fill with spaces until the first character,
            // then squiggle the remainder of the line.
            write_str(writer, &go_repeat(" ", first_line_char));
            write_str(
                writer,
                &go_repeat("~", last_char_for_line - first_line_char),
            );
        } else if i == last_line {
            // Squiggle until the final character.
            write_str(writer, &go_repeat("~", last_line_char));
        } else {
            // Squiggle the entire line.
            write_str(writer, &go_repeat("~", utf16_len(&line_content)));
        }

        write_str(writer, RESET_ESCAPE_SEQUENCE);
        i += 1;
    }
}

// Go: diagnosticwriter/diagnosticwriter.go:366 WriteFlattenedDiagnosticMessage
// PORT: also Go `WriteFlattenedASTDiagnosticMessage` (:338): the port takes
// the ast diagnostic and wraps it.
pub fn write_flattened_diagnostic_message(
    writer: &Writer,
    diagnostic: &Diagnostic,
    newline: &str,
    locale: &Locale,
) {
    write_str(writer, &diagnostic.localize(locale));

    for chain in AstDiagnostic(diagnostic).message_chain() {
        flatten_diagnostic_message_chain(writer, &chain, newline, locale, 1);
    }
}

// Go: diagnosticwriter/diagnosticwriter.go:374 flattenDiagnosticMessageChain
fn flatten_diagnostic_message_chain(
    writer: &Writer,
    chain: &Diagnostic,
    new_line: &str,
    locale: &Locale,
    level: usize,
) {
    write_str(writer, new_line);
    for _ in 0..level {
        write_str(writer, "  ");
    }

    write_str(writer, &chain.localize(locale));
    for child in AstDiagnostic(chain).message_chain() {
        flatten_diagnostic_message_chain(writer, &child, new_line, locale, level + 1);
    }
}

// Go: diagnosticwriter/diagnosticwriter.go:388 diagnosticPrefix (tsgo#4712)
// diagnosticPrefix returns the prefix shown before a diagnostic's code, e.g. "TS" for compiler
// diagnostics or a content mapper's custom source for its diagnostics.
fn diagnostic_prefix(diagnostic: AstDiagnostic<'_>) -> &str {
    let source = diagnostic.source();
    if !source.is_empty() {
        return source;
    }
    "TS"
}

// Go: diagnosticwriter/diagnosticwriter.go:395 getCategoryFormat
fn get_category_format(category: Category) -> &'static str {
    match category {
        Category::Error => FOREGROUND_COLOR_ESCAPE_RED,
        Category::Warning => FOREGROUND_COLOR_ESCAPE_YELLOW,
        Category::Suggestion => FOREGROUND_COLOR_ESCAPE_GREY,
        Category::Message => FOREGROUND_COLOR_ESCAPE_BLUE,
        _ => crate::core::go_panic("Unhandled diagnostic category".to_string()),
    }
}

// Go: diagnosticwriter/diagnosticwriter.go:409 FormattedWriter
pub type FormattedWriter = fn(output: &Writer, text: &str, format_style: &str);

// Go: diagnosticwriter/diagnosticwriter.go:411 writeWithStyleAndReset
fn write_with_style_and_reset(output: &Writer, text: &str, format_style: &str) {
    write_str(output, format_style);
    write_str(output, text);
    write_str(output, RESET_ESCAPE_SEQUENCE);
}

// Go: diagnosticwriter/diagnosticwriter.go:417 WriteLocation
// PORT: `file` is a `FileLike` or a source file node.
pub fn write_location(
    output: &Writer,
    file: impl Into<FileLike>,
    pos: i32,
    format_opts: Option<&FormattingOptions>,
    write_with_style_and_reset: FormattedWriter,
) {
    let file = file.into();
    let (first_line, first_char) = get_ecma_line_and_utf16_character_of_file_position(&file, pos);
    let relative_file_name = match format_opts {
        Some(format_opts) => {
            convert_to_relative_path(file.file_name(), &format_opts.compare_paths_options)
        }
        None => file.file_name().to_string(),
    };

    write_with_style_and_reset(output, &relative_file_name, FOREGROUND_COLOR_ESCAPE_CYAN);
    write_str(output, ":");
    write_with_style_and_reset(
        output,
        &(first_line + 1).to_string(),
        FOREGROUND_COLOR_ESCAPE_YELLOW,
    );
    write_str(output, ":");
    write_with_style_and_reset(
        output,
        &(first_char + 1).to_string(),
        FOREGROUND_COLOR_ESCAPE_YELLOW,
    );
}

// Some of these lived in watch.ts, but they're not specific to the watch API.

// Go: diagnosticwriter/diagnosticwriter.go:435 ErrorSummary
// PORT: the lists borrow the diagnostics.
struct ErrorSummary<'a> {
    total_error_count: i32,
    global_errors: Vec<&'a Diagnostic>,
    errors_by_file: FxHashMap<FileLike, Vec<&'a Diagnostic>>,
    sorted_files: Vec<FileLike>,
}

// Go: diagnosticwriter/diagnosticwriter.go:442 WriteErrorSummaryText
pub fn write_error_summary_text(
    output: &Writer,
    all_diagnostics: &[Diagnostic],
    format_opts: &FormattingOptions,
) {
    // Roughly corresponds to 'getErrorSummaryText' from watch.ts

    let error_summary = get_error_summary(all_diagnostics);
    let total_error_count = error_summary.total_error_count;
    if total_error_count == 0 {
        return;
    }

    let first_file = error_summary.sorted_files.first();
    let first_file_name = pretty_path_for_file_error(
        first_file,
        first_file
            .and_then(|file| error_summary.errors_by_file.get(file))
            .map(Vec::as_slice)
            .unwrap_or_default(),
        format_opts,
    );
    let num_erroring_files = error_summary.errors_by_file.len();

    let locale = &format_opts.locale;
    let message = if total_error_count == 1 {
        // Special-case a single error.
        if !error_summary.global_errors.is_empty() || first_file_name.is_empty() {
            message_localize(diag::Found_1_error, locale, &[])
        } else {
            message_localize(diag::Found_1_error_in_0, locale, &args![first_file_name])
        }
    } else {
        match num_erroring_files {
            // No file-specific errors.
            0 => message_localize(diag::Found_0_errors, locale, &args![total_error_count]),
            // One file with errors.
            1 => message_localize(
                diag::Found_0_errors_in_the_same_file_starting_at_Colon_1,
                locale,
                &args![total_error_count, first_file_name],
            ),
            // Multiple files with errors.
            _ => message_localize(
                diag::Found_0_errors_in_1_files,
                locale,
                &args![total_error_count, num_erroring_files],
            ),
        }
    };
    write_str(output, &format_opts.new_line);
    write_str(output, &message);
    write_str(output, &format_opts.new_line);
    write_str(output, &format_opts.new_line);
    if num_erroring_files > 1 {
        write_tabular_errors_display(output, &error_summary, format_opts);
        write_str(output, &format_opts.new_line);
    }
}

// Go: diagnosticwriter/diagnosticwriter.go:489 getErrorSummary
// PORT: Go calls `File()` twice for each diagnostic. For a content-mapped
// file each call makes a new `originalTextFile`, so each such diagnostic has
// its own entry (tsgo#4712). One call per diagnostic gives the same entries.
fn get_error_summary(diags: &[Diagnostic]) -> ErrorSummary<'_> {
    let mut total_error_count = 0;
    let mut global_errors = Vec::new();
    let mut errors_by_file: FxHashMap<FileLike, Vec<&Diagnostic>> = FxHashMap::default();
    // The files in the order of their first diagnostic.
    let mut sorted_files: Vec<FileLike> = Vec::new();

    for diagnostic in diags {
        if diagnostic.category != Category::Error {
            continue;
        }

        total_error_count += 1;
        match AstDiagnostic(diagnostic).file() {
            None => {
                global_errors.push(diagnostic);
            }
            Some(file) => {
                let file_errors = errors_by_file.entry(file.clone()).or_default();
                if file_errors.is_empty() {
                    sorted_files.push(file);
                }
                file_errors.push(diagnostic);
            }
        }
    }

    // !!!
    // Need an ordered map here, but sorting for consistency.
    // Go: diagnosticwriter/diagnosticwriter.go:488 slices.SortedFunc(maps.Keys(errorsByFile), ...)
    // PORT: Go compares the bytes of the names (see `compare_go_bytes`). Go
    // sorts the keys in map order, so the entries of one content-mapped file
    // (same name) come in any order. Here the sort starts from diagnostic
    // order.
    crate::gostd::slices::sort_func(&mut sorted_files, |a, b| {
        compare_go_bytes(a.file_name(), b.file_name()) as i32
    });

    ErrorSummary {
        total_error_count,
        global_errors,
        errors_by_file,
        sorted_files,
    }
}

// Go: diagnosticwriter/diagnosticwriter.go:524 writeTabularErrorsDisplay
fn write_tabular_errors_display(
    output: &Writer,
    error_summary: &ErrorSummary<'_>,
    format_opts: &FormattingOptions,
) {
    let sorted_files = &error_summary.sorted_files;

    let mut max_errors = 0;
    for errors_for_file in error_summary.errors_by_file.values() {
        max_errors = max_errors.max(errors_for_file.len());
    }

    // !!!
    // TODO (drosen): This was never localized.
    // Should make this better.
    let header_row = message_localize(diag::Errors_Files, &format_opts.locale, &[]);
    let left_column_heading_length = header_row.split(' ').next().unwrap_or_default().len() as i32;
    let length_of_biggest_error_count = max_errors.to_string().len() as i32;
    let left_padding_goal = left_column_heading_length.max(length_of_biggest_error_count);
    let header_padding = (length_of_biggest_error_count - left_column_heading_length).max(0);

    write_str(output, &go_repeat(" ", header_padding));
    write_str(output, &header_row);
    write_str(output, &format_opts.new_line);

    for file in sorted_files {
        let file_errors = &error_summary.errors_by_file[file];
        let error_count = file_errors.len();

        write_str(
            output,
            &format!(
                "{}  ",
                go_pad(&error_count.to_string(), left_padding_goal, false)
            ),
        );
        write_str(
            output,
            &pretty_path_for_file_error(Some(file), file_errors, format_opts),
        );
        write_str(output, &format_opts.new_line);
    }
}

// Go: diagnosticwriter/diagnosticwriter.go:555 prettyPathForFileError
fn pretty_path_for_file_error(
    file: Option<&FileLike>,
    file_errors: &[&Diagnostic],
    format_opts: &FormattingOptions,
) -> String {
    let Some(file) = file else {
        return String::new();
    };
    if file_errors.is_empty() {
        return String::new();
    }
    let line = get_ecma_line_of_file_position(file, AstDiagnostic(file_errors[0]).pos());
    let mut file_name = file.file_name().to_string();
    if path_is_absolute(&file_name)
        && path_is_absolute(&format_opts.compare_paths_options.current_directory)
    {
        file_name = convert_to_relative_path(file.file_name(), &format_opts.compare_paths_options);
    }
    format!(
        "{}{}:{}{}",
        file_name,
        FOREGROUND_COLOR_ESCAPE_GREY,
        line + 1,
        RESET_ESCAPE_SEQUENCE,
    )
}

// Go: diagnosticwriter/diagnosticwriter.go:576 WriteFormatDiagnostic
pub fn write_format_diagnostic(
    output: &Writer,
    diagnostic: &Diagnostic,
    format_opts: &FormattingOptions,
) {
    let wrapped = AstDiagnostic(diagnostic);
    if let Some(file) = wrapped.file() {
        let (line, character) =
            get_ecma_line_and_utf16_character_of_file_position(&file, wrapped.pos());
        let file_name = file.file_name();
        let relative_file_name =
            convert_to_relative_path(file_name, &format_opts.compare_paths_options);
        write_str(
            output,
            &format!("{}({},{}): ", relative_file_name, line + 1, character + 1),
        );
    }

    write_str(
        output,
        &format!(
            "{} {}{}: ",
            diagnostic.category.name(),
            diagnostic_prefix(wrapped),
            diagnostic.code
        ),
    );
    write_flattened_diagnostic_message(
        output,
        diagnostic,
        &format_opts.new_line,
        &format_opts.locale,
    );
    write_str(output, &format_opts.new_line);
}

// Go: diagnosticwriter/diagnosticwriter.go:570 WriteFormatDiagnostics
pub fn write_format_diagnostics_to(
    output: &Writer,
    diagnostics: &[Diagnostic],
    format_opts: &FormattingOptions,
) {
    for diagnostic in diagnostics {
        write_format_diagnostic(output, diagnostic, format_opts);
    }
}

// Go: diagnosticwriter/diagnosticwriter.go:589 FormatDiagnosticsStatusWithColorAndTime
pub fn format_diagnostics_status_with_color_and_time(
    output: &Writer,
    time: &str,
    diag: &Diagnostic,
    format_opts: &FormattingOptions,
) {
    write_str(output, "[");
    write_with_style_and_reset(output, time, FOREGROUND_COLOR_ESCAPE_GREY);
    write_str(output, "] ");
    write_flattened_diagnostic_message(output, diag, &format_opts.new_line, &format_opts.locale);
}

// Go: diagnosticwriter/diagnosticwriter.go:596 FormatDiagnosticsStatusAndTime
pub fn format_diagnostics_status_and_time(
    output: &Writer,
    time: &str,
    diag: &Diagnostic,
    format_opts: &FormattingOptions,
) {
    write_str(output, &format!("{time} - "));
    write_flattened_diagnostic_message(output, diag, &format_opts.new_line, &format_opts.locale);
}

// Go: diagnosticwriter/diagnosticwriter.go:606 TryClearScreen
pub fn try_clear_screen(output: &Writer, diag: &Diagnostic, options: &CompilerOptions) -> bool {
    // Go: diagnosticwriter/diagnosticwriter.go:596 ScreenStartingCodes
    let screen_starting_codes = [
        diag::Starting_compilation_in_watch_mode.code() as i32,
        diag::File_change_detected_Starting_incremental_compilation.code() as i32,
    ];
    if !options.preserve_watch_output.is_true()
        && !options.extended_diagnostics.is_true()
        && !options.diagnostics.is_true()
        && screen_starting_codes.contains(&diag.code)
    {
        write_str(output, "\x1B[2J\x1B[3J\x1B[H"); // Clear screen and move cursor to home position
        return true;
    }
    false
}

// ---------------------------------------------------------------------------
// Go standard library helpers used above and in help.rs
// ---------------------------------------------------------------------------

/// Go `fmt.Sprintf("%*s", width, s)`, or `"%-*s"` when `left` is true. Go
/// pads to `width` runes, and a negative width pads on the right.
pub(super) fn go_pad(s: &str, width: i32, left: bool) -> String {
    let left = left || width < 0;
    let width = width.unsigned_abs() as usize;
    if left {
        format!("{s:<width$}")
    } else {
        format!("{s:>width$}")
    }
}

// Go: strings/strings.go:597 Repeat (go1.27.1)
/// Go `strings.Repeat`, which panics on a negative count.
// PORT: the Go panic ends the run with Go's exit code 2 (`core::go_panic`),
// not the port crash code 70. `--pretty` reaches it in Go too
// (diagnosticwriter.go:335), when a squiggle ends before it starts.
pub(super) fn go_repeat(s: &str, count: i32) -> String {
    match usize::try_from(count) {
        Ok(count) => s.repeat(count),
        Err(_) => crate::core::go_panic("strings: negative Repeat count".to_string()),
    }
}

#[cfg(test)]
mod tests {
    /// Go `strings.Repeat` panics on a negative count. `--pretty` reaches it
    /// when a squiggle ends before it starts (diagnosticwriter.go:335), and
    /// Go ends the run with exit code 2. It is a Go panic, not a port gap
    /// (exit 70).
    #[test]
    fn go_repeat_with_a_negative_count_is_a_go_panic() {
        assert_eq!(super::go_repeat("~", 2), "~~");
        let payload = std::panic::catch_unwind(|| super::go_repeat("~", -1))
            .expect_err("a negative count panics");
        let panic = payload
            .downcast_ref::<crate::core::GoPanic>()
            .expect("a Go panic");
        assert_eq!(panic.message, "strings: negative Repeat count");
    }
}
