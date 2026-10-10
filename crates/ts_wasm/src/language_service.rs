//! A message-driven editor host. The native server's thread and timer loops are not needed here.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use serde_json::Value;
use ts_goport::api::proto::new_diagnostic_responses;
use ts_goport::ast::FileText;
use ts_goport::core::Diagnostic;
use ts_goport::execute::tsc::{System, SystemParseConfigHost, new_os_system};
use ts_goport::frontend::compiler::{self, ProgramOptions};
use ts_goport::frontend::json::{MarshalerTo, UnmarshalerFrom, json_marshal, json_unmarshal};
use ts_goport::frontend::tsoptions::{self, ParsedCommandLine};
use ts_goport::frontend::tspath;
use ts_goport::frontend::vfs::{Fs, OsOverride, install_os_override};
use ts_goport::gostd::{Context, GoError, context};
use ts_goport::ls::{self, lsconv, lsutil};
use ts_goport::lsp::lsproto;
use ts_goport::options::Tristate;
use ts_goport::program::ls_program;
use ts_goport::sourcemap;

use crate::host;

enum ServiceError {
    Parse(String),
    InvalidParams(String),
    MethodNotFound(String),
    Internal(String),
    Configuration { message: String, diagnostics: Value },
}

impl From<String> for ServiceError {
    fn from(message: String) -> Self {
        Self::InvalidParams(message)
    }
}
impl From<&str> for ServiceError {
    fn from(message: &str) -> Self {
        Self::InvalidParams(message.into())
    }
}
impl ServiceError {
    fn response(self) -> String {
        let (code, message, data) = match self {
            Self::Parse(message) => (-32700, message, Value::Null),
            Self::InvalidParams(message) => (-32602, message, Value::Null),
            Self::MethodNotFound(method) => (
                -32601,
                format!("unsupported language service method: {method}"),
                Value::Null,
            ),
            Self::Internal(message) => (-32603, message, Value::Null),
            Self::Configuration {
                message,
                diagnostics,
            } => (
                -32602,
                message,
                serde_json::json!({"diagnostics": diagnostics}),
            ),
        };
        serde_json::json!({"error": {"code": code, "message": message, "data": data}}).to_string()
    }
}

fn check_config(diagnostics: &[Diagnostic]) -> Result<(), ServiceError> {
    if diagnostics.is_empty() {
        return Ok(());
    }
    let diagnostics = new_diagnostic_responses(diagnostics);
    let message = diagnostics
        .iter()
        .map(|diagnostic| format!("TS{}: {}", diagnostic.code, diagnostic.text))
        .collect::<Vec<_>>()
        .join("\n");
    let json = json_marshal(&diagnostics, &[])
        .map_err(|error| ServiceError::Internal(error.to_string()))?;
    let diagnostics =
        serde_json::from_str(&json).map_err(|error| ServiceError::Internal(error.to_string()))?;
    Err(ServiceError::Configuration {
        message,
        diagnostics,
    })
}

#[derive(Clone, Copy, serde::Deserialize)]
enum Method {
    #[serde(rename = "textDocument/hover")]
    Hover,
    #[serde(rename = "textDocument/completion")]
    Completion,
    #[serde(rename = "textDocument/definition")]
    Definition,
    #[serde(rename = "textDocument/typeDefinition")]
    TypeDefinition,
    #[serde(rename = "textDocument/references")]
    References,
    #[serde(rename = "textDocument/rename")]
    Rename,
    #[serde(rename = "textDocument/implementation")]
    Implementation,
    #[serde(rename = "textDocument/documentHighlight")]
    DocumentHighlight,
    #[serde(rename = "textDocument/diagnostic")]
    Diagnostic,
    #[serde(rename = "textDocument/signatureHelp")]
    SignatureHelp,
    #[serde(rename = "textDocument/documentSymbol")]
    DocumentSymbol,
    #[serde(rename = "textDocument/codeAction")]
    CodeAction,
    #[serde(rename = "textDocument/formatting")]
    Formatting,
    #[serde(rename = "textDocument/rangeFormatting")]
    RangeFormatting,
    #[serde(rename = "textDocument/semanticTokens/full")]
    SemanticTokensFull,
    #[serde(rename = "textDocument/semanticTokens/range")]
    SemanticTokensRange,
}

pub struct Service {
    cwd: String,
    args: Vec<String>,
    context: Context,
    language: Option<ls::LanguageService>,
}

impl Service {
    fn new(request: &Value) -> Result<Self, ServiceError> {
        let cwd = request.get("cwd").and_then(Value::as_str).unwrap_or("/");
        if !cwd.starts_with('/') {
            return Err("cwd must be an absolute path".into());
        }
        let args: Vec<String> =
            serde_json::from_value(request["args"].clone()).map_err(|error| error.to_string())?;
        let capabilities = decode::<lsproto::ClientCapabilities>(
            request
                .get("capabilities")
                .unwrap_or(&serde_json::json!({})),
        )?;
        let context = lsproto::with_client_capabilities(
            &context::background(),
            Arc::new(lsproto::ClientCapabilities::resolve(Some(&capabilities))),
        );
        if ts_goport::frontend::vfs::os_override_installed() {
            return Err("each WASM instance owns one language service".into());
        }
        let case_sensitive = !request["caseInsensitive"].as_bool().unwrap_or(false);
        let _ = host::CASE_SENSITIVE.set(case_sensitive);
        install_os_override(OsOverride {
            fs: Arc::new(|| Rc::new(host::HostFs::new()) as Rc<dyn Fs>),
            current_directory: cwd.into(),
        });
        ts_goport::ast::set_editor_process();
        Ok(Self {
            cwd: cwd.into(),
            args,
            context,
            language: None,
        })
    }

    fn invalidate(&mut self) {
        let Some(language) = self.language.take() else {
            return;
        };
        let program = language.program.clone();
        drop(language);
        ls_program::release_program(&program);
    }

    fn language(&mut self) -> Result<&ls::LanguageService, ServiceError> {
        if self.language.is_none() {
            self.language = Some(build_language(&self.cwd, &self.args)?);
        }
        self.language
            .as_ref()
            .ok_or_else(|| ServiceError::Internal("language service is unavailable".into()))
    }

    fn request(&mut self, method: &str, params: &Value) -> Result<String, ServiceError> {
        let method: Method = serde_json::from_value(Value::String(method.into()))
            .map_err(|_| ServiceError::MethodNotFound(method.into()))?;
        let mut params = params.clone();
        let context = self.context.clone();
        let language = self.language()?;
        let _guard = language.enter_program();
        normalize_document(language, &mut params)?;
        dispatch(language, &context, method, &params)
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        self.invalidate();
    }
}

pub fn receive(service: &mut Option<Service>, input: &[u8]) -> String {
    let result = serde_json::from_slice::<Value>(input)
        .map_err(|error| ServiceError::Parse(error.to_string()))
        .and_then(|request| command(service, &request));
    match result {
        Ok(result) => format!("{{\"result\":{result}}}"),
        Err(error) => error.response(),
    }
}

fn command(service: &mut Option<Service>, request: &Value) -> Result<String, ServiceError> {
    match request["action"].as_str() {
        Some("initialize") => {
            if service.is_some() {
                return Err("language service is already initialized".into());
            }
            *service = Some(Service::new(request)?);
            Ok("null".into())
        }
        Some("dispose") => {
            *service = None;
            Ok("null".into())
        }
        Some("invalidate") => {
            service
                .as_mut()
                .ok_or("language service is disposed")?
                .invalidate();
            Ok("null".into())
        }
        Some("request") => {
            let method = request["method"]
                .as_str()
                .ok_or("method must be a string")?;
            service
                .as_mut()
                .ok_or("language service is disposed")?
                .request(method, &request["params"])
        }
        _ => Err("unknown language service action".into()),
    }
}

fn build_language(cwd: &str, args: &[String]) -> Result<ls::LanguageService, ServiceError> {
    let sys = new_os_system().map_err(|status| {
        ServiceError::Internal(format!("cannot initialize host: {}", status.code()))
    })?;
    let config = read_config(args, &sys)?;
    let host =
        compiler::new_compiler_host(cwd, sys.fs(), &sys.default_library_path(), None, None, None);
    let program = ls_program::new_program(
        ProgramOptions {
            host,
            config: Rc::new(config),
            use_source_of_project_reference: false,
            single_threaded: Tristate::True,
            typings_location: String::new(),
            project_name: cwd.into(),
            create_module_resolver: None,
            skip_module_resolution: false,
        },
        None,
    );
    let fs = program.host().fs();
    let lines_fs = fs.clone();
    let line_maps = RefCell::new(HashMap::<String, Rc<lsconv::LSPLineMap>>::new());
    let converters = lsconv::new_converters(lsproto::PositionEncodingKind::UTF16, move |name| {
        if let Some(lines) = line_maps.borrow().get(name).cloned() {
            return Some(lines);
        }
        let (text, found) = lines_fs.read_file(name);
        if !found {
            return None;
        }
        let lines = lsconv::compute_lsp_line_starts(&text);
        line_maps.borrow_mut().insert(name.into(), lines.clone());
        Some(lines)
    });
    let host = Rc::new(LanguageHost { fs, converters });
    Ok(ls::new_language_service(
        ls::autoimport::ProjectID(cwd.into()),
        program,
        host,
        "",
    ))
}

fn read_config(args: &[String], sys: &dyn System) -> Result<ParsedCommandLine, ServiceError> {
    let host = SystemParseConfigHost(sys);
    let parsed = tsoptions::parse_command_line(args, &host);
    check_config(&parsed.get_config_file_parsing_diagnostics())?;
    let options = parsed.compiler_options();
    if options.project.is_empty() {
        return Ok(parsed);
    }
    let mut path =
        tspath::get_normalized_absolute_path(&options.project, &sys.get_current_directory());
    if sys.fs().directory_exists(&path) {
        path = tspath::combine_paths(&path, &["tsconfig.json"]);
    }
    let (config, errors) =
        tsoptions::get_parsed_command_line_of_config_file(&path, Some(options), None, &host, None);
    check_config(&errors)?;
    let config = config.ok_or_else(|| {
        ServiceError::Internal(format!("project configuration was not returned: {path}"))
    })?;
    check_config(&config.get_config_file_parsing_diagnostics())?;
    Ok(config)
}

struct LanguageHost {
    fs: Rc<dyn Fs>,
    converters: Rc<lsconv::Converters>,
}

impl ls::Host for LanguageHost {
    fn use_case_sensitive_file_names(&self) -> bool {
        self.fs.use_case_sensitive_file_names()
    }
    fn read_file(&self, path: &str) -> (FileText, bool) {
        let (text, found) = self.fs.read_file(path);
        (FileText::Shared(Arc::from(text)), found)
    }
    fn converters(&self) -> Rc<lsconv::Converters> {
        self.converters.clone()
    }
    fn get_preferences(&self, _: &str) -> lsutil::UserPreferences {
        let mut preferences = lsutil::new_default_user_preferences();
        preferences.include_completions_for_module_exports = Tristate::False;
        preferences
    }
    fn get_ecma_line_info(&self, _: &str) -> Option<Rc<sourcemap::lineinfo::ECMALineInfo>> {
        None
    }
    fn auto_import_registry(&self) -> Option<Rc<ls::autoimport::Registry>> {
        None
    }
    fn read_directory(
        &self,
        cwd: &str,
        path: &str,
        extensions: &[String],
        excludes: &[String],
        includes: &[String],
        depth: i32,
    ) -> Vec<String> {
        ts_goport::frontend::vfs::read_directory(
            &*self.fs, cwd, path, extensions, excludes, includes, depth,
        )
    }
    fn get_directories(&self, path: &str) -> Vec<String> {
        self.fs.get_accessible_entries(path).directories
    }
    fn directory_exists(&self, path: &str) -> bool {
        self.fs.directory_exists(path)
    }
    fn file_exists(&self, path: &str) -> bool {
        self.fs.file_exists(path)
    }
}

fn normalize_document(
    language: &ls::LanguageService,
    params: &mut Value,
) -> Result<(), ServiceError> {
    let uri = params
        .pointer("/textDocument/uri")
        .and_then(Value::as_str)
        .ok_or("textDocument.uri must be a file URI")?;
    let parsed = ts_goport::gostd::url::parse(uri).map_err(|error| error.error())?;
    if parsed.scheme != "file" || !parsed.path.starts_with('/') {
        return Err("textDocument.uri must be a file URI".into());
    }
    let uri = lsproto::DocumentUri(uri.into());
    let (_, file) = language.try_get_program_and_file(&uri.file_name());
    if file.is_nil() {
        return Err("document is not part of the program".into());
    }
    let text = language.program.host().fs().read_file(&uri.file_name()).0;
    let lines = lsconv::compute_lsp_line_starts(&text);
    if let Some(position) = params.get_mut("position") {
        normalize_position(&text, &lines, position)?;
    }
    if let Some(range) = params.get_mut("range") {
        normalize_range(&text, &lines, range)?;
    }
    let Some(diagnostics) = params
        .pointer_mut("/context/diagnostics")
        .and_then(Value::as_array_mut)
    else {
        return Ok(());
    };
    for diagnostic in diagnostics {
        if !diagnostic.is_object() {
            return Err("diagnostics must contain diagnostic objects".into());
        }
        let range = diagnostic
            .get_mut("range")
            .ok_or("diagnostic range is required")?;
        normalize_range(&text, &lines, range)?;
    }
    Ok(())
}

fn normalize_range(
    text: &str,
    lines: &lsconv::LSPLineMap,
    range: &mut Value,
) -> Result<(), ServiceError> {
    normalize_position(
        text,
        lines,
        range.get_mut("start").ok_or("range start is required")?,
    )?;
    normalize_position(
        text,
        lines,
        range.get_mut("end").ok_or("range end is required")?,
    )?;
    let start = (
        range["start"]["line"].as_u64(),
        range["start"]["character"].as_u64(),
    );
    let end = (
        range["end"]["line"].as_u64(),
        range["end"]["character"].as_u64(),
    );
    if start > end {
        return Err("range end must be at or after its start".into());
    }
    Ok(())
}

fn normalize_position(
    text: &str,
    lines: &lsconv::LSPLineMap,
    position: &mut Value,
) -> Result<(), ServiceError> {
    let line = position["line"]
        .as_u64()
        .ok_or("line must be a nonnegative integer")?;
    let character = position["character"]
        .as_u64()
        .ok_or("character must be a nonnegative integer")?;
    let index = usize::try_from(line).map_err(|error| error.to_string())?;
    let start = *lines
        .line_starts
        .get(index)
        .ok_or("line is outside the document")?;
    let start =
        usize::try_from(start).map_err(|error| ServiceError::Internal(error.to_string()))?;
    let end = lines
        .line_starts
        .get(index + 1)
        .copied()
        .map(usize::try_from)
        .transpose()
        .map_err(|error| ServiceError::Internal(error.to_string()))?
        .unwrap_or(text.len());
    let maximum = text[start..end]
        .trim_end_matches(['\r', '\n'])
        .encode_utf16()
        .count();
    let maximum =
        u64::try_from(maximum).map_err(|error| ServiceError::Internal(error.to_string()))?;
    position["character"] = Value::from(character.min(maximum));
    Ok(())
}

fn decode<T: UnmarshalerFrom + Default>(params: &Value) -> Result<T, ServiceError> {
    let mut result = T::default();
    json_unmarshal(params.to_string().as_bytes(), &mut result, &[])
        .map_err(|error| error.to_string())?;
    Ok(result)
}

fn encode<T: MarshalerTo>(result: Result<T, GoError>) -> Result<String, ServiceError> {
    let result = result.map_err(|error| ServiceError::Internal(error.error()))?;
    json_marshal(&result, &[]).map_err(|error| ServiceError::Internal(error.to_string()))
}

fn dispatch(
    language: &ls::LanguageService,
    ctx: &Context,
    method: Method,
    params: &Value,
) -> Result<String, ServiceError> {
    macro_rules! call {
        ($params:ty, $function:ident) => {
            encode(language.$function(ctx, &decode::<$params>(params)?))
        };
    }
    match method {
        Method::Hover => call!(lsproto::HoverParams, provide_hover),
        Method::Completion => {
            let p: lsproto::CompletionParams = decode(params)?;
            encode(language.provide_completion(
                ctx,
                &p.text_document.uri,
                p.position,
                p.context.as_ref(),
            ))
        }
        Method::Definition => {
            let p: lsproto::DefinitionParams = decode(params)?;
            encode(language.provide_definition(ctx, &p.text_document.uri, p.position))
        }
        Method::TypeDefinition => {
            let p: lsproto::TypeDefinitionParams = decode(params)?;
            encode(language.provide_type_definition(ctx, &p.text_document.uri, p.position))
        }
        Method::References => encode(language.provide_references(ctx, &decode(params)?, None)),
        Method::Rename => encode(language.provide_rename(ctx, &decode(params)?, None)),
        Method::Implementation => {
            encode(language.provide_implementations(ctx, &decode(params)?, None))
        }
        Method::DocumentHighlight => {
            let p: lsproto::DocumentHighlightParams = decode(params)?;
            encode(language.provide_document_highlights(ctx, &p.text_document.uri, p.position))
        }
        Method::Diagnostic => {
            let p: lsproto::DocumentDiagnosticParams = decode(params)?;
            encode(language.provide_diagnostics(ctx, &p.text_document.uri))
        }
        Method::SignatureHelp => {
            let p: lsproto::SignatureHelpParams = decode(params)?;
            encode(language.provide_signature_help(
                ctx,
                &p.text_document.uri,
                p.position,
                p.context.as_ref(),
            ))
        }
        Method::DocumentSymbol => {
            let p: lsproto::DocumentSymbolParams = decode(params)?;
            encode(language.provide_document_symbols(ctx, &p.text_document.uri))
        }
        Method::CodeAction => call!(lsproto::CodeActionParams, provide_code_actions),
        Method::Formatting => {
            let p: lsproto::DocumentFormattingParams = decode(params)?;
            let options = p
                .options
                .as_ref()
                .ok_or("formatting options are required")?;
            encode(language.provide_format_document(ctx, &p.text_document.uri, options))
        }
        Method::RangeFormatting => {
            let p: lsproto::DocumentRangeFormattingParams = decode(params)?;
            let options = p
                .options
                .as_ref()
                .ok_or("formatting options are required")?;
            encode(language.provide_format_document_range(
                ctx,
                &p.text_document.uri,
                options,
                p.range,
            ))
        }
        Method::SemanticTokensFull => {
            let p: lsproto::SemanticTokensParams = decode(params)?;
            encode(language.provide_semantic_tokens(ctx, &p.text_document.uri))
        }
        Method::SemanticTokensRange => {
            let p: lsproto::SemanticTokensRangeParams = decode(params)?;
            encode(language.provide_semantic_tokens_range(ctx, &p.text_document.uri, p.range))
        }
    }
}
