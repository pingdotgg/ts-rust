//! Port of execute/incremental/snapshottobuildinfo.go.
//!
//! PORT: Go `*compiler.Program` is the current program (`prog()`). Go nil
//! slices in `BuildInfo` are `None`: a Go `append` to a nil slice or a
//! `core.Map` of a non-empty slice makes `Some`, and nothing appended (or a
//! `core.Map` of a nil or empty `slices.Collect`) stays `None`.

use super::build_info::*;
use super::checker_access::*;
use super::hash::FileInfo;
use super::hash::*;
use super::snapshot::*;
use crate::frontend::prelude::*;
use crate::gostd::GoError;
use crate::gostd::slices::stable_sort_by;
use crate::program::source_file_may_be_emitted;

/// Go `core.Map` over a slice that is nil when empty (`slices.Collect`,
/// diagnostic chains, message args).
fn non_empty<T>(items: Vec<T>) -> Option<Vec<T>> {
    if items.is_empty() { None } else { Some(items) }
}

// Go: incremental/snapshottobuildinfo.go:18 snapshotToBuildInfo
// PORT: Go `program.ContentMapperProject()` is the Go frontend program's
// (none when no program is loaded).
pub fn snapshot_to_build_info(
    snapshot: &Snapshot,
    build_info_file_name: &str,
) -> Result<BuildInfo, GoError> {
    let content_mapper_project =
        crate::program::go_frontend_program().and_then(|program| program.content_mapper_project());
    let content_mapper_identities = content_mapper_identities(content_mapper_project.as_deref())?;
    // A standalone API process without the Effect rules writes plain build
    // info (`rulerunner::enabled_options`).
    let effect = crate::effect::rulerunner::enabled_options(snapshot.options);
    let build_info = BuildInfo {
        version: build_info_version(effect.is_some()).into_owned(),
        content_mapper_identities,
        // Effect-TS/tsgo patch 028.
        effect: effect.map(crate::effect::etscore::EffectPluginOptions::to_value),
        ..BuildInfo::default()
    };
    let mut to = ToBuildInfo {
        snapshot,
        build_info,
        build_info_directory: get_directory_path(build_info_file_name),
        compare_paths_options: ComparePathsOptions {
            current_directory: get_current_directory().to_string(),
            use_case_sensitive_file_names: use_case_sensitive_file_names(),
        },
        file_name_to_file_id: FxHashMap::default(),
        file_names_to_file_id_list_id: FxHashMap::default(),
        roots: FxIndexMap::default(),
    };

    if snapshot.options.is_incremental() {
        to.collect_root_files();
        to.set_file_info_and_emit_signatures();
        to.set_root_of_incremental_program();
        to.set_compiler_options();
        to.set_referenced_map();
        to.set_change_file_set();
        to.set_semantic_diagnostics();
        to.set_emit_diagnostics();
        to.set_affected_files_pending_emit();
        if !snapshot.latest_changed_dts_file.is_empty() {
            to.build_info.latest_changed_dts_file =
                to.relative_to_build_info(&snapshot.latest_changed_dts_file);
        }
    } else {
        to.set_root_of_non_incremental_program();
    }
    to.build_info.errors = snapshot.has_errors.is_true();
    to.build_info.semantic_errors = snapshot.has_semantic_errors;
    to.build_info.check_pending = snapshot.check_pending;
    to.set_package_jsons();
    Ok(to.build_info)
}

// Go: incremental/snapshottobuildinfo.go:64 toBuildInfo
// PORT: Go `roots map[*ast.SourceFile]tspath.Path` is an `FxIndexMap`; Go
// sorts its keys before use.
struct ToBuildInfo<'a> {
    snapshot: &'a Snapshot,
    build_info: BuildInfo,
    build_info_directory: String,
    compare_paths_options: ComparePathsOptions,
    file_name_to_file_id: FxHashMap<String, BuildInfoFileId>,
    // PORT: Go keys this map by the ids joined with ","; the sorted id list
    // is the same key without the text.
    file_names_to_file_id_list_id: FxHashMap<Vec<BuildInfoFileId>, BuildInfoFileIdListId>,
    roots: FxIndexMap<Node, Path>,
}

impl ToBuildInfo<'_> {
    // Go: incremental/snapshottobuildinfo.go:75 relativeToBuildInfo
    fn relative_to_build_info(&self, path: &str) -> String {
        ensure_path_is_non_module_name(&get_relative_path_from_directory(
            &self.build_info_directory,
            path,
            &self.compare_paths_options,
        ))
    }

    // Go: incremental/snapshottobuildinfo.go:79 toFileId
    fn to_file_id(&mut self, path: &Path) -> BuildInfoFileId {
        let mut file_id = self
            .file_name_to_file_id
            .get(path.as_str())
            .copied()
            .unwrap_or_default();
        if file_id.0 == 0 {
            let name = match get_default_lib_file(path) {
                Some(lib_file) if !lib_file.replaced => lib_file.name.clone(),
                _ => self.relative_to_build_info(path),
            };
            let file_names = self.build_info.file_names.get_or_insert_with(Vec::new);
            file_names.push(name);
            file_id = BuildInfoFileId(file_names.len() as i32);
            self.file_name_to_file_id.insert(path.0.clone(), file_id);
        }
        file_id
    }

    // Go: incremental/snapshottobuildinfo.go:93 toFileIdListId
    fn to_file_id_list_id(&mut self, set: &FxIndexSet<Path>) -> BuildInfoFileIdListId {
        let mut file_ids: Vec<BuildInfoFileId> =
            set.iter().map(|path| self.to_file_id(path)).collect();
        stable_sort_by(&mut file_ids, Ord::cmp);

        let mut file_id_list_id = self
            .file_names_to_file_id_list_id
            .get(&file_ids)
            .copied()
            .unwrap_or_default();
        if file_id_list_id.0 == 0 {
            let file_ids_list = self.build_info.file_ids_list.get_or_insert_with(Vec::new);
            file_ids_list.push(file_ids.clone());
            file_id_list_id = BuildInfoFileIdListId(file_ids_list.len() as i32);
            self.file_names_to_file_id_list_id
                .insert(file_ids, file_id_list_id);
        }
        file_id_list_id
    }

    // Go: incremental/snapshottobuildinfo.go:109 toRelativeToBuildInfoCompilerOptionValue
    fn to_relative_to_build_info_compiler_option_value(
        &self,
        option: &CommandLineOption,
        v: CompilerOptionsValue,
    ) -> CompilerOptionsValue {
        if option.kind == CommandLineOptionKind::LIST {
            if option
                .elements()
                .is_some_and(|elements| elements.is_file_path)
            {
                if let CompilerOptionsValue::StringList(arr) = &v {
                    return CompilerOptionsValue::StringList(
                        arr.iter()
                            .map(|item| self.relative_to_build_info(item))
                            .collect(),
                    );
                }
            }
        } else if option.is_file_path {
            if let CompilerOptionsValue::String(str_) = &v {
                if !str_.is_empty() {
                    return CompilerOptionsValue::String(self.relative_to_build_info(str_));
                }
            }
        }
        v
    }

    // Go: incremental/snapshottobuildinfo.go:124 toBuildInfoDiagnosticsFromFileNameDiagnostics
    fn to_build_info_diagnostics_from_file_name_diagnostics(
        &mut self,
        diagnostics: &[BuildInfoDiagnosticWithFileName],
    ) -> Vec<BuildInfoDiagnostic> {
        diagnostics
            .iter()
            .map(|d| {
                let mut file = BuildInfoFileId::default();
                if !d.file.is_empty() {
                    file = self.to_file_id(&d.file);
                }
                BuildInfoDiagnostic {
                    file,
                    no_file: d.no_file,
                    pos: d.pos,
                    end: d.end,
                    code: d.code,
                    category: d.category,
                    source: d.source.clone(),
                    message_text: d.message_text.clone(),
                    message_key: d.message_key.clone(),
                    message_args: d.message_args.clone(),
                    // Go `core.Map` keeps nil and empty apart.
                    message_chain: d.message_chain.as_deref().map(|chain| {
                        self.to_build_info_diagnostics_from_file_name_diagnostics(chain)
                    }),
                    related_information: d.related_information.as_deref().map(|info| {
                        self.to_build_info_diagnostics_from_file_name_diagnostics(info)
                    }),
                    reports_unnecessary: d.reports_unnecessary,
                    reports_deprecated: d.reports_deprecated,
                    skipped_on_no_emit: d.skipped_on_no_emit,
                    repopulate_info: to_build_info_repopulate_info(d.repopulate_info.as_deref()),
                }
            })
            .collect()
    }

    // Go: incremental/snapshottobuildinfo.go:151 toBuildInfoDiagnosticsFromDiagnostics
    fn to_build_info_diagnostics_from_diagnostics(
        &mut self,
        file_path: &Path,
        diagnostics: &[Diagnostic],
    ) -> Vec<BuildInfoDiagnostic> {
        diagnostics
            .iter()
            .map(|d| {
                let mut file = BuildInfoFileId::default();
                let mut no_file = false;
                if d.file().is_nil() {
                    no_file = true;
                } else if source_file_info(d.file()).path != file_path.as_str() {
                    file = self.to_file_id(&Path(source_file_info(d.file()).path.clone()));
                }
                // PORT: Go byte offsets (see `go_text_range`).
                let (pos, end) = go_text_range(d.file(), d.loc());
                BuildInfoDiagnostic {
                    file,
                    no_file,
                    pos,
                    end,
                    code: d.code(),
                    category: d.category().0,
                    source: d.source().to_string(),
                    message_text: d.message_text().to_string(),
                    message_key: d.message_key().to_string(),
                    message_args: non_empty(d.message_args().to_vec()),
                    message_chain: non_empty(
                        self.to_build_info_diagnostics_from_diagnostics(
                            file_path,
                            d.message_chain(),
                        ),
                    ),
                    related_information: non_empty(
                        self.to_build_info_diagnostics_from_diagnostics(
                            file_path,
                            d.related_information(),
                        ),
                    ),
                    reports_unnecessary: d.reports_unnecessary(),
                    reports_deprecated: d.reports_deprecated(),
                    skipped_on_no_emit: d.skipped_on_no_emit(),
                    repopulate_info: to_build_info_repopulate_info(d.repopulate_info().as_deref()),
                }
            })
            .collect()
    }

    // Go: incremental/snapshottobuildinfo.go:193 toBuildInfoDiagnosticsOfFile
    fn to_build_info_diagnostics_of_file(
        &mut self,
        file_path: &Path,
        diags: &DiagnosticsOrBuildInfoDiagnosticsWithFileName,
    ) -> Option<BuildInfoDiagnosticsOfFile> {
        if let Some(diagnostics) = diags.diagnostics.as_ref().filter(|d| !d.is_empty()) {
            return Some(BuildInfoDiagnosticsOfFile {
                file_id: self.to_file_id(file_path),
                diagnostics: self
                    .to_build_info_diagnostics_from_diagnostics(file_path, diagnostics),
            });
        }
        if !diags.build_info_diagnostics.is_empty() {
            return Some(BuildInfoDiagnosticsOfFile {
                file_id: self.to_file_id(file_path),
                diagnostics: self.to_build_info_diagnostics_from_file_name_diagnostics(
                    &diags.build_info_diagnostics,
                ),
            });
        }
        None
    }

    // Go: incremental/snapshottobuildinfo.go:209 collectRootFiles
    fn collect_root_files(&mut self) {
        for file_name in command_line().file_names() {
            let redirect = get_parse_file_redirect(file_name);
            let file = if !redirect.is_empty() {
                get_source_file(&redirect)
            } else {
                get_source_file(file_name)
            };
            if file.is_some() {
                self.roots.insert(
                    file,
                    to_path(
                        file_name,
                        &self.compare_paths_options.current_directory,
                        self.compare_paths_options.use_case_sensitive_file_names,
                    ),
                );
            }
        }
    }

    // Go: incremental/snapshottobuildinfo.go:223 setFileInfoAndEmitSignatures
    fn set_file_info_and_emit_signatures(&mut self) {
        let snapshot = self.snapshot;
        let mut file_infos = Vec::new();
        for file in source_files() {
            let file_path = Path(source_file_info(file).path.clone());
            let info = snapshot
                .file_infos
                .get(&file_path)
                .expect("file info of a program file");
            // PERF: the check below computes the relative path of the file
            // again. When `to_file_id` makes the id here, it stores that
            // path or the lib name, which the check accepts, so only an id
            // made before is checked (none today: program paths are unique
            // and this is the first `to_file_id` caller). effect: about 650
            // files.
            let made_before = self.file_name_to_file_id.contains_key(file_path.as_str());
            let file_id = self.to_file_id(&file_path);
            //  tryAddRoot(key, fileId);
            if made_before {
                let stored_name = self.build_info.file_names.as_ref().expect("fileNames")
                    [(file_id.0 - 1) as usize]
                    .clone();
                if stored_name != self.relative_to_build_info(&file_path) {
                    let lib_file = get_default_lib_file(&file_path);
                    if lib_file
                        .is_none_or(|lib_file| lib_file.replaced || stored_name != lib_file.name)
                    {
                        panic!(
                            "File name at index {} does not match expected relative path or libName: {} != {}",
                            file_id.0 - 1,
                            stored_name,
                            self.relative_to_build_info(&file_path)
                        );
                    }
                }
            }
            if snapshot.options.composite.is_true()
                && !is_json_source_file(file)
                && source_file_may_be_emitted(file, false)
            {
                match snapshot.emit_signatures.get(&file_path) {
                    None => {
                        self.build_info
                            .emit_signatures
                            .get_or_insert_with(Vec::new)
                            .push(BuildInfoEmitSignature {
                                file_id,
                                ..Default::default()
                            });
                    }
                    Some(emit_signature) if emit_signature.signature != info.signature => {
                        let mut incremental_emit_signature = BuildInfoEmitSignature {
                            file_id,
                            ..Default::default()
                        };
                        if !emit_signature.signature.is_empty() {
                            incremental_emit_signature.signature = emit_signature.signature.clone();
                        } else {
                            let first = &emit_signature
                                .signature_with_different_options
                                .as_ref()
                                .expect("signatureWithDifferentOptions")[0];
                            if *first == info.signature {
                                incremental_emit_signature.differs_only_in_dts_map = true;
                            } else {
                                incremental_emit_signature.signature = first.clone();
                                incremental_emit_signature.differs_in_options = true;
                            }
                        }
                        self.build_info
                            .emit_signatures
                            .get_or_insert_with(Vec::new)
                            .push(incremental_emit_signature);
                    }
                    Some(_) => {}
                }
            }
            file_infos.push(new_build_info_file_info(info));
        }
        self.build_info.file_infos = Some(file_infos);
    }

    // Go: incremental/snapshottobuildinfo.go:259 setRootOfIncrementalProgram
    fn set_root_of_incremental_program(&mut self) {
        let mut keys: Vec<(Node, BuildInfoFileId)> = Vec::with_capacity(self.roots.len());
        let files: Vec<Node> = self.roots.keys().copied().collect();
        for file in files {
            let id = self.to_file_id(&Path(source_file_info(file).path.clone()));
            keys.push((file, id));
        }
        // Go: incremental/snapshottobuildinfo.go:261 slices.SortFunc(keys, ...) by file id
        crate::gostd::slices::sort_func(&mut keys, |a, b| a.1.cmp(&b.1) as i32);
        for (file, _) in keys {
            let root_path = self.roots[&file].clone();
            let root = self.to_file_id(&root_path);
            let resolved = self.to_file_id(&Path(source_file_info(file).path.clone()));
            match &mut self.build_info.root {
                None => {
                    // First fileId as is
                    self.build_info.root = Some(vec![BuildInfoRoot {
                        start: resolved,
                        ..Default::default()
                    }]);
                }
                Some(roots) => {
                    let last = roots.last_mut().expect("a root");
                    if last.end.0 == resolved.0 - 1 {
                        // If its [..., last = [start, end = fileId - 1]], update last to [start, fileId]
                        last.end = resolved;
                    } else if last.end.0 == 0 && last.start.0 == resolved.0 - 1 {
                        // If its [..., last = start = fileId - 1 ], update last to [start, fileId]
                        last.end = resolved;
                    } else {
                        roots.push(BuildInfoRoot {
                            start: resolved,
                            ..Default::default()
                        });
                    }
                }
            }
            if root != resolved {
                self.build_info
                    .resolved_root
                    .get_or_insert_with(Vec::new)
                    .push(BuildInfoResolvedRoot { resolved, root });
            }
        }
    }

    // Go: incremental/snapshottobuildinfo.go:291 setCompilerOptions
    // PORT: ts#64457 makes Go call the generated
    // `tsoptions.ForEachCompilerOptionAffectingBuildInfo`
    // (options_generated.go:465), which skips unset values. The port keeps
    // `for_each_compiler_option_value` with the `affects_build_info` filter
    // and `is_zero_compiler_option_value`: the same 67 options in the same
    // order (checked at fed0bf24149f).
    fn set_compiler_options(&mut self) {
        let options = self.snapshot.options;
        let field_values = compiler_options_field_values(options);
        let mut values: Vec<(&'static CommandLineOption, CompilerOptionsValue)> = Vec::new();
        for_each_compiler_option_value(
            options,
            &|option| option.affects_build_info,
            &mut |option, value, i| {
                if is_zero_compiler_option_value(field_values[i].0, &value, options) {
                    return false;
                }
                values.push((option, value));
                false
            },
        );
        for (option, value) in values {
            // Make it relative to buildInfo directory if file path
            let value = self.to_relative_to_build_info_compiler_option_value(option, value);
            self.build_info
                .options
                .get_or_insert_with(IndexMap::default)
                .insert(option.name.to_string(), value);
        }
    }

    // Go: incremental/snapshottobuildinfo.go:311 setReferencedMap
    fn set_referenced_map(&mut self) {
        let snapshot = self.snapshot;
        let mut keys = snapshot.referenced_map.get_paths_with_references();
        // PORT: Go sorts by the bytes of the paths (see `compare_go_bytes`).
        stable_sort_by(&mut keys, |a, b| compare_go_bytes(a.as_str(), b.as_str()));
        let entries: Vec<BuildInfoReferenceMapEntry> = keys
            .iter()
            .map(|file_path| {
                let references = snapshot
                    .referenced_map
                    .get_references(file_path)
                    .expect("references of a key");
                BuildInfoReferenceMapEntry {
                    file_id: self.to_file_id(file_path),
                    file_id_list_id: self.to_file_id_list_id(references),
                }
            })
            .collect();
        self.build_info.referenced_map = non_empty(entries);
    }

    // Go: incremental/snapshottobuildinfo.go:323 setChangeFileSet
    fn set_change_file_set(&mut self) {
        let mut files: Vec<Path> = self.snapshot.changed_files_set.iter().cloned().collect();
        stable_sort_by(&mut files, |a, b| compare_go_bytes(a.as_str(), b.as_str()));
        let ids: Vec<BuildInfoFileId> = files.iter().map(|file| self.to_file_id(file)).collect();
        self.build_info.change_file_set = non_empty(ids);
    }

    // Go: incremental/snapshottobuildinfo.go:329 setSemanticDiagnostics
    fn set_semantic_diagnostics(&mut self) {
        let snapshot = self.snapshot;
        for file in source_files() {
            let file_path = Path(source_file_info(file).path.clone());
            match snapshot.semantic_diagnostics_per_file.get(&file_path) {
                None => {
                    if !snapshot.changed_files_set.contains(&file_path) {
                        let file_id = self.to_file_id(&file_path);
                        self.build_info
                            .semantic_diagnostics_per_file
                            .get_or_insert_with(Vec::new)
                            .push(BuildInfoSemanticDiagnostic {
                                file_id,
                                ..Default::default()
                            });
                    }
                }
                Some(value) => {
                    let diagnostics = self.to_build_info_diagnostics_of_file(&file_path, value);
                    if diagnostics.is_some() {
                        self.build_info
                            .semantic_diagnostics_per_file
                            .get_or_insert_with(Vec::new)
                            .push(BuildInfoSemanticDiagnostic {
                                diagnostics,
                                ..Default::default()
                            });
                    }
                }
            }
        }
    }

    // Go: incremental/snapshottobuildinfo.go:349 setEmitDiagnostics
    // PORT: Go `core.Map` keeps a nil entry for a file whose cached lists are
    // both empty. It marshals as `null` (`BuildInfoDiagnosticsOfFilePtr`).
    fn set_emit_diagnostics(&mut self) {
        let snapshot = self.snapshot;
        let mut files: Vec<Path> = snapshot.emit_diagnostics_per_file.keys().cloned().collect();
        stable_sort_by(&mut files, |a, b| compare_go_bytes(a.as_str(), b.as_str()));
        let mut entries = Vec::with_capacity(files.len());
        for file_path in &files {
            let value = &snapshot.emit_diagnostics_per_file[file_path];
            entries.push(BuildInfoDiagnosticsOfFilePtr(
                self.to_build_info_diagnostics_of_file(file_path, value),
            ));
        }
        self.build_info.emit_diagnostics_per_file = non_empty(entries);
    }

    // Go: incremental/snapshottobuildinfo.go:358 setAffectedFilesPendingEmit
    fn set_affected_files_pending_emit(&mut self) {
        let snapshot = self.snapshot;
        let mut files: Vec<Path> = snapshot
            .affected_files_pending_emit
            .keys()
            .cloned()
            .collect();
        stable_sort_by(&mut files, |a, b| compare_go_bytes(a.as_str(), b.as_str()));
        let full_emit_kind = get_file_emit_kind(snapshot.options);
        for file_path in &files {
            let file = get_source_file_by_path(file_path);
            if file.is_nil() || !source_file_may_be_emitted(file, false) {
                continue;
            }
            let pending_emit = snapshot.affected_files_pending_emit[file_path];
            let file_id = self.to_file_id(file_path);
            self.build_info
                .affected_files_pending_emit
                .get_or_insert_with(Vec::new)
                .push(BuildInfoFilePendingEmit {
                    file_id,
                    emit_kind: if pending_emit == full_emit_kind {
                        FileEmitKind::NONE
                    } else {
                        pending_emit
                    },
                });
        }
    }

    // Go: incremental/snapshottobuildinfo.go:375 setRootOfNonIncrementalProgram
    fn set_root_of_non_incremental_program(&mut self) {
        let roots = command_line()
            .file_names()
            .iter()
            .map(|file_name| BuildInfoRoot {
                non_incremental: self.relative_to_build_info(&to_path(
                    file_name,
                    &self.compare_paths_options.current_directory,
                    self.compare_paths_options.use_case_sensitive_file_names,
                )),
                ..Default::default()
            })
            .collect();
        self.build_info.root = Some(roots);
    }

    // Go: incremental/snapshottobuildinfo.go:383 setPackageJsons
    fn set_package_jsons(&mut self) {
        let snapshot = self.snapshot;
        if let Some(package_jsons) = &snapshot.package_jsons
            && !package_jsons.is_empty()
        {
            let package_jsons = package_jsons
                .iter()
                .map(|package_json| self.relative_to_build_info(package_json))
                .collect();
            self.build_info.package_jsons = Some(package_jsons);
        }
        if let Some(missing_package_jsons) = &snapshot.missing_package_jsons
            && !missing_package_jsons.is_empty()
        {
            let missing_package_jsons = missing_package_jsons
                .iter()
                .map(|package_json| self.relative_to_build_info(package_json))
                .collect();
            self.build_info.missing_package_jsons = Some(missing_package_jsons);
        }
    }
}

// Go: incremental/snapshottobuildinfo.go:181 toBuildInfoRepopulateInfo
#[must_use]
pub fn to_build_info_repopulate_info(
    info: Option<&RepopulateDiagnosticInfo>,
) -> Option<BuildInfoRepopulateInfo> {
    let info = info?;
    Some(BuildInfoRepopulateInfo {
        kind: info.kind,
        module_reference: info.module_reference.clone(),
        mode: info.mode,
        package_name: info.package_name.clone(),
    })
}

/// Go `reflect.Value.IsZero` for one `core.CompilerOptions` field, as
/// `setCompilerOptions` uses it.
// PORT: `compiler_options_field_values` lists `Lib` and `TypeRoots` (Go
// `[]string`, Rust `Option<Vec<String>>`) as lists, so those two read the
// field: `None` is the Go nil slice. Other Go slice fields are Rust `Vec`s,
// which cannot tell a nil slice from an empty one; empty counts as zero.
#[must_use]
pub fn is_zero_compiler_option_value(
    field_name: &str,
    value: &CompilerOptionsValue,
    options: &CompilerOptions,
) -> bool {
    use CompilerOptionsValue as V;
    match field_name {
        "Lib" => return options.lib.is_none(),
        "TypeRoots" => return options.type_roots.is_none(),
        _ => {}
    }
    match value {
        V::Nil | V::EmptyStruct => true,
        V::Bool(b) => !*b,
        V::Int(i) => *i == 0,
        V::Number(n) => n.to_bits() == 0,
        V::String(s) => s.is_empty(),
        V::Message(_) => false,
        V::Tristate(t) => *t == Tristate::Unknown,
        V::ScriptTarget(v) => *v == Default::default(),
        V::ModuleKind(v) => *v == Default::default(),
        V::ModuleResolutionKind(v) => *v == Default::default(),
        V::ModuleDetectionKind(v) => *v == Default::default(),
        V::JsxEmit(v) => *v == Default::default(),
        V::NewLineKind(v) => *v == Default::default(),
        V::List(v) => v.is_empty(),
        V::NilList => true,
        V::Map(v) => v.is_empty(),
        V::StringList(v) => v.is_empty(),
        V::Paths(v) => v.is_none(),
        V::IntPtr(v) => v.is_none(),
    }
}
