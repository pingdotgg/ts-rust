//! Go: internal/testutil/contentmappertest/transforming.go (tsgo#4712).

use std::collections::HashMap;
use std::sync::Mutex;

use indexmap::IndexMap;
use ts_goport::ast::TextRange;

use super::prelude::*;

// Go: transforming.go:17 preamble
const PREAMBLE: &str = "const __VERSION = \"1.0.0\";\n";

// Go: transforming.go:19 DeclaredOptions
pub const DECLARED_OPTIONS: &[&str] = &["target", "jsx"];

// Go: transforming.go:21
const DIAGNOSTIC_SOURCE: &str = "box";
const UNCLOSED_INTERPOLATION_CODE: i32 = 1000;

/// Go `*collections.OrderedMap[string, json.Value]`; nil is `None`.
type Options = Option<IndexMap<String, JsonValue>>;

// Go: transforming.go:27 Handler
// Handler implements the transforming content mapper protocol.
// PORT: Go creates the map on the first `OpenProject`; here it starts empty.
#[derive(Default)]
pub struct Handler {
    compiler_options: Mutex<HashMap<String, Options>>,
}

impl Handler {
    fn compiler_options(&self) -> std::sync::MutexGuard<'_, HashMap<String, Options>> {
        self.compiler_options
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl ProjectLifecycleHandler for Handler {
    // Go: transforming.go:35 Handler.OpenProject
    fn open_project(&self, p: &OpenProjectParams) -> Result<(), GoError> {
        let mut options: Options = None;
        json_unmarshal(&p.compiler_options.0, &mut options, &[]).map_err(errors::from_value)?;
        self.compiler_options()
            .insert(p.project_handle.clone(), options);
        Ok(())
    }

    // Go: transforming.go:49 Handler.CloseProject
    fn close_project(&self, p: &CloseProjectParams) {
        self.compiler_options().remove(&p.project_handle);
    }
}

impl MapperHandler for Handler {
    // Go: transforming.go:55 Handler.HandleRequest
    fn handle_request(&self, _ctx: &Context, method: &str, params: JsonValue) -> HandlerResult {
        match method {
            contentmapper::METHOD_INITIALIZE => reply(initialize_result(DIAGNOSTIC_SOURCE)),
            contentmapper::METHOD_TRANSFORM => {
                let p: TransformParams = unmarshal_params(&params)?;
                let options = self
                    .compiler_options()
                    .get(&p.project_handle)
                    .cloned()
                    .flatten();
                let Some(options) = options else {
                    return Err(errors::new(format!(
                        "contentmappertest: project {} is not open",
                        strconv::quote(&p.project_handle)
                    )));
                };
                let (text, mappings, diagnostics, diagnostic_directives) =
                    transform(&p.content, Some(&options))?;
                reply(TransformResult {
                    mapped_output: MappedOutput {
                        text,
                        extension: mapped_extension(&p.content),
                        mappings,
                        diagnostic_directives,
                    },
                    diagnostics,
                    ..TransformResult::default()
                })
            }
            _ => Err(unexpected_method(method)),
        }
    }

    // Go: `*Handler` implements `projectLifecycleHandler`.
    fn project_lifecycle(&self) -> Option<&dyn ProjectLifecycleHandler> {
        Some(self)
    }
}

// Go: transforming.go:83 mappedExtension
fn mapped_extension(content: &str) -> String {
    const PREFIX: &str = "// @box-extension:";
    if let Some((first_line, _)) = content.split_once('\n')
        && let Some(extension) = first_line.strip_prefix(PREFIX)
    {
        return extension.trim().to_string();
    }
    ".ts".to_string()
}

/// The virtual text and its segments that `transform` writes (Go: the
/// `virtual` builder and `segments` of its closures).
#[derive(Default)]
struct VirtualWriter {
    virtual_: String,
    segments: Vec<Segment>,
}

impl VirtualWriter {
    // Go: transforming.go:98 writeVerbatim
    fn write_verbatim(&mut self, content: &str, from: usize, to: usize) {
        if to <= from {
            return;
        }
        let virtual_start = self.virtual_.len() as i32;
        self.virtual_.push_str(&content[from..to]);
        self.segments.push(Segment {
            virtual_start,
            virtual_end: self.virtual_.len() as i32,
            original_start: from as i32,
            original_end: to as i32,
            kind: Kind::VERBATIM,
            features: Feature::ALL,
        });
    }

    // Go: transforming.go:113 writeAtom
    fn write_atom(&mut self, value: &str, from: usize, to: usize) {
        let virtual_start = self.virtual_.len() as i32;
        self.virtual_.push_str(value);
        self.segments.push(Segment {
            virtual_start,
            virtual_end: self.virtual_.len() as i32,
            original_start: from as i32,
            original_end: to as i32,
            kind: Kind::ATOM,
            features: Feature::ALL,
        });
    }
}

// Go: transforming.go:91 transform
#[allow(clippy::type_complexity)]
fn transform(
    content: &str,
    options: Option<&IndexMap<String, JsonValue>>,
) -> Result<
    (
        String,
        JsonValue,
        Vec<contentmapper::Diagnostic>,
        Option<DiagnosticDirectives>,
    ),
    GoError,
> {
    let mut w = VirtualWriter::default();
    let mut diagnostics: Vec<contentmapper::Diagnostic> = Vec::new();

    w.virtual_.push_str(PREAMBLE);

    let mut pos = 0usize;
    while pos < content.len() {
        let Some(rel) = content[pos..].find("#{") else {
            w.write_verbatim(content, pos, content.len());
            break;
        };
        let token_start = pos + rel;

        // Go `tokenStart + strings.IndexByte(...)`, or `len(content)` when
        // there is no line break.
        let line_end = content[token_start..]
            .find('\n')
            .map_or(content.len(), |i| token_start + i);
        let Some(close_rel) = content[token_start..line_end].find('}') else {
            w.write_verbatim(content, pos, token_start);
            w.write_atom("undefined", token_start, line_end);
            diagnostics.push(contentmapper::Diagnostic {
                message_text: "Unclosed interpolation.".to_string(),
                start: token_start as i32,
                length: (line_end - token_start) as i32,
                code: UNCLOSED_INTERPOLATION_CODE,
            });
            pos = line_end;
            continue;
        };
        let token_end = token_start + close_rel + 1;
        let name = &content[token_start + "#{".len()..token_end - "}".len()];

        w.write_verbatim(content, pos, token_start);
        w.write_atom(&render_option(options, name), token_start, token_end);
        pos = token_end;
    }

    let span_map = spanmap::new(&w.segments);
    let mappings = span_map.marshal()?;
    let directives = diagnostic_directives(content, &span_map);
    Ok((w.virtual_, JsonValue(mappings), diagnostics, directives))
}

/// Go `wrap` of `diagnosticDirectives`.
fn wrap(
    directives: Vec<contentmapper::MappedDiagnosticDirective>,
    unused: Vec<UnusedExpectDirectiveDiagnostic>,
) -> Option<DiagnosticDirectives> {
    Some(DiagnosticDirectives {
        unused_expect_directive_diagnostics: unused,
        directives,
    })
}

/// A `contentmapper.MappedDiagnosticDirective` literal with the given policy.
fn directive(policy: DiagnosticDirectivePolicy) -> contentmapper::MappedDiagnosticDirective {
    contentmapper::MappedDiagnosticDirective {
        policy,
        ..contentmapper::MappedDiagnosticDirective::default()
    }
}

// Go: transforming.go:169 diagnosticDirectives
fn diagnostic_directives(content: &str, mappings: &SpanMap) -> Option<DiagnosticDirectives> {
    const INVALID_PREFIX: &str = "// @box-invalid-directive:";
    if content.starts_with(INVALID_PREFIX) {
        // Go `strings.SplitN(content, "\n", 2)[0]`.
        let first_line = content.split('\n').next().unwrap_or("");
        let kind = first_line
            .strip_prefix(INVALID_PREFIX)
            .unwrap_or(first_line)
            .trim();
        match kind {
            "invalid-range" => {
                return wrap(
                    vec![contentmapper::MappedDiagnosticDirective {
                        virtual_start: -1,
                        ..directive(DiagnosticDirectivePolicy::IGNORE)
                    }],
                    Vec::new(),
                );
            }
            "original-range-out-of-bounds" => {
                return wrap(
                    vec![contentmapper::MappedDiagnosticDirective {
                        original_start: content.len() as i32 + 1,
                        ..directive(DiagnosticDirectivePolicy::IGNORE)
                    }],
                    Vec::new(),
                );
            }
            "virtual-range-out-of-bounds" => {
                return wrap(
                    vec![contentmapper::MappedDiagnosticDirective {
                        virtual_start: 1 << 20,
                        virtual_end: 1 << 20,
                        ..directive(DiagnosticDirectivePolicy::IGNORE)
                    }],
                    Vec::new(),
                );
            }
            "invalid-policy" => {
                return wrap(vec![directive(DiagnosticDirectivePolicy(2))], Vec::new());
            }
            "ignore-with-unused-diagnostic" => {
                return wrap(
                    vec![directive(DiagnosticDirectivePolicy::IGNORE)],
                    vec![UnusedExpectDirectiveDiagnostic::default()],
                );
            }
            "expect-without-unused-diagnostic" => {
                return wrap(
                    vec![directive(DiagnosticDirectivePolicy::EXPECT)],
                    Vec::new(),
                );
            }
            "invalid-unused-diagnostic-index" => {
                return wrap(
                    vec![contentmapper::MappedDiagnosticDirective {
                        unused_expect_directive_index: Some(1),
                        ..directive(DiagnosticDirectivePolicy::EXPECT)
                    }],
                    vec![UnusedExpectDirectiveDiagnostic::default()],
                );
            }
            "overlap" => {
                return wrap(
                    vec![
                        contentmapper::MappedDiagnosticDirective {
                            virtual_end: 2,
                            ..directive(DiagnosticDirectivePolicy::IGNORE)
                        },
                        contentmapper::MappedDiagnosticDirective {
                            virtual_start: 1,
                            virtual_end: 3,
                            ..directive(DiagnosticDirectivePolicy::IGNORE)
                        },
                    ],
                    Vec::new(),
                );
            }
            _ => {}
        }
    }
    const IGNORE_PREFIX: &str = "// @box-ignore";
    const EXPECT_PREFIX_COLON: &str = "// @box-expect-error:";
    let mut result: Vec<contentmapper::MappedDiagnosticDirective> = Vec::new();
    let mut unused_diagnostics: Vec<UnusedExpectDirectiveDiagnostic> = Vec::new();
    let mut line_start = 0usize;
    while line_start < content.len() {
        let line_end = content[line_start..]
            .find('\n')
            .map_or(content.len(), |i| i + line_start);
        let line = &content[line_start..line_end];
        let trimmed = line.trim();
        let mut policy = DiagnosticDirectivePolicy::default();
        let mut has_policy = false;
        let mut unused_diagnostic_index: i32 = -1;
        if trimmed == IGNORE_PREFIX {
            policy = DiagnosticDirectivePolicy::IGNORE;
            has_policy = true;
        } else if let Some(message_text) = trimmed.strip_prefix(EXPECT_PREFIX_COLON) {
            policy = DiagnosticDirectivePolicy::EXPECT;
            has_policy = true;
            unused_diagnostic_index = unused_diagnostics.len() as i32;
            unused_diagnostics.push(UnusedExpectDirectiveDiagnostic {
                code: 2578,
                message_text: message_text.trim().to_string(),
            });
        }
        if has_policy && line_end < content.len() {
            let affected_start = line_end + 1;
            let affected_length = content[affected_start..]
                .find('\n')
                .unwrap_or(content.len() - affected_start);
            let virtual_spans = SpanMap::original_to_virtual_spans(
                Some(mappings),
                TextRange::new(
                    affected_start as i32,
                    (affected_start + affected_length) as i32,
                ),
                Feature::ALL,
            );
            if virtual_spans.len() == 1 {
                let mut directive = contentmapper::MappedDiagnosticDirective {
                    original_start: line_start as i32,
                    original_length: (line_end - line_start) as i32,
                    virtual_start: virtual_spans[0].span.pos(),
                    virtual_end: virtual_spans[0].span.end(),
                    policy,
                    unused_expect_directive_index: None,
                    diagnostic_codes: None,
                };
                if unused_diagnostic_index >= 0 {
                    directive.unused_expect_directive_index = Some(unused_diagnostic_index);
                }
                result.push(directive);
            }
        }
        if line_end == content.len() {
            break;
        }
        line_start = line_end + 1;
    }
    if unused_diagnostics.len() == 1 {
        for directive in &mut result {
            directive.unused_expect_directive_index = None;
        }
    }
    if result.is_empty() && unused_diagnostics.is_empty() {
        return None;
    }
    wrap(result, unused_diagnostics)
}

// Go: transforming.go:267 renderOption
fn render_option(options: Option<&IndexMap<String, JsonValue>>, name: &str) -> String {
    if let Some(options) = options
        && let Some(value) = options.get(name)
        && !value.0.is_empty()
    {
        return String::from_utf8_lossy(&value.0).into_owned();
    }
    "undefined".to_string()
}
