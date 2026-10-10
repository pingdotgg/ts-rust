//! Port of tspath/extension.go.

use crate::frontend::prelude::*;

// Go: tspath/extension.go:8 Extension constants
pub const EXTENSION_TS: &str = ".ts";
pub const EXTENSION_TSX: &str = ".tsx";
pub const EXTENSION_DTS: &str = ".d.ts";
pub const EXTENSION_JS: &str = ".js";
pub const EXTENSION_JSX: &str = ".jsx";
pub const EXTENSION_JSON: &str = ".json";
pub const EXTENSION_TS_BUILD_INFO: &str = ".tsbuildinfo";
pub const EXTENSION_MJS: &str = ".mjs";
pub const EXTENSION_MTS: &str = ".mts";
pub const EXTENSION_DMTS: &str = ".d.mts";
pub const EXTENSION_CJS: &str = ".cjs";
pub const EXTENSION_CTS: &str = ".cts";
pub const EXTENSION_DCTS: &str = ".d.cts";

// Go: tspath/extension.go:24 extension lists
// PORT: Go `slices.Concat` values are written out as constant lists with the
// same order.
pub const SUPPORTED_DECLARATION_EXTENSIONS: &[&str] =
    &[EXTENSION_DTS, EXTENSION_DCTS, EXTENSION_DMTS];
pub const SUPPORTED_TS_IMPLEMENTATION_EXTENSIONS: &[&str] =
    &[EXTENSION_TS, EXTENSION_TSX, EXTENSION_MTS, EXTENSION_CTS];
const SUPPORTED_TS_EXTENSIONS_FOR_EXTRACT_EXTENSION: &[&str] = &[
    EXTENSION_DTS,
    EXTENSION_DCTS,
    EXTENSION_DMTS,
    EXTENSION_TS,
    EXTENSION_TSX,
    EXTENSION_MTS,
    EXTENSION_CTS,
];
pub const ALL_SUPPORTED_EXTENSIONS: &[&[&str]] = &[
    &[
        EXTENSION_TS,
        EXTENSION_TSX,
        EXTENSION_DTS,
        EXTENSION_JS,
        EXTENSION_JSX,
    ],
    &[EXTENSION_CTS, EXTENSION_DCTS, EXTENSION_CJS],
    &[EXTENSION_MTS, EXTENSION_DMTS, EXTENSION_MJS],
];
pub const SUPPORTED_TS_EXTENSIONS: &[&[&str]] = &[
    &[EXTENSION_TS, EXTENSION_TSX, EXTENSION_DTS],
    &[EXTENSION_CTS, EXTENSION_DCTS],
    &[EXTENSION_MTS, EXTENSION_DMTS],
];
pub const SUPPORTED_TS_EXTENSIONS_FLAT: &[&str] = &[
    EXTENSION_TS,
    EXTENSION_TSX,
    EXTENSION_DTS,
    EXTENSION_CTS,
    EXTENSION_DCTS,
    EXTENSION_MTS,
    EXTENSION_DMTS,
];
pub const SUPPORTED_JS_EXTENSIONS: &[&[&str]] = &[
    &[EXTENSION_JS, EXTENSION_JSX],
    &[EXTENSION_MJS],
    &[EXTENSION_CJS],
];
pub const SUPPORTED_JS_EXTENSIONS_FLAT: &[&str] =
    &[EXTENSION_JS, EXTENSION_JSX, EXTENSION_MJS, EXTENSION_CJS];
pub const ALL_SUPPORTED_EXTENSIONS_WITH_JSON: &[&[&str]] = &[
    &[
        EXTENSION_TS,
        EXTENSION_TSX,
        EXTENSION_DTS,
        EXTENSION_JS,
        EXTENSION_JSX,
    ],
    &[EXTENSION_CTS, EXTENSION_DCTS, EXTENSION_CJS],
    &[EXTENSION_MTS, EXTENSION_DMTS, EXTENSION_MJS],
    &[EXTENSION_JSON],
];
pub const SUPPORTED_TS_EXTENSIONS_WITH_JSON: &[&[&str]] = &[
    &[EXTENSION_TS, EXTENSION_TSX, EXTENSION_DTS],
    &[EXTENSION_CTS, EXTENSION_DCTS],
    &[EXTENSION_MTS, EXTENSION_DMTS],
    &[EXTENSION_JSON],
];
pub const SUPPORTED_TS_EXTENSIONS_WITH_JSON_FLAT: &[&str] = &[
    EXTENSION_TS,
    EXTENSION_TSX,
    EXTENSION_DTS,
    EXTENSION_CTS,
    EXTENSION_DCTS,
    EXTENSION_MTS,
    EXTENSION_DMTS,
    EXTENSION_JSON,
];
pub const EXTENSIONS_NOT_SUPPORTING_EXTENSIONLESS_RESOLUTION: &[&str] = &[
    EXTENSION_MTS,
    EXTENSION_DMTS,
    EXTENSION_MJS,
    EXTENSION_CTS,
    EXTENSION_DCTS,
    EXTENSION_CJS,
];

// Go: tspath/extension.go:39 ExtensionIsTs
pub fn extension_is_ts(ext: &str) -> bool {
    let b = ext.as_bytes();
    ext == EXTENSION_TS
        || ext == EXTENSION_TSX
        || ext == EXTENSION_DTS
        || ext == EXTENSION_MTS
        || ext == EXTENSION_DMTS
        || ext == EXTENSION_CTS
        || ext == EXTENSION_DCTS
        || b.len() >= 7 && &b[..3] == b".d." && &b[b.len() - 3..] == b".ts"
}

// Go: tspath/extension.go:43 extensionsToRemove
const EXTENSIONS_TO_REMOVE: &[&str] = &[
    EXTENSION_DTS,
    EXTENSION_DMTS,
    EXTENSION_DCTS,
    EXTENSION_MJS,
    EXTENSION_MTS,
    EXTENSION_CJS,
    EXTENSION_CTS,
    EXTENSION_TS,
    EXTENSION_JS,
    EXTENSION_TSX,
    EXTENSION_JSX,
    EXTENSION_JSON,
];

// Go: tspath/extension.go:45 RemoveFileExtension
pub fn remove_file_extension(path: &str) -> &str {
    // Remove any known extension even if it has more than one dot
    for ext in EXTENSIONS_TO_REMOVE {
        if let Some(stripped) = path.strip_suffix(ext) {
            return stripped;
        }
    }

    path
}

// Go: tspath/extension.go:56 RemoveAnyFileExtension (tsgo#4712)
/// Removes a known extension, or else any extension after the last ".".
pub fn remove_any_file_extension(path: &str) -> &str {
    let without_extension = remove_file_extension(path);
    if without_extension != path {
        return without_extension;
    }
    let extension = get_any_extension_from_path(path, &[], false);
    if !extension.is_empty() {
        return remove_extension(path, &extension);
    }
    path
}

// Go: tspath/extension.go:66 TryGetExtensionFromPath
pub fn try_get_extension_from_path(p: &str) -> &'static str {
    for ext in EXTENSIONS_TO_REMOVE {
        if file_extension_is(p, ext) {
            return ext;
        }
    }
    ""
}

// Go: tspath/extension.go:75 RemoveExtension
pub fn remove_extension<'a>(path: &'a str, extension: &str) -> &'a str {
    &path[..path.len() - extension.len()]
}

// Go: tspath/extension.go:79 FileExtensionIsOneOf
pub fn file_extension_is_one_of(path: &str, extensions: &[&str]) -> bool {
    for ext in extensions {
        if file_extension_is(path, ext) {
            return true;
        }
    }
    false
}

// Go: tspath/extension.go:88 TryExtractTSExtension
pub fn try_extract_ts_extension(file_name: &str) -> &'static str {
    for ext in SUPPORTED_TS_EXTENSIONS_FOR_EXTRACT_EXTENSION {
        if file_extension_is(file_name, ext) {
            return ext;
        }
    }
    ""
}

// Go: tspath/extension.go:97 HasTSFileExtension
pub fn has_ts_file_extension(path: &str) -> bool {
    file_extension_is_one_of(path, SUPPORTED_TS_EXTENSIONS_FLAT)
}

// Go: tspath/extension.go:101 HasImplementationTSFileExtension
pub fn has_implementation_ts_file_extension(path: &str) -> bool {
    file_extension_is_one_of(path, SUPPORTED_TS_IMPLEMENTATION_EXTENSIONS)
        && !is_declaration_file_name(path)
}

// Go: tspath/extension.go:105 HasJSFileExtension
pub fn has_js_file_extension(path: &str) -> bool {
    file_extension_is_one_of(path, SUPPORTED_JS_EXTENSIONS_FLAT)
}

// Go: tspath/extension.go:109 HasJSONFileExtension
pub fn has_json_file_extension(path: &str) -> bool {
    file_extension_is(path, EXTENSION_JSON)
}

// Go: tspath/extension.go:113 IsDeclarationFileName
pub fn is_declaration_file_name(file_name: &str) -> bool {
    !get_declaration_file_extension(file_name).is_empty()
}

// Go: tspath/extension.go:117 ExtensionIsOneOf
pub fn extension_is_one_of(ext: &str, extensions: &[&str]) -> bool {
    extensions.contains(&ext)
}

// Go: tspath/extension.go:121 GetDeclarationFileExtension
pub fn get_declaration_file_extension(file_name: &str) -> String {
    let base = get_base_file_name(file_name);
    for ext in SUPPORTED_DECLARATION_EXTENSIONS {
        if base.ends_with(ext) {
            return ext.to_string();
        }
    }
    if base.ends_with(EXTENSION_TS) {
        if let Some(index) = base.find(".d.") {
            return base[index..].to_string();
        }
    }
    String::new()
}

// Go: tspath/extension.go:141 GetDeclarationEmitExtensionForPath
pub fn get_declaration_emit_extension_for_path(path: &str) -> String {
    if file_extension_is_one_of(path, &[EXTENSION_MJS, EXTENSION_MTS]) {
        EXTENSION_DMTS.to_string()
    } else if file_extension_is_one_of(path, &[EXTENSION_CJS, EXTENSION_CTS]) {
        EXTENSION_DCTS.to_string()
    } else if file_extension_is_one_of(
        path,
        &[EXTENSION_TS, EXTENSION_TSX, EXTENSION_JS, EXTENSION_JSX],
    ) {
        EXTENSION_DTS.to_string()
    } else {
        let ext = get_any_extension_from_path(path, &[], false);
        if !ext.is_empty() {
            return format!(".d{ext}.ts");
        }
        EXTENSION_DTS.to_string()
    }
}

// Go: tspath/extension.go:163 ChangeAnyExtension
// ChangeAnyExtension changes the extension of a path to the provided extension if it has one of the provided extensions.
//
// ChangeAnyExtension("/path/to/file.ext", ".js", ".ext") === "/path/to/file.js"
// ChangeAnyExtension("/path/to/file.ext", ".js", ".ts") === "/path/to/file.ext"
// ChangeAnyExtension("/path/to/file.ext", ".js", [".ext", ".ts"]) === "/path/to/file.js"
pub fn change_any_extension(
    path: &str,
    ext: &str,
    extensions: &[&str],
    ignore_case: bool,
) -> String {
    let pathext = get_any_extension_from_path(path, extensions, ignore_case);
    if !pathext.is_empty() {
        let result = &path[..path.len() - pathext.len()];
        if ext.is_empty() {
            return result.to_string();
        }
        if ext.starts_with('.') {
            return format!("{result}{ext}");
        }
        return format!("{result}.{ext}");
    }
    path.to_string()
}

// Go: tspath/extension.go:178 ChangeExtension
pub fn change_extension(path: &str, new_extension: &str) -> String {
    change_any_extension(
        path,
        new_extension,
        EXTENSIONS_TO_REMOVE,
        false, /*ignoreCase*/
    )
}

// Go: tspath/extension.go:187 ChangeFullExtension
// Like `changeAnyExtension`, but declaration file extensions are recognized
// and replaced starting from the `.d`.
//
//	changeAnyExtension("file.d.ts", ".js") === "file.d.js"
//	changeFullExtension("file.d.ts", ".js") === "file.js"
pub fn change_full_extension(path: &str, new_extension: &str) -> String {
    let declaration_extension = get_declaration_file_extension(path);
    if !declaration_extension.is_empty() {
        let ext = if !new_extension.starts_with('.') {
            format!(".{new_extension}")
        } else {
            new_extension.to_string()
        };
        return format!("{}{ext}", &path[..path.len() - declaration_extension.len()]);
    }
    change_extension(path, new_extension)
}

// Go: tspath/extension.go:202 GetPossibleOriginalInputExtensionForExtension
pub fn get_possible_original_input_extension_for_extension(path: &str) -> Vec<String> {
    if file_extension_is_one_of(path, &[EXTENSION_DMTS, EXTENSION_MJS, EXTENSION_MTS]) {
        return vec![EXTENSION_MTS.to_string(), EXTENSION_MJS.to_string()];
    }
    if file_extension_is_one_of(path, &[EXTENSION_DCTS, EXTENSION_CJS, EXTENSION_CTS]) {
        return vec![EXTENSION_CTS.to_string(), EXTENSION_CJS.to_string()];
    }
    // Handle any custom .d.x.ts extension (e.g., .d.json.ts -> .json, .d.css.ts -> .css)
    let ext = get_declaration_file_extension(path);
    if !ext.is_empty() && ext != EXTENSION_DTS {
        let inner = &ext[".d.".len()..ext.len() - ".ts".len()];
        return vec![format!(".{inner}")];
    }
    vec![
        EXTENSION_TSX.to_string(),
        EXTENSION_TS.to_string(),
        EXTENSION_JSX.to_string(),
        EXTENSION_JS.to_string(),
    ]
}
