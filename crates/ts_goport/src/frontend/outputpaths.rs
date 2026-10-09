//! Port of Go `internal/outputpaths` (outputpaths.go, commonsourcedirectory.go).

use crate::frontend::prelude::*;

// Go: outputpaths/outputpaths.go:9 OutputPathsHost
pub trait OutputPathsHost {
    fn common_source_directory(&self) -> String;
    /// #4712: the file extensions of the configured content mappers.
    fn content_mapper_extensions(&self) -> Vec<String>;
    fn get_current_directory(&self) -> String;
    fn use_case_sensitive_file_names(&self) -> bool;
}

// Go: outputpaths/outputpaths.go:15 OutputPaths
#[derive(Clone, Debug, Default)]
pub struct OutputPaths {
    js_file_path: String,
    source_map_file_path: String,
    declaration_file_path: String,
    declaration_map_path: String,
}

impl OutputPaths {
    // Go: outputpaths/outputpaths.go:23 (*OutputPaths).DeclarationFilePath
    pub fn declaration_file_path(&self) -> &str {
        &self.declaration_file_path
    }

    // Go: outputpaths/outputpaths.go:28 (*OutputPaths).JsFilePath
    pub fn js_file_path(&self) -> &str {
        &self.js_file_path
    }

    // Go: outputpaths/outputpaths.go:32 (*OutputPaths).SourceMapFilePath
    pub fn source_map_file_path(&self) -> &str {
        &self.source_map_file_path
    }

    // Go: outputpaths/outputpaths.go:36 (*OutputPaths).DeclarationMapPath
    pub fn declaration_map_path(&self) -> &str {
        &self.declaration_map_path
    }
}

// Go: `*outputpaths.OutputPaths` implements `declarations.OutputPaths`
// (transformers/declarations/transform.go:28).
impl crate::declarations::OutputPaths for OutputPaths {
    fn declaration_file_path(&self) -> String {
        self.declaration_file_path.clone()
    }

    fn js_file_path(&self) -> String {
        self.js_file_path.clone()
    }
}

// Go: outputpaths/outputpaths.go:40 ForceEmitPaths
/// Output paths to compute even when the options turn that output off (#4699).
/// The API emit and the builder signature emit set them.
#[derive(Clone, Copy, Debug, Default)]
pub struct ForceEmitPaths {
    pub dts: bool,
    pub js: bool,
    pub declaration_map: bool,
}

// Go: outputpaths/outputpaths.go:46 GetOutputPathsFor
pub fn get_output_paths_for(
    source_file: &ParsedSourceFile,
    options: &CompilerOptions,
    host: &dyn OutputPathsHost,
    force: ForceEmitPaths,
) -> OutputPaths {
    get_output_paths_for_file(
        source_file.file_name(),
        source_file.script_kind,
        &source_file.content_mapper(),
        options,
        host,
        force,
    )
}

/// Go `GetOutputPathsFor` with the three source file fields it reads.
/// Checker threads have no `ParsedSourceFile`, only the file name, the
/// script kind and the content mapper (Go `sourceFile.ContentMapper()`,
/// "" when no content mapper made the file).
pub fn get_output_paths_for_file(
    file_name: &str,
    script_kind: ScriptKind,
    content_mapper: &str,
    options: &CompilerOptions,
    host: &dyn OutputPathsHost,
    force: ForceEmitPaths,
) -> OutputPaths {
    let own_output_file_path = get_own_emit_output_file_path(
        file_name,
        options,
        host,
        get_output_extension(file_name, options.jsx),
    );
    let is_json_file = script_kind == ScriptKind::JSON;
    // If json file emits to the same location skip writing it, if emitDeclarationOnly skip writing it
    let is_json_emitted_to_same_location = is_json_file
        && compare_paths(
            file_name,
            &own_output_file_path,
            &ComparePathsOptions {
                current_directory: host.get_current_directory(),
                use_case_sensitive_file_names: host.use_case_sensitive_file_names(),
            },
        ) == 0;
    let mut paths = OutputPaths::default();
    // #4699: `force.js`, `force.dts` and `force.declaration_map` (Go `ForceEmitPaths`).
    // #4712: a content-mapped file gets no JS output. ts#63936: it keeps its
    // declaration map.
    if content_mapper.is_empty()
        && (force.js || options.emit_declaration_only != Tristate::True)
        && !is_json_emitted_to_same_location
    {
        paths.js_file_path = own_output_file_path;
        if script_kind != ScriptKind::JSON {
            paths.source_map_file_path = get_source_map_file_path(&paths.js_file_path, options);
        }
    }
    if force.dts || options.get_emit_declarations() && !is_json_file {
        paths.declaration_file_path =
            get_declaration_emit_output_file_path(file_name, options, host);
        if options.get_are_declaration_maps_enabled()
            || force.declaration_map && options.declaration_map.is_true()
        {
            paths.declaration_map_path = format!("{}.map", paths.declaration_file_path);
        }
    }
    paths
}

// Go: outputpaths/outputpaths.go:69 ForEachEmittedFile
pub fn for_each_emitted_file(
    host: &dyn OutputPathsHost,
    options: &CompilerOptions,
    mut action: impl FnMut(&OutputPaths, Option<&Rc<ParsedSourceFile>>) -> bool,
    source_files: &[Rc<ParsedSourceFile>],
    force_dts_emit: bool,
) -> bool {
    for source_file in source_files {
        // #4699: Go `ForceEmitPaths{Dts: forceDtsEmit}`.
        if action(
            &get_output_paths_for(
                source_file,
                options,
                host,
                ForceEmitPaths {
                    dts: force_dts_emit,
                    ..ForceEmitPaths::default()
                },
            ),
            Some(source_file),
        ) {
            return true;
        }
    }
    false
}

// Go: outputpaths/outputpaths.go:78 GetOutputJSFileName
pub fn get_output_js_file_name(
    input_file_name: &str,
    options: &CompilerOptions,
    host: &dyn OutputPathsHost,
) -> String {
    // #4712: a content-mapped file has no JS output.
    if options.emit_declaration_only.is_true() || is_content_mapped_file_name(input_file_name, host)
    {
        return String::new();
    }
    let output_file_name = get_output_js_file_name_worker(input_file_name, options, host);
    if !file_extension_is(&output_file_name, EXTENSION_JSON)
        || compare_paths(
            input_file_name,
            &output_file_name,
            &ComparePathsOptions {
                current_directory: host.get_current_directory(),
                use_case_sensitive_file_names: host.use_case_sensitive_file_names(),
            },
        ) != 0
    {
        return output_file_name;
    }
    String::new()
}

// Go: outputpaths/outputpaths.go:91 isContentMappedFileName (#4712)
fn is_content_mapped_file_name(file_name: &str, host: &dyn OutputPathsHost) -> bool {
    !get_longest_extension_from_path(
        file_name,
        &host.content_mapper_extensions(),
        !host.use_case_sensitive_file_names(),
    )
    .is_empty()
}

// Go: outputpaths/outputpaths.go:95 GetOutputJSFileNameWorker
pub fn get_output_js_file_name_worker(
    input_file_name: &str,
    options: &CompilerOptions,
    host: &dyn OutputPathsHost,
) -> String {
    change_extension(
        &get_output_path_without_changing_extension(input_file_name, &options.out_dir, host),
        get_output_extension(input_file_name, options.jsx),
    )
}

// Go: outputpaths/outputpaths.go:100 GetOutputDeclarationFileNameWorker
pub fn get_output_declaration_file_name_worker(
    input_file_name: &str,
    options: &CompilerOptions,
    host: &dyn OutputPathsHost,
) -> String {
    let mut dir = options.declaration_dir.as_str();
    if dir.is_empty() {
        dir = options.out_dir.as_str();
    }
    // #4712: Go `ChangeToDeclarationExtension`, not `ChangeExtension`.
    change_to_declaration_extension(
        &get_output_path_without_changing_extension(input_file_name, dir, host),
        host,
    )
}

// Go: outputpaths/outputpaths.go:109 GetOutputExtension
pub fn get_output_extension(file_name: &str, jsx: JsxEmit) -> &'static str {
    if file_extension_is(file_name, EXTENSION_JSON) {
        EXTENSION_JSON
    } else if jsx == JsxEmit::PRESERVE
        && file_extension_is_one_of(file_name, &[EXTENSION_JSX, EXTENSION_TSX])
    {
        EXTENSION_JSX
    } else if file_extension_is_one_of(file_name, &[EXTENSION_MTS, EXTENSION_MJS]) {
        EXTENSION_MJS
    } else if file_extension_is_one_of(file_name, &[EXTENSION_CTS, EXTENSION_CJS]) {
        EXTENSION_CJS
    } else {
        EXTENSION_JS
    }
}

// Go: outputpaths/outputpaths.go:131 GetDeclarationEmitOutputFilePath (at 673a5f17d713;
// ts#64159 renames it getDeclarationEmitOutputFilePathForFileName, outputpaths/outputpaths.go:128)
pub fn get_declaration_emit_output_file_path(
    file: &str,
    options: &CompilerOptions,
    host: &dyn OutputPathsHost,
) -> String {
    let output_dir: Option<&str> = if !options.declaration_dir.is_empty() {
        Some(&options.declaration_dir)
    } else if !options.out_dir.is_empty() {
        Some(&options.out_dir)
    } else {
        None
    };

    let path = if let Some(output_dir) = output_dir {
        get_source_file_path_in_new_dir_worker(
            file,
            output_dir,
            &host.get_current_directory(),
            &host.common_source_directory(),
            host.use_case_sensitive_file_names(),
        )
    } else {
        file.to_string()
    };
    // #4712
    change_to_declaration_extension(&path, host)
}

// Go: outputpaths/outputpaths.go:150 ChangeToDeclarationExtension (#4712)
/// A content mapper extension `.ext` becomes `.d.ext.ts`. Other paths get
/// their declaration extension (Go `GetDeclarationEmitExtensionForPath`).
pub fn change_to_declaration_extension(path: &str, host: &dyn OutputPathsHost) -> String {
    let extension = get_longest_extension_from_path(path, &host.content_mapper_extensions(), false);
    if !extension.is_empty() {
        return format!("{}.d{}.ts", remove_extension(path, &extension), extension);
    }
    let mut path_without_extension = remove_file_extension(path);
    if path_without_extension == path {
        let extension = get_any_extension_from_path(path, &[], false);
        if !extension.is_empty() {
            path_without_extension = remove_extension(path, &extension);
        }
    }
    format!(
        "{}{}",
        path_without_extension,
        get_declaration_emit_extension_for_path(path)
    )
}

// Go: outputpaths/outputpaths.go:161 GetSourceFilePathInNewDir (at 673a5f17d713;
// ts#64159 renames it GetSourceFileNameInNewDir, outputpaths/outputpaths.go:178)
// tsgo#4900: the same as the worker.
pub fn get_source_file_path_in_new_dir(
    file_name: &str,
    new_dir_path: &str,
    current_directory: &str,
    common_source_directory: &str,
    use_case_sensitive_file_names: bool,
) -> String {
    get_source_file_path_in_new_dir_worker(
        file_name,
        new_dir_path,
        current_directory,
        common_source_directory,
        use_case_sensitive_file_names,
    )
}

// Go: outputpaths/outputpaths.go:165 getOutputPathWithoutChangingExtension (at 673a5f17d713;
// ts#64159 renames it getOutputFileNameWithoutChangingExtension, outputpaths/outputpaths.go:163)
fn get_output_path_without_changing_extension(
    input_file_name: &str,
    output_directory: &str,
    host: &dyn OutputPathsHost,
) -> String {
    if !output_directory.is_empty() {
        return resolve_path(
            output_directory,
            &[&get_relative_path_from_directory(
                &host.common_source_directory(),
                input_file_name,
                &ComparePathsOptions {
                    use_case_sensitive_file_names: host.use_case_sensitive_file_names(),
                    current_directory: host.get_current_directory(),
                },
            )],
        );
    }
    input_file_name.to_string()
}

// Go: outputpaths/outputpaths.go:175 GetSourceFilePathInNewDirWorker (at 673a5f17d713;
// ts#64159 merges it into GetSourceFileNameInNewDir, outputpaths/outputpaths.go:178)
// tsgo#4900: `TrimFilePathPrefix` cuts the common source directory by runes,
// not by its byte length.
pub fn get_source_file_path_in_new_dir_worker(
    file_name: &str,
    new_dir_path: &str,
    current_directory: &str,
    common_source_directory: &str,
    use_case_sensitive_file_names: bool,
) -> String {
    let source_file_path = get_normalized_absolute_path(file_name, current_directory);
    // ts#64159 (outputpaths.go:179): a file name equal to the common source
    // directory stays as it is.
    if source_file_path == common_source_directory {
        return source_file_path;
    }
    match trim_file_path_prefix(
        &source_file_path,
        common_source_directory,
        use_case_sensitive_file_names,
    ) {
        Some(trimmed) => combine_paths(new_dir_path, &[&trimmed]),
        None => combine_paths(new_dir_path, &[&source_file_path]),
    }
}

// Go: outputpaths/outputpaths.go:183 getOwnEmitOutputFilePath (at 673a5f17d713;
// ts#64159 renames it getOwnEmitOutputFilePathForFileName, outputpaths/outputpaths.go:188)
fn get_own_emit_output_file_path(
    file_name: &str,
    options: &CompilerOptions,
    host: &dyn OutputPathsHost,
    extension: &str,
) -> String {
    let emit_output_file_path_without_extension = if !options.out_dir.is_empty() {
        let current_directory = host.get_current_directory();
        remove_file_extension(&get_source_file_path_in_new_dir(
            file_name,
            &options.out_dir,
            &current_directory,
            &host.common_source_directory(),
            host.use_case_sensitive_file_names(),
        ))
        .to_string()
    } else {
        remove_file_extension(file_name).to_string()
    };
    emit_output_file_path_without_extension + extension
}

// Go: outputpaths/outputpaths.go:203 GetSourceMapFilePath
pub fn get_source_map_file_path(js_file_path: &str, options: &CompilerOptions) -> String {
    if options.source_map.is_true() && !options.inline_source_map.is_true() {
        return format!("{js_file_path}.map");
    }
    String::new()
}

// Go: outputpaths/outputpaths.go:210 GetBuildInfoFileName
// ts#64159: the output name comes from the config file name, and the
// extension goes last, so an empty or dot stem ("c:/src/p/.json") keeps its
// directory. A config file on another root than rootDir keeps its own name
// (R4).
pub fn get_build_info_file_name(options: &CompilerOptions, opts: &ComparePathsOptions) -> String {
    if !options.is_incremental() && !options.build.is_true() {
        return String::new();
    }
    if !options.ts_build_info_file.is_empty() {
        return options.ts_build_info_file.clone();
    }
    if options.config_file_path.is_empty() {
        return String::new();
    }
    let config_file_name = options.config_file_path.as_str();
    let build_info_file_name = if !options.out_dir.is_empty() {
        if !options.root_dir.is_empty() {
            match relative_path_from_directory(
                &options.root_dir,
                config_file_name,
                opts.use_case_sensitive_file_names,
            ) {
                // Go: options.OutDir.ResolveRelativeFile(relativePath)
                Some(relative_path) => resolve_path(&options.out_dir, &[&relative_path]),
                None => config_file_name.to_string(),
            }
        } else {
            // Go: options.OutDir.ResolveFile(configFileName.BaseName())
            combine_paths(&options.out_dir, &[&get_base_file_name(config_file_name)])
        }
    } else {
        config_file_name.to_string()
    };
    remove_file_extension(&build_info_file_name).to_string() + EXTENSION_TS_BUILD_INFO
}

// Go: outputpaths/commonsourcedirectory.go:8 computeCommonSourceDirectoryOfFilenames
fn compute_common_source_directory_of_filenames(
    file_names: &[String],
    current_directory: &str,
    use_case_sensitive_file_names: bool,
) -> String {
    // PORT: Go nil slice is `None`.
    let mut common_path_components: Option<Vec<String>> = None;
    for source_file in file_names {
        // Each file contributes into common source file path
        let mut source_path_components =
            get_normalized_path_components(source_file, current_directory);

        // The base file name is not part of the common directory path
        source_path_components.pop();

        let Some(common) = common_path_components.as_mut() else {
            // first file
            common_path_components = Some(source_path_components);
            continue;
        };

        let n = common.len().min(source_path_components.len());
        for i in 0..n {
            if get_canonical_file_name(&common[i], use_case_sensitive_file_names)
                != get_canonical_file_name(
                    &source_path_components[i],
                    use_case_sensitive_file_names,
                )
            {
                if i == 0 {
                    // Failed to find any common path component
                    return String::new();
                }

                // New common path found that is 0 -> i-1
                common.truncate(i);
                break;
            }
        }

        // If the sourcePathComponents was shorter than the commonPathComponents, truncate to the sourcePathComponents
        if source_path_components.len() < common.len() {
            common.truncate(source_path_components.len());
        }
    }

    let common_path_components = common_path_components.unwrap_or_default();
    if common_path_components.is_empty() {
        // Can happen when all input files are .d.ts files
        return current_directory.to_string();
    }

    get_path_from_path_components(&common_path_components)
}

// Go: outputpaths/commonsourcedirectory.go:16 GetComputedCommonSourceDirectory
pub fn get_computed_common_source_directory(
    emitted_files: &[String],
    current_directory: &str,
    use_case_sensitive_file_names: bool,
) -> String {
    let mut common_source_directory = compute_common_source_directory_of_filenames(
        emitted_files,
        current_directory,
        use_case_sensitive_file_names,
    );
    if !common_source_directory.is_empty() {
        common_source_directory = ensure_trailing_directory_separator(&common_source_directory);
    }
    common_source_directory
}

// Go: outputpaths/commonsourcedirectory.go:20 GetCommonSourceDirectory
// PORT: Go `checkSourceFilesBelongToPath` is a nillable callback; its
// result is unused.
pub fn get_common_source_directory(
    options: &CompilerOptions,
    files: impl FnOnce() -> Vec<String>,
    current_directory: &str,
    use_case_sensitive_file_names: bool,
    check_source_files_belong_to_path: Option<&mut dyn FnMut(&[String], &str) -> bool>,
) -> String {
    let mut common_source_directory;
    if !options.root_dir.is_empty() {
        // If a rootDir is specified use it as the commonSourceDirectory
        common_source_directory = options.root_dir.clone();
        if let Some(check) = check_source_files_belong_to_path {
            check(&files(), &options.root_dir);
        }
    } else if !options.config_file_path.is_empty() {
        // If the rootDir is not specified, then the common source directory is the directory of the config file.
        common_source_directory = get_directory_path(&options.config_file_path);
        if let Some(check) = check_source_files_belong_to_path {
            check(&files(), &common_source_directory);
        }
    } else {
        common_source_directory = compute_common_source_directory_of_filenames(
            &files(),
            current_directory,
            use_case_sensitive_file_names,
        );
    }

    if !common_source_directory.is_empty() {
        // Make sure directory path ends with directory separator so this string can directly
        // used to replace with "" to get the relative path of the source file and the relative path doesn't
        // start with / making it rooted path
        common_source_directory = ensure_trailing_directory_separator(&common_source_directory);
    }

    common_source_directory
}
