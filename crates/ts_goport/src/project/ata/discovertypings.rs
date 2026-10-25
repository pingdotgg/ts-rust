//! Go `internal/project/ata/discovertypings.go`.
//!
//! PORT: Go iterates `map` values here (`inferredTypings`,
//! `possibleSearchDirs`, the dependency maps and the typing cache). Go map
//! order is random. The port keeps insertion order (`IndexMap`) where it
//! owns the map. The order reaches only log text and the order of the
//! returned slices, which `InstallTypings` sorts.

use crate::project::ata::prelude::*;

use crate::frontend::core_ext::TypeAcquisition;
use crate::frontend::core_nodemodules;
use crate::frontend::json::{self, JsonDecoder, JsonError, UnmarshalerFrom, json_unmarshal_decode};
use crate::frontend::json_ext::unmarshal_struct_fields;
use crate::frontend::scanner::scanner_p1::{
    utf8_decode_last_rune_in_string, utf8_decode_rune_in_string,
};
use crate::frontend::{packagejson, semver, tspath, vfs};
use crate::project::logging::{self, Logger as _};

// Go: project/ata/discovertypings.go:20 isTypingUpToDate
// PORT: Go `availableTypingVersions map[string]string` can be nil; `None` is
// the nil map (every lookup misses).
pub fn is_typing_up_to_date(
    cached_typing: &CachedTyping,
    available_typing_versions: Option<&FxHashMap<String, String>>,
) -> bool {
    let (mut use_version, ok) = match available_typing_versions
        .and_then(|m| m.get(&format!("ts{}", crate::core::version_major_minor())))
    {
        Some(v) => (v.clone(), true),
        None => (String::new(), false),
    };
    if !ok {
        use_version = available_typing_versions
            .and_then(|m| m.get("latest"))
            .cloned()
            .unwrap_or_default();
    }
    let available_version = semver::must_parse_version(&use_version);
    available_version.compare(&cached_typing.version) <= 0
}

// Go: project/ata/discovertypings.go:29 DiscoverTypings
// PORT: Go `packageNameToTypingLocation *collections.SyncMap` is the
// installer's `RefCell<IndexMap>` (read only here). A `typesRegistry` entry
// is `Option` because Go keeps a JSON `null` entry as a nil map, which the
// `registryEntry != nil` check below tells apart from `{}`.
pub fn discover_typings(
    fs: &dyn vfs::Fs,
    logger: &dyn logging::Logger,
    typings_info: &TypingsInfo,
    file_names: &[String],
    project_root_path: &str,
    package_name_to_typing_location: &RefCell<IndexMap<String, Rc<CachedTyping>>>,
    types_registry: &FxHashMap<String, Option<FxHashMap<String, String>>>,
) -> (Vec<String>, Vec<String>, Vec<String>) {
    let mut cached_typing_paths: Vec<String> = Vec::new();
    let mut new_typing_names: Vec<String> = Vec::new();
    let mut files_to_watch: Vec<String> = Vec::new();

    // A typing name to typing file path mapping
    let mut inferred_typings: IndexMap<String, String> = IndexMap::default();

    // Only infer typings for .js and .jsx files
    let file_names: Vec<String> = file_names
        .iter()
        .filter(|file_name| tspath::has_js_file_extension(file_name))
        .cloned()
        .collect();

    // PORT: Go dereferences `typingsInfo.TypeAcquisition` and
    // `typingsInfo.CompilerOptions` without a nil check (a nil pointer
    // panics).
    let type_acquisition: &TypeAcquisition = typings_info
        .type_acquisition
        .as_deref()
        .unwrap_or_else(|| crate::core::go_nil_dereference());
    // PORT: Go tests `Include != nil`. `TypeAcquisition.include` is a `Vec`
    // with no nil, so an explicit empty list skips the call. The call adds
    // nothing for an empty list; only its log line is lost.
    if !type_acquisition.include.is_empty() {
        add_inferred_typings(
            fs,
            logger,
            &mut inferred_typings,
            &type_acquisition.include,
            "Explicitly included types",
        );
    }
    let exclude = &type_acquisition.exclude;

    // Directories to search for package.json, bower.json and other typing information
    let compiler_options = typings_info
        .compiler_options
        .as_deref()
        .unwrap_or_else(|| crate::core::go_nil_dereference());
    if compiler_options.types.is_none() {
        let mut possible_search_dirs: IndexMap<String, bool> = IndexMap::default();
        for file_name in &file_names {
            possible_search_dirs.insert(tspath::get_directory_path(file_name), true);
        }
        possible_search_dirs.insert(project_root_path.to_string(), true);
        for search_dir in possible_search_dirs.keys() {
            files_to_watch = add_typing_names_and_get_files_to_watch(
                fs,
                logger,
                &mut inferred_typings,
                files_to_watch,
                search_dir,
                "bower.json",
                "bower_components",
            );
            files_to_watch = add_typing_names_and_get_files_to_watch(
                fs,
                logger,
                &mut inferred_typings,
                files_to_watch,
                search_dir,
                "package.json",
                "node_modules",
            );
        }
    }

    if !type_acquisition
        .disable_filename_based_type_acquisition
        .is_true()
    {
        get_typing_names_from_source_file_names(fs, logger, &mut inferred_typings, &file_names);
    }

    // add typings for unresolved imports
    let mut modules: Vec<String> = Vec::new();
    if let Some(unresolved_imports) = &typings_info.unresolved_imports {
        modules = Vec::with_capacity(unresolved_imports.len());
        for module in unresolved_imports.iter() {
            modules.push(core_nodemodules::non_relative_module_name_for_typing_cache(
                module,
            ));
        }
        modules.sort();
        modules.dedup();
    }
    add_inferred_typings(
        fs,
        logger,
        &mut inferred_typings,
        &modules,
        "Inferred typings from unresolved imports",
    );

    // Remove typings that the user has added to the exclude list
    for exclude_typing_name in exclude {
        // PORT: Go `delete`; `shift_remove` keeps the order of the rest.
        inferred_typings.shift_remove(exclude_typing_name);
        logger.log(&format!(
            "ATA:: Typing for {exclude_typing_name} is in exclude list, will be ignored."
        ));
    }

    // Add the cached typing locations for inferred typings that are already installed
    for (name, typing) in package_name_to_typing_location.borrow().iter() {
        let registry_entry = types_registry.get(name).and_then(Option::as_ref);
        if inferred_typings
            .get(name)
            .map_or("", String::as_str)
            .is_empty()
            && registry_entry.is_some()
            && is_typing_up_to_date(typing, registry_entry)
        {
            inferred_typings.insert(name.clone(), typing.typings_location.clone());
        }
    }

    for (typing, inferred) in &inferred_typings {
        if !inferred.is_empty() {
            cached_typing_paths.push(inferred.clone());
        } else {
            new_typing_names.push(typing.clone());
        }
    }
    // PORT: Go `%v` of a slice; log text is not compared.
    logger.log(&format!(
        "ATA:: Finished typings discovery: cachedTypingsPaths: {cached_typing_paths:?} newTypingNames: {new_typing_names:?}, filesToWatch {files_to_watch:?}"
    ));
    (cached_typing_paths, new_typing_names, files_to_watch)
}

// Go: project/ata/discovertypings.go:106 addInferredTyping
pub fn add_inferred_typing(inferred_typings: &mut IndexMap<String, String>, typing_name: &str) {
    if !inferred_typings.contains_key(typing_name) {
        inferred_typings.insert(typing_name.to_string(), String::new());
    }
}

// Go: project/ata/discovertypings.go:112 addInferredTypings
pub fn add_inferred_typings(
    fs: &dyn vfs::Fs,
    logger: &dyn logging::Logger,
    inferred_typings: &mut IndexMap<String, String>,
    typing_names: &[String],
    message: &str,
) {
    // Go does not read `fs`.
    let _ = fs;
    // PORT: Go `%v` of a slice; log text is not compared.
    logger.log(&format!("ATA:: {message}: {typing_names:?}"));
    for typing_name in typing_names {
        add_inferred_typing(inferred_typings, typing_name);
    }
}

// Go: project/ata/discovertypings.go:130 getTypingNamesFromSourceFileNames
// Infer typing names from given file names. For example, the file name "jquery-min.2.3.4.js"
// should be inferred to the 'jquery' typing name; and "angular-route.1.2.3.js" should be inferred
// to the 'angular-route' typing name.
// @param fileNames are the names for source files in the project
pub fn get_typing_names_from_source_file_names(
    fs: &dyn vfs::Fs,
    logger: &dyn logging::Logger,
    inferred_typings: &mut IndexMap<String, String>,
    file_names: &[String],
) {
    let mut has_jsx_file = false;
    let mut from_file_names: Vec<String> = Vec::new();
    for file_name in file_names {
        has_jsx_file = has_jsx_file || tspath::file_extension_is(file_name, tspath::EXTENSION_JSX);
        let lower_base_file_name =
            tspath::to_file_name_lower_case(&tspath::get_base_file_name(file_name));
        let inferred_typing_name = tspath::remove_file_extension(&lower_base_file_name);
        let cleaned_typing_name = remove_min_and_version_numbers(inferred_typing_name);
        if let Some(type_name) = SAFE_FILE_NAME_TO_TYPE_NAME.get(cleaned_typing_name.as_str()) {
            from_file_names.push((*type_name).to_string());
        }
    }
    if !from_file_names.is_empty() {
        add_inferred_typings(
            fs,
            logger,
            inferred_typings,
            &from_file_names,
            "Inferred typings from file names",
        );
    }
    if has_jsx_file {
        logger.log("ATA:: Inferred 'react' typings due to presence of '.jsx' extension");
        add_inferred_typing(inferred_typings, "react");
    }
}

// Go: project/ata/discovertypings.go:163 addTypingNamesAndGetFilesToWatch
// Adds inferred typings from manifest/module pairs (think package.json + node_modules)
//
// @param projectRootPath is the path to the directory where to look for package.json, bower.json and other typing information
// @param manifestName is the name of the manifest (package.json or bower.json)
// @param modulesDirName is the directory name for modules (node_modules or bower_components). Should be lowercase!
// @param filesToWatch are the files to watch for changes. We will push things into this array.
pub fn add_typing_names_and_get_files_to_watch(
    fs: &dyn vfs::Fs,
    logger: &dyn logging::Logger,
    inferred_typings: &mut IndexMap<String, String>,
    mut files_to_watch: Vec<String>,
    project_root_path: &str,
    manifest_name: &str,
    modules_dir_name: &str,
) -> Vec<String> {
    // First, we check the manifests themselves. They're not
    // _required_, but they allow us to do some filtering when dealing
    // with big flat dep directories.
    let manifest_path = tspath::combine_paths(project_root_path, &[manifest_name]);
    let mut manifest_typing_names: Vec<String> = Vec::new();
    let (manifest_contents, ok) = fs.read_file(&manifest_path);
    if ok {
        let mut manifest = packagejson::DependencyFields::default();
        files_to_watch.push(manifest_path.clone());
        // var manifest map[string]any
        let err = json::json_unmarshal(manifest_contents.as_bytes(), &mut manifest, &[]);
        if err.is_ok() {
            // PORT: Go `maps.Keys` order is random; the Rust maps give their
            // own order.
            manifest_typing_names.extend(manifest.dependencies.value.keys().cloned());
            manifest_typing_names.extend(manifest.dev_dependencies.value.keys().cloned());
            manifest_typing_names.extend(manifest.optional_dependencies.value.keys().cloned());
            manifest_typing_names.extend(manifest.peer_dependencies.value.keys().cloned());
            add_inferred_typings(
                fs,
                logger,
                inferred_typings,
                &manifest_typing_names,
                &format!("Typing names in '{manifest_path}' dependencies"),
            );
        }
    }

    // Now we scan the directories for typing information in
    // already-installed dependencies (if present). Note that this
    // step happens regardless of whether a manifest was present,
    // which is certainly a valid configuration, if an unusual one.
    let packages_folder_path = tspath::combine_paths(project_root_path, &[modules_dir_name]);
    files_to_watch.push(packages_folder_path.clone());
    if !fs.directory_exists(&packages_folder_path) {
        return files_to_watch;
    }

    // There's two cases we have to take into account here:
    // 1. If manifest is undefined, then we're not using a manifest.
    //    That means that we should scan _all_ dependencies at the top
    //    level of the modulesDir.
    // 2. If manifest is defined, then we can do some special
    //    filtering to reduce the amount of scanning we need to do.
    //
    // Previous versions of this algorithm checked for a `_requiredBy`
    // field in the package.json, but that field is only present in
    // `npm@>=3 <7`.

    // Package names that do **not** provide their own typings, so
    // we'll look them up.
    let mut package_names: Vec<String> = Vec::new();

    let mut dependency_manifest_names: Vec<String> = Vec::new();
    if !manifest_typing_names.is_empty() {
        // This is #1 described above.
        for typing_name in &manifest_typing_names {
            dependency_manifest_names.push(tspath::combine_paths(
                &packages_folder_path,
                &[typing_name.as_str(), manifest_name],
            ));
        }
    } else {
        // And #2. Depth = 3 because scoped packages look like `node_modules/@foo/bar/package.json`
        let depth = 3;
        for manifest_path in vfs::vfsmatch::read_directory(
            fs,
            project_root_path,
            &packages_folder_path,
            &[tspath::EXTENSION_JSON.to_string()],
            &[],
            &[],
            depth,
        ) {
            if tspath::get_base_file_name(&manifest_path) != manifest_name {
                continue;
            }

            // It's ok to treat
            // `node_modules/@foo/bar/package.json` as a manifest,
            // but not `node_modules/jquery/nested/package.json`.
            // We only assume depth 3 is ok for formally scoped
            // packages. So that needs this dance here.

            let path_components = tspath::get_path_components(&manifest_path, "");
            let len_path_components = path_components.len();
            let (ch, _) = utf8_decode_rune_in_string(&path_components[len_path_components - 3], 0);
            let is_scoped = ch == '@' as i32;

            if is_scoped
                && tspath::to_file_name_lower_case(&path_components[len_path_components - 4])
                    == modules_dir_name // `node_modules/@foo/bar`
                || !is_scoped
                    && tspath::to_file_name_lower_case(&path_components[len_path_components - 3])
                        == modules_dir_name
            // `node_modules/foo`
            {
                dependency_manifest_names.push(manifest_path);
            }
        }
    }

    // PORT: Go `%v` of a slice; log text is not compared.
    logger.log(&format!(
        "ATA:: Searching for typing names in {packages_folder_path}; all files: {dependency_manifest_names:?}"
    ));

    // Once we have the names of things to look up, we iterate over
    // and either collect their included typings, or add them to the
    // list of typings we need to look up separately.
    for manifest_path in &dependency_manifest_names {
        let (manifest_contents, ok) = fs.read_file(manifest_path);
        if !ok {
            continue;
        }
        // PORT: Go `manifest, err := packagejson.Parse(..)`; on error the
        // loop continues before `manifest` is read.
        let Ok(manifest) = packagejson::parse(manifest_contents.as_bytes()) else {
            continue;
        };
        // If the package has its own d.ts typings, those will take precedence. Otherwise the package name will be used
        // to download d.ts files from DefinitelyTyped
        if manifest.header_fields.name.value.is_empty() {
            continue;
        }
        let mut own_types = manifest.path_fields.types.value.clone();
        if own_types.is_empty() {
            own_types = manifest.path_fields.typings.value.clone();
        }
        if !own_types.is_empty() {
            let absolute_path = tspath::get_normalized_absolute_path(
                &own_types,
                &tspath::get_directory_path(manifest_path),
            );
            if fs.file_exists(&absolute_path) {
                logger.log(&format!(
                    "ATA::     Package '{}' provides its own types.",
                    manifest.header_fields.name.value
                ));
                inferred_typings.insert(manifest.header_fields.name.value.clone(), absolute_path);
            } else {
                logger.log(&format!(
                    "ATA::     Package '{}' provides its own types but they are missing.",
                    manifest.header_fields.name.value
                ));
            }
        } else {
            package_names.push(manifest.header_fields.name.value.clone());
        }
    }
    add_inferred_typings(
        fs,
        logger,
        inferred_typings,
        &package_names,
        "    Found package names",
    );
    files_to_watch
}

// Go: project/ata/discovertypings.go:291 removeMinAndVersionNumbers
// Takes a string like "jquery-min.4.2.3" and returns "jquery"
//
// @internal
pub fn remove_min_and_version_numbers(file_name: &str) -> String {
    // We used to use the regex /[.-]((min)|(\d+(\.\d+)*))$/ and would just .replace it twice.
    // Unfortunately, that regex has O(n^2) performance because v8 doesn't match from the end of the string.
    // Instead, we now essentially scan the filename (backwards) ourselves.
    // PORT: `utf8_decode_last_rune_in_string(file_name, pos)` is Go
    // `utf8.DecodeLastRuneInString(fileName[:pos])`.
    let mut end = file_name.len();
    let mut pos = end;
    while pos > 0 {
        let (mut ch, mut size) = utf8_decode_last_rune_in_string(file_name, pos);
        if ch >= '0' as i32 && ch <= '9' as i32 {
            // Match a \d+ segment
            loop {
                pos -= size as usize;
                (ch, size) = utf8_decode_last_rune_in_string(file_name, pos);
                if pos == 0 || ch < '0' as i32 || ch > '9' as i32 {
                    break;
                }
            }
        } else if pos > 4 && (ch == 'n' as i32 || ch == 'N' as i32) {
            // Looking for "min" or "min"
            // Already matched the 'n'
            pos -= size as usize;
            (ch, size) = utf8_decode_last_rune_in_string(file_name, pos);
            if ch != 'i' as i32 && ch != 'I' as i32 {
                break;
            }
            pos -= size as usize;
            (ch, size) = utf8_decode_last_rune_in_string(file_name, pos);
            if ch != 'm' as i32 && ch != 'M' as i32 {
                break;
            }
            pos -= size as usize;
            (ch, size) = utf8_decode_last_rune_in_string(file_name, pos);
        } else {
            // This character is not part of either suffix pattern
            break;
        }

        if ch != '-' as i32 && ch != '.' as i32 {
            break;
        }
        pos -= size as usize;
        end = pos;
    }
    file_name[0..end].to_string()
}
