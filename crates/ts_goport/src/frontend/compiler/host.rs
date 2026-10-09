//! Go `internal/compiler/host.go`: the compiler host that reads and parses
//! source files and resolves project references.

use crate::contentmapper::{self, Mapper, Project, SourceFiles};
use crate::frontend::prelude::*;
use crate::gostd::GoError;

/// Go `compiler.CompilerHost`.
// PORT: Go `FS()` returns the `vfs.FS` interface. This returns a shared
// `Rc<dyn Fs>`, so callers can keep it.
// PORT: Go `*ast.SourceFile` is `Rc<ParsedSourceFile>`. Go `nil` is `None`.
pub trait CompilerHost {
    fn fs(&self) -> Rc<dyn Fs>;
    fn default_library_path(&self) -> String;
    fn get_current_directory(&self) -> String;
    fn trace(&self, msg: &'static Message, args: Vec<String>);
    fn get_source_file(&self, opts: &SourceFileParseOptions) -> Option<Rc<ParsedSourceFile>>;
    // GetContentMappedSourceFile produces the source file for a content-mapped (foreign) file by running
    // the given mapper's transform on the file's content. The caller resolves the mapper (and owns the
    // failure accounting), so implementations must use it as-is. It returns nil if the file cannot be read,
    // or an error if the transform fails or the mapper produces invalid position mappings. Implementations
    // may cache successful results.
    // Go: host.go:25 CompilerHost.GetContentMappedSourceFiles (tsgo#4712)
    // PORT: Go returns `(contentmapper.SourceFiles, error)`; a file that
    // cannot be read is `Ok` with no canonical file.
    fn get_content_mapped_source_files(
        &self,
        parse_options: &SourceFileParseOptions,
        mapper: &Rc<Mapper>,
    ) -> Result<SourceFiles, GoError>;
    // ContentMapperProject returns the project-scoped content mapper used by this host, or nil when the
    // command line has no content mappers. The project owns transform identity and lifecycle state.
    // Go: host.go:28 CompilerHost.ContentMapperProject (tsgo#4712)
    fn content_mapper_project(&self) -> Option<Rc<dyn Project>>;
    fn get_resolved_project_reference(
        &self,
        file_name: &str,
        path: &Path,
    ) -> Option<Rc<ParsedCommandLine>>;

    /// Runs `f` with no tracking of the file system calls of `fs()`. Only
    /// the language server's project host tracks them (Go `sourceFS`
    /// `tracking`); Go stops that when it freezes the host
    /// (project/compilerhost.go:57), so a lazy program value that Go
    /// computes after the freeze makes no tracked calls.
    // PORT: not in Go. Go computes such values on first use.
    fn without_fs_tracking(&self, f: &mut dyn FnMut()) {
        f();
    }

    /// True when `fs()` shows the plain OS file system (Go `sys.FS()`,
    /// maybe behind `cachedvfs`), so a parse worker thread reads the same
    /// files, directories and bytes as this host. Then the loader can use
    /// what the workers read and resolved.
    // PORT: not in Go. Go reads and resolves on the host in every parse
    // task; here the parse workers use the OS file system of their thread.
    fn is_plain_os_fs(&self) -> bool {
        false
    }

    /// The cache where `fs()` keeps its `FileExists`, `DirectoryExists`,
    /// `Realpath` and `GetAccessibleEntries` lookups, when it keeps them
    /// where the parse workers can use them too (`BuildStatCache`). Only
    /// the `tsc -b` host does: its cache lasts for all programs of a build.
    // PORT: not in Go. Go parse tasks share the host's cachedvfs.
    fn stat_cache(&self) -> Option<std::sync::Arc<BuildStatCache>> {
        None
    }

    /// The host's part in resolve ahead (resolve_ahead.rs): the module
    /// resolution keys of its previous program load, the workers' view of
    /// `fs()`, and the check of a worker answer on `fs()`. Only the
    /// language server's project host has it, on the OS file system with
    /// open files over it.
    // PORT: not in Go (perf). Go resolves in every parse task on the host's
    // file system.
    fn resolve_ahead(&self) -> Option<super::resolve_ahead::ResolveAheadHost> {
        None
    }

    /// True when a program load with this host starts parse workers that
    /// parse the queued files ahead of the loader (`FilesParser::parse`).
    /// A host that gives most files from its own cache returns false: the
    /// loader does not use a worker parse of a cached file, and the nodes
    /// of that parse stay in the worker's AST arena. With false, the loader
    /// parses each file itself, so the output is the same.
    // PORT: not in Go. Go parses only when the host asks (`GetSourceFile`).
    fn prefetch_parses(&self) -> bool {
        true
    }

    /// True when the parse workers of a program load with this host send
    /// the transform requests of the content-mapped files ahead of the
    /// loader, once the loader's first transform opened the mapper project
    /// (`FileLoader::concurrent_content_mapper_transform`). Then
    /// `get_content_mapped_source_files` takes what they made
    /// (`content_mapped_source_files`). The language server's project host
    /// returns false: its parse cache makes each transform, of the editor
    /// text.
    // PORT: not in Go, where the parse goroutines send the transforms.
    fn prefetch_content_mapped(&self) -> bool {
        false
    }

    /// The files that `get_source_file` gives now from the host's own
    /// cache, with no read and no parse, by name, with their references.
    /// The parse workers of a program load do not parse these files; they
    /// resolve and queue their references (`FilesParser::parse`). When the
    /// map has every root file, the load starts no worker. The `tsc -b`
    /// host shares the parsed `.d.ts` and `.json` files between the
    /// programs of a build; the language server's project host has the
    /// session's parse cache.
    // PORT: not in Go (see `prefetch_parses`). A name, not the full parse
    // options: the workers only guess the options. A cached parse with
    // other options is a cache miss, and the loader then parses the file
    // itself, with the same result.
    fn cached_source_file_refs(&self) -> FxHashMap<String, std::sync::Arc<FileRefs>> {
        FxHashMap::default()
    }

    /// True when a parse worker's parse of a path that this thread
    /// published before is a freeable parse (`ast::freeable_path`), as the
    /// loader's own parse of it is: its store owns its nodes, so the file
    /// version frees them, and a parse that the loader does not take is
    /// freed after the load. False: such a worker parse is static, and a
    /// file version of it leaks its nodes. The watch hosts return true.
    // PORT: not in Go (see `prefetch_parses`).
    fn freeable_worker_parses(&self) -> bool {
        false
    }

    /// Drops the data that the host keeps for its programs (for example a
    /// snapshot file system). `ls_program` calls it when the last live
    /// program that uses this host is released. The host must not read
    /// files after this.
    // PORT: not in Go. Go's GC frees the host with its last program. The
    // port keeps each program shell (multiprog M2), and the shell keeps its
    // host, so the host lets go of its data instead.
    fn release(&self) {}
}

/// The body of Go `GetContentMappedSourceFiles` of the compiler host
/// (host.go:100) and of the `tsc -b` project host (build/compilerHost.go:41),
/// with the transform that a parse worker sent for the file
/// (`take_prefetched_mapped`) in place of the host's own request.
// PORT: the worker part is not in Go (see
// `CompilerHost::prefetch_content_mapped`).
pub(crate) fn content_mapped_source_files(
    fs: &dyn Fs,
    project: &dyn Project,
    parse_options: &SourceFileParseOptions,
    mapper: &Rc<Mapper>,
) -> Result<SourceFiles, GoError> {
    let (content, ok) = fs.read_file(&parse_options.file_name);
    if !ok {
        return Ok(SourceFiles::default());
    }
    let prefetched = take_prefetched_mapped(parse_options, &content);
    let files = contentmapper::transform_and_parse_prefetched(
        parse_options,
        &content,
        mapper,
        project,
        prefetched,
    )?;
    contentmapper::check_supplemental_file_name_collisions(&files, &|name: &str| {
        fs.file_exists(name)
    })?;
    Ok(files)
}

/// Go trace callback `func(msg *diagnostics.Message, args ...any)`.
pub type TraceFn = Rc<dyn Fn(&'static Message, Vec<String>)>;

/// Go `compilerHost`.
// PORT: Go unexported type. Other packages only see it through the
// `CompilerHost` interface, so the Rust name is `CompilerHostImpl`.
pub struct CompilerHostImpl {
    current_directory: String,
    fs: Rc<dyn Fs>,
    default_library_path: String,
    // PORT: Go nil interface is `None`.
    extended_config_cache: Option<Rc<dyn ExtendedConfigCache>>,
    trace: TraceFn,
    // tsgo#4712. PORT: Go nil interface is `None`.
    content_mapper_project: Option<Rc<dyn Project>>,
    /// `fs` is `bundled::is_wrapped_os_fs` (maybe behind the cache).
    plain_os_fs: bool,
}

// Go: host.go:42 NewCachedFSCompilerHost
pub fn new_cached_fs_compiler_host(
    current_directory: &str,
    fs: Rc<dyn Fs>,
    default_library_path: &str,
    extended_config_cache: Option<Rc<dyn ExtendedConfigCache>>,
    trace: Option<TraceFn>,
    content_mapper_project: Option<Rc<dyn Project>>,
) -> Rc<dyn CompilerHost> {
    let plain_os_fs = is_wrapped_os_fs(&fs);
    new_compiler_host_with(
        current_directory,
        cachedvfs_from(fs),
        default_library_path,
        extended_config_cache,
        trace,
        content_mapper_project,
        plain_os_fs,
    )
}

/// Go `NewCompilerHost` on `fs`, a file system that caches the lookups of
/// `base` (Go `cachedvfs.From(base)`, as the `tsc -b` host makes it). The
/// host shows the plain OS file system (`CompilerHost::is_plain_os_fs`)
/// when `base` is the wrapped OS file system.
// PORT: not in Go. `new_compiler_host` cannot see through the cache.
pub fn new_compiler_host_over(
    current_directory: &str,
    fs: Rc<dyn Fs>,
    base: &Rc<dyn Fs>,
    default_library_path: &str,
) -> Rc<dyn CompilerHost> {
    new_compiler_host_with(
        current_directory,
        fs,
        default_library_path,
        None,
        None,
        None,
        is_wrapped_os_fs(base),
    )
}

// Go: host.go:52 NewCompilerHost
pub fn new_compiler_host(
    current_directory: &str,
    fs: Rc<dyn Fs>,
    default_library_path: &str,
    extended_config_cache: Option<Rc<dyn ExtendedConfigCache>>,
    trace: Option<TraceFn>,
    content_mapper_project: Option<Rc<dyn Project>>,
) -> Rc<dyn CompilerHost> {
    let plain_os_fs = is_wrapped_os_fs(&fs);
    new_compiler_host_with(
        current_directory,
        fs,
        default_library_path,
        extended_config_cache,
        trace,
        content_mapper_project,
        plain_os_fs,
    )
}

/// Go `NewCompilerHost` body. `plain_os_fs`: `fs` shows the plain OS file
/// system (`CompilerHost::is_plain_os_fs`).
fn new_compiler_host_with(
    current_directory: &str,
    fs: Rc<dyn Fs>,
    default_library_path: &str,
    extended_config_cache: Option<Rc<dyn ExtendedConfigCache>>,
    trace: Option<TraceFn>,
    content_mapper_project: Option<Rc<dyn Project>>,
    plain_os_fs: bool,
) -> Rc<dyn CompilerHost> {
    // PORT: Go nil func is `None`.
    let trace = trace.unwrap_or_else(|| Rc::new(|_msg: &'static Message, _args: Vec<String>| {}));
    Rc::new(CompilerHostImpl {
        current_directory: current_directory.to_string(),
        fs,
        default_library_path: default_library_path.to_string(),
        extended_config_cache,
        trace,
        content_mapper_project,
        plain_os_fs,
    })
}

impl CompilerHost for CompilerHostImpl {
    // Go: host.go:71 (*compilerHost).FS
    fn fs(&self) -> Rc<dyn Fs> {
        self.fs.clone()
    }

    // Go: host.go:75 (*compilerHost).DefaultLibraryPath
    fn default_library_path(&self) -> String {
        self.default_library_path.clone()
    }

    // Go: host.go:84 (*compilerHost).GetCurrentDirectory (at 673a5f17d713; removed by ts#64159; use the program base directory, compiler/program.go:150)
    fn get_current_directory(&self) -> String {
        self.current_directory.clone()
    }

    // Go: host.go:79 (*compilerHost).Trace
    fn trace(&self, msg: &'static Message, args: Vec<String>) {
        (self.trace)(msg, args);
    }

    // Go: host.go:83 (*compilerHost).GetSourceFile
    // PORT: a parse worker may have parsed the file already (`FilesParser`
    // prefetch, `take_prefetched`). A file text is leaked for the program
    // lifetime, except the text of a freeable file version (`FileText::new`),
    // which goes with the version.
    fn get_source_file(&self, opts: &SourceFileParseOptions) -> Option<Rc<ParsedSourceFile>> {
        let script_kind = ensure_script_kind_from_file_name(&opts.file_name);
        let freeable = crate::ast::freeable_path(&opts.path.0);
        let text: FileText = if self.plain_os_fs {
            // PERF: on the plain OS file system a worker read the same
            // bytes, so the file is read once, as in Go. A bundled lib is
            // its embedded text (what `WrappedFs::read_file` copies).
            match take_prefetched(opts, script_kind, None) {
                Prefetched::Parse(file) => return Some(Rc::new(file)),
                Prefetched::Text(text) => text,
                Prefetched::Nothing => match bundled_text(&opts.file_name) {
                    Some(text) => text.into(),
                    None => {
                        let (text, ok) = self.fs.read_file(&opts.file_name);
                        if !ok {
                            return None;
                        }
                        FileText::new(text, freeable)
                    }
                },
            }
        } else {
            let (text, ok) = CompilerHost::fs(self).read_file(&opts.file_name);
            if !ok {
                return None;
            }
            // Another file system can show other bytes than the worker's
            // OS file system, so a worker result is used only for the same
            // text.
            match take_prefetched(opts, script_kind, Some(text.as_str())) {
                Prefetched::Parse(file) => return Some(Rc::new(file)),
                Prefetched::Text(worker_text) => worker_text,
                Prefetched::Nothing => FileText::new(text, freeable),
            }
        };
        // Not in Go: while `ast::free_file_versions` is on (`tsc --watch`,
        // or `GOPORT_FREE_FILE_VERSIONS=1`), a new parse of a path that this
        // thread published keeps its nodes in its store (lsshells M3c), as
        // the language server parse cache does;
        // `program::mark_freeable_parses` gives it a `FileVersion`. A store
        // that gets none is published static (its nodes are then leaked).
        let _owned_nodes = freeable.then(crate::ast::enter_freeable_parse);
        Some(Rc::new(parse_source_file(opts, text, script_kind)))
    }

    // Go: host.go:91 (*compilerHost).GetContentMappedSourceFiles (tsgo#4712)
    fn get_content_mapped_source_files(
        &self,
        parse_options: &SourceFileParseOptions,
        mapper: &Rc<Mapper>,
    ) -> Result<SourceFiles, GoError> {
        let Some(project) = &self.content_mapper_project else {
            return Err(contentmapper::ERR_PROJECT_UNAVAILABLE.clone());
        };
        content_mapped_source_files(&*CompilerHost::fs(self), &**project, parse_options, mapper)
    }

    // PORT: not in Go (see `CompilerHost::prefetch_content_mapped`).
    fn prefetch_content_mapped(&self) -> bool {
        true
    }

    // Go: host.go:106 (*compilerHost).ContentMapperProject (tsgo#4712)
    fn content_mapper_project(&self) -> Option<Rc<dyn Project>> {
        self.content_mapper_project.clone()
    }

    // Go: host.go:110 (*compilerHost).GetResolvedProjectReference
    fn get_resolved_project_reference(
        &self,
        file_name: &str,
        path: &Path,
    ) -> Option<Rc<ParsedCommandLine>> {
        let (command_line, _) = get_parsed_command_line_of_config_file_path(
            file_name,
            path.clone(),
            None,
            None, /*optionsRaw*/
            self,
            self.extended_config_cache.as_deref(),
        );
        command_line.map(Into::into)
    }

    fn is_plain_os_fs(&self) -> bool {
        self.plain_os_fs
    }
}

// PORT: Go passes the `*compilerHost` as a `tsoptions.ParseConfigHost`
// (it has `FS()` and `GetCurrentDirectory()`). Rust needs the explicit impl.
impl ParseConfigHost for CompilerHostImpl {
    fn fs(&self) -> Rc<dyn Fs> {
        self.fs.clone()
    }

    fn get_current_directory(&self) -> String {
        self.current_directory.clone()
    }
}
