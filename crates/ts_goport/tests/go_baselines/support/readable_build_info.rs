//! Port of internal/execute/tsctests/readablebuildinfo.go: the
//! `<buildinfo>.readable.baseline.txt` text.
//!
//! PORT: Go marshals the readable structs through JSON v2 reflection and
//! `MarshalJSON` methods. Here each struct implements `MarshalerTo` by hand
//! in Go field order, with Go `omitzero` (a Go `[]T` field is
//! `Option<Vec<T>>`: nil is omitted, an empty slice is `[]`). The
//! `original` values use the production `MarshalerTo` impls of the build
//! info types. The Go `UnmarshalJSON` methods are not ported; no test reads
//! a readable build info back.

use indexmap::IndexMap;
use ts_goport::execute::incremental::build_info::{
    BuildInfo, BuildInfoDiagnostic, BuildInfoDiagnosticsOfFile, BuildInfoEmitSignature,
    BuildInfoFileId, BuildInfoFileIdListId, BuildInfoFileInfo, BuildInfoFilePendingEmit,
    BuildInfoRepopulateInfo, BuildInfoRoot, marshal_any,
};
use ts_goport::execute::incremental::hash::{FileEmitKind, get_file_emit_kind};
use ts_goport::frontend::json::{JsonError, MarshalerTo, json_marshal_indent};
use ts_goport::scanner_util::go_len;

/// Writes the members of a Go struct in field order, with `omitzero`.
struct ObjectWriter<'e> {
    enc: &'e mut String,
    first: bool,
}

impl<'e> ObjectWriter<'e> {
    fn new(enc: &'e mut String) -> ObjectWriter<'e> {
        enc.push('{');
        ObjectWriter { enc, first: true }
    }

    fn name(&mut self, name: &str) -> &mut String {
        if !self.first {
            self.enc.push(',');
        }
        self.first = false;
        self.enc.push('"');
        self.enc.push_str(name);
        self.enc.push_str("\":");
        self.enc
    }

    fn bool_omitzero(&mut self, name: &str, v: bool) {
        if v {
            self.name(name).push_str("true");
        }
    }

    fn int_omitzero(&mut self, name: &str, v: i64) {
        if v != 0 {
            self.name(name).push_str(&v.to_string());
        }
    }

    fn string_omitzero(&mut self, name: &str, v: &str) -> Result<(), JsonError> {
        if !v.is_empty() {
            v.marshal_json_to(self.name(name))?;
        }
        Ok(())
    }

    fn slice_omitzero<T: MarshalerTo>(
        &mut self,
        name: &str,
        v: Option<&Vec<T>>,
    ) -> Result<(), JsonError> {
        if let Some(v) = v {
            v.marshal_json_to(self.name(name))?;
        }
        Ok(())
    }

    fn value<T: MarshalerTo + ?Sized>(&mut self, name: &str, v: &T) -> Result<(), JsonError> {
        v.marshal_json_to(self.name(name))
    }

    fn end(self) {
        self.enc.push('}');
    }
}

// Go: tsctests/readablebuildinfo.go:15 readableBuildInfo
struct ReadableBuildInfo<'a> {
    build_info: &'a BuildInfo,
    version: String,

    // Common between incremental and tsc -b buildinfo for non incremental programs
    errors: bool,
    check_pending: bool,
    root: Option<Vec<ReadableBuildInfoRoot<'a>>>,
    package_jsons: Option<Vec<String>>,
    missing_package_jsons: Option<Vec<String>>,

    // IncrementalProgram info
    file_names: Option<Vec<String>>,
    file_infos: Option<Vec<ReadableBuildInfoFileInfo<'a>>>,
    file_ids_list: Option<Vec<Vec<String>>>,
    // PORT: Go shares the `*OrderedMap` of the build info; this is its
    // `options` field (`build_info.options`).
    referenced_map: Option<IndexMap<String, Vec<String>>>,
    semantic_diagnostics_per_file: Option<Vec<ReadableBuildInfoSemanticDiagnostic>>,
    emit_diagnostics_per_file: Option<Vec<ReadableBuildInfoDiagnosticsOfFile>>,
    change_file_set: Option<Vec<String>>, // List of changed files in the program, not the whole set of files
    affected_files_pending_emit: Option<Vec<ReadableBuildInfoFilePendingEmit<'a>>>,
    latest_changed_dts_file: String, // Because this is only output file in the program, we dont need fileId to deduplicate name
    emit_signatures: Option<Vec<ReadableBuildInfoEmitSignature<'a>>>,
    resolved_root: Option<Vec<ReadableBuildInfoResolvedRoot>>,
    size: usize, // Size of the build info file

    // NonIncrementalProgram info
    semantic_errors: bool,
}

impl MarshalerTo for ReadableBuildInfo<'_> {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        let mut w = ObjectWriter::new(enc);
        w.string_omitzero("version", &self.version)?;
        w.bool_omitzero("errors", self.errors);
        w.bool_omitzero("checkPending", self.check_pending);
        w.slice_omitzero("root", self.root.as_ref())?;
        w.slice_omitzero("packageJsons", self.package_jsons.as_ref())?;
        w.slice_omitzero("missingPackageJsons", self.missing_package_jsons.as_ref())?;
        w.slice_omitzero("fileNames", self.file_names.as_ref())?;
        w.slice_omitzero("fileInfos", self.file_infos.as_ref())?;
        w.slice_omitzero("fileIdsList", self.file_ids_list.as_ref())?;
        if let Some(options) = &self.build_info.options {
            // Go: collections/ordered_map.go:217 MarshalJSONTo
            let enc = w.name("options");
            enc.push('{');
            for (i, (k, v)) in options.iter().enumerate() {
                if i > 0 {
                    enc.push(',');
                }
                k.marshal_json_to(enc)?;
                enc.push(':');
                marshal_any(enc, v)?;
            }
            enc.push('}');
        }
        if let Some(referenced_map) = &self.referenced_map {
            // Go: collections/ordered_map.go:217 MarshalJSONTo
            let enc = w.name("referencedMap");
            enc.push('{');
            for (i, (k, v)) in referenced_map.iter().enumerate() {
                if i > 0 {
                    enc.push(',');
                }
                k.marshal_json_to(enc)?;
                enc.push(':');
                v.marshal_json_to(enc)?;
            }
            enc.push('}');
        }
        w.slice_omitzero(
            "semanticDiagnosticsPerFile",
            self.semantic_diagnostics_per_file.as_ref(),
        )?;
        w.slice_omitzero(
            "emitDiagnosticsPerFile",
            self.emit_diagnostics_per_file.as_ref(),
        )?;
        w.slice_omitzero("changeFileSet", self.change_file_set.as_ref())?;
        w.slice_omitzero(
            "affectedFilesPendingEmit",
            self.affected_files_pending_emit.as_ref(),
        )?;
        w.string_omitzero("latestChangedDtsFile", &self.latest_changed_dts_file)?;
        w.slice_omitzero("emitSignatures", self.emit_signatures.as_ref())?;
        w.slice_omitzero("resolvedRoot", self.resolved_root.as_ref())?;
        w.int_omitzero("size", self.size as i64);
        w.bool_omitzero("semanticErrors", self.semantic_errors);
        w.end();
        Ok(())
    }
}

// Go: tsctests/readablebuildinfo.go:45 readableBuildInfoRoot
struct ReadableBuildInfoRoot<'a> {
    files: Vec<String>,
    original: &'a BuildInfoRoot,
}

impl MarshalerTo for ReadableBuildInfoRoot<'_> {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        let mut w = ObjectWriter::new(enc);
        // Go `Files []string` with omitzero: `setRoot` always sets a non-nil slice.
        w.value("files", &self.files)?;
        w.value("original", self.original)?;
        w.end();
        Ok(())
    }
}

// Go: tsctests/readablebuildinfo.go:50 readableBuildInfoFileInfo
struct ReadableBuildInfoFileInfo<'a> {
    file_name: String,
    version: String,
    signature: String,
    affects_global_scope: bool,
    implied_node_format: String,
    original: Option<&'a BuildInfoFileInfo>, // Original file path, if available
}

impl MarshalerTo for ReadableBuildInfoFileInfo<'_> {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        let mut w = ObjectWriter::new(enc);
        w.string_omitzero("fileName", &self.file_name)?;
        w.string_omitzero("version", &self.version)?;
        w.string_omitzero("signature", &self.signature)?;
        w.bool_omitzero("affectsGlobalScope", self.affects_global_scope);
        w.string_omitzero("impliedNodeFormat", &self.implied_node_format)?;
        if let Some(original) = self.original {
            w.value("original", original)?;
        }
        w.end();
        Ok(())
    }
}

// Go: tsctests/readablebuildinfo.go:59 readableBuildInfoDiagnostic
struct ReadableBuildInfoDiagnostic {
    // incrementalBuildInfoFileId if it is for a File thats other than its stored for
    file: String,
    no_file: bool,
    pos: i32,
    end: i32,
    code: i32,
    category: i32,
    message_key: String,
    message_args: Option<Vec<String>>,
    message_chain: Option<Vec<ReadableBuildInfoDiagnostic>>,
    related_information: Option<Vec<ReadableBuildInfoDiagnostic>>,
    reports_unnecessary: bool,
    reports_deprecated: bool,
    skipped_on_no_emit: bool,
    repopulate_info: Option<ReadableBuildInfoRepopulateInfo>,
}

impl MarshalerTo for ReadableBuildInfoDiagnostic {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        let mut w = ObjectWriter::new(enc);
        w.string_omitzero("file", &self.file)?;
        w.bool_omitzero("noFile", self.no_file);
        w.int_omitzero("pos", i64::from(self.pos));
        w.int_omitzero("end", i64::from(self.end));
        w.int_omitzero("code", i64::from(self.code));
        w.int_omitzero("category", i64::from(self.category));
        w.string_omitzero("messageKey", &self.message_key)?;
        w.slice_omitzero("messageArgs", self.message_args.as_ref())?;
        w.slice_omitzero("messageChain", self.message_chain.as_ref())?;
        w.slice_omitzero("relatedInformation", self.related_information.as_ref())?;
        w.bool_omitzero("reportsUnnecessary", self.reports_unnecessary);
        w.bool_omitzero("reportsDeprecated", self.reports_deprecated);
        w.bool_omitzero("skippedOnNoEmit", self.skipped_on_no_emit);
        if let Some(repopulate_info) = &self.repopulate_info {
            w.value("repopulateInfo", repopulate_info)?;
        }
        w.end();
        Ok(())
    }
}

// Go: tsctests/readablebuildinfo.go:77 readableBuildInfoRepopulateInfo
struct ReadableBuildInfoRepopulateInfo {
    kind: i32,
    module_reference: String,
    mode: i32,
    package_name: String,
}

impl MarshalerTo for ReadableBuildInfoRepopulateInfo {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        let mut w = ObjectWriter::new(enc);
        w.name("kind").push_str(&self.kind.to_string());
        w.string_omitzero("moduleReference", &self.module_reference)?;
        w.int_omitzero("mode", i64::from(self.mode));
        w.string_omitzero("packageName", &self.package_name)?;
        w.end();
        Ok(())
    }
}

// Go: tsctests/readablebuildinfo.go:84 readableBuildInfoDiagnosticsOfFile
struct ReadableBuildInfoDiagnosticsOfFile {
    file: String,
    diagnostics: Option<Vec<ReadableBuildInfoDiagnostic>>,
}

impl MarshalerTo for ReadableBuildInfoDiagnosticsOfFile {
    // Go: tsctests/readablebuildinfo.go:78 MarshalJSON
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        enc.push('[');
        self.file.marshal_json_to(enc)?;
        enc.push(',');
        // Go v2 marshals a nil slice inside `[]any` as `[]`.
        match &self.diagnostics {
            Some(diagnostics) => diagnostics.marshal_json_to(enc)?,
            None => enc.push_str("[]"),
        }
        enc.push(']');
        Ok(())
    }
}

// Go: tsctests/readablebuildinfo.go:119 readableBuildInfoSemanticDiagnostic
struct ReadableBuildInfoSemanticDiagnostic {
    file: String, // File is not in changedSet and still doesnt have cached diagnostics
    diagnostics: Option<ReadableBuildInfoDiagnosticsOfFile>, // Diagnostics for file
}

impl MarshalerTo for ReadableBuildInfoSemanticDiagnostic {
    // Go: tsctests/readablebuildinfo.go:113 MarshalJSON
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        if !self.file.is_empty() {
            return self.file.marshal_json_to(enc);
        }
        match &self.diagnostics {
            Some(diagnostics) => diagnostics.marshal_json_to(enc),
            // Go `json.Marshal` of a nil pointer.
            None => {
                enc.push_str("null");
                Ok(())
            }
        }
    }
}

// Go: tsctests/readablebuildinfo.go:149 readableBuildInfoFilePendingEmit
struct ReadableBuildInfoFilePendingEmit<'a> {
    file: String,
    emit_kind: String,
    original: &'a BuildInfoFilePendingEmit,
}

impl MarshalerTo for ReadableBuildInfoFilePendingEmit<'_> {
    // Go: tsctests/readablebuildinfo.go:144 MarshalJSON
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        enc.push('[');
        self.file.marshal_json_to(enc)?;
        enc.push(',');
        self.emit_kind.marshal_json_to(enc)?;
        enc.push(',');
        self.original.marshal_json_to(enc)?;
        enc.push(']');
        Ok(())
    }
}

// Go: tsctests/readablebuildinfo.go:189 readableBuildInfoEmitSignature
struct ReadableBuildInfoEmitSignature<'a> {
    file: String,
    signature: String,
    differs_only_in_dts_map: bool,
    differs_in_options: bool,
    original: &'a BuildInfoEmitSignature,
}

impl MarshalerTo for ReadableBuildInfoEmitSignature<'_> {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        let mut w = ObjectWriter::new(enc);
        w.string_omitzero("file", &self.file)?;
        w.string_omitzero("signature", &self.signature)?;
        w.bool_omitzero("differsOnlyInDtsMap", self.differs_only_in_dts_map);
        w.bool_omitzero("differsInOptions", self.differs_in_options);
        w.value("original", self.original)?;
        w.end();
        Ok(())
    }
}

// Go: tsctests/readablebuildinfo.go:197 readableBuildInfoResolvedRoot
struct ReadableBuildInfoResolvedRoot {
    resolved: String,
    root: String,
}

impl MarshalerTo for ReadableBuildInfoResolvedRoot {
    // Go: tsctests/readablebuildinfo.go:191 MarshalJSON
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        enc.push('[');
        self.resolved.marshal_json_to(enc)?;
        enc.push(',');
        self.root.marshal_json_to(enc)?;
        enc.push(']');
        Ok(())
    }
}

// Go: tsctests/readablebuildinfo.go:218 toReadableBuildInfo
/// The readable build info text: Go `json.MarshalIndent(readable, "", "  ")`.
/// `build_info_text` is the (sanitized) build info file text; only its Go
/// byte length is used.
pub fn to_readable_build_info(build_info: &BuildInfo, build_info_text: &str) -> String {
    let mut readable = ReadableBuildInfo {
        build_info,
        version: build_info.version.clone(),
        errors: build_info.errors,
        check_pending: build_info.check_pending,
        root: None,
        package_jsons: build_info.package_jsons.clone(),
        missing_package_jsons: build_info.missing_package_jsons.clone(),
        file_names: build_info.file_names.clone(),
        file_infos: None,
        file_ids_list: None,
        referenced_map: None,
        semantic_diagnostics_per_file: None,
        emit_diagnostics_per_file: None,
        change_file_set: None,
        affected_files_pending_emit: None,
        latest_changed_dts_file: build_info.latest_changed_dts_file.clone(),
        emit_signatures: None,
        resolved_root: None,
        semantic_errors: build_info.semantic_errors,
        size: go_len(build_info_text),
    };
    readable.set_file_infos();
    readable.set_root();
    readable.set_file_ids_list();
    readable.set_referenced_map();
    readable.set_change_file_set();
    readable.set_semantic_diagnostics();
    readable.set_emit_diagnostics();
    readable.set_affected_files_pending_emit();
    readable.set_emit_signatures();
    readable.set_resolved_root();
    json_marshal_indent(&readable, "", "  ").unwrap_or_else(|err| {
        panic!("readableBuildInfo: failed to marshal readable build info: {err}")
    })
}

impl ReadableBuildInfo<'_> {
    // Go: tsctests/readablebuildinfo.go:253 toFilePath
    // PORT: Go indexes `FileNames` and panics on an id out of range;
    // `BuildInfo::file_name` gives "" there, so it is not used.
    fn to_file_path(&self, file_id: BuildInfoFileId) -> String {
        self.build_info.file_names.as_ref().expect("fileNames")[(file_id.0 - 1) as usize].clone()
    }

    // Go: tsctests/readablebuildinfo.go:257 toFilePathSet
    fn to_file_path_set(&self, file_id_list_id: BuildInfoFileIdListId) -> Vec<String> {
        self.file_ids_list.as_ref().expect("fileIdsList")[(file_id_list_id.0 - 1) as usize].clone()
    }

    // Go: tsctests/readablebuildinfo.go:261 toReadableBuildInfoDiagnostic
    // PORT: Go `core.Map` returns nil for a nil slice; the caller passes the
    // `Option`.
    fn to_readable_build_info_diagnostic(
        &self,
        diagnostics: &[BuildInfoDiagnostic],
    ) -> Vec<ReadableBuildInfoDiagnostic> {
        diagnostics
            .iter()
            .map(|d| {
                let file = if d.file.0 != 0 {
                    self.to_file_path(d.file)
                } else {
                    String::new()
                };
                ReadableBuildInfoDiagnostic {
                    file,
                    no_file: d.no_file,
                    pos: d.pos,
                    end: d.end,
                    code: d.code,
                    category: d.category,
                    message_key: d.message_key.clone(),
                    message_args: d.message_args.clone(),
                    message_chain: d
                        .message_chain
                        .as_deref()
                        .map(|chain| self.to_readable_build_info_diagnostic(chain)),
                    related_information: d
                        .related_information
                        .as_deref()
                        .map(|related| self.to_readable_build_info_diagnostic(related)),
                    reports_unnecessary: d.reports_unnecessary,
                    reports_deprecated: d.reports_deprecated,
                    skipped_on_no_emit: d.skipped_on_no_emit,
                    repopulate_info: to_readable_build_info_repopulate_info(
                        d.repopulate_info.as_ref(),
                    ),
                }
            })
            .collect()
    }

    // Go: tsctests/readablebuildinfo.go:298 toReadableBuildInfoDiagnosticsOfFile
    fn to_readable_build_info_diagnostics_of_file(
        &self,
        diagnostics: &BuildInfoDiagnosticsOfFile,
    ) -> ReadableBuildInfoDiagnosticsOfFile {
        ReadableBuildInfoDiagnosticsOfFile {
            file: self.to_file_path(diagnostics.file_id),
            diagnostics: Some(self.to_readable_build_info_diagnostic(&diagnostics.diagnostics)),
        }
    }

    // Go: tsctests/readablebuildinfo.go:305 setFileInfos
    fn set_file_infos(&mut self) {
        let build_info = self.build_info;
        self.file_infos = build_info.file_infos.as_ref().map(|file_infos| {
            file_infos
                .iter()
                .enumerate()
                .map(|(index, original)| {
                    let file_info = original.get_file_info();
                    ReadableBuildInfoFileInfo {
                        file_name: self.to_file_path(BuildInfoFileId(index as i32 + 1)),
                        version: file_info.version.clone(),
                        signature: file_info.signature.clone(),
                        affects_global_scope: file_info.affects_global_scope,
                        implied_node_format: file_info.implied_node_format.string(),
                        // Dont set original for string encoding
                        original: if original.has_signature() {
                            None
                        } else {
                            Some(original)
                        },
                    }
                })
                .collect()
        });
    }

    // Go: tsctests/readablebuildinfo.go:323 setRoot
    fn set_root(&mut self) {
        let build_info = self.build_info;
        self.root = build_info.root.as_ref().map(|roots| {
            roots
                .iter()
                .map(|original| {
                    let files = if !original.non_incremental.is_empty() {
                        vec![original.non_incremental.clone()]
                    } else if original.end.0 == 0 {
                        vec![self.to_file_path(original.start)]
                    } else {
                        (original.start.0..=original.end.0)
                            .map(|i| self.to_file_path(BuildInfoFileId(i)))
                            .collect()
                    };
                    ReadableBuildInfoRoot { files, original }
                })
                .collect()
        });
    }

    // Go: tsctests/readablebuildinfo.go:343 setFileIdsList
    fn set_file_ids_list(&mut self) {
        let build_info = self.build_info;
        self.file_ids_list = build_info.file_ids_list.as_ref().map(|lists| {
            lists
                .iter()
                .map(|ids| ids.iter().map(|&id| self.to_file_path(id)).collect())
                .collect()
        });
    }

    // Go: tsctests/readablebuildinfo.go:349 setReferencedMap
    // PORT: Go `OrderedMap.Set` keeps the first position of a key and
    // replaces its value, as `IndexMap::insert` does.
    fn set_referenced_map(&mut self) {
        let build_info = self.build_info;
        if let Some(entries) = &build_info.referenced_map {
            let mut referenced_map = IndexMap::new();
            for entry in entries {
                referenced_map.insert(
                    self.to_file_path(entry.file_id),
                    self.to_file_path_set(entry.file_id_list_id),
                );
            }
            self.referenced_map = Some(referenced_map);
        }
    }

    // Go: tsctests/readablebuildinfo.go:358 setChangeFileSet
    fn set_change_file_set(&mut self) {
        let build_info = self.build_info;
        self.change_file_set = build_info
            .change_file_set
            .as_ref()
            .map(|ids| ids.iter().map(|&id| self.to_file_path(id)).collect());
    }

    // Go: tsctests/readablebuildinfo.go:362 setSemanticDiagnostics
    fn set_semantic_diagnostics(&mut self) {
        let build_info = self.build_info;
        self.semantic_diagnostics_per_file =
            build_info
                .semantic_diagnostics_per_file
                .as_ref()
                .map(|list| {
                    list.iter()
                        .map(|diagnostics| {
                            if diagnostics.file_id.0 != 0 {
                                return ReadableBuildInfoSemanticDiagnostic {
                                    file: self.to_file_path(diagnostics.file_id),
                                    diagnostics: None,
                                };
                            }
                            // Go dereferences `Diagnostics` (a nil pointer panics).
                            let of_file = diagnostics
                                .diagnostics
                                .as_ref()
                                .expect("invalid memory address or nil pointer dereference");
                            ReadableBuildInfoSemanticDiagnostic {
                                file: String::new(),
                                diagnostics: Some(
                                    self.to_readable_build_info_diagnostics_of_file(of_file),
                                ),
                            }
                        })
                        .collect()
                });
    }

    // Go: tsctests/readablebuildinfo.go:375 setEmitDiagnostics
    fn set_emit_diagnostics(&mut self) {
        let build_info = self.build_info;
        self.emit_diagnostics_per_file =
            build_info.emit_diagnostics_per_file.as_ref().map(|list| {
                list.iter()
                    .map(|ptr| {
                        // Deref panics on a nil element, like Go.
                        let diagnostics: &BuildInfoDiagnosticsOfFile = ptr;
                        self.to_readable_build_info_diagnostics_of_file(diagnostics)
                    })
                    .collect()
            });
    }

    // Go: tsctests/readablebuildinfo.go:379 setAffectedFilesPendingEmit
    fn set_affected_files_pending_emit(&mut self) {
        let build_info = self.build_info;
        let Some(list) = &build_info.affected_files_pending_emit else {
            return;
        };
        let full_emit_kind = get_file_emit_kind(&build_info.get_compiler_options(""));
        self.affected_files_pending_emit = Some(
            list.iter()
                .map(|pending_emit| {
                    let emit_kind = if pending_emit.emit_kind.0 == 0 {
                        full_emit_kind
                    } else {
                        pending_emit.emit_kind
                    };
                    ReadableBuildInfoFilePendingEmit {
                        file: self.to_file_path(pending_emit.file_id),
                        emit_kind: to_readable_file_emit_kind(emit_kind),
                        original: pending_emit,
                    }
                })
                .collect(),
        );
    }

    // Go: tsctests/readablebuildinfo.go:434 setEmitSignatures
    fn set_emit_signatures(&mut self) {
        let build_info = self.build_info;
        self.emit_signatures = build_info.emit_signatures.as_ref().map(|list| {
            list.iter()
                .map(|signature| ReadableBuildInfoEmitSignature {
                    file: self.to_file_path(signature.file_id),
                    signature: signature.signature.clone(),
                    differs_only_in_dts_map: signature.differs_only_in_dts_map,
                    differs_in_options: signature.differs_in_options,
                    original: signature,
                })
                .collect()
        });
    }

    // Go: tsctests/readablebuildinfo.go:446 setResolvedRoot
    fn set_resolved_root(&mut self) {
        let build_info = self.build_info;
        self.resolved_root = build_info.resolved_root.as_ref().map(|list| {
            list.iter()
                .map(|original| ReadableBuildInfoResolvedRoot {
                    resolved: self.to_file_path(original.resolved),
                    root: self.to_file_path(original.root),
                })
                .collect()
        });
    }
}

// Go: tsctests/readablebuildinfo.go:286 toReadableBuildInfoRepopulateInfo
// PORT: a free function after the `readableBuildInfo` methods (Go puts it
// between them).
fn to_readable_build_info_repopulate_info(
    info: Option<&BuildInfoRepopulateInfo>,
) -> Option<ReadableBuildInfoRepopulateInfo> {
    let info = info?;
    Some(ReadableBuildInfoRepopulateInfo {
        kind: info.kind.0,
        module_reference: info.module_reference.clone(),
        mode: info.mode.0,
        package_name: info.package_name.clone(),
    })
}

// Go: tsctests/readablebuildinfo.go:394 toReadableFileEmitKind
fn to_readable_file_emit_kind(file_emit_kind: FileEmitKind) -> String {
    let mut builder = String::new();
    let mut add_flags = |flags: &str| {
        if builder.is_empty() {
            builder.push_str(flags);
        } else {
            builder.push('|');
            builder.push_str(flags);
        }
    };
    let has = |flag: FileEmitKind| file_emit_kind.0 & flag.0 != 0;
    if file_emit_kind.0 != 0 {
        if has(FileEmitKind::JS) {
            add_flags("Js");
        }
        if has(FileEmitKind::JS_MAP) {
            add_flags("JsMap");
        }
        if has(FileEmitKind::JS_INLINE_MAP) {
            add_flags("JsInlineMap");
        }
        if file_emit_kind.0 & FileEmitKind::DTS.0 == FileEmitKind::DTS.0 {
            add_flags("Dts");
        } else {
            if has(FileEmitKind::DTS_EMIT) {
                add_flags("DtsEmit");
            }
            if has(FileEmitKind::DTS_ERRORS) {
                add_flags("DtsErrors");
            }
        }
        if has(FileEmitKind::DTS_MAP) {
            add_flags("DtsMap");
        }
    }
    if !builder.is_empty() {
        return builder;
    }
    "None".to_string()
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use ts_goport::execute::incremental::build_info::BuildInfo;
    use ts_goport::frontend::json::json_unmarshal;
    use ts_goport::scanner_util::{go_string_bytes, go_string_from_bytes};

    use super::to_readable_build_info;
    use crate::support::baseline::{first_difference, reference_root};

    const READABLE_SUFFIX: &str = ".readable.baseline.txt";

    /// Every file under `dir`.
    fn files_under(dir: &Path, out: &mut Vec<PathBuf>) {
        let entries = std::fs::read_dir(dir)
            .unwrap_or_else(|err| panic!("failed to read directory {}: {err}", dir.display()));
        for entry in entries {
            let path = entry.expect("directory entry").path();
            if path.is_dir() {
                files_under(&path, out);
            } else {
                out.push(path);
            }
        }
    }

    /// The path of a `//// [<path>] *new* ` or `//// [<path>] *modified* `
    /// line, whose content follows on the next lines.
    fn content_header(line: &str) -> Option<&str> {
        let rest = line.strip_prefix("//// [")?;
        rest.strip_suffix("] *new* ")
            .or_else(|| rest.strip_suffix("] *modified* "))
    }

    /// Checks each readable build info text in the reference baselines under
    /// `folders` against `to_readable_build_info` of the build info text
    /// written just before it. Returns the number of checked pairs.
    fn check_reference_folders(folders: &[&str]) -> usize {
        let root = reference_root();
        let mut files = Vec::new();
        for folder in folders {
            files_under(&root.join(folder), &mut files);
        }
        files.sort();
        let mut checked = 0usize;
        let mut failures = Vec::new();
        for file in &files {
            let bytes = std::fs::read(file)
                .unwrap_or_else(|err| panic!("failed to read {}: {err}", file.display()));
            let text = go_string_from_bytes(bytes);
            let lines: Vec<&str> = text.split('\n').collect();
            // The last written text of each build info file.
            let mut build_infos: std::collections::HashMap<&str, String> =
                std::collections::HashMap::new();
            let mut i = 0usize;
            while i < lines.len() {
                let Some(path) = content_header(lines[i]) else {
                    i += 1;
                    continue;
                };
                if let Some(build_info_path) = path.strip_suffix(READABLE_SUFFIX) {
                    // The readable text is indented JSON: it ends at the
                    // first line that is a top-level "}".
                    let end = (i + 1..lines.len())
                        .find(|&j| lines[j] == "}")
                        .unwrap_or_else(|| {
                            panic!("{}: unterminated readable text for {path}", file.display())
                        });
                    let expected = lines[i + 1..=end].join("\n");
                    i = end + 1;
                    let Some(build_info_text) = build_infos.get(build_info_path) else {
                        failures.push(format!(
                            "{}: no build info text before {path}",
                            file.display()
                        ));
                        continue;
                    };
                    checked += 1;
                    let mut build_info = BuildInfo::default();
                    if let Err(err) =
                        json_unmarshal(&go_string_bytes(build_info_text), &mut build_info, &[])
                    {
                        failures.push(format!(
                            "{}: {build_info_path}: failed to unmarshal the build info: {err}",
                            file.display()
                        ));
                        continue;
                    }
                    let actual = to_readable_build_info(&build_info, build_info_text);
                    let expected_bytes = go_string_bytes(&expected);
                    let actual_bytes = go_string_bytes(&actual);
                    if expected_bytes != actual_bytes {
                        failures.push(format!(
                            "{}: {path}\n{}",
                            file.display(),
                            first_difference(&expected_bytes, &actual_bytes)
                        ));
                    }
                } else if path.ends_with(".tsbuildinfo") {
                    // Compact JSON is one line; stop at the next entry or
                    // the blank line that ends the section.
                    let end = (i + 1..lines.len())
                        .find(|&j| lines[j].is_empty() || lines[j].starts_with("//// ["))
                        .unwrap_or(lines.len());
                    build_infos.insert(path, lines[i + 1..end].join("\n"));
                    i = end;
                } else {
                    i += 1;
                }
            }
        }
        assert!(
            failures.is_empty(),
            "{} of {checked} readable build infos differ:\n{}",
            failures.len(),
            failures.join("\n")
        );
        checked
    }

    // Self-check of `to_readable_build_info` against the Go reference
    // baselines of tsc/incremental.
    #[test]
    fn readable_build_info_self_check() {
        let checked = check_reference_folders(&["tsc/incremental"]);
        assert!(
            checked > 0,
            "no readable build info found in tsc/incremental"
        );
    }

    // The same check over every tsc, tsbuild, tscWatch and tsbuildWatch
    // reference baseline.
    #[test]
    fn readable_build_info_self_check_all_folders() {
        let checked = check_reference_folders(&["tsc", "tsbuild", "tscWatch", "tsbuildWatch"]);
        assert!(checked > 0, "no readable build info found");
    }
}
