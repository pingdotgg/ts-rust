//! Port of Go `compiler/emitter.go`: the per-file emitter, the script
//! transformer pipeline and the source map path helpers.
//!
//! `sourceFileMayBeEmitted`, `getSourceFilesToEmit`, `isSourceFileNotJson`
//! and `getDeclarationDiagnostics` are ported in `program.rs`.

use crate::prelude::*;

use super::program_emit::{EmitResult, PoolJsPart, SourceMapEmitResult, WriteFile, WriteFileData};
use crate::declarations::{
    DeclarationTransformer, SupplementalReferencesTransformer,
    new_supplemental_references_transformer,
};
use crate::frontend::outputpaths::OutputPaths;
use crate::frontend::outputpaths::get_source_file_path_in_new_dir;
use crate::frontend::tspath::{
    ComparePathsOptions, combine_paths, ensure_trailing_directory_separator, file_extension_is,
    get_base_file_name, get_directory_path, get_relative_path_to_directory_or_url, get_root_length,
    normalize_path, normalize_slashes,
};
use crate::printer::EmitResolver;
use crate::printer::emit_context::ParseEmitNodes;
use crate::sourcemap::generator::{Generator, new_generator};
use crate::transformers::reference_resolver::new_binder_reference_resolver;
use crate::transformers::transformer::{
    EmitResolverReferenceResolver, TransformOptions, TransformReferenceResolver, TransformerBox,
};

// Go: compiler/emitter.go:24 EmitOnly
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EmitOnly {
    #[default]
    All,
    Js,
    Dts,
    // #4849: renamed from Go `EmitOnlyForcedDts`.
    BuilderSignature,
}

// Go: compiler/emitter.go:33 emitter
// PORT: Go `tr *tracing.Tracing` is the process session
// (`crate::tracing::get`), so it is not a field.
pub struct Emitter {
    pub host: Rc<crate::program::EmitHost>,
    pub emit_only: EmitOnly,
    pub emitter_diagnostics: DiagnosticsCollection,
    pub writer: Option<Rc<RefCell<dyn EmitTextWriter>>>,
    pub paths: OutputPaths,
    pub source_file: Node,
    pub emit_result: EmitResult,
    // #4699
    pub force_emit: bool,
    pub write_file: Option<WriteFile>,
    /// PORT: not in Go. Set when this emitter runs only the d.ts part of the
    /// file and another emitter runs the JS part: on the emit pool, or before
    /// it on the twin of the file's checker (`program_emit`). The d.ts part
    /// waits for the JS part before it writes, so a file's outputs are
    /// written in Go's order (JS, then d.ts), and its `WriteFileData` holds
    /// the JS diagnostics too.
    pub js_part: Option<Rc<RefCell<PoolJsPart>>>,
}

/// PORT: not in Go (perf). The second half of Go `emitDeclarationFile`: the
/// print of a transformed d.ts tree (`Emitter::transform_declaration_file`).
/// A split file's d.ts part hands it to the d.ts twin of its checker
/// (`program_emit`).
pub struct DeclarationPrint {
    /// The transformed SourceFile.
    pub source_file: Node,
    /// Go `contentMappedSource`: the SourceFile before the transforms. The
    /// print reads its span map for the declaration map.
    pub content_mapped_source: Node,
    pub emit_declaration_map: bool,
    pub emit_context: Rc<EmitContext>,
    /// The Go `emitDeclarationFileOrBundle` trace event, which ends after
    /// the print.
    trace: Option<crate::tracing::Pop>,
}

impl DeclarationPrint {
    /// A print with no trace event, for another thread.
    #[must_use]
    pub fn new(
        source_file: Node,
        content_mapped_source: Node,
        emit_declaration_map: bool,
        emit_context: Rc<EmitContext>,
    ) -> Self {
        Self {
            source_file,
            content_mapped_source,
            emit_declaration_map,
            emit_context,
            trace: None,
        }
    }
}

/// PORT: not in Go (perf). The second half of Go `emitJSFile`: the print of
/// a transformed JS tree (`Emitter::transform_js_file`). With twins on, a
/// file whose JS transforms ran on its checker thread hands it to the twin
/// of its checker (`program_emit`).
pub struct JsPrint {
    /// The transformed SourceFile.
    pub source_file: Node,
    pub emit_context: Rc<EmitContext>,
    /// The Go `emitJsFileOrBundle` trace event, which ends after the print.
    trace: Option<crate::tracing::Pop>,
}

impl JsPrint {
    /// A print with no trace event, for another thread.
    #[must_use]
    pub fn new(source_file: Node, emit_context: Rc<EmitContext>) -> Self {
        Self {
            source_file,
            emit_context,
            trace: None,
        }
    }
}

// Go: compiler/emitter.go:57 declarationTransformer (#4712)
// PORT: renamed, because `DeclarationTransformer` is the declarations
// transformer struct.
trait DeclarationTransformerLike {
    fn transform_source_file(&mut self, source_file: Node) -> Node;
    fn get_diagnostics(&self) -> Vec<Diagnostic>;
}

impl DeclarationTransformerLike for DeclarationTransformer {
    fn transform_source_file(&mut self, source_file: Node) -> Node {
        self.transform_source_file_root(source_file)
    }

    fn get_diagnostics(&self) -> Vec<Diagnostic> {
        DeclarationTransformer::get_diagnostics(self)
    }
}

impl DeclarationTransformerLike for SupplementalReferencesTransformer {
    fn transform_source_file(&mut self, source_file: Node) -> Node {
        SupplementalReferencesTransformer::transform_source_file(self, source_file)
    }

    fn get_diagnostics(&self) -> Vec<Diagnostic> {
        SupplementalReferencesTransformer::get_diagnostics(self)
    }
}

impl Emitter {
    fn writer(&self) -> std::cell::RefMut<'_, dyn EmitTextWriter> {
        self.writer.as_ref().expect("nil writer").borrow_mut()
    }

    // Go: compiler/emitter.go:46 emitter.emit
    pub fn emit(&mut self) {
        let _trace = crate::tracing::get().map(|tr| {
            tr.push(
                crate::tracing::Phase::Emit,
                "emit",
                vec![(
                    "path",
                    source_file_info(self.source_file).path.clone().into(),
                )],
                true,
            )
        });
        let js_file_path = self.paths.js_file_path().to_string();
        let source_map_file_path = self.paths.source_map_file_path().to_string();
        let declaration_file_path = self.paths.declaration_file_path().to_string();
        let declaration_map_path = self.paths.declaration_map_path().to_string();
        // ts#64649 (emitter.go:50-53): one emit context and one emit resolver
        // for both outputs, so the d.ts part sees what the JS transforms
        // wrote in the context. Where the port runs the two parts in two
        // emitters (`program_emit`: the emit pool and the d.ts twins), the
        // d.ts part gets that data in `transform_declaration_part`.
        let emit_context = new_emit_context();
        let emit_resolver = self.host.new_emit_resolver(emit_context);
        self.emit_js_file(
            emit_resolver.clone(),
            self.source_file,
            &js_file_path,
            &source_map_file_path,
        );
        self.emit_declaration_file(
            emit_resolver,
            self.source_file,
            &declaration_file_path,
            &declaration_map_path,
        );
        self.emit_result.diagnostics = self.emitter_diagnostics.get_diagnostics();
    }

    // Go: compiler/emitter.go:62 emitter.getDeclarationTransformers
    // #4712: takes the source file, and adds the supplemental references
    // transformer.
    // ts#64649: takes the emit resolver of this emit.
    fn get_declaration_transformers(
        &self,
        emit_resolver: Rc<dyn EmitResolver>,
        source_file: Node,
        declaration_file_path: &str,
        declaration_map_path: &str,
    ) -> Vec<Box<dyn DeclarationTransformerLike>> {
        let force_dts_emit = self.emit_only == EmitOnly::BuilderSignature
            || self.force_emit && self.emit_only == EmitOnly::Dts;
        let mut transformers: Vec<Box<dyn DeclarationTransformerLike>> = Vec::with_capacity(2);
        transformers.push(Box::new(crate::declarations::new_declaration_transformer(
            self.host.clone(),
            emit_resolver,
            options(),
            declaration_file_path,
            declaration_map_path,
        )));
        // PORT: Go passes the source file, and the transformer reads its
        // `SupplementalSourceFiles()`. The Rust transformer takes that list.
        transformers.push(Box::new(new_supplemental_references_transformer(
            self.host.clone(),
            source_file_supplemental_source_files(source_file).to_vec(),
            declaration_file_path,
            force_dts_emit,
        )));
        transformers
    }

    // Go: compiler/emitter.go:70 emitter.runScriptTransformers
    // ts#64649: takes the emit resolver of this emit.
    fn run_script_transformers(
        &self,
        emit_resolver: Rc<dyn EmitResolver>,
        mut source_file: Node,
    ) -> Node {
        let _trace = crate::tracing::get().map(|tr| {
            tr.push(
                crate::tracing::Phase::Emit,
                "transformNodes",
                vec![("path", source_file_info(source_file).path.clone().into())],
                false,
            )
        });
        for mut transformer in get_script_transformers(emit_resolver, source_file) {
            source_file = transformer.transform_source_file(source_file);
        }
        source_file
    }

    // Go: compiler/emitter.go:80 emitter.runDeclarationTransformers
    // ts#64649: takes the emit resolver of this emit.
    fn run_declaration_transformers(
        &self,
        emit_resolver: Rc<dyn EmitResolver>,
        mut source_file: Node,
        declaration_file_path: &str,
        declaration_map_path: &str,
    ) -> (Node, Vec<Diagnostic>) {
        let _trace = crate::tracing::get().map(|tr| {
            tr.push(
                crate::tracing::Phase::Emit,
                "transformNodes",
                vec![("path", source_file_info(source_file).path.clone().into())],
                false,
            )
        });
        let mut diags = Vec::new();
        for mut transformer in self.get_declaration_transformers(
            emit_resolver,
            source_file,
            declaration_file_path,
            declaration_map_path,
        ) {
            source_file = transformer.transform_source_file(source_file);
            diags.extend(transformer.get_diagnostics());
        }
        emit_test_panic(EmitTestPanic::DeclarationTransforms, self.source_file);
        (source_file, diags)
    }

    // Go: compiler/emitter.go:185 emitter.emitJSFile
    fn emit_js_file(
        &mut self,
        emit_resolver: Rc<dyn EmitResolver>,
        source_file: Node,
        js_file_path: &str,
        source_map_file_path: &str,
    ) {
        if let Some(print) = self.transform_js_file(emit_resolver, source_file, js_file_path) {
            self.print_js_file(print, js_file_path, source_map_file_path);
        }
    }

    /// PORT: not in Go. The JS part of a file (`emit_only` is `Js`) up to
    /// its print: the script transforms. It returns the print, or `None`
    /// when the part has nothing to print (skipped or blocked).
    /// `finish_js_part` runs the rest. The part has its own emit context
    /// and resolver, as `emit` makes them (emitter.go:50-51). When the
    /// file's d.ts part runs in another emitter, it gets the parse-tree
    /// entries of this context (`EmitContext::export_parse_emit_nodes` of
    /// `JsPrint::emit_context`) for `transform_declaration_part`.
    pub fn transform_js_part(&mut self) -> Option<JsPrint> {
        debug_assert!(self.emit_only == EmitOnly::Js, "not a JS part");
        let js_file_path = self.paths.js_file_path().to_string();
        let emit_resolver = self.host.new_emit_resolver(new_emit_context());
        let print = self.transform_js_file(emit_resolver, self.source_file, &js_file_path);
        if print.is_none() {
            self.emit_result.diagnostics = self.emitter_diagnostics.get_diagnostics();
        }
        print
    }

    /// PORT: not in Go. The rest of `emit` after `transform_js_part`
    /// returned `print`. It needs no checker: Go `emitJSFile` gives its
    /// printer no handlers.
    pub fn finish_js_part(&mut self, print: JsPrint) {
        let js_file_path = self.paths.js_file_path().to_string();
        let source_map_file_path = self.paths.source_map_file_path().to_string();
        self.print_js_file(print, &js_file_path, &source_map_file_path);
        self.emit_result.diagnostics = self.emitter_diagnostics.get_diagnostics();
    }

    /// The part of Go `emitJSFile` up to its printer: the script
    /// transforms. None when it returns before the print.
    fn transform_js_file(
        &mut self,
        emit_resolver: Rc<dyn EmitResolver>,
        source_file: Node,
        js_file_path: &str,
    ) -> Option<JsPrint> {
        // Go: compiler/emitter.go:186 (ts#64649)
        let emit_context = emit_resolver.emit_context().clone();
        let options = options();

        if source_file.is_nil()
            || self.emit_only != EmitOnly::All && self.emit_only != EmitOnly::Js
            || js_file_path.is_empty()
        {
            return None;
        }

        if !self.force_emit
            && (options.no_emit == Tristate::True
                || crate::printer::EmitHost::is_emit_blocked(self.host.as_ref(), js_file_path))
        {
            self.emit_result.emit_skipped = true;
            return None;
        }

        let trace = crate::tracing::get().map(|tr| {
            tr.push(
                crate::tracing::Phase::Emit,
                "emitJsFileOrBundle",
                vec![("jsFilePath", js_file_path.to_string().into())],
                true,
            )
        });

        let source_file = self.run_script_transformers(emit_resolver, source_file);
        Some(JsPrint {
            source_file,
            emit_context,
            trace,
        })
    }

    /// The rest of Go `emitJSFile`: the printer and the print of `print`.
    fn print_js_file(&mut self, print: JsPrint, js_file_path: &str, source_map_file_path: &str) {
        emit_test_panic(EmitTestPanic::JsPrint, self.source_file);
        let options = options();
        let JsPrint {
            source_file,
            emit_context,
            trace: _trace,
        } = print;

        let printer_options = PrinterOptions {
            remove_comments: options.remove_comments.is_true(),
            new_line: options.new_line,
            no_emit_helpers: options.no_emit_helpers.is_true(),
            source_map: options.source_map.is_true(),
            inline_source_map: options.inline_source_map.is_true(),
            inline_sources: options.inline_sources.is_true(),
            target: options.target,
            // !!!
            ..PrinterOptions::default()
        };

        // create a printer to print the nodes
        let mut printer = new_printer(
            printer_options,
            PrintHandlers {
                // !!!
                ..PrintHandlers::default()
            },
            Some(emit_context),
        );

        // PORT: not in Go. Size the output buffer once. JS output is close to
        // the source length (measured 0.7 to 1.1 times on hono, zod and
        // effect), so most files need no regrowth copy.
        let source_len = source_file_text(self.source_file).len() as i32;
        let size_hint = source_len.saturating_add(source_len / 8);
        self.writer().grow(size_hint);

        let should_emit_source_maps = should_emit_source_maps(options, source_file);
        self.print_source_file(
            js_file_path,
            source_map_file_path,
            source_file,
            &mut printer,
            options,
            should_emit_source_maps,
        );
    }

    /// PORT: not in Go. The d.ts part of a split file (`emit_only` is
    /// `Dts`) up to its print: the declaration transforms, which call the
    /// emit resolver, so they run on the file's checker thread. It returns
    /// the print, or `None` when the part has nothing to print (skipped or
    /// blocked). `finish_declaration_part` runs the rest.
    ///
    /// `js_emit_nodes` holds what the file's JS transforms wrote on
    /// parse-tree nodes, when another emitter ran them for this emit (the
    /// emit pool or the checker before its twin). Go runs both parts in one
    /// context (emitter.go:50-53), so the part's new context gets them
    /// before the declaration transforms. It is None when no JS transforms
    /// ran for this emit (no JS part, skipped, blocked), as Go's d.ts part
    /// then sees no JS data. One difference to Go: the JS print can write
    /// too (Go printer.go:1601, `EFNoSourceMap` on a function body), after
    /// the export. A d.ts print never prints a function body.
    pub fn transform_declaration_part(
        &mut self,
        js_emit_nodes: Option<ParseEmitNodes>,
    ) -> Option<DeclarationPrint> {
        debug_assert!(self.emit_only == EmitOnly::Dts, "not a d.ts part");
        let declaration_file_path = self.paths.declaration_file_path().to_string();
        let declaration_map_path = self.paths.declaration_map_path().to_string();
        let emit_context = new_emit_context();
        if let Some(js_emit_nodes) = js_emit_nodes {
            emit_context.import_parse_emit_nodes(js_emit_nodes);
        }
        let emit_resolver = self.host.new_emit_resolver(emit_context);
        let print = self.transform_declaration_file(
            emit_resolver,
            self.source_file,
            &declaration_file_path,
            &declaration_map_path,
        );
        if print.is_none() {
            self.emit_result.diagnostics = self.emitter_diagnostics.get_diagnostics();
        }
        print
    }

    /// PORT: not in Go. The rest of `emit` after `transform_declaration_part`
    /// returned `print`. It needs no checker.
    pub fn finish_declaration_part(&mut self, print: DeclarationPrint) {
        let declaration_file_path = self.paths.declaration_file_path().to_string();
        let declaration_map_path = self.paths.declaration_map_path().to_string();
        self.print_declaration_file(print, &declaration_file_path, &declaration_map_path);
        self.emit_result.diagnostics = self.emitter_diagnostics.get_diagnostics();
    }

    // Go: compiler/emitter.go:223 emitter.emitDeclarationFile
    fn emit_declaration_file(
        &mut self,
        emit_resolver: Rc<dyn EmitResolver>,
        source_file: Node,
        declaration_file_path: &str,
        declaration_map_path: &str,
    ) {
        if let Some(print) = self.transform_declaration_file(
            emit_resolver,
            source_file,
            declaration_file_path,
            declaration_map_path,
        ) {
            self.print_declaration_file(print, declaration_file_path, declaration_map_path);
        }
    }

    /// The part of Go `emitDeclarationFile` up to its printer: the
    /// declaration transforms. None when it returns before the print.
    fn transform_declaration_file(
        &mut self,
        emit_resolver: Rc<dyn EmitResolver>,
        source_file: Node,
        declaration_file_path: &str,
        declaration_map_path: &str,
    ) -> Option<DeclarationPrint> {
        // Go: compiler/emitter.go:224 (ts#64649)
        let emit_context = emit_resolver.emit_context().clone();
        let options = options();

        if source_file.is_nil()
            || self.emit_only == EmitOnly::Js
            || declaration_file_path.is_empty()
        {
            return None;
        }
        let emit_declaration_map =
            self.emit_only != EmitOnly::BuilderSignature && options.declaration_map.is_true();
        let content_mapped_source = source_file;

        let trace = crate::tracing::get().map(|tr| {
            tr.push(
                crate::tracing::Phase::Emit,
                "emitDeclarationFileOrBundle",
                vec![(
                    "declarationFilePath",
                    declaration_file_path.to_string().into(),
                )],
                true,
            )
        });

        let (source_file, diags) = self.run_declaration_transformers(
            emit_resolver,
            source_file,
            declaration_file_path,
            declaration_map_path,
        );

        for elem in &diags {
            // Add declaration transform diagnostics to emit diagnostics
            self.emitter_diagnostics.add(elem.clone());
        }

        if !self.force_emit
            && self.emit_only != EmitOnly::BuilderSignature
            && (options.no_emit == Tristate::True
                || crate::printer::EmitHost::is_emit_blocked(
                    self.host.as_ref(),
                    declaration_file_path,
                ))
        {
            self.emit_result.emit_skipped = true;
            return None;
        }

        let decl_blocked =
            !diags.is_empty() && !self.force_emit && self.emit_only != EmitOnly::BuilderSignature;
        if decl_blocked {
            self.emit_result.emit_skipped = true;
            return None;
        }

        Some(DeclarationPrint {
            source_file,
            content_mapped_source,
            emit_declaration_map,
            emit_context,
            trace,
        })
    }

    /// The rest of Go `emitDeclarationFile`: the printer and the print of
    /// `print`.
    fn print_declaration_file(
        &mut self,
        print: DeclarationPrint,
        declaration_file_path: &str,
        declaration_map_path: &str,
    ) {
        let options = options();
        let DeclarationPrint {
            source_file,
            content_mapped_source,
            emit_declaration_map,
            emit_context,
            trace: _trace,
        } = print;

        let printer_options = PrinterOptions {
            remove_comments: options.remove_comments.is_true(),
            new_line: options.new_line,
            no_emit_helpers: true,
            // Module: 			   options.Module, // NYI
            // ModuleResolution:   options.ModuleResolution, // NYI
            target: options.get_emit_script_target(),
            source_map: emit_declaration_map,
            inline_source_map: options.inline_source_map.is_true(),
            // InlineSources:       options.InlineSources.IsTrue(), // ignored, per strada
            // ExtendedDiagnostics: options.ExtendedDiagnostics.IsTrue(), // NYI
            only_print_js_doc_style: true,
            omit_brace_source_map_positions: true,
            ..PrinterOptions::default()
        };

        // create a printer to print the nodes
        let mut print_handlers = PrintHandlers::default();
        if let Some(span_map) = source_file_span_map(content_mapped_source)
            && emit_declaration_map
        {
            let original_source: Rc<dyn crate::sourcemap::source::Source> =
                Rc::new(new_declaration_map_source(content_mapped_source));
            let content_mapped_file_name = source_file_file_name(content_mapped_source);
            // PORT: Go takes and returns a `sourcemap.Source`. The printer's
            // source is a source file node; `None` in the result is the
            // source that Go returns unchanged, and a `None` result is Go
            // `ok == false`.
            print_handlers.map_source_position = Some(Rc::new(move |source: Node, pos: i32| {
                if source_file_file_name(source) != content_mapped_file_name {
                    return Some((None, pos));
                }
                let (mapped, ok) = crate::spanmap::SpanMap::virtual_to_original_position_exact(
                    Some(span_map),
                    pos,
                );
                if !ok {
                    return None;
                }
                Some((Some(original_source.clone()), mapped))
            }));
        }
        let mut printer = new_printer(printer_options, print_handlers, Some(emit_context));

        let declaration_map_options = CompilerOptions {
            source_map: if emit_declaration_map {
                Tristate::True
            } else {
                Tristate::False
            },
            source_root: options.source_root.clone(),
            map_root: options.map_root.clone(),
            // Explicitly do not pass through either inline option.
            ..CompilerOptions::default()
        };
        // PORT: not in Go. Size the output buffer once. A d.ts file is
        // usually smaller than its source (measured median 0.5 times on
        // hono, 0.7 times on effect).
        let source_len = source_file_text(self.source_file).len() as i32;
        self.writer().grow(source_len / 2);

        let should_emit_source_maps =
            should_emit_source_maps(&declaration_map_options, source_file);
        self.print_source_file(
            declaration_file_path,
            declaration_map_path,
            source_file,
            &mut printer,
            &declaration_map_options,
            should_emit_source_maps,
        );
    }

    // Go: compiler/emitter.go:315 emitter.printSourceFile
    fn print_source_file(
        &mut self,
        js_file_path: &str,
        source_map_file_path: &str,
        source_file: Node,
        printer: &mut Printer,
        map_options: &CompilerOptions,
        should_emit_source_maps: bool,
    ) {
        // !!! sourceMapGenerator
        let options = options();
        let mut source_map_generator: Option<Rc<RefCell<Generator>>> = None;
        if should_emit_source_maps {
            source_map_generator = Some(Rc::new(RefCell::new(new_generator(
                &get_base_file_name(&normalize_slashes(js_file_path)),
                &get_source_root(map_options),
                &self.get_source_map_directory(map_options, js_file_path, source_file),
                ComparePathsOptions {
                    use_case_sensitive_file_names: use_case_sensitive_file_names(),
                    current_directory: get_current_directory().to_string(),
                },
            ))));
        }

        let writer = self.writer.clone().expect("nil writer");
        printer.write_exported(
            source_file,
            source_file,
            writer,
            source_map_generator.clone(),
        );
        // PORT: Go N' (ts#64649) calls `emitContext.Factory.ReleaseArenas()`
        // here (emitter.go:329), after each file, on the file's own emit
        // context. It only lets the GC take nodes that nothing reaches. Here
        // the transform and declaration emit nodes stay with their thread or
        // program version (`ast::synthetic` owners); only the checker's
        // to-string builder frees its nodes per call (`PrintScope` in
        // checker/printer_impl.rs).
        // PORT: not in Go. See `js_part`.
        self.wait_for_js_part();

        let mut source_map_url_pos = -1;
        if let Some(generator) = &source_map_generator {
            if map_options.source_map.is_true() || map_options.inline_source_map.is_true() {
                let mut generator = generator.borrow_mut();
                let input_source_file_names = generator.sources();
                let source_map = generator.raw_source_map();
                self.emit_result.source_maps.push(SourceMapEmitResult {
                    input_source_file_names,
                    source_map,
                    generated_file: js_file_path.to_string(),
                });
            }

            let source_mapping_url = self.get_source_mapping_url(
                map_options,
                &mut generator.borrow_mut(),
                js_file_path,
                source_map_file_path,
                source_file,
            );

            if !source_mapping_url.is_empty() {
                let mut writer = self.writer();
                if !writer.is_at_start_of_line() {
                    writer.raw_write(if options.new_line == NewLineKind::CRLF {
                        "\r\n"
                    } else {
                        "\n"
                    });
                }
                source_map_url_pos = writer.get_text_pos();
                writer.write_comment("//# sourceMappingURL=");
                writer.write_comment(&source_mapping_url);
            }

            // Write the source map
            if !source_map_file_path.is_empty() {
                let source_map = generator.borrow_mut().string();
                // #4699: the source map write gets the source file too.
                let result = self.write_text(
                    source_map_file_path,
                    &source_map,
                    &mut WriteFileData {
                        source_file: self.source_file,
                        ..WriteFileData::default()
                    },
                );
                match result {
                    Err(err) => {
                        self.emitter_diagnostics.add(new_compiler_diagnostic(
                            diag::Could_not_write_file_0_Colon_1,
                            args![js_file_path, err],
                        ));
                    }
                    Ok(()) => self
                        .emit_result
                        .emitted_files
                        .push(source_map_file_path.to_string()),
                }
            }
        } else {
            self.writer().write_line();
        }

        // Write the output file
        // PORT: Go `e.writer.String()` shares the builder's bytes. Here the
        // text moves out of the writer with no copy; the `clear` below stays.
        let mut text = self.writer().take_string();
        if options.emit_bom.is_true() {
            text = add_utf8_byte_order_mark(text);
        }
        let mut data = WriteFileData {
            source_map_url_pos,
            build_info: None,
            diagnostics: self.write_data_diagnostics(),
            skipped_dts_write: false,
            source_file: self.source_file,
        };
        let result = self.write_text(js_file_path, &text, &mut data);
        let skipped_dts_write = data.skipped_dts_write;
        match result {
            Err(err) => {
                self.emitter_diagnostics.add(new_compiler_diagnostic(
                    diag::Could_not_write_file_0_Colon_1,
                    args![js_file_path, err],
                ));
            }
            Ok(()) => {
                if !skipped_dts_write {
                    self.emit_result
                        .emitted_files
                        .push(js_file_path.to_string());
                }
            }
        }

        // Reset state
        self.writer().clear();
    }

    /// Waits for the JS part, if this emitter runs a d.ts part (`js_part`).
    /// Go runs the JS part first on the same goroutine.
    fn wait_for_js_part(&self) {
        if let Some(js_part) = &self.js_part {
            js_part.borrow_mut().wait();
        }
    }

    /// Go `e.emitterDiagnostics.GetDiagnostics()` for `WriteFileData`. In Go
    /// the collection also holds the diagnostics of the JS part when the
    /// d.ts part writes; with the JS part in another emitter (`js_part`)
    /// they are added here.
    fn write_data_diagnostics(&self) -> Vec<Diagnostic> {
        let Some(js_part) = &self.js_part else {
            return self.emitter_diagnostics.get_diagnostics();
        };
        let mut js_part = js_part.borrow_mut();
        js_part.wait();
        let mut diagnostics = DiagnosticsCollection::default();
        for diagnostic in js_part.diagnostics() {
            diagnostics.add(diagnostic.clone());
        }
        for diagnostic in self.emitter_diagnostics.get_diagnostics() {
            diagnostics.add(diagnostic);
        }
        diagnostics.get_diagnostics()
    }

    // Go: compiler/emitter.go:394 emitter.writeText
    fn write_text(
        &self,
        file_name: &str,
        text: &str,
        data: &mut WriteFileData,
    ) -> Result<(), String> {
        if let Some(write_file) = &self.write_file {
            return write_file(file_name, text, data);
        }
        crate::printer::EmitHost::write_file(self.host.as_ref(), file_name, text)
    }

    // Go: compiler/emitter.go:432 emitter.getSourceMapDirectory
    fn get_source_map_directory(
        &self,
        map_options: &CompilerOptions,
        file_path: &str,
        source_file: Node,
    ) -> String {
        if !map_options.source_root.is_empty() {
            return common_source_directory().to_string();
        }
        if !map_options.map_root.is_empty() {
            let mut source_map_dir = normalize_slashes(&map_options.map_root);
            if source_file.is_some() {
                // For modules or multiple emit files the mapRoot will have directory structure like the sources
                // So if src\a.ts and src\lib\b.ts are compiled together user would be moving the maps into mapRoot\a.js.map and mapRoot\lib\b.js.map
                source_map_dir = get_directory_path(&get_source_file_path_in_new_dir(
                    source_file_file_name(source_file),
                    &source_map_dir,
                    get_current_directory(),
                    common_source_directory(),
                    use_case_sensitive_file_names(),
                ));
            }
            if get_root_length(&source_map_dir) == 0 {
                // The relative paths are relative to the common directory
                source_map_dir = combine_paths(common_source_directory(), &[&source_map_dir]);
            }
            return source_map_dir;
        }
        get_directory_path(&normalize_path(file_path))
    }

    // Go: compiler/emitter.go:442 emitter.getSourceMappingURL
    fn get_source_mapping_url(
        &self,
        map_options: &CompilerOptions,
        source_map_generator: &mut Generator,
        file_path: &str,
        source_map_file_path: &str,
        source_file: Node,
    ) -> String {
        if map_options.inline_source_map.is_true() {
            // Encode the sourceMap into the sourceMap url
            return source_map_generator.base64_data_url();
        }

        let source_map_file = get_base_file_name(&normalize_slashes(source_map_file_path));
        if !map_options.map_root.is_empty() {
            let mut source_map_dir = normalize_slashes(&map_options.map_root);
            if source_file.is_some() {
                // For modules or multiple emit files the mapRoot will have directory structure like the sources
                // So if src\a.ts and src\lib\b.ts are compiled together user would be moving the maps into mapRoot\a.js.map and mapRoot\lib\b.js.map
                source_map_dir = get_directory_path(&get_source_file_path_in_new_dir(
                    source_file_file_name(source_file),
                    &source_map_dir,
                    get_current_directory(),
                    common_source_directory(),
                    use_case_sensitive_file_names(),
                ));
            }
            if get_root_length(&source_map_dir) == 0 {
                // The relative paths are relative to the common directory
                source_map_dir = combine_paths(common_source_directory(), &[&source_map_dir]);
                return encode_uri(&get_relative_path_to_directory_or_url(
                    &get_directory_path(&normalize_path(file_path)), // get the relative sourceMapDir path based on jsFilePath
                    &combine_paths(&source_map_dir, &[&source_map_file]), // this is where user expects to see sourceMap
                    /*isAbsolutePathAnUrl*/ true,
                    &ComparePathsOptions {
                        use_case_sensitive_file_names: use_case_sensitive_file_names(),
                        current_directory: get_current_directory().to_string(),
                    },
                ));
            } else {
                return encode_uri(&combine_paths(&source_map_dir, &[&source_map_file]));
            }
        }
        encode_uri(&source_map_file)
    }
}

// Go: compiler/emitter.go:296 declarationMapSource
struct DeclarationMapSource {
    file_name: &'static str,
    text: FileText,
    line_map: Vec<i32>,
}

// Go: compiler/emitter.go:302 newDeclarationMapSource
fn new_declaration_map_source(source_file: Node) -> DeclarationMapSource {
    let text = source_file_original_text(source_file);
    DeclarationMapSource {
        file_name: source_file_original_file_name(source_file),
        line_map: compute_ecma_line_starts(&text),
        text,
    }
}

// Go: compiler/emitter.go:311 declarationMapSource.FileName, Text, ECMALineMap
impl crate::sourcemap::source::Source for DeclarationMapSource {
    fn text(&self) -> &str {
        &self.text
    }

    fn file_name(&self) -> &str {
        self.file_name
    }

    fn ecma_line_map(&self) -> &[i32] {
        &self.line_map
    }
}

// Go: compiler/emitter.go:92 getModuleTransformer
fn get_module_transformer(opts: &TransformOptions) -> Option<TransformerBox> {
    use crate::transformers::moduletransforms;
    match opts.compiler_options.get_emit_module_kind() {
        ModuleKind::PRESERVE => {
            // `ESModuleTransformer` contains logic for preserving CJS input syntax in `--module preserve`
            into_transformer(moduletransforms::new_es_module_transformer(opts))
        }

        ModuleKind::ES_NEXT
        | ModuleKind::ES2022
        | ModuleKind::ES2020
        | ModuleKind::ES2015
        | ModuleKind::NODE20
        | ModuleKind::NODE18
        | ModuleKind::NODE16
        | ModuleKind::NODE_NEXT
        | ModuleKind::COMMON_JS => {
            into_transformer(moduletransforms::new_implied_module_transformer(opts))
        }

        _ => into_transformer(moduletransforms::new_commonjs_module_transformer(opts)),
    }
}

/// Go appends each constructor result to `tx`. Constructors that never
/// return nil return a plain `TransformerBox`; the others return an
/// `Option`. This takes either.
// PORT: Go would panic later on a nil transformer; a `None` is skipped.
trait IntoTransformer {
    fn into_transformer(self) -> Option<TransformerBox>;
}

impl IntoTransformer for TransformerBox {
    fn into_transformer(self) -> Option<TransformerBox> {
        Some(self)
    }
}

impl IntoTransformer for Option<TransformerBox> {
    fn into_transformer(self) -> Option<TransformerBox> {
        self
    }
}

fn into_transformer(t: impl IntoTransformer) -> Option<TransformerBox> {
    t.into_transformer()
}

/// The choices Go `getScriptTransformers` makes from the options and the
/// file before it builds the transformers.
struct ScriptTransformChoice {
    import_elision_enabled: bool,
    jsx_transform_enabled: bool,
    /// The emit resolver (the checker) is the reference resolver. Else the
    /// binder reference resolver is.
    emit_resolver_references: bool,
}

// Go: compiler/emitter.go:114 getScriptTransformers (its first lines)
fn script_transform_choice(options: &CompilerOptions, source_file: Node) -> ScriptTransformChoice {
    // JS files don't use reference calculations as they don't do import elision, no need to calculate it
    let import_elision_enabled =
        !options.verbatim_module_syntax.is_true() && !is_in_js_file(source_file);
    let jsx_transform_enabled = options.get_jsx_transform_enabled()
        && source_file_info(source_file).language_variant == LanguageVariant::JSX;
    ScriptTransformChoice {
        import_elision_enabled,
        jsx_transform_enabled,
        emit_resolver_references: import_elision_enabled
            || jsx_transform_enabled
            || !options.get_isolated_modules()
            || options.emit_decorator_metadata.is_true(),
    }
}

/// PORT: not in Go. True when the script transforms of `source_file` (its JS
/// part) can call the emit resolver, which needs the file's checker. When
/// false, the JS part runs on the program's emit pool (`program_emit`).
///
/// The script transforms reach the checker in these places only:
/// - the emit resolver as the reference resolver, the import elision
///   transform (with `MarkLinkedReferencesRecursively`), the JSX transform,
///   decorator metadata and const enum inlining (all of them only when
///   `script_transform_choice` picks the emit resolver);
/// - `GetEnumMemberValue` for each member of an enum declaration (the
///   runtime syntax transform).
///
/// The module transforms pass a nil resolver to
/// `getExternalModuleNameLiteral`. The emit host of the pool has an emit
/// resolver that panics on every call, so a use this rule misses crashes the
/// run and cannot change an output.
pub fn js_emit_needs_checker(source_file: Node) -> bool {
    script_transform_choice(options(), source_file).emit_resolver_references
        || may_have_enum_declaration(source_file)
}

/// PORT: not in Go. True when `source_file` may have an `EnumDeclaration`.
/// The binder gives every enum declaration a symbol with an enum flag
/// (`bind_enum_declaration`), so this scans the symbols in the node records
/// of the file and its binder extras (local symbols) instead of walking its
/// tree. A merged symbol (an enum and a namespace of one name) also counts,
/// which only keeps a JS part on the checker thread. It is true for a file
/// that is not bound.
fn may_have_enum_declaration(source_file: Node) -> bool {
    let file = source_file.file_index();
    let go_file = crate::ast::go_file(file);
    let (Some(node_bind), Some(symbols)) = (
        go_file.node_bind.get(),
        crate::program::bound_symbols_of(prog()),
    ) else {
        return true;
    };
    let is_enum = |symbol: SymbolId| {
        symbol.is_some() && symbols.sym(symbol).flags.intersects(SymbolFlags::ENUM)
    };
    crate::ast::frozen_store_any_symbol(file, is_enum).unwrap_or(true)
        || node_bind
            .extras()
            .iter()
            .any(|extra| is_enum(extra.local_symbol))
}

// Go: compiler/emitter.go:114 getScriptTransformers
// ts#64649: takes the emit resolver, and reads the emit context from it. Go
// panics when the resolver or its context is nil; neither is nil here.
pub fn get_script_transformers(
    emit_resolver: Rc<dyn EmitResolver>,
    source_file: Node,
) -> Vec<TransformerBox> {
    use crate::transformers::{estransforms, inliners, jsxtransforms, tstransforms};

    let emit_context = emit_resolver.emit_context().clone();

    let mut tx: Vec<TransformerBox> = Vec::new();
    let options = options();

    let ScriptTransformChoice {
        import_elision_enabled,
        jsx_transform_enabled,
        emit_resolver_references,
    } = script_transform_choice(options, source_file);

    let reference_resolver: Rc<dyn TransformReferenceResolver> = if emit_resolver_references {
        Rc::new(EmitResolverReferenceResolver(emit_resolver.clone()))
    } else {
        Rc::new(new_binder_reference_resolver(options))
    };

    let opts = TransformOptions {
        context: emit_context,
        compiler_options: options,
        resolver: reference_resolver,
        emit_resolver,
        get_emit_module_format_of_file: Rc::new(|file| {
            get_emit_module_format_of_file(parsed_source_file(file))
        }),
    };

    let mut push = |t: Option<TransformerBox>| {
        if let Some(t) = t {
            tx.push(t);
        }
    };

    // transform TypeScript syntax
    {
        // use type nodes to add metadata decorators
        if options.emit_decorator_metadata.is_true() {
            push(into_transformer(tstransforms::new_metadata_transformer(
                &opts,
            )));
        }

        // erase types
        push(into_transformer(tstransforms::new_type_eraser_transformer(
            &opts,
        )));

        // elide imports
        if import_elision_enabled {
            push(into_transformer(
                tstransforms::new_import_elision_transformer(&opts),
            ));
        }

        // transform `enum`, `namespace`, and parameter properties
        push(into_transformer(
            tstransforms::new_runtime_syntax_transformer(&opts),
        ));

        if options.experimental_decorators.is_true() {
            push(into_transformer(
                tstransforms::new_legacy_decorators_transformer(&opts),
            ));
        }
    }

    if jsx_transform_enabled {
        push(into_transformer(jsxtransforms::new_jsx_transformer(&opts)));
    }

    let downleveler = into_transformer(estransforms::get_es_transformer(&opts));
    if downleveler.is_some() {
        push(downleveler);
    }

    push(into_transformer(estransforms::new_use_strict_transformer(
        &opts,
    )));

    // transform module syntax
    push(get_module_transformer(&opts));

    // inlining (formerly done via substitutions)
    if !options.get_isolated_modules() {
        push(into_transformer(
            inliners::new_const_enum_inlining_transformer(&opts),
        ));
    }
    tx
}

/// The parsed SourceFile for Go `ast.HasFileName` callbacks.
// PORT: Go `GetEmitModuleFormatOfFile(file ast.HasFileName)` reads only the
// file name and path, which a transformed (factory) SourceFile copies from
// its original. The program functions read parsed files only, so a factory
// SourceFile maps to the parsed file with the same path.
pub fn parsed_source_file(file: Node) -> Node {
    if is_synthetic_node(file) {
        let path = with_synthetic_source_file(file, |d| d.path.clone());
        return get_source_file_by_path(&path);
    }
    file
}

// Go: compiler/emitter.go:401 shouldEmitSourceMaps
fn should_emit_source_maps(map_options: &CompilerOptions, source_file: Node) -> bool {
    (map_options.source_map.is_true() || map_options.inline_source_map.is_true())
        && !file_extension_is(source_file_file_name(source_file), ".json")
}

// Go: compiler/emitter.go:406 getSourceRoot
fn get_source_root(map_options: &CompilerOptions) -> String {
    // Normalize source root and make sure it has trailing "/" so that it can be used to combine paths with the
    // relative paths of the sources list in the sourcemap
    let mut source_root = normalize_slashes(&map_options.source_root);
    if !source_root.is_empty() {
        source_root = ensure_trailing_directory_separator(&source_root);
    }
    source_root
}

// Go: stringutil/util.go:144 EncodeURI
// PORT: Go escapes the bytes of the Go string. `s` is a port form (see
// `scanner_util::GO_STRING_MARKER`), so this escapes its Go bytes.
pub fn encode_uri(s: &str) -> String {
    const UPPERHEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut builder = String::with_capacity(s.len());
    for &b in go_string_bytes(s).iter() {
        if !should_escape_for_encode_uri(b) {
            builder.push(b as char);
            continue;
        }
        builder.push('%');
        builder.push(UPPERHEX[(b >> 4) as usize] as char);
        builder.push(UPPERHEX[(b & 0x0f) as usize] as char);
    }
    builder
}

// Go: stringutil/util.go:164 shouldEscapeForEncodeURI
fn should_escape_for_encode_uri(b: u8) -> bool {
    if b.is_ascii_alphanumeric() {
        return false;
    }
    !matches!(
        b,
        b';' | b'/'
            | b'?'
            | b':'
            | b'@'
            | b'&'
            | b'='
            | b'+'
            | b'$'
            | b','
            | b'#'
            | b'-'
            | b'_'
            | b'.'
            | b'!'
            | b'~'
            | b'*'
            | b'\''
            | b'('
            | b')'
    )
}

// Go: stringutil/util.go:215 AddUTF8ByteOrderMark
// PORT: a Rust `String` is UTF-8, so only the UTF-8 mark can be present.
pub fn add_utf8_byte_order_mark(text: String) -> String {
    if text.starts_with('\u{FEFF}') {
        return text;
    }
    format!("\u{FEFF}{text}")
}

/// A step of a file's emit where `set_emit_test_panic` makes it panic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EmitTestPanic {
    /// After the declaration transforms of the file, on the thread that runs
    /// them (its checker thread).
    DeclarationTransforms,
    /// At the start of the JS print of the file, on the thread that prints
    /// it (its checker thread, the emit pool or the twin of its checker).
    JsPrint,
}

/// The panic that `set_emit_test_panic` set: the step and the end of the
/// file name.
static EMIT_TEST_PANIC: std::sync::Mutex<Option<(EmitTestPanic, String)>> =
    std::sync::Mutex::new(None);

/// True while `EMIT_TEST_PANIC` holds a panic, so the emit reads only this
/// flag when no test set one.
static EMIT_TEST_PANIC_ON: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// PORT: not in Go. Tests: makes the emit panic at `step` in each file whose
/// name ends with the given text, or with `None` stops it. The panic tests
/// of the twins use it (`tests/emit_pool.rs`).
pub fn set_emit_test_panic(panic: Option<(EmitTestPanic, &str)>) {
    let mut set = EMIT_TEST_PANIC
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *set = panic.map(|(step, file_name_end)| (step, file_name_end.to_string()));
    EMIT_TEST_PANIC_ON.store(set.is_some(), std::sync::atomic::Ordering::Relaxed);
}

/// Panics when `set_emit_test_panic` set `step` for `source_file`.
fn emit_test_panic(step: EmitTestPanic, source_file: Node) {
    if !EMIT_TEST_PANIC_ON.load(std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    let file_name = source_file_file_name(source_file);
    let hit = EMIT_TEST_PANIC
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
        .is_some_and(|(at, end)| *at == step && file_name.ends_with(end.as_str()));
    if hit {
        panic!("emit test panic: {step:?} in {file_name}");
    }
}
