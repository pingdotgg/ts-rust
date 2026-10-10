//! Go `internal/compiler/processingDiagnostic.go`: diagnostics found while
//! files are loaded. They become real diagnostics after the program is built.

use crate::frontend::prelude::*;

/// Go `processingDiagnosticKind`.
// PORT: Go `int` enum with iota constants, kept as a newtype like
// `FileIncludeKind`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ProcessingDiagnosticKind(pub i32);

impl ProcessingDiagnosticKind {
    pub const UNKNOWN_REFERENCE: ProcessingDiagnosticKind = ProcessingDiagnosticKind(0);
    pub const EXPLAINING_FILE_INCLUDE: ProcessingDiagnosticKind = ProcessingDiagnosticKind(1);
}

/// Go `processingDiagnostic.data any`.
// PORT: Go stores a `*FileIncludeReason` or an `*includeExplainingDiagnostic`
// in an `any` field. `asFileIncludeReason` and
// `asIncludeExplainingDiagnostic` are the `match` arms.
#[derive(Clone)]
pub enum ProcessingDiagnosticData {
    FileIncludeReason(Rc<FileIncludeReason>),
    IncludeExplaining(IncludeExplainingDiagnostic),
}

/// Go `processingDiagnostic`.
// PORT: Go shares `*processingDiagnostic`; callers hold `Rc<ProcessingDiagnostic>`.
#[derive(Clone)]
pub struct ProcessingDiagnostic {
    pub kind: ProcessingDiagnosticKind,
    pub data: ProcessingDiagnosticData,
}

/// Go `includeExplainingDiagnostic`.
// PORT: Go `file == ""` is an empty `Path`. Go nil `diagnosticReason` is
// `None`. Go `args []any` are formatted strings.
#[derive(Clone)]
pub struct IncludeExplainingDiagnostic {
    pub file: Path,
    pub diagnostic_reason: Option<Rc<FileIncludeReason>>,
    pub message: &'static Message,
    pub args: Vec<String>,
}

impl ProcessingDiagnostic {
    // Go: processingDiagnostic.go:43 (*processingDiagnostic).toDiagnostic
    pub fn to_diagnostic(&self, program: &NewProgram) -> Diagnostic {
        match self.kind {
            ProcessingDiagnosticKind::UNKNOWN_REFERENCE => {
                let r = self.as_file_include_reason();
                let loc = r.get_referenced_location(program);
                // PORT: Go reads `loc.ref.FileName`; `ref` is not nil for
                // these kinds. `None` panics like a Go nil dereference.
                let reference = loc
                    .ref_
                    .as_ref()
                    .expect("reference location has no file reference");
                match r.kind {
                    FileIncludeKind::TYPE_REFERENCE_DIRECTIVE => loc.diagnostic_at(
                        diag::Cannot_find_type_definition_file_for_0,
                        args![reference.file_name],
                    ),
                    FileIncludeKind::LIB_REFERENCE_DIRECTIVE => {
                        let lib_name = to_file_name_lower_case(&reference.file_name);
                        let without_prefix = lib_name.strip_prefix("lib.").unwrap_or(&lib_name);
                        let unqualified_lib_name = without_prefix
                            .strip_suffix(".d.ts")
                            .unwrap_or(without_prefix);
                        let suggestion = get_spelling_suggestion_for_strings(
                            unqualified_lib_name,
                            LIBS.iter().cloned(),
                        );
                        let message = if !suggestion.is_empty() {
                            diag::Cannot_find_lib_definition_for_0_Did_you_mean_1
                        } else {
                            diag::Cannot_find_lib_definition_for_0
                        };
                        loc.diagnostic_at(message, args![lib_name, suggestion])
                    }
                    _ => panic!("unknown include kind"),
                }
            }
            ProcessingDiagnosticKind::EXPLAINING_FILE_INCLUDE => {
                self.create_diagnostic_explaining_file(program)
            }
            _ => panic!("unknown processingDiagnosticKind"),
        }
    }

    // Go: processingDiagnostic.go:28 (*processingDiagnostic).asFileIncludeReason
    // PORT: Go panics on the failed type assertion; so does this.
    pub fn as_file_include_reason(&self) -> &Rc<FileIncludeReason> {
        match &self.data {
            ProcessingDiagnosticData::FileIncludeReason(r) => r,
            ProcessingDiagnosticData::IncludeExplaining(_) => {
                panic!("processing diagnostic data is not a FileIncludeReason")
            }
        }
    }

    // Go: processingDiagnostic.go:39 (*processingDiagnostic).asIncludeExplainingDiagnostic
    // PORT: Go panics on the failed type assertion; so does this.
    pub fn as_include_explaining_diagnostic(&self) -> &IncludeExplainingDiagnostic {
        match &self.data {
            ProcessingDiagnosticData::IncludeExplaining(d) => d,
            ProcessingDiagnosticData::FileIncludeReason(_) => {
                panic!("processing diagnostic data is not an includeExplainingDiagnostic")
            }
        }
    }

    // Go: processingDiagnostic.go:70 (*processingDiagnostic).createDiagnosticExplainingFile
    pub fn create_diagnostic_explaining_file(&self, program: &NewProgram) -> Diagnostic {
        let diag = self.as_include_explaining_diagnostic();
        // PORT: Go nil slices are `None`, so the `!= nil` checks stay exact.
        let mut include_details: Option<Vec<Diagnostic>> = None;
        let mut related_info: Option<Vec<Diagnostic>> = None;
        let mut redirect_info: Option<Vec<Diagnostic>> = None;
        let mut preferred_location: Option<Rc<FileIncludeReason>> = None;
        // PORT: Go `collections.Set[*FileIncludeReason]` keys by pointer.
        let mut seen_reasons: FxHashSet<*const FileIncludeReason> = FxHashSet::default();
        if let Some(reason) = &diag.diagnostic_reason
            && reason.is_referenced_file()
            && !program
                .include_processor
                .get_reference_location(reason, program)
                .is_synthetic
        {
            preferred_location = Some(reason.clone());
        }

        // PORT: the Go closures `processRelatedInfo` and `processInclude`
        // capture locals by reference. Here they take the locals as arguments.
        let process_related_info =
            |include_reason: &Rc<FileIncludeReason>,
             preferred_location: &mut Option<Rc<FileIncludeReason>>,
             related_info: &mut Option<Vec<Diagnostic>>| {
                if preferred_location.is_none()
                    && include_reason.is_referenced_file()
                    && !program
                        .include_processor
                        .get_reference_location(include_reason, program)
                        .is_synthetic
                {
                    *preferred_location = Some(include_reason.clone());
                } else if !preferred_location
                    .as_ref()
                    .is_some_and(|p| Rc::ptr_eq(p, include_reason))
                {
                    let info = program
                        .include_processor
                        .get_related_info(include_reason, program);
                    if let Some(info) = info {
                        related_info.get_or_insert_with(Vec::new).push(info);
                    }
                }
            };
        let process_include =
            |include_reason: &Rc<FileIncludeReason>,
             seen_reasons: &mut FxHashSet<*const FileIncludeReason>,
             include_details: &mut Option<Vec<Diagnostic>>,
             preferred_location: &mut Option<Rc<FileIncludeReason>>,
             related_info: &mut Option<Vec<Diagnostic>>| {
                if !seen_reasons.insert(Rc::as_ptr(include_reason)) {
                    return;
                }
                include_details
                    .get_or_insert_with(Vec::new)
                    .push(include_reason.to_diagnostic(program, false, ""));
                process_related_info(include_reason, preferred_location, related_info);
            };

        // !!! todo sheetal caching

        if !diag.file.is_empty() {
            let reasons: Vec<Rc<FileIncludeReason>> = program
                .include_processor
                .file_include_reasons
                .get(&diag.file)
                .cloned()
                .unwrap_or_default();
            include_details = Some(Vec::with_capacity(reasons.len()));
            for reason in &reasons {
                process_include(
                    reason,
                    &mut seen_reasons,
                    &mut include_details,
                    &mut preferred_location,
                    &mut related_info,
                );
            }
            redirect_info = Some(
                program
                    .include_processor
                    .explain_redirect_and_implied_format_for_collection(program, &diag.file),
            );
        }
        if let Some(reason) = &diag.diagnostic_reason {
            process_include(
                reason,
                &mut seen_reasons,
                &mut include_details,
                &mut preferred_location,
                &mut related_info,
            );
        }
        let mut chain: Option<Vec<Diagnostic>> = None;
        if let Some(include_details) = include_details
            && (preferred_location.is_none() || seen_reasons.len() != 1)
        {
            let mut file_reason =
                new_compiler_diagnostic(diag::The_file_is_in_the_program_because_Colon, args![]);
            file_reason.set_message_chain(include_details);
            chain = Some(vec![file_reason]);
        }
        // PORT: Go `append(nil, empty...)` stays nil, so an empty
        // `redirectInfo` does not create the chain.
        if let Some(redirect_info) = redirect_info
            && !redirect_info.is_empty()
        {
            chain.get_or_insert_with(Vec::new).extend(redirect_info);
        }

        let mut result: Option<Diagnostic> = None;
        if let Some(preferred_location) = &preferred_location {
            result = Some(
                program
                    .include_processor
                    .get_reference_location(preferred_location, program)
                    .diagnostic_at(diag.message, diag.args.clone()),
            );
        }
        let mut result =
            result.unwrap_or_else(|| new_compiler_diagnostic(diag.message, diag.args.clone()));
        if let Some(chain) = chain {
            result.set_message_chain(chain);
        }
        if let Some(related_info) = related_info {
            result.set_related_info(related_info);
        }
        result
    }
}
