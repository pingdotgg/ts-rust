//! Go `core` and `ast` pieces that only the frontend needs.

use crate::frontend::prelude::*;

// Go: core/options_generated.go:853 TypeAcquisition (ts#64457 generates it)
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TypeAcquisition {
    pub enable: Tristate,
    pub include: Vec<String>,
    pub exclude: Vec<String>,
    pub disable_filename_based_type_acquisition: Tristate,
}

// Go: core/projectreference.go:5 ProjectReference
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProjectReference {
    // Path is a normalized path on disk.
    pub path: String,
    // OriginalPath is the path as it was originally written.
    pub original_path: String,
    // Circular indicates that this reference is intended to form a circularity.
    pub circular: bool,
}

// Go: core/projectreference.go:5 ProjectReference (JSON v2 struct marshaler)
// PORT: Go marshals the struct by reflection in field order with the tags
// `json:"path"`, `json:"originalPath,omitempty"` and
// `json:"circular,omitempty"` (tsgo#4627, ts#64326). v2 `omitempty` drops an
// empty string but writes `false`.
impl MarshalerTo for ProjectReference {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        enc.push_str("{\"path\":");
        self.path.marshal_json_to(enc)?;
        // `path` is written, so each later member needs a comma.
        let mut first = false;
        crate::options_json::marshal_field_omitempty(
            enc,
            &mut first,
            "originalPath",
            &self.original_path,
        )?;
        crate::options_json::marshal_field_omitempty(enc, &mut first, "circular", &self.circular)?;
        enc.push('}');
        Ok(())
    }
}

// Go: core/projectreference.go:14 ResolveProjectReferencePath
pub fn resolve_project_reference_path(r: &ProjectReference) -> String {
    resolve_config_file_name_of_project_reference(&r.path)
}

// Go: core/projectreference.go:18 ResolveConfigFileNameOfProjectReference
pub fn resolve_config_file_name_of_project_reference(path: &str) -> String {
    if file_extension_is(path, EXTENSION_JSON) {
        return path.to_string();
    }
    combine_paths(path, &["tsconfig.json"])
}

/// Go interface `ast.HasFileName`.
pub trait HasFileName {
    fn file_name(&self) -> String;
    fn path(&self) -> Path;
}

impl HasFileName for HasFileNameImpl {
    fn file_name(&self) -> String {
        self.file_name.clone()
    }
    fn path(&self) -> Path {
        Path(self.path.clone())
    }
}

// Go: ast.SourceFile implements `ast.HasFileName`.
impl HasFileName for ParsedSourceFile {
    fn file_name(&self) -> String {
        ParsedSourceFile::file_name(self).to_string()
    }
    fn path(&self) -> Path {
        ParsedSourceFile::path(self).clone()
    }
}

// Go: core/core.go:525 GetScriptKindFromFileName
pub fn get_script_kind_from_file_name(file_name: &str) -> ScriptKind {
    if let Some(dot_pos) = file_name.rfind('.') {
        let ext = file_name[dot_pos..].to_lowercase();
        match ext.as_str() {
            EXTENSION_JS | EXTENSION_CJS | EXTENSION_MJS => return ScriptKind::JS,
            EXTENSION_JSX => return ScriptKind::JSX,
            EXTENSION_TS | EXTENSION_CTS | EXTENSION_MTS => return ScriptKind::TS,
            EXTENSION_TSX => return ScriptKind::TSX,
            EXTENSION_JSON => return ScriptKind::JSON,
            _ => {}
        }
    }
    ScriptKind::UNKNOWN
}

// Go: core/core.go:544 GetDefaultExtensionForScriptKind (tsgo#4712)
#[must_use]
pub fn get_default_extension_for_script_kind(script_kind: ScriptKind) -> &'static str {
    match script_kind {
        ScriptKind::JS => EXTENSION_JS,
        ScriptKind::JSX => EXTENSION_JSX,
        ScriptKind::TSX => EXTENSION_TSX,
        ScriptKind::JSON => EXTENSION_JSON,
        _ => EXTENSION_TS,
    }
}

// EnsureScriptKindFromFileName is like GetScriptKindFromFileName, but defaults to
// ScriptKindTS when the file name has no recognized extension (e.g. files included
// with allowNonTsExtensions), so the result is always safe to hand to the parser.
// Go: core/core.go:562 EnsureScriptKindFromFileName
pub fn ensure_script_kind_from_file_name(file_name: &str) -> ScriptKind {
    let kind = get_script_kind_from_file_name(file_name);
    if kind != ScriptKind::UNKNOWN {
        return kind;
    }
    ScriptKind::TS
}
