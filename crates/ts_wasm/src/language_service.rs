//! A message-driven editor host. The native server's thread and timer loops are not needed here.

use std::rc::Rc;
use std::sync::Arc;

use serde_json::Value;
use ts_goport::ast::FileText;
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

pub struct Service {
    cwd: String,
    args: Vec<String>,
    context: Context,
    language: Option<ls::LanguageService>,
}

impl Service {
    fn new(request: &Value) -> Result<Self, String> {
        let cwd = request.get("cwd").and_then(Value::as_str).unwrap_or("/");
        if !cwd.starts_with('/') {
            return Err("cwd must be an absolute path".into());
        }
        let args: Vec<String> =
            serde_json::from_value(request["args"].clone()).map_err(|error| error.to_string())?;
        let case_sensitive = !request["caseInsensitive"].as_bool().unwrap_or(false);
        let _ = host::CASE_SENSITIVE.set(case_sensitive);
        install_os_override(OsOverride {
            fs: Arc::new(|| Rc::new(host::HostFs::new()) as Rc<dyn Fs>),
            current_directory: cwd.into(),
        });
        ts_goport::ast::set_editor_process();
        let capabilities = decode::<lsproto::ClientCapabilities>(
            request
                .get("capabilities")
                .unwrap_or(&serde_json::json!({})),
        )?;
        let context = lsproto::with_client_capabilities(
            &context::background(),
            Arc::new(lsproto::ClientCapabilities::resolve(Some(&capabilities))),
        );
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

    fn language(&mut self) -> Result<&ls::LanguageService, String> {
        if self.language.is_none() {
            self.language = Some(build_language(&self.cwd, &self.args)?);
        }
        self.language
            .as_ref()
            .ok_or_else(|| "language service is unavailable".into())
    }

    fn request(&mut self, method: &str, params: &Value) -> Result<String, String> {
        let context = self.context.clone();
        let language = self.language()?;
        let _guard = language.enter_program();
        validate_document(language, params)?;
        dispatch(language, &context, method, params)
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        self.invalidate();
    }
}

pub fn receive(service: &mut Option<Service>, input: &[u8]) -> String {
    let result = serde_json::from_slice::<Value>(input)
        .map_err(|error| error.to_string())
        .and_then(|request| command(service, &request));
    match result {
        Ok(result) => format!("{{\"result\":{result}}}"),
        Err(message) => {
            serde_json::json!({"error": {"code": -32602, "message": message}}).to_string()
        }
    }
}

fn command(service: &mut Option<Service>, request: &Value) -> Result<String, String> {
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

fn build_language(cwd: &str, args: &[String]) -> Result<ls::LanguageService, String> {
    let sys =
        new_os_system().map_err(|status| format!("cannot initialize host: {}", status.code()))?;
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
    let converters = lsconv::new_converters(lsproto::PositionEncodingKind::UTF16, move |name| {
        let (text, found) = lines_fs.read_file(name);
        found.then(|| lsconv::compute_lsp_line_starts(&text))
    });
    let host = Rc::new(LanguageHost { fs, converters });
    Ok(ls::new_language_service(
        ls::autoimport::ProjectID(cwd.into()),
        program,
        host,
        "",
    ))
}

fn read_config(args: &[String], sys: &dyn System) -> Result<ParsedCommandLine, String> {
    let host = SystemParseConfigHost(sys);
    let parsed = tsoptions::parse_command_line(args, &host);
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
    if !errors.is_empty() {
        return Err(format!("cannot read project configuration: {}", path));
    }
    config.ok_or_else(|| format!("cannot read project configuration: {path}"))
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

fn validate_document(language: &ls::LanguageService, params: &Value) -> Result<(), String> {
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
    if let Some(position) = params.get("position") {
        validate_position(&text, position)?;
    }
    if let Some(range) = params.get("range") {
        validate_range(&text, range)?;
    }
    let Some(diagnostics) = params
        .pointer("/context/diagnostics")
        .and_then(Value::as_array)
    else {
        return Ok(());
    };
    for diagnostic in diagnostics {
        if !diagnostic.is_object() {
            return Err("diagnostics must contain diagnostic objects".into());
        }
        let range = diagnostic
            .get("range")
            .ok_or("diagnostic range is required")?;
        validate_range(&text, range)?;
    }
    Ok(())
}

fn validate_range(text: &str, range: &Value) -> Result<(), String> {
    validate_position(text, &range["start"])?;
    validate_position(text, &range["end"])?;
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

fn validate_position(text: &str, position: &Value) -> Result<(), String> {
    let line = position["line"]
        .as_u64()
        .ok_or("line must be a nonnegative integer")?;
    let character = position["character"]
        .as_u64()
        .ok_or("character must be a nonnegative integer")?;
    let lines = lsconv::compute_lsp_line_starts(text);
    let index = usize::try_from(line).map_err(|error| error.to_string())?;
    let start = *lines
        .line_starts
        .get(index)
        .ok_or("line is outside the document")? as usize;
    let end = lines
        .line_starts
        .get(index + 1)
        .map_or(text.len(), |offset| *offset as usize);
    if character
        > text[start..end]
            .trim_end_matches(['\r', '\n'])
            .encode_utf16()
            .count() as u64
    {
        return Err("character is outside the line".into());
    }
    Ok(())
}

fn decode<T: UnmarshalerFrom + Default>(params: &Value) -> Result<T, String> {
    let mut result = T::default();
    json_unmarshal(params.to_string().as_bytes(), &mut result, &[])
        .map_err(|error| error.to_string())?;
    Ok(result)
}

fn encode<T: MarshalerTo>(result: Result<T, GoError>) -> Result<String, String> {
    let result = result.map_err(|error| error.error())?;
    json_marshal(&result, &[]).map_err(|error| error.to_string())
}

fn dispatch(
    language: &ls::LanguageService,
    ctx: &Context,
    method: &str,
    params: &Value,
) -> Result<String, String> {
    macro_rules! call {
        ($params:ty, $function:ident) => {
            encode(language.$function(&ctx, &decode::<$params>(params)?))
        };
    }
    match method {
        "textDocument/hover" => call!(lsproto::HoverParams, provide_hover),
        "textDocument/completion" => {
            let p: lsproto::CompletionParams = decode(params)?;
            encode(language.provide_completion(
                &ctx,
                &p.text_document.uri,
                p.position,
                p.context.as_ref(),
            ))
        }
        "textDocument/definition" => {
            let p: lsproto::DefinitionParams = decode(params)?;
            encode(language.provide_definition(&ctx, &p.text_document.uri, p.position))
        }
        "textDocument/typeDefinition" => {
            let p: lsproto::TypeDefinitionParams = decode(params)?;
            encode(language.provide_type_definition(&ctx, &p.text_document.uri, p.position))
        }
        "textDocument/references" => {
            encode(language.provide_references(&ctx, &decode(params)?, None))
        }
        "textDocument/rename" => encode(language.provide_rename(&ctx, &decode(params)?, None)),
        "textDocument/implementation" => {
            encode(language.provide_implementations(&ctx, &decode(params)?, None))
        }
        "textDocument/documentHighlight" => {
            let p: lsproto::DocumentHighlightParams = decode(params)?;
            encode(language.provide_document_highlights(&ctx, &p.text_document.uri, p.position))
        }
        "textDocument/diagnostic" => {
            let p: lsproto::DocumentDiagnosticParams = decode(params)?;
            encode(language.provide_diagnostics(&ctx, &p.text_document.uri))
        }
        "textDocument/signatureHelp" => {
            let p: lsproto::SignatureHelpParams = decode(params)?;
            encode(language.provide_signature_help(
                ctx,
                &p.text_document.uri,
                p.position,
                p.context.as_ref(),
            ))
        }
        "textDocument/documentSymbol" => {
            let p: lsproto::DocumentSymbolParams = decode(params)?;
            encode(language.provide_document_symbols(&ctx, &p.text_document.uri))
        }
        "textDocument/codeAction" => call!(lsproto::CodeActionParams, provide_code_actions),
        "textDocument/formatting" => {
            let p: lsproto::DocumentFormattingParams = decode(params)?;
            let options = p
                .options
                .as_ref()
                .ok_or("formatting options are required")?;
            encode(language.provide_format_document(ctx, &p.text_document.uri, options))
        }
        "textDocument/rangeFormatting" => {
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
        "textDocument/semanticTokens/full" => {
            let p: lsproto::SemanticTokensParams = decode(params)?;
            encode(language.provide_semantic_tokens(&ctx, &p.text_document.uri))
        }
        "textDocument/semanticTokens/range" => {
            let p: lsproto::SemanticTokensRangeParams = decode(params)?;
            encode(language.provide_semantic_tokens_range(&ctx, &p.text_document.uri, p.range))
        }
        _ => Err(format!("unsupported language service method: {method}")),
    }
}
