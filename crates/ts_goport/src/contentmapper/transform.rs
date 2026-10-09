//! Port of Go `internal/contentmapper/transform.go` (tsgo#4712).
//!
//! PORT: Go `*ast.SourceFile` is `Rc<ParsedSourceFile>` here, as in the
//! compiler host (`frontend/compiler/host.rs`). Go `SetContentMapperInfo` is
//! `ParsedSourceFile::set_content_mapper_info`. The canonical file holds its
//! supplemental files as `Rc`, and each supplemental file points back with a
//! `Weak`, so the two links make no `Rc` cycle (see that method).

use crate::contentmapper::prelude::*;

use crate::frontend::core_ext::get_script_kind_from_file_name;
use crate::frontend::parser::source_file::ParsedSourceFile;
use crate::frontend::parser::{SourceFileParseOptions, parse_source_file};
use crate::frontend::tspath::Path;
use std::sync::Arc;

// Go: contentmapper/transform.go:14 SourceFiles
// SourceFiles is the canonical output and its unnamed supplemental compiler inputs.
// PORT: Go `Canonical` is nil in the zero value; that is `None`.
#[derive(Clone, Default)]
pub struct SourceFiles {
    pub canonical: Option<Rc<ParsedSourceFile>>,
    pub supplemental: Vec<Rc<ParsedSourceFile>>,
}

// Go: contentmapper/transform.go:26 TransformAndParse
// TransformAndParse runs the given content mapper's transform for a content-mapped source file and
// parses the resulting TypeScript, preserving the original file name and retaining the untransformed text
// on the source file. The mapper is supplied by the caller (which also owns the failure accounting) so it
// is neither re-resolved nor substituted here. It returns an error if the transform fails or the mapper
// produces invalid position mappings (a *spanmap.MappingError); the caller decides how to report the failure
// and what placeholder file to substitute. It is the shared implementation behind
// CompilerHost.GetContentMappedSourceFile.
pub fn transform_and_parse(
    parse_options: &SourceFileParseOptions,
    content: &str,
    mapper: &Rc<Mapper>,
    project: &dyn Project,
) -> std::result::Result<SourceFiles, GoError> {
    transform_and_parse_prefetched(parse_options, content, mapper, project, None)
}

/// What a parse worker made for a content-mapped file
/// (`files_parser::prefetch_mapped`): the result of its transform request,
/// as `Project::transform` returns it, and its parse of the virtual text
/// when the loader can use that parse.
// PORT: not in Go, where the parse goroutines transform (tsgo#4712).
pub struct PrefetchedTransform {
    pub result: std::result::Result<Result, GoError>,
    pub parse: Option<ParsedSourceFile>,
}

/// `transform_and_parse` with the transform that a parse worker sent
/// (`prefetched`) in place of the call of `project.transform`, so a file
/// gets one transform request, as in Go.
// PORT: not in Go (see `PrefetchedTransform`).
pub fn transform_and_parse_prefetched(
    parse_options: &SourceFileParseOptions,
    content: &str,
    mapper: &Rc<Mapper>,
    project: &dyn Project,
    prefetched: Option<PrefetchedTransform>,
) -> std::result::Result<SourceFiles, GoError> {
    let transform_identity = match project.identity(mapper) {
        Ok(transform_identity) => transform_identity,
        Err(err) => {
            return Err(new_transform_error(TransformErrorKind::PROJECT, Some(err)).to_go_error());
        }
    };
    let (result, parse) = match prefetched {
        Some(PrefetchedTransform { result, parse }) => (result?, parse),
        None => (
            project.transform(
                mapper,
                Request {
                    file_name: parse_options.file_name.clone(),
                    content: content.to_string(),
                },
            )?,
            None,
        ),
    };
    parse_result_with(
        parse_options,
        content,
        mapper,
        &transform_identity,
        result,
        parse,
    )
}

// Go: contentmapper/transform.go:47 ParseResult
// ParseResult validates and parses one mapper result and all its supplemental outputs.
pub fn parse_result(
    parse_options: &SourceFileParseOptions,
    content: &str,
    mapper: &Mapper,
    transform_identity: &str,
    result: Result,
) -> std::result::Result<SourceFiles, GoError> {
    parse_result_with(
        parse_options,
        content,
        mapper,
        transform_identity,
        result,
        None,
    )
}

/// Whether `parse_result` parses the canonical output of `result` for a
/// file whose text is `content`. Go `ParseResult` checks the mappings, then
/// the virtual extension, and parses only after both pass. A parse worker
/// parses the virtual text only then (`files_parser::prefetch_mapped`), so
/// no parse runs before those checks, as in Go.
// PORT: not in Go (see `PrefetchedTransform`).
pub(crate) fn parses_canonical_output(result: &Result, content: &str) -> bool {
    result.mappings.as_ref().is_some_and(|mappings| {
        spanmap::SpanMap::validate(Some(&**mappings), &result.text, content).is_none()
    }) && is_supported_virtual_extension(&result.virtual_extension)
}

/// `parse_result` that takes `parse` as the parse of the virtual text when
/// a parse worker made it (`PrefetchedTransform`).
// PORT: not in Go (see `PrefetchedTransform`).
fn parse_result_with(
    parse_options: &SourceFileParseOptions,
    content: &str,
    mapper: &Mapper,
    transform_identity: &str,
    result: Result,
    parse: Option<ParsedSourceFile>,
) -> std::result::Result<SourceFiles, GoError> {
    let Some(mappings) = result.mappings.clone() else {
        return Err(new_transform_error(TransformErrorKind::MAPPINGS, None).to_go_error());
    };
    if let Some(problem) = spanmap::SpanMap::validate(Some(&*mappings), &result.text, content) {
        return Err(errors::from_value(problem));
    }
    let virtual_extension = result.virtual_extension.clone();
    if !is_supported_virtual_extension(&virtual_extension) {
        return Err(new_transform_error(TransformErrorKind::RESPONSE, None).to_go_error());
    }
    let base_parse_options = parse_options.clone();
    let virtual_file_name = format!("{}{}", base_parse_options.file_name, virtual_extension);
    let mut parse_options = base_parse_options.clone();
    if is_module_virtual_extension(&virtual_extension) {
        parse_options.external_module_indicator_options.force = true;
    }
    // PORT: a content-mapped parse never gets a `FileVersion`, so its store
    // is published static and its text is leaked, as the compiler host
    // leaks a static file text (`FileText::new`).
    let mut source_file = match parse {
        Some(parse) => parse,
        None => {
            let text: &'static str = Box::leak(result.text.clone().into_boxed_str());
            parse_source_file(
                &parse_options,
                text,
                get_script_kind_from_file_name(&virtual_file_name),
            )
        }
    };
    if !result.diagnostics.is_empty() {
        // The runner produces diagnostics without a source file (it doesn't have one yet); associate
        // them with the file now so they are reported against it.
        let mut diagnostics = source_file.diagnostics.clone();
        for diagnostic in &result.diagnostics {
            let mut diagnostic = diagnostic.clone();
            diagnostic.set_file(source_file.root);
            diagnostics.push(diagnostic);
        }
        // PORT: Go `sourceFile.SetDiagnostics(...)`. The parsed file keeps its
        // diagnostics in its `diagnostics` field and in the table that
        // `parse_source_file` fills; both change.
        source_file.diagnostics = diagnostics.clone();
        crate::frontend::parser::source_file::set_source_file_diagnostics(
            source_file.root,
            diagnostics,
        );
    }
    let source_file = Rc::new(source_file);
    let mut files = SourceFiles {
        canonical: Some(source_file.clone()),
        supplemental: Vec::with_capacity(result.supplemental.len()),
    };
    for (i, supplemental) in result.supplemental.iter().enumerate() {
        let Some(supplemental_mappings) = &supplemental.mappings else {
            return Err(new_transform_error(TransformErrorKind::MAPPINGS, None).to_go_error());
        };
        if let Some(problem) =
            spanmap::SpanMap::validate(Some(&**supplemental_mappings), &supplemental.text, content)
        {
            return Err(errors::from_value(problem));
        }
        let mut supplemental_options = base_parse_options.clone();
        if !is_supported_virtual_extension(&supplemental.virtual_extension) {
            return Err(new_transform_error(TransformErrorKind::RESPONSE, None).to_go_error());
        }
        let suffix = format!(".{i}{}", supplemental.virtual_extension);
        supplemental_options.file_name.push_str(&suffix);
        supplemental_options.path = Path(format!("{}{}", parse_options.path.0, suffix));
        if is_module_virtual_extension(&supplemental.virtual_extension) {
            supplemental_options.external_module_indicator_options.force = true;
        }

        let supplemental_text: &'static str = Box::leak(supplemental.text.clone().into_boxed_str());
        let file = parse_source_file(
            &supplemental_options,
            supplemental_text,
            get_script_kind_from_file_name(&supplemental_options.file_name),
        );

        files.supplemental.push(Rc::new(file));
    }
    let mapper_identity = mapper.identity();
    source_file.set_content_mapper_info(ast::ContentMapperSourceFileInfo {
        content_mapper: mapper_identity.clone(),
        transform_identity: transform_identity.to_string(),
        parse_options: base_parse_options.clone(),
        virtual_file_name,
        original_text: content.to_string(),
        span_map: result.mappings.clone(),
        diagnostic_directives: result.diagnostic_directives.clone(),
        supplemental_source_files: files.supplemental.clone(),
        canonical_source_file: None,
    });
    for (i, file) in files.supplemental.iter().enumerate() {
        let supplemental = &result.supplemental[i];
        file.set_content_mapper_info(ast::ContentMapperSourceFileInfo {
            content_mapper: mapper_identity.clone(),
            transform_identity: transform_identity.to_string(),
            parse_options: base_parse_options.clone(),
            virtual_file_name: file.file_name().to_string(),
            original_text: content.to_string(),
            span_map: supplemental.mappings.clone(),
            diagnostic_directives: supplemental.diagnostic_directives.clone(),
            supplemental_source_files: Vec::new(),
            canonical_source_file: Some(source_file.clone()),
        });
    }
    Ok(files)
}

// Go: contentmapper/transform.go:123 isModuleVirtualExtension
pub(crate) fn is_module_virtual_extension(extension: &str) -> bool {
    [
        tspath::EXTENSION_MTS,
        tspath::EXTENSION_CTS,
        tspath::EXTENSION_MJS,
        tspath::EXTENSION_CJS,
    ]
    .contains(&extension)
}

// Go: contentmapper/transform.go:128 CheckSupplementalFileNameCollisions
// CheckSupplementalFileNameCollisions rejects compiler-assigned virtual filenames that name physical files.
pub fn check_supplemental_file_name_collisions(
    files: &SourceFiles,
    file_exists: &dyn Fn(&str) -> bool,
) -> std::result::Result<(), GoError> {
    for file in &files.supplemental {
        if file_exists(file.file_name()) {
            return Err(SupplementalFileCollisionError {
                file_name: file.file_name().to_string(),
            }
            .to_go_error());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    // Go: contentmapper/transform_test.go
    use super::*;

    fn astro_parse_options() -> SourceFileParseOptions {
        SourceFileParseOptions {
            file_name: "/component.astro".to_string(),
            path: Path("/component.astro".to_string()),
            ..Default::default()
        }
    }

    // PORT: no Go counterpart (cmpar1). A parse worker parses the virtual
    // text only for a result that `parse_result` parses: Go checks the
    // mappings, then the virtual extension, before its parse
    // (transform.go:48-58).
    #[test]
    fn a_worker_parses_only_what_parse_result_parses() {
        let text = "export const a = 1;";
        let verbatim = |end: usize| {
            Some(spanmap::new(&[spanmap::Segment {
                virtual_end: end as i32,
                original_end: text.len() as i32,
                kind: spanmap::Kind::VERBATIM,
                ..Default::default()
            }]))
        };
        let result = |mappings: Option<spanmap::SpanMap>, extension: &str| Result {
            text: text.to_string(),
            virtual_extension: extension.to_string(),
            mappings: mappings.map(Arc::new),
            ..Default::default()
        };
        let cases = [
            (result(verbatim(text.len()), ".ts"), true),
            (result(Some(spanmap::new(&[])), ".mts"), true),
            // The mapping ends past the virtual text.
            (result(verbatim(text.len() + 1), ".ts"), false),
            (result(None, ".ts"), false),
            (result(verbatim(text.len()), ".vue"), false),
        ];
        for (i, (result, parses)) in cases.into_iter().enumerate() {
            assert_eq!(parses_canonical_output(&result, text), parses, "case {i}");
            let parsed = parse_result(&astro_parse_options(), text, &Mapper::default(), "", result);
            assert_eq!(parsed.is_ok(), parses, "case {i}");
        }
    }

    // Go: transform_test.go:14 TestParseResultSupplementalFileExtensions
    #[test]
    fn test_parse_result_supplemental_file_extensions() {
        let mappings = Some(Arc::new(spanmap::new(&[])));
        let mapped = |extension: &str| MappedResult {
            virtual_extension: extension.to_string(),
            mappings: mappings.clone(),
            ..Default::default()
        };
        let result = Result {
            virtual_extension: ".ts".to_string(),
            mappings: mappings.clone(),
            supplemental: vec![
                mapped(".js"),
                mapped(".jsx"),
                mapped(".ts"),
                mapped(".tsx"),
                mapped(".mts"),
                mapped(".cts"),
                mapped(".json"),
            ],
            ..Default::default()
        };
        let mapper = Mapper {
            definition: Definition {
                extensions: vec![".astro".to_string()],
                ..Default::default()
            },
            manifest: Manifest {
                name: "mapper".to_string(),
                ..Default::default()
            },
            ..Default::default()
        };
        let files = parse_result(
            &astro_parse_options(),
            "",
            &mapper,
            "transform-identity",
            result,
        )
        .expect("parse result");
        let canonical = files.canonical.clone().expect("canonical file");
        assert_eq!(
            canonical.content_mapper_transform_identity(),
            "transform-identity"
        );
        let canonical_supplementals = canonical.supplemental_source_files();
        assert_eq!(canonical_supplementals.len(), files.supplemental.len());

        let expected: [(&str, ScriptKind); 7] = [
            ("/component.astro.0.js", ScriptKind::JS),
            ("/component.astro.1.jsx", ScriptKind::JSX),
            ("/component.astro.2.ts", ScriptKind::TS),
            ("/component.astro.3.tsx", ScriptKind::TSX),
            ("/component.astro.4.mts", ScriptKind::TS),
            ("/component.astro.5.cts", ScriptKind::TS),
            ("/component.astro.6.json", ScriptKind::JSON),
        ];
        assert_eq!(files.supplemental.len(), expected.len());
        for (i, (file_name, script_kind)) in expected.iter().enumerate() {
            let file = &files.supplemental[i];
            assert_eq!(file.file_name(), *file_name);
            assert_eq!(*file.path(), Path((*file_name).to_string()));
            assert_eq!(file.script_kind, *script_kind);
            assert_eq!(
                file.content_mapper_transform_identity(),
                "transform-identity"
            );
            assert!(Rc::ptr_eq(&canonical_supplementals[i], file));
            let file_canonical = file.canonical_source_file().expect("canonical source file");
            assert!(Rc::ptr_eq(&file_canonical, &canonical));
        }
    }

    // Go: transform_test.go:65 TestParseResultAllowsSupplementalModules
    #[test]
    fn test_parse_result_allows_supplemental_modules() {
        let mappings = Some(Arc::new(spanmap::new(&[])));
        let mapper = Mapper {
            definition: Definition {
                extensions: vec![".astro".to_string()],
                ..Default::default()
            },
            manifest: Manifest {
                name: "mapper".to_string(),
                ..Default::default()
            },
            ..Default::default()
        };
        let files = parse_result(
            &astro_parse_options(),
            "",
            &mapper,
            "",
            Result {
                text: "export {};".to_string(),
                virtual_extension: ".ts".to_string(),
                mappings: mappings.clone(),
                supplemental: vec![MappedResult {
                    text: "export const value = 1;".to_string(),
                    virtual_extension: ".mts".to_string(),
                    mappings: mappings,
                    ..Default::default()
                }],
                ..Default::default()
            },
        )
        .expect("parse result");
        // PORT: Go `ast.IsExternalModule(file)` reads the parsed file's
        // indicator; `ast::is_external_module` reads the published store, and
        // this file is not published.
        assert!(files.supplemental[0].external_module_indicator.is_some());
    }

    // Go: transform_test.go:88 TestParseResultDoesNotLeakCanonicalModuleForcingToSupplementals
    #[test]
    fn test_parse_result_does_not_leak_canonical_module_forcing_to_supplementals() {
        let mappings = Some(Arc::new(spanmap::new(&[])));
        let mapper = Mapper {
            manifest: Manifest {
                name: "mapper".to_string(),
                ..Default::default()
            },
            ..Default::default()
        };
        let files = parse_result(
            &astro_parse_options(),
            "",
            &mapper,
            "",
            Result {
                text: "const canonical = 1;".to_string(),
                virtual_extension: ".mts".to_string(),
                mappings: mappings.clone(),
                supplemental: vec![MappedResult {
                    text: "const supplemental = 1;".to_string(),
                    virtual_extension: ".ts".to_string(),
                    mappings: mappings,
                    ..Default::default()
                }],
                ..Default::default()
            },
        )
        .expect("parse result");
        let canonical = files.canonical.expect("canonical file");
        assert!(
            canonical
                .parse_options()
                .external_module_indicator_options
                .force
        );
        assert!(
            !files.supplemental[0]
                .parse_options()
                .external_module_indicator_options
                .force
        );
    }
}
