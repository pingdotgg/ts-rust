use crate::frontend::prelude::*;

// Go: tsoptions/wildcarddirectories.go:10 getWildcardDirectories
// ts#64159: each directory is a rooted directory path ("c:/", and the root
// itself for a pattern in the root), and its key is the path key
// (`to_path`).
// PORT: Go returns a nil map for an empty include list. Every reader treats
// nil and empty the same, so this returns an empty map. Go deletes subpaths
// while it ranges over the map. The delete test depends only on the path and
// `recursive_keys`, so `retain` gives the same set.
pub fn get_wildcard_directories(
    include: &[String],
    exclude: &[String],
    compare_paths_options: &ComparePathsOptions,
) -> FxHashMap<String, bool> {
    // We watch a directory recursively if it contains a wildcard anywhere in a directory segment
    // of the pattern:
    //
    //  /a/b/**/d   - Watch /a/b recursively to catch changes to any d in any subfolder recursively
    //  /a/b/*/d    - Watch /a/b recursively to catch any d in any immediate subfolder, even if a new subfolder is added
    //  /a/b        - Watch /a/b recursively to catch changes to anything in any recursive subfoler
    //
    // We watch a directory without recursion if it contains a wildcard in the file segment of
    // the pattern:
    //
    //  /a/b/*      - Watch /a/b directly to catch any new file
    //  /a/b/a?z    - Watch /a/b directly to catch any new file matching a?z

    if include.is_empty() {
        return FxHashMap::default();
    }

    let exclude_matcher = new_spec_matcher(
        exclude,
        &compare_paths_options.current_directory,
        Usage::Exclude,
        compare_paths_options.use_case_sensitive_file_names,
    );

    let mut wildcard_directories: FxHashMap<String, bool> = FxHashMap::default();
    let mut wild_card_key_to_path: FxHashMap<String, String> = FxHashMap::default();

    let mut recursive_keys: Vec<String> = Vec::new();

    for file in include {
        let spec = normalize_path(&combine_paths(
            &compare_paths_options.current_directory,
            &[file.as_str()],
        ));
        if let Some(exclude_matcher) = &exclude_matcher
            && exclude_matcher.match_string(&spec)
        {
            continue;
        }

        let match_ = get_wildcard_directory_from_spec(&spec);
        if let Some(match_) = match_ {
            let mut path = match_.path;
            if path.is_empty() {
                path = spec[..get_root_length(&spec)].to_string();
            }
            let key = to_canonical_key(&path, compare_paths_options.use_case_sensitive_file_names);
            let recursive = match_.recursive;

            let existing_path = wild_card_key_to_path.get(&key).cloned();
            let exists_path = existing_path.is_some();
            let mut existing_recursive = false;

            if let Some(existing_path) = &existing_path {
                existing_recursive = wildcard_directories
                    .get(existing_path)
                    .copied()
                    .unwrap_or(false);
            }

            if !exists_path || (!existing_recursive && recursive) {
                let path_to_use = match &existing_path {
                    Some(existing_path) => existing_path.clone(),
                    None => path.clone(),
                };
                wildcard_directories.insert(path_to_use, recursive);

                if !exists_path {
                    wild_card_key_to_path.insert(key.clone(), path);
                }

                if recursive {
                    recursive_keys.push(key);
                }
            }
        }

        // Remove any subpaths under an existing recursively watched directory
        wildcard_directories.retain(|path, _| {
            for recursive_key in &recursive_keys {
                let key =
                    to_canonical_key(path, compare_paths_options.use_case_sensitive_file_names);
                if &key != recursive_key
                    && contains_path(recursive_key, &key, compare_paths_options)
                {
                    return false;
                }
            }
            true
        });
    }

    wildcard_directories
}

// Go: tsoptions/wildcarddirectories.go:85 toCanonicalKey (at 673a5f17d713; removed by ts#64159,
// which keys by tspath.CaseSensitivity.PathKey)
// PORT: Go `PathKey` of a rooted path: the canonical file name
// (`to_path` of a rooted, normal path).
fn to_canonical_key(path: &str, use_case_sensitive_file_names: bool) -> String {
    to_path(path, "", use_case_sensitive_file_names).0
}

/// Go `wildcardDirectoryMatch`: the result of a wildcard directory match.
// Go: tsoptions/wildcarddirectories.go:89 wildcardDirectoryMatch
// ts#64159: no key; the caller makes it from the rooted path.
#[derive(Clone, Debug)]
struct WildcardDirectoryMatch {
    /// Empty when the pattern is in the root (Go: a zero RootedDirectoryPath).
    path: String,
    recursive: bool,
}

/// Go `tspath.RootedDirectoryPathFromAbsolute` of a normal absolute path:
/// a root keeps a trailing separator ("c:" is "c:/").
fn rooted_directory_path(path: &str) -> String {
    let normalized = get_normalized_absolute_path(path, "");
    if get_root_length(&normalized) == normalized.len() {
        return ensure_trailing_directory_separator(&normalized);
    }
    normalized
}

// Go: tsoptions/wildcarddirectories.go:94 getWildcardDirectoryFromSpec
// PORT: Go `strings.ToLower` and Rust `to_lowercase` agree on the ASCII and
// common Unicode cases that appear in paths.
fn get_wildcard_directory_from_spec(spec: &str) -> Option<WildcardDirectoryMatch> {
    // Find the first occurrence of a wildcard character
    if let Some(first_wildcard) = spec.find(['*', '?']) {
        // Find the last directory separator before the wildcard
        if let Some(last_sep_before_wildcard) = spec[..first_wildcard].rfind('/') {
            let path_text = &spec[..last_sep_before_wildcard];
            let path = if path_text.is_empty() {
                String::new()
            } else {
                rooted_directory_path(path_text)
            };
            let last_directory_separator_index = spec.rfind('/');

            // Determine if this should be watched recursively:
            // recursive if the wildcard appears in a directory segment (not just the final file segment)
            // PORT: Go compares with -1 when there is no separator. There is
            // always one here (found above), so `is_some_and` is always true on Some.
            let recursive = last_directory_separator_index.is_some_and(|i| first_wildcard < i);

            return Some(WildcardDirectoryMatch { path, recursive });
        }
    }

    if let Some(last_sep_index) = spec.rfind('/') {
        let last_segment = &spec[last_sep_index + 1..];
        if is_implicit_glob(last_segment) {
            let path = rooted_directory_path(remove_trailing_directory_separator(spec));
            return Some(WildcardDirectoryMatch {
                path,
                recursive: true,
            });
        }
    }

    None
}
