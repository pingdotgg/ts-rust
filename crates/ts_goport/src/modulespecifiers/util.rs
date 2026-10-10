//! Port of modulespecifiers/util.go.

use crate::prelude::*;

#[cfg(not(target_family = "wasm"))]
use std::sync::{Arc, LazyLock, PoisonError, RwLock};

#[cfg(not(target_family = "wasm"))]
use crate::gostd::regexp;

use super::deps;
use super::specifiers::{get_all_module_paths, get_info, try_get_module_name_as_node_module};
use super::tspath;
use super::types::*;

// Go: modulespecifiers/util.go:30 comparePathsByRedirect
pub(crate) fn compare_paths_by_redirect(
    a: &ModulePath,
    b: &ModulePath,
    use_case_sensitive_file_names: bool,
) -> i32 {
    // Redirects sort first, matching Strada's compareBooleans(b.isRedirect, a.isRedirect).
    let c = deps::compare_booleans(b.is_redirect, a.is_redirect);
    if c != 0 {
        return c;
    }
    let c = tspath::compare_number_of_directory_separators(&a.file_name, &b.file_name);
    if c != 0 {
        return c;
    }
    // Strada relies on Map insertion order to break remaining ties deterministically;
    // Go maps are unordered, so compare paths to keep the ordering stable.
    tspath::compare_paths(
        &a.file_name,
        &b.file_name,
        &tspath::ComparePathsOptions {
            use_case_sensitive_file_names,
            ..Default::default()
        },
    )
}

// Go: modulespecifiers/util.go:43 PathIsBareSpecifier
pub fn path_is_bare_specifier(path: &str) -> bool {
    !tspath::path_is_absolute(path) && !tspath::path_is_relative(path)
}

// Go: modulespecifiers/util.go:20 regexPatternCacheKey
#[cfg(not(target_family = "wasm"))]
#[derive(Clone, PartialEq, Eq, Hash)]
struct RegexPatternCacheKey {
    pattern: String,
    case_insensitive: bool,
}

// Go: modulespecifiers/util.go:27 regexPatternCache
// PORT: Go guards the map with a sync.RWMutex. Checker threads can reach
// this code (the declaration emitter), so the port keeps the lock. A panic
// under the lock (an unported regexp feature) must not block later calls,
// so a poisoned lock is used as is.
#[cfg(not(target_family = "wasm"))]
static REGEX_PATTERN_CACHE: LazyLock<
    RwLock<FxHashMap<RegexPatternCacheKey, Option<Arc<regexp::Regexp>>>>,
> = LazyLock::new(Default::default);

// Go: modulespecifiers/util.go:47 IsExcludedByRegex
#[cfg(not(target_family = "wasm"))]
pub fn is_excluded_by_regex(module_specifier: &str, excludes: &[String]) -> bool {
    for pattern in excludes {
        let Some(re) = string_to_regex(pattern) else {
            continue;
        };
        if re.match_string(module_specifier) {
            return true;
        }
    }
    false
}

/// The wasm build runs tsc only, and tsc passes default user preferences
/// (the checker's node builder and `get_module_specifier`), so `excludes`
/// is always empty there. Only the language service fills it, from the
/// editor's `autoImportSpecifierExcludeRegexes`. This leaves the Go regexp
/// engine and the unicode tables that only it uses out of the wasm module.
// PORT: not in Go.
#[cfg(target_family = "wasm")]
pub fn is_excluded_by_regex(_module_specifier: &str, excludes: &[String]) -> bool {
    if !excludes.is_empty() {
        unreachable!("the wasm build has no regexp engine for autoImportSpecifierExcludeRegexes");
    }
    false
}

// Go: modulespecifiers/util.go:60 stringToRegex
#[cfg(not(target_family = "wasm"))]
fn string_to_regex(pattern: &str) -> Option<Arc<regexp::Regexp>> {
    let mut pattern = pattern;
    let mut case_insensitive = false;

    let pb = pattern.as_bytes();
    if pb.len() > 2 && pb[0] == b'/' {
        // PORT: Go's strings.LastIndex returns -1 when there is no match;
        // pattern[0] is '/', so there is always one.
        let last_slash = pattern.rfind('/').unwrap_or(0);
        if last_slash > 0 {
            let mut has_unescaped_middle_slash = false;
            for i in 1..last_slash {
                if pb[i] == b'/' && (i == 0 || pb[i - 1] != b'\\') {
                    has_unescaped_middle_slash = true;
                    break;
                }
            }

            if !has_unescaped_middle_slash {
                let flags = &pattern[last_slash + 1..];
                pattern = &pattern[1..last_slash];

                for flag in flags.chars() {
                    if flag == 'i' {
                        case_insensitive = true;
                    }
                }
            }
        }
    }
    let key = RegexPatternCacheKey {
        pattern: pattern.to_string(),
        case_insensitive,
    };

    {
        let cache = REGEX_PATTERN_CACHE
            .read()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(re) = cache.get(&key) {
            return re.clone();
        }
    }

    let mut cache = REGEX_PATTERN_CACHE
        .write()
        .unwrap_or_else(PoisonError::into_inner);

    if let Some(re) = cache.get(&key) {
        return re.clone();
    }

    if cache.len() > 1000 {
        cache.clear();
    }

    let compile_pattern = if case_insensitive {
        format!("(?i:{pattern})")
    } else {
        pattern.to_string()
    };

    match regexp::compile_exported(&compile_pattern) {
        Err(_) => {
            cache.insert(key, None);
            None
        }
        Ok(compiled) => {
            let compiled = Arc::new(compiled);
            cache.insert(key, Some(compiled.clone()));
            Some(compiled)
        }
    }
}

// Go: modulespecifiers/util.go:136 ensurePathIsNonModuleName (at 673a5f17d713;
// ts#64159 uses tspath.EnsurePathIsNonModuleName, tspath/path.go:987)
/// Ensures a path is either absolute (prefixed with `/` or `c:`) or dot-relative (prefixed
/// with `./` or `../`) so as not to be confused with an unprefixed module name.
pub(crate) fn ensure_path_is_non_module_name(path: &str) -> String {
    if path_is_bare_specifier(path) {
        return format!("./{path}");
    }
    path.to_string()
}

// Go: modulespecifiers/util.go:125 GetJSExtensionForDeclarationFileExtension
pub fn get_js_extension_for_declaration_file_extension(ext: &str) -> String {
    match ext {
        tspath::EXTENSION_DTS => tspath::EXTENSION_JS.to_string(),
        tspath::EXTENSION_DMTS => tspath::EXTENSION_MJS.to_string(),
        tspath::EXTENSION_DCTS => tspath::EXTENSION_CJS.to_string(),
        // .d.json.ts and the like
        _ => ext[".d".len()..ext.len() - tspath::EXTENSION_TS.len()].to_string(),
    }
}

// Go: modulespecifiers/util.go:141 TryGetRealFileNameForNonJSDeclarationFileName
/// Remaps files like `foo.d.json.ts` or `foo.module.d.css.ts` back to their
/// real non-JS names.
pub fn try_get_real_file_name_for_non_js_declaration_file_name(file_name: &str) -> String {
    let base_name = tspath::get_base_file_name(file_name);
    // Ends with .ts, contains ".d.", and is NOT a standard .d.ts file
    if !file_name.ends_with(tspath::EXTENSION_TS)
        || !base_name.contains(".d.")
        || base_name.ends_with(tspath::EXTENSION_DTS)
    {
        return String::new();
    }
    let no_extension = tspath::remove_extension(file_name, tspath::EXTENSION_TS);
    // PORT: Go slices from LastIndex, which is -1 (whole string) when there
    // is no dot. A ".d." is present, so there is always a dot.
    let last_dot_index = no_extension.rfind('.').unwrap_or(0);
    let ext = &no_extension[last_dot_index..];
    let before = no_extension
        .split_once(".d.")
        .map_or(no_extension, |(before, _)| before);
    format!("{before}{ext}")
}

// Go: modulespecifiers/util.go:156 getJSExtensionForFile
pub(crate) fn get_js_extension_for_file(
    file_name: &str,
    options: &CompilerOptions,
) -> &'static str {
    let result = deps::try_get_js_extension_for_file(file_name, options);
    if result.is_empty() {
        panic!(
            "Extension {} is unsupported:: FileName:: {}",
            extension_from_path(file_name),
            file_name
        );
    }
    result
}

// Go: modulespecifiers/util.go:176 extensionFromPath
/// Gets the extension from a path. Path must have a valid extension.
fn extension_from_path(path: &str) -> &'static str {
    let ext = tspath::try_get_extension_from_path(path);
    if ext.is_empty() {
        panic!("File {path} has unknown extension.");
    }
    ext
}

// Go: modulespecifiers/util.go:184 tryGetAnyFileFromPath
pub(crate) fn try_get_any_file_from_path(
    host: &dyn ModuleSpecifierGenerationHost,
    path: &str,
) -> bool {
    // !!! TODO: shouldn't this use readdir instead of fileexists for perf?
    // We check all js, `node` and `json` extensions in addition to TS, since node module resolution would also choose those over the directory
    // PORT: Go builds the groups with tsoptions.GetSupportedExtensions
    // (AllowJs plus the extra extensions ".node" and ".json"). tsgo#4712:
    // neither is a built-in extension, so each is added as its own group
    // after tspath.AllSupportedExtensions.
    let extra_groups: &[&[&str]] = &[&[".node"], &[".json"]];
    let ext_groups = tspath::ALL_SUPPORTED_EXTENSIONS.iter().chain(extra_groups);
    for exts in ext_groups {
        for e in exts.iter() {
            let full_path = format!("{path}{e}");
            if host.file_exists(&tspath::get_normalized_absolute_path(
                &full_path,
                &host.get_current_directory(),
            )) {
                return true;
            }
        }
    }
    false
}

// Go: modulespecifiers/util.go:203 getPathsRelativeToRootDirs
pub(crate) fn get_paths_relative_to_root_dirs(
    path: &str,
    root_dirs: &[String],
    use_case_sensitive_file_names: bool,
) -> Vec<String> {
    let mut results = Vec::new();
    for root_dir in root_dirs {
        let relative_path =
            get_relative_path_if_in_same_volume(path, root_dir, use_case_sensitive_file_names);
        if !is_path_relative_to_parent(&relative_path) {
            results.push(relative_path);
        }
    }
    results
}

// Go: modulespecifiers/util.go:214 isPathRelativeToParent
// ts#64159 (tspath/relative_path.go:44 RelativePath.IsParentRelative): ".." or
// "../..." only, not a name that starts with ".." ("..foo.ts").
pub(crate) fn is_path_relative_to_parent(path: &str) -> bool {
    path == ".." || path.starts_with("../")
}

// Go: modulespecifiers/util.go:218 getRelativePathIfInSameVolume
pub(crate) fn get_relative_path_if_in_same_volume(
    path: &str,
    directory_path: &str,
    use_case_sensitive_file_names: bool,
) -> String {
    let relative_path = tspath::get_relative_path_to_directory_or_url(
        directory_path,
        path,
        false,
        &tspath::ComparePathsOptions {
            use_case_sensitive_file_names,
            current_directory: directory_path.to_string(),
        },
    );
    if tspath::is_rooted_disk_path(&relative_path) {
        return String::new();
    }
    relative_path
}

// Go: modulespecifiers/util.go:241 packageJsonPathsAreEqual
pub(crate) fn package_json_paths_are_equal(
    a: &str,
    b: &str,
    options: &tspath::ComparePathsOptions,
) -> bool {
    if a == b {
        return true;
    }
    if a.is_empty() || b.is_empty() {
        return false;
    }
    tspath::compare_paths(a, b, options) == 0
}

// Go: modulespecifiers/util.go:251 prefersTsExtension
pub(crate) fn prefers_ts_extension(allowed_endings: &[ModuleSpecifierEnding]) -> bool {
    let js_priority = index_of(allowed_endings, ModuleSpecifierEnding::JsExtension);
    let ts_priority = index_of(allowed_endings, ModuleSpecifierEnding::TsExtension);
    if ts_priority > -1 {
        return ts_priority < js_priority;
    }
    false
}

/// Go `slices.Index`: the first index of `value`, or -1.
pub(crate) fn index_of(endings: &[ModuleSpecifierEnding], value: ModuleSpecifierEnding) -> isize {
    endings
        .iter()
        .position(|e| *e == value)
        .map_or(-1, |i| i as isize)
}

// Go: modulespecifiers/util.go:260 replaceFirstStar
// PORT: Go joins the bytes (see `scanner_util::go_value`).
pub(crate) fn replace_first_star(s: &str, replacement: &str) -> String {
    go_value_owned(s.replacen('*', replacement, 1))
}

// Go: modulespecifiers/util.go:264 NodeModulePathParts
// PORT: the port keeps the index form of Go N. `package_root_index` is -1
// when the file has no package root (Go N' `IsDirectNodeModulesFile`,
// util.go:333).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NodeModulePathParts {
    pub top_level_node_modules_index: isize,
    pub top_level_package_name_index: isize,
    pub package_root_index: isize,
    pub file_name_index: isize,
}

// Go: modulespecifiers/util.go:274 nodeModulesPathParseState
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum NodeModulesPathParseState {
    BeforeNodeModules,
    NodeModules,
    Scope,
    PackageContent,
}

// Go: modulespecifiers/util.go:283 GetNodeModulePathParts
pub fn get_node_module_path_parts(full_path: &str) -> Option<NodeModulePathParts> {
    // If fullPath can't be valid module file within node_modules, returns undefined.
    // Example of expected pattern: /base/path/node_modules/[@scope/otherpackage/@otherscope/node_modules/]package/[subdirectory/]file.js
    // Returns indices:                       ^            ^                                                      ^             ^
    use NodeModulesPathParseState::*;

    let mut top_level_node_modules_index: isize = 0;
    let mut top_level_package_name_index: isize = 0;
    // ts#64159 (util.go:287): -1 until a package root is found, so a path
    // that ends in a scope (`node_modules/@a.d.ts`) has no package root.
    let mut package_root_index: isize = -1;

    let mut part_start: isize = 0;
    let mut part_end: isize = 0;
    let mut state = BeforeNodeModules;

    let bytes = full_path.as_bytes();
    while part_end >= 0 {
        part_start = part_end;
        part_end = deps::index_after(full_path, "/", (part_start + 1) as usize);
        match state {
            BeforeNodeModules => {
                if full_path[part_start as usize..].starts_with("/node_modules/") {
                    top_level_node_modules_index = part_start;
                    top_level_package_name_index = part_end;
                    state = NodeModules;
                }
            }
            NodeModules | Scope => {
                if state == NodeModules && bytes[(part_start + 1) as usize] == b'@' {
                    state = Scope;
                } else {
                    package_root_index = part_end;
                    state = PackageContent;
                }
            }
            PackageContent => {
                if full_path[part_start as usize..].starts_with("/node_modules/") {
                    state = NodeModules;
                } else {
                    state = PackageContent;
                }
            }
        }
    }

    let file_name_index = part_start;

    if state > NodeModules {
        return Some(NodeModulePathParts {
            top_level_node_modules_index,
            top_level_package_name_index,
            package_root_index,
            file_name_index,
        });
    }
    None
}

// Go: modulespecifiers/util.go:353 GetNodeModulesPackageName
pub fn get_node_modules_package_name(
    compiler_options: &CompilerOptions,
    importing_source_file: Node, // !!! | FutureSourceFile
    node_modules_file_name: &str,
    host: &dyn ModuleSpecifierGenerationHost,
    preferences: &UserPreferences,
    options: ModuleSpecifierOptions,
) -> String {
    let info = get_info(&importing_source_file.file_name(), host);
    let module_paths = get_all_module_paths(
        &info,
        node_modules_file_name,
        host,
        compiler_options,
        preferences,
        options,
    );
    for module_path in &module_paths {
        let result = try_get_module_name_as_node_module(
            module_path,
            &info,
            &importing_source_file,
            host,
            compiler_options,
            preferences,
            true, /*packageNameOnly*/
            options.override_import_mode,
        );
        if !result.is_empty() {
            return result;
        }
    }
    String::new()
}

// Go: modulespecifiers/util.go:371 allKeysStartWithDot
pub(crate) fn all_keys_start_with_dot(
    obj: &IndexMap<String, super::packagejson::ExportsOrImports>,
) -> bool {
    obj.keys().all(|k| k.starts_with('.'))
}

// Go: modulespecifiers/util.go:380 GetPackageNameFromDirectory
pub fn get_package_name_from_directory(file_or_directory_path: &str) -> String {
    let Some(idx) = file_or_directory_path.rfind("/node_modules/") else {
        return String::new();
    };

    let basename = &file_or_directory_path[idx + "/node_modules/".len()..];
    // PORT: Go indexes basename[0] and panics on an empty basename.
    if basename.as_bytes()[0] == b'.' {
        return String::new();
    }

    let Some(next_slash) = basename.find('/') else {
        return basename.to_string();
    };

    if basename.as_bytes()[0] != b'@' || next_slash == basename.len() - 1 {
        return basename[..next_slash].to_string();
    }

    let Some(second_slash) = basename[next_slash + 1..].find('/') else {
        return basename.to_string();
    };

    basename[..next_slash + 1 + second_slash].to_string()
}

// Go: modulespecifiers/util.go:410 ProcessEntrypointEnding
// PORT: not ported. It takes a `module.ResolvedEntrypoint`, which only the
// language service auto-import code produces.

#[cfg(test)]
mod tests {
    use super::*;

    // Go: modulespecifiers/specifiers_test.go:432 TestIsPathRelativeToParent (ts#64159)
    #[test]
    fn is_path_relative_to_parent_needs_a_parent_segment() {
        for (path, expected) in [
            ("..", true),
            ("../sibling.ts", true),
            ("..foo.ts", false),
            ("child/..foo.ts", false),
        ] {
            assert_eq!(is_path_relative_to_parent(path), expected, "{path}");
        }
    }

    // Go: modulespecifiers/specifiers_test.go:16 TestGetNodeModulePathParts (ts#64159),
    // the nil, package root and `isDirectNodeModulesFile` cases, and the ls
    // skeptic's specifier-direct-pkg probe paths. A path that ends in a scope
    // kept package root 0, and willRenameFiles then panicked in
    // tryDirectoryWithPackageJson.
    #[test]
    fn get_node_module_path_parts_package_root() {
        assert_eq!(get_node_module_path_parts("/workspace/src/index.ts"), None);
        for (path, root) in [
            (
                "/workspace/node_modules/pkg/lib/index.d.ts",
                "/workspace/node_modules/pkg",
            ),
            (
                "/node_modules/@scope/pkg/index.d.ts",
                "/node_modules/@scope/pkg",
            ),
            ("c:/node_modules/pkg/index.d.ts", "c:/node_modules/pkg"),
            (
                "/workspace/node_modules/pkg/node_modules/@scope/dep/index.d.ts",
                "/workspace/node_modules/pkg/node_modules/@scope/dep",
            ),
        ] {
            let parts = get_node_module_path_parts(path).unwrap();
            assert_eq!(&path[..parts.package_root_index as usize], root, "{path}");
        }
        for path in [
            "/workspace/node_modules/pkg",
            "/workspace/node_modules/@scope",
            "/p/node_modules/plain.d.ts",
            "/p/node_modules/@foo.d.ts",
            "/p/node_modules/@scope/direct.d.ts",
            "c:/node_modules/@foo.d.ts",
        ] {
            let parts = get_node_module_path_parts(path).unwrap();
            assert_eq!(parts.package_root_index, -1, "{path}");
        }
    }
}
