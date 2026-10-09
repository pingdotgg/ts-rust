//! Port of compiler/fileInclude.go: why a file is part of the program, and
//! the "explain files" diagnostics for each reason.

use crate::frontend::prelude::*;

/// Go `caseSensitivity.RelativePathFromDirectory(directory, fileName)` and
/// its fallback to the absolute name (ts#64159: fileInclude.go:155,
/// program.go:2145). Used for the include-reason text and `explain_files`.
// PORT: the names are normalized, so not reducing "." and ".." (rule R3)
// changes nothing here. Another root, or an empty `directory` (Go tests pass
// ""), gives the absolute name (rule R4).
pub(crate) fn relative_file_name_from_directory(
    directory: &str,
    file_name: &str,
    program: &NewProgram,
) -> String {
    let directory_root = &directory[..get_root_length(directory)];
    let file_root = &file_name[..get_root_length(file_name)];
    if directory_root.is_empty()
        || !directory_root
            .trim_end_matches('/')
            .eq_ignore_ascii_case(file_root.trim_end_matches('/'))
    {
        return file_name.to_string();
    }
    get_relative_path_from_directory(
        directory,
        file_name,
        &ComparePathsOptions {
            use_case_sensitive_file_names: program.use_case_sensitive_file_names(),
            current_directory: directory.to_string(),
        },
    )
}

// Go: fileInclude.go:14 fileIncludeKind
// PORT: Go `int` enum with iota constants. The newtype keeps the Go order,
// so `is_referenced_file` can compare with `<=`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FileIncludeKind(pub i32);

impl FileIncludeKind {
    // References from file
    pub const IMPORT: FileIncludeKind = FileIncludeKind(0);
    pub const REFERENCE_FILE: FileIncludeKind = FileIncludeKind(1);
    pub const TYPE_REFERENCE_DIRECTIVE: FileIncludeKind = FileIncludeKind(2);
    pub const LIB_REFERENCE_DIRECTIVE: FileIncludeKind = FileIncludeKind(3);

    pub const ROOT_FILE: FileIncludeKind = FileIncludeKind(4);
    pub const LIB_FILE: FileIncludeKind = FileIncludeKind(5);
    pub const AUTOMATIC_TYPE_DIRECTIVE_FILE: FileIncludeKind = FileIncludeKind(6);
    // tsgo#4712
    pub const CONTENT_MAPPER_SUPPLEMENTAL: FileIncludeKind = FileIncludeKind(7);
}

/// Go `FileIncludeReason.data any`.
// PORT: Go stores an `int`, a `*referencedFileData`, a
// `*automaticTypeDirectiveFileData`, a `tspath.Path` (the canonical file of
// a content mapper supplemental file, tsgo#4712) or nil in an `any` field.
// `None` is the Go nil (a default lib file reason has no index).
#[derive(Clone, Debug, Default)]
pub enum FileIncludeData {
    #[default]
    None,
    Index(i32),
    ReferencedFile(ReferencedFileData),
    AutomaticTypeDirectiveFile(AutomaticTypeDirectiveFileData),
    Path(Path),
}

// Go: fileInclude.go:29 FileIncludeReason
// ts#64519: the reason no longer caches its diagnostics; each program does
// (`IncludeProcessor::reason_diagnostics`). PORT: Go ts#64519 replaces
// `data any` with the typed fields `index`, `isDefaultLib`, `referencedFile`,
// `automaticTypeDirective` and `canonicalSourceFile`; the port keeps one
// typed enum (`FileIncludeData`).
#[derive(Debug, Default)]
pub struct FileIncludeReason {
    pub kind: FileIncludeKind,
    pub data: FileIncludeData,
}

// PORT: Go builds reasons with `&FileIncludeReason{kind: .., data: ..}`.
// This is that struct literal; the reason is shared by pointer in Go.
#[must_use]
pub fn new_file_include_reason(
    kind: FileIncludeKind,
    data: FileIncludeData,
) -> Rc<FileIncludeReason> {
    Rc::new(FileIncludeReason { kind, data })
}

// Go: fileInclude.go:38 referencedFileData
// PORT: `synthetic` is `Node::NIL` for the Go nil.
#[derive(Clone, Debug, Default)]
pub struct ReferencedFileData {
    pub file: Path,
    pub index: i32,
    pub synthetic: Node,
}

// Go: fileInclude.go:44 referenceFileLocation
// PORT: `node` is `Node::NIL` for the Go nil. `ref_` is `None` for the Go nil.
#[derive(Clone)]
pub struct ReferenceFileLocation {
    pub file: Rc<ParsedSourceFile>,
    pub node: Node,
    pub ref_: Option<FileReference>,
    pub package_id: PackageId,
    pub is_synthetic: bool,
}

impl ReferenceFileLocation {
    // Go: fileInclude.go:52 (*referenceFileLocation).text
    #[must_use]
    pub fn text(&self) -> String {
        if !self.node.is_nil() {
            if !node_is_synthesized(self.node) {
                let text = self.file.text();
                text[skip_trivia(text, self.node.loc().pos()) as usize..self.node.end() as usize]
                    .to_string()
            } else {
                format!("\"{}\"", self.node.text())
            }
        } else {
            let range = self
                .ref_
                .as_ref()
                .expect("reference location without a node or a reference")
                .range;
            self.file.text()[range.pos() as usize..range.end() as usize].to_string()
        }
    }

    // Go: fileInclude.go:64 (*referenceFileLocation).diagnosticAt
    #[must_use]
    pub fn diagnostic_at(&self, message: &'static Message, args: Vec<String>) -> Diagnostic {
        if !self.node.is_nil() {
            create_diagnostic_for_node_in_source_file(self.file.root, self.node, message, args)
        } else {
            let range = self
                .ref_
                .as_ref()
                .expect("reference location without a node or a reference")
                .range;
            new_diagnostic(self.file.root, range, message, args)
        }
    }
}

// Go: fileInclude.go:72 automaticTypeDirectiveFileData
#[derive(Clone, Debug, Default)]
pub struct AutomaticTypeDirectiveFileData {
    pub type_reference: String,
    pub package_id: PackageId,
}

impl FileIncludeReason {
    // Go: fileInclude.go:77 (*FileIncludeReason).asIndex
    // PORT: a Go failed type assertion panics.
    #[must_use]
    pub fn as_index(&self) -> i32 {
        match &self.data {
            FileIncludeData::Index(index) => *index,
            _ => panic!("interface conversion: FileIncludeReason data is not int"),
        }
    }

    // Go: fileInclude.go:81 (*FileIncludeReason).asLibFileIndex
    #[must_use]
    pub fn as_lib_file_index(&self) -> (i32, bool) {
        match &self.data {
            FileIncludeData::Index(index) => (*index, true),
            _ => (0, false),
        }
    }

    // Go: fileInclude.go:85 (*FileIncludeReason).isReferencedFile
    // PORT: the Go nil receiver check is on the caller (`Option`).
    #[must_use]
    pub fn is_referenced_file(&self) -> bool {
        self.kind <= FileIncludeKind::LIB_REFERENCE_DIRECTIVE
    }

    // Go: fileInclude.go:89 (*FileIncludeReason).asReferencedFileData
    #[must_use]
    pub fn as_referenced_file_data(&self) -> &ReferencedFileData {
        match &self.data {
            FileIncludeData::ReferencedFile(data) => data,
            _ => panic!("interface conversion: FileIncludeReason data is not *referencedFileData"),
        }
    }

    // Go: fileInclude.go:93 (*FileIncludeReason).asAutomaticTypeDirectiveFileData
    #[must_use]
    pub fn as_automatic_type_directive_file_data(&self) -> &AutomaticTypeDirectiveFileData {
        match &self.data {
            FileIncludeData::AutomaticTypeDirectiveFile(data) => data,
            _ => panic!(
                "interface conversion: FileIncludeReason data is not *automaticTypeDirectiveFileData"
            ),
        }
    }

    // Go: fileInclude.go:97 (*FileIncludeReason).getReferencedLocation
    // PORT: Go dereferences a nil file or resolution and panics; `expect`
    // does the same.
    #[must_use]
    pub fn get_referenced_location(&self, program: &NewProgram) -> ReferenceFileLocation {
        let ref_ = self.as_referenced_file_data();
        let file = program
            .get_source_file_by_path(&ref_.file)
            .expect("referenced file is not in the program");
        match self.kind {
            FileIncludeKind::IMPORT => {
                let mut specifier = Node::NIL;
                let mut is_synthetic = false;
                if !ref_.synthetic.is_nil() {
                    specifier = ref_.synthetic;
                    is_synthetic = true;
                } else if (ref_.index as usize) < file.imports.len() {
                    specifier = file.imports[ref_.index as usize];
                } else {
                    let mut aug_index = file.imports.len() as i32;
                    for imp in &file.module_augmentations {
                        if imp.kind() == SyntaxKind::StringLiteral {
                            if aug_index == ref_.index {
                                specifier = *imp;
                                break;
                            }
                            aug_index += 1;
                        }
                    }
                }
                let resolution = program
                    .get_resolved_module_from_module_specifier(&*file, specifier)
                    .expect("import reason without a resolved module");
                ReferenceFileLocation {
                    file,
                    node: specifier,
                    ref_: None,
                    package_id: resolution.package_id.clone(),
                    is_synthetic,
                }
            }
            FileIncludeKind::REFERENCE_FILE => {
                let r = file.referenced_files[ref_.index as usize].clone();
                ReferenceFileLocation {
                    file,
                    node: Node::NIL,
                    ref_: Some(r),
                    package_id: PackageId::default(),
                    is_synthetic: false,
                }
            }
            FileIncludeKind::TYPE_REFERENCE_DIRECTIVE => {
                let r = file.type_reference_directives[ref_.index as usize].clone();
                ReferenceFileLocation {
                    file,
                    node: Node::NIL,
                    ref_: Some(r),
                    package_id: PackageId::default(),
                    is_synthetic: false,
                }
            }
            FileIncludeKind::LIB_REFERENCE_DIRECTIVE => {
                let r = file.lib_reference_directives[ref_.index as usize].clone();
                ReferenceFileLocation {
                    file,
                    node: Node::NIL,
                    ref_: Some(r),
                    package_id: PackageId::default(),
                    is_synthetic: false,
                }
            }
            _ => panic!("unknown reason: {}", self.kind.0),
        }
    }

    // Go: fileInclude.go:148 (*FileIncludeReason).toDiagnostic
    // ts#64519: the diagnostic is cached by the program, not by the reason,
    // so a program that shares the reasons (`ReuseProgram`) computes its
    // own. PORT: Go returns the cached pointer; this returns a copy.
    // ts#64159: a relative file name is relative to `relative_to` (Go
    // `relativeTo`, fileInclude.go:148), not to the program's directory:
    // `explain_files` passes the system current directory, a processing
    // diagnostic passes "" with `relative_file_name` false
    // (processingDiagnostic.go:95). The key holds `relative_to`.
    pub fn to_diagnostic(
        &self,
        program: &NewProgram,
        relative_file_name: bool,
        relative_to: &str,
    ) -> Diagnostic {
        let key = IncludeReasonDiagnosticKey {
            reason: std::ptr::from_ref(self),
            relative_file_name,
            relative_to: relative_to.to_string(),
        };
        let reason_diagnostics = &program.include_processor.reason_diagnostics;
        if let Some(diagnostic) = reason_diagnostics.borrow().get(&key) {
            return diagnostic.clone();
        }
        // PORT: the borrow is released while the diagnostic is computed.
        let diagnostic = self.compute_diagnostic(program, &|file_name: &str| {
            if relative_file_name {
                return relative_file_name_from_directory(relative_to, file_name, program);
            }
            file_name.to_string()
        });
        reason_diagnostics
            .borrow_mut()
            .entry(key)
            .or_insert(diagnostic)
            .clone()
    }

    // Go: fileInclude.go:165 (*FileIncludeReason).computeDiagnostic
    #[must_use]
    pub fn compute_diagnostic(
        &self,
        program: &NewProgram,
        to_file_name: &dyn Fn(&str) -> String,
    ) -> Diagnostic {
        if self.is_referenced_file() {
            return self.compute_reference_file_diagnostic(program, to_file_name);
        }
        match self.kind {
            FileIncludeKind::ROOT_FILE => {
                if program.opts.config.config_file.is_some() {
                    let config = &program.opts.config;
                    // ts#64159 (fileInclude.go:173): the name as listed, which
                    // config parsing made absolute (N normalized it again).
                    let file_name = config.file_names()[self.as_index() as usize].clone();
                    let matched_file_spec = config.get_matched_file_spec(&file_name);
                    if !matched_file_spec.is_empty() {
                        return new_compiler_diagnostic(
                            diag::Part_of_files_list_in_tsconfig_json,
                            args![matched_file_spec, to_file_name(&file_name)],
                        );
                    }
                    let (matched_include_spec, is_default_include_spec) =
                        config.get_matched_include_spec(&file_name);
                    if !matched_include_spec.is_empty() {
                        if is_default_include_spec {
                            new_compiler_diagnostic(
                                diag::Matched_by_default_include_pattern_Asterisk_Asterisk_Slash_Asterisk,
                                args![],
                            )
                        } else {
                            new_compiler_diagnostic(
                                diag::Matched_by_include_pattern_0_in_1,
                                args![matched_include_spec, to_file_name(&config.config_name())],
                            )
                        }
                    } else {
                        new_compiler_diagnostic(diag::Root_file_specified_for_compilation, args![])
                    }
                } else {
                    new_compiler_diagnostic(diag::Root_file_specified_for_compilation, args![])
                }
            }
            FileIncludeKind::AUTOMATIC_TYPE_DIRECTIVE_FILE => {
                let data = self.as_automatic_type_directive_file_data();
                if !program.options().uses_wildcard_types() {
                    if !data.package_id.name.is_empty() {
                        new_compiler_diagnostic(
                            diag::Entry_point_of_type_library_0_specified_in_compilerOptions_with_packageId_1,
                            args![data.type_reference, data.package_id.string()],
                        )
                    } else {
                        new_compiler_diagnostic(
                            diag::Entry_point_of_type_library_0_specified_in_compilerOptions,
                            args![data.type_reference],
                        )
                    }
                } else if !data.package_id.name.is_empty() {
                    new_compiler_diagnostic(
                        diag::Entry_point_for_implicit_type_library_0_with_packageId_1,
                        args![data.type_reference, data.package_id.string()],
                    )
                } else {
                    new_compiler_diagnostic(
                        diag::Entry_point_for_implicit_type_library_0,
                        args![data.type_reference],
                    )
                }
            }
            FileIncludeKind::LIB_FILE => {
                let (index, ok) = self.as_lib_file_index();
                if ok {
                    return new_compiler_diagnostic(
                        diag::Library_0_specified_in_compilerOptions,
                        args![
                            program
                                .options()
                                .lib
                                .as_ref()
                                .expect("lib is set for an indexed lib file")
                                [index as usize]
                        ],
                    );
                }
                let target = program.options().get_emit_script_target().string();
                if !target.is_empty() {
                    new_compiler_diagnostic(diag::Default_library_for_target_0, args![target])
                } else {
                    new_compiler_diagnostic(diag::Default_library, args![])
                }
            }
            // tsgo#4712
            FileIncludeKind::CONTENT_MAPPER_SUPPLEMENTAL => {
                let FileIncludeData::Path(canonical_path) = &self.data else {
                    panic!("interface conversion: FileIncludeReason data is not tspath.Path");
                };
                let canonical = program
                    .get_source_file_by_path(canonical_path)
                    .expect("nil pointer dereference: canonical source file");
                new_compiler_diagnostic(
                    diag::Supplemental_virtual_file_produced_by_the_content_mapper_for_file_0,
                    args![to_file_name(canonical.file_name())],
                )
            }
            _ => panic!("unknown reason: {}", self.kind.0),
        }
    }

    // Go: fileInclude.go:219 (*FileIncludeReason).computeReferenceFileDiagnostic
    #[must_use]
    pub fn compute_reference_file_diagnostic(
        &self,
        program: &NewProgram,
        to_file_name: &dyn Fn(&str) -> String,
    ) -> Diagnostic {
        let reference_location = program
            .processed_files
            .include_processor
            .get_reference_location(self, program);
        let reference_text = reference_location.text();
        let file_name = to_file_name(reference_location.file.file_name());
        let has_package_id = !reference_location.package_id.name.is_empty();
        match self.kind {
            FileIncludeKind::IMPORT => {
                if !reference_location.is_synthetic {
                    if has_package_id {
                        new_compiler_diagnostic(
                            diag::Imported_via_0_from_file_1_with_packageId_2,
                            args![
                                reference_text,
                                file_name,
                                reference_location.package_id.string()
                            ],
                        )
                    } else {
                        new_compiler_diagnostic(
                            diag::Imported_via_0_from_file_1,
                            args![reference_text, file_name],
                        )
                    }
                } else if program
                    .processed_files
                    .import_helpers_import_specifiers
                    .as_ref()
                    .and_then(|m| m.get(reference_location.file.path()))
                    .is_some_and(|specifier| *specifier == reference_location.node)
                {
                    if has_package_id {
                        new_compiler_diagnostic(
                            diag::Imported_via_0_from_file_1_with_packageId_2_to_import_importHelpers_as_specified_in_compilerOptions,
                            args![reference_text, file_name, reference_location.package_id.string()],
                        )
                    } else {
                        new_compiler_diagnostic(
                            diag::Imported_via_0_from_file_1_to_import_importHelpers_as_specified_in_compilerOptions,
                            args![reference_text, file_name],
                        )
                    }
                } else if has_package_id {
                    new_compiler_diagnostic(
                        diag::Imported_via_0_from_file_1_with_packageId_2_to_import_jsx_and_jsxs_factory_functions,
                        args![reference_text, file_name, reference_location.package_id.string()],
                    )
                } else {
                    new_compiler_diagnostic(
                        diag::Imported_via_0_from_file_1_to_import_jsx_and_jsxs_factory_functions,
                        args![reference_text, file_name],
                    )
                }
            }
            FileIncludeKind::REFERENCE_FILE => new_compiler_diagnostic(
                diag::Referenced_via_0_from_file_1,
                args![reference_text, file_name],
            ),
            FileIncludeKind::TYPE_REFERENCE_DIRECTIVE => {
                if has_package_id {
                    new_compiler_diagnostic(
                        diag::Type_library_referenced_via_0_from_file_1_with_packageId_2,
                        args![
                            reference_text,
                            file_name,
                            reference_location.package_id.string()
                        ],
                    )
                } else {
                    new_compiler_diagnostic(
                        diag::Type_library_referenced_via_0_from_file_1,
                        args![reference_text, file_name],
                    )
                }
            }
            FileIncludeKind::LIB_REFERENCE_DIRECTIVE => new_compiler_diagnostic(
                diag::Library_referenced_via_0_from_file_1,
                args![reference_text, file_name],
            ),
            _ => panic!("unknown reason: {}", self.kind.0),
        }
    }

    // Go: fileInclude.go:258 (*FileIncludeReason).toRelatedInfo
    // PORT: Go nil is `None`. The tsoptions syntax helpers return
    // `Node::NIL` for the Go nil.
    #[must_use]
    pub fn to_related_info(&self, program: &NewProgram) -> Option<Diagnostic> {
        if self.is_referenced_file() {
            return self.compute_reference_file_related_info(program);
        }
        let config = &program.opts.config;
        let config_file = config.config_file.as_ref()?;
        let config_source_file = config_file.source_file;
        match self.kind {
            FileIncludeKind::ROOT_FILE => {
                // ts#64159 (fileInclude.go:268): the name as listed.
                let file_name = config.file_names()[self.as_index() as usize].clone();
                let matched_file_spec = config.get_matched_file_spec(&file_name);
                if !matched_file_spec.is_empty() {
                    let files_node = get_tsconfig_prop_array_element_value(
                        config_source_file,
                        "files",
                        &matched_file_spec,
                    );
                    if !files_node.is_nil() {
                        return Some(create_diagnostic_for_node_in_source_file(
                            config_source_file,
                            files_node,
                            diag::File_is_matched_by_files_list_specified_here,
                            args![],
                        ));
                    }
                } else {
                    let (matched_include_spec, is_default_include_spec) =
                        config.get_matched_include_spec(&file_name);
                    if !matched_include_spec.is_empty() && !is_default_include_spec {
                        let include_node = get_tsconfig_prop_array_element_value(
                            config_source_file,
                            "include",
                            &matched_include_spec,
                        );
                        if !include_node.is_nil() {
                            return Some(create_diagnostic_for_node_in_source_file(
                                config_source_file,
                                include_node,
                                diag::File_is_matched_by_include_pattern_specified_here,
                                args![],
                            ));
                        }
                    }
                }
            }
            FileIncludeKind::AUTOMATIC_TYPE_DIRECTIVE_FILE => {
                if !program.options().uses_wildcard_types() {
                    let data = self.as_automatic_type_directive_file_data();
                    let types_syntax = get_options_syntax_by_array_element_value(
                        program
                            .processed_files
                            .include_processor
                            .get_compiler_options_object_literal_syntax(program),
                        "types",
                        &data.type_reference,
                    );
                    if !types_syntax.is_nil() {
                        return Some(create_diagnostic_for_node_in_source_file(
                            config_source_file,
                            types_syntax,
                            diag::File_is_entry_point_of_type_library_specified_here,
                            args![],
                        ));
                    }
                }
            }
            FileIncludeKind::LIB_FILE => {
                let (index, ok) = self.as_lib_file_index();
                if ok {
                    let lib_syntax = get_options_syntax_by_array_element_value(
                        program
                            .processed_files
                            .include_processor
                            .get_compiler_options_object_literal_syntax(program),
                        "lib",
                        &program
                            .options()
                            .lib
                            .as_ref()
                            .expect("lib is set for an indexed lib file")
                            [index as usize],
                    );
                    if !lib_syntax.is_nil() {
                        return Some(create_diagnostic_for_node_in_source_file(
                            config_source_file,
                            lib_syntax,
                            diag::File_is_library_specified_here,
                            args![],
                        ));
                    }
                } else {
                    let target = program.options().get_emit_script_target().string();
                    if !target.is_empty() {
                        let target_value_syntax = for_each_property_assignment(
                            program
                                .processed_files
                                .include_processor
                                .get_compiler_options_object_literal_syntax(program),
                            "target",
                            get_callback_for_finding_property_assignment_by_value(&target),
                            &[],
                        );
                        if let Some(target_value_syntax) = target_value_syntax {
                            return Some(create_diagnostic_for_node_in_source_file(
                                config_source_file,
                                target_value_syntax,
                                diag::File_is_default_library_for_target_specified_here,
                                args![],
                            ));
                        }
                    }
                }
            }
            // tsgo#4712
            FileIncludeKind::CONTENT_MAPPER_SUPPLEMENTAL => return None,
            _ => panic!("unknown reason: {}", self.kind.0),
        }
        None
    }

    // Go: fileInclude.go:303 (*FileIncludeReason).computeReferenceFileRelatedInfo
    #[must_use]
    pub fn compute_reference_file_related_info(&self, program: &NewProgram) -> Option<Diagnostic> {
        let reference_location = program
            .processed_files
            .include_processor
            .get_reference_location(self, program);
        if reference_location.is_synthetic {
            return None;
        }
        Some(match self.kind {
            FileIncludeKind::IMPORT => {
                reference_location.diagnostic_at(diag::File_is_included_via_import_here, args![])
            }
            FileIncludeKind::REFERENCE_FILE => {
                reference_location.diagnostic_at(diag::File_is_included_via_reference_here, args![])
            }
            FileIncludeKind::TYPE_REFERENCE_DIRECTIVE => reference_location.diagnostic_at(
                diag::File_is_included_via_type_library_reference_here,
                args![],
            ),
            FileIncludeKind::LIB_REFERENCE_DIRECTIVE => reference_location
                .diagnostic_at(diag::File_is_included_via_library_reference_here, args![]),
            _ => panic!("unknown reason: {}", self.kind.0),
        })
    }
}
