//! Port of Go `internal/transformers/declarations/supplementalreferences.go`
//! (tsgo#4712).

use super::DeclarationEmitHost;
use crate::checker::checker_p17::tspath_p17;
use crate::frontend::tspath;
use crate::prelude::*;

// Go: transformers/declarations/supplementalreferences.go:12 SupplementalReferencesTransformer
/// SupplementalReferencesTransformer adds triple-slash path references from a content mapper's
/// canonical declaration output to the declaration files emitted for its supplemental files.
/// This ensures that consumers loading the canonical declaration also include the supplemental types.
pub struct SupplementalReferencesTransformer {
    host: Rc<dyn DeclarationEmitHost>,
    supplemental_files: Vec<Node>,
    declaration_file_path: String,
    force_declaration_paths: bool,
}

// Go: transformers/declarations/supplementalreferences.go:19 NewSupplementalReferencesTransformer
// PORT: Go takes the source file and reads `sourceFile.SupplementalSourceFiles()`.
// The emitter holds the source file as a node, and the content mapper info is
// on the parsed file, so the caller passes the supplemental files (their
// `SourceFile` nodes, in Go order) instead of the source file.
#[must_use]
pub fn new_supplemental_references_transformer(
    host: Rc<dyn DeclarationEmitHost>,
    supplemental_files: Vec<Node>,
    declaration_file_path: &str,
    force_declaration_paths: bool,
) -> SupplementalReferencesTransformer {
    SupplementalReferencesTransformer {
        host,
        supplemental_files,
        declaration_file_path: declaration_file_path.to_string(),
        force_declaration_paths,
    }
}

impl SupplementalReferencesTransformer {
    // Go: transformers/declarations/supplementalreferences.go:28 (*SupplementalReferencesTransformer).TransformSourceFile
    // PORT: Go appends to `sourceFile.ReferencedFiles`. Here the file is the
    // factory SourceFile that the declaration transformer made. The write
    // happens only when a reference is added, so a file with no emitted
    // supplemental files (every file without a content mapper) is not touched.
    pub fn transform_source_file(&self, source_file: Node) -> Node {
        for &supplemental in &self.supplemental_files {
            if !self
                .host
                .source_file_may_be_emitted(supplemental, self.force_declaration_paths)
            {
                continue;
            }
            let declaration_path = self
                .host
                .get_output_paths_for(supplemental, self.force_declaration_paths)
                .declaration_file_path();
            if declaration_path.is_empty() {
                continue;
            }
            // ts#64159 (supplementalreferences.go:37): both outputs must share a root.
            let Some(relative_path) = tspath_p17::relative_path_from_file(
                &self.declaration_file_path,
                &declaration_path,
                self.host.use_case_sensitive_file_names(),
            ) else {
                panic!(
                    "supplemental declaration output must share a root with the primary declaration"
                );
            };
            let file_name = tspath::ensure_path_is_non_module_name(&relative_path);
            update_synthetic_source_file(source_file, |d| {
                d.referenced_files.push(FileReference {
                    range: TextRange::new(-1, -1),
                    file_name,
                    ..FileReference::default()
                });
            });
        }
        source_file
    }

    // Go: transformers/declarations/supplementalreferences.go:52 (*SupplementalReferencesTransformer).GetDiagnostics
    #[must_use]
    pub fn get_diagnostics(&self) -> Vec<Diagnostic> {
        Vec::new()
    }
}
