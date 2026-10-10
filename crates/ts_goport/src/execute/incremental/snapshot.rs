//! Port of execute/incremental/snapshot.go from line 133 (after
//! `emitSignature`, which is in `hash.rs`): the build info diagnostics with
//! file names, the diagnostics repopulation, and the `snapshot` state.
//!
//! PORT: Go `*compiler.Program` parameters are dropped and the current
//! program (`prog()`) is read through the `program.rs` free functions. Go
//! `SyncMap` and `SyncSet` fields are `FxIndexMap` and `FxIndexSet` (see
//! `reference_map.rs` for the order note); Go `atomic.Bool` and `sync.Once`
//! become plain fields, because the snapshot is only used on the loading
//! thread.
//!
//! PERF: the map keys are long paths, so the maps use the Fx hasher instead
//! of SipHash. The hasher does not change the order: it is insertion order.

use super::hash::FileInfo;
use super::hash::*;
use super::reference_map::ReferenceMap;
use crate::emitter::program_emit::WriteFileData;
use crate::frontend::prelude::*;
use std::cell::OnceCell;

/// Go `*ast.RepopulateDiagnosticInfo`, as `Diagnostic::repopulate_info`
/// returns it (`Arc`, because a `Diagnostic` crosses the checker threads).
pub type RepopulateInfoRef = std::sync::Arc<RepopulateDiagnosticInfo>;

// Go: incremental/snapshot.go:133 buildInfoDiagnosticWithFileName
// PORT: Go `diagnostics.Category` is kept as the raw Go value, like
// `BuildInfoDiagnostic.category` (`diagnostics::Category(raw)` gives the Go
// value). The Go slices are `Option`s, because the build info leaves out a
// nil slice but writes an empty one (`omitzero`, buildInfo.go:209 to :211).
// A read empty list stays empty (`core.Map`, buildinfotosnapshot.go:85 to
// :87), so it is written again as `[]` (snapshottobuildinfo.go:140 to
// :142).
#[derive(Clone, Debug, Default)]
pub struct BuildInfoDiagnosticWithFileName {
    // filename if it is for a File thats other than its stored for
    pub file: Path,
    pub no_file: bool,
    pub pos: i32,
    pub end: i32,
    pub code: i32,
    pub category: i32,
    pub source: String,
    pub message_text: String,
    pub message_key: String,
    pub message_args: Option<Vec<String>>,
    pub message_chain: Option<Vec<BuildInfoDiagnosticWithFileName>>,
    pub related_information: Option<Vec<BuildInfoDiagnosticWithFileName>>,
    pub reports_unnecessary: bool,
    pub reports_deprecated: bool,
    pub skipped_on_no_emit: bool,
    pub repopulate_info: Option<RepopulateInfoRef>,
}

/// The Go byte offsets of the port text range `loc` in `file`.
// PORT: Go keeps diagnostic positions as byte offsets of the file text, and
// `BuildInfoDiagnosticWithFileName` holds them as Go does. The port text is
// the port form of the Go text (see `scanner_util::GO_STRING_MARKER`), whose
// offsets differ after a unit, so a `Diagnostic` range is converted when it
// becomes one (`go_text_range`) and back (`port_text_range`). A nil file
// keeps the range.
pub fn go_text_range(file: Node, loc: TextRange) -> (i32, i32) {
    if file.is_nil() {
        return (loc.pos(), loc.end());
    }
    let text = source_file_text(file);
    (
        go_byte_offset(&text, loc.pos()),
        go_byte_offset(&text, loc.end()),
    )
}

/// The port text range of the Go byte offsets `pos` and `end` in `file` (see
/// `go_text_range`).
pub fn port_text_range(file: Node, pos: i32, end: i32) -> TextRange {
    if file.is_nil() {
        return TextRange::new(pos, end);
    }
    let text = source_file_text(file);
    TextRange::new(port_byte_offset(&text, pos), port_byte_offset(&text, end))
}

// Go: incremental/snapshot.go:153 DiagnosticsOrBuildInfoDiagnosticsWithFileName
// PORT: Go nil `diagnostics` is `None`; it marks "not converted yet".
// PORT: testing. Go maps hold pointers to these, and the Go test harness
// compares the old and new program's pointers (tsctests/sys.go OnProgram).
// `id` is that identity: each new entry (Go
// `&DiagnosticsOrBuildInfoDiagnosticsWithFileName{...}`, here `default()`
// or `..Default::default()`) gets a new id, and a clone (Go keeps the
// pointer) keeps it. No output reads it.
#[derive(Clone, Debug)]
pub struct DiagnosticsOrBuildInfoDiagnosticsWithFileName {
    pub diagnostics: Option<Vec<Diagnostic>>,
    pub build_info_diagnostics: Vec<BuildInfoDiagnosticWithFileName>,
    pub id: u64,
}

// PORT: testing (see `DiagnosticsOrBuildInfoDiagnosticsWithFileName`)
impl Default for DiagnosticsOrBuildInfoDiagnosticsWithFileName {
    fn default() -> Self {
        DiagnosticsOrBuildInfoDiagnosticsWithFileName {
            diagnostics: None,
            build_info_diagnostics: Vec::new(),
            id: new_diagnostics_id(),
        }
    }
}

/// PORT: testing. A new `DiagnosticsOrBuildInfoDiagnosticsWithFileName` id.
pub fn new_diagnostics_id() -> u64 {
    static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

impl BuildInfoDiagnosticWithFileName {
    // Go: incremental/snapshot.go:158 toDiagnostic
    #[must_use]
    pub fn to_diagnostic(&self, file: Node) -> Diagnostic {
        let mut file_for_diagnostic = Node::NIL;
        if !self.file.is_empty() {
            file_for_diagnostic = get_source_file_by_path(&self.file);
        } else if !self.no_file {
            file_for_diagnostic = file;
        }

        if self.repopulate_info.is_some() {
            return repopulate_diagnostic_chain(self, file_for_diagnostic);
        }

        let message_chain = self
            .message_chain
            .iter()
            .flatten()
            .map(|msg| msg.to_diagnostic(file_for_diagnostic))
            .collect();
        let related_information = self
            .related_information
            .iter()
            .flatten()
            .map(|info| info.to_diagnostic(file_for_diagnostic))
            .collect();
        let mut diagnostic = new_diagnostic_from_serialized(
            file_for_diagnostic,
            port_text_range(file_for_diagnostic, self.pos, self.end),
            self.code,
            crate::diagnostics::Category(self.category),
            &self.message_key,
            self.message_args.clone().unwrap_or_default(),
            message_chain,
            related_information,
            self.reports_unnecessary,
            self.reports_deprecated,
            self.skipped_on_no_emit,
        );
        if !self.source.is_empty() || !self.message_text.is_empty() {
            diagnostic.set_external_data(&self.source, &self.message_text);
        }
        diagnostic
    }

    // Go: incremental/snapshot.go:212 toDiagnosticWithoutRepopulate
    #[must_use]
    pub fn to_diagnostic_without_repopulate(&self, file: Node) -> Diagnostic {
        let message_chain = self
            .message_chain
            .iter()
            .flatten()
            .map(|msg| msg.to_diagnostic(file))
            .collect();
        let related_information = self
            .related_information
            .iter()
            .flatten()
            .map(|info| info.to_diagnostic(file))
            .collect();
        new_diagnostic_from_serialized(
            file,
            port_text_range(file, self.pos, self.end),
            self.code,
            crate::diagnostics::Category(self.category),
            &self.message_key,
            self.message_args.clone().unwrap_or_default(),
            message_chain,
            related_information,
            self.reports_unnecessary,
            self.reports_deprecated,
            self.skipped_on_no_emit,
        )
    }
}

// Go: incremental/snapshot.go:199 repopulateDiagnosticChain
// repopulateDiagnosticChain recomputes a diagnostic chain entry that depends on
// program state which may have changed between incremental builds.
#[must_use]
pub fn repopulate_diagnostic_chain(b: &BuildInfoDiagnosticWithFileName, file: Node) -> Diagnostic {
    let info = b
        .repopulate_info
        .clone()
        .expect("repopulateDiagnosticChain without repopulate info");
    match info.kind {
        RepopulateDiagnosticKind::MODE_MISMATCH => repopulate_mode_mismatch_chain(b, file),
        RepopulateDiagnosticKind::MODULE_NOT_FOUND => {
            repopulate_module_not_found_chain(b, file, &info)
        }
        // Fall back to using the stored (possibly stale) data
        _ => b.to_diagnostic_without_repopulate(file),
    }
}

// Go: incremental/snapshot.go:236 repopulateModeMismatchChain
#[must_use]
pub fn repopulate_mode_mismatch_chain(
    b: &BuildInfoDiagnosticWithFileName,
    file: Node,
) -> Diagnostic {
    if file.is_nil() {
        return b.to_diagnostic_without_repopulate(file);
    }

    let details = create_mode_mismatch_details(prog(), file);

    let next_chain = b
        .message_chain
        .iter()
        .flatten()
        .map(|msg| msg.to_diagnostic(file))
        .collect();

    new_diagnostic_from_serialized(
        file,
        port_text_range(file, b.pos, b.end),
        details.message.code() as i32,
        details.message.category(),
        details.message.key(),
        details.args,
        next_chain,
        Vec::new(),
        false,
        false,
        false,
    )
}

// Go: incremental/snapshot.go:263 repopulateModuleNotFoundChain
#[must_use]
pub fn repopulate_module_not_found_chain(
    b: &BuildInfoDiagnosticWithFileName,
    file: Node,
    info: &RepopulateDiagnosticInfo,
) -> Diagnostic {
    if file.is_nil() {
        return b.to_diagnostic_without_repopulate(file);
    }

    let mut package_name = info.package_name.as_str();
    if package_name.is_empty() {
        package_name = &info.module_reference;
    }

    let details = create_module_not_found_chain(
        prog(),
        file,
        &info.module_reference,
        info.mode,
        package_name,
    );

    let next_chain = b
        .message_chain
        .iter()
        .flatten()
        .map(|msg| msg.to_diagnostic(file))
        .collect();

    new_diagnostic_from_serialized(
        file,
        port_text_range(file, b.pos, b.end),
        details.message.code() as i32,
        details.message.category(),
        details.message.key(),
        details.args,
        next_chain,
        Vec::new(),
        false,
        false,
        false,
    )
}

impl DiagnosticsOrBuildInfoDiagnosticsWithFileName {
    // Go: incremental/snapshot.go:295 getDiagnostics
    pub fn get_diagnostics(&mut self, file: Node) -> Vec<Diagnostic> {
        if let Some(diagnostics) = &self.diagnostics {
            return diagnostics.clone();
        }
        // Convert and cache the diagnostics
        let diagnostics: Vec<Diagnostic> = self
            .build_info_diagnostics
            .iter()
            .map(|diag| diag.to_diagnostic(file))
            .collect();
        self.diagnostics = Some(diagnostics.clone());
        diagnostics
    }
}

// Go: incremental/snapshot.go:306 snapshot
// PORT: Go `options *core.CompilerOptions` is `&'static`: the program
// options are static, and `buildInfoToSnapshot` leaks the options it reads
// (one per process). `compiler.ProgramLike.Options` needs `&'static`.
#[derive(Debug)]
pub struct Snapshot {
    // These are the fields that get serialized

    // Information of the file eg. its version, signature etc
    pub file_infos: FxIndexMap<Path, FileInfo>,
    pub options: &'static CompilerOptions,
    //  Contains the map of ReferencedSet=Referenced files of the file if module emit is enabled
    pub referenced_map: ReferenceMap,
    // Cache of semantic diagnostics for files with their Path being the key
    pub semantic_diagnostics_per_file:
        FxIndexMap<Path, DiagnosticsOrBuildInfoDiagnosticsWithFileName>,
    // Cache of dts emit diagnostics for files with their Path being the key
    pub emit_diagnostics_per_file: FxIndexMap<Path, DiagnosticsOrBuildInfoDiagnosticsWithFileName>,
    // The map has key by source file's path that has been changed
    pub changed_files_set: FxIndexSet<Path>,
    // Files pending to be emitted
    pub affected_files_pending_emit: FxIndexMap<Path, FileEmitKind>,
    // Name of the file whose dts was the latest to change
    pub latest_changed_dts_file: String,
    // Hash of d.ts emitted for the file, use to track when emit of d.ts changes
    // PORT: `FxHashMap` because `BuildInfoEmitSignature::to_emit_signature`
    // reads it; no Go code depends on its order.
    pub emit_signatures: FxHashMap<Path, EmitSignature>,
    // Recorded if program had errors that need to be reported even with --noCheck
    pub has_errors: Tristate,
    // Recorded if program had semantic errors only for non incremental build
    pub has_semantic_errors: bool,
    // If semantic diagnostic check is pending
    pub check_pending: bool,
    // Looked up package.json files from
    // PORT: Go nil (not computed yet) is `None`.
    pub package_jsons: Option<Vec<String>>,
    pub missing_package_jsons: Option<Vec<String>>,

    // Additional fields that are not serialized but needed to track state

    // true if build info emit is pending
    pub build_info_emit_pending: bool,
    pub has_errors_from_old_state: Tristate,
    pub has_semantic_errors_from_old_state: bool,
    // PORT: Go reads these two only with `slices.Equal`, where a nil slice
    // equals an empty one, so `None` is the empty `Vec`.
    pub package_jsons_from_old_state: Vec<String>,
    pub missing_package_jsons_from_old_state: Vec<String>,
    //  Cache of all files excluding default library file for the current program
    // PORT: Go `allFilesExcludingDefaultLibraryFile` plus its sync.Once.
    pub all_files_excluding_default_library_file: OnceCell<Vec<Node>>,
    pub has_changed_dts_file: bool,
    pub has_emit_diagnostics: bool,

    // Used with testing to add text of hash for better comparison
    pub hash_with_text: bool,

    /// PORT: not in Go. The freeable file versions (`ast::FileVersion`)
    /// that the diagnostics copied from the old snapshot point at
    /// (`programToSnapshot`, Go `repopulateDiagnosticsOfFile`): their
    /// files, related information and message chains. Go's GC keeps an old
    /// `*ast.SourceFile` alive through a copied `*ast.Diagnostic`. Here a
    /// watch rebuild can free a file version that the new program does not
    /// have, so the snapshot holds it while the report and the build info
    /// can read it. Empty unless a freeable version is published
    /// (`ast::any_freeable_published`).
    pub held_file_versions: Vec<std::sync::Arc<crate::ast::FileVersion>>,
}

impl Snapshot {
    /// A snapshot with Go zero values and the given options.
    #[must_use]
    pub fn new(options: &'static CompilerOptions) -> Self {
        Snapshot {
            file_infos: FxIndexMap::default(),
            options,
            referenced_map: ReferenceMap::default(),
            semantic_diagnostics_per_file: FxIndexMap::default(),
            emit_diagnostics_per_file: FxIndexMap::default(),
            changed_files_set: FxIndexSet::default(),
            affected_files_pending_emit: FxIndexMap::default(),
            latest_changed_dts_file: String::new(),
            emit_signatures: FxHashMap::default(),
            has_errors: Tristate::Unknown,
            has_semantic_errors: false,
            check_pending: false,
            package_jsons: None,
            missing_package_jsons: None,
            build_info_emit_pending: false,
            has_errors_from_old_state: Tristate::Unknown,
            has_semantic_errors_from_old_state: false,
            package_jsons_from_old_state: Vec::new(),
            missing_package_jsons_from_old_state: Vec::new(),
            all_files_excluding_default_library_file: OnceCell::new(),
            has_changed_dts_file: false,
            has_emit_diagnostics: false,
            hash_with_text: false,
            held_file_versions: Vec::new(),
        }
    }

    // Go: incremental/snapshot.go:354 addFileToChangeSet
    pub fn add_file_to_change_set(&mut self, file_path: Path) {
        self.changed_files_set.insert(file_path);
        self.build_info_emit_pending = true;
    }

    // Go: incremental/snapshot.go:359 addFileToAffectedFilesPendingEmit
    pub fn add_file_to_affected_files_pending_emit(
        &mut self,
        file_path: Path,
        emit_kind: FileEmitKind,
    ) {
        let existing_kind = self
            .affected_files_pending_emit
            .get(&file_path)
            .copied()
            .unwrap_or_default();
        if emit_kind.intersects(FileEmitKind::DTS_ERRORS) {
            self.emit_diagnostics_per_file.shift_remove(&file_path);
        }
        self.affected_files_pending_emit
            .insert(file_path, existing_kind | emit_kind);
        self.build_info_emit_pending = true;
    }

    // Go: incremental/snapshot.go:368 getAllFilesExcludingDefaultLibraryFile
    pub fn get_all_files_excluding_default_library_file(&self, first_source_file: Node) -> &[Node] {
        self.all_files_excluding_default_library_file
            .get_or_init(|| {
                let files = source_files();
                let mut result = Vec::with_capacity(files.len());
                let mut add_source_file = |file: Node| {
                    if !is_source_file_default_library(&source_file_info(file).path) {
                        result.push(file);
                    }
                };
                if first_source_file.is_some() {
                    add_source_file(first_source_file);
                }
                for file in files {
                    if file != first_source_file {
                        add_source_file(file);
                    }
                }
                result
            })
    }

    // Go: incremental/snapshot.go:396 computeSignatureWithDiagnostics
    #[must_use]
    pub fn compute_signature_with_diagnostics(
        &self,
        file: Node,
        text: &str,
        data: &WriteFileData,
    ) -> String {
        compute_signature_with_diagnostics(file, text, data, self.hash_with_text)
    }

    // Go: incremental/snapshot.go:437 computeHash
    #[must_use]
    pub fn compute_hash(&self, text: &str) -> String {
        compute_hash(text, self.hash_with_text)
    }

    // Go: incremental/snapshot.go:441 canUseIncrementalState
    #[must_use]
    pub fn can_use_incremental_state(&self) -> bool {
        if !self.options.is_incremental() && self.options.build.is_true() {
            // If not incremental build (with tsc -b), we don't need to track state except diagnostics per file so we can use it
            return false;
        }
        true
    }
}

// Go: incremental/snapshot.go:389 getTextHandlingSourceMapForSignature
#[must_use]
pub fn get_text_handling_source_map_for_signature<'a>(
    text: &'a str,
    data: &WriteFileData,
) -> &'a str {
    if data.source_map_url_pos != -1 {
        return &text[..data.source_map_url_pos as usize];
    }
    text
}

// Go: incremental/snapshot.go:396 computeSignatureWithDiagnostics
// PORT: the body of the Go method, as a free function over `hashWithText`,
// so emit `WriteFile` callbacks on the checker threads can call it without
// the snapshot.
#[must_use]
pub fn compute_signature_with_diagnostics(
    file: Node,
    text: &str,
    data: &WriteFileData,
    hash_with_text: bool,
) -> String {
    let mut builder = String::new();
    builder.push_str(get_text_handling_source_map_for_signature(text, data));
    for diag in &data.diagnostics {
        diagnostic_to_string_builder(Some(diag), file, &mut builder);
    }
    compute_hash(&builder, hash_with_text)
}

// Go: incremental/snapshot.go:405 diagnosticToStringBuilder
pub fn diagnostic_to_string_builder(
    diagnostic: Option<&Diagnostic>,
    file: Node,
    builder: &mut String,
) {
    let Some(diagnostic) = diagnostic else {
        return;
    };
    builder.push('\n');
    if diagnostic.file() != file {
        // Go `diagnostic.File().FileName()` dereferences nil for a
        // diagnostic with no file, such as a TS5033 of a failed write in the
        // d.ts `data.Diagnostics` (emitfileshandler.go:212).
        if diagnostic.file().is_nil() {
            crate::core::go_nil_dereference();
        }
        // ts#64159: the file names (not the path keys), compared without
        // case; a file on another root keeps its absolute name
        // (snapshot.go:411).
        let diagnostic_file_name = source_file_file_name(diagnostic.file());
        match relative_path_from_directory(
            &get_directory_path(source_file_file_name(file)),
            diagnostic_file_name,
            false,
        ) {
            Some(relative_path) => {
                builder.push_str(&ensure_path_is_non_module_name(&relative_path));
            }
            None => builder.push_str(diagnostic_file_name),
        }
    }
    if diagnostic.file().is_some() {
        // PORT: Go writes byte offsets (see `go_text_range`).
        let (pos, end) = go_text_range(diagnostic.file(), diagnostic.loc());
        builder.push_str(&format!("({},{}): ", pos, end - pos));
    }
    builder.push_str(diagnostic.category().name());
    builder.push_str(&format!("{}: ", diagnostic.code()));
    builder.push_str(diagnostic.message_key());
    builder.push('\n');
    for arg in diagnostic.message_args() {
        builder.push_str(arg);
        builder.push('\n');
    }
    for chain in diagnostic.message_chain() {
        diagnostic_to_string_builder(Some(chain), file, builder);
    }
    for info in diagnostic.related_information() {
        diagnostic_to_string_builder(Some(info), file, builder);
    }
}
