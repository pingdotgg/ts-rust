//! Go `internal/compiler/includeprocessor.go`: include reasons per file,
//! the processing diagnostics, and caches used to explain why a file is in
//! the program.

use crate::frontend::prelude::*;
use std::cell::OnceCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Go `fileIncludeData` and `includeProcessor`.
// Go: includeprocessor.go:15 fileIncludeData, includeprocessor.go:20
// includeProcessor (ts#64519: the processed files keep the reasons and the
// processing diagnostics, and each program has its own includeProcessor
// with the caches)
// PORT: one struct holds both. The processed files hold it, and its `Clone`
// (`ReuseProgram`) shares the reasons and the diagnostics and starts the
// caches empty, which is the Go split.
// PORT: Go `collections.SyncMap` caches are `RefCell` maps (single thread).
// Maps keyed by `*FileIncludeReason` use the `Rc` pointer as the key, like
// Go pointer keys. Go `sync.Once` values are `OnceCell`.
#[derive(Default)]
pub struct IncludeProcessor {
    /// Shared by the program versions that `UpdateProgram` makes, as in Go.
    pub file_include_reasons: Rc<FxHashMap<Path, Vec<Rc<FileIncludeReason>>>>,
    pub processing_diagnostics: Vec<Rc<ProcessingDiagnostic>>,
    // PORT: Go `checkSourceFilesBelongToPath` appends processing diagnostics
    // from `CommonSourceDirectory`, which takes `&self` here. They are kept
    // apart and read after `processing_diagnostics`, the Go append order.
    pub late_processing_diagnostics: RefCell<Vec<Rc<ProcessingDiagnostic>>>,

    // ts#64519
    pub(crate) reason_diagnostics: RefCell<FxHashMap<IncludeReasonDiagnosticKey, Diagnostic>>,
    pub(crate) reason_to_reference_location:
        RefCell<FxHashMap<*const FileIncludeReason, Rc<ReferenceFileLocation>>>,
    pub(crate) include_reason_to_related_info:
        RefCell<FxHashMap<*const FileIncludeReason, Option<Diagnostic>>>,
    pub(crate) redirect_and_file_format: RefCell<FxHashMap<Path, Vec<Diagnostic>>>,
    // PORT: Go returns a shared `*ast.DiagnosticsCollection` that callers
    // sort in place. Here callers borrow the `RefCell` mutably.
    pub(crate) computed_diagnostics: OnceCell<RefCell<DiagnosticsCollection>>,
    /// PORT: Go builds `computed_diagnostics` lazily, from
    /// `GetProgramDiagnostics` or `GetIncludeProcessorDiagnostics`. The port
    /// builds it when it makes the program (`GoSharedState::new`), before
    /// `ExplainFiles` can run. The `redirectAndFileFormat` lines that the
    /// collection makes (with absolute names) wait here until the port
    /// reaches a point where Go builds the collection
    /// (`mark_diagnostics_read`). Until then `ExplainFiles` makes its own
    /// lines with relative names, as in Go.
    pub(crate) collection_redirect_and_file_format: RefCell<FxHashMap<Path, Vec<Diagnostic>>>,
    /// True once the port reached a point where Go builds
    /// `computed_diagnostics`. The checker threads set it
    /// (`GoSharedState::get_include_processor_diagnostics`), so it is shared.
    pub(crate) diagnostics_read: Arc<AtomicBool>,
    // PORT: Go nil `*ast.ObjectLiteralExpression` is `Node::NIL`.
    pub(crate) compiler_options_syntax: OnceCell<Node>,
}

/// Go `includeReasonDiagnosticKey` (ts#64519).
// Go: includeprocessor.go:31 includeReasonDiagnosticKey
// PORT: the Go `*FileIncludeReason` key is the reason's address.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct IncludeReasonDiagnosticKey {
    pub reason: *const FileIncludeReason,
    pub relative_file_name: bool,
    // ts#64159 (includeprocessor.go:34)
    pub relative_to: String,
}

// PORT: Go `ReuseProgram` copies `processedFiles` (the reasons and the
// processing diagnostics) and gives the new program an empty
// `includeProcessor` (ts#64519). A clone shares the reasons and the
// diagnostics, and its caches start empty.
impl Clone for IncludeProcessor {
    fn clone(&self) -> Self {
        IncludeProcessor {
            file_include_reasons: self.file_include_reasons.clone(),
            processing_diagnostics: self.processing_diagnostics.clone(),
            late_processing_diagnostics: self.late_processing_diagnostics.clone(),
            ..IncludeProcessor::default()
        }
    }
}

impl IncludeProcessor {
    // Go: includeprocessor.go:37 (*includeProcessor).getDiagnostics
    pub fn get_diagnostics(&self, p: &NewProgram) -> &RefCell<DiagnosticsCollection> {
        self.computed_diagnostics.get_or_init(|| {
            let mut computed_diagnostics = DiagnosticsCollection::default();
            for d in self
                .processing_diagnostics
                .iter()
                .chain(self.late_processing_diagnostics.borrow().iter())
            {
                computed_diagnostics.add(d.to_diagnostic(p));
            }
            // PORT: Go map order is random here; the collection sorts later.
            for resolutions in p.resolved_modules.values() {
                for resolved_module in resolutions.values() {
                    for diag in &resolved_module.resolution_diagnostics {
                        computed_diagnostics.add(diag.clone());
                    }
                }
            }
            for type_resolutions in p.type_resolutions_in_file.values() {
                for resolved_type_ref in type_resolutions.values() {
                    for diag in &resolved_type_ref.resolution_diagnostics {
                        computed_diagnostics.add(diag.clone());
                    }
                }
            }
            RefCell::new(computed_diagnostics)
        })
    }

    /// Marks the point where Go builds `computed_diagnostics`: Go
    /// `GetProgramDiagnostics` and `GetIncludeProcessorDiagnostics` when the
    /// file is checked. See `collection_redirect_and_file_format`.
    pub fn mark_diagnostics_read(&self) {
        self.diagnostics_read.store(true, Ordering::Relaxed);
    }

    // Go: includeprocessor.go:61 (*fileIncludeData).addProcessingDiagnostic
    pub fn add_processing_diagnostic(
        &mut self,
        d: impl IntoIterator<Item = Rc<ProcessingDiagnostic>>,
    ) {
        self.processing_diagnostics.extend(d);
    }

    // Go: includeprocessor.go:65 (*fileIncludeData).addProcessingDiagnosticsForFileCasing
    pub fn add_processing_diagnostics_for_file_casing(
        &mut self,
        file: &Path,
        existing_casing: &str,
        current_casing: &str,
        reason: Rc<FileIncludeReason>,
    ) {
        if !reason.is_referenced_file()
            && self
                .file_include_reasons
                .get(file)
                .is_some_and(|reasons| reasons.iter().any(|r| r.is_referenced_file()))
        {
            self.add_processing_diagnostic([Rc::new(ProcessingDiagnostic {
                kind: ProcessingDiagnosticKind::EXPLAINING_FILE_INCLUDE,
                data: ProcessingDiagnosticData::IncludeExplaining(IncludeExplainingDiagnostic {
                    file: file.clone(),
                    diagnostic_reason: Some(reason),
                    message:
                        diag::Already_included_file_name_0_differs_from_file_name_1_only_in_casing,
                    args: args![existing_casing, current_casing],
                }),
            })]);
        } else {
            self.add_processing_diagnostic([Rc::new(ProcessingDiagnostic {
                kind: ProcessingDiagnosticKind::EXPLAINING_FILE_INCLUDE,
                data: ProcessingDiagnosticData::IncludeExplaining(IncludeExplainingDiagnostic {
                    file: file.clone(),
                    diagnostic_reason: Some(reason),
                    message:
                        diag::File_name_0_differs_from_already_included_file_name_1_only_in_casing,
                    args: args![current_casing, existing_casing],
                }),
            })]);
        }
    }

    // Go: includeprocessor.go:91 (*includeProcessor).getReferenceLocation
    pub fn get_reference_location(
        &self,
        r: &FileIncludeReason,
        program: &NewProgram,
    ) -> Rc<ReferenceFileLocation> {
        let key: *const FileIncludeReason = r;
        if let Some(existing) = self.reason_to_reference_location.borrow().get(&key) {
            return existing.clone();
        }

        // PORT: the borrow is released while the location is computed.
        let loc = Rc::new(r.get_referenced_location(program));
        self.reason_to_reference_location
            .borrow_mut()
            .entry(key)
            .or_insert(loc)
            .clone()
    }

    // Go: includeprocessor.go:100 (*includeProcessor).getCompilerOptionsObjectLiteralSyntax
    pub fn get_compiler_options_object_literal_syntax(&self, program: &NewProgram) -> Node {
        *self.compiler_options_syntax.get_or_init(|| {
            if let Some(config_file) = &program.opts.config.config_file {
                if let Some(compiler_options_property) =
                    for_each_tsconfig_prop_array(config_file.source_file, "compilerOptions", Some)
                    && compiler_options_property.initializer().is_some()
                    && is_object_literal_expression(compiler_options_property.initializer())
                {
                    return compiler_options_property.initializer();
                }
                Node::NIL
            } else {
                Node::NIL
            }
        })
    }

    // Go: includeprocessor.go:116 (*includeProcessor).getRelatedInfo
    pub fn get_related_info(
        &self,
        r: &FileIncludeReason,
        program: &NewProgram,
    ) -> Option<Diagnostic> {
        let key: *const FileIncludeReason = r;
        if let Some(existing) = self.include_reason_to_related_info.borrow().get(&key) {
            return existing.clone();
        }

        // PORT: the borrow is released while the info is computed.
        let related_info = r.to_related_info(program);
        self.include_reason_to_related_info
            .borrow_mut()
            .entry(key)
            .or_insert(related_info)
            .clone()
    }

    // Go: includeprocessor.go:125 (*includeProcessor).explainRedirectAndImpliedFormat
    // PORT: Go nil and empty results are both an empty `Vec`. Callers only
    // append the result. This is the `ExplainFiles` caller; the diagnostics
    // collection calls `explain_redirect_and_implied_format_for_collection`.
    pub fn explain_redirect_and_implied_format(
        &self,
        program: &NewProgram,
        file_path: &Path,
        to_file_name: impl Fn(&str) -> String,
    ) -> Vec<Diagnostic> {
        if let Some(existing) = self.redirect_and_file_format.borrow().get(file_path) {
            return existing.clone();
        }
        // Go built the collection before this call, so its lines are in
        // the cache.
        if self.diagnostics_read.load(Ordering::Relaxed)
            && let Some(lines) = self
                .collection_redirect_and_file_format
                .borrow()
                .get(file_path)
        {
            return self
                .redirect_and_file_format
                .borrow_mut()
                .entry(file_path.clone())
                .or_insert_with(|| lines.clone())
                .clone();
        }
        let Some(result) = self.redirect_and_implied_format(program, file_path, to_file_name)
        else {
            return Vec::new();
        };
        self.redirect_and_file_format
            .borrow_mut()
            .entry(file_path.clone())
            .or_insert(result)
            .clone()
    }

    // Go: processingDiagnostic.go:106 the `explainRedirectAndImpliedFormat`
    // call of the diagnostics collection, with absolute names.
    // PORT: the lines go to `collection_redirect_and_file_format`, not to
    // the Go cache (see that field).
    pub fn explain_redirect_and_implied_format_for_collection(
        &self,
        program: &NewProgram,
        file_path: &Path,
    ) -> Vec<Diagnostic> {
        if let Some(existing) = self.redirect_and_file_format.borrow().get(file_path) {
            return existing.clone();
        }
        if let Some(existing) = self
            .collection_redirect_and_file_format
            .borrow()
            .get(file_path)
        {
            return existing.clone();
        }
        let Some(result) =
            self.redirect_and_implied_format(program, file_path, |file_name| file_name.to_string())
        else {
            return Vec::new();
        };
        self.collection_redirect_and_file_format
            .borrow_mut()
            .entry(file_path.clone())
            .or_insert(result)
            .clone()
    }

    // Go: includeprocessor.go:131-181, the body of
    // explainRedirectAndImpliedFormat between the cache read and the cache
    // write. `None` is Go's early `return nil` (no file at the path), which
    // caches nothing.
    fn redirect_and_implied_format(
        &self,
        program: &NewProgram,
        file_path: &Path,
        to_file_name: impl Fn(&str) -> String,
    ) -> Option<Vec<Diagnostic>> {
        // PORT: Go `file ast.HasFileName` is the redirect file or the source
        // file. Both are kept, and `file_name`/`path` read the one that is set.
        let mut source_file: Option<Rc<ParsedSourceFile>> = None;
        let redirects_file: Option<&RedirectsFile> = program
            .redirect_files_by_path
            .as_ref()
            .and_then(|redirects| redirects.get(file_path));
        let file: &dyn HasFileName = if let Some(redirects_file) = redirects_file {
            redirects_file
        } else {
            source_file = program.get_source_file_by_path(file_path);
            match &source_file {
                None => return None,
                Some(source_file) => &**source_file,
            }
        };
        let mut result: Vec<Diagnostic> = Vec::new();
        let source = program.get_source_of_project_reference_if_output_included(file);
        if source != file.file_name() {
            result.push(new_compiler_diagnostic(
                diag::File_is_output_of_project_reference_source_0,
                args![to_file_name(&source)],
            ));
        }

        if let Some(redirects_file) = redirects_file {
            // PORT: Go dereferences a nil target file and panics; so does this.
            let target_file = program
                .get_source_file_by_path(&redirects_file.target)
                .expect("redirect target file is not in the program");
            result.push(new_compiler_diagnostic(
                diag::File_redirects_to_file_0,
                args![to_file_name(target_file.file_name())],
            ));
        }

        // PORT: Go `ast.IsExternalOrCommonJSModule` reads the indicators on
        // the source file. `prog()` is not set during program construction,
        // so this reads the `ParsedSourceFile` fields.
        if let Some(source_file) = &source_file
            && (source_file.external_module_indicator.is_some()
                || source_file.common_js_module_indicator.is_some())
        {
            let meta_data = program.get_source_file_meta_data(&file.path());
            // ts#64159 (includeprocessor.go:168): the package.json name is
            // `PackageJsonDirectory.ResolveFile("package.json")`, so one in
            // the root is "/package.json" (N: "//package.json").
            match program.get_implied_node_format_for_emit(file) {
                ModuleKind::ES_NEXT => {
                    if meta_data.package_json_type == "module" {
                        result.push(new_compiler_diagnostic(
                            diag::File_is_ECMAScript_module_because_0_has_field_type_with_value_module,
                            args![to_file_name(&combine_paths(&meta_data.package_json_directory, &["package.json"]))],
                        ));
                    }
                }
                ModuleKind::COMMON_JS => {
                    if !meta_data.package_json_type.is_empty() {
                        result.push(new_compiler_diagnostic(
                            diag::File_is_CommonJS_module_because_0_has_field_type_whose_value_is_not_module,
                            args![to_file_name(&combine_paths(&meta_data.package_json_directory, &["package.json"]))],
                        ));
                    } else if !meta_data.package_json_directory.is_empty() {
                        if meta_data.package_json_type.is_empty() {
                            result.push(new_compiler_diagnostic(
                                diag::File_is_CommonJS_module_because_0_does_not_have_field_type,
                                args![to_file_name(&combine_paths(
                                    &meta_data.package_json_directory,
                                    &["package.json"]
                                ))],
                            ));
                        }
                    } else {
                        result.push(new_compiler_diagnostic(
                            diag::File_is_CommonJS_module_because_package_json_was_not_found,
                            args![],
                        ));
                    }
                }
                _ => {}
            }
        }

        Some(result)
    }
}
