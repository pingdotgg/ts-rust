//! Port of tspath/path.go and tspath/ignoredpaths.go.

use crate::frontend::prelude::*;

use super::dynamic::{
    DYNAMIC_URI_FILE_NAME_PREFIX, canonical_dynamic_uri_path, is_encoded_dynamic_file_name,
};
use crate::gostd::unicode;
use std::borrow::Cow;

// Go: tspath/path.go:14 Path (at 673a5f17d713; ts#64159 renames it PathKey, tspath/pathkey.go:15)
// PORT: Go `type Path string`. A newtype keeps the Go method set
// (`GetDirectoryPath`, `ContainsPath`, ...). `Deref<Target = str>` and
// `Borrow<str>` stand in for Go `string(p)`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Path(pub String);

impl Path {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl std::ops::Deref for Path {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0
    }
}

impl std::borrow::Borrow<str> for Path {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Path {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

// Internally, we represent paths as strings with '/' as the directory separator.
// When we make system calls (eg: LanguageServiceHost.getDirectory()),
// we expect the host to correctly handle paths in our specified format.
pub const DIRECTORY_SEPARATOR: u8 = b'/';
const URL_SCHEME_SEPARATOR: &str = "://";

// ---------------------------------------------------------------------------
// stringutil/compare.go helpers used by tspath.
// PORT: stringutil is not a unit of this wave. These are private copies of
// the Go functions that tspath calls, so no other unit's names are claimed.
// ---------------------------------------------------------------------------

// PORT: simple uppercase. A multi-rune full uppercase (for example U+00DF)
// has no simple mapping in UnicodeData, so the rune stays the same.
fn simple_to_upper(c: char) -> char {
    let mut u = c.to_uppercase();
    match (u.next(), u.next()) {
        (Some(single), None) => single,
        _ => c,
    }
}

// PORT: simple case fold key. Two runes are in the same Go `unicode.SimpleFold`
// orbit when their keys are equal. The key is lower(upper(c)) with simple
// mappings. U+0131 (dotless i) has only a Turkic fold entry, so its orbit is
// itself, like Go. U+0130 stays itself because it has no simple lowercase
// other than through the full mapping. Rust and Go can use different Unicode
// versions; this matters only for runes added between those versions.
fn simple_fold_key(c: char) -> char {
    if c == '\u{0131}' {
        return c;
    }
    let u = simple_to_upper(c);
    let mut l = u.to_lowercase();
    match (l.next(), l.next()) {
        (Some(single), None) => single,
        _ => u,
    }
}

// Go: stringutil/compare.go:9 EquateStringCaseInsensitive
// PORT: Go `strings.EqualFold`. Each rune pair must be equal, ASCII case
// equal, or in the same simple fold orbit (see `simple_fold_key`).
fn equate_string_case_insensitive(a: &str, b: &str) -> bool {
    let mut ai = a.chars();
    let mut bi = b.chars();
    loop {
        match (ai.next(), bi.next()) {
            (None, None) => return true,
            (Some(sr), Some(tr)) => {
                if sr == tr {
                    continue;
                }
                let (sr, tr) = if tr < sr { (tr, sr) } else { (sr, tr) };
                if (tr as u32) < 0x80 {
                    // ASCII only, sr/tr must be upper/lower case
                    if sr.is_ascii_uppercase() && tr as u32 == sr as u32 + ('a' as u32 - 'A' as u32)
                    {
                        continue;
                    }
                    return false;
                }
                if simple_fold_key(sr) == simple_fold_key(tr) {
                    continue;
                }
                return false;
            }
            _ => return false,
        }
    }
}

// Go: stringutil/compare.go:15 EquateStringCaseSensitive
fn equate_string_case_sensitive(a: &str, b: &str) -> bool {
    a == b
}

// Go: stringutil/compare.go:19 GetStringEqualityComparer
fn get_string_equality_comparer(ignore_case: bool) -> fn(&str, &str) -> bool {
    if ignore_case {
        return equate_string_case_insensitive;
    }
    equate_string_case_sensitive
}

// Go: stringutil/compare.go:34 CompareStringsCaseInsensitive
fn compare_strings_case_insensitive(a: &str, b: &str) -> i32 {
    if a == b {
        return 0;
    }
    let mut ai = a.chars();
    let mut bi = b.chars();
    loop {
        match (ai.next(), bi.next()) {
            (None, None) => return 0,
            (None, Some(_)) => return -1,
            (Some(_), None) => return 1,
            (Some(ca), Some(cb)) => {
                let lca = unicode::to_lower(ca);
                let lcb = unicode::to_lower(cb);
                if lca != lcb {
                    if lca < lcb {
                        return -1;
                    }
                    return 1;
                }
            }
        }
    }
}

// Go: stringutil/compare.go:63 CompareStringsCaseSensitive
// PORT: Go compares bytes; the strings are port forms (see
// `scanner_util::compare_go_bytes`).
fn compare_strings_case_sensitive(a: &str, b: &str) -> i32 {
    compare_go_bytes(a, b) as i32
}

// Go: stringutil/compare.go:67 GetStringComparer
fn get_string_comparer(ignore_case: bool) -> fn(&str, &str) -> i32 {
    if ignore_case {
        return compare_strings_case_insensitive;
    }
    compare_strings_case_sensitive
}

// PORT: Go `strings.LastIndex(s, "/")` / `LastIndexByte` as a signed index.
fn last_index_byte(s: &str, c: u8) -> isize {
    s.as_bytes()
        .iter()
        .rposition(|&b| b == c)
        .map_or(-1, |i| i as isize)
}

//// Path Tests

// Go: tspath/path.go:25 isAnyDirectorySeparator
// Determines whether a byte corresponds to `/` or `\`.
fn is_any_directory_separator(char: u8) -> bool {
    char == b'/' || char == b'\\'
}

// Go: tspath/path.go:30 IsUrl
// Determines whether a path starts with a URL scheme (e.g. starts with `http://`, `ftp://`, `file://`, etc.).
pub fn is_url(path: &str) -> bool {
    get_encoded_root_length(path) < 0
}

// Go: tspath/path.go:36 IsRootedDiskPath
// Determines whether a path is an absolute disk path (e.g. starts with `/`, or a dos path
// like `c:`, `c:\` or `c:/`).
pub fn is_rooted_disk_path(path: &str) -> bool {
    get_encoded_root_length(path) > 0
}

// Go: tspath/path.go:43 IsDiskPathRoot (at 673a5f17d713; removed by ts#64159)
// Determines whether a path consists only of a path root.
pub fn is_disk_path_root(path: &str) -> bool {
    let root_length = get_encoded_root_length(path);
    root_length > 0 && root_length as usize == path.len()
}

// Go: tspath/path.go:42 IsDynamicFileName
// IsDynamicFileName returns true if the file name represents a dynamic/virtual file
// that doesn't exist on disk (e.g., untitled files with paths like "^/untitled/...").
pub fn is_dynamic_file_name(file_name: &str) -> bool {
    file_name.starts_with("^/")
}

// Go: tspath/path.go:59 PathIsAbsolute
// Determines whether a path starts with an absolute path component (i.e. `/`, `c:/`, `file://`, etc.).
pub fn path_is_absolute(path: &str) -> bool {
    get_encoded_root_length(path) != 0
}

// Go: tspath/path.go:63 HasTrailingDirectorySeparator
pub fn has_trailing_directory_separator(path: &str) -> bool {
    !path.is_empty() && is_any_directory_separator(path.as_bytes()[path.len() - 1])
}

// Go: tspath/path.go:83 CombinePaths
// Combines paths. If a path is absolute, it replaces any previous path. Relative paths are not simplified.
// PORT: Go writes into one `strings.Builder` and "sets" the result by moving
// `start`. Building the current result directly gives the same string.
pub fn combine_paths(first_path: &str, paths: &[&str]) -> String {
    // TODO (drosen): There is potential for a fast path here.
    // In the case where we find the last absolute path and just path.Join from there.
    let mut result = normalize_slashes(first_path);
    for trailing_path in paths {
        if trailing_path.is_empty() {
            continue;
        }
        let trailing_path = normalize_slashes(trailing_path);
        if result.is_empty() || get_root_length(&trailing_path) != 0 {
            // `trailingPath` is absolute.
            result = trailing_path;
        } else {
            if !has_trailing_directory_separator(&result) {
                result.push(DIRECTORY_SEPARATOR as char);
            }
            result.push_str(&trailing_path);
        }
    }
    result
}

// Go: tspath/path.go:128 GetPathComponents
pub fn get_path_components(path: &str, current_directory: &str) -> Vec<String> {
    let path = combine_paths(current_directory, &[path]);
    let root_length = get_root_length(&path);
    path_components(&path, root_length)
}

// Go: tspath/path.go:138 pathComponents
fn path_components(path: &str, root_length: usize) -> Vec<String> {
    let root = &path[..root_length];
    let mut rest: Vec<String> = path[root_length..].split('/').map(str::to_string).collect();
    if rest.last().is_some_and(|s| s.is_empty()) {
        rest.pop();
    }
    let mut result = Vec::with_capacity(rest.len() + 1);
    result.push(root.to_string());
    result.extend(rest);
    result
}

// Go: tspath/path.go:147 IsVolumeCharacter
pub fn is_volume_character(char: u8) -> bool {
    char.is_ascii_lowercase() || char.is_ascii_uppercase()
}

// Go: tspath/path.go:151 getFileUrlVolumeSeparatorEnd
fn get_file_url_volume_separator_end(url: &str, start: usize) -> i32 {
    let url = url.as_bytes();
    if url.len() <= start {
        return -1;
    }
    let ch0 = url[start];
    if ch0 == b':' {
        return (start + 1) as i32;
    }
    if ch0 == b'%' && url.len() > start + 2 && url[start + 1] == b'3' {
        let ch2 = url[start + 2];
        if ch2 == b'a' || ch2 == b'A' {
            return (start + 3) as i32;
        }
    }
    -1
}

// Go: tspath/path.go:168 GetEncodedRootLength (ts#64544: dynamic file names,
// case-insensitive file URLs)
pub fn get_encoded_root_length(path: &str) -> i32 {
    let bytes = path.as_bytes();
    let ln = bytes.len();
    if ln == 0 {
        return 0;
    }
    let ch0 = bytes[0];

    // POSIX or UNC
    if ch0 == b'/' || ch0 == b'\\' {
        if ln == 1 || bytes[1] != ch0 {
            return 1; // POSIX: "/" (or non-normalized "\")
        }

        let offset = 2;
        let p1 = bytes[offset..].iter().position(|&b| b == ch0);
        return match p1 {
            None => ln as i32,                    // UNC: "//server" or "\\server"
            Some(p1) => (p1 + offset + 1) as i32, // UNC: "//server/" or "\\server\"
        };
    }

    // DOS
    if is_volume_character(ch0) && ln > 1 && bytes[1] == b':' {
        if ln == 2 {
            return 2; // DOS: "c:" (but not "c:d")
        }
        let ch2 = bytes[2];
        if ch2 == b'/' || ch2 == b'\\' {
            return 3; // DOS: "c:/" or "c:\"
        }
    }

    // Untitled paths (e.g., "^/untitled/ts-nul-authority/Untitled-1")
    if ch0 == b'^' && ln > 1 && bytes[1] == b'/' {
        // ts#64544: an encoded dynamic file name is rooted at its authority:
        // "^/~ts-uri~/<scheme>/<authority>/".
        if path.starts_with(DYNAMIC_URI_FILE_NAME_PREFIX) {
            let prefix_len = DYNAMIC_URI_FILE_NAME_PREFIX.len();
            if let Some(scheme_end) = path[prefix_len..].find('/') {
                let scheme_end = scheme_end + prefix_len;
                if let Some(authority_end) = path[scheme_end + 1..].find('/') {
                    return (scheme_end + authority_end + 2) as i32;
                }
                // ts#64159: a root with no separator after the authority is
                // URL-like (negative), so it is not a rooted disk path.
                return !(ln as i32);
            }
        }
        return 2; // Untitled: "^/"
    }

    // URL
    if let Some(scheme_end) = path.find(URL_SCHEME_SEPARATOR) {
        let authority_start = scheme_end + URL_SCHEME_SEPARATOR.len();
        if let Some(authority_length) = path[authority_start..].find('/') {
            // URL: "file:///", "file://server/", "file://server/path"
            let authority_end = authority_start + authority_length;

            // For local "file" URLs, include the leading DOS volume (if present).
            // Per https://www.ietf.org/rfc/rfc1738.txt, a host of "" or "localhost" is a
            // special case interpreted as "the machine from which the URL is being interpreted".
            let scheme = &path[..scheme_end];
            let authority = &path[authority_start..authority_end];
            // ts#64544: the scheme and the authority compare without case.
            if equate_string_case_insensitive(scheme, "file")
                && (authority.is_empty() || equate_string_case_insensitive(authority, "localhost"))
                && (ln > authority_end + 2)
                && is_volume_character(bytes[authority_end + 1])
            {
                let volume_separator_end =
                    get_file_url_volume_separator_end(path, authority_end + 2);
                if volume_separator_end != -1 {
                    if volume_separator_end as usize == ln {
                        // URL: "file:///c:", "file://localhost/c:", "file:///c$3a", "file://localhost/c%3a"
                        // but not "file:///c:d" or "file:///c%3ad"
                        return !volume_separator_end;
                    }
                    if bytes[volume_separator_end as usize] == b'/' {
                        // URL: "file:///c:/", "file://localhost/c:/", "file:///c%3a/", "file://localhost/c%3a/"
                        return !(volume_separator_end + 1);
                    }
                }
            }
            return !((authority_end + 1) as i32); // URL: "file://server/", "http://server/"
        }
        return !(ln as i32); // URL: "file://server", "http://server"
    }

    // relative
    0
}

// Go: tspath/path.go:255 GetRootLength
// PORT: returns `usize` because the result is never negative and is used to slice.
pub fn get_root_length(path: &str) -> usize {
    let root_length = get_encoded_root_length(path);
    if root_length < 0 {
        return (!root_length) as usize;
    }
    root_length as usize
}

// Go: tspath/path.go:263 GetDirectoryPath
// PERF (pgoedit1): one string, not two. `normalize_slashes_cow` borrows a
// path that has no backslash, as Go's `NormalizeSlashes` returns its input.
// memchr and memrchr pick their SIMD code at run time. The byte loops they
// replace were vectorized or not by the PGO profile alone: in the R175
// release build the language server, whose loader resolves each import name
// itself, paid about 5x the instructions here (state note
// edbisect1-2026-10-06, `scripts/state history --kind note`).
pub fn get_directory_path(path: &str) -> String {
    let path = normalize_slashes_cow(path);

    // If the path provided is itself a root, then return it.
    let root_length = get_root_length(&path);
    if root_length == path.len() {
        return path.into_owned();
    }

    // return the leading portion of the path up to the last (non-terminal) directory separator
    // but not including any trailing directory separator.
    let path = remove_trailing_directory_separator(&path);
    let end = memchr::memrchr(b'/', path.as_bytes()).map_or(root_length, |i| i.max(root_length));
    path[..end].to_string()
}

impl Path {
    // Go: tspath/path.go:263 Path.GetDirectoryPath
    pub fn get_directory_path(&self) -> Path {
        Path(get_directory_path(&self.0))
    }
}

// Go: tspath/path.go:280 GetPathFromPathComponents
pub fn get_path_from_path_components(path_components: &[String]) -> String {
    if path_components.is_empty() {
        return String::new();
    }

    let mut root = path_components[0].clone();
    if !root.is_empty() {
        root = ensure_trailing_directory_separator(&root);
    }

    root + &path_components[1..].join("/")
}

// Go: tspath/path.go:293 NormalizeSlashes
pub fn normalize_slashes(path: &str) -> String {
    normalize_slashes_cow(path).into_owned()
}

/// `normalize_slashes` that borrows `path` when it has no backslash.
// PORT: Go's `strings.ReplaceAll` returns its input when nothing matches,
// so Go makes no copy there either.
pub fn normalize_slashes_cow(path: &str) -> Cow<'_, str> {
    if memchr::memchr(b'\\', path.as_bytes()).is_some() {
        Cow::Owned(path.replace('\\', "/"))
    } else {
        Cow::Borrowed(path)
    }
}

// Go: tspath/path.go:297 reducePathComponents
fn reduce_path_components(components: Vec<String>) -> Vec<String> {
    if components.is_empty() {
        return Vec::new();
    }
    let mut components = components.into_iter();
    let mut reduced = vec![components.next().unwrap_or_default()];
    for component in components {
        if component.is_empty() {
            continue;
        }
        if component == "." {
            continue;
        }
        if component == ".." {
            if reduced.len() > 1 {
                if reduced[reduced.len() - 1] != ".." {
                    reduced.pop();
                    continue;
                }
            } else if !reduced[0].is_empty() {
                continue;
            }
        }
        reduced.push(component);
    }
    reduced
}

// Go: tspath/path.go:333 ResolvePath
// Combines and resolves paths. If a path is absolute, it replaces any previous path. Any
// `.` and `..` path components are resolved. Trailing directory separators are preserved.
pub fn resolve_path(path: &str, paths: &[&str]) -> String {
    let combined_path = if !paths.is_empty() {
        combine_paths(path, paths)
    } else {
        normalize_slashes(path)
    };
    normalize_path(&combined_path)
}

// Go: tspath/path.go:343 ResolvePathWithoutTrailingDirectorySeparator (ts#64159)
// `resolve_path` with no trailing separator, except on a root.
pub fn resolve_path_without_trailing_directory_separator(path: &str, paths: &[&str]) -> String {
    let resolved = resolve_path(path, paths);
    if resolved.len() > get_root_length(&resolved) {
        return remove_trailing_directory_separator(&resolved).to_string();
    }
    resolved
}

// Go: tspath/path.go:333 ResolveTripleslashReference (at 673a5f17d713; removed by ts#64159)
pub fn resolve_tripleslash_reference(module_name: &str, containing_file: &str) -> String {
    let base_path = get_directory_path(containing_file);
    if is_rooted_disk_path(module_name) {
        return normalize_path(module_name);
    }
    normalize_path(&combine_paths(&base_path, &[module_name]))
}

// Go: tspath/path.go:351 GetNormalizedPathComponents
pub fn get_normalized_path_components(path: &str, current_directory: &str) -> Vec<String> {
    let combined = combine_paths(current_directory, &[path]);
    get_normalized_path_components_from_combined(&combined)
}

// Go: tspath/path.go:356 getNormalizedPathComponentsFromCombined
fn get_normalized_path_components_from_combined(path: &str) -> Vec<String> {
    let bytes = path.as_bytes();
    let root_length = get_root_length(path);
    // Always include the root component (empty string for relative paths).
    let mut components: Vec<String> = Vec::with_capacity(8);
    components.push(path[..root_length].to_string());

    let mut i = root_length;
    while i < bytes.len() {
        // Skip directory separators (handles consecutive separators and trailing '/').
        while i < bytes.len() && bytes[i] == b'/' {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }

        let start = i;
        while i < bytes.len() && bytes[i] != b'/' {
            i += 1;
        }
        let component = &path[start..i];

        if component.is_empty() || component == "." {
            continue;
        }
        if component == ".." {
            if components.len() > 1 {
                if components[components.len() - 1] != ".." {
                    components.pop();
                    continue;
                }
            } else if !components[0].is_empty() {
                // If this is an absolute path, we can't go above the root.
                continue;
            }
        }

        components.push(component.to_string());
    }

    components
}

// Go: tspath/path.go:388 GetNormalizedAbsolutePathWithoutRoot (at 673a5f17d713;
// removed by ts#64159)
pub fn get_normalized_absolute_path_without_root(
    file_name: &str,
    current_directory: &str,
) -> String {
    let absolute_path = get_normalized_absolute_path(file_name, current_directory);
    let root_length = get_root_length(&absolute_path);
    absolute_path[root_length..].to_string()
}

// Go: tspath/path.go:398 GetNormalizedAbsolutePath
pub fn get_normalized_absolute_path(file_name: &str, current_directory: &str) -> String {
    let root_length = get_root_length(file_name);
    let file_name = if root_length == 0 && !current_directory.is_empty() {
        combine_paths(current_directory, &[file_name])
    } else {
        // CombinePaths normalizes slashes, so not necessary in other branch
        normalize_slashes(file_name)
    };
    let root_length = get_root_length(&file_name);

    // The `simpleNormalizePath` result, with the trailing separator fixed.
    let finish = |mut simple_normalized: String| {
        let length = simple_normalized.len();
        if length > root_length {
            let trimmed = remove_trailing_directory_separator(&simple_normalized).len();
            simple_normalized.truncate(trimmed);
            return simple_normalized;
        }
        if length == root_length && root_length != 0 {
            return ensure_trailing_directory_separator(&simple_normalized);
        }
        simple_normalized
    };
    // PORT: `simpleNormalizePath` returns the path itself when it has no
    // relative segment. Reuse `file_name` then instead of copying it.
    if !has_relative_path_segment(&file_name) {
        return finish(file_name);
    }
    if let Some(simple_normalized) = simple_normalize_path(&file_name) {
        return finish(simple_normalized);
    }

    let fb = file_name.as_bytes();
    let length = fb.len();
    let root = &file_name[..root_length];
    // `normalized` is only initialized once `fileName` is determined to be non-normalized.
    // `changed` is set at the same time.
    let mut changed = false;
    let mut normalized = String::new();
    let mut segment_start: usize;
    let mut index = root_length;
    let mut normalized_up_to = index;
    let mut seen_non_dot_dot_segment = root_length != 0;
    while index < length {
        // At beginning of segment
        segment_start = index;
        let mut ch = fb[index];
        while ch == b'/' {
            index += 1;
            if index < length {
                ch = fb[index];
            } else {
                break;
            }
        }
        if index > segment_start {
            // Seen superfluous separator
            if !changed {
                let end = (root_length as isize).max(segment_start as isize - 1) as usize;
                normalized = file_name[..end].to_string();
                changed = true;
            }
            if index == length {
                break;
            }
            segment_start = index;
        }
        // Past any superfluous separators
        let segment_end = match fb[index + 1..].iter().position(|&b| b == b'/') {
            None => length,
            Some(i) => i + index + 1,
        };
        let segment_length = segment_end - segment_start;
        if segment_length == 1 && fb[index] == b'.' {
            // "." segment (skip)
            if !changed {
                normalized = file_name[..normalized_up_to].to_string();
                changed = true;
            }
        } else if segment_length == 2 && fb[index] == b'.' && fb[index + 1] == b'.' {
            // ".." segment
            if !seen_non_dot_dot_segment {
                if changed {
                    if normalized.len() == root_length {
                        normalized.push_str("..");
                    } else {
                        normalized.push_str("/..");
                    }
                } else {
                    normalized_up_to = index + 2;
                }
            } else if !changed {
                if normalized_up_to as isize - 1 >= 0 {
                    // PORT: Go slices bytes; the byte before `normalizedUpTo`
                    // can be inside a char, so search the bytes.
                    let last = fb[..normalized_up_to - 1]
                        .iter()
                        .rposition(|&b| b == b'/')
                        .map_or(-1, |i| i as isize);
                    normalized = file_name[..(root_length as isize).max(last) as usize].to_string();
                } else {
                    normalized = file_name[..normalized_up_to].to_string();
                }
                changed = true;
                seen_non_dot_dot_segment = (normalized.len() != root_length || root_length != 0)
                    && normalized != ".."
                    && !normalized.ends_with("/..");
            } else {
                let last_slash = last_index_byte(&normalized, b'/');
                if last_slash != -1 {
                    normalized.truncate(root_length.max(last_slash as usize));
                } else {
                    normalized = root.to_string();
                }
                seen_non_dot_dot_segment = (normalized.len() != root_length || root_length != 0)
                    && normalized != ".."
                    && !normalized.ends_with("/..");
            }
        } else if changed {
            if normalized.len() != root_length {
                normalized.push('/');
            }
            seen_non_dot_dot_segment = true;
            normalized.push_str(&file_name[segment_start..segment_end]);
        } else {
            seen_non_dot_dot_segment = true;
            normalized_up_to = segment_end;
        }
        index = segment_end + 1;
    }
    if changed {
        return normalized;
    }
    if length > root_length {
        return remove_trailing_directory_separators(&file_name).to_string();
    }
    if length == root_length {
        return ensure_trailing_directory_separator(&file_name);
    }
    file_name
}

// Go: tspath/path.go:537 simpleNormalizePath
// PORT: Go `(string, bool)` becomes `Option<String>`.
fn simple_normalize_path(path: &str) -> Option<String> {
    // Most paths don't require normalization
    if !has_relative_path_segment(path) {
        return Some(path.to_string());
    }
    // Some paths only require cleanup of `/./` or leading `./`
    let simplified = path.replace("/./", "/");
    let trimmed = simplified.strip_prefix("./").unwrap_or(&simplified);
    if trimmed != path
        && !has_relative_path_segment(trimmed)
        && !(trimmed != simplified && trimmed.starts_with('/'))
    {
        // If we trimmed a leading "./" and the path now starts with "/", we changed the meaning
        return Some(trimmed.to_string());
    }
    None
}

// Go: tspath/path.go:554 hasRelativePathSegment
// hasRelativePathSegment reports whether p contains ".", "..", "./", "../", "/.", "/..", "//", "/./", or "/../".
fn has_relative_path_segment(p: &str) -> bool {
    // PORT: the answer is true exactly when a segment is "." or ".." or
    // empty between two slashes. memchr jumps from '/' to '/' and tests
    // only the bytes after it, so a `node_modules/.pnpm` segment (a dot
    // that starts a name) does not need the byte loop of Go.
    let fast = has_dot_or_empty_segment(p.as_bytes());
    debug_assert_eq!(fast, has_relative_path_segment_go(p), "{p}");
    fast
}

/// True when a segment of `b` is "." or "..", or a slash follows a slash.
/// Two SIMD searches, for "//" and for "/." (a dot that starts a segment,
/// rare in a path), cost less than a search per slash.
fn has_dot_or_empty_segment(b: &[u8]) -> bool {
    static SLASH_SLASH: std::sync::LazyLock<memchr::memmem::Finder<'static>> =
        std::sync::LazyLock::new(|| memchr::memmem::Finder::new(b"//"));
    static SLASH_DOT: std::sync::LazyLock<memchr::memmem::Finder<'static>> =
        std::sync::LazyLock::new(|| memchr::memmem::Finder::new(b"/."));
    // The segment that starts at `i` is "." or "..".
    let dot_segment = |i: usize| match b.get(i) {
        Some(b'.') => match b.get(i + 1) {
            None | Some(b'/') => true,
            Some(b'.') => matches!(b.get(i + 2), None | Some(b'/')),
            Some(_) => false,
        },
        _ => false,
    };
    dot_segment(0)
        || SLASH_SLASH.find(b).is_some()
        || SLASH_DOT.find_iter(b).any(|i| dot_segment(i + 1))
}

// Go: tspath/path.go:554 hasRelativePathSegment (the debug check of
// `has_relative_path_segment`).
fn has_relative_path_segment_go(p: &str) -> bool {
    let b = p.as_bytes();
    let n = b.len();
    if n == 0 {
        return false;
    }

    if p == "." || p == ".." {
        return true;
    }

    // Leading "./" OR "../"
    if b[0] == b'.' {
        if n >= 2 && b[1] == b'/' {
            return true;
        }
        // Leading "../"
        if n >= 3 && b[1] == b'.' && b[2] == b'/' {
            return true;
        }
    }
    // Trailing "/." OR "/.."
    if b[n - 1] == b'.' {
        if n >= 2 && b[n - 2] == b'/' {
            return true;
        }
        if n >= 3 && b[n - 2] == b'.' && b[n - 3] == b'/' {
            return true;
        }
    }

    // Now look for any `//` or `/./` or `/../`

    let mut prev_slash = false;
    let mut seg_len = 0; // length of current segment since last slash
    let mut dot_count: i32 = 0; // consecutive dots at start of the current segment; -1 => not only dots

    for &c in b {
        if c == b'/' {
            // "//"
            if prev_slash {
                return true;
            }
            // "/./" or "/../"
            if (seg_len == 1 && dot_count == 1) || (seg_len == 2 && dot_count == 2) {
                return true;
            }
            prev_slash = true;
            seg_len = 0;
            dot_count = 0;
            continue;
        }

        if c == b'.' {
            if dot_count >= 0 {
                dot_count += 1;
            }
        } else {
            dot_count = -1;
        }
        seg_len += 1;
        prev_slash = false;
    }

    // Trailing "/." or "/.."
    (seg_len == 1 && dot_count == 1) || (seg_len == 2 && dot_count == 2)
}

// Go: tspath/path.go:622 NormalizePath
pub fn normalize_path(path: &str) -> String {
    let path = normalize_slashes(path);
    // PORT: the common case of `simpleNormalizePath` with no copy. A path
    // with no relative segment is normal, except a root with no trailing
    // separator, which gets one (ts#64159: "c:" is "c:/").
    if !has_relative_path_segment(&path) {
        let root_length = get_root_length(&path);
        if root_length == 0 || path.len() > root_length {
            return path;
        }
        return ensure_trailing_directory_separator(&path);
    }
    // ts#64159: Go `getNormalizedAbsolutePathFromNormalizedSlashes`, which
    // this is with no current directory.
    let mut normalized = get_normalized_absolute_path(&path, "");
    if !normalized.is_empty() && has_trailing_directory_separator(&path) {
        normalized = ensure_trailing_directory_separator(&normalized);
    }
    normalized
}

/// True when `normalize_path(path)` is `path` itself: it has no backslash,
/// no "." or ".." segment or empty segment, and is not a root with no
/// trailing separator. (False does not mean that normalizing changes it.)
// PORT: not in Go (perf). The names that the resolver and the loader make
// are normal already, and Go normalizes them again; with this test a
// caller can keep the name and skip the copy.
pub fn is_normalized_path(path: &str) -> bool {
    !path.as_bytes().contains(&b'\\') && !has_relative_path_segment(path) && !is_bare_root(path)
}

/// A root with no trailing separator ("c:", "//server"), which
/// `normalize_path` gives a separator (ts#64159).
fn is_bare_root(path: &str) -> bool {
    let root_length = get_root_length(path);
    root_length != 0 && root_length == path.len() && !has_trailing_directory_separator(path)
}

// Go: tspath/path.go:612 GetCanonicalFileName (at 673a5f17d713;
// ts#64159 makes it CaseSensitivity.PathKey, tspath/pathkey.go:43)
pub fn get_canonical_file_name(file_name: &str, use_case_sensitive_file_names: bool) -> String {
    if use_case_sensitive_file_names {
        return file_name.to_string();
    }
    to_file_name_lower_case(file_name)
}

// Go: tspath/path.go:629 TrimFilePathPrefix (at 673a5f17d713, tsgo#4900; ts#64159
// makes it CaseSensitivity.TrimPrefix, tspath/path.go:1031)
// TrimFilePathPrefix removes prefix from the start of path, honoring
// useCaseSensitiveFileNames the same way GetCanonicalFileName does. It returns
// the remainder of path if path starts with prefix.
//
// This must not slice path using len(prefix): case-folding (as performed by
// GetCanonicalFileName) can change a string's UTF-8 byte length without
// changing its rune count (e.g. the Kelvin sign '\u212A' case-folds to the
// single-byte 'k'), so path and prefix can disagree in byte length even when
// one is (a case-insensitive match for) a prefix of the other.
// PORT: Go returns `path, false` when path does not start with prefix; this
// returns `None`. `path` and `prefix` are port forms (see
// `scanner_util::GO_STRING_MARKER`): the test and the cut use the Go bytes,
// and the remainder is in the value form (`go_slice`).
pub fn trim_file_path_prefix<'a>(
    path: &'a str,
    prefix: &str,
    use_case_sensitive_file_names: bool,
) -> Option<Cow<'a, str>> {
    if use_case_sensitive_file_names {
        if !go_has_prefix(path, prefix) {
            return None;
        }
        return Some(go_slice(path, go_len(prefix), go_len(path)));
    }
    let canonical_prefix =
        get_canonical_file_name(prefix, false /*useCaseSensitiveFileNames*/);
    if !go_has_prefix(
        &get_canonical_file_name(path, false /*useCaseSensitiveFileNames*/),
        &canonical_prefix,
    ) {
        return None;
    }
    Some(trim_rune_count(
        path,
        go_rune_count_in_string(&canonical_prefix),
    ))
}

// Go: tspath/path.go:633 trimRuneCount (tsgo#4900)
// trimRuneCount returns the suffix of s after skipping up to runeCount runes,
// clamping to the end of s if it has fewer runes than runeCount.
// PORT: Go decodes the bytes of `s`, so a byte that is not valid UTF-8 is one
// rune. This decodes the Go bytes of the port form the same way.
fn trim_rune_count(s: &str, rune_count: usize) -> Cow<'_, str> {
    let bytes = go_string_bytes(s);
    let mut i = 0;
    for _ in 0..rune_count {
        if i >= bytes.len() {
            break;
        }
        i += go_decode_rune_bytes(&bytes[i..]).1;
    }
    go_slice(s, i, bytes.len())
}

// PORT: Go `utf8.RuneCountInString` on the Go bytes of the port form `s`:
// each byte that is not valid UTF-8 is one rune.
fn go_rune_count_in_string(s: &str) -> usize {
    let bytes = go_string_bytes(s);
    let mut i = 0;
    let mut count = 0;
    while i < bytes.len() {
        i += go_decode_rune_bytes(&bytes[i..]).1;
        count += 1;
    }
    count
}

// Go: tspath/path.go:665 ToFileNameLowerCase
// We convert the file names to lower case as key for file name on case insensitive file system
// While doing so we need to handle special characters (eg \u0130) to ensure that we dont convert
// it to lower case, fileName with its lowercase form can exist along side it.
// Handle special characters and make those case sensitive instead
//
// Because \u0130 is special where in its lowercase character has its own
// upper case form we cant convert its case.
// Rest special characters are either already in lower case format or
// they have corresponding upper case character so they dont need special handling
// PORT: the Go `unsafe.String` fast path is a plain ASCII lowercase copy.
// `file_name` is a port form (see `scanner_util::GO_STRING_MARKER`). Go
// `strings.Map` writes each byte that is not valid UTF-8 as U+FFFD, and
// `go_map_runes` does the same.
pub fn to_file_name_lower_case(file_name: &str) -> String {
    const I_WITH_DOT: char = '\u{0130}';

    let mut ascii = true;
    let mut needs_lower = false;
    for &c in file_name.as_bytes() {
        if c >= 0x80 {
            ascii = false;
            break;
        }
        if c.is_ascii_uppercase() {
            needs_lower = true;
        }
    }
    if ascii {
        if !needs_lower {
            return file_name.to_string();
        }
        return file_name.to_ascii_lowercase();
    }

    go_map_runes(file_name, |r| {
        if r == I_WITH_DOT {
            r
        } else {
            unicode::to_lower(r)
        }
    })
}

// Go: tspath/path.go:723 ToPath (at 673a5f17d713;
// ts#64159 makes it CaseSensitivity.PathKey, tspath/pathkey.go:43)
// ts#64544: an encoded dynamic file name keeps its case (see
// `canonical_dynamic_uri_path`).
pub fn to_path(file_name: &str, base_path: &str, use_case_sensitive_file_names: bool) -> Path {
    // PERF: a rooted name that is normal already is its own normal path
    // (`normalize_path`), so only the canonical copy is made.
    if is_rooted_disk_path(file_name) && is_normalized_path(file_name) {
        if is_encoded_dynamic_file_name(file_name) {
            return Path(canonical_dynamic_uri_path(file_name).into_owned());
        }
        return Path(get_canonical_file_name(
            file_name,
            use_case_sensitive_file_names,
        ));
    }
    let non_canonicalized_path = if is_rooted_disk_path(file_name) {
        normalize_path(file_name)
    } else {
        get_normalized_absolute_path(file_name, base_path)
    };
    if is_encoded_dynamic_file_name(&non_canonicalized_path) {
        return Path(canonical_dynamic_uri_path(&non_canonicalized_path).into_owned());
    }
    Path(get_canonical_file_name(
        &non_canonicalized_path,
        use_case_sensitive_file_names,
    ))
}

// Go: tspath/path.go:714 RemoveTrailingDirectorySeparator
pub fn remove_trailing_directory_separator(path: &str) -> &str {
    if has_trailing_directory_separator(path) {
        return &path[..path.len() - 1];
    }
    path
}

impl Path {
    // Go: tspath/path.go:714 Path.RemoveTrailingDirectorySeparator
    pub fn remove_trailing_directory_separator(&self) -> Path {
        Path(remove_trailing_directory_separator(&self.0).to_string())
    }
}

// Go: tspath/path.go:744 RemoveTrailingDirectorySeparators (at 673a5f17d713; removed by ts#64159)
pub fn remove_trailing_directory_separators(path: &str) -> &str {
    let mut path = path;
    while has_trailing_directory_separator(path) {
        path = remove_trailing_directory_separator(path);
    }
    path
}

// Go: tspath/path.go:728 EnsureTrailingDirectorySeparator
pub fn ensure_trailing_directory_separator(path: &str) -> String {
    if !has_trailing_directory_separator(path) {
        return format!("{path}/");
    }

    path.to_string()
}

impl Path {
    // Go: tspath/path.go:728 Path.EnsureTrailingDirectorySeparator
    pub fn ensure_trailing_directory_separator(&self) -> Path {
        Path(ensure_trailing_directory_separator(&self.0))
    }
}

//// Relative Paths

// Go: tspath/path.go:738 GetPathComponentsRelativeTo
pub fn get_path_components_relative_to(
    from: &str,
    to: &str,
    options: &ComparePathsOptions,
) -> Vec<String> {
    path_components_relative_to(
        reduce_path_components(get_path_components(from, &options.current_directory)),
        reduce_path_components(get_path_components(to, &options.current_directory)),
        options.use_case_sensitive_file_names,
    )
}

// Go: tspath/path.go:754 getPathComponentsRelativeTo
// PORT: the case sensitivity is the bool.
fn path_components_relative_to(
    from_components: Vec<String>,
    to_components: Vec<String>,
    use_case_sensitive_file_names: bool,
) -> Vec<String> {
    let mut start = 0;
    let max_common_components = from_components.len().min(to_components.len());
    let string_equaler = get_string_equality_comparer(!use_case_sensitive_file_names);
    while start < max_common_components {
        let from_component = &from_components[start];
        let to_component = &to_components[start];
        if start == 0 {
            if !equate_string_case_insensitive(from_component, to_component) {
                break;
            }
        } else if !string_equaler(from_component, to_component) {
            break;
        }
        start += 1;
    }

    if start == 0 {
        return to_components;
    }

    let num_dot_dot_slashes = from_components.len() - start;
    let mut result = Vec::with_capacity(1 + num_dot_dot_slashes + to_components.len() - start);

    result.push(String::new());
    // Add all the relative components until we hit a common directory.
    for _ in 0..num_dot_dot_slashes {
        result.push("..".to_string());
    }
    // Now add all the remaining components of the "to" path.
    result.extend(to_components.into_iter().skip(start));

    result
}

// Go: tspath/path.go:795 GetRelativePathFromDirectory
pub fn get_relative_path_from_directory(
    from_directory: &str,
    to: &str,
    options: &ComparePathsOptions,
) -> String {
    if (get_root_length(from_directory) > 0) != (get_root_length(to) > 0) {
        panic!("paths must either both be absolute or both be relative");
    }
    let path_components = get_path_components_relative_to(from_directory, to, options);
    get_path_from_path_components(&path_components)
}

// Go: tspath/path.go:811 GetRelativePathFromFile
pub fn get_relative_path_from_file(from: &str, to: &str, options: &ComparePathsOptions) -> String {
    ensure_path_is_non_module_name(&get_relative_path_from_directory(
        &get_directory_path(from),
        to,
        options,
    ))
}

// Go: tspath/relative_path.go:82 CaseSensitivity.RelativePathFromPath (ts#64159)
/// The path from `directory` to `path`, both rooted and normalized, without
/// reducing "." or "..". `None` when the result is rooted: the roots differ
/// (R4: the caller then uses the absolute name). Dynamic roots compare
/// exactly, the rest by the case sensitivity.
// PORT: Go `(RelativePath, bool)` is `Option<String>`; the case sensitivity
// is the bool. Go RelativePathFromDirectory (:76) is this function for a
// file name.
pub fn relative_path_from_directory(
    directory: &str,
    path: &str,
    use_case_sensitive_file_names: bool,
) -> Option<String> {
    let relative =
        relative_path_from_normalized_paths(directory, path, use_case_sensitive_file_names);
    if get_encoded_root_length(&relative) != 0 {
        return None;
    }
    Some(relative)
}

// Go: tspath/rooted_path.go:607 relativePathFromNormalizedPaths (ts#64159)
// When one name is an encoded dynamic name, Go trims one trailing separator
// of both root components, compares them exactly, and gives the trimmed
// components to getPathComponentsRelativeTo, which compares index 0 again.
// So a bare dynamic root and the same root with its separator are one root.
fn relative_path_from_normalized_paths(
    from: &str,
    to: &str,
    use_case_sensitive_file_names: bool,
) -> String {
    let mut from_components = path_components(from, get_root_length(from));
    let mut to_components = path_components(to, get_root_length(to));
    let mut use_case_sensitive_file_names = use_case_sensitive_file_names;
    if is_encoded_dynamic_file_name(from) || is_encoded_dynamic_file_name(to) {
        for root in [&mut from_components[0], &mut to_components[0]] {
            if root.ends_with('/') {
                root.pop();
            }
        }
        if from_components[0] != to_components[0] {
            return to.to_string();
        }
        use_case_sensitive_file_names = true;
    }
    get_path_from_path_components(&path_components_relative_to(
        from_components,
        to_components,
        use_case_sensitive_file_names,
    ))
}

// Go: tspath/rooted_path.go:106 hasRootedURLSuffix (ts#64159)
/// A URL root with a query or a fragment after its scheme.
// PORT: private in Go tspath. The Rust ports of the rooted path
// constructors that use it are outside tspath (`transpile.rs`,
// `contentmapper/hostimpl.rs`).
pub fn has_rooted_url_suffix(path: &str) -> bool {
    if !has_url_root(path) {
        return false;
    }
    let after_scheme = path.split_once("://").map_or("", |(_, after)| after);
    after_scheme.contains(['?', '#'])
}

// Go: tspath/rooted_path.go:114 hasURLRoot (ts#64159)
// PORT: private in Go tspath (see `has_rooted_url_suffix`).
pub fn has_url_root(path: &str) -> bool {
    get_encoded_root_length(path) < 0 && path.contains("://")
}

// Go: tspath/rooted_path.go:575 CaseSensitivity.trimContainedPath (ts#64159)
/// The rest of `child` below `parent` (both rooted and normalized), with no
/// leading separator; `Some("")` when they are the same path. `None` when
/// `child` is not `parent` or below it. Roots compare without case, dynamic
/// roots exactly; the rest by the file system's case sensitivity. Go
/// `CaseSensitivity.ContainsPath` is `is_some()`, `StartsWithDirectory` is a
/// non-empty result, `RelativePathWithinDirectory` the result.
pub fn relative_path_within_directory<'a>(
    parent: &str,
    child: &'a str,
    use_case_sensitive_file_names: bool,
) -> Option<Cow<'a, str>> {
    if parent.is_empty() || child.is_empty() {
        return None;
    }
    let parent_root_length = get_root_length(parent);
    let child_root_length = get_root_length(child);
    let mut parent_root = &parent[..parent_root_length];
    let mut child_root = &child[..child_root_length];
    let mut case_sensitive = use_case_sensitive_file_names;
    let roots_equal = if is_encoded_dynamic_file_name(parent) || is_encoded_dynamic_file_name(child)
    {
        parent_root = parent_root.strip_suffix('/').unwrap_or(parent_root);
        child_root = child_root.strip_suffix('/').unwrap_or(child_root);
        case_sensitive = true;
        parent_root == child_root
    } else {
        equate_string_case_insensitive(parent_root, child_root)
    };
    if !roots_equal {
        return None;
    }
    let relative = match trim_file_path_prefix(
        &child[child_root_length..],
        &parent[parent_root_length..],
        case_sensitive,
    ) {
        Some(relative) => relative,
        None => return None,
    };
    if !relative.is_empty()
        && !has_trailing_directory_separator(parent)
        && parent.len() != parent_root_length
        && !relative.starts_with('/')
    {
        return None;
    }
    Some(match relative {
        Cow::Borrowed(r) => Cow::Borrowed(r.strip_prefix('/').unwrap_or(r)),
        Cow::Owned(r) => Cow::Owned(r.strip_prefix('/').unwrap_or(&r).to_string()),
    })
}

// Go: tspath/path.go:815 ConvertToRelativePath
pub fn convert_to_relative_path(
    absolute_or_relative_path: &str,
    options: &ComparePathsOptions,
) -> String {
    if !is_rooted_disk_path(absolute_or_relative_path) {
        return absolute_or_relative_path.to_string();
    }

    get_relative_path_to_directory_or_url(
        &options.current_directory,
        absolute_or_relative_path,
        false, /*isAbsolutePathAnUrl*/
        options,
    )
}

// Go: tspath/path.go:823 GetRelativePathToDirectoryOrUrl
pub fn get_relative_path_to_directory_or_url(
    directory_path_or_url: &str,
    relative_or_absolute_path: &str,
    is_absolute_path_an_url: bool,
    options: &ComparePathsOptions,
) -> String {
    let mut path_components =
        get_path_components_relative_to(directory_path_or_url, relative_or_absolute_path, options);

    let first_component = path_components[0].clone();
    if is_absolute_path_an_url && is_rooted_disk_path(&first_component) {
        let prefix = if first_component.as_bytes()[0] == DIRECTORY_SEPARATOR {
            "file://"
        } else {
            "file:///"
        };
        path_components[0] = format!("{prefix}{first_component}");
    }

    get_path_from_path_components(&path_components)
}

// Go: tspath/path.go:883 GetBaseFileName
// Gets the portion of a path following the last (non-terminal) separator (`/`).
// Semantics align with NodeJS's `path.basename` except that we support URL's as well.
// If the base name has any one of the provided extensions, it is removed.
pub fn get_base_file_name(path: &str) -> String {
    let path = normalize_slashes(path);

    // if the path provided is itself the root, then it has no file name.
    let root_length = get_root_length(&path);
    if root_length == path.len() {
        return String::new();
    }

    // return the trailing portion of the path starting after the last (non-terminal) directory
    // separator but not including any trailing directory separator.
    let path = remove_trailing_directory_separator(&path);
    let start = (get_root_length(path) as isize).max(last_index_byte(path, DIRECTORY_SEPARATOR) + 1)
        as usize;
    path[start..].to_string()
}

// Go: tspath/path.go:911 GetAnyExtensionFromPath
// Gets the file extension for a path.
// If extensions are provided, gets the file extension for a path, provided it is one of the provided extensions.
pub fn get_any_extension_from_path(path: &str, extensions: &[&str], ignore_case: bool) -> String {
    // Retrieves any string from the final "." onwards from a base file name.
    // Unlike extensionFromPath, which throws an exception on unrecognized extensions.
    if !extensions.is_empty() {
        return get_any_extension_from_path_worker(
            remove_trailing_directory_separator(path),
            extensions,
            get_string_equality_comparer(ignore_case),
        );
    }

    let base_file_name = get_base_file_name(path);
    if let Some(extension_index) = base_file_name.rfind('.') {
        return base_file_name[extension_index..].to_string();
    }
    String::new()
}

// Go: tspath/path.go:930 GetLongestExtensionFromPath (tsgo#4712)
/// The longest of `extensions` that ends `path`, as it is written in
/// `path`, or "" when none does.
pub fn get_longest_extension_from_path<S: AsRef<str>>(
    path: &str,
    extensions: &[S],
    ignore_case: bool,
) -> String {
    let path = remove_trailing_directory_separator(path);
    let comparer = get_string_equality_comparer(ignore_case);
    let mut longest = String::new();
    for extension in extensions {
        let extension = extension.as_ref();
        if extension.len() > longest.len() {
            let matched = try_get_extension_from_path_worker(path, extension, comparer);
            if !matched.is_empty() {
                longest = matched;
            }
        }
    }
    longest
}

// Go: tspath/path.go:944 getAnyExtensionFromPathWorker
fn get_any_extension_from_path_worker(
    path: &str,
    extensions: &[&str],
    string_equality_comparer: fn(&str, &str) -> bool,
) -> String {
    for extension in extensions {
        let result = try_get_extension_from_path_worker(path, extension, string_equality_comparer);
        if !result.is_empty() {
            return result;
        }
    }
    String::new()
}

// Go: tspath/path.go:954 tryGetExtensionFromPath
// PORT: renamed to `try_get_extension_from_path_worker`. The exported Go
// `TryGetExtensionFromPath` (extension.go) has the same snake name.
fn try_get_extension_from_path_worker(
    path: &str,
    extension: &str,
    string_equality_comparer: fn(&str, &str) -> bool,
) -> String {
    let extension = if !extension.starts_with('.') {
        format!(".{extension}")
    } else {
        extension.to_string()
    };
    let pb = path.as_bytes();
    if pb.len() >= extension.len() && pb[pb.len() - extension.len()] == b'.' {
        let path_extension = &path[path.len() - extension.len()..];
        if string_equality_comparer(path_extension, &extension) {
            return path_extension.to_string();
        }
    }
    String::new()
}

// Go: tspath/path.go:967 PathIsRelative
pub fn path_is_relative(path: &str) -> bool {
    // True if path is ".", "..", or starts with "./", "../", ".\\", or "..\\".

    if path == "." || path == ".." {
        return true;
    }

    let b = path.as_bytes();
    if b.len() >= 2 && b[0] == b'.' && (b[1] == b'/' || b[1] == b'\\') {
        return true;
    }

    if b.len() >= 3 && b[0] == b'.' && b[1] == b'.' && (b[2] == b'/' || b[2] == b'\\') {
        return true;
    }

    false
}

// Go: tspath/path.go:987 EnsurePathIsNonModuleName
// EnsurePathIsNonModuleName ensures a path is either absolute (prefixed with `/` or `c:`) or dot-relative (prefixed
// with `./` or `../`) so as not to be confused with an unprefixed module name.
pub fn ensure_path_is_non_module_name(path: &str) -> String {
    if !path_is_absolute(path) && !path_is_relative(path) {
        return format!("./{path}");
    }
    path.to_string()
}

// Go: tspath/path.go:994 IsExternalModuleNameRelative
pub fn is_external_module_name_relative(module_name: &str) -> bool {
    // TypeScript 1.0 spec (April 2014): 11.2.1
    // An external module name is "relative" if the first term is "." or "..".
    // Update: We also consider a path like `C:\foo.ts` "relative" because we do not search for it in `node_modules` or treat it as an ambient module.
    path_is_relative(module_name) || is_rooted_disk_path(module_name)
}

// Go: tspath/path.go:988 ComparePathsOptions (at 673a5f17d713;
// ts#64159 makes it CaseSensitivity, tspath/path.go:1003)
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ComparePathsOptions {
    pub use_case_sensitive_file_names: bool,
    pub current_directory: String,
}

impl ComparePathsOptions {
    // Go: tspath/path.go:993 ComparePathsOptions.GetComparer (at 673a5f17d713;
    // ts#64159 makes it CaseSensitivity.GetComparer, tspath/path.go:1053)
    pub fn get_comparer(&self) -> fn(&str, &str) -> i32 {
        get_string_comparer(!self.use_case_sensitive_file_names)
    }

    // Go: tspath/path.go:997 ComparePathsOptions.getEqualityComparer (at 673a5f17d713;
    // ts#64159 makes it CaseSensitivity.getEqualityComparer, tspath/path.go:1057)
    fn get_equality_comparer(&self) -> fn(&str, &str) -> bool {
        get_string_equality_comparer(!self.use_case_sensitive_file_names)
    }
}

// Go: tspath/path.go:1067 ComparePaths
pub fn compare_paths(a: &str, b: &str, options: &ComparePathsOptions) -> i32 {
    let a = combine_paths(&options.current_directory, &[a]);
    let b = combine_paths(&options.current_directory, &[b]);

    if a == b {
        return 0;
    }
    if a.is_empty() {
        return -1;
    }
    if b.is_empty() {
        return 1;
    }

    // NOTE: Performance optimization - shortcut if the root segments differ as there would be no
    //       need to perform path reduction.
    let a_root = &a[..get_root_length(&a)];
    let b_root = &b[..get_root_length(&b)];
    let result = compare_strings_case_insensitive(a_root, b_root);
    if result != 0 {
        return result;
    }

    // NOTE: Performance optimization - shortcut if there are no relative path segments in
    //       the non-root portion of the path
    let a_rest = &a[a_root.len()..];
    let b_rest = &b[b_root.len()..];
    if !has_relative_path_segment(a_rest) && !has_relative_path_segment(b_rest) {
        return options.get_comparer()(a_rest, b_rest);
    }

    // The path contains a relative path segment. Normalize the paths and perform a slower component
    // by component comparison.
    let a_components = reduce_path_components(get_path_components(&a, ""));
    let b_components = reduce_path_components(get_path_components(&b, ""));
    let shared_length = a_components.len().min(b_components.len());
    for i in 1..shared_length {
        let result = options.get_comparer()(&a_components[i], &b_components[i]);
        if result != 0 {
            return result;
        }
    }
    a_components.len().cmp(&b_components.len()) as i32
}

// Go: tspath/path.go:1046 ComparePathsCaseSensitive (at 673a5f17d713; removed by ts#64159)
pub fn compare_paths_case_sensitive(a: &str, b: &str, current_directory: &str) -> i32 {
    compare_paths(
        a,
        b,
        &ComparePathsOptions {
            use_case_sensitive_file_names: true,
            current_directory: current_directory.to_string(),
        },
    )
}

// Go: tspath/path.go:1050 ComparePathsCaseInsensitive (at 673a5f17d713; removed by ts#64159)
pub fn compare_paths_case_insensitive(a: &str, b: &str, current_directory: &str) -> i32 {
    compare_paths(
        a,
        b,
        &ComparePathsOptions {
            use_case_sensitive_file_names: false,
            current_directory: current_directory.to_string(),
        },
    )
}

// Go: tspath/rooted_path.go:524 CaseSensitivity.compareRootedText (ts#64159)
// Compares rooted, normalized path text and does not reduce "." or ".."
// components (Go CaseSensitivity.ComparePaths, CompareFilePaths and
// CompareFileNameStems call it). `compare_paths` reduces them.
pub fn compare_rooted_text(a: &str, b: &str, use_case_sensitive_file_names: bool) -> i32 {
    if a == b {
        return 0;
    }
    if a.is_empty() {
        return -1;
    }
    if b.is_empty() {
        return 1;
    }

    if is_encoded_dynamic_file_name(a) || is_encoded_dynamic_file_name(b) {
        return compare_strings_case_sensitive(
            &canonical_dynamic_uri_path(a),
            &canonical_dynamic_uri_path(b),
        );
    }
    let a_root_length = get_root_length(a);
    let b_root_length = get_root_length(b);
    let result = compare_strings_case_insensitive(&a[..a_root_length], &b[..b_root_length]);
    if result != 0 {
        return result;
    }
    get_string_comparer(!use_case_sensitive_file_names)(&a[a_root_length..], &b[b_root_length..])
}

// Go: tspath/path.go:1111 ContainsPath
pub fn contains_path(parent: &str, child: &str, options: &ComparePathsOptions) -> bool {
    let parent = combine_paths(&options.current_directory, &[parent]);
    let child = combine_paths(&options.current_directory, &[child]);
    if parent.is_empty() || child.is_empty() {
        return false;
    }
    if parent == child {
        return true;
    }
    let parent_components = reduce_path_components(get_path_components(&parent, ""));
    let child_components = reduce_path_components(get_path_components(&child, ""));
    if child_components.len() < parent_components.len() {
        return false;
    }

    let component_comparer = options.get_equality_comparer();
    for (i, parent_component) in parent_components.iter().enumerate() {
        let comparer: fn(&str, &str) -> bool = if i == 0 {
            equate_string_case_insensitive
        } else {
            component_comparer
        };
        if !comparer(parent_component, &child_components[i]) {
            return false;
        }
    }

    true
}

impl Path {
    // Go: tspath/path.go:1111 Path.ContainsPath
    // ContainsPath checks whether child is contained within or equal to p.
    // Since Path values are already rooted, reduced, and case-canonicalized,
    // this is a simple string prefix check.
    pub fn contains_path(&self, child: &Path) -> bool {
        let p = self.0.as_bytes();
        let c = child.0.as_bytes();
        if p.is_empty() {
            return false;
        }
        self == child
            || c.len() > p.len()
                && c.starts_with(p)
                && (p[p.len() - 1] == b'/' || c[p.len()] == b'/')
    }
}

// Go: tspath/path.go:1142 FileExtensionIs
pub fn file_extension_is(path: &str, extension: &str) -> bool {
    path.len() > extension.len() && path.ends_with(extension)
}

// Go: tspath/path.go:1148 ForEachAncestorDirectoryStoppingAtGlobalCache
// Calls `callback` on `directory` and every ancestor directory it has, returning the first defined result.
// Stops at global cache location
pub fn for_each_ancestor_directory_stopping_at_global_cache<T: Default>(
    global_cache_location: &str,
    directory: &str,
    mut callback: impl FnMut(&str) -> (T, bool),
) -> T {
    let (result, _) = for_each_ancestor_directory(directory, |ancestor_directory| {
        let (result, stop) = callback(ancestor_directory);
        if stop || ancestor_directory == global_cache_location {
            return (result, true);
        }
        (result, false)
    });
    result
}

// Go: tspath/path.go:1163 ForEachAncestorDirectory
// PORT: the Go zero value of T is `T::default()`.
pub fn for_each_ancestor_directory<T: Default>(
    directory: &str,
    mut callback: impl FnMut(&str) -> (T, bool),
) -> (T, bool) {
    let mut directory = directory.to_string();
    loop {
        let (result, stop) = callback(&directory);
        if stop {
            return (result, true);
        }

        let parent_path = get_directory_path(&directory);
        if parent_path == directory {
            return (T::default(), false);
        }

        directory = parent_path;
    }
}

impl Path {
    // Go: tspath/path.go:1163 (Path).ForEachAncestorDirectory (ts#63902)
    pub fn for_each_ancestor_directory<T: Default>(
        &self,
        mut callback: impl FnMut(Path) -> (T, bool),
    ) -> (T, bool) {
        for_each_ancestor_directory(&self.0, |directory| callback(Path(directory.to_string())))
    }
}

// Go: tspath/path.go:1180 HasExtension
pub fn has_extension(file_name: &str) -> bool {
    get_base_file_name(file_name).contains('.')
}

// Go: tspath/path.go:1184 SplitVolumePath
// PORT: Go `(volume, rest, ok)`. The volume is two ASCII bytes, so
// `strings.ToLower` is an ASCII lowercase.
pub fn split_volume_path(path: &str) -> (String, &str, bool) {
    let b = path.as_bytes();
    if b.len() >= 2 && is_volume_character(b[0]) && b[1] == b':' {
        return (path[0..2].to_ascii_lowercase(), &path[2..], true);
    }
    (String::new(), path, false)
}

// Go: tspath/path.go:1158 GetCommonParents (at 673a5f17d713;
// ts#64159 makes it getCommonParents, tspath/path.go:1199)
// GetCommonParents returns the smallest set of directories that are parents of all paths with
// at least `minComponents` directory components. Any path that has fewer than `minComponents` directory components
// will be returned in the second return value.
// PORT: a Go nil slice or nil map is an empty `Vec` or set. Go map iteration
// order is random, so callers cannot depend on the set order.
pub fn get_common_parents(
    paths: &[String],
    min_components: i32,
    get_path_components: &dyn Fn(&str, &str) -> Vec<String>,
    options: &ComparePathsOptions,
) -> (Vec<String>, FxHashSet<String>) {
    if min_components < 1 {
        panic!("minComponents must be at least 1");
    }
    if paths.is_empty() {
        return (Vec::new(), FxHashSet::default());
    }
    if paths.len() == 1 {
        if (reduce_path_components(get_path_components(&paths[0], &options.current_directory)).len()
            as i32)
            < min_components
        {
            let mut ignored = FxHashSet::default();
            ignored.insert(paths[0].clone());
            return (Vec::new(), ignored);
        }
        return (paths.to_vec(), FxHashSet::default());
    }

    let mut ignored = FxHashSet::default();
    let mut path_components: Vec<Vec<String>> = Vec::with_capacity(paths.len());
    for path in paths {
        let components =
            reduce_path_components(get_path_components(path, &options.current_directory));
        if (components.len() as i32) < min_components {
            ignored.insert(path.clone());
        } else {
            path_components.push(components);
        }
    }

    let results = get_common_parents_worker(&path_components, min_components, options);
    let result_paths = results
        .iter()
        .map(|comps| get_path_from_path_components(comps))
        .collect();

    (result_paths, ignored)
}

// Go: tspath/path.go:1238 getCommonParentsWorker
// ts#64493: each result gets its own copy of the group head (Go no longer
// appends into the head's backing array). The function is used only by
// project watching, which is out of scope for the frontend.
fn get_common_parents_worker(
    component_groups: &[Vec<String>],
    min_components: i32,
    options: &ComparePathsOptions,
) -> Vec<Vec<String>> {
    if component_groups.is_empty() {
        return Vec::new();
    }
    // Determine the maximum depth we can consider
    let mut max_depth = component_groups[0].len();
    for comps in &component_groups[1..] {
        let l = comps.len();
        if l < max_depth {
            max_depth = l;
        }
    }

    let equality = options.get_equality_comparer();
    for last_common_index in 0..max_depth {
        let candidate = &component_groups[0][last_common_index];
        for comps in &component_groups[1..] {
            if !equality(candidate, &comps[last_common_index]) {
                // divergence
                if (last_common_index as i32) < min_components {
                    // Not enough components, we need to fan out
                    let mut ordered_groups: Vec<Path> = Vec::new();
                    let mut new_groups: FxHashMap<Path, (Vec<String>, Vec<Vec<String>>)> =
                        FxHashMap::default();
                    for g in component_groups {
                        let key = to_path(
                            &g[last_common_index],
                            &options.current_directory,
                            options.use_case_sensitive_file_names,
                        );
                        let entry = new_groups.entry(key.clone()).or_insert_with(|| {
                            ordered_groups.push(key);
                            (Vec::new(), Vec::new())
                        });
                        entry.0 = g[..last_common_index + 1].to_vec();
                        entry.1.push(g[last_common_index + 1..].to_vec());
                    }
                    // PORT: Go sorts by bytes (see `compare_go_bytes`).
                    ordered_groups.sort_by(|a, b| compare_go_bytes(a.as_str(), b.as_str()));
                    let mut result = Vec::with_capacity(new_groups.len());
                    for key in &ordered_groups {
                        let (head, tails) = &new_groups[key];
                        let sub_results = get_common_parents_worker(
                            tails,
                            min_components - (last_common_index as i32 + 1),
                            options,
                        );
                        for sr in sub_results {
                            if sr.is_empty() {
                                result.push(head.clone());
                            } else {
                                result.push([head.as_slice(), sr.as_slice()].concat());
                            }
                        }
                    }
                    return result;
                }
                return vec![component_groups[0][..last_common_index].to_vec()];
            }
        }
    }

    vec![component_groups[0][..max_depth].to_vec()]
}

// Go: tspath/path.go:1298 StartsWithDirectory
pub fn starts_with_directory(
    file_name: &str,
    directory_name: &str,
    use_case_sensitive_file_names: bool,
) -> bool {
    if directory_name.is_empty() {
        return false;
    }

    let canonical_file_name = get_canonical_file_name(file_name, use_case_sensitive_file_names);
    let canonical_directory_name =
        get_canonical_file_name(directory_name, use_case_sensitive_file_names);
    let canonical_directory_name = canonical_directory_name
        .strip_suffix('/')
        .unwrap_or(&canonical_directory_name);
    let canonical_directory_name = canonical_directory_name
        .strip_suffix('\\')
        .unwrap_or(canonical_directory_name);

    canonical_file_name.starts_with(&format!("{canonical_directory_name}/"))
        || canonical_file_name.starts_with(&format!("{canonical_directory_name}\\"))
}

// Go: tspath/path.go:1312 CompareNumberOfDirectorySeparators
pub fn compare_number_of_directory_separators(path1: &str, path2: &str) -> i32 {
    path1.matches('/').count().cmp(&path2.matches('/').count()) as i32
}

// Go: tspath/ignoredpaths.go:5 ignoredPaths
const IGNORED_PATHS: [&str; 3] = ["/node_modules/.", "/.git", ".#"];

// Go: tspath/ignoredpaths.go:20 ContainsIgnoredPath
pub fn contains_ignored_path(path: &str) -> bool {
    for pattern in IGNORED_PATHS {
        if path.contains(pattern) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod relative_segment_tests {
    use super::{
        has_dot_or_empty_segment, has_relative_path_segment_go, is_normalized_path, normalize_path,
        to_path,
    };

    /// `is_normalized_path` is true only for a name that `normalize_path`
    /// keeps, and the `to_path` fast path for it gives the path of the slow
    /// path, for every name of up to 6 bytes over '.', '/', '\\', ':', 'a'
    /// and 'B' (so rooted, drive-rooted and relative names), on both kinds
    /// of file system.
    #[test]
    fn normalized_path_fast_paths_match() {
        let slow_to_path = |name: &str, base: &str, case_sensitive: bool| {
            let normal = if super::is_rooted_disk_path(name) {
                normalize_path(name)
            } else {
                super::get_normalized_absolute_path(name, base)
            };
            super::get_canonical_file_name(&normal, case_sensitive)
        };
        let mut level = vec![String::new()];
        for _ in 0..6 {
            level = level
                .iter()
                .flat_map(|path| ['.', '/', '\\', ':', 'a', 'B'].map(|c| format!("{path}{c}")))
                .collect();
            for path in &level {
                if is_normalized_path(path) {
                    assert_eq!(normalize_path(path), *path, "{path}");
                }
                for case_sensitive in [true, false] {
                    assert_eq!(
                        to_path(path, "/base/Dir", case_sensitive).0,
                        slow_to_path(path, "/base/Dir", case_sensitive),
                        "{path} {case_sensitive}"
                    );
                }
            }
        }
    }

    /// The fast `has_relative_path_segment` gives Go's answer for every path
    /// of up to 8 bytes over '.', '/' and 'a'.
    #[test]
    fn has_relative_path_segment_matches_go() {
        let mut level = vec![String::new()];
        for _ in 0..8 {
            level = level
                .iter()
                .flat_map(|path| ['.', '/', 'a'].map(|c| format!("{path}{c}")))
                .collect();
            for path in &level {
                assert_eq!(
                    has_dot_or_empty_segment(path.as_bytes()),
                    has_relative_path_segment_go(path),
                    "{path}"
                );
            }
        }
    }
}

#[cfg(test)]
mod unicode_case_tests {
    use super::{compare_strings_case_insensitive, to_path};

    // Go `ToPath` on a case-insensitive host (macOS, Windows) lowers with
    // `unicode.ToLower` (tspath/path.go:674 ToFileNameLowerCase). The pin N
    // oracle is go1.27.1 (Unicode 17.0.0), where U+A7CB lowers to U+0264 and
    // U+10D50 to U+10D70. go1.26.8 (Unicode 15.0.0) kept them.
    #[test]
    fn to_path_lowers_unicode_17_letters() {
        assert_eq!(
            to_path("/Proj/\u{A7CB}\u{10D50}.ts", "/", false).0,
            "/proj/\u{264}\u{10D70}.ts"
        );
        assert_eq!(
            to_path("/Proj/\u{A7CB}.ts", "/", true).0,
            "/Proj/\u{A7CB}.ts"
        );
        assert_eq!(
            compare_strings_case_insensitive("/a/\u{A7CB}", "/a/\u{264}"),
            0
        );
    }
}

#[cfg(test)]
mod directory_path_tests {
    use super::{
        get_directory_path, get_root_length, normalize_slashes, normalize_slashes_cow,
        remove_trailing_directory_separator,
    };
    use std::borrow::Cow;

    /// Go's `GetDirectoryPath` (tspath/path.go:251) as Go writes it: copy,
    /// then a second string.
    fn go_get_directory_path(path: &str) -> String {
        let path = path.replace('\\', "/");
        let root_length = get_root_length(&path);
        if root_length == path.len() {
            return path;
        }
        let path = remove_trailing_directory_separator(&path);
        let last_slash = path.rfind('/').map_or(-1, |i| i as isize);
        path[..(root_length as isize).max(last_slash) as usize].to_string()
    }

    /// `get_directory_path` gives Go's answer for every name of up to 5
    /// bytes over '/', '\\', ':', '.', 'a' and 'c', alone and after disk,
    /// UNC and URL roots, with and without backslashes.
    #[test]
    fn directory_path_matches_go() {
        let roots = [
            "",
            "/",
            "c:",
            "c:\\",
            "//server/",
            "\\\\server\\share\\",
            "file:///",
            "file:///c:/",
            "file://server/",
            "http://server/",
            "https://h/\u{e4}\\\u{fc}/",
        ];
        let mut level = vec![String::new()];
        let mut names = vec![String::new()];
        for _ in 0..5 {
            level = level
                .iter()
                .flat_map(|path| ['/', '\\', ':', '.', 'a', 'c'].map(|c| format!("{path}{c}")))
                .collect();
            names.extend(level.iter().cloned());
        }
        for root in roots {
            for name in &names {
                let path = format!("{root}{name}");
                assert_eq!(
                    get_directory_path(&path),
                    go_get_directory_path(&path),
                    "{path:?}"
                );
            }
        }
    }

    /// `normalize_slashes_cow` borrows a path without a backslash and
    /// replaces each backslash otherwise.
    #[test]
    fn normalize_slashes_borrows_without_backslash() {
        for path in [
            "",
            "a",
            "/a/b.ts",
            "c:/a",
            "file:///c:/a",
            "http://server/\u{e4}",
        ] {
            assert!(matches!(normalize_slashes_cow(path), Cow::Borrowed(p) if p == path));
            assert_eq!(normalize_slashes(path), path);
        }
        for (path, want) in [
            ("\\", "/"),
            ("a\\b", "a/b"),
            ("c:\\a\\", "c:/a/"),
            ("\\\\server\\share", "//server/share"),
            ("file:///c:\\\u{e4}\\b", "file:///c:/\u{e4}/b"),
        ] {
            assert!(matches!(normalize_slashes_cow(path), Cow::Owned(ref p) if p == want));
            assert_eq!(normalize_slashes(path), want);
        }
    }
}

#[cfg(test)]
mod relative_path_tests {
    use super::{get_directory_path, relative_path_from_directory};

    // Go: tspath/relative_path.go:82 RelativePathFromPath and :94
    // RelativePathFromFileToPath (ts#64159). The answers of Go N'
    // (fed0bf24149f) for these inputs, on both kinds of file system: a
    // relative `path` comes back as it is, a dynamic root that ends in "//"
    // loses one separator, and a result that reads as rooted is None.
    #[test]
    fn relative_path_edge_cases_match_go() {
        let cases: &[(&str, &str, Option<&str>)] = &[
            ("^/~ts-uri~/custom//x", "^/~ts-uri~/custom/", None),
            (
                "^/~ts-uri~/custom//x",
                "^/~ts-uri~/custom//y/a.ts",
                Some("../y/a.ts"),
            ),
            ("^/~ts-uri~/custom/authority", "a.ts", Some("a.ts")),
            ("/project", "a.ts", Some("a.ts")),
            (
                "^/~ts-uri~/custom/authority/src",
                "../a.ts",
                Some("../a.ts"),
            ),
            ("c:/x", "c:", None),
            (
                "^/~ts-uri~/custom/authority/",
                "^/~ts-uri~/custom/authority//a.ts",
                None,
            ),
            (
                "^/~ts-uri~/custom/authority",
                "^/~ts-uri~/custom/authority/Foo.ts",
                Some("Foo.ts"),
            ),
            (
                "/project/src",
                "/project/lib/file.ts",
                Some("../lib/file.ts"),
            ),
            ("c:/project/src", "d:/project/lib/file.ts", None),
        ];
        for case_sensitive in [true, false] {
            for (directory, path, want) in cases {
                assert_eq!(
                    relative_path_from_directory(directory, path, case_sensitive).as_deref(),
                    *want,
                    "{directory} {path} {case_sensitive}"
                );
            }
        }
        // Go RelativePathFromFileToPath is the same from the file's directory.
        assert_eq!(
            relative_path_from_directory(
                &get_directory_path("^/~ts-uri~/custom/authority/a.ts"),
                "b.ts",
                true
            )
            .as_deref(),
            Some("b.ts")
        );
        assert_eq!(
            relative_path_from_directory("/PROJECT/src", "/project/lib/util.ts", false).as_deref(),
            Some("../lib/util.ts")
        );
        assert_eq!(
            relative_path_from_directory("/PROJECT/src", "/project/lib/util.ts", true).as_deref(),
            Some("../../project/lib/util.ts")
        );
    }
}
