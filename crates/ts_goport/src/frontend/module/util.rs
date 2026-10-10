//! Port of module/util.go.

use crate::frontend::prelude::*;
use std::sync::LazyLock;

// Go: module/util.go:13 typeScriptVersion
// PORT: Go package-level var is a `LazyLock`. It stays private, because
// packagejson has its own copy with the same Go name.
static TYPE_SCRIPT_VERSION: LazyLock<Version> =
    LazyLock::new(|| must_parse_version(crate::core::version()));

// Go: module/util.go:15 InferredTypesContainingFile
pub const INFERRED_TYPES_CONTAINING_FILE: &str = "__inferred type names__.ts";

// Go: module/util.go:17 IsApplicableVersionedTypesKey
#[must_use]
pub fn is_applicable_versioned_types_key(key: &str) -> bool {
    if !key.starts_with("types@") {
        return false;
    }
    let (range, ok) = try_parse_version_range(&key["types@".len()..]);
    if !ok {
        return false;
    }
    range.test(&TYPE_SCRIPT_VERSION)
}

// Go: module/util.go:28 NodeModulePackageRootForFile (ts#64544)
#[must_use]
pub fn node_module_package_root_for_file(resolved: &str) -> String {
    parse_node_module_package_root(resolved, false /*isDirectory*/)
}

// Go: module/util.go:32 NodeModulePackageRootForDirectory (ts#64544)
#[must_use]
pub fn node_module_package_root_for_directory(resolved: &str) -> String {
    parse_node_module_package_root(resolved, true /*isDirectory*/)
}

// Go: module/util.go:28 ParseNodeModuleFromPath (removed by ts#64544, which
// splits it into NodeModulePackageRootForFile and
// NodeModulePackageRootForDirectory)
// PORT: kept for the ls callers (ls/rename.rs, ls/autoimport/util.rs) until
// the ls lane ports their ts#64544 parts.
#[must_use]
pub fn parse_node_module_from_path(resolved: &str, is_folder: bool) -> String {
    parse_node_module_package_root(resolved, is_folder)
}

// Go: module/util.go:36 parseNodeModulePackageRoot (ts#64544)
fn parse_node_module_package_root(path: &str, is_directory: bool) -> String {
    let path = normalize_path(path);
    let Some(idx) = path.rfind("/node_modules/") else {
        return String::new();
    };

    // PORT: Go `int` indexes are `i32` to match `move_to_next_directory_separator_if_available`.
    let index_after_node_modules = (idx + "/node_modules/".len()) as i32;
    let mut index_after_package_name = move_to_next_directory_separator_if_available(
        &path,
        index_after_node_modules,
        is_directory,
    );
    if path.as_bytes()[index_after_node_modules as usize] == b'@' {
        index_after_package_name = move_to_next_directory_separator_if_available(
            &path,
            index_after_package_name,
            is_directory,
        );
    }
    path[..index_after_package_name as usize].to_string()
}

// Go: module/util.go:50 ParsePackageName
#[must_use]
pub fn parse_package_name(module_name: &str) -> (String, String) {
    let mut idx: Option<usize> = module_name.find('/');
    if !module_name.is_empty() && module_name.as_bytes()[0] == b'@' {
        // PORT: Go `idx + 1` with `idx == -1` gives offset 0.
        let offset = idx.map_or(0, |i| i + 1);
        idx = module_name[offset..].find('/').map(|i| i + offset);
    }
    match idx {
        None => (module_name.to_string(), String::new()),
        Some(idx) => (
            module_name[..idx].to_string(),
            module_name[idx + 1..].to_string(),
        ),
    }
}

// Go: module/util.go:65 MangleScopedPackageName
#[must_use]
pub fn mangle_scoped_package_name(package_name: &str) -> String {
    if !package_name.is_empty() && package_name.as_bytes()[0] == b'@' {
        let Some(idx) = package_name.find('/') else {
            return package_name.to_string();
        };
        return format!("{}__{}", &package_name[1..idx], &package_name[idx + 1..]);
    }
    package_name.to_string()
}

// Go: module/util.go:76 UnmangleScopedPackageName
#[must_use]
pub fn unmangle_scoped_package_name(package_name: &str) -> String {
    if let Some((before, after)) = package_name.split_once("__") {
        return format!("@{before}/{after}");
    }
    package_name.to_string()
}

// Go: module/util.go:84 GetTypesPackageName
#[must_use]
pub fn get_types_package_name(package_name: &str) -> String {
    format!("@types/{}", mangle_scoped_package_name(package_name))
}

// Go: module/util.go:88 GetPackageNameFromTypesPackageName
#[must_use]
pub fn get_package_name_from_types_package_name(mangled_name: &str) -> String {
    if let Some(without_at_type_prefix) = mangled_name.strip_prefix("@types/") {
        return unmangle_scoped_package_name(without_at_type_prefix);
    }
    mangled_name.to_string()
}

// Go: module/util.go:96 ComparePatternKeys
#[must_use]
// PORT: lengths and offsets are Go byte counts of the port forms (see
// `scanner_util::GO_STRING_MARKER`).
pub fn compare_pattern_keys(a: &str, b: &str) -> i32 {
    let a_pattern_index = a.find('*');
    let b_pattern_index = b.find('*');
    let base_len_a = a_pattern_index.map_or(go_len(a), |i| go_len(&a[..i]) + 1);
    let base_len_b = b_pattern_index.map_or(go_len(b), |i| go_len(&b[..i]) + 1);

    if base_len_a > base_len_b {
        return -1;
    }
    if base_len_b > base_len_a {
        return 1;
    }
    if a_pattern_index.is_none() {
        return 1;
    }
    if b_pattern_index.is_none() {
        return -1;
    }
    if go_len(a) > go_len(b) {
        return -1;
    }
    if go_len(b) > go_len(a) {
        return 1;
    }
    0
}

// Go: module/util.go:132 GetResolutionDiagnostic
// Returns a DiagnosticMessage if we won't include a resolved module due to its extension.
// The DiagnosticMessage's parameters are the imported module name, and the filename it resolved to.
// This returns a diagnostic even if the module will be an untyped module.
// PORT: Go `*ast.SourceFile` is the loader's `ParsedSourceFile`. The only
// caller runs during program construction, before `source_file_info` exists.
#[must_use]
pub fn get_resolution_diagnostic(
    options: &CompilerOptions,
    resolved_module: &ResolvedModule,
    file: &ParsedSourceFile,
) -> Option<&'static Message> {
    let need_jsx = || -> Option<&'static Message> {
        if options.jsx != JsxEmit::NONE {
            return None;
        }
        Some(diag::Module_0_was_resolved_to_1_but_jsx_is_not_set)
    };

    let need_allow_js = || -> Option<&'static Message> {
        if options.get_allow_js()
            || !options
                .no_implicit_any
                .default_if_unknown(options.strict)
                .is_true()
        {
            return None;
        }
        Some(diag::Could_not_find_a_declaration_file_for_module_0_1_implicitly_has_an_any_type)
    };

    let need_resolve_json_module = || -> Option<&'static Message> {
        if options.get_resolve_json_module() {
            return None;
        }
        Some(diag::Module_0_was_resolved_to_1_but_resolveJsonModule_is_not_used)
    };

    let need_allow_arbitrary_extensions = || -> Option<&'static Message> {
        if file.is_declaration_file || options.allow_arbitrary_extensions.is_true() {
            return None;
        }
        Some(diag::Module_0_was_resolved_to_1_but_allowArbitraryExtensions_is_not_set)
    };

    // tsgo#4712
    if resolved_module.resolved_using_extra_extensions {
        return None;
    }

    // PORT: Go string `switch` is an if/else chain.
    let ext = resolved_module.extension.as_str();
    if ext == EXTENSION_TS
        || ext == EXTENSION_DTS
        || ext == EXTENSION_MTS
        || ext == EXTENSION_DMTS
        || ext == EXTENSION_CTS
        || ext == EXTENSION_DCTS
    {
        // These are always allowed.
        None
    } else if ext == EXTENSION_TSX {
        need_jsx()
    } else if ext == EXTENSION_JSX {
        if let Some(message) = need_jsx() {
            return Some(message);
        }
        need_allow_js()
    } else if ext == EXTENSION_JS || ext == EXTENSION_MJS || ext == EXTENSION_CJS {
        need_allow_js()
    } else if ext == EXTENSION_JSON {
        need_resolve_json_module()
    } else {
        need_allow_arbitrary_extensions()
    }
}

// Go: module/util.go:189 TryGetJSExtensionForFile
// TryGetJSExtensionForFile maps TS/JS/DTS extensions to the output JS-side extension.
// Returns an empty string if the extension is unsupported.
#[must_use]
pub fn try_get_js_extension_for_file(file_name: &str, options: &CompilerOptions) -> &'static str {
    let ext = try_get_extension_from_path(file_name);
    if ext == EXTENSION_TS || ext == EXTENSION_DTS {
        EXTENSION_JS
    } else if ext == EXTENSION_TSX {
        if options.jsx == JsxEmit::PRESERVE {
            return EXTENSION_JSX;
        }
        EXTENSION_JS
    } else if ext == EXTENSION_JS || ext == EXTENSION_JSX || ext == EXTENSION_JSON {
        ext
    } else if ext == EXTENSION_DMTS || ext == EXTENSION_MTS || ext == EXTENSION_MJS {
        EXTENSION_MJS
    } else if ext == EXTENSION_DCTS || ext == EXTENSION_CTS || ext == EXTENSION_CJS {
        EXTENSION_CJS
    } else {
        ""
    }
}
