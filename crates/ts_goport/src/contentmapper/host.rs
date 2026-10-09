//! Port of Go `internal/contentmapper/host.go` (tsgo#4712).
//!
//! PORT: Go returns the pointer error types (`*TransformError`, ...) as
//! `error`. Each Rust error type is a value with `to_go_error()`, which wraps
//! it with `errors::from_value` (and its `Unwrap()` result, when it has
//! one), so `errors::as_type::<T>` finds it. `errors::as_type` returns a
//! copy: code that changes a found Go error in place builds a new one.

use crate::contentmapper::prelude::*;

use crate::flags_macros::go_enum;
use crate::locale::Locale;
use std::sync::Arc;
use std::time::Duration;

// Go: contentmapper/host.go:15 TransformErrorKind
// TransformErrorKind identifies the stage at which a content mapper transform failed.
go_enum!(TransformErrorKind, u8 {
    UNKNOWN = 0;
    INITIALIZE = 1;
    PROJECT = 2;
    REQUEST = 3;
    RESPONSE = 4;
    MAPPINGS = 5;
});

// Go: contentmapper/host.go:27 TransformError
// TransformError reports a failure while preparing, requesting, or decoding a transform.
#[derive(Clone, Debug, PartialEq)]
pub struct TransformError {
    pub kind: TransformErrorKind,
    err: Option<GoError>,
}

// Go: contentmapper/host.go:33 NewTransformError
// NewTransformError creates a transform error for the given stage and underlying error.
#[must_use]
pub fn new_transform_error(kind: TransformErrorKind, err: Option<GoError>) -> TransformError {
    TransformError { kind, err }
}

impl TransformError {
    // Go: contentmapper/host.go:37 TransformError.Error
    #[must_use]
    pub fn error(&self) -> String {
        // Go `%v` of a nil error prints `<nil>`.
        match &self.err {
            Some(err) => format!("content mapper transform failed: {}", err.error()),
            None => "content mapper transform failed: <nil>".to_string(),
        }
    }

    // Go: contentmapper/host.go:41 TransformError.Unwrap
    #[must_use]
    pub fn unwrap(&self) -> Option<GoError> {
        self.err.clone()
    }

    /// Go `*TransformError` as an `error`.
    #[must_use]
    pub fn to_go_error(&self) -> GoError {
        match self.unwrap() {
            Some(inner) => errors::from_value_with_unwrap(self.clone(), inner),
            None => errors::from_value(self.clone()),
        }
    }
}

impl std::fmt::Display for TransformError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.error())
    }
}

// Go: contentmapper/host.go:44 DiagnosticDirectiveErrorKind
// DiagnosticDirectiveErrorKind identifies why a diagnostic directive was rejected.
go_enum!(DiagnosticDirectiveErrorKind, u8 {
    INVALID_RANGE = 0;
    INVALID_POLICY = 1;
    EXPECT_MISSING_UNUSED_DIAGNOSTIC = 2;
    INVALID_UNUSED_DIAGNOSTIC_INDEX = 3;
    OVERLAP = 4;
});

// Go: contentmapper/host.go:55 DiagnosticDirectiveError
// DiagnosticDirectiveError reports an invalid diagnostic directive in a transform response.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DiagnosticDirectiveError {
    pub kind: DiagnosticDirectiveErrorKind,
    pub index: i32,
    pub supplemental_index: i32,
    pub policy: DiagnosticDirectivePolicy,
}

impl DiagnosticDirectiveError {
    // Go: contentmapper/host.go:62 DiagnosticDirectiveError.Error
    #[must_use]
    pub fn error(&self) -> String {
        format!("invalid content mapper diagnostic directive {}", self.index)
    }

    /// Go `*DiagnosticDirectiveError` as an `error`.
    #[must_use]
    pub fn to_go_error(&self) -> GoError {
        errors::from_value(self.clone())
    }
}

impl std::fmt::Display for DiagnosticDirectiveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.error())
    }
}

// Go: contentmapper/host.go:67 InvalidVirtualExtensionError
// InvalidVirtualExtensionError reports an unsupported or missing virtual extension on a mapped output.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InvalidVirtualExtensionError {
    pub extension: String,
}

impl InvalidVirtualExtensionError {
    // Go: contentmapper/host.go:71 InvalidVirtualExtensionError.Error
    #[must_use]
    pub fn error(&self) -> String {
        format!(
            "invalid virtual extension {}",
            gostd::strconv::quote(&self.extension)
        )
    }

    /// Go `*InvalidVirtualExtensionError` as an `error`.
    #[must_use]
    pub fn to_go_error(&self) -> GoError {
        errors::from_value(self.clone())
    }
}

impl std::fmt::Display for InvalidVirtualExtensionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.error())
    }
}

// Go: contentmapper/host.go:76 ProjectErrorKind
// ProjectErrorKind identifies why a mapper's openProject response was rejected.
go_enum!(ProjectErrorKind, u8 {
    MALFORMED_RESPONSE = 0;
    MISSING_CONFIG_IDENTITY = 1;
    NON_ABSOLUTE_WATCHED_FILE = 2;
    UNEXPECTED_CONFIG_IDENTITY = 3;
    UNEXPECTED_WATCHED_FILES = 4;
});

// Go: contentmapper/host.go:87 ProjectError
// ProjectError reports an invalid mapper openProject response.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProjectError {
    pub kind: ProjectErrorKind,
}

impl ProjectError {
    // Go: contentmapper/host.go:91 ProjectError.Error
    #[must_use]
    pub fn error(&self) -> String {
        match self.kind {
            ProjectErrorKind::MALFORMED_RESPONSE => {
                "content mapper returned a malformed project response"
            }
            ProjectErrorKind::MISSING_CONFIG_IDENTITY => {
                "content mapper did not return configIdentity for dynamic configuration"
            }
            ProjectErrorKind::NON_ABSOLUTE_WATCHED_FILE => {
                "content mapper returned a non-absolute path in watchedFiles"
            }
            ProjectErrorKind::UNEXPECTED_CONFIG_IDENTITY => {
                "content mapper returned configIdentity without declaring dynamicConfig"
            }
            ProjectErrorKind::UNEXPECTED_WATCHED_FILES => {
                "content mapper returned watchedFiles without declaring dynamicConfig"
            }
            _ => "content mapper returned an invalid project response",
        }
        .to_string()
    }

    /// Go `*ProjectError` as an `error`.
    #[must_use]
    pub fn to_go_error(&self) -> GoError {
        errors::from_value(self.clone())
    }
}

impl std::fmt::Display for ProjectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.error())
    }
}

// Go: contentmapper/host.go:109 InitializeErrorKind
// InitializeErrorKind identifies why a mapper's initialize response was rejected.
go_enum!(InitializeErrorKind, u8 {
    PROCESS_START = 0;
    PROCESS_EXIT = 1;
    NO_RESPONSE = 2;
    INVALID_RESPONSE = 3;
    REQUEST = 4;
    POSITION_ENCODING = 5;
    EMPTY_DIAGNOSTIC_SOURCE = 6;
    RESERVED_DIAGNOSTIC_SOURCE = 7;
});

// Go: contentmapper/host.go:123 InitializeError
// InitializeError reports an invalid or unsupported mapper initialize response.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InitializeError {
    pub kind: InitializeErrorKind,
    pub mapper_name: String,
    pub command: String,
    pub detail: String,
    pub exit_code: i32,
    pub timeout_seconds: i32,
    pub position_encoding: PositionEncoding,
    pub diagnostic_source: String,
}

// Go: contentmapper/host.go:135 SupplementalFileCollisionError
// SupplementalFileCollisionError reports a compiler-assigned supplemental filename that already exists.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SupplementalFileCollisionError {
    pub file_name: String,
}

impl SupplementalFileCollisionError {
    // Go: contentmapper/host.go:139 SupplementalFileCollisionError.Error
    #[must_use]
    pub fn error(&self) -> String {
        format!(
            "content mapper supplemental output file {} already exists",
            gostd::strconv::quote(&self.file_name)
        )
    }

    /// Go `*SupplementalFileCollisionError` as an `error`.
    #[must_use]
    pub fn to_go_error(&self) -> GoError {
        errors::from_value(self.clone())
    }
}

impl std::fmt::Display for SupplementalFileCollisionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.error())
    }
}

impl InitializeError {
    // Go: contentmapper/host.go:143 InitializeError.Error
    #[must_use]
    pub fn error(&self) -> String {
        match self.kind {
            InitializeErrorKind::PROCESS_START => format!(
                "could not start content mapper command {}: {}",
                gostd::strconv::quote(&self.command),
                self.detail
            ),
            InitializeErrorKind::PROCESS_EXIT => format!(
                "content mapper process exited before initialization with code {}",
                self.exit_code
            ),
            InitializeErrorKind::NO_RESPONSE => {
                "content mapper did not respond to the initialize request".to_string()
            }
            InitializeErrorKind::INVALID_RESPONSE => format!(
                "content mapper returned an invalid initialize response: {}",
                self.detail
            ),
            InitializeErrorKind::REQUEST => {
                format!("content mapper initialize request failed: {}", self.detail)
            }
            InitializeErrorKind::POSITION_ENCODING => format!(
                "unsupported position encoding {}",
                gostd::strconv::quote(&self.position_encoding.0)
            ),
            InitializeErrorKind::EMPTY_DIAGNOSTIC_SOURCE => {
                "diagnostic source must not be empty".to_string()
            }
            InitializeErrorKind::RESERVED_DIAGNOSTIC_SOURCE => format!(
                "diagnostic source {} is reserved by TypeScript",
                gostd::strconv::quote(&self.diagnostic_source)
            ),
            _ => "content mapper initialization failed".to_string(),
        }
    }

    /// Go `*InitializeError` as an `error`.
    #[must_use]
    pub fn to_go_error(&self) -> GoError {
        errors::from_value(self.clone())
    }
}

impl std::fmt::Display for InitializeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.error())
    }
}

// Go: contentmapper/host.go:167 Result
// Result is the outcome of transforming a content-mapped source file into virtual TypeScript.
// PORT: the name shadows `std::result::Result` in this package; package
// files write `std::result::Result` in full. Go `*spanmap.SpanMap` is
// `Option<Arc<SpanMap>>` (the source files and checker threads share it).
// Go `*ast.Diagnostic` is `crate::core::Diagnostic`.
#[derive(Clone, Default)]
pub struct Result {
    // Text is the virtual TypeScript source text that is parsed into the program.
    pub text: String,
    // VirtualExtension determines how Text is parsed.
    pub virtual_extension: String,
    // Diagnostics are syntax errors in the original content.
    pub diagnostics: Vec<crate::core::Diagnostic>,
    // Mappings maps positions in Text back to the original content, so that diagnostics the compiler
    // produces against the virtual text can be reported at their original locations. A successful
    // transform must return a non-nil map; an empty map describes fully synthesized output.
    pub mappings: Option<Arc<spanmap::SpanMap>>,
    // DiagnosticDirectives control TypeScript diagnostics produced in virtual ranges.
    pub diagnostic_directives: Vec<ast::MappedDiagnosticDirective>,
    // Supplemental contains additional unnamed outputs associated with the canonical result.
    pub supplemental: Vec<MappedResult>,
}

// Go: contentmapper/host.go:185 MappedResult
// MappedResult is one virtual source file and its mapping to the original input.
#[derive(Clone, Default)]
pub struct MappedResult {
    pub text: String,
    pub virtual_extension: String,
    pub mappings: Option<Arc<spanmap::SpanMap>>,
    pub diagnostic_directives: Vec<ast::MappedDiagnosticDirective>,
}

// Go: contentmapper/host.go:193 Request
// Request carries the inputs for transforming one content-mapped source file.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Request {
    // FileName is the content-mapped source file being transformed.
    pub file_name: String,
    // Content is the content-mapped source file's text.
    pub content: String,
}

// Go: contentmapper/host.go:201 ProjectSpec
// ProjectSpec describes the project configuration visible to its content mappers.
// PORT: Go `*core.CompilerOptions` is `Option<Rc<CompilerOptions>>` (nil is
// `None`); the host keys projects by its pointer, as Go does.
#[derive(Clone, Default)]
pub struct ProjectSpec {
    // ConfigFileName is the absolute project configuration file name, or empty for a project without one.
    pub config_file_name: String,
    // Mappers are the resolved content mapper entries configured for the project.
    pub mappers: Vec<Rc<Mapper>>,
    // CompilerOptions are the project's effective compiler options.
    pub compiler_options: Option<Rc<CompilerOptions>>,
}

// Go: contentmapper/host.go:210 OptionPathSegment
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OptionPathSegment {
    pub property: String,
    pub index: i32,
    pub is_index: bool,
}

// Go: contentmapper/host.go:216 OptionDiagnostic
#[derive(Clone, Debug)]
pub struct OptionDiagnostic {
    pub mapper: Rc<Mapper>,
    pub path: Vec<OptionPathSegment>,
    pub source: String,
    pub code: i32,
    pub message_text: String,
}

// Go: contentmapper/host.go:225 OperationTiming
// OperationTiming is the cumulative wall time and invocation count for one mapper operation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OperationTiming {
    pub count: u64,
    pub duration: Duration,
}

// Go: contentmapper/host.go:231 MapperTimings
// MapperTimings is cumulative process and protocol activity for one resolved mapper identity.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MapperTimings {
    pub spawn: OperationTiming,
    pub initialize: OperationTiming,
    pub open_project: OperationTiming,
    pub close_project: OperationTiming,
    pub transform: OperationTiming,
}

// Go: contentmapper/host.go:240 Timings
// Timings is a cumulative snapshot of content mapper process and protocol activity.
// PORT: Go `map[string]MapperTimings` is an `IndexMap` (Go map order is
// random; readers sort or look up by identity).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Timings {
    pub mappers: IndexMap<String, MapperTimings>,
    pub request_wait: Duration,
}

impl Timings {
    // Go: contentmapper/host.go:246 Timings.Since
    // Since returns the non-negative operation delta since previous.
    #[must_use]
    pub fn since(&self, previous: &Timings) -> Timings {
        let mut result = Timings {
            mappers: IndexMap::with_capacity(self.mappers.len()),
            // Go `max(t.RequestWait-previous.RequestWait, 0)`.
            request_wait: self.request_wait.saturating_sub(previous.request_wait),
        };
        for (identity, current) in &self.mappers {
            let before = previous.mappers.get(identity).copied().unwrap_or_default();
            result.mappers.insert(
                identity.clone(),
                MapperTimings {
                    spawn: operation_timing_since(current.spawn, before.spawn),
                    initialize: operation_timing_since(current.initialize, before.initialize),
                    open_project: operation_timing_since(current.open_project, before.open_project),
                    close_project: operation_timing_since(
                        current.close_project,
                        before.close_project,
                    ),
                    transform: operation_timing_since(current.transform, before.transform),
                },
            );
        }
        result
    }
}

// Go: contentmapper/host.go:264 operationTimingSince
fn operation_timing_since(current: OperationTiming, previous: OperationTiming) -> OperationTiming {
    OperationTiming {
        count: current.count - current.count.min(previous.count),
        duration: current.duration.saturating_sub(previous.duration),
    }
}

// Go: contentmapper/host.go:274 Project
// Project is the project-scoped view of a Host. It owns mapper configuration handles and provides the
// identities and watch dependencies needed for caching and incremental builds. Mapper projects are opened
// lazily when a transform is requested, or earlier when dynamic configuration is needed.
pub trait Project {
    // Refresh closes opened mapper projects so they are reopened on the next transform or configuration identity query.
    fn refresh(&self) -> std::result::Result<(), GoError>;
    // Identities returns sorted transform identities for all configured mappers. It returns an error if
    // dynamic project configuration cannot be opened or validated.
    fn identities(&self) -> std::result::Result<Vec<String>, GoError>;
    // Identity returns the transform identity for mapper, or an empty string if mapper is not in this
    // project. It returns an error if dynamic project configuration cannot be opened or validated.
    fn identity(&self, mapper: &Rc<Mapper>) -> std::result::Result<String, GoError>;
    // WatchedFiles returns the absolute files reported by mappers whose package.json declares dynamicConfig.
    // It returns an error if project configuration cannot be opened or validated.
    fn watched_files(&self) -> std::result::Result<Vec<String>, GoError>;
    // Diagnostics returns option diagnostics cached by mapper projects that have already been opened.
    fn diagnostics(&self) -> Vec<OptionDiagnostic>;
    // Transform transforms one content-mapped source file using mapper in this project's configuration.
    fn transform(
        &self,
        mapper: &Rc<Mapper>,
        request: Request,
    ) -> std::result::Result<Result, GoError>;
    // Close releases this project reference and closes mapper project handles when no references remain.
    fn close(&self) -> std::result::Result<(), GoError>;
    // PORT: not in Go, where the parse goroutines call `Transform`. What a
    // parse worker can transform this project's files of `mapper` with
    // (`ConcurrentTransform`); `None` leaves every transform to the loading
    // thread.
    fn concurrent_transform(&self, mapper: &Rc<Mapper>) -> Option<Arc<ConcurrentTransform>> {
        let _ = mapper;
        None
    }
}

// Go: contentmapper/host.go:296 Host
// Host transforms otherwise unsupported file content into virtual TypeScript during program construction, by driving the
// configured content mappers. Create one with NewHost; Close tears down every mapper it spawned.
pub trait Host {
    // Timings returns a cumulative snapshot of mapper process and protocol activity.
    fn timings(&self) -> Timings;
    // Project returns a retained project-scoped view for spec. Equivalent specs share underlying mapper
    // configuration state; the caller must close the returned Project.
    // PORT: Go returns a nil Project after Close; that is `None`.
    fn project(&self, spec: ProjectSpec) -> Option<Rc<dyn Project>>;
    // Acquire retains the processes for the given mapper identities until the returned lease is released.
    // Acquiring a mapper does not start its process; processes remain lazy until Transform is called.
    // PORT: Go returns a `sync.OnceFunc`; the release can be called again
    // and only its first call releases.
    fn acquire(&self, mappers: &[Rc<Mapper>]) -> Rc<dyn Fn()>;
    // SetLocale updates the locale used to initialize mapper processes. Existing processes are stopped
    // and respawned lazily so subsequent transforms use the new locale.
    fn set_locale(&self, locale: Locale);
    // Transform maps a content-mapped source file to virtual TypeScript using the given content mapper
    // in a short-lived project with default compiler options.
    //
    // A non-nil error indicates the mapper itself failed to produce a result — for example the
    // host hit a broken pipe, a process crash, or could not deserialize the mapper's response.
    fn transform(
        &self,
        mapper: &Rc<Mapper>,
        request: Request,
    ) -> std::result::Result<Result, GoError>;
    // Close shuts down every mapper process the host spawned.
    fn close(&self) -> std::result::Result<(), GoError>;
}
