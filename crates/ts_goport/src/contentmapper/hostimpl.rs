//! Port of Go `internal/contentmapper/hostimpl.go` (tsgo#4712).
//!
//! PORT: threads. The host, its projects and its connections are
//! dispatch-thread values (see the module comment in `mod.rs`): Go's
//! `lifecycleMu` and `mu` are dropped, and the maps are `RefCell`s. Go holds
//! `mu` across some protocol calls; here each call runs with no map
//! borrowed. Other threads share the spawned process (`ProcessExitState`, an
//! `ipc::ReadWriteCloser`), its connection (`MuxConn`, whose read loop runs
//! on its own thread), the timing collector and the stderr logger, so these
//! use `Arc`, `Mutex` and atomics. A parse worker sends the transform
//! requests of an open project through a `ConcurrentTransform`.
//!
//! PORT: Go map iteration order is random. The host maps are `IndexMap`s in
//! insertion order, so the order of the close calls is fixed.

use crate::contentmapper::prelude::*;

use crate::contentmapper::muxconn::{MuxConn, ProtocolFactory};

use crate::flags_macros::go_enum;
use crate::frontend::json_ext::{
    AnyValue, marshal_field, unmarshal_string_as, unmarshal_struct_fields, unmarshal_uint_as,
    write_object_end, write_object_start,
};
use crate::frontend::stringutil_ls::equate_string_case_insensitive;
use crate::gostd::context::{self, AfterFuncStop, CancelFunc};
use crate::gostd::slices::sort_func;
use crate::ipc::{Conn as _, Handler as _, Protocol as _, ReadWriteCloser as _};
use crate::locale::Locale;
use crate::options_json::{CompilerOptionsJSON, marshal_field_omitempty};
use std::borrow::Cow;
use std::cell::Cell;
use std::io::Write;
use std::rc::Weak;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

/// wasm cannot start a process (execute/tsc/compile.rs `spawn_process`), so
/// no mapper has a connection there: its dial always fails. The wasm build
/// calls this where a connection would be used, so that the compiler leaves
/// the mapper protocol (ipc and the requests) out of the wasm module.
// PORT: not in Go.
#[cfg(target_family = "wasm")]
fn no_mapper_connection() {
    unreachable!("wasm cannot start a content mapper process");
}

// PORT: Go mutexes do not poison.
fn lock<T: ?Sized>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

// Go: tspath/rooted_path.go:134 TryRootedFilePathFromAbsolute (ts#64159)
// TryRootedFilePathFromAbsolute validates and normalizes an absolute path,
// including converting platform directory separators to '/', then gives it
// file intent.
// PORT: Go `tspath.RootedFilePath` is a `String`; `None` is Go `ok == false`.
// Lane-local (with `has_rooted_url_suffix`, also in `transpile.rs`) until
// `tspath` has the rooted path helpers of ts#64159.
fn try_rooted_file_path_from_absolute(file_name: &str) -> Option<String> {
    // Go: tspath/rooted_path.go:58 TryRootedPathFromAbsolute
    if has_rooted_url_suffix(file_name) || !tspath::path_is_absolute(file_name) {
        return None;
    }
    let mut path = tspath::get_normalized_absolute_path(file_name, "");
    // Go: tspath/rooted_path.go:65 ensureRootedPathRootSeparator
    if tspath::get_root_length(&path) == path.len()
        && !tspath::has_trailing_directory_separator(&path)
    {
        path.push('/');
    }
    Some(path)
}

// Go: tspath/rooted_path.go:106 hasRootedURLSuffix (ts#64159)
fn has_rooted_url_suffix(path: &str) -> bool {
    // Go: tspath/rooted_path.go:114 hasURLRoot
    let has_url_root = tspath::get_encoded_root_length(path) < 0 && path.contains("://");
    if !has_url_root {
        return false;
    }
    let after_scheme = path.split_once("://").map_or("", |(_, after)| after);
    after_scheme.contains(['?', '#'])
}

// Go: contentmapper/hostimpl.go:30 initializeTimeoutSeconds
const INITIALIZE_TIMEOUT_SECONDS: i32 = 5;

// Go: contentmapper/hostimpl.go:32 initializeTimeout
const INITIALIZE_TIMEOUT: Duration = Duration::from_secs(INITIALIZE_TIMEOUT_SECONDS as u64);

// Go: contentmapper/hostimpl.go:35
// Content mapper protocol method names.
pub const METHOD_INITIALIZE: &str = "initialize";
pub const METHOD_OPEN_PROJECT: &str = "openProject";
pub const METHOD_CLOSE_PROJECT: &str = "closeProject";
pub const METHOD_TRANSFORM: &str = "transform";

// Go: contentmapper/hostimpl.go:43 InitializeParams
// InitializeParams is the parameter object for the initialize request.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InitializeParams {
    // Locale is the BCP 47 locale to use for mapper-authored diagnostic messages, when configured.
    pub locale: String,
    // PositionEncodings lists the coordinate spaces the host accepts.
    pub position_encodings: Vec<PositionEncoding>,
}

impl MarshalerTo for InitializeParams {
    fn marshal_json_to(&self, enc: &mut String) -> std::result::Result<(), JsonError> {
        let first = &mut true;
        write_object_start(enc);
        marshal_field_omitempty(enc, first, "locale", &self.locale)?;
        marshal_field(enc, first, "positionEncodings", &self.position_encodings)?;
        write_object_end(enc);
        Ok(())
    }
}

impl UnmarshalerFrom for InitializeParams {
    fn unmarshal_json_from(
        &mut self,
        dec: &mut JsonDecoder<'_>,
    ) -> std::result::Result<(), JsonError> {
        let is_object =
            unmarshal_struct_fields(dec, "contentmapper.InitializeParams", |name, dec| {
                match name {
                    "locale" => json_unmarshal_decode(dec, &mut self.locale)?,
                    "positionEncodings" => {
                        json_unmarshal_decode(dec, &mut self.position_encodings)?;
                    }
                    _ => return Ok(false),
                }
                Ok(true)
            })?;
        if !is_object {
            *self = InitializeParams::default();
        }
        Ok(())
    }
}

// Go: contentmapper/hostimpl.go:51 InitializeResult
// InitializeResult is the mapper's response to the initialize request.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InitializeResult {
    // PositionEncoding selects the coordinate space for all mappings and diagnostics.
    pub position_encoding: PositionEncoding,
    // DiagnosticSource is the prefix used for every mapper-authored diagnostic code.
    pub diagnostic_source: String,
}

impl MarshalerTo for InitializeResult {
    fn marshal_json_to(&self, enc: &mut String) -> std::result::Result<(), JsonError> {
        let first = &mut true;
        write_object_start(enc);
        marshal_field(enc, first, "positionEncoding", &self.position_encoding)?;
        marshal_field(enc, first, "diagnosticSource", &self.diagnostic_source)?;
        write_object_end(enc);
        Ok(())
    }
}

impl UnmarshalerFrom for InitializeResult {
    fn unmarshal_json_from(
        &mut self,
        dec: &mut JsonDecoder<'_>,
    ) -> std::result::Result<(), JsonError> {
        let is_object =
            unmarshal_struct_fields(dec, "contentmapper.InitializeResult", |name, dec| {
                match name {
                    "positionEncoding" => json_unmarshal_decode(dec, &mut self.position_encoding)?,
                    "diagnosticSource" => json_unmarshal_decode(dec, &mut self.diagnostic_source)?,
                    _ => return Ok(false),
                }
                Ok(true)
            })?;
        if !is_object {
            *self = InitializeResult::default();
        }
        Ok(())
    }
}

// Go: contentmapper/hostimpl.go:59 OpenProjectParams
// OpenProjectParams is the parameter object for the openProject request.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OpenProjectParams {
    // ConfigFileName is the absolute project configuration file name, or empty when there is none.
    pub config_file_name: String,
    // ProjectHandle is an opaque, process-local handle assigned by the host.
    pub project_handle: String,
    // Options is the mapper entry's options from the project's contentMappers configuration.
    pub options: JsonValue,
    // CompilerOptions contains the project's effective compiler options.
    pub compiler_options: JsonValue,
}

impl MarshalerTo for OpenProjectParams {
    fn marshal_json_to(&self, enc: &mut String) -> std::result::Result<(), JsonError> {
        let first = &mut true;
        write_object_start(enc);
        marshal_field(enc, first, "configFileName", &self.config_file_name)?;
        marshal_field(enc, first, "projectHandle", &self.project_handle)?;
        marshal_field_omitempty(enc, first, "options", &self.options)?;
        marshal_field(enc, first, "compilerOptions", &self.compiler_options)?;
        write_object_end(enc);
        Ok(())
    }
}

impl UnmarshalerFrom for OpenProjectParams {
    fn unmarshal_json_from(
        &mut self,
        dec: &mut JsonDecoder<'_>,
    ) -> std::result::Result<(), JsonError> {
        let is_object =
            unmarshal_struct_fields(dec, "contentmapper.OpenProjectParams", |name, dec| {
                match name {
                    "configFileName" => json_unmarshal_decode(dec, &mut self.config_file_name)?,
                    "projectHandle" => json_unmarshal_decode(dec, &mut self.project_handle)?,
                    "options" => json_unmarshal_decode(dec, &mut self.options)?,
                    "compilerOptions" => json_unmarshal_decode(dec, &mut self.compiler_options)?,
                    _ => return Ok(false),
                }
                Ok(true)
            })?;
        if !is_object {
            *self = OpenProjectParams::default();
        }
        Ok(())
    }
}

// Go: contentmapper/hostimpl.go:72 OpenProjectResult
// OpenProjectResult is the mapper's response to an openProject request. ConfigIdentity and WatchedFiles
// may only be returned by mappers that declare dynamicConfig.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OpenProjectResult {
    // ConfigIdentity is a stable fingerprint of all dynamic configuration that can affect transforms.
    pub config_identity: String,
    // WatchedFiles are absolute files whose changes may alter ConfigIdentity or transform output.
    pub watched_files: Vec<String>,
    // OptionDiagnostics report invalid mapper options. Paths are relative to the mapper entry's options object.
    pub option_diagnostics: Vec<OptionDiagnosticResult>,
}

impl MarshalerTo for OpenProjectResult {
    fn marshal_json_to(&self, enc: &mut String) -> std::result::Result<(), JsonError> {
        let first = &mut true;
        write_object_start(enc);
        marshal_field(enc, first, "configIdentity", &self.config_identity)?;
        marshal_field_omitempty(enc, first, "watchedFiles", &self.watched_files)?;
        marshal_field_omitempty(enc, first, "optionDiagnostics", &self.option_diagnostics)?;
        write_object_end(enc);
        Ok(())
    }
}

impl UnmarshalerFrom for OpenProjectResult {
    fn unmarshal_json_from(
        &mut self,
        dec: &mut JsonDecoder<'_>,
    ) -> std::result::Result<(), JsonError> {
        let is_object =
            unmarshal_struct_fields(dec, "contentmapper.OpenProjectResult", |name, dec| {
                match name {
                    "configIdentity" => json_unmarshal_decode(dec, &mut self.config_identity)?,
                    "watchedFiles" => json_unmarshal_decode(dec, &mut self.watched_files)?,
                    "optionDiagnostics" => {
                        json_unmarshal_decode(dec, &mut self.option_diagnostics)?;
                    }
                    _ => return Ok(false),
                }
                Ok(true)
            })?;
        if !is_object {
            *self = OpenProjectResult::default();
        }
        Ok(())
    }
}

// Go: contentmapper/hostimpl.go:81 OptionDiagnosticResult
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OptionDiagnosticResult {
    pub path: Vec<JsonValue>,
    pub message_text: String,
    pub code: i32,
}

impl MarshalerTo for OptionDiagnosticResult {
    fn marshal_json_to(&self, enc: &mut String) -> std::result::Result<(), JsonError> {
        let first = &mut true;
        write_object_start(enc);
        marshal_field(enc, first, "path", &self.path)?;
        marshal_field(enc, first, "messageText", &self.message_text)?;
        marshal_field(enc, first, "code", &self.code)?;
        write_object_end(enc);
        Ok(())
    }
}

impl UnmarshalerFrom for OptionDiagnosticResult {
    fn unmarshal_json_from(
        &mut self,
        dec: &mut JsonDecoder<'_>,
    ) -> std::result::Result<(), JsonError> {
        let is_object =
            unmarshal_struct_fields(dec, "contentmapper.OptionDiagnosticResult", |name, dec| {
                match name {
                    "path" => json_unmarshal_decode(dec, &mut self.path)?,
                    "messageText" => json_unmarshal_decode(dec, &mut self.message_text)?,
                    "code" => json_unmarshal_decode(dec, &mut self.code)?,
                    _ => return Ok(false),
                }
                Ok(true)
            })?;
        if !is_object {
            *self = OptionDiagnosticResult::default();
        }
        Ok(())
    }
}

// Go: contentmapper/hostimpl.go:88 CloseProjectParams
// CloseProjectParams is the parameter object for the closeProject request.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CloseProjectParams {
    pub project_handle: String,
}

impl MarshalerTo for CloseProjectParams {
    fn marshal_json_to(&self, enc: &mut String) -> std::result::Result<(), JsonError> {
        let first = &mut true;
        write_object_start(enc);
        marshal_field(enc, first, "projectHandle", &self.project_handle)?;
        write_object_end(enc);
        Ok(())
    }
}

impl UnmarshalerFrom for CloseProjectParams {
    fn unmarshal_json_from(
        &mut self,
        dec: &mut JsonDecoder<'_>,
    ) -> std::result::Result<(), JsonError> {
        let is_object =
            unmarshal_struct_fields(dec, "contentmapper.CloseProjectParams", |name, dec| {
                match name {
                    "projectHandle" => json_unmarshal_decode(dec, &mut self.project_handle)?,
                    _ => return Ok(false),
                }
                Ok(true)
            })?;
        if !is_object {
            *self = CloseProjectParams::default();
        }
        Ok(())
    }
}

// Go: contentmapper/hostimpl.go:93 PositionEncoding
// PositionEncoding is the coordinate space a mapper uses for mappings and diagnostics.
// PORT: a Go string type, like the lsproto string enums.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct PositionEncoding(pub Cow<'static, str>);

// Go: contentmapper/hostimpl.go:95
impl PositionEncoding {
    pub const UTF8: PositionEncoding = PositionEncoding(Cow::Borrowed("utf-8"));
    pub const UTF16: PositionEncoding = PositionEncoding(Cow::Borrowed("utf-16"));
}

impl MarshalerTo for PositionEncoding {
    fn marshal_json_to(&self, enc: &mut String) -> std::result::Result<(), JsonError> {
        self.0.marshal_json_to(enc)
    }
}

impl UnmarshalerFrom for PositionEncoding {
    fn unmarshal_json_from(
        &mut self,
        dec: &mut JsonDecoder<'_>,
    ) -> std::result::Result<(), JsonError> {
        let mut s = String::new();
        unmarshal_string_as(dec, &mut s, "contentmapper.PositionEncoding")?;
        self.0 = Cow::Owned(s);
        Ok(())
    }
}

// Go: contentmapper/hostimpl.go:101 TransformParams
// TransformParams is the parameter object for the transform request.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TransformParams {
    // FileName is the absolute name of the content-mapped source file being transformed.
    pub file_name: String,
    // Content is the content-mapped source file's text.
    pub content: String,
    // ProjectHandle identifies the mapper project configuration opened for this transform.
    pub project_handle: String,
}

impl MarshalerTo for TransformParams {
    fn marshal_json_to(&self, enc: &mut String) -> std::result::Result<(), JsonError> {
        let first = &mut true;
        write_object_start(enc);
        marshal_field(enc, first, "fileName", &self.file_name)?;
        marshal_field(enc, first, "content", &self.content)?;
        marshal_field(enc, first, "projectHandle", &self.project_handle)?;
        write_object_end(enc);
        Ok(())
    }
}

impl UnmarshalerFrom for TransformParams {
    fn unmarshal_json_from(
        &mut self,
        dec: &mut JsonDecoder<'_>,
    ) -> std::result::Result<(), JsonError> {
        let is_object =
            unmarshal_struct_fields(dec, "contentmapper.TransformParams", |name, dec| {
                match name {
                    "fileName" => json_unmarshal_decode(dec, &mut self.file_name)?,
                    "content" => json_unmarshal_decode(dec, &mut self.content)?,
                    "projectHandle" => json_unmarshal_decode(dec, &mut self.project_handle)?,
                    _ => return Ok(false),
                }
                Ok(true)
            })?;
        if !is_object {
            *self = TransformParams::default();
        }
        Ok(())
    }
}

// Go: contentmapper/hostimpl.go:111 MappedOutput
// MappedOutput is virtual source text and its mapping to an original input.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MappedOutput {
    // Text is the virtual JavaScript or TypeScript source text.
    pub text: String,
    // Extension determines the virtual source file's syntax.
    pub extension: String,
    // Mappings is the span map's tuple-array JSON (see spanmap.Marshal), expressed in the selected
    // position encoding. Absent or empty means the output is fully synthesized.
    pub mappings: JsonValue,
    // DiagnosticDirectives describe framework directives that suppress TypeScript diagnostics in
    // virtual ranges and optionally report an error when no diagnostic is produced.
    // PORT: Go `*DiagnosticDirectives`; nil is `None`.
    pub diagnostic_directives: Option<DiagnosticDirectives>,
}

impl MappedOutput {
    /// The members of `MappedOutput`, for the structs that embed it (Go
    /// inlines an embedded struct's fields).
    fn marshal_fields(
        &self,
        enc: &mut String,
        first: &mut bool,
    ) -> std::result::Result<(), JsonError> {
        marshal_field(enc, first, "text", &self.text)?;
        marshal_field(enc, first, "extension", &self.extension)?;
        marshal_field_omitempty(enc, first, "mappings", &self.mappings)?;
        marshal_field_omitempty(
            enc,
            first,
            "diagnosticDirectives",
            &self.diagnostic_directives,
        )
    }

    /// Decodes one member of `MappedOutput`; false for an unknown name.
    fn unmarshal_field(
        &mut self,
        name: &str,
        dec: &mut JsonDecoder<'_>,
    ) -> std::result::Result<bool, JsonError> {
        match name {
            "text" => json_unmarshal_decode(dec, &mut self.text)?,
            "extension" => json_unmarshal_decode(dec, &mut self.extension)?,
            "mappings" => json_unmarshal_decode(dec, &mut self.mappings)?,
            "diagnosticDirectives" => json_unmarshal_decode(dec, &mut self.diagnostic_directives)?,
            _ => return Ok(false),
        }
        Ok(true)
    }
}

impl MarshalerTo for MappedOutput {
    fn marshal_json_to(&self, enc: &mut String) -> std::result::Result<(), JsonError> {
        let first = &mut true;
        write_object_start(enc);
        self.marshal_fields(enc, first)?;
        write_object_end(enc);
        Ok(())
    }
}

impl UnmarshalerFrom for MappedOutput {
    fn unmarshal_json_from(
        &mut self,
        dec: &mut JsonDecoder<'_>,
    ) -> std::result::Result<(), JsonError> {
        let is_object = unmarshal_struct_fields(dec, "contentmapper.MappedOutput", |name, dec| {
            self.unmarshal_field(name, dec)
        })?;
        if !is_object {
            *self = MappedOutput::default();
        }
        Ok(())
    }
}

// Go: contentmapper/hostimpl.go:125 DiagnosticDirectivePolicy
// DiagnosticDirectivePolicy is the numeric policy stored in a mapped diagnostic directive tuple.
go_enum!(DiagnosticDirectivePolicy, u8 {
    IGNORE = 0;
    EXPECT = 1;
});

impl MarshalerTo for DiagnosticDirectivePolicy {
    fn marshal_json_to(&self, enc: &mut String) -> std::result::Result<(), JsonError> {
        u32::from(self.0).marshal_json_to(enc)
    }
}

impl UnmarshalerFrom for DiagnosticDirectivePolicy {
    fn unmarshal_json_from(
        &mut self,
        dec: &mut JsonDecoder<'_>,
    ) -> std::result::Result<(), JsonError> {
        self.0 = unmarshal_uint_as::<u8>(dec, "contentmapper.DiagnosticDirectivePolicy")?;
        Ok(())
    }
}

// Go: contentmapper/hostimpl.go:132 UnusedExpectDirectiveDiagnostic
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UnusedExpectDirectiveDiagnostic {
    pub code: i32,
    pub message_text: String,
}

impl MarshalerTo for UnusedExpectDirectiveDiagnostic {
    fn marshal_json_to(&self, enc: &mut String) -> std::result::Result<(), JsonError> {
        let first = &mut true;
        write_object_start(enc);
        marshal_field(enc, first, "code", &self.code)?;
        marshal_field(enc, first, "messageText", &self.message_text)?;
        write_object_end(enc);
        Ok(())
    }
}

impl UnmarshalerFrom for UnusedExpectDirectiveDiagnostic {
    fn unmarshal_json_from(
        &mut self,
        dec: &mut JsonDecoder<'_>,
    ) -> std::result::Result<(), JsonError> {
        let is_object = unmarshal_struct_fields(
            dec,
            "contentmapper.UnusedExpectDirectiveDiagnostic",
            |name, dec| {
                match name {
                    "code" => json_unmarshal_decode(dec, &mut self.code)?,
                    "messageText" => json_unmarshal_decode(dec, &mut self.message_text)?,
                    _ => return Ok(false),
                }
                Ok(true)
            },
        )?;
        if !is_object {
            *self = UnusedExpectDirectiveDiagnostic::default();
        }
        Ok(())
    }
}

// Go: contentmapper/hostimpl.go:138 DiagnosticDirectives
// DiagnosticDirectives shares unused-expect diagnostics across compact directive tuples.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DiagnosticDirectives {
    pub unused_expect_directive_diagnostics: Vec<UnusedExpectDirectiveDiagnostic>,
    pub directives: Vec<MappedDiagnosticDirective>,
}

impl MarshalerTo for DiagnosticDirectives {
    fn marshal_json_to(&self, enc: &mut String) -> std::result::Result<(), JsonError> {
        let first = &mut true;
        write_object_start(enc);
        marshal_field(
            enc,
            first,
            "unusedExpectDirectiveDiagnostics",
            &self.unused_expect_directive_diagnostics,
        )?;
        marshal_field(enc, first, "directives", &self.directives)?;
        write_object_end(enc);
        Ok(())
    }
}

impl UnmarshalerFrom for DiagnosticDirectives {
    fn unmarshal_json_from(
        &mut self,
        dec: &mut JsonDecoder<'_>,
    ) -> std::result::Result<(), JsonError> {
        let is_object =
            unmarshal_struct_fields(dec, "contentmapper.DiagnosticDirectives", |name, dec| {
                match name {
                    "unusedExpectDirectiveDiagnostics" => {
                        json_unmarshal_decode(dec, &mut self.unused_expect_directive_diagnostics)?;
                    }
                    "directives" => json_unmarshal_decode(dec, &mut self.directives)?,
                    _ => return Ok(false),
                }
                Ok(true)
            })?;
        if !is_object {
            *self = DiagnosticDirectives::default();
        }
        Ok(())
    }
}

// Go: contentmapper/hostimpl.go:146 MappedDiagnosticDirective
// MappedDiagnosticDirective is encoded as
// [originalStart, originalLength, virtualStart, virtualEnd, policy, unusedExpectDirectiveIndex?].
// An omitted index selects the only unused-expect diagnostic and is invalid when there is not exactly one.
// PORT: Go `*int` is `Option<i32>` (nil is `None`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MappedDiagnosticDirective {
    pub original_start: i32,
    pub original_length: i32,
    pub virtual_start: i32,
    pub virtual_end: i32,
    pub policy: DiagnosticDirectivePolicy,
    pub unused_expect_directive_index: Option<i32>,
}

// Go: contentmapper/hostimpl.go:160 MappedDiagnosticDirective.MarshalJSONTo
impl MarshalerTo for MappedDiagnosticDirective {
    fn marshal_json_to(&self, enc: &mut String) -> std::result::Result<(), JsonError> {
        let mut tuple: Vec<i32> = vec![
            self.original_start,
            self.original_length,
            self.virtual_start,
            self.virtual_end,
            i32::from(self.policy.0),
        ];
        if let Some(index) = self.unused_expect_directive_index {
            tuple.push(index);
        }
        tuple.marshal_json_to(enc)
    }
}

// Go: contentmapper/hostimpl.go:168 MappedDiagnosticDirective.UnmarshalJSONFrom
// PORT: Go returns plain `fmt.Errorf` errors from the method; here they are
// `JsonError`s with the same text (the JSON layer wraps method errors).
impl UnmarshalerFrom for MappedDiagnosticDirective {
    fn unmarshal_json_from(
        &mut self,
        dec: &mut JsonDecoder<'_>,
    ) -> std::result::Result<(), JsonError> {
        let mut tuple: Vec<JsonValue> = Vec::new();
        json_unmarshal_decode(dec, &mut tuple)?;
        if tuple.len() != 5 && tuple.len() != 6 {
            return Err(JsonError {
                message: format!(
                    "diagnostic directive tuple must contain 5 or 6 elements, got {}",
                    tuple.len()
                ),
            });
        }
        *self = MappedDiagnosticDirective::default();
        let element_error = |i: usize, err: JsonError| JsonError {
            message: format!("invalid diagnostic directive tuple element {i}: {err}"),
        };
        json_unmarshal(&tuple[0].0, &mut self.original_start, &[])
            .map_err(|e| element_error(0, e))?;
        json_unmarshal(&tuple[1].0, &mut self.original_length, &[])
            .map_err(|e| element_error(1, e))?;
        json_unmarshal(&tuple[2].0, &mut self.virtual_start, &[])
            .map_err(|e| element_error(2, e))?;
        json_unmarshal(&tuple[3].0, &mut self.virtual_end, &[]).map_err(|e| element_error(3, e))?;
        json_unmarshal(&tuple[4].0, &mut self.policy, &[]).map_err(|e| element_error(4, e))?;
        if tuple.len() == 6 {
            let mut index: i32 = 0;
            let result = json_unmarshal(&tuple[5].0, &mut index, &[]);
            self.unused_expect_directive_index = Some(index);
            result.map_err(|e| element_error(5, e))?;
        }
        Ok(())
    }
}

// Go: contentmapper/hostimpl.go:193 SupplementalOutput
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SupplementalOutput {
    pub mapped_output: MappedOutput,
}

impl MarshalerTo for SupplementalOutput {
    fn marshal_json_to(&self, enc: &mut String) -> std::result::Result<(), JsonError> {
        self.mapped_output.marshal_json_to(enc)
    }
}

impl UnmarshalerFrom for SupplementalOutput {
    fn unmarshal_json_from(
        &mut self,
        dec: &mut JsonDecoder<'_>,
    ) -> std::result::Result<(), JsonError> {
        let is_object =
            unmarshal_struct_fields(dec, "contentmapper.SupplementalOutput", |name, dec| {
                self.mapped_output.unmarshal_field(name, dec)
            })?;
        if !is_object {
            *self = SupplementalOutput::default();
        }
        Ok(())
    }
}

// Go: contentmapper/hostimpl.go:198 TransformResult
// TransformResult is the canonical output for one input file.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TransformResult {
    pub mapped_output: MappedOutput,
    // Diagnostics are mapper-authored errors expressed in original-source coordinates.
    pub diagnostics: Vec<Diagnostic>,
    // Supplemental contains additional unnamed compiler inputs associated with this source file.
    pub supplemental: Vec<SupplementalOutput>,
}

impl MarshalerTo for TransformResult {
    fn marshal_json_to(&self, enc: &mut String) -> std::result::Result<(), JsonError> {
        let first = &mut true;
        write_object_start(enc);
        self.mapped_output.marshal_fields(enc, first)?;
        marshal_field_omitempty(enc, first, "diagnostics", &self.diagnostics)?;
        marshal_field_omitempty(enc, first, "supplemental", &self.supplemental)?;
        write_object_end(enc);
        Ok(())
    }
}

impl UnmarshalerFrom for TransformResult {
    fn unmarshal_json_from(
        &mut self,
        dec: &mut JsonDecoder<'_>,
    ) -> std::result::Result<(), JsonError> {
        let is_object =
            unmarshal_struct_fields(dec, "contentmapper.TransformResult", |name, dec| {
                match name {
                    "diagnostics" => json_unmarshal_decode(dec, &mut self.diagnostics)?,
                    "supplemental" => json_unmarshal_decode(dec, &mut self.supplemental)?,
                    _ => return self.mapped_output.unmarshal_field(name, dec),
                }
                Ok(true)
            })?;
        if !is_object {
            *self = TransformResult::default();
        }
        Ok(())
    }
}

// Go: contentmapper/hostimpl.go:207 Diagnostic
// Diagnostic is an error reported by a mapper in original-source coordinates.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Diagnostic {
    // MessageText is the diagnostic message.
    pub message_text: String,
    // Start and Length locate the diagnostic in the original content using the selected position encoding.
    pub start: i32,
    pub length: i32,
    pub code: i32,
}

impl MarshalerTo for Diagnostic {
    fn marshal_json_to(&self, enc: &mut String) -> std::result::Result<(), JsonError> {
        let first = &mut true;
        write_object_start(enc);
        marshal_field(enc, first, "messageText", &self.message_text)?;
        marshal_field(enc, first, "start", &self.start)?;
        marshal_field(enc, first, "length", &self.length)?;
        marshal_field(enc, first, "code", &self.code)?;
        write_object_end(enc);
        Ok(())
    }
}

impl UnmarshalerFrom for Diagnostic {
    fn unmarshal_json_from(
        &mut self,
        dec: &mut JsonDecoder<'_>,
    ) -> std::result::Result<(), JsonError> {
        let is_object = unmarshal_struct_fields(dec, "contentmapper.Diagnostic", |name, dec| {
            match name {
                "messageText" => json_unmarshal_decode(dec, &mut self.message_text)?,
                "start" => json_unmarshal_decode(dec, &mut self.start)?,
                "length" => json_unmarshal_decode(dec, &mut self.length)?,
                "code" => json_unmarshal_decode(dec, &mut self.code)?,
                _ => return Ok(false),
            }
            Ok(true)
        })?;
        if !is_object {
            *self = Diagnostic::default();
        }
        Ok(())
    }
}

// Go: contentmapper/hostimpl.go:218 dialFunc
// dialFunc establishes a running connection to a mapper. In production it spawns the mapper's process;
// tests substitute an in-memory connection. It returns the connection and a closer that tears it down.
// PORT: Go's `io.Closer` is the spawned process connection.
type DialFunc = Rc<
    dyn Fn(
        &Context,
        &Rc<Mapper>,
        &Locale,
    ) -> std::result::Result<
        (
            Rc<dyn ipc::Conn>,
            Arc<dyn ProcessExitState>,
            PositionEncoding,
            String,
        ),
        GoError,
    >,
>;

// Go: contentmapper/hostimpl.go:221 host
// host manages one child process per mapper identity. It is the production implementation of Host.
// PORT: Go unexported `host`; other packages see it through the `Host`
// interface, so the Rust name is `HostImpl` (as `CompilerHostImpl`). Go nil
// maps after Close are `None`. `this` gives the project leases and the
// acquire releases their `*host` pointer.
pub struct HostImpl {
    ctx: Context,
    cancel: CancelFunc,
    stop: AfterFuncStop,
    dial: DialFunc,
    timing: Arc<TimingCollector>,

    diagnostic_locale: RefCell<Locale>,

    conns: RefCell<Option<IndexMap<String, Rc<RefCell<MapperConn>>>>>,
    projects: RefCell<Option<IndexMap<String, Rc<RefCell<ProjectEntry>>>>>,
    project_leases: RefCell<Option<IndexMap<String, Rc<ProjectLease>>>>,
    next_project_id: Cell<u64>,

    // PORT: Go's `AfterFunc(ctx, h.Close)` runs on another goroutine. The
    // host is dispatch-thread state, so the context callback closes only
    // the processes, which it reaches through this list (see `new_with_dial`).
    processes: Arc<Mutex<Vec<std::sync::Weak<dyn ProcessExitState>>>>,
    this: Weak<HostImpl>,
}

// Go: contentmapper/hostimpl.go:238 projectEntry
struct ProjectEntry {
    mapper: Rc<Mapper>,
    spec: ProjectSpec,
    project_handle: String,
    opened: bool,
    config_identity: String,
    watched_files: Vec<String>,
    option_diagnostics: Vec<OptionDiagnostic>,
}

// Go: contentmapper/hostimpl.go:248 mapperConn
#[derive(Default)]
struct MapperConn {
    conn: Option<Rc<dyn ipc::Conn>>,
    closer: Option<Arc<dyn ProcessExitState>>,
    // err, when non-nil, records that this mapper failed to start; it is cached so we do not repeatedly
    // try (and fail) to spawn a broken mapper.
    err: Option<GoError>,
    position_encoding: PositionEncoding,
    diagnostic_source: String,
    // refs is the number of active Acquire calls retaining this identity.
    refs: i32,
}

// Go: contentmapper/hostimpl.go:260 operationTiming
// PORT: Go `operationTiming` and `OperationTiming` (host.go) are both
// `OperationTiming` in Rust; the unexported one is `OperationTimingImpl`.
// The parse workers record transforms too (`ConcurrentTransform`), so the
// collectors keep Go's atomics and mutex.
#[derive(Default)]
struct OperationTimingImpl {
    count: AtomicU64,
    /// Nanoseconds (Go `atomic.Int64` of a `time.Duration`).
    duration: AtomicU64,
}

impl OperationTimingImpl {
    // Go: contentmapper/hostimpl.go:265 operationTiming.record
    fn record(&self, start: Instant) {
        self.count.fetch_add(1, Ordering::Relaxed);
        self.duration.fetch_add(
            u64::try_from(start.elapsed().as_nanos()).unwrap_or(u64::MAX),
            Ordering::Relaxed,
        );
    }

    // Go: contentmapper/hostimpl.go:270 operationTiming.snapshot
    fn snapshot(&self) -> OperationTiming {
        OperationTiming {
            count: self.count.load(Ordering::Relaxed),
            duration: Duration::from_nanos(self.duration.load(Ordering::Relaxed)),
        }
    }
}

// Go: contentmapper/hostimpl.go:274 timingCollector
struct TimingCollector {
    mu: Mutex<TimingState>,
}

/// The fields of Go `timingCollector` that its `mu` guards.
struct TimingState {
    mappers: IndexMap<String, Arc<MapperTimingCollector>>,
    active_requests: u64,
    request_wait_start: Instant,
    request_wait_elapsed: Duration,
}

// Go: contentmapper/hostimpl.go:282 mapperTimingCollector
// PORT: Go's `owner` pointer is a `Weak`, so the collector and its mapper
// entries do not keep each other alive.
#[derive(Default)]
struct MapperTimingCollector {
    spawn: OperationTimingImpl,
    initialize: OperationTimingImpl,
    open_project: OperationTimingImpl,
    close_project: OperationTimingImpl,
    transform: OperationTimingImpl,
    owner: std::sync::Weak<TimingCollector>,
}

impl TimingCollector {
    /// Go `&timingCollector{mappers: make(map[string]*mapperTimingCollector)}`.
    fn new() -> TimingCollector {
        TimingCollector {
            mu: Mutex::new(TimingState {
                mappers: IndexMap::new(),
                active_requests: 0,
                request_wait_start: Instant::now(),
                request_wait_elapsed: Duration::ZERO,
            }),
        }
    }

    // Go: contentmapper/hostimpl.go:291 timingCollector.mapper
    fn mapper(self: &Arc<Self>, identity: &str) -> Arc<MapperTimingCollector> {
        let mut state = lock(&self.mu);
        if let Some(timing) = state.mappers.get(identity) {
            return timing.clone();
        }
        let timing = Arc::new(MapperTimingCollector {
            owner: Arc::downgrade(self),
            ..Default::default()
        });
        state.mappers.insert(identity.to_string(), timing.clone());
        timing
    }

    // Go: contentmapper/hostimpl.go:302 timingCollector.snapshot
    fn snapshot(&self) -> Timings {
        let (request_wait, mappers) = {
            let state = lock(&self.mu);
            let mut request_wait = state.request_wait_elapsed;
            if state.active_requests != 0 {
                request_wait += state.request_wait_start.elapsed();
            }
            (request_wait, state.mappers.clone())
        };
        let mut result = Timings {
            mappers: IndexMap::with_capacity(mappers.len()),
            request_wait,
        };
        for (identity, timing) in mappers {
            result.mappers.insert(
                identity,
                MapperTimings {
                    spawn: timing.spawn.snapshot(),
                    initialize: timing.initialize.snapshot(),
                    open_project: timing.open_project.snapshot(),
                    close_project: timing.close_project.snapshot(),
                    transform: timing.transform.snapshot(),
                },
            );
        }
        result
    }
}

impl MapperTimingCollector {
    fn owner(&self) -> Arc<TimingCollector> {
        self.owner
            .upgrade()
            .expect("content mapper timing collector outlives its host")
    }

    // Go: contentmapper/hostimpl.go:324 mapperTimingCollector.startRequest
    fn start_request(&self) -> Instant {
        let owner = self.owner();
        let mut state = lock(&owner.mu);
        if state.active_requests == 0 {
            state.request_wait_start = Instant::now();
        }
        state.active_requests += 1;
        drop(state);
        Instant::now()
    }

    // Go: contentmapper/hostimpl.go:334 mapperTimingCollector.finishRequest
    fn finish_request(&self, operation: &OperationTimingImpl, start: Instant) {
        operation.record(start);
        let owner = self.owner();
        let mut state = lock(&owner.mu);
        state.active_requests -= 1;
        if state.active_requests == 0 {
            let elapsed = state.request_wait_start.elapsed();
            state.request_wait_elapsed += elapsed;
        }
    }
}

// Go: contentmapper/hostimpl.go:349 Spawner
// Spawner starts a child process, returning its stdio as an io.ReadWriteCloser (Read is the
// process's stdout, Write is its stdin) whose Close tears the process down. This seam keeps os/exec out
// of this package: production hosts spawn a real process, tests supply an in-process pipe.
// PORT: Go `io.Discard` as `stderr` is `None`. The result is a
// `ProcessExitState` (see there).
pub trait Spawner {
    fn spawn(
        &self,
        command: &[String],
        dir: &str,
        stderr: Option<Box<dyn Write + Send>>,
    ) -> std::result::Result<Arc<dyn ProcessExitState>, GoError>;
}

// Go: contentmapper/hostimpl.go:353 processExitState
// PORT: Go type-asserts the spawned `io.ReadWriteCloser` for
// `ExitCode() (int, bool)`. Rust has no dynamic interface check, so a
// spawned process is this trait: an `ipc::ReadWriteCloser` whose
// `exit_code` defaults to `(0, false)`, the result for a Go value that has no
// `ExitCode` method.
pub trait ProcessExitState: ipc::ReadWriteCloser {
    fn exit_code(&self) -> (i32, bool) {
        (0, false)
    }
}

/// Go `SpawnerFunc` spawn function.
pub type SpawnFn = dyn Fn(
    &[String],
    &str,
    Option<Box<dyn Write + Send>>,
) -> std::result::Result<Arc<dyn ProcessExitState>, GoError>;

// Go: contentmapper/hostimpl.go:358 SpawnerFunc
// SpawnerFunc adapts a spawn function to the Spawner interface.
pub struct SpawnerFunc(pub Box<SpawnFn>);

impl Spawner for SpawnerFunc {
    // Go: contentmapper/hostimpl.go:360 SpawnerFunc.Spawn
    fn spawn(
        &self,
        command: &[String],
        dir: &str,
        stderr: Option<Box<dyn Write + Send>>,
    ) -> std::result::Result<Arc<dyn ProcessExitState>, GoError> {
        (self.0)(command, dir, stderr)
    }
}

// Go: contentmapper/hostimpl.go:365 Logger
// Logger receives content mapper protocol and process output as complete log lines.
// PORT: the stderr lines come from the spawner's thread, so it is `Send + Sync`.
pub type Logger = Arc<dyn Fn(&str) + Send + Sync>;

// Go: contentmapper/hostimpl.go:368 HostOptions
// HostOptions configures optional content mapper process logging.
// PORT: Go nil `Logger` is `None`.
#[derive(Clone, Default)]
pub struct HostOptions {
    pub logger: Option<Logger>,
}

// Go: contentmapper/hostimpl.go:372 loggingProtocol
// PORT: Go embeds `ipc.Protocol`; the methods below forward to `protocol`.
struct LoggingProtocol {
    protocol: Box<dyn ipc::Protocol>,
    mapper_name: String,
    logger: Logger,
}

impl LoggingProtocol {
    // Go: contentmapper/hostimpl.go:378 loggingProtocol.log
    fn log(&self, direction: &str, message: &dyn MarshalerTo) {
        match json_marshal(message, &[]) {
            Err(err) => (self.logger)(&format!(
                "[content mapper: {}] {}: <failed to serialize: {}>",
                self.mapper_name, direction, err
            )),
            Ok(data) => (self.logger)(&format!(
                "[content mapper: {}] {}: {}",
                self.mapper_name, direction, data
            )),
        }
    }
}

impl ipc::Protocol for LoggingProtocol {
    // Go: contentmapper/hostimpl.go:387 loggingProtocol.ReadMessage
    fn read_message(&mut self) -> std::result::Result<ipc::Message, GoError> {
        let message = self.protocol.read_message();
        if let Ok(message) = &message {
            self.log("receive", message);
        }
        message
    }

    // Go: contentmapper/hostimpl.go:395 loggingProtocol.WriteRequest
    // PORT: Go logs a message that shares `params` with the write. The Rust
    // params are owned, so the message is logged first and the params move
    // on to the write.
    fn write_request(
        &mut self,
        id: Option<&jsonrpc::ID>,
        method: &str,
        params: Option<Box<dyn AnyValue>>,
    ) -> std::result::Result<(), GoError> {
        let mut message = jsonrpc::RequestMessage {
            id: id.cloned(),
            method: method.to_string(),
            params,
            ..Default::default()
        };
        self.log("send", &message);
        let params = message.params.take();
        self.protocol.write_request(id, method, params)
    }

    // Go: contentmapper/hostimpl.go:401 loggingProtocol.WriteNotification
    fn write_notification(
        &mut self,
        method: &str,
        params: Option<Box<dyn AnyValue>>,
    ) -> std::result::Result<(), GoError> {
        let mut message = jsonrpc::RequestMessage {
            method: method.to_string(),
            params,
            ..Default::default()
        };
        self.log("send", &message);
        let params = message.params.take();
        self.protocol.write_notification(method, params)
    }

    // Go: contentmapper/hostimpl.go:407 loggingProtocol.WriteResponse
    fn write_response(
        &mut self,
        id: Option<&jsonrpc::ID>,
        result: Option<Box<dyn AnyValue>>,
    ) -> std::result::Result<(), GoError> {
        let mut message = jsonrpc::ResponseMessage {
            id: id.cloned(),
            result,
            ..Default::default()
        };
        self.log("send", &message);
        let result = message.result.take();
        self.protocol.write_response(id, result)
    }

    // Go: contentmapper/hostimpl.go:413 loggingProtocol.WriteError
    fn write_error(
        &mut self,
        id: Option<&jsonrpc::ID>,
        response_error: &jsonrpc::ResponseError,
    ) -> std::result::Result<(), GoError> {
        let message = jsonrpc::ResponseMessage {
            id: id.cloned(),
            error: Some(response_error.clone()),
            ..Default::default()
        };
        self.log("send", &message);
        self.protocol.write_error(id, response_error)
    }
}

// Go: contentmapper/hostimpl.go:419 stderrLogger
// PORT: Go keeps `pending` as a string of raw bytes. Here it is bytes too,
// so a character split between two writes stays whole; each complete line
// goes to the logger as text (invalid UTF-8 becomes U+FFFD).
struct StderrLogger {
    mapper_name: String,
    logger: Logger,
    pending: Mutex<Vec<u8>>,
}

impl StderrLogger {
    // Go: contentmapper/hostimpl.go:426 stderrLogger.Write
    fn write(&self, data: &[u8]) -> usize {
        let mut pending = lock(&self.pending);
        pending.extend_from_slice(data);
        loop {
            let Some(index) = pending.iter().position(|&b| b == b'\n') else {
                break;
            };
            let line: Vec<u8> = pending.drain(..=index).collect();
            let line = &line[..line.len() - 1];
            self.log(line.strip_suffix(b"\r").unwrap_or(line));
        }
        data.len()
    }

    // Go: contentmapper/hostimpl.go:441 stderrLogger.flush
    fn flush(&self) {
        let mut pending = lock(&self.pending);
        if !pending.is_empty() {
            let line = std::mem::take(&mut *pending);
            self.log(line.strip_suffix(b"\r").unwrap_or(&line));
        }
    }

    // Go: contentmapper/hostimpl.go:450 stderrLogger.log
    fn log(&self, message: &[u8]) {
        (self.logger)(&format!(
            "[content mapper: {}] stderr: {}",
            self.mapper_name,
            String::from_utf8_lossy(message)
        ));
    }
}

/// The `io.Writer` view of a `stderrLogger` that the spawner gets.
// PORT: Go passes the `*stderrLogger` itself; the spawner owns its writer
// here, and the logged process keeps the shared logger to flush it.
struct StderrLoggerWriter(Arc<StderrLogger>);

impl Write for StderrLoggerWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        Ok(self.0.write(buf))
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

// Go: contentmapper/hostimpl.go:454 loggedProcess
// PORT: Go embeds `io.ReadWriteCloser`, which does not promote the spawned
// value's `ExitCode` method, so a logged process reports no exit state
// (`exit_code` keeps the default).
struct LoggedProcess {
    inner: Arc<dyn ProcessExitState>,
    stderr: Arc<StderrLogger>,
}

impl ipc::ReadWriteCloser for LoggedProcess {
    fn read(&self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.inner.read(buf)
    }

    fn write(&self, buf: &[u8]) -> std::io::Result<usize> {
        self.inner.write(buf)
    }

    fn flush(&self) -> std::io::Result<()> {
        self.inner.flush()
    }

    // Go: contentmapper/hostimpl.go:459 loggedProcess.Close
    fn close(&self) -> std::result::Result<(), GoError> {
        let err = self.inner.close();
        self.stderr.flush();
        err
    }
}

impl ProcessExitState for LoggedProcess {}

// Go: contentmapper/hostimpl.go:465 closeOnceReadWriteCloser
struct CloseOnceReadWriteCloser {
    inner: Arc<dyn ProcessExitState>,
    // Go `once` and `err`.
    once: OnceLock<std::result::Result<(), GoError>>,
}

impl ipc::ReadWriteCloser for CloseOnceReadWriteCloser {
    fn read(&self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.inner.read(buf)
    }

    fn write(&self, buf: &[u8]) -> std::io::Result<usize> {
        self.inner.write(buf)
    }

    fn flush(&self) -> std::io::Result<()> {
        self.inner.flush()
    }

    // Go: contentmapper/hostimpl.go:471 closeOnceReadWriteCloser.Close
    fn close(&self) -> std::result::Result<(), GoError> {
        self.once.get_or_init(|| self.inner.close()).clone()
    }
}

impl ProcessExitState for CloseOnceReadWriteCloser {
    // Go: contentmapper/hostimpl.go:476 closeOnceReadWriteCloser.ExitCode
    fn exit_code(&self) -> (i32, bool) {
        self.inner.exit_code()
    }
}

/// The error text prefix of Go `ipc.AsyncConn.Call` for an error response.
const IPC_REMOTE_ERROR_PREFIX: &str = "ipc: remote error [";

// PORT: Go runs the connection's read loop on a goroutine and closes the
// process when it ends: `go func() { _ = conn.Run(ctx); _ = rwc.Close() }()`.
// `MuxConn` reads on its own thread, as `Run` does, so that the parse
// workers can call the mapper too (`ConcurrentTransform`). This connection
// closes the process when a call fails because the connection ended (a read
// or a write failed), which is where Go's read loop ends. A call that ends
// with `ctx` or with the mapper's error response leaves the process open,
// as in Go.
#[derive(Clone)]
struct ProcessConn {
    conn: Arc<MuxConn>,
    rwc: Arc<dyn ProcessExitState>,
}

impl ProcessConn {
    /// `MuxConn::call`, then the process close of a call that failed
    /// because the connection ended. Any thread can call it.
    fn call_any_thread(
        &self,
        ctx: &Context,
        method: &str,
        params: Option<Box<dyn AnyValue>>,
    ) -> std::result::Result<JsonValue, GoError> {
        let result = ipc::Conn::call(&*self.conn, ctx, method, params);
        if let Err(err) = &result {
            self.close_if_ended(ctx, err);
        }
        result
    }

    fn close_if_ended(&self, ctx: &Context, err: &GoError) {
        if ctx.err().is_none() && !err.error().starts_with(IPC_REMOTE_ERROR_PREFIX) {
            let _ = self.rwc.close();
        }
    }
}

impl ipc::Conn for ProcessConn {
    fn run(&self, ctx: &Context) -> std::result::Result<(), GoError> {
        let result = ipc::Conn::run(&*self.conn, ctx);
        let _ = self.rwc.close();
        result
    }

    // PORT: a panic of the read loop ends the process in Go. Here it
    // panics on the loading thread, in the first call after it.
    fn call(
        &self,
        ctx: &Context,
        method: &str,
        params: Option<Box<dyn AnyValue>>,
    ) -> std::result::Result<JsonValue, GoError> {
        let result = self.call_any_thread(ctx, method, params);
        if result.is_err()
            && let Some(payload) = self.conn.take_read_panic()
        {
            std::panic::resume_unwind(payload);
        }
        result
    }

    fn notify(
        &self,
        ctx: &Context,
        method: &str,
        params: Option<Box<dyn AnyValue>>,
    ) -> std::result::Result<(), GoError> {
        let result = ipc::Conn::notify(&*self.conn, ctx, method, params);
        if let Err(err) = &result {
            self.close_if_ended(ctx, err);
        }
        result
    }

    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
}

// PORT: not in Go, where the parse goroutines call `projectLease.Transform`
// (hostimpl.go:1000) on an open project. The project is dispatch-thread
// state; this is what a parse worker needs to send the transform request of
// a file of an open project, as `transform_locked` does: the mapper's
// connection, which threads can share (`MuxConn`), the project handle, the
// host context, the timing and what decoding needs. The loader disables it
// when the mapper fails (`disable`), so that the workers send no more.
pub struct ConcurrentTransform {
    conn: ProcessConn,
    ctx: Context,
    project_handle: String,
    position_encoding: PositionEncoding,
    diagnostic_source: String,
    timing: Arc<MapperTimingCollector>,
    disabled: AtomicBool,
}

impl ConcurrentTransform {
    /// The transform of `content` of `file_name`, as `transform_locked`
    /// makes it, errors and timings included. `None` when the mapper is
    /// disabled, or when the connection's read loop panicked: then the
    /// loader transforms the file itself, and its call panics.
    // Go: contentmapper/hostimpl.go:772 host.transformLocked
    pub fn transform(
        &self,
        file_name: &str,
        content: &str,
    ) -> Option<std::result::Result<Result, GoError>> {
        if self.disabled.load(Ordering::Relaxed) {
            return None;
        }
        let start = self.timing.start_request();
        let raw = self.conn.call_any_thread(
            &self.ctx,
            METHOD_TRANSFORM,
            Some(Box::new(TransformParams {
                file_name: file_name.to_string(),
                content: content.to_string(),
                project_handle: self.project_handle.clone(),
            })),
        );
        self.timing.finish_request(&self.timing.transform, start);
        let raw = match raw {
            Ok(raw) => raw,
            Err(_) if self.conn.conn.read_panicked() => return None,
            Err(err) => {
                return Some(Err(new_transform_error(
                    TransformErrorKind::REQUEST,
                    Some(err),
                )
                .to_go_error()));
            }
        };
        Some(
            decode_transform_result(
                &raw,
                content,
                &self.position_encoding,
                &self.diagnostic_source,
            )
            .map_err(|err| {
                new_transform_error(TransformErrorKind::RESPONSE, Some(err)).to_go_error()
            }),
        )
    }

    /// Stops the transforms that the workers start after this: the loader
    /// disabled the mapper (`FileLoader::content_mapper_unavailable`).
    pub fn disable(&self) {
        self.disabled.store(true, Ordering::Relaxed);
    }
}

// Go: contentmapper/hostimpl.go:487 NewHost
// NewHost creates a Host that spawns each mapper's process via the given spawner and drives it over a
// JSON-RPC connection. The host's lifetime is bound to ctx: cancelling it (e.g. the CLI's signal context
// on SIGINT, or a build/watch session ending) tears every mapper process down, so owners of a session
// context need not close the host explicitly. Close does the same synchronously.
pub fn new_host(
    ctx: &Context,
    spawner: Rc<dyn Spawner>,
    diagnostic_locale: Locale,
) -> Rc<dyn Host> {
    new_host_with_options(ctx, spawner, diagnostic_locale, HostOptions::default())
}

// Go: contentmapper/hostimpl.go:492 NewHostWithOptions
// NewHostWithOptions creates a Host with optional protocol and process logging.
pub fn new_host_with_options(
    ctx: &Context,
    spawner: Rc<dyn Spawner>,
    diagnostic_locale: Locale,
    options: HostOptions,
) -> Rc<dyn Host> {
    let logger = options.logger;
    let timing = Arc::new(TimingCollector::new());
    let dial_timing = timing.clone();
    let dial: DialFunc = Rc::new(
        move |ctx: &Context, mapper: &Rc<Mapper>, diagnostic_locale: &Locale| {
            if mapper.manifest.exec.is_empty() {
                return Err(errors::new(format!(
                    "content mapper {} declares no command to run",
                    gostd::strconv::quote(&mapper.definition.package)
                )));
            }
            let mapper_timing = dial_timing.mapper(&mapper.identity());
            let diagnostic_name = mapper.diagnostic_name();
            let spawn_start = Instant::now();
            // Go `io.Discard`.
            let mut stderr: Option<Box<dyn Write + Send>> = None;
            let mut stderr_log: Option<Arc<StderrLogger>> = None;
            if let Some(logger) = &logger {
                let log = Arc::new(StderrLogger {
                    mapper_name: diagnostic_name.clone(),
                    logger: logger.clone(),
                    pending: Mutex::new(Vec::new()),
                });
                stderr = Some(Box::new(StderrLoggerWriter(log.clone())));
                stderr_log = Some(log);
            }
            let spawned = spawner.spawn(&mapper.manifest.exec, &mapper.package_directory, stderr);
            mapper_timing.spawn.record(spawn_start);
            let mut rwc: Arc<dyn ProcessExitState> = match spawned {
                Ok(rwc) => {
                    // PORT: not in Go (see `no_mapper_connection`).
                    #[cfg(target_family = "wasm")]
                    no_mapper_connection();
                    rwc
                }
                Err(err) => {
                    return Err(InitializeError {
                        kind: InitializeErrorKind::PROCESS_START,
                        mapper_name: diagnostic_name,
                        command: mapper.manifest.exec[0].clone(),
                        detail: err.error(),
                        ..Default::default()
                    }
                    .to_go_error());
                }
            };
            if let Some(stderr_log) = stderr_log {
                rwc = Arc::new(LoggedProcess {
                    inner: rwc,
                    stderr: stderr_log,
                });
            }
            let rwc: Arc<dyn ProcessExitState> = Arc::new(CloseOnceReadWriteCloser {
                inner: rwc,
                once: OnceLock::new(),
            });
            let transport: Arc<dyn ipc::ReadWriteCloser> = rwc.clone();
            let protocol_logger = logger
                .clone()
                .map(|logger| (diagnostic_name.clone(), logger));
            let new_protocol: ProtocolFactory = Arc::new(move || {
                let protocol: Box<dyn ipc::Protocol> =
                    Box::new(ipc::new_jsonrpc_protocol(transport.clone()));
                match &protocol_logger {
                    Some((mapper_name, logger)) => Box::new(LoggingProtocol {
                        protocol,
                        mapper_name: mapper_name.clone(),
                        logger: logger.clone(),
                    }),
                    None => protocol,
                }
            });
            // Go starts the read loop here (`go conn.Run(ctx)`); `MuxConn`
            // starts its reader thread.
            let conn: Rc<dyn ipc::Conn> = Rc::new(ProcessConn {
                conn: MuxConn::start(new_protocol, Arc::new(RejectHandler)),
                rwc: rwc.clone(),
            });
            let (initialize_ctx, cancel) = context::with_timeout(ctx, INITIALIZE_TIMEOUT);
            let initialize_start = mapper_timing.start_request();
            let handshake_result = handshake(&initialize_ctx, &conn, diagnostic_locale);
            mapper_timing.finish_request(&mapper_timing.initialize, initialize_start);
            let initialize_ctx_err = initialize_ctx.err();
            cancel();
            match handshake_result {
                Ok((position_encoding, diagnostic_source)) => {
                    Ok((conn, rwc, position_encoding, diagnostic_source))
                }
                Err(err) => {
                    let (exit_code, exited) = rwc.exit_code();
                    let _ = rwc.close();
                    if let Some(mut initialize_error) = errors::as_type::<InitializeError>(&err) {
                        // PORT: Go sets the name on the error it found; that
                        // error is `err` itself, so the port returns a copy.
                        initialize_error.mapper_name = diagnostic_name;
                        return Err(initialize_error.to_go_error());
                    }
                    if exited {
                        return Err(InitializeError {
                            kind: InitializeErrorKind::PROCESS_EXIT,
                            mapper_name: diagnostic_name,
                            exit_code,
                            ..Default::default()
                        }
                        .to_go_error());
                    }
                    if initialize_ctx_err.is_some()
                        || errors::is(&err, &context::CANCELED)
                        || errors::is(&err, &context::DEADLINE_EXCEEDED)
                    {
                        return Err(InitializeError {
                            kind: InitializeErrorKind::NO_RESPONSE,
                            mapper_name: diagnostic_name,
                            timeout_seconds: INITIALIZE_TIMEOUT_SECONDS,
                            ..Default::default()
                        }
                        .to_go_error());
                    }
                    Err(InitializeError {
                        kind: InitializeErrorKind::REQUEST,
                        mapper_name: diagnostic_name,
                        detail: err.error(),
                        ..Default::default()
                    }
                    .to_go_error())
                }
            }
        },
    );
    new_with_dial(ctx, diagnostic_locale, timing, dial)
}

// Go: contentmapper/hostimpl.go:555 newWithDial
// PORT: Go's `AfterFunc(ctx, func() { _ = h.Close() })` would close the
// host from another goroutine. The host is dispatch-thread state, so the
// callback closes the spawned processes (the part of Close that other
// threads can reach, through `processes`); the host maps stay until the
// owner closes or drops the host.
// Go has no close at drop, and neither has the host. A mapper process
// closes at `close`, when `ctx` ends (the callback above), or when the last
// project lease or `acquire` that uses its identity is released
// (`release`). The leases and the acquire releases hold the host (`rc`), as
// Go's `*host` pointers do, and the `MuxConn` reader thread holds each
// process, as Go's `Run` goroutine holds `rwc`. So, as in Go, an owner that
// drops the host while a project or an acquire is not released leaves
// those processes running until `ctx` ends. The owners follow Go's: `tsc`
// closes its project, `tsc -b` closes the host, and the watcher and the LSP
// session end with their context.
fn new_with_dial(
    ctx: &Context,
    diagnostic_locale: Locale,
    timing: Arc<TimingCollector>,
    dial: DialFunc,
) -> Rc<HostImpl> {
    let (host_ctx, cancel) = context::with_cancel(ctx);
    let processes: Arc<Mutex<Vec<std::sync::Weak<dyn ProcessExitState>>>> =
        Arc::new(Mutex::new(Vec::new()));
    let stop = {
        let processes = processes.clone();
        context::after_func(ctx, move || {
            let processes: Vec<Arc<dyn ProcessExitState>> = lock(&processes)
                .iter()
                .filter_map(std::sync::Weak::upgrade)
                .collect();
            for process in processes {
                let _ = process.close();
            }
        })
    };
    Rc::new_cyclic(|this| HostImpl {
        ctx: host_ctx,
        cancel,
        stop,
        dial,
        timing,
        diagnostic_locale: RefCell::new(diagnostic_locale),
        conns: RefCell::new(Some(IndexMap::new())),
        projects: RefCell::new(Some(IndexMap::new())),
        project_leases: RefCell::new(Some(IndexMap::new())),
        next_project_id: Cell::new(0),
        processes,
        this: this.clone(),
    })
}

impl HostImpl {
    fn rc(&self) -> Rc<HostImpl> {
        self.this.upgrade().expect("content mapper host is alive")
    }

    // Go: contentmapper/hostimpl.go:650 host.openProjectLocked
    fn open_project_locked(
        &self,
        ctx: &Context,
        entry: &Rc<RefCell<ProjectEntry>>,
    ) -> std::result::Result<(), GoError> {
        if entry.borrow().opened {
            return Ok(());
        }
        let mapper = entry.borrow().mapper.clone();
        let (conn, _, diagnostic_source) = self.conn_for_locked(&mapper)?;
        // PORT: not in Go (see `no_mapper_connection`).
        #[cfg(target_family = "wasm")]
        no_mapper_connection();
        let (config_file_name, project_handle, compiler_options) = {
            let entry = entry.borrow();
            (
                entry.spec.config_file_name.clone(),
                entry.project_handle.clone(),
                entry.spec.compiler_options.clone(),
            )
        };
        // Go `json.Marshal(entry.spec.CompilerOptions)`; a nil pointer is `null`.
        let compiler_options = match &compiler_options {
            None => "null".to_string(),
            Some(options) => {
                json_marshal(&CompilerOptionsJSON(options), &[]).map_err(errors::from_value)?
            }
        };
        let mapper_timing = self.timing.mapper(&mapper.identity());
        let start = mapper_timing.start_request();
        let raw = conn.call(
            ctx,
            METHOD_OPEN_PROJECT,
            Some(Box::new(OpenProjectParams {
                config_file_name,
                project_handle,
                options: mapper.definition.options.clone(),
                // PORT: a `json.Value` holds Go bytes; the text is in the
                // port form.
                compiler_options: JsonValue(
                    crate::scanner_util::go_string_bytes(&compiler_options).into_owned(),
                ),
            })),
        );
        mapper_timing.finish_request(&mapper_timing.open_project, start);
        let raw = raw?;
        let malformed = || {
            ProjectError {
                kind: ProjectErrorKind::MALFORMED_RESPONSE,
            }
            .to_go_error()
        };
        let mut result = OpenProjectResult::default();
        if json_unmarshal(&raw.0, &mut result, &[]).is_err() {
            return Err(malformed());
        }
        if mapper.manifest.dynamic_config && result.config_identity.is_empty() {
            return Err(ProjectError {
                kind: ProjectErrorKind::MISSING_CONFIG_IDENTITY,
            }
            .to_go_error());
        }
        if !mapper.manifest.dynamic_config && !result.config_identity.is_empty() {
            return Err(ProjectError {
                kind: ProjectErrorKind::UNEXPECTED_CONFIG_IDENTITY,
            }
            .to_go_error());
        }
        if !mapper.manifest.dynamic_config && !result.watched_files.is_empty() {
            return Err(ProjectError {
                kind: ProjectErrorKind::UNEXPECTED_WATCHED_FILES,
            }
            .to_go_error());
        }
        let mut entry = entry.borrow_mut();
        entry.config_identity = result.config_identity.clone();
        // ts#64159: a watched file is kept as a rooted file path, normalized.
        // A URL with a query or fragment is not one.
        entry.watched_files = vec![String::new(); result.watched_files.len()];
        for (i, file_name) in result.watched_files.iter().enumerate() {
            let Some(typed_file_name) = try_rooted_file_path_from_absolute(file_name) else {
                return Err(ProjectError {
                    kind: ProjectErrorKind::NON_ABSOLUTE_WATCHED_FILE,
                }
                .to_go_error());
            };
            entry.watched_files[i] = typed_file_name;
        }
        entry.option_diagnostics = Vec::with_capacity(result.option_diagnostics.len());
        for diagnostic in &result.option_diagnostics {
            let mut path = vec![OptionPathSegment::default(); diagnostic.path.len()];
            for (j, raw_segment) in diagnostic.path.iter().enumerate() {
                match json_value_kind(raw_segment) {
                    b'"' => {
                        let mut property = String::new();
                        if json_unmarshal(&raw_segment.0, &mut property, &[]).is_err() {
                            return Err(malformed());
                        }
                        path[j].property = property;
                    }
                    b'0' => {
                        let mut index: i32 = 0;
                        if json_unmarshal(&raw_segment.0, &mut index, &[]).is_err() || index < 0 {
                            return Err(malformed());
                        }
                        path[j].index = index;
                        path[j].is_index = true;
                    }
                    _ => return Err(malformed()),
                }
            }
            let option_diagnostic = OptionDiagnostic {
                mapper: entry.mapper.clone(),
                path,
                source: diagnostic_source.clone(),
                code: diagnostic.code,
                message_text: diagnostic.message_text.clone(),
            };
            entry.option_diagnostics.push(option_diagnostic);
        }
        entry.opened = true;
        Ok(())
    }

    // Go: contentmapper/hostimpl.go:730 host.closeProject
    fn close_project(
        &self,
        mapper: &Mapper,
        conn: &Rc<dyn ipc::Conn>,
        project_handle: &str,
    ) -> std::result::Result<(), GoError> {
        let mapper_timing = self.timing.mapper(&mapper.identity());
        let start = mapper_timing.start_request();
        let result = conn.call(
            &self.ctx,
            METHOD_CLOSE_PROJECT,
            Some(Box::new(CloseProjectParams {
                project_handle: project_handle.to_string(),
            })),
        );
        mapper_timing.finish_request(&mapper_timing.close_project, start);
        result.map(|_| ())
    }

    // Go: contentmapper/hostimpl.go:772 host.transformLocked
    fn transform_locked(
        &self,
        mapper: &Rc<Mapper>,
        request: Request,
        project_handle: &str,
    ) -> std::result::Result<Result, GoError> {
        if project_handle.is_empty() {
            return Err(errors::new("content mapper project handle is required"));
        }
        let (conn, position_encoding, diagnostic_source) = match self.conn_for(mapper) {
            Ok(conn) => conn,
            Err(err) => {
                return Err(
                    new_transform_error(TransformErrorKind::INITIALIZE, Some(err)).to_go_error(),
                );
            }
        };
        // PORT: not in Go (see `no_mapper_connection`).
        #[cfg(target_family = "wasm")]
        no_mapper_connection();
        let mapper_timing = self.timing.mapper(&mapper.identity());
        let start = mapper_timing.start_request();
        let raw = conn.call(
            &self.ctx,
            METHOD_TRANSFORM,
            Some(Box::new(TransformParams {
                file_name: request.file_name.clone(),
                content: request.content.clone(),
                project_handle: project_handle.to_string(),
            })),
        );
        mapper_timing.finish_request(&mapper_timing.transform, start);
        let raw = match raw {
            Ok(raw) => raw,
            Err(err) => {
                return Err(
                    new_transform_error(TransformErrorKind::REQUEST, Some(err)).to_go_error()
                );
            }
        };
        match decode_transform_result(
            &raw,
            &request.content,
            &position_encoding,
            &diagnostic_source,
        ) {
            Ok(decoded) => Ok(decoded),
            Err(err) => {
                Err(new_transform_error(TransformErrorKind::RESPONSE, Some(err)).to_go_error())
            }
        }
    }

    // Go: contentmapper/hostimpl.go:827 host.connFor
    // connFor returns the connection for a mapper's identity, spawning its process on first use. Mappers
    // sharing an identity share a single process.
    fn conn_for(
        &self,
        mapper: &Rc<Mapper>,
    ) -> std::result::Result<(Rc<dyn ipc::Conn>, PositionEncoding, String), GoError> {
        self.conn_for_locked(mapper)
    }

    // PORT: not in Go (see `ConcurrentTransform`). What a parse worker can
    // send the transforms of the project of `entry` with, once the project
    // is open. `None` before (Go opens it on the first transform, here the
    // loader's), and when the mapper's connection is not a process
    // connection (a test dialer). It never dials.
    fn concurrent_transform(
        &self,
        mapper: &Rc<Mapper>,
        entry: &Rc<RefCell<ProjectEntry>>,
    ) -> Option<Arc<ConcurrentTransform>> {
        let project_handle = {
            let entry = entry.borrow();
            if !entry.opened || entry.project_handle.is_empty() {
                return None;
            }
            entry.project_handle.clone()
        };
        let identity = mapper.identity();
        let conn = self.conns.borrow().as_ref()?.get(&identity)?.clone();
        let conn = conn.borrow();
        let process = conn
            .conn
            .as_ref()?
            .as_any()?
            .downcast_ref::<ProcessConn>()?
            .clone();
        Some(Arc::new(ConcurrentTransform {
            conn: process,
            ctx: self.ctx.clone(),
            project_handle,
            position_encoding: conn.position_encoding.clone(),
            diagnostic_source: conn.diagnostic_source.clone(),
            timing: self.timing.mapper(&identity),
            disabled: AtomicBool::new(false),
        }))
    }

    // Go: contentmapper/hostimpl.go:833 host.connForLocked
    fn conn_for_locked(
        &self,
        mapper: &Rc<Mapper>,
    ) -> std::result::Result<(Rc<dyn ipc::Conn>, PositionEncoding, String), GoError> {
        let identity = mapper.identity();
        let entry = {
            let mut conns = self.conns.borrow_mut();
            let Some(conns) = conns.as_mut() else {
                return Err(errors::new("content mapper host is closed"));
            };
            conns
                .entry(identity)
                .or_insert_with(|| Rc::new(RefCell::new(MapperConn::default())))
                .clone()
        };
        {
            let entry = entry.borrow();
            if let Some(err) = &entry.err {
                return Err(err.clone());
            }
            if let Some(conn) = &entry.conn {
                return Ok((
                    conn.clone(),
                    entry.position_encoding.clone(),
                    entry.diagnostic_source.clone(),
                ));
            }
        }
        let diagnostic_locale = self.diagnostic_locale.borrow().clone();
        let dialed = (self.dial)(&self.ctx, mapper, &diagnostic_locale);
        let mut entry = entry.borrow_mut();
        match dialed {
            Ok((conn, closer, position_encoding, diagnostic_source)) => {
                lock(&self.processes).push(Arc::downgrade(&closer));
                entry.conn = Some(conn.clone());
                entry.closer = Some(closer);
                entry.err = None;
                entry.position_encoding = position_encoding.clone();
                entry.diagnostic_source = diagnostic_source.clone();
                Ok((conn, position_encoding, diagnostic_source))
            }
            Err(err) => {
                entry.conn = None;
                entry.closer = None;
                entry.err = Some(err.clone());
                entry.position_encoding = PositionEncoding::default();
                entry.diagnostic_source = String::new();
                Err(err)
            }
        }
    }

    // Go: contentmapper/hostimpl.go:1069 host.release
    fn release(&self, identities: &[String]) {
        let mut closers: Vec<Arc<dyn ProcessExitState>> = Vec::new();
        {
            let mut conns = self.conns.borrow_mut();
            if let Some(conns) = conns.as_mut() {
                for identity in identities {
                    let Some(entry) = conns.get(identity).cloned() else {
                        continue;
                    };
                    let mut entry = entry.borrow_mut();
                    entry.refs -= 1;
                    if entry.refs == 0 {
                        conns.shift_remove(identity);
                        if let Some(closer) = &entry.closer {
                            closers.push(closer.clone());
                        }
                    }
                }
            }
        }
        for closer in closers {
            let _ = closer.close();
        }
    }

    /// The project entry for `key`, or `None` (Go reads a nil map or a
    /// missing key as nil).
    fn project_entry(&self, key: &str) -> Option<Rc<RefCell<ProjectEntry>>> {
        self.projects.borrow().as_ref()?.get(key).cloned()
    }

    /// The live connection of `identity`, or `None`.
    fn live_conn(&self, identity: &str) -> Option<Rc<dyn ipc::Conn>> {
        let conns = self.conns.borrow();
        let entry = conns.as_ref()?.get(identity)?.clone();
        let conn = entry.borrow().conn.clone();
        // PORT: not in Go (see `no_mapper_connection`).
        #[cfg(target_family = "wasm")]
        if conn.is_some() {
            no_mapper_connection();
        }
        conn
    }
}

impl Host for HostImpl {
    // Go: contentmapper/hostimpl.go:562 host.Timings
    fn timings(&self) -> Timings {
        self.timing.snapshot()
    }

    // Go: contentmapper/hostimpl.go:566 host.SetLocale
    fn set_locale(&self, diagnostic_locale: Locale) {
        if self.diagnostic_locale.borrow().string() == diagnostic_locale.string() {
            return;
        }
        *self.diagnostic_locale.borrow_mut() = diagnostic_locale;

        let mut closers: Vec<Arc<dyn ProcessExitState>> = Vec::new();
        if let Some(conns) = self.conns.borrow().as_ref() {
            for entry in conns.values() {
                let mut entry = entry.borrow_mut();
                if let Some(closer) = entry.closer.take() {
                    closers.push(closer);
                }
                entry.conn = None;
                entry.err = None;
                entry.position_encoding = PositionEncoding::default();
                entry.diagnostic_source = String::new();
            }
        }
        if let Some(projects) = self.projects.borrow().as_ref() {
            for project in projects.values() {
                project.borrow_mut().opened = false;
            }
        }
        for closer in closers {
            let _ = closer.close();
        }
    }

    // Go: contentmapper/hostimpl.go:595 host.Project
    fn project(&self, spec: ProjectSpec) -> Option<Rc<dyn Project>> {
        let key = project_spec_key(&spec);
        if self.projects.borrow().is_none() {
            return None;
        }
        let existing = self
            .project_leases
            .borrow()
            .as_ref()
            .and_then(|leases| leases.get(&key).cloned());
        if let Some(lease) = existing {
            return Some(lease.retain_locked());
        }
        let mut entries: IndexMap<*const Mapper, String> =
            IndexMap::with_capacity(spec.mappers.len());
        {
            let mut projects = self.projects.borrow_mut();
            let projects = projects.as_mut().expect("checked above");
            let mut conns = self.conns.borrow_mut();
            for mapper in &spec.mappers {
                let entry_key = format!("{}:{}", mapper.identity(), self.next_project_id.get());
                self.next_project_id.set(self.next_project_id.get() + 1);
                let entry = ProjectEntry {
                    mapper: mapper.clone(),
                    spec: spec.clone(),
                    project_handle: entry_key.clone(),
                    opened: false,
                    config_identity: String::new(),
                    watched_files: Vec::new(),
                    option_diagnostics: Vec::new(),
                };
                projects.insert(entry_key.clone(), Rc::new(RefCell::new(entry)));
                // PORT: Go writes to `h.conns`, which is nil only after Close
                // (when `h.projects` is nil too).
                if let Some(conns) = conns.as_mut() {
                    let conn_entry = conns
                        .entry(mapper.identity())
                        .or_insert_with(|| Rc::new(RefCell::new(MapperConn::default())));
                    conn_entry.borrow_mut().refs += 1;
                }
                entries.insert(Rc::as_ptr(mapper), entry_key);
            }
        }
        let lease = Rc::new(ProjectLease {
            host: self.rc(),
            key: key.clone(),
            mappers: spec.mappers.clone(),
            entries,
            refs: Cell::new(1),
            once: Cell::new(false),
        });
        if let Some(leases) = self.project_leases.borrow_mut().as_mut() {
            leases.insert(key, lease.clone());
        }
        Some(lease)
    }

    // Go: contentmapper/hostimpl.go:738 host.Acquire
    fn acquire(&self, mappers: &[Rc<Mapper>]) -> Rc<dyn Fn()> {
        let mut seen: FxHashSet<String> = FxHashSet::default();
        let mut identities: Vec<String> = Vec::with_capacity(mappers.len());
        if let Some(conns) = self.conns.borrow_mut().as_mut() {
            for mapper in mappers {
                let identity = mapper.identity();
                if seen.contains(&identity) {
                    continue;
                }
                seen.insert(identity.clone());
                identities.push(identity.clone());
                let entry = conns
                    .entry(identity)
                    .or_insert_with(|| Rc::new(RefCell::new(MapperConn::default())));
                entry.borrow_mut().refs += 1;
            }
        }
        // Go `sync.OnceFunc(func() { h.release(identities) })`.
        let host = self.rc();
        let released = Cell::new(false);
        Rc::new(move || {
            if released.replace(true) {
                return;
            }
            host.release(&identities);
        })
    }

    // Go: contentmapper/hostimpl.go:763 host.Transform
    // Transform sends the file's content to the mapper's process and decodes the transformed result.
    fn transform(
        &self,
        mapper: &Rc<Mapper>,
        request: Request,
    ) -> std::result::Result<Result, GoError> {
        let project = self.project(ProjectSpec {
            config_file_name: String::new(),
            mappers: vec![mapper.clone()],
            compiler_options: Some(Rc::new(CompilerOptions::default())),
        });
        // Go calls a method on the nil interface that a closed host returns.
        let Some(project) = project else {
            panic!("runtime error: invalid memory address or nil pointer dereference");
        };
        let result = project.transform(mapper, request);
        // Go: defer project.Close()
        let _ = project.close();
        result
    }

    // Go: contentmapper/hostimpl.go:800 host.Close
    // Close shuts down every mapper process. It is safe to call more than once and is invoked automatically
    // when the context passed to New is cancelled.
    fn close(&self) -> std::result::Result<(), GoError> {
        (self.stop)();
        (self.cancel)();
        let mut closers: Vec<Arc<dyn ProcessExitState>> = Vec::new();
        if let Some(conns) = self.conns.borrow_mut().take() {
            for mc in conns.values() {
                if let Some(closer) = &mc.borrow().closer {
                    closers.push(closer.clone());
                }
            }
        }
        *self.projects.borrow_mut() = None;
        // PORT: dropping the leases also ends the `Rc` cycle between the
        // host and its leases.
        *self.project_leases.borrow_mut() = None;
        let mut errs: Vec<GoError> = Vec::new();
        for closer in closers {
            if let Err(err) = closer.close() {
                errs.push(err);
            }
        }
        match errors::join(errs) {
            None => Ok(()),
            Some(err) => Err(err),
        }
    }
}

// Go: contentmapper/hostimpl.go:627 projectSpecKey
// PORT: Go `%p` of a nil pointer prints `0x0`, as Rust `{:p}` of a null pointer.
fn project_spec_key(spec: &ProjectSpec) -> String {
    let compiler_options: *const CompilerOptions = spec
        .compiler_options
        .as_ref()
        .map_or(std::ptr::null(), Rc::as_ptr);
    let mut key = format!("{}\x00{:p}", spec.config_file_name, compiler_options);
    for mapper in &spec.mappers {
        key.push_str(&format!("\x00{:p}", Rc::as_ptr(mapper)));
    }
    key
}

// Go: contentmapper/hostimpl.go:636 combinedIdentity
// PORT: Go `hex.EncodeToString` of the big-endian `Bytes()` of an
// `xxh3.Uint128` is `{:032x}` of the `u128`.
fn combined_identity(
    mapper: &Mapper,
    config_identity: &str,
    compiler_options: Option<&CompilerOptions>,
) -> String {
    let transform_identity = mapper.transform_identity(compiler_options).to_be_bytes();
    let identity = mapper.identity();
    let mut buf: Vec<u8> = Vec::with_capacity(
        identity.len()
            + mapper.definition.options.0.len()
            + config_identity.len()
            + transform_identity.len()
            + 3,
    );
    buf.extend_from_slice(identity.as_bytes());
    buf.push(0);
    buf.extend_from_slice(&mapper.definition.options.0);
    buf.push(0);
    buf.extend_from_slice(config_identity.as_bytes());
    buf.push(0);
    buf.extend_from_slice(&transform_identity);
    let hash = xxhash_rust::xxh3::xxh3_128(&buf);
    format!("{}:{:032x}", identity, hash)
}

/// Go `mapper.Identity() + ":" + hex.EncodeToString(mapper.TransformIdentity(options).Bytes())`.
fn static_identity(mapper: &Mapper, compiler_options: Option<&CompilerOptions>) -> String {
    format!(
        "{}:{:032x}",
        mapper.identity(),
        mapper.transform_identity(compiler_options)
    )
}

// Go: jsontext `Value.Kind`: the kind of the value's first token, with
// every number as '0'; 0 for an empty value.
fn json_value_kind(value: &JsonValue) -> u8 {
    match value.0.trim_ascii_start().first() {
        None => 0,
        Some(b'-' | b'0'..=b'9') => b'0',
        Some(&c) => c,
    }
}

// Go: contentmapper/hostimpl.go:855 projectLease
// PORT: Go `map[*Mapper]string` is an `IndexMap` keyed by the mapper's `Rc`
// pointer, in insertion order. `refs` and `once` are cells (dispatch thread).
struct ProjectLease {
    host: Rc<HostImpl>,
    key: String,
    mappers: Vec<Rc<Mapper>>,
    entries: IndexMap<*const Mapper, String>,
    refs: Cell<i32>,
    once: Cell<bool>,
}

// Go: contentmapper/hostimpl.go:864 retainedProject
// PORT: Go embeds `*projectLease`; every method but Close forwards to it.
struct RetainedProject {
    project_lease: Rc<ProjectLease>,
    once: Cell<bool>,
}

impl ProjectLease {
    // Go: contentmapper/hostimpl.go:869 projectLease.retainLocked
    fn retain_locked(self: &Rc<Self>) -> Rc<dyn Project> {
        self.refs.set(self.refs.get() + 1);
        Rc::new(RetainedProject {
            project_lease: self.clone(),
            once: Cell::new(false),
        })
    }

    /// Go `p.entries[mapper]`, the lookup that also reports `ok`.
    fn entry_key(&self, mapper: &Rc<Mapper>) -> Option<&String> {
        self.entries.get(&Rc::as_ptr(mapper))
    }

    // Go: contentmapper/hostimpl.go:1031 projectLease.release
    fn release(&self) -> std::result::Result<(), GoError> {
        let host = &self.host;
        let mut result: Option<GoError> = None;
        let mut released_identities: Vec<String> = Vec::new();
        self.refs.set(self.refs.get() - 1);
        if self.refs.get() < 0 {
            panic!("content mapper project reference count below zero");
        }
        if self.refs.get() != 0 {
            return Ok(());
        }
        {
            let mut leases = host.project_leases.borrow_mut();
            if let Some(leases) = leases.as_mut() {
                if leases
                    .get(&self.key)
                    .is_some_and(|lease| std::ptr::eq(Rc::as_ptr(lease), self))
                {
                    leases.shift_remove(&self.key);
                }
            }
        }
        for key in self.entries.values() {
            let Some(entry) = host.project_entry(key) else {
                continue;
            };
            let (opened, mapper, project_handle) = {
                let entry = entry.borrow();
                (
                    entry.opened,
                    entry.mapper.clone(),
                    entry.project_handle.clone(),
                )
            };
            if opened {
                if let Some(conn) = host.live_conn(&mapper.identity()) {
                    result = errors::join([
                        result,
                        host.close_project(&mapper, &conn, &project_handle).err(),
                    ]);
                }
            }
            if let Some(projects) = host.projects.borrow_mut().as_mut() {
                projects.shift_remove(key);
            }
            released_identities.push(mapper.identity());
        }
        host.release(&released_identities);
        match result {
            None => Ok(()),
            Some(err) => Err(err),
        }
    }
}

impl Project for ProjectLease {
    // Go: contentmapper/hostimpl.go:879 projectLease.Refresh
    fn refresh(&self) -> std::result::Result<(), GoError> {
        let host = &self.host;
        if host.projects.borrow().is_none() {
            return Ok(());
        }
        let mut result: Option<GoError> = None;
        for key in self.entries.values() {
            let Some(entry) = host.project_entry(key) else {
                continue;
            };
            let (opened, mapper, project_handle) = {
                let entry = entry.borrow();
                (
                    entry.opened,
                    entry.mapper.clone(),
                    entry.project_handle.clone(),
                )
            };
            if !opened {
                continue;
            }
            if let Some(conn) = host.live_conn(&mapper.identity()) {
                result = errors::join([
                    result,
                    host.close_project(&mapper, &conn, &project_handle).err(),
                ]);
            }
            entry.borrow_mut().opened = false;
        }
        match result {
            None => Ok(()),
            Some(err) => Err(err),
        }
    }

    // Go: contentmapper/hostimpl.go:901 projectLease.Identities
    fn identities(&self) -> std::result::Result<Vec<String>, GoError> {
        let host = &self.host;
        if host.projects.borrow().is_none() {
            return Ok(Vec::new());
        }
        let mut identities: Vec<String> = Vec::with_capacity(self.entries.len());
        for mapper in &self.mappers {
            let key = self.entry_key(mapper).cloned().unwrap_or_default();
            let Some(entry) = host.project_entry(&key) else {
                continue;
            };
            let compiler_options = entry.borrow().spec.compiler_options.clone();
            if mapper.manifest.dynamic_config {
                host.open_project_locked(&host.ctx, &entry)?;
                let config_identity = entry.borrow().config_identity.clone();
                identities.push(combined_identity(
                    mapper,
                    &config_identity,
                    compiler_options.as_deref(),
                ));
            } else {
                identities.push(static_identity(mapper, compiler_options.as_deref()));
            }
        }
        Ok(identities)
    }

    // Go: contentmapper/hostimpl.go:929 projectLease.Identity
    fn identity(&self, mapper: &Rc<Mapper>) -> std::result::Result<String, GoError> {
        let host = &self.host;
        if host.projects.borrow().is_none() {
            return Ok(String::new());
        }
        let Some(key) = self.entry_key(mapper) else {
            return Ok(String::new());
        };
        let Some(entry) = host.project_entry(key) else {
            return Ok(String::new());
        };
        let compiler_options = entry.borrow().spec.compiler_options.clone();
        if mapper.manifest.dynamic_config {
            host.open_project_locked(&host.ctx, &entry)?;
            let config_identity = entry.borrow().config_identity.clone();
            return Ok(combined_identity(
                mapper,
                &config_identity,
                compiler_options.as_deref(),
            ));
        }
        Ok(static_identity(mapper, compiler_options.as_deref()))
    }

    // Go: contentmapper/hostimpl.go:955 projectLease.WatchedFiles
    fn watched_files(&self) -> std::result::Result<Vec<String>, GoError> {
        let host = &self.host;
        if host.projects.borrow().is_none() {
            return Ok(Vec::new());
        }
        let mut files: Vec<String> = Vec::new();
        for key in self.entries.values() {
            let Some(entry) = host.project_entry(key) else {
                continue;
            };
            if entry.borrow().mapper.manifest.dynamic_config {
                host.open_project_locked(&host.ctx, &entry)?;
            }
            files.extend(entry.borrow().watched_files.iter().cloned());
        }
        crate::gostd::slices::stable_sort_by(&mut files, Ord::cmp);
        files.dedup();
        Ok(files)
    }

    // Go: contentmapper/hostimpl.go:980 projectLease.Diagnostics
    fn diagnostics(&self) -> Vec<OptionDiagnostic> {
        let host = &self.host;
        if host.projects.borrow().is_none() {
            return Vec::new();
        }
        let mut diagnostics: Vec<OptionDiagnostic> = Vec::new();
        for mapper in &self.mappers {
            let key = self.entry_key(mapper).cloned().unwrap_or_default();
            let Some(entry) = host.project_entry(&key) else {
                continue;
            };
            let entry = entry.borrow();
            if !entry.opened {
                continue;
            }
            diagnostics.extend(entry.option_diagnostics.iter().cloned());
        }
        diagnostics
    }

    // Go: contentmapper/hostimpl.go:1002 projectLease.Transform
    fn transform(
        &self,
        mapper: &Rc<Mapper>,
        request: Request,
    ) -> std::result::Result<Result, GoError> {
        let host = &self.host;
        let key = self.entry_key(mapper).cloned().unwrap_or_default();
        let Some(entry) = host.project_entry(&key) else {
            return Err(errors::new("content mapper project is closed"));
        };
        if let Err(err) = host.open_project_locked(&host.ctx, &entry) {
            if errors::as_type::<InitializeError>(&err).is_some() {
                return Err(
                    new_transform_error(TransformErrorKind::INITIALIZE, Some(err)).to_go_error(),
                );
            }
            return Err(new_transform_error(TransformErrorKind::PROJECT, Some(err)).to_go_error());
        }
        let handle = entry.borrow().project_handle.clone();
        host.transform_locked(mapper, request, &handle)
    }

    // PORT: not in Go (see `ConcurrentTransform`).
    fn concurrent_transform(&self, mapper: &Rc<Mapper>) -> Option<Arc<ConcurrentTransform>> {
        let host = &self.host;
        let entry = host.project_entry(self.entry_key(mapper)?)?;
        host.concurrent_transform(mapper, &entry)
    }

    // Go: contentmapper/hostimpl.go:1023 projectLease.Close
    fn close(&self) -> std::result::Result<(), GoError> {
        if self.once.replace(true) {
            return Ok(());
        }
        self.release()
    }
}

impl Project for RetainedProject {
    fn refresh(&self) -> std::result::Result<(), GoError> {
        self.project_lease.refresh()
    }

    fn identities(&self) -> std::result::Result<Vec<String>, GoError> {
        self.project_lease.identities()
    }

    fn identity(&self, mapper: &Rc<Mapper>) -> std::result::Result<String, GoError> {
        self.project_lease.identity(mapper)
    }

    fn watched_files(&self) -> std::result::Result<Vec<String>, GoError> {
        self.project_lease.watched_files()
    }

    fn diagnostics(&self) -> Vec<OptionDiagnostic> {
        self.project_lease.diagnostics()
    }

    fn transform(
        &self,
        mapper: &Rc<Mapper>,
        request: Request,
    ) -> std::result::Result<Result, GoError> {
        Project::transform(&*self.project_lease, mapper, request)
    }

    fn concurrent_transform(&self, mapper: &Rc<Mapper>) -> Option<Arc<ConcurrentTransform>> {
        Project::concurrent_transform(&*self.project_lease, mapper)
    }

    // Go: contentmapper/hostimpl.go:874 retainedProject.Close
    fn close(&self) -> std::result::Result<(), GoError> {
        if self.once.replace(true) {
            return Ok(());
        }
        self.project_lease.release()
    }
}

// Go: contentmapper/hostimpl.go:1093 handshake
fn handshake(
    ctx: &Context,
    conn: &Rc<dyn ipc::Conn>,
    diagnostic_locale: &Locale,
) -> std::result::Result<(PositionEncoding, String), GoError> {
    let raw = conn.call(
        ctx,
        METHOD_INITIALIZE,
        Some(Box::new(InitializeParams {
            locale: diagnostic_locale.string(),
            position_encodings: vec![PositionEncoding::UTF8, PositionEncoding::UTF16],
        })),
    )?;
    let mut res = InitializeResult::default();
    if let Err(err) = json_unmarshal(&raw.0, &mut res, &[]) {
        return Err(InitializeError {
            kind: InitializeErrorKind::INVALID_RESPONSE,
            detail: err.to_string(),
            ..Default::default()
        }
        .to_go_error());
    }
    if res.position_encoding != PositionEncoding::UTF8
        && res.position_encoding != PositionEncoding::UTF16
    {
        return Err(InitializeError {
            kind: InitializeErrorKind::POSITION_ENCODING,
            position_encoding: res.position_encoding,
            ..Default::default()
        }
        .to_go_error());
    }
    if res.diagnostic_source.trim().is_empty() {
        return Err(InitializeError {
            kind: InitializeErrorKind::EMPTY_DIAGNOSTIC_SOURCE,
            ..Default::default()
        }
        .to_go_error());
    }
    if equate_string_case_insensitive(&res.diagnostic_source, "typescript")
        || equate_string_case_insensitive(&res.diagnostic_source, "tsc")
    {
        return Err(InitializeError {
            kind: InitializeErrorKind::RESERVED_DIAGNOSTIC_SOURCE,
            diagnostic_source: res.diagnostic_source,
            ..Default::default()
        }
        .to_go_error());
    }
    // Go `core.Flatten(tspath.AllSupportedExtensionsWithJson)`.
    let mut native_extensions = tspath::ALL_SUPPORTED_EXTENSIONS_WITH_JSON
        .iter()
        .flat_map(|group| group.iter());
    if native_extensions.any(|extension: &&str| {
        let extension: &str = extension;
        equate_string_case_insensitive(
            &res.diagnostic_source,
            extension.strip_prefix('.').unwrap_or(extension),
        )
    }) {
        return Err(InitializeError {
            kind: InitializeErrorKind::RESERVED_DIAGNOSTIC_SOURCE,
            diagnostic_source: res.diagnostic_source,
            ..Default::default()
        }
        .to_go_error());
    }
    Ok((res.position_encoding, res.diagnostic_source))
}

// Go: contentmapper/hostimpl.go:1123 decodeTransformResult
fn decode_transform_result(
    raw: &JsonValue,
    original_text: &str,
    position_encoding: &PositionEncoding,
    diagnostic_source: &str,
) -> std::result::Result<Result, GoError> {
    let mut res = TransformResult::default();
    json_unmarshal(&raw.0, &mut res, &[]).map_err(errors::from_value)?;
    let (mapped, original_positions) = decode_mapped_output(
        &res.mapped_output,
        original_text,
        position_encoding,
        diagnostic_source,
    )?;
    let mut result = Result {
        text: mapped.text,
        virtual_extension: mapped.virtual_extension,
        mappings: mapped.mappings,
        diagnostic_directives: mapped.diagnostic_directives,
        ..Default::default()
    };
    for (supplemental_index, supplemental) in res.supplemental.iter().enumerate() {
        match decode_mapped_output(
            &supplemental.mapped_output,
            original_text,
            position_encoding,
            diagnostic_source,
        ) {
            Err(err) => {
                if let Some(mut directive_error) = errors::as_type::<DiagnosticDirectiveError>(&err)
                {
                    // PORT: Go sets the index on the error it found; that error
                    // is `err` itself here, so the port returns a copy.
                    directive_error.supplemental_index = supplemental_index as i32;
                    return Err(directive_error.to_go_error());
                }
                return Err(err);
            }
            Ok((mapped, _)) => result.supplemental.push(mapped),
        }
    }
    for d in &res.diagnostics {
        // PORT: Go `int(^uint(0)>>1)` is the largest Go int; the port's
        // positions are `i32`.
        if d.start < 0 || d.length < 0 || d.start > i32::MAX - d.length {
            return Err(errors::new(format!(
                "invalid content mapper diagnostic range [{}, {})",
                d.start,
                d.start.wrapping_add(d.length)
            )));
        }
        let start = original_positions.normalize(d.start).map_err(|err| {
            errors::errorf(
                format!("invalid content mapper diagnostic start: {}", err.error()),
                vec![err],
            )
        })?;
        let end = original_positions
            .normalize(d.start + d.length)
            .map_err(|err| {
                errors::errorf(
                    format!("invalid content mapper diagnostic end: {}", err.error()),
                    vec![err],
                )
            })?;
        result.diagnostics.push(ast::new_external_diagnostic(
            Node::NIL,
            TextRange::new(start, end),
            diagnostic_source,
            crate::diagnostics::Category::Error,
            d.code,
            &d.message_text,
        ));
    }
    Ok(result)
}

// Go: contentmapper/hostimpl.go:1172 decodeMappedOutput
fn decode_mapped_output<'a>(
    output: &'a MappedOutput,
    original_text: &'a str,
    position_encoding: &PositionEncoding,
    diagnostic_source: &str,
) -> std::result::Result<(MappedResult, PositionNormalizer<'a>), GoError> {
    if !is_supported_virtual_extension(&output.extension) {
        return Err(InvalidVirtualExtensionError {
            extension: output.extension.clone(),
        }
        .to_go_error());
    }
    let mut result = MappedResult {
        text: output.text.clone(),
        virtual_extension: output.extension.clone(),
        ..Default::default()
    };
    let virtual_positions = new_position_normalizer(&output.text, position_encoding)?;
    let original_positions = new_position_normalizer(original_text, position_encoding)?;
    // A successful transform always carries a span map. Absent or empty mappings describe fully
    // synthesized output (no segment corresponds to the original), so decode to an empty map rather than
    // nil, which would mean "not content-mapped".
    if !output.mappings.0.is_empty() {
        let mappings = spanmap::unmarshal(&output.mappings.0)?;
        result.mappings = Some(Arc::new(normalize_mappings(
            &mappings,
            &virtual_positions,
            &original_positions,
        )?));
    } else {
        result.mappings = Some(Arc::new(spanmap::new(&[])));
    }
    result.diagnostic_directives = normalize_diagnostic_directives(
        output.diagnostic_directives.as_ref(),
        &virtual_positions,
        &original_positions,
        diagnostic_source,
    )?;
    Ok((result, original_positions))
}

// Go: contentmapper/hostimpl.go:1210 normalizeDiagnosticDirectives
fn normalize_diagnostic_directives(
    diagnostic_directives: Option<&DiagnosticDirectives>,
    virtual_positions: &PositionNormalizer<'_>,
    original_positions: &PositionNormalizer<'_>,
    diagnostic_source: &str,
) -> std::result::Result<Vec<ast::MappedDiagnosticDirective>, GoError> {
    let Some(diagnostic_directives) = diagnostic_directives else {
        return Ok(Vec::new());
    };
    let directives = &diagnostic_directives.directives;
    let unused_diagnostics = &diagnostic_directives.unused_expect_directive_diagnostics;
    let mut result: Vec<ast::MappedDiagnosticDirective> = Vec::with_capacity(directives.len());
    for (i, directive) in directives.iter().enumerate() {
        let directive_error = |kind: DiagnosticDirectiveErrorKind| DiagnosticDirectiveError {
            kind,
            index: i as i32,
            supplemental_index: -1,
            policy: DiagnosticDirectivePolicy::default(),
        };
        let mut normalized = ast::MappedDiagnosticDirective {
            source: diagnostic_source.to_string(),
            ..Default::default()
        };
        match directive.policy {
            DiagnosticDirectivePolicy::IGNORE => {
                normalized.policy = ast::MappedDiagnosticDirectivePolicy::IGNORE;
            }
            DiagnosticDirectivePolicy::EXPECT => {
                let mut unused_diagnostic_index: i32 = 0;
                if let Some(index) = directive.unused_expect_directive_index {
                    unused_diagnostic_index = index;
                } else if unused_diagnostics.len() != 1 {
                    return Err(directive_error(
                        DiagnosticDirectiveErrorKind::EXPECT_MISSING_UNUSED_DIAGNOSTIC,
                    )
                    .to_go_error());
                }
                if unused_diagnostic_index < 0
                    || unused_diagnostic_index as usize >= unused_diagnostics.len()
                {
                    return Err(directive_error(
                        DiagnosticDirectiveErrorKind::INVALID_UNUSED_DIAGNOSTIC_INDEX,
                    )
                    .to_go_error());
                }
                let unused_diagnostic = &unused_diagnostics[unused_diagnostic_index as usize];
                normalized.policy = ast::MappedDiagnosticDirectivePolicy::EXPECT;
                normalized.unused_code = unused_diagnostic.code;
                normalized.unused_message_text = unused_diagnostic.message_text.clone();
            }
            _ => {
                let mut validation_error =
                    directive_error(DiagnosticDirectiveErrorKind::INVALID_POLICY);
                validation_error.policy = directive.policy;
                return Err(validation_error.to_go_error());
            }
        }
        let invalid_range =
            || directive_error(DiagnosticDirectiveErrorKind::INVALID_RANGE).to_go_error();
        if directive.virtual_start < 0 || directive.virtual_end < directive.virtual_start {
            return Err(invalid_range());
        }
        let Ok(virtual_start) = virtual_positions.normalize(directive.virtual_start) else {
            return Err(invalid_range());
        };
        let Ok(virtual_end) = virtual_positions.normalize(directive.virtual_end) else {
            return Err(invalid_range());
        };
        normalized.virtual_range = TextRange::new(virtual_start, virtual_end);
        // PORT: Go compares with the largest Go int; the port's positions are `i32`.
        let mut valid_original_range = directive.original_start >= 0
            && directive.original_length >= 0
            && directive.original_start <= i32::MAX - directive.original_length;
        if valid_original_range {
            let original_start = original_positions.normalize(directive.original_start);
            let original_end =
                original_positions.normalize(directive.original_start + directive.original_length);
            match (original_start, original_end) {
                (Ok(original_start), Ok(original_end)) => {
                    normalized.original_range = TextRange::new(original_start, original_end);
                }
                _ => valid_original_range = false,
            }
        }
        if normalized.policy == ast::MappedDiagnosticDirectivePolicy::EXPECT
            && !valid_original_range
        {
            return Err(invalid_range());
        }
        result.push(normalized);
    }
    // Go `indexedDirective{directive, index}`. The sort reads only the
    // virtual range, so the port sorts (range, index) pairs; Go's pdqsort
    // moves them the same way.
    let mut sorted: Vec<(TextRange, usize)> = result
        .iter()
        .enumerate()
        .map(|(i, directive)| (directive.virtual_range, i))
        .collect();
    sort_func(
        &mut sorted,
        |a: &(TextRange, usize), b: &(TextRange, usize)| match a.0.pos().cmp(&b.0.pos()) {
            std::cmp::Ordering::Less => -1,
            std::cmp::Ordering::Equal => 0,
            std::cmp::Ordering::Greater => 1,
        },
    );
    for i in 1..sorted.len() {
        if sorted[i].0.pos() < sorted[i - 1].0.end() {
            return Err(DiagnosticDirectiveError {
                kind: DiagnosticDirectiveErrorKind::OVERLAP,
                index: sorted[i].1 as i32,
                supplemental_index: -1,
                policy: DiagnosticDirectivePolicy::default(),
            }
            .to_go_error());
        }
    }
    Ok(result)
}

// Go: contentmapper/hostimpl.go:1293 normalizeMappings
fn normalize_mappings(
    mappings: &spanmap::SpanMap,
    virtual_positions: &PositionNormalizer<'_>,
    original_positions: &PositionNormalizer<'_>,
) -> std::result::Result<spanmap::SpanMap, GoError> {
    let wrap =
        |text: String, err: GoError| errors::errorf(format!("{text}: {}", err.error()), vec![err]);
    let mut segments = spanmap::SpanMap::segments(Some(mappings));
    for (i, segment) in segments.iter_mut().enumerate() {
        segment.virtual_start = virtual_positions
            .normalize_text_pos(segment.virtual_start)
            .map_err(|err| {
                wrap(
                    format!("invalid content mapper mapping {i} virtual start"),
                    err,
                )
            })?;
        segment.virtual_end = virtual_positions
            .normalize_text_pos(segment.virtual_end)
            .map_err(|err| {
                wrap(
                    format!("invalid content mapper mapping {i} virtual end"),
                    err,
                )
            })?;
        segment.original_start = original_positions
            .normalize_text_pos(segment.original_start)
            .map_err(|err| {
                wrap(
                    format!("invalid content mapper mapping {i} original start"),
                    err,
                )
            })?;
        segment.original_end = original_positions
            .normalize_text_pos(segment.original_end)
            .map_err(|err| {
                wrap(
                    format!("invalid content mapper mapping {i} original end"),
                    err,
                )
            })?;
    }
    Ok(spanmap::new(&segments))
}

// Go: contentmapper/hostimpl.go:1318 positionNormalizer
struct PositionNormalizer<'a> {
    text: &'a str,
    encoding: PositionEncoding,
    position_map: Option<ast::PositionMap>,
    length: i32,
}

// Go: contentmapper/hostimpl.go:1325 newPositionNormalizer
fn new_position_normalizer<'a>(
    text: &'a str,
    encoding: &PositionEncoding,
) -> std::result::Result<PositionNormalizer<'a>, GoError> {
    let mut normalizer = PositionNormalizer {
        text,
        encoding: encoding.clone(),
        position_map: None,
        length: 0,
    };
    if *encoding == PositionEncoding::UTF8 {
        normalizer.length = text.len() as i32;
    } else if *encoding == PositionEncoding::UTF16 {
        let position_map = ast::compute_position_map(text);
        normalizer.length = position_map.utf8_to_utf16(text.len() as i32);
        normalizer.position_map = Some(position_map);
    } else {
        return Err(errors::new(format!(
            "unsupported position encoding {}",
            gostd::strconv::quote(&encoding.0)
        )));
    }
    Ok(normalizer)
}

impl PositionNormalizer<'_> {
    // Go: contentmapper/hostimpl.go:1339 positionNormalizer.normalizeTextPos
    fn normalize_text_pos(&self, position: i32) -> std::result::Result<i32, GoError> {
        self.normalize(position)
    }

    // Go: contentmapper/hostimpl.go:1344 positionNormalizer.normalize
    fn normalize(&self, position: i32) -> std::result::Result<i32, GoError> {
        if position < 0 {
            return Err(errors::new(format!("position {position} is negative")));
        }
        if position > self.length {
            return Err(errors::new(format!(
                "position {} exceeds {} length {}",
                position, self.encoding.0, self.length
            )));
        }
        let mut byte_position: i32 = 0;
        if self.encoding == PositionEncoding::UTF8 {
            byte_position = position;
        } else if self.encoding == PositionEncoding::UTF16 {
            byte_position = self
                .position_map
                .as_ref()
                .expect("a UTF-16 normalizer has a position map")
                .utf16_to_utf8(position);
        }
        // Go `utf8.RuneStart`: the byte is not a continuation byte.
        if (byte_position as usize) < self.text.len()
            && self.text.as_bytes()[byte_position as usize] & 0xC0 == 0x80
        {
            return Err(errors::new(format!(
                "position {position} splits a Unicode code point"
            )));
        }
        Ok(byte_position)
    }
}

// Go: contentmapper/hostimpl.go:1366 rejectHandler
// rejectHandler rejects any request initiated by the mapper. The content mapper protocol is currently
// parent-driven only; a request from the child is a protocol violation.
struct RejectHandler;

impl ipc::Handler for RejectHandler {
    // Go: contentmapper/hostimpl.go:1368 rejectHandler.HandleRequest
    fn handle_request(
        &self,
        ctx: &Context,
        method: &str,
        params: JsonValue,
    ) -> std::result::Result<Option<Box<dyn AnyValue>>, GoError> {
        Err(errors::new(format!(
            "content mapper sent an unexpected request: {method}"
        )))
    }

    // Go: contentmapper/hostimpl.go:1372 rejectHandler.HandleNotification
    fn handle_notification(
        &self,
        ctx: &Context,
        method: &str,
        params: JsonValue,
    ) -> std::result::Result<(), GoError> {
        Ok(())
    }
}

#[cfg(all(test, unix))]
mod tests {
    // Go: contentmapper/host_test.go
    //
    // PORT: Go serves each fake mapper with `ipc.NewAsyncConn(server, handler).Run`
    // on a goroutine over `net.Pipe`. Here the pipe is a Unix socket pair,
    // and the mapper side runs on its own thread: the handler is `Send`
    // (`MapperHandler`), and the thread makes its own connection around it.
    use super::*;
    use crate::gostd::context::background;
    use crate::ipc::{Protocol as _, ReadWriteCloser as _};
    use std::io::Read;
    use std::net::Shutdown;
    use std::os::unix::net::UnixStream;
    use std::sync::atomic::AtomicI32;
    use std::sync::{Once, mpsc};

    /// One end of Go `net.Pipe`.
    struct PipeEnd {
        stream: UnixStream,
    }

    impl ipc::ReadWriteCloser for PipeEnd {
        fn read(&self, buf: &mut [u8]) -> std::io::Result<usize> {
            (&self.stream).read(buf)
        }

        fn write(&self, buf: &[u8]) -> std::io::Result<usize> {
            (&self.stream).write(buf)
        }

        fn flush(&self) -> std::io::Result<()> {
            (&self.stream).flush()
        }

        // Go `net.Pipe` Close returns nil.
        fn close(&self) -> std::result::Result<(), GoError> {
            let _ = self.stream.shutdown(Shutdown::Both);
            Ok(())
        }
    }

    impl ProcessExitState for PipeEnd {}

    /// Go `net.Pipe()`.
    fn net_pipe() -> (Arc<PipeEnd>, Arc<PipeEnd>) {
        let (client, server) = UnixStream::pair().expect("socket pair");
        (
            Arc::new(PipeEnd { stream: client }),
            Arc::new(PipeEnd { stream: server }),
        )
    }

    /// A fake mapper process: Go `ipc.Handler` values that the tests pass to
    /// `fakeSpawner`.
    trait MapperHandler: Send + Sync {
        fn handle_request(
            &self,
            method: &str,
            params: &JsonValue,
        ) -> std::result::Result<Option<Box<dyn AnyValue>>, GoError>;

        /// Go `interface{ handlesProjects() }`.
        fn handles_projects(&self) -> bool {
            false
        }
    }

    // Go: host_test.go:198 noOpProjectMapper, applied by `fakeSpawner.Spawn`.
    struct HandlerAdapter(Arc<dyn MapperHandler>);

    impl ipc::Handler for HandlerAdapter {
        fn handle_request(
            &self,
            _ctx: &Context,
            method: &str,
            params: JsonValue,
        ) -> std::result::Result<Option<Box<dyn AnyValue>>, GoError> {
            if !self.0.handles_projects() {
                match method {
                    METHOD_OPEN_PROJECT => return Ok(Some(Box::new(OpenProjectResult::default()))),
                    METHOD_CLOSE_PROJECT => return Ok(None),
                    _ => {}
                }
            }
            self.0.handle_request(method, &params)
        }

        fn handle_notification(
            &self,
            _ctx: &Context,
            _method: &str,
            _params: JsonValue,
        ) -> std::result::Result<(), GoError> {
            Ok(())
        }
    }

    fn unmarshal_params<T: UnmarshalerFrom + Default>(
        params: &JsonValue,
    ) -> std::result::Result<T, GoError> {
        let mut p = T::default();
        json_unmarshal(&params.0, &mut p, &[]).map_err(errors::from_value)?;
        Ok(p)
    }

    fn unexpected_method(method: &str) -> GoError {
        errors::new(format!("unexpected method {method}"))
    }

    fn initialize_result(source: &str) -> Box<dyn AnyValue> {
        Box::new(InitializeResult {
            position_encoding: PositionEncoding::UTF8,
            diagnostic_source: source.to_string(),
        })
    }

    // Go: host_test.go:28 fakeMapper
    // fakeMapper is an in-process mapper that transforms content verbatim and reports one diagnostic.
    struct FakeMapper;

    impl MapperHandler for FakeMapper {
        // Go: host_test.go:53 fakeMapper.HandleRequest
        fn handle_request(
            &self,
            method: &str,
            params: &JsonValue,
        ) -> std::result::Result<Option<Box<dyn AnyValue>>, GoError> {
            match method {
                METHOD_INITIALIZE => Ok(Some(initialize_result("vue"))),
                METHOD_TRANSFORM => {
                    let p: TransformParams = unmarshal_params(params)?;
                    let length = p.content.len() as i32;
                    let mappings = spanmap::new(&[spanmap::Segment {
                        virtual_end: length,
                        original_end: length,
                        kind: spanmap::Kind::VERBATIM,
                        ..Default::default()
                    }])
                    .marshal()?;
                    Ok(Some(Box::new(TransformResult {
                        mapped_output: MappedOutput {
                            text: p.content.clone(),
                            extension: ".ts".to_string(),
                            mappings: JsonValue(mappings),
                            ..Default::default()
                        },
                        diagnostics: vec![Diagnostic {
                            message_text: "boom".to_string(),
                            start: 0,
                            length: 3.min(length),
                            code: 9999,
                        }],
                        ..Default::default()
                    })))
                }
                _ => Err(unexpected_method(method)),
            }
        }
    }

    type ResponseFn = dyn Fn(TransformParams) -> Box<dyn AnyValue> + Send + Sync;

    // Go: host_test.go:30 responseMapper
    struct ResponseMapper {
        response: Box<ResponseFn>,
    }

    impl MapperHandler for ResponseMapper {
        // Go: host_test.go:34 responseMapper.HandleRequest
        fn handle_request(
            &self,
            method: &str,
            params: &JsonValue,
        ) -> std::result::Result<Option<Box<dyn AnyValue>>, GoError> {
            match method {
                METHOD_INITIALIZE => Ok(Some(initialize_result("mapper"))),
                METHOD_TRANSFORM => {
                    let p: TransformParams = unmarshal_params(params)?;
                    Ok(Some((self.response)(p)))
                }
                _ => Err(unexpected_method(method)),
            }
        }
    }

    fn response_mapper(
        response: impl Fn(TransformParams) -> Box<dyn AnyValue> + Send + Sync + 'static,
    ) -> Arc<dyn MapperHandler> {
        Arc::new(ResponseMapper {
            response: Box::new(response),
        })
    }

    // Go: host_test.go:84 unicodeMapper
    struct UnicodeMapper {
        encoding: PositionEncoding,
        source: Option<String>,
    }

    // Go: host_test.go:89 protocolDiagnosticDirectives
    fn protocol_diagnostic_directives(
        directives: Vec<MappedDiagnosticDirective>,
        unused: Vec<UnusedExpectDirectiveDiagnostic>,
    ) -> Option<DiagnosticDirectives> {
        Some(DiagnosticDirectives {
            unused_expect_directive_diagnostics: unused,
            directives,
        })
    }

    impl MapperHandler for UnicodeMapper {
        // Go: host_test.go:96 unicodeMapper.HandleRequest
        fn handle_request(
            &self,
            method: &str,
            params: &JsonValue,
        ) -> std::result::Result<Option<Box<dyn AnyValue>>, GoError> {
            match method {
                METHOD_INITIALIZE => {
                    let p: InitializeParams = unmarshal_params(params)?;
                    let offered = p.position_encodings.contains(&self.encoding);
                    if !offered
                        && (self.encoding == PositionEncoding::UTF8
                            || self.encoding == PositionEncoding::UTF16)
                    {
                        return Err(errors::new(format!(
                            "position encoding {} was not offered",
                            gostd::strconv::quote(&self.encoding.0)
                        )));
                    }
                    let source = self.source.clone().unwrap_or_else(|| "mapper".to_string());
                    Ok(Some(Box::new(InitializeResult {
                        position_encoding: self.encoding.clone(),
                        diagnostic_source: source,
                    })))
                }
                METHOD_TRANSFORM => {
                    let p: TransformParams = unmarshal_params(params)?;
                    let (emoji_length, text_length) = if self.encoding == PositionEncoding::UTF8 {
                        (2, 3)
                    } else if self.encoding == PositionEncoding::UTF16 {
                        (1, 2)
                    } else {
                        return Ok(Some(Box::new(TransformResult {
                            mapped_output: MappedOutput {
                                text: p.content,
                                extension: ".ts".to_string(),
                                ..Default::default()
                            },
                            ..Default::default()
                        })));
                    };
                    let verbatim = spanmap::Kind::VERBATIM.0;
                    let mappings = json_marshal(
                        &vec![
                            vec![0, emoji_length, 0, emoji_length, verbatim],
                            vec![
                                emoji_length,
                                text_length - emoji_length,
                                emoji_length,
                                text_length - emoji_length,
                                verbatim,
                            ],
                        ],
                        &[],
                    )
                    .map_err(errors::from_value)?;
                    Ok(Some(Box::new(TransformResult {
                        mapped_output: MappedOutput {
                            text: p.content,
                            extension: ".ts".to_string(),
                            mappings: JsonValue(mappings.into_bytes()),
                            diagnostic_directives: protocol_diagnostic_directives(
                                vec![MappedDiagnosticDirective {
                                    original_start: emoji_length,
                                    original_length: text_length - emoji_length,
                                    virtual_start: emoji_length,
                                    virtual_end: text_length,
                                    policy: DiagnosticDirectivePolicy::IGNORE,
                                    unused_expect_directive_index: None,
                                }],
                                Vec::new(),
                            ),
                        },
                        diagnostics: vec![Diagnostic {
                            message_text: "after non-ASCII character".to_string(),
                            start: emoji_length,
                            length: text_length - emoji_length,
                            code: 1001,
                        }],
                        ..Default::default()
                    })))
                }
                _ => Err(unexpected_method(method)),
            }
        }
    }

    // Go: host_test.go:160 invalidDiagnosticMapper
    struct InvalidDiagnosticMapper {
        encoding: PositionEncoding,
    }

    impl MapperHandler for InvalidDiagnosticMapper {
        // Go: host_test.go:164 invalidDiagnosticMapper.HandleRequest
        fn handle_request(
            &self,
            method: &str,
            _params: &JsonValue,
        ) -> std::result::Result<Option<Box<dyn AnyValue>>, GoError> {
            match method {
                METHOD_INITIALIZE => Ok(Some(Box::new(InitializeResult {
                    position_encoding: self.encoding.clone(),
                    diagnostic_source: "mapper".to_string(),
                }))),
                METHOD_TRANSFORM => Ok(Some(Box::new(TransformResult {
                    mapped_output: MappedOutput {
                        extension: ".ts".to_string(),
                        ..Default::default()
                    },
                    diagnostics: vec![Diagnostic {
                        message_text: "invalid boundary".to_string(),
                        start: 1,
                        code: 1002,
                        ..Default::default()
                    }],
                    ..Default::default()
                }))),
                _ => Err(unexpected_method(method)),
            }
        }
    }

    // Go: host_test.go:192 fakeSpawner
    // fakeSpawner serves each spawn request with an in-process mapper over a net.Pipe, counting spawns so
    // tests can assert process consolidation. When handler is nil it serves a fakeMapper.
    #[derive(Default)]
    struct FakeSpawner {
        spawns: AtomicI32,
        closes: Arc<AtomicI32>,
        handler: Option<Arc<dyn MapperHandler>>,
    }

    fn fake_spawner(handler: Arc<dyn MapperHandler>) -> Rc<FakeSpawner> {
        Rc::new(FakeSpawner {
            handler: Some(handler),
            ..Default::default()
        })
    }

    impl Spawner for FakeSpawner {
        // Go: host_test.go:211 fakeSpawner.Spawn
        fn spawn(
            &self,
            _command: &[String],
            _dir: &str,
            _stderr: Option<Box<dyn Write + Send>>,
        ) -> std::result::Result<Arc<dyn ProcessExitState>, GoError> {
            self.spawns.fetch_add(1, Ordering::SeqCst);
            let handler: Arc<dyn MapperHandler> =
                self.handler.clone().unwrap_or_else(|| Arc::new(FakeMapper));
            let (client, server) = net_pipe();
            std::thread::spawn(move || {
                let server: Arc<dyn ipc::ReadWriteCloser> = server;
                let conn = ipc::new_async_conn(server, Rc::new(HandlerAdapter(handler)));
                let _ = conn.run(&background());
            });
            Ok(Arc::new(CountingReadWriteCloser {
                inner: client,
                closes: self.closes.clone(),
                once: Once::new(),
            }))
        }
    }

    // Go: host_test.go:225 countingReadWriteCloser
    struct CountingReadWriteCloser {
        inner: Arc<PipeEnd>,
        closes: Arc<AtomicI32>,
        once: Once,
    }

    impl ipc::ReadWriteCloser for CountingReadWriteCloser {
        fn read(&self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.inner.read(buf)
        }

        fn write(&self, buf: &[u8]) -> std::io::Result<usize> {
            self.inner.write(buf)
        }

        fn flush(&self) -> std::io::Result<()> {
            self.inner.flush()
        }

        // Go: host_test.go:231 countingReadWriteCloser.Close
        fn close(&self) -> std::result::Result<(), GoError> {
            self.once.call_once(|| {
                self.closes.fetch_add(1, Ordering::SeqCst);
            });
            self.inner.close()
        }
    }

    impl ProcessExitState for CountingReadWriteCloser {}

    fn test_ctx() -> Context {
        background()
    }

    fn mapper(package: &str, name: &str, version: &str, exec: &str) -> Rc<Mapper> {
        Rc::new(Mapper {
            definition: Definition {
                package: package.to_string(),
                ..Default::default()
            },
            manifest: Manifest {
                name: name.to_string(),
                version: version.to_string(),
                exec: vec![exec.to_string()],
                ..Default::default()
            },
            ..Default::default()
        })
    }

    fn vue_mapper_with_extension() -> Rc<Mapper> {
        Rc::new(Mapper {
            definition: Definition {
                extensions: vec![".vue".to_string()],
                ..Default::default()
            },
            manifest: Manifest {
                name: "mapper".to_string(),
                exec: vec!["mapper".to_string()],
                ..Default::default()
            },
            ..Default::default()
        })
    }

    fn request(file_name: &str, content: &str) -> Request {
        Request {
            file_name: file_name.to_string(),
            content: content.to_string(),
        }
    }

    fn default_options() -> Option<Rc<CompilerOptions>> {
        Some(Rc::new(CompilerOptions::default()))
    }

    fn assert_error_contains<T>(result: std::result::Result<T, GoError>, text: &str) {
        match result {
            Ok(_) => panic!("expected an error containing {text:?}"),
            Err(err) => assert!(
                err.error().contains(text),
                "error {:?} does not contain {text:?}",
                err.error()
            ),
        }
    }

    // Go: host_test.go:236 TestRunnerTransform
    #[test]
    fn test_runner_transform() {
        let r = new_host(
            &test_ctx(),
            Rc::new(FakeSpawner::default()),
            locale::DEFAULT,
        );

        let mapper = Rc::new(Mapper {
            definition: Definition {
                extensions: vec![".vue".to_string()],
                ..Default::default()
            },
            manifest: Manifest {
                name: "vue".to_string(),
                version: "1.0.0".to_string(),
                exec: vec!["vue-mapper".to_string()],
                ..Default::default()
            },
            ..Default::default()
        });
        let result = r
            .transform(&mapper, request("/a.vue", "export const x = 1;"))
            .expect("transform");
        assert_eq!(result.text, "export const x = 1;");
        assert_eq!(result.virtual_extension, ".ts");
        assert!(result.mappings.is_some());
        assert_eq!(result.diagnostics.len(), 1);
        assert_eq!(result.diagnostics[0].code, 9999);
        assert_eq!(result.diagnostics[0].source(), "vue");
        let _ = r.close();
    }

    // Go: host_test.go:252 TestHostLogging
    #[test]
    fn test_host_logging() {
        let logs: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let logger: Logger = {
            let logs = logs.clone();
            Arc::new(move |message: &str| lock(&logs).push(message.to_string()))
        };
        let spawner = SpawnerFunc(Box::new(
            |command: &[String], dir: &str, mut stderr: Option<Box<dyn Write + Send>>| {
                if let Some(stderr) = stderr.as_mut() {
                    let _ = stderr.write_all(b"mapper diagnostic\n");
                }
                FakeSpawner::default().spawn(command, dir, stderr)
            },
        ));
        let host = new_host_with_options(
            &test_ctx(),
            Rc::new(spawner),
            locale::DEFAULT,
            HostOptions {
                logger: Some(logger),
            },
        );
        let mapper = mapper("configured", "resolved", "1.0.0", "mapper");
        host.transform(&mapper, request("/a.vue", "export const x = 1;"))
            .expect("transform");

        let joined = lock(&logs).join("\n");
        assert!(joined.contains(
            r#"[content mapper: resolved] send: {"jsonrpc":"2.0","id":"api1","method":"initialize""#
        ));
        assert!(joined.contains(
            r#"[content mapper: resolved] receive: {"jsonrpc":"2.0","id":"api1","result":"#
        ));
        assert!(joined.contains("[content mapper: resolved] stderr: mapper diagnostic"));
        let _ = host.close();
    }

    // Go: host_test.go:282 TestHostDiscardsStderrWithoutLogging
    // PORT: Go compares `stderr` with `io.Discard`; the port passes `None`.
    #[test]
    fn test_host_discards_stderr_without_logging() {
        let spawner = SpawnerFunc(Box::new(
            |command: &[String], dir: &str, stderr: Option<Box<dyn Write + Send>>| {
                assert!(stderr.is_none());
                FakeSpawner::default().spawn(command, dir, stderr)
            },
        ));
        let host = new_host(&test_ctx(), Rc::new(spawner), locale::DEFAULT);
        let mapper = mapper("configured", "resolved", "1.0.0", "mapper");
        host.transform(&mapper, request("/a.vue", "export const x = 1;"))
            .expect("transform");
        let _ = host.close();
    }

    // Go: host_test.go:298 TestMapperDiagnosticName
    #[test]
    fn test_mapper_diagnostic_name() {
        let tests: [(Mapper, &str); 3] = [
            (
                Mapper {
                    definition: Definition {
                        package: "configured".to_string(),
                        ..Default::default()
                    },
                    manifest: Manifest {
                        name: "resolved".to_string(),
                        ..Default::default()
                    },
                    contribution_id: "contributed".to_string(),
                    ..Default::default()
                },
                "resolved",
            ),
            (
                Mapper {
                    definition: Definition {
                        package: "configured".to_string(),
                        ..Default::default()
                    },
                    contribution_id: "contributed".to_string(),
                    ..Default::default()
                },
                "configured",
            ),
            (
                Mapper {
                    contribution_id: "contributed".to_string(),
                    ..Default::default()
                },
                "contributed",
            ),
        ];
        for (mapper, want) in tests {
            assert_eq!(mapper.diagnostic_name(), want);
        }
    }

    // Go: host_test.go:313 TestRunnerTransformResponseValidation
    #[test]
    fn test_runner_transform_response_validation() {
        let mapper = vue_mapper_with_extension();

        // malformed result fails the request
        let host = new_host(
            &test_ctx(),
            fake_spawner(response_mapper(|_p| {
                Box::new(JsonValue(br#"{"text":1}"#.to_vec()))
            })),
            locale::DEFAULT,
        );
        let result = host.transform(&mapper, request("/a.vue", "a"));
        assert!(result.is_err());
        let _ = host.close();
    }

    // Go: host_test.go:401 exitedReadWriteCloser
    struct ExitedReadWriteCloser {
        inner: Arc<PipeEnd>,
        exit_code: i32,
    }

    impl ipc::ReadWriteCloser for ExitedReadWriteCloser {
        fn read(&self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.inner.read(buf)
        }

        fn write(&self, buf: &[u8]) -> std::io::Result<usize> {
            self.inner.write(buf)
        }

        fn flush(&self) -> std::io::Result<()> {
            self.inner.flush()
        }

        fn close(&self) -> std::result::Result<(), GoError> {
            self.inner.close()
        }
    }

    impl ProcessExitState for ExitedReadWriteCloser {
        // Go: host_test.go:406 exitedReadWriteCloser.ExitCode
        fn exit_code(&self) -> (i32, bool) {
            (self.exit_code, true)
        }
    }

    // Go: host_test.go:410 exitOnCloseReadWriteCloser
    struct ExitOnCloseReadWriteCloser {
        inner: Arc<PipeEnd>,
        exited: AtomicBool,
    }

    impl ipc::ReadWriteCloser for ExitOnCloseReadWriteCloser {
        fn read(&self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.inner.read(buf)
        }

        fn write(&self, buf: &[u8]) -> std::io::Result<usize> {
            self.inner.write(buf)
        }

        fn flush(&self) -> std::io::Result<()> {
            self.inner.flush()
        }

        // Go: host_test.go:415 exitOnCloseReadWriteCloser.Close
        fn close(&self) -> std::result::Result<(), GoError> {
            self.exited.store(true, Ordering::SeqCst);
            self.inner.close()
        }
    }

    impl ProcessExitState for ExitOnCloseReadWriteCloser {
        // Go: host_test.go:420 exitOnCloseReadWriteCloser.ExitCode
        fn exit_code(&self) -> (i32, bool) {
            (1, self.exited.load(Ordering::SeqCst))
        }
    }

    // Go: host_test.go:424 closeSignalReadWriteCloser
    struct CloseSignalReadWriteCloser {
        inner: Arc<PipeEnd>,
        closed: mpsc::SyncSender<()>,
        once: Once,
    }

    impl ipc::ReadWriteCloser for CloseSignalReadWriteCloser {
        fn read(&self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.inner.read(buf)
        }

        fn write(&self, buf: &[u8]) -> std::io::Result<usize> {
            self.inner.write(buf)
        }

        fn flush(&self) -> std::io::Result<()> {
            self.inner.flush()
        }

        // Go: host_test.go:430 closeSignalReadWriteCloser.Close
        fn close(&self) -> std::result::Result<(), GoError> {
            let err = self.inner.close();
            self.once.call_once(|| {
                let _ = self.closed.try_send(());
            });
            err
        }
    }

    impl ProcessExitState for CloseSignalReadWriteCloser {}

    // Go: host_test.go:329 TestHostClosesProcessWhenReadLoopFails
    #[test]
    fn test_host_closes_process_when_read_loop_fails() {
        let (closed_tx, closed_rx) = mpsc::sync_channel::<()>(1);
        let spawner = SpawnerFunc(Box::new(
            move |_command: &[String], _dir: &str, _stderr: Option<Box<dyn Write + Send>>| {
                let (client, server) = net_pipe();
                std::thread::spawn(move || {
                    let transport: Arc<dyn ipc::ReadWriteCloser> = server.clone();
                    let mut protocol = ipc::new_jsonrpc_protocol(transport);
                    let message = protocol.read_message().expect("read initialize");
                    assert_eq!(message.method, METHOD_INITIALIZE);
                    protocol
                        .write_response(message.id.as_ref(), Some(initialize_result("mapper")))
                        .expect("write initialize response");
                    server.write(b"oops\n").expect("write garbage");
                    let _ = server.close();
                });
                let process: Arc<dyn ProcessExitState> = Arc::new(CloseSignalReadWriteCloser {
                    inner: client,
                    closed: closed_tx.clone(),
                    once: Once::new(),
                });
                Ok(process)
            },
        ));
        let host = new_host(&test_ctx(), Rc::new(spawner), locale::DEFAULT);
        let mapper = mapper("", "mapper", "", "mapper");
        let result = host.transform(&mapper, request("/a.vue", ""));
        assert!(result.is_err());
        let process_closed = closed_rx.recv_timeout(Duration::from_secs(1)).is_ok();
        assert!(
            process_closed,
            "mapper process was not closed after its read loop failed"
        );
        let _ = host.close();
    }

    // Go: host_test.go:364 TestHostReportsInitializationTimeoutBeforeClosingProcess
    // PORT: Go's mapper waits for the test context. This one reads until the
    // host closes its end, which it does at the initialize deadline.
    #[test]
    fn test_host_reports_initialization_timeout_before_closing_process() {
        let spawner = SpawnerFunc(Box::new(
            |_command: &[String], _dir: &str, _stderr: Option<Box<dyn Write + Send>>| {
                let (client, server) = net_pipe();
                std::thread::spawn(move || {
                    let transport: Arc<dyn ipc::ReadWriteCloser> = server.clone();
                    let _ = ipc::new_jsonrpc_protocol(transport).read_message();
                    let mut buf = [0u8; 256];
                    while matches!(server.read(&mut buf), Ok(n) if n > 0) {}
                    let _ = server.close();
                });
                let process: Arc<dyn ProcessExitState> = Arc::new(ExitOnCloseReadWriteCloser {
                    inner: client,
                    exited: AtomicBool::new(false),
                });
                Ok(process)
            },
        ));
        let host = new_host(&test_ctx(), Rc::new(spawner), locale::DEFAULT);
        let mapper = mapper("", "mapper", "", "mapper");
        let err = host
            .transform(&mapper, request("/a.vue", ""))
            .err()
            .expect("transform error");
        let initialize_error = errors::as_type::<InitializeError>(&err);
        assert!(
            initialize_error.is_some(),
            "expected InitializeError, got {err:?}"
        );
        assert_eq!(
            initialize_error.unwrap().kind,
            InitializeErrorKind::NO_RESPONSE
        );
        let _ = host.close();
    }

    // Go: host_test.go:384 TestHostReportsProcessExitBeforeInitialization
    #[test]
    fn test_host_reports_process_exit_before_initialization() {
        let spawner = SpawnerFunc(Box::new(
            |_command: &[String], _dir: &str, _stderr: Option<Box<dyn Write + Send>>| {
                let (client, server) = net_pipe();
                server.close().expect("close server");
                let process: Arc<dyn ProcessExitState> = Arc::new(ExitedReadWriteCloser {
                    inner: client,
                    exit_code: 42,
                });
                Ok(process)
            },
        ));
        let host = new_host(&test_ctx(), Rc::new(spawner), locale::DEFAULT);
        let mapper = mapper("", "mapper", "", "mapper");
        let err = host
            .transform(&mapper, request("/a.vue", ""))
            .err()
            .expect("transform error");
        let initialize_error = errors::as_type::<InitializeError>(&err)
            .unwrap_or_else(|| panic!("expected InitializeError, got {err:?}"));
        assert_eq!(initialize_error.kind, InitializeErrorKind::PROCESS_EXIT);
        assert_eq!(initialize_error.exit_code, 42);
        let _ = host.close();
    }

    // Go: host_test.go:436 TestRunnerTransformDiagnosticDirectives
    #[test]
    fn test_runner_transform_diagnostic_directives() {
        let mapper = vue_mapper_with_extension();
        let transform = |output: MappedOutput| -> std::result::Result<Result, GoError> {
            let host = new_host(
                &test_ctx(),
                fake_spawner(response_mapper(move |_p| {
                    Box::new(TransformResult {
                        mapped_output: output.clone(),
                        ..Default::default()
                    })
                })),
                locale::DEFAULT,
            );
            let result = host.transform(&mapper, request("/a.vue", "directive\nsource"));
            let _ = host.close();
            result
        };

        let result = transform(MappedOutput {
            text: "virtual source".to_string(),
            extension: ".ts".to_string(),
            diagnostic_directives: protocol_diagnostic_directives(
                vec![MappedDiagnosticDirective {
                    original_start: 0,
                    original_length: 9,
                    virtual_start: 8,
                    virtual_end: 14,
                    policy: DiagnosticDirectivePolicy::EXPECT,
                    unused_expect_directive_index: None,
                }],
                vec![UnusedExpectDirectiveDiagnostic {
                    code: 2578,
                    message_text: "Unused framework directive.".to_string(),
                }],
            ),
            ..Default::default()
        })
        .expect("transform");
        assert_eq!(result.diagnostic_directives.len(), 1);
        let directive = &result.diagnostic_directives[0];
        assert_eq!(directive.original_range.pos(), 0);
        assert_eq!(directive.original_range.end(), 9);
        assert_eq!(directive.virtual_range.pos(), 8);
        assert_eq!(directive.virtual_range.end(), 14);
        assert_eq!(
            directive.policy,
            ast::MappedDiagnosticDirectivePolicy::EXPECT
        );
        assert_eq!(directive.unused_code, 2578);
        assert_eq!(directive.unused_message_text, "Unused framework directive.");
        assert_eq!(directive.source, "mapper");
        let result = transform(MappedOutput {
            text: "virtual source".to_string(),
            extension: ".ts".to_string(),
            diagnostic_directives: protocol_diagnostic_directives(
                vec![MappedDiagnosticDirective {
                    original_length: 9,
                    virtual_start: 8,
                    virtual_end: 14,
                    policy: DiagnosticDirectivePolicy::EXPECT,
                    unused_expect_directive_index: Some(1),
                    ..Default::default()
                }],
                vec![
                    UnusedExpectDirectiveDiagnostic {
                        code: 1,
                        message_text: "first".to_string(),
                    },
                    UnusedExpectDirectiveDiagnostic {
                        code: 2,
                        message_text: "second".to_string(),
                    },
                ],
            ),
            ..Default::default()
        })
        .expect("transform");
        assert_eq!(result.diagnostic_directives[0].unused_code, 2);
        assert_eq!(
            result.diagnostic_directives[0].unused_message_text,
            "second"
        );
        transform(MappedOutput {
            text: "x".to_string(),
            extension: ".ts".to_string(),
            diagnostic_directives: protocol_diagnostic_directives(
                vec![MappedDiagnosticDirective {
                    original_start: -1,
                    policy: DiagnosticDirectivePolicy::IGNORE,
                    ..Default::default()
                }],
                vec![UnusedExpectDirectiveDiagnostic::default()],
            ),
            ..Default::default()
        })
        .expect("transform");

        let invalid: Vec<(
            &str,
            &str,
            Vec<MappedDiagnosticDirective>,
            DiagnosticDirectiveErrorKind,
        )> = vec![
            (
                "invalid range",
                "x",
                vec![MappedDiagnosticDirective {
                    virtual_start: -1,
                    policy: DiagnosticDirectivePolicy::IGNORE,
                    ..Default::default()
                }],
                DiagnosticDirectiveErrorKind::INVALID_RANGE,
            ),
            (
                "unknown policy",
                "x",
                vec![MappedDiagnosticDirective {
                    policy: DiagnosticDirectivePolicy(2),
                    ..Default::default()
                }],
                DiagnosticDirectiveErrorKind::INVALID_POLICY,
            ),
            (
                "expect requires unused diagnostic",
                "x",
                vec![MappedDiagnosticDirective {
                    policy: DiagnosticDirectivePolicy::EXPECT,
                    ..Default::default()
                }],
                DiagnosticDirectiveErrorKind::EXPECT_MISSING_UNUSED_DIAGNOSTIC,
            ),
            (
                "multiple unused diagnostics require index",
                "x",
                vec![MappedDiagnosticDirective {
                    policy: DiagnosticDirectivePolicy::EXPECT,
                    ..Default::default()
                }],
                DiagnosticDirectiveErrorKind::EXPECT_MISSING_UNUSED_DIAGNOSTIC,
            ),
            (
                "original range out of bounds",
                "x",
                vec![MappedDiagnosticDirective {
                    original_start: 99,
                    policy: DiagnosticDirectivePolicy::EXPECT,
                    ..Default::default()
                }],
                DiagnosticDirectiveErrorKind::INVALID_RANGE,
            ),
            (
                "virtual range out of bounds",
                "x",
                vec![MappedDiagnosticDirective {
                    virtual_start: 99,
                    policy: DiagnosticDirectivePolicy::IGNORE,
                    ..Default::default()
                }],
                DiagnosticDirectiveErrorKind::INVALID_RANGE,
            ),
            (
                "overlap",
                "abc",
                vec![
                    MappedDiagnosticDirective {
                        virtual_end: 2,
                        policy: DiagnosticDirectivePolicy::IGNORE,
                        ..Default::default()
                    },
                    MappedDiagnosticDirective {
                        virtual_start: 1,
                        virtual_end: 3,
                        policy: DiagnosticDirectivePolicy::IGNORE,
                        ..Default::default()
                    },
                ],
                DiagnosticDirectiveErrorKind::OVERLAP,
            ),
        ];
        for (name, text, directives, kind) in invalid {
            let mut diagnostic_directives =
                protocol_diagnostic_directives(directives, Vec::new()).expect("directives");
            if name == "original range out of bounds" {
                diagnostic_directives.unused_expect_directive_diagnostics =
                    vec![UnusedExpectDirectiveDiagnostic::default()];
            } else if name == "multiple unused diagnostics require index" {
                diagnostic_directives.unused_expect_directive_diagnostics = vec![
                    UnusedExpectDirectiveDiagnostic::default(),
                    UnusedExpectDirectiveDiagnostic::default(),
                ];
            }
            let err = transform(MappedOutput {
                text: text.to_string(),
                extension: ".ts".to_string(),
                diagnostic_directives: Some(diagnostic_directives),
                ..Default::default()
            })
            .err()
            .unwrap_or_else(|| panic!("{name}: expected an error"));
            let directive_error = errors::as_type::<DiagnosticDirectiveError>(&err)
                .unwrap_or_else(|| panic!("{name}: expected DiagnosticDirectiveError"));
            assert_eq!(directive_error.kind, kind, "{name}");
        }
    }

    // Go: host_test.go:579 TestMappedDiagnosticDirectiveJSON
    #[test]
    fn test_mapped_diagnostic_directive_json() {
        let tests: [(&str, MappedDiagnosticDirective, &str); 2] = [
            (
                "ignore",
                MappedDiagnosticDirective {
                    virtual_start: 8,
                    virtual_end: 14,
                    original_start: 0,
                    original_length: 9,
                    policy: DiagnosticDirectivePolicy::IGNORE,
                    unused_expect_directive_index: None,
                },
                "[0,9,8,14,0]",
            ),
            (
                "expect",
                MappedDiagnosticDirective {
                    virtual_start: 8,
                    virtual_end: 14,
                    original_start: 0,
                    original_length: 9,
                    policy: DiagnosticDirectivePolicy::EXPECT,
                    unused_expect_directive_index: None,
                },
                "[0,9,8,14,1]",
            ),
        ];
        for (name, directive, want) in tests {
            let data = json_marshal(&directive, &[]).expect("marshal");
            assert_eq!(data, want, "{name}");
            let mut decoded = MappedDiagnosticDirective::default();
            json_unmarshal(data.as_bytes(), &mut decoded, &[]).expect("unmarshal");
            assert_eq!(decoded, directive, "{name}");
        }

        for data in ["[0,0,0,0]", "[0,0,0,0,0,1,2]"] {
            let mut directive = MappedDiagnosticDirective::default();
            let err =
                json_unmarshal(data.as_bytes(), &mut directive, &[]).expect_err("want an error");
            assert!(
                err.to_string().contains("diagnostic directive tuple"),
                "{err}"
            );
        }

        let diagnostic_directives = DiagnosticDirectives {
            unused_expect_directive_diagnostics: vec![
                UnusedExpectDirectiveDiagnostic {
                    code: 1,
                    message_text: "first".to_string(),
                },
                UnusedExpectDirectiveDiagnostic {
                    code: 2,
                    message_text: "second".to_string(),
                },
            ],
            directives: vec![MappedDiagnosticDirective {
                original_start: 2,
                original_length: 3,
                virtual_start: 5,
                virtual_end: 9,
                policy: DiagnosticDirectivePolicy::EXPECT,
                unused_expect_directive_index: Some(1),
            }],
        };
        let data = json_marshal(&diagnostic_directives, &[]).expect("marshal");
        assert_eq!(
            data,
            r#"{"unusedExpectDirectiveDiagnostics":[{"code":1,"messageText":"first"},{"code":2,"messageText":"second"}],"directives":[[2,3,5,9,1,1]]}"#
        );
        let mut decoded = DiagnosticDirectives::default();
        json_unmarshal(data.as_bytes(), &mut decoded, &[]).expect("unmarshal");
        assert_eq!(decoded, diagnostic_directives);
    }

    // Go: host_test.go:646 TestRunnerTransformSupplementalOutputs
    #[test]
    fn test_runner_transform_supplemental_outputs() {
        let host = new_host(
            &test_ctx(),
            fake_spawner(response_mapper(|_p| {
                Box::new(TransformResult {
                    mapped_output: MappedOutput {
                        text: "export default 1;".to_string(),
                        extension: ".ts".to_string(),
                        ..Default::default()
                    },
                    supplemental: vec![
                        SupplementalOutput {
                            mapped_output: MappedOutput {
                                text: "declare const first: string;".to_string(),
                                extension: ".ts".to_string(),
                                diagnostic_directives: protocol_diagnostic_directives(
                                    vec![MappedDiagnosticDirective {
                                        virtual_end: 7,
                                        policy: DiagnosticDirectivePolicy::IGNORE,
                                        ..Default::default()
                                    }],
                                    Vec::new(),
                                ),
                                ..Default::default()
                            },
                        },
                        SupplementalOutput {
                            mapped_output: MappedOutput {
                                text: "declare const second: number;".to_string(),
                                extension: ".mjs".to_string(),
                                ..Default::default()
                            },
                        },
                    ],
                    ..Default::default()
                })
            })),
            locale::DEFAULT,
        );
        let mapper = vue_mapper_with_extension();
        let result = host
            .transform(&mapper, request("/component.vue", "component"))
            .expect("transform");
        assert_eq!(result.supplemental.len(), 2);
        assert_eq!(result.supplemental[0].text, "declare const first: string;");
        assert_eq!(result.supplemental[0].virtual_extension, ".ts");
        assert!(result.supplemental[0].mappings.is_some());
        assert_eq!(result.supplemental[0].diagnostic_directives.len(), 1);
        assert_eq!(
            result.supplemental[0].diagnostic_directives[0]
                .virtual_range
                .end(),
            7
        );
        assert_eq!(result.supplemental[1].virtual_extension, ".mjs");
        assert!(result.supplemental[1].mappings.is_some());
        let _ = host.close();
    }

    // Go: host_test.go:677 TestRunnerTransformInvalidSupplementalDiagnosticDirective
    #[test]
    fn test_runner_transform_invalid_supplemental_diagnostic_directive() {
        let host = new_host(
            &test_ctx(),
            fake_spawner(response_mapper(|_p| {
                Box::new(TransformResult {
                    mapped_output: MappedOutput {
                        text: "export {};".to_string(),
                        extension: ".ts".to_string(),
                        ..Default::default()
                    },
                    supplemental: vec![
                        SupplementalOutput {
                            mapped_output: MappedOutput {
                                text: "export {};".to_string(),
                                extension: ".ts".to_string(),
                                ..Default::default()
                            },
                        },
                        SupplementalOutput {
                            mapped_output: MappedOutput {
                                text: "export {};".to_string(),
                                extension: ".ts".to_string(),
                                diagnostic_directives: protocol_diagnostic_directives(
                                    vec![MappedDiagnosticDirective {
                                        policy: DiagnosticDirectivePolicy::EXPECT,
                                        ..Default::default()
                                    }],
                                    Vec::new(),
                                ),
                                ..Default::default()
                            },
                        },
                    ],
                    ..Default::default()
                })
            })),
            locale::DEFAULT,
        );
        let mapper = vue_mapper_with_extension();
        let err = host
            .transform(&mapper, request("/component.vue", "component"))
            .err()
            .expect("transform error");
        let directive_error =
            errors::as_type::<DiagnosticDirectiveError>(&err).expect("DiagnosticDirectiveError");
        assert_eq!(
            directive_error.kind,
            DiagnosticDirectiveErrorKind::EXPECT_MISSING_UNUSED_DIAGNOSTIC
        );
        assert_eq!(directive_error.index, 0);
        assert_eq!(directive_error.supplemental_index, 1);
        let _ = host.close();
    }

    // Go: host_test.go:703 TestRunnerRejectsInvalidVirtualExtension
    #[test]
    fn test_runner_rejects_invalid_virtual_extension() {
        for supplemental in [false, true] {
            for extension in ["", ".coffee"] {
                let host = new_host(
                    &test_ctx(),
                    fake_spawner(response_mapper(move |_p| {
                        let mut canonical_extension = extension.to_string();
                        let mut supplemental_outputs: Vec<SupplementalOutput> = Vec::new();
                        if supplemental {
                            canonical_extension = ".ts".to_string();
                            supplemental_outputs = vec![SupplementalOutput {
                                mapped_output: MappedOutput {
                                    text: "export {};".to_string(),
                                    extension: extension.to_string(),
                                    ..Default::default()
                                },
                            }];
                        }
                        Box::new(TransformResult {
                            mapped_output: MappedOutput {
                                text: "export {};".to_string(),
                                extension: canonical_extension,
                                ..Default::default()
                            },
                            supplemental: supplemental_outputs,
                            ..Default::default()
                        })
                    })),
                    locale::DEFAULT,
                );
                let mapper = vue_mapper_with_extension();
                assert_error_contains(
                    host.transform(&mapper, request("/component.vue", "component")),
                    "invalid virtual extension",
                );
                let _ = host.close();
            }
        }
    }

    // Go: host_test.go:730 TestRunnerPositionEncodings
    #[test]
    fn test_runner_position_encodings() {
        for encoding in [PositionEncoding::UTF8, PositionEncoding::UTF16] {
            let r = new_host(
                &test_ctx(),
                fake_spawner(Arc::new(UnicodeMapper {
                    encoding: encoding.clone(),
                    source: None,
                })),
                locale::DEFAULT,
            );
            let mapper = mapper("", &encoding.0, "", "mapper");
            let result = r
                .transform(&mapper, request("/a.vue", "éx"))
                .expect("transform");
            let mappings = result.mappings.as_deref();
            let segments = spanmap::SpanMap::segments(mappings);
            assert_eq!(segments.len(), 2);
            assert_eq!(segments[0].virtual_end, 2);
            assert_eq!(segments[0].original_end, 2);
            assert_eq!(segments[1].virtual_start, 2);
            assert_eq!(segments[1].original_start, 2);
            assert_eq!(result.text, "éx");
            let problem = spanmap::SpanMap::validate(mappings, &result.text, "éx");
            assert!(problem.is_none(), "{problem:?}");
            let (mapped, fidelity) = spanmap::SpanMap::virtual_to_original_position(mappings, 2);
            assert_eq!(mapped, 2);
            assert_eq!(fidelity, spanmap::Fidelity::EXACT);
            assert_eq!(result.diagnostics[0].pos, 2);
            assert_eq!(result.diagnostics[0].end, 3);
            assert_eq!(result.diagnostic_directives[0].original_range.pos(), 2);
            assert_eq!(result.diagnostic_directives[0].original_range.end(), 3);
            assert_eq!(result.diagnostic_directives[0].virtual_range.pos(), 2);
            assert_eq!(result.diagnostic_directives[0].virtual_range.end(), 3);
            let _ = r.close();
        }
    }

    // Go: host_test.go:765 TestRunnerRejectsUnsupportedPositionEncoding
    #[test]
    fn test_runner_rejects_unsupported_position_encoding() {
        let r = new_host(
            &test_ctx(),
            fake_spawner(Arc::new(UnicodeMapper {
                encoding: PositionEncoding(Cow::Borrowed("utf-32")),
                source: None,
            })),
            locale::DEFAULT,
        );
        let mapper = mapper("", "invalid", "", "mapper");
        assert_error_contains(
            r.transform(&mapper, request("/a.vue", "x")),
            "unsupported position encoding",
        );
        let _ = r.close();
    }

    // Go: host_test.go:774 TestRunnerRejectsInvalidDiagnosticSource
    #[test]
    fn test_runner_rejects_invalid_diagnostic_source() {
        for source in [
            "",
            " ",
            "ts",
            "TS",
            "d.ts",
            "json",
            "typescript",
            "TypeScript",
            "tsc",
            "TSC",
        ] {
            let handler = UnicodeMapper {
                encoding: PositionEncoding::UTF8,
                source: Some(source.to_string()),
            };
            let r = new_host(
                &test_ctx(),
                fake_spawner(Arc::new(handler)),
                locale::DEFAULT,
            );
            let mapper = mapper("", "invalid", "", "mapper");
            let result = r.transform(&mapper, request("/a.vue", "x"));
            if source.trim().is_empty() {
                assert_error_contains(result, "diagnostic source must not be empty");
            } else {
                assert_error_contains(result, "is reserved by TypeScript");
            }
            let _ = r.close();
        }
    }

    // Go: host_test.go:793 TestRunnerRejectsPositionsInsideUnicodeCharacters
    #[test]
    fn test_runner_rejects_positions_inside_unicode_characters() {
        for (encoding, content) in [
            (PositionEncoding::UTF8, "é"),
            (PositionEncoding::UTF16, "😀"),
        ] {
            let r = new_host(
                &test_ctx(),
                fake_spawner(Arc::new(InvalidDiagnosticMapper {
                    encoding: encoding.clone(),
                })),
                locale::DEFAULT,
            );
            let mapper = mapper("", &encoding.0, "", "mapper");
            assert_error_contains(
                r.transform(&mapper, request("/a.vue", content)),
                "splits a Unicode code point",
            );
            let _ = r.close();
        }
    }

    // Go: host_test.go:813 TestRunnerConsolidatesByIdentity
    #[test]
    fn test_runner_consolidates_by_identity() {
        let spawner = Rc::new(FakeSpawner::default());
        let r = new_host(&test_ctx(), spawner.clone(), locale::DEFAULT);

        // Two logically-separate mappers with the same identity share one process.
        let vue_a = mapper("a", "vue", "1.0.0", "vue-mapper");
        let vue_b = mapper("b", "vue", "1.0.0", "vue-mapper");
        let svelte = mapper("", "svelte", "2.0.0", "svelte-mapper");
        let project = r
            .project(ProjectSpec {
                mappers: vec![vue_a.clone(), vue_b.clone(), svelte.clone()],
                compiler_options: default_options(),
                ..Default::default()
            })
            .expect("project");

        for m in [&vue_a, &vue_b, &vue_a, &svelte] {
            project.transform(m, request("/x", "y")).expect("transform");
        }
        assert_eq!(
            spawner.spawns.load(Ordering::SeqCst),
            2,
            "expected one process per identity"
        );
        let _ = project.close();
        let _ = r.close();
    }

    // Go: host_test.go:833 TestRunnerLeaseLifecycle
    #[test]
    fn test_runner_lease_lifecycle() {
        let spawner = Rc::new(FakeSpawner::default());
        let r = new_host(&test_ctx(), spawner.clone(), locale::DEFAULT);

        let vue_a = mapper("a", "vue", "1.0.0", "vue-mapper");
        let vue_b = mapper("b", "vue", "1.0.0", "vue-mapper");
        let svelte = mapper("", "svelte", "2.0.0", "svelte-mapper");

        let release_vue_a = r.acquire(&[vue_a.clone(), vue_a.clone()]);
        let release_vue_b = r.acquire(&[vue_b.clone()]);
        let release_svelte = r.acquire(&[svelte.clone()]);
        for mapper in [&vue_a, &svelte] {
            r.transform(mapper, request("/x", "y")).expect("transform");
        }
        assert_eq!(spawner.spawns.load(Ordering::SeqCst), 2);

        release_vue_a();
        assert_eq!(
            spawner.closes.load(Ordering::SeqCst),
            0,
            "shared vue process should remain owned"
        );
        release_svelte();
        assert_eq!(
            spawner.closes.load(Ordering::SeqCst),
            1,
            "final release should close the process"
        );
        release_vue_b();
        release_vue_b();
        assert_eq!(
            spawner.closes.load(Ordering::SeqCst),
            2,
            "final vue owner should close once"
        );

        let release_new = r.acquire(&[vue_a.clone()]);
        r.transform(&vue_a, request("/x", "y")).expect("transform");
        assert_eq!(
            spawner.spawns.load(Ordering::SeqCst),
            3,
            "reacquiring should spawn a fresh process lazily"
        );
        release_new();
        assert_eq!(spawner.closes.load(Ordering::SeqCst), 3);
        let _ = r.close();
    }

    // Go: host_test.go:869 recordingMapper
    // recordingMapper captures project configuration and lifecycle requests for host protocol tests.
    #[derive(Default)]
    struct RecordingState {
        received: String,
        received_options: String,
        received_locale: String,
        project_handles: Vec<String>,
        closed_handles: Vec<String>,
        transform_handle: String,
        transform_params: String,
    }

    // PORT: Go keeps every field under `mu`; the test configuration fields
    // are set once, before the mapper runs, so only the recorded state is
    // behind the mutex here. Go nil `watchedFiles` and `configIdentity` are
    // `None`.
    #[derive(Default)]
    struct RecordingMapper {
        state: Mutex<RecordingState>,
        watched_files: Option<Vec<String>>,
        config_identity: Option<String>,
        dynamic_config: bool,
        option_diagnostics: Vec<OptionDiagnosticResult>,
    }

    impl MapperHandler for RecordingMapper {
        // Go: host_test.go:890 recordingMapper.handlesProjects
        fn handles_projects(&self) -> bool {
            true
        }

        // Go: host_test.go:900 recordingMapper.HandleRequest
        fn handle_request(
            &self,
            method: &str,
            params: &JsonValue,
        ) -> std::result::Result<Option<Box<dyn AnyValue>>, GoError> {
            match method {
                METHOD_INITIALIZE => {
                    let p: InitializeParams = unmarshal_params(params)?;
                    lock(&self.state).received_locale = p.locale;
                    Ok(Some(initialize_result("mapper")))
                }
                METHOD_OPEN_PROJECT => {
                    let p: OpenProjectParams = unmarshal_params(params)?;
                    let raw = json_marshal(&p.compiler_options, &[]).map_err(errors::from_value)?;
                    {
                        let mut state = lock(&self.state);
                        state.project_handles.push(p.project_handle.clone());
                        state.received = raw;
                        state.received_options = String::from_utf8_lossy(&p.options.0).into_owned();
                    }
                    let mut watched_files = self.watched_files.clone();
                    if !self.dynamic_config
                        && watched_files.is_none()
                        && self.config_identity.is_none()
                        && self.option_diagnostics.is_empty()
                    {
                        return Ok(Some(Box::new(OpenProjectResult::default())));
                    }
                    if self.dynamic_config && watched_files.is_none() {
                        watched_files = Some(vec![tspath::combine_paths(
                            &tspath::get_directory_path(&p.config_file_name),
                            &["mapper.config.js"],
                        )]);
                    }
                    let mut config_identity = String::new();
                    if self.dynamic_config {
                        config_identity =
                            format!("config:{}", String::from_utf8_lossy(&p.options.0));
                    }
                    if let Some(identity) = &self.config_identity {
                        config_identity = identity.clone();
                    }
                    Ok(Some(Box::new(OpenProjectResult {
                        config_identity,
                        watched_files: watched_files.unwrap_or_default(),
                        option_diagnostics: self.option_diagnostics.clone(),
                    })))
                }
                METHOD_CLOSE_PROJECT => {
                    let p: CloseProjectParams = unmarshal_params(params)?;
                    lock(&self.state).closed_handles.push(p.project_handle);
                    Ok(None)
                }
                METHOD_TRANSFORM => {
                    let p: TransformParams = unmarshal_params(params)?;
                    {
                        let mut state = lock(&self.state);
                        state.transform_handle = p.project_handle.clone();
                        state.transform_params = String::from_utf8_lossy(&params.0).into_owned();
                    }
                    Ok(Some(Box::new(TransformResult {
                        mapped_output: MappedOutput {
                            text: p.content,
                            extension: ".ts".to_string(),
                            ..Default::default()
                        },
                        ..Default::default()
                    })))
                }
                _ => Err(unexpected_method(method)),
            }
        }
    }

    /// Forwards to a shared `RecordingMapper`, so the test keeps a handle.
    struct SharedRecordingMapper(Arc<RecordingMapper>);

    impl MapperHandler for SharedRecordingMapper {
        fn handles_projects(&self) -> bool {
            self.0.handles_projects()
        }

        fn handle_request(
            &self,
            method: &str,
            params: &JsonValue,
        ) -> std::result::Result<Option<Box<dyn AnyValue>>, GoError> {
            self.0.handle_request(method, params)
        }
    }

    fn recording_spawner(mapper: &Arc<RecordingMapper>) -> Rc<FakeSpawner> {
        fake_spawner(Arc::new(SharedRecordingMapper(mapper.clone())))
    }

    fn dynamic_mapper(options: &str, compiler_options: &[&str]) -> Rc<Mapper> {
        Rc::new(Mapper {
            definition: Definition {
                options: JsonValue(options.as_bytes().to_vec()),
                ..Default::default()
            },
            manifest: Manifest {
                name: "dynamic".to_string(),
                version: "1.0.0".to_string(),
                exec: vec!["mapper".to_string()],
                compiler_options: compiler_options.iter().map(|s| (*s).to_string()).collect(),
                dynamic_config: true,
            },
            ..Default::default()
        })
    }

    fn project_spec(
        config_file_name: &str,
        mappers: &[&Rc<Mapper>],
        compiler_options: &Option<Rc<CompilerOptions>>,
    ) -> ProjectSpec {
        ProjectSpec {
            config_file_name: config_file_name.to_string(),
            mappers: mappers.iter().map(|m| Rc::clone(m)).collect(),
            compiler_options: compiler_options.clone(),
        }
    }

    // Go: host_test.go:971 TestProjectLifecycle
    #[test]
    fn test_project_lifecycle() {
        let mapper_process = Arc::new(RecordingMapper {
            dynamic_config: true,
            ..Default::default()
        });
        let spawner = recording_spawner(&mapper_process);
        let host = new_host(&test_ctx(), spawner.clone(), locale::DEFAULT);

        let static_mapper = Rc::new(Mapper {
            definition: Definition {
                options: JsonValue(br#"{"mode":"static"}"#.to_vec()),
                ..Default::default()
            },
            manifest: Manifest {
                name: "static".to_string(),
                version: "1.0.0".to_string(),
                exec: vec!["mapper".to_string()],
                ..Default::default()
            },
            ..Default::default()
        });
        let static_project = host
            .project(project_spec(
                "/repo/tsconfig.json",
                &[&static_mapper],
                &default_options(),
            ))
            .expect("project");
        assert_eq!(
            spawner.spawns.load(Ordering::SeqCst),
            0,
            "static identity should not spawn the mapper"
        );
        let static_identities = static_project.identities().expect("identities");
        assert_eq!(static_identities.len(), 1);
        static_project.close().expect("close");

        let dynamic_a = dynamic_mapper(r#"{"mode":"a"}"#, &["jsx"]);
        let dynamic_b = dynamic_mapper(r#"{"mode":"b"}"#, &[]);
        let dynamic_a_options = default_options();
        let project_a = host
            .project(project_spec(
                "/repo/a/tsconfig.json",
                &[&dynamic_a, &dynamic_b],
                &dynamic_a_options,
            ))
            .expect("project");
        let project_a_reversed = host
            .project(project_spec(
                "/repo/reversed/tsconfig.json",
                &[&dynamic_b, &dynamic_a],
                &dynamic_a_options,
            ))
            .expect("project");
        let project_different_options = host
            .project(project_spec(
                "/repo/options/tsconfig.json",
                &[&dynamic_a],
                &Some(Rc::new(CompilerOptions {
                    jsx: JsxEmit::REACT,
                    ..Default::default()
                })),
            ))
            .expect("project");
        let project_b = host
            .project(project_spec(
                "/repo/b/tsconfig.json",
                &[&dynamic_a],
                &default_options(),
            ))
            .expect("project");
        let project_a_again = host
            .project(project_spec(
                "/repo/a/tsconfig.json",
                &[&dynamic_a, &dynamic_b],
                &dynamic_a_options,
            ))
            .expect("project");
        assert_eq!(
            spawner.spawns.load(Ordering::SeqCst),
            0,
            "getting dynamic projects should not start the mapper"
        );
        let project_a_identities = project_a.identities().expect("identities");
        let project_a_reversed_identities = project_a_reversed.identities().expect("identities");
        let project_different_option_identities =
            project_different_options.identities().expect("identities");
        let project_b_identities = project_b.identities().expect("identities");
        assert_eq!(project_a_identities.len(), 2);
        assert_eq!(project_a_reversed_identities.len(), 2);
        assert_eq!(project_a_identities[0], project_a_reversed_identities[1]);
        assert_eq!(project_a_identities[1], project_a_reversed_identities[0]);
        assert_ne!(
            project_a_identities[0],
            project_different_option_identities[0]
        );
        assert_eq!(project_b_identities.len(), 1);
        assert_eq!(
            spawner.spawns.load(Ordering::SeqCst),
            1,
            "dynamic projects should share one mapper process"
        );
        let project_a_watched_files = project_a.watched_files().expect("watched files");
        let project_b_watched_files = project_b.watched_files().expect("watched files");
        assert_eq!(project_a_watched_files.len(), 1);
        assert_eq!(project_b_watched_files.len(), 1);
        // ts#64159 (host_test.go:1049)
        assert_eq!(project_a_watched_files[0], "/repo/a/mapper.config.js");
        assert_eq!(project_b_watched_files[0], "/repo/b/mapper.config.js");

        project_a
            .transform(&dynamic_b, request("/repo/a/file.ext", "x"))
            .expect("transform");
        {
            let state = lock(&mapper_process.state);
            assert!(state.project_handles[..2].contains(&state.transform_handle));
        }

        project_a_again.close().expect("close");
        project_a.close().expect("close");
        project_a_reversed.close().expect("close");
        project_different_options.close().expect("close");
        project_b.close().expect("close");
        let timings = host.timings();
        let dynamic_timings = timings.mappers[&dynamic_a.identity()];
        assert_eq!(dynamic_timings.spawn.count, 1);
        assert_eq!(dynamic_timings.initialize.count, 1);
        assert_eq!(dynamic_timings.open_project.count, 6);
        assert_eq!(dynamic_timings.transform.count, 1);
        assert_eq!(dynamic_timings.close_project.count, 6);
        let state = lock(&mapper_process.state);
        assert_eq!(state.project_handles.len(), 6);
        assert_eq!(state.closed_handles.len(), 6);
        drop(state);
        let _ = host.close();
    }

    // Go: host_test.go:1076 TestProjectMethodsAfterHostClose
    #[test]
    fn test_project_methods_after_host_close() {
        let mapper_process = Arc::new(RecordingMapper {
            dynamic_config: true,
            ..Default::default()
        });
        let host = new_host(
            &test_ctx(),
            recording_spawner(&mapper_process),
            locale::DEFAULT,
        );
        let mapper = dynamic_mapper("", &[]);
        let project = host
            .project(project_spec(
                "/repo/tsconfig.json",
                &[&mapper],
                &default_options(),
            ))
            .expect("project");
        project
            .transform(&mapper, request("/repo/file.ext", "x"))
            .expect("transform");
        let identity = project
            .identity(&Rc::new(Mapper::default()))
            .expect("identity");
        assert_eq!(identity, "");
        host.close().expect("close");

        project.refresh().expect("refresh");
        let identities = project.identities().expect("identities");
        assert_eq!(identities.len(), 0);
        let identity = project.identity(&mapper).expect("identity");
        assert_eq!(identity, "");
        let identity = project
            .identity(&Rc::new(Mapper::default()))
            .expect("identity");
        assert_eq!(identity, "");
        let watched_files = project.watched_files().expect("watched files");
        assert_eq!(watched_files.len(), 0);
        assert_eq!(project.diagnostics().len(), 0);
        assert_error_contains(
            project.transform(&mapper, request("/repo/file.ext", "x")),
            "content mapper project is closed",
        );
        // Go: defer project.Close()
        let _ = project.close();
    }

    /// The project error inside a transform error (Go
    /// `errors.AsType[*TransformError]` then `errors.AsType[*ProjectError]`).
    fn project_error_kind(err: &GoError) -> ProjectErrorKind {
        assert!(
            errors::as_type::<TransformError>(err).is_some(),
            "expected TransformError, got {err:?}"
        );
        errors::as_type::<ProjectError>(err)
            .unwrap_or_else(|| panic!("expected ProjectError, got {err:?}"))
            .kind
    }

    // Go: host_test.go:1114 TestProjectRejectsRelativeWatchedFiles
    #[test]
    fn test_project_rejects_relative_watched_files() {
        let mapper_process = Arc::new(RecordingMapper {
            watched_files: Some(vec!["mapper.config.js".to_string()]),
            dynamic_config: true,
            ..Default::default()
        });
        let host = new_host(
            &test_ctx(),
            recording_spawner(&mapper_process),
            locale::DEFAULT,
        );

        let project_mapper = Rc::new(Mapper {
            definition: Definition {
                package: "dynamic".to_string(),
                ..Default::default()
            },
            ..(*dynamic_mapper("", &[])).clone()
        });
        let project = host
            .project(project_spec(
                "/repo/tsconfig.json",
                &[&project_mapper],
                &default_options(),
            ))
            .expect("project");
        let err = project
            .transform(&project_mapper, request("/repo/file.ext", "x"))
            .err()
            .expect("transform error");
        assert_eq!(
            project_error_kind(&err),
            ProjectErrorKind::NON_ABSOLUTE_WATCHED_FILE
        );
        let _ = host.close();
    }

    /// ts#64159 (hostimpl.go:690 `TryRootedFilePathFromAbsolute`): a watched
    /// file is normalized, and a URL with a query or fragment is rejected.
    // PORT: not in Go; Go has no test of either case.
    #[test]
    fn test_project_watched_files_are_rooted_file_paths() {
        let watched_files = |files: &[&str]| -> std::result::Result<Vec<String>, GoError> {
            let mapper_process = Arc::new(RecordingMapper {
                watched_files: Some(files.iter().map(|f| (*f).to_string()).collect()),
                dynamic_config: true,
                ..Default::default()
            });
            let host = new_host(
                &test_ctx(),
                recording_spawner(&mapper_process),
                locale::DEFAULT,
            );
            let project_mapper = dynamic_mapper("", &[]);
            let project = host
                .project(project_spec(
                    "/repo/tsconfig.json",
                    &[&project_mapper],
                    &default_options(),
                ))
                .expect("project");
            let result = project.watched_files();
            let _ = host.close();
            result
        };
        assert_eq!(
            watched_files(&["/repo/a/../b//mapper.config.js", "c:\\repo\\x.js", "c:"])
                .expect("watched files"),
            ["/repo/b/mapper.config.js", "c:/", "c:/repo/x.js"]
        );
        for file in [
            "file:///repo/mapper.config.js?v=1",
            "file:///repo/mapper.config.js#a",
        ] {
            let err = watched_files(&[file]).expect_err(file);
            assert_eq!(
                errors::as_type::<ProjectError>(&err)
                    .unwrap_or_else(|| panic!("expected ProjectError, got {err:?}"))
                    .kind,
                ProjectErrorKind::NON_ABSOLUTE_WATCHED_FILE
            );
        }
    }

    // Go: host_test.go:1137 TestDynamicProjectRequiresConfigIdentity
    #[test]
    fn test_dynamic_project_requires_config_identity() {
        let mapper_process = Arc::new(RecordingMapper {
            config_identity: Some(String::new()),
            dynamic_config: true,
            ..Default::default()
        });
        let host = new_host(
            &test_ctx(),
            recording_spawner(&mapper_process),
            locale::DEFAULT,
        );

        let project_mapper = Rc::new(Mapper {
            definition: Definition {
                package: "dynamic".to_string(),
                ..Default::default()
            },
            ..(*dynamic_mapper("", &[])).clone()
        });
        let project = host
            .project(project_spec(
                "/repo/tsconfig.json",
                &[&project_mapper],
                &default_options(),
            ))
            .expect("project");
        let err = project
            .transform(&project_mapper, request("/repo/file.ext", "x"))
            .err()
            .expect("transform error");
        assert_eq!(
            project_error_kind(&err),
            ProjectErrorKind::MISSING_CONFIG_IDENTITY
        );
        let _ = host.close();
    }

    // Go: host_test.go:1161 TestStaticMapperRejectsDynamicProjectResponseFields
    #[test]
    fn test_static_mapper_rejects_dynamic_project_response_fields() {
        let tests: [(&str, RecordingMapper, ProjectErrorKind); 2] = [
            (
                "config identity",
                RecordingMapper {
                    config_identity: Some("dynamic".to_string()),
                    ..Default::default()
                },
                ProjectErrorKind::UNEXPECTED_CONFIG_IDENTITY,
            ),
            (
                "watched files",
                RecordingMapper {
                    watched_files: Some(vec!["/repo/mapper.config.js".to_string()]),
                    ..Default::default()
                },
                ProjectErrorKind::UNEXPECTED_WATCHED_FILES,
            ),
        ];
        for (name, mapper_process, kind) in tests {
            let host = new_host(
                &test_ctx(),
                recording_spawner(&Arc::new(mapper_process)),
                locale::DEFAULT,
            );
            let project_mapper = mapper("", "static", "1.0.0", "mapper");
            let project = host
                .project(project_spec(
                    "/repo/tsconfig.json",
                    &[&project_mapper],
                    &default_options(),
                ))
                .expect("project");
            let err = project
                .transform(&project_mapper, request("/repo/file.ext", "x"))
                .err()
                .unwrap_or_else(|| panic!("{name}: expected an error"));
            assert_eq!(project_error_kind(&err), kind, "{name}");
            let _ = project.close();
            let _ = host.close();
        }
    }

    // Go: host_test.go:1193 TestProjectRejectsInvalidOptionDiagnosticPath
    #[test]
    fn test_project_rejects_invalid_option_diagnostic_path() {
        let mapper_process = Arc::new(RecordingMapper {
            option_diagnostics: vec![OptionDiagnosticResult {
                path: vec![JsonValue(b"null".to_vec())],
                message_text: "Invalid option.".to_string(),
                code: 123,
            }],
            ..Default::default()
        });
        let host = new_host(
            &test_ctx(),
            recording_spawner(&mapper_process),
            locale::DEFAULT,
        );
        let project_mapper = mapper("", "mapper", "1.0.0", "mapper");
        let project = host
            .project(ProjectSpec {
                mappers: vec![project_mapper.clone()],
                compiler_options: default_options(),
                ..Default::default()
            })
            .expect("project");
        let err = project
            .transform(&project_mapper, request("/repo/file.ext", "x"))
            .err()
            .expect("transform error");
        assert_eq!(
            project_error_kind(&err),
            ProjectErrorKind::MALFORMED_RESPONSE
        );
        let _ = project.close();
        let _ = host.close();
    }

    // Go: host_test.go:1217 TestRunnerForwardsProjectOptions
    #[test]
    fn test_runner_forwards_project_options() {
        let mapper_process = Arc::new(RecordingMapper::default());
        let (diagnostic_locale, ok) = locale::parse("cs-CZ");
        assert!(ok);
        let r = new_host(
            &test_ctx(),
            recording_spawner(&mapper_process),
            diagnostic_locale,
        );

        let mapper_definition = Rc::new(Mapper {
            definition: Definition {
                options: JsonValue(br#"{"strictTemplates":true}"#.to_vec()),
                ..Default::default()
            },
            manifest: Manifest {
                name: "vue".to_string(),
                version: "1.0.0".to_string(),
                exec: vec!["vue-mapper".to_string()],
                compiler_options: vec!["target".to_string(), "jsx".to_string()],
                ..Default::default()
            },
            ..Default::default()
        });
        let compiler_options = Rc::new(CompilerOptions {
            target: ScriptTarget::ES2020,
            strict: Tristate::True,
            ..Default::default()
        });
        let project = r
            .project(ProjectSpec {
                mappers: vec![mapper_definition.clone()],
                compiler_options: Some(compiler_options.clone()),
                ..Default::default()
            })
            .expect("project");
        project
            .transform(&mapper_definition, request("/a.vue", "x"))
            .expect("transform");

        let want = json_marshal(&CompilerOptionsJSON(&compiler_options), &[]).expect("marshal");
        {
            let state = lock(&mapper_process.state);
            assert_eq!(state.received, want);
            assert_eq!(state.received_options, r#"{"strictTemplates":true}"#);
            assert_eq!(state.received_locale, "cs-CZ");
            assert!(!state.transform_params.contains(r#""options""#));
            assert!(!state.transform_params.contains(r#""compilerOptions""#));
        }
        let _ = project.close();
        let _ = r.close();
    }

    // Go: host_test.go:1249 TestHostSetLocaleRestartsMapper
    #[test]
    fn test_host_set_locale_restarts_mapper() {
        let mapper_process = Arc::new(RecordingMapper::default());
        let spawner = recording_spawner(&mapper_process);
        let r = new_host(&test_ctx(), spawner.clone(), locale::DEFAULT);

        let definition = mapper("", "vue", "1.0.0", "vue-mapper");
        let release = r.acquire(&[definition.clone()]);

        r.transform(&definition, request("/a.vue", "x"))
            .expect("transform");
        assert_eq!(spawner.spawns.load(Ordering::SeqCst), 1);

        let (french, ok) = locale::parse("fr");
        assert!(ok);
        r.set_locale(french);
        assert_eq!(spawner.closes.load(Ordering::SeqCst), 1);

        r.transform(&definition, request("/a.vue", "x"))
            .expect("transform");
        assert_eq!(spawner.spawns.load(Ordering::SeqCst), 2);
        assert_eq!(lock(&mapper_process.state).received_locale, "fr");
        // Go: defer release()
        release();
        let _ = r.close();
    }

    // Go: host_test.go:1277 TestHostSetLocaleWaitsForTransform
    // PORT: Go runs the transform and SetLocale on two goroutines and checks
    // that SetLocale waits for the transform. The port's host is
    // dispatch-thread state, so a transform and SetLocale cannot overlap;
    // this checks what remains: SetLocale after the transform closes the
    // mapper process once.
    #[test]
    fn test_host_set_locale_waits_for_transform() {
        let mapper_process = Arc::new(RecordingMapper::default());
        let spawner = recording_spawner(&mapper_process);
        let r = new_host(&test_ctx(), spawner.clone(), locale::DEFAULT);
        let definition = mapper("", "vue", "1.0.0", "vue-mapper");
        let release = r.acquire(&[definition.clone()]);

        r.transform(&definition, request("/a.vue", "x"))
            .expect("transform");

        let (french, ok) = locale::parse("fr");
        assert!(ok);
        r.set_locale(french);
        assert_eq!(spawner.closes.load(Ordering::SeqCst), 1);
        release();
        let _ = r.close();
    }
}
