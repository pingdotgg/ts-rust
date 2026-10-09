//! Go `internal/project/compilerhost.go`.
//!
//! PORT: Go `compilerHost` is the struct `CompilerHost`; the Go interface
//! `compiler.CompilerHost` is always written `compiler::CompilerHost`
//! (map-project.md section 4). The host is shared (`Rc<CompilerHost>` in
//! the project and `Rc<dyn compiler::CompilerHost>` in the program), and
//! `freeze` writes it after sharing, so the fields that `freeze` clears are
//! `RefCell`s. `freeze` drops the `builder` and `project` references, which
//! breaks the `Project -> host -> builder -> Project` cycle.

use crate::project::prelude::*;

use crate::contentmapper;
use crate::frontend::core_ext::{
    ensure_script_kind_from_file_name, get_script_kind_from_file_name,
};
use crate::frontend::module::{AheadAnswer, AheadCall, KeyList};
use crate::frontend::parser;
use std::cell::Cell;
use std::sync::Arc;
use xxhash_rust::xxh3::xxh3_128;

// Go: project/compilerhost.go:21 compilerHost
pub struct CompilerHost {
    pub config_file_path: tspath::Path,
    pub current_directory: String,
    pub session_options: Rc<SessionOptions>,

    pub source_fs: Rc<SourceFS>,
    pub config_file_registry: RefCell<Option<Rc<ConfigFileRegistry>>>,

    pub project: RefCell<Option<Rc<RefCell<Project>>>>,
    pub builder: RefCell<Option<Rc<ProjectCollectionBuilder>>>,
    pub logger: RefCell<Option<Rc<logging::LogTree>>>,
    // tsgo#4712. PORT: Go nil interface is `None`. `content_mapper_once`
    // is Go `contentMapperOnce` (`sync.Once`).
    pub content_mapper_project: RefCell<Option<Rc<dyn contentmapper::Project>>>,
    pub content_mapper_once: Cell<bool>,

    /// True when the project had no program when this host was made (its
    /// first load). `compiler::CompilerHost::prefetch_parses` returns it.
    // PORT: not in Go (see `compiler::CompilerHost::prefetch_parses`).
    pub first_load: bool,

    /// The module resolution keys of the last program load with this host,
    /// or else of the project's host before it, or else of the deleted
    /// project of the same config (`ResolveAheadStash`): the keys that the
    /// next load resolves ahead (`compiler::CompilerHost::resolve_ahead`).
    // PORT: not in Go (perf).
    pub resolution_keys: Rc<RefCell<Option<Arc<KeyList>>>>,

    /// The project's share in what the resolve-ahead workers keep from
    /// load to load, from the project's host before this one or from the
    /// stash (`ResolveAheadStash`). `release` drops it; when no host of the
    /// project and no stash has it, the workers drop what they keep
    /// (`compiler::resolve_ahead::KeptShare`).
    // PORT: not in Go (perf).
    pub kept_share: RefCell<Option<Rc<compiler::resolve_ahead::KeptShare>>>,
}

/// The resolve-ahead keys and kept share of the projects that a snapshot
/// clone made and deleted, by config file path. The first host of a later
/// project of the same config takes them (`new_compiler_host`), so its load
/// resolves ahead the keys of the deleted project's last load. The project
/// search of each hono file open makes and deletes tsconfig.spec.json; with
/// no keys, its load resolves its 1338 module keys on the loading thread.
/// The keys are hints only: the loader checks each answer
/// (`accept_ahead_answer`), and resolves itself a key that it does not
/// find. One per session (`SnapshotHost`), with at most `CAPACITY`
/// projects; a new one drops the least recently deleted (a project
/// deleted again moves to the end, and `take` removes its entry).
///
/// Rule: only `Snapshot::clone` puts, for a program that it made for a
/// project that its new collection does not have. A host that `release`
/// frees (the old host of a live project, or the last host of a project
/// whose files closed) drops its keys and share as before, so the kept
/// state of a closed project goes as `KeptShare` says.
// PORT: not in Go (perf; Go has no resolve ahead).
#[derive(Default)]
pub struct ResolveAheadStash {
    projects: RefCell<Vec<StashedProject>>,
}

struct StashedProject {
    config_file_path: tspath::Path,
    keys: Arc<KeyList>,
    share: Rc<compiler::resolve_ahead::KeptShare>,
}

impl ResolveAheadStash {
    const CAPACITY: usize = 8;

    /// Moves the keys and the share of `host` here, in place of older ones
    /// of its config. Nothing when its loads recorded no keys.
    pub fn put(&self, host: &CompilerHost) {
        if host.resolution_keys.borrow().is_none() {
            return;
        }
        let keys = host.resolution_keys.borrow_mut().take();
        let share = host.kept_share.borrow_mut().take();
        let (Some(keys), Some(share)) = (keys, share) else {
            return;
        };
        let mut projects = self.projects.borrow_mut();
        let older = projects
            .iter()
            .position(|project| project.config_file_path == host.config_file_path)
            .map(|index| projects.remove(index));
        projects.push(StashedProject {
            config_file_path: host.config_file_path.clone(),
            keys,
            share,
        });
        let least_recent = (projects.len() > Self::CAPACITY).then(|| projects.remove(0));
        drop(projects);
        // A dropped share can make the workers forget what they keep.
        drop(older);
        drop(least_recent);
    }

    /// Takes the keys and the share of the deleted project of
    /// `config_file_path`.
    fn take(
        &self,
        config_file_path: &tspath::Path,
    ) -> Option<(Arc<KeyList>, Rc<compiler::resolve_ahead::KeptShare>)> {
        let mut projects = self.projects.borrow_mut();
        let index = projects
            .iter()
            .position(|project| project.config_file_path == *config_file_path)?;
        let project = projects.remove(index);
        Some((project.keys, project.share))
    }
}

// Go: project/compilerhost.go:36 newCompilerHost
// PORT: reads `project.configFilePath`, so the caller must not hold a
// mutable borrow of `project` during this call. The host keeps its own
// `Rc`s of `project` and `builder` until `freeze`.
pub fn new_compiler_host(
    current_directory: &str,
    project: &Rc<RefCell<Project>>,
    builder: &Rc<ProjectCollectionBuilder>,
    logger: Option<Rc<logging::LogTree>>,
) -> Rc<CompilerHost> {
    let (config_file_path, first_load, resolution_keys, kept_share) = {
        let project = project.borrow();
        let (resolution_keys, kept_share) = match &project.host {
            Some(host) => (
                host.resolution_keys.borrow().clone(),
                host.kept_share.borrow().clone(),
            ),
            None => builder
                .resolve_ahead_stash
                .take(&project.config_file_path)
                .map_or((None, None), |(keys, share)| (Some(keys), Some(share))),
        };
        (
            project.config_file_path.clone(),
            project.program.is_none(),
            resolution_keys,
            kept_share,
        )
    };
    let source_fs = new_source_fs(true, builder.fs.clone(), builder.to_path.clone());
    UNFROZEN_HOSTS.with(|unfrozen| unfrozen.set(unfrozen.get() + 1));
    Rc::new(CompilerHost {
        config_file_path,
        current_directory: current_directory.to_string(),
        session_options: builder.session_options.clone(),

        source_fs,
        config_file_registry: RefCell::new(None),

        project: RefCell::new(Some(project.clone())),
        builder: RefCell::new(Some(builder.clone())),
        logger: RefCell::new(logger),
        content_mapper_project: RefCell::new(None),
        content_mapper_once: Cell::new(false),

        first_load,
        resolution_keys: Rc::new(RefCell::new(resolution_keys)),
        kept_share: RefCell::new(Some(kept_share.unwrap_or_default())),
    })
}

thread_local! {
    /// The hosts of this thread that are neither frozen nor dropped
    /// (`unfrozen_compiler_hosts`).
    static UNFROZEN_HOSTS: Cell<usize> = const { Cell::new(0) };
}

/// Not in Go: the number of hosts that `new_compiler_host` made on this
/// thread and that are neither frozen nor dropped. A host keeps its project
/// and the builder until `freeze`. Go's GC frees them with the host; the
/// port keeps the host of a released program (`release`), so every host
/// that a snapshot clone made must be frozen when the clone ends. Tests
/// check it.
#[must_use]
pub fn unfrozen_compiler_hosts() -> usize {
    UNFROZEN_HOSTS.with(Cell::get)
}

impl Drop for CompilerHost {
    fn drop(&mut self) {
        if self.builder.get_mut().is_some() {
            UNFROZEN_HOSTS.with(|unfrozen| unfrozen.set(unfrozen.get() - 1));
        }
    }
}

impl CompilerHost {
    // Go: project/compilerhost.go:57 compilerHost.freeze
    // freeze clears references to mutable state to make the compilerHost safe for use
    // after the snapshot has been finalized. See the usage in snapshot.go for more details.
    pub fn freeze(
        &self,
        snapshot_fs: Rc<SnapshotFS>,
        config_file_registry: Rc<ConfigFileRegistry>,
    ) {
        if self.builder.borrow().is_none() {
            crate::core::go_panic("freeze can only be called once".to_string());
        }
        *self.source_fs.source.borrow_mut() = snapshot_fs;
        self.source_fs.disable_tracking();
        *self.config_file_registry.borrow_mut() = Some(config_file_registry);
        // PORT: the old values are dropped after the borrows end, because
        // dropping the builder can drop other hosts and projects.
        let builder = self.builder.borrow_mut().take();
        // The count goes down where the builder goes, so a panic before
        // this line leaves the host counted once (`Drop` counts it down).
        UNFROZEN_HOSTS.with(|unfrozen| unfrozen.set(unfrozen.get() - 1));
        let project = self.project.borrow_mut().take();
        let logger = self.logger.borrow_mut().take();
        drop(builder);
        drop(project);
        drop(logger);
    }

    // Go: project/compilerhost.go:69 compilerHost.ensureAlive
    pub fn ensure_alive(&self) {
        if self.builder.borrow().is_none() || self.project.borrow().is_none() {
            crate::core::go_panic(
                "method must not be called after snapshot initialization".to_string(),
            );
        }
    }
}

// Go: project/compilerhost.go:14 `var _ compiler.CompilerHost = (*compilerHost)(nil)`
impl compiler::CompilerHost for CompilerHost {
    // Go: project/compilerhost.go:76 compilerHost.DefaultLibraryPath
    // DefaultLibraryPath implements compiler.CompilerHost.
    fn default_library_path(&self) -> String {
        self.session_options.default_library_path.clone()
    }

    // Go: project/compilerhost.go:81 compilerHost.FS
    // FS implements compiler.CompilerHost.
    fn fs(&self) -> Rc<dyn vfs::Fs> {
        self.source_fs.clone()
    }

    // PORT: not in Go (see `compiler::CompilerHost::without_fs_tracking`).
    // The tracking comes back also when `f` panics (`SourceFS::without_tracking`).
    fn without_fs_tracking(&self, f: &mut dyn FnMut()) {
        self.source_fs.without_tracking(f);
    }

    // Go: project/compilerhost.go:86 compilerHost.GetCurrentDirectory
    // GetCurrentDirectory implements compiler.CompilerHost.
    fn get_current_directory(&self) -> String {
        self.current_directory.clone()
    }

    // Go: project/compilerhost.go:91 compilerHost.GetResolvedProjectReference
    // GetResolvedProjectReference implements compiler.CompilerHost.
    fn get_resolved_project_reference(
        &self,
        file_name: &str,
        path: &tspath::Path,
    ) -> Option<Rc<tsoptions::ParsedCommandLine>> {
        let builder = self.builder.borrow().clone();
        match builder {
            None => self
                .config_file_registry
                .borrow()
                .as_ref()
                .unwrap_or_else(|| crate::core::go_nil_dereference())
                .get_config(path),
            Some(builder) => {
                // acquireConfigForProject will bypass sourceFS, so track the file here.
                self.source_fs.track(file_name);
                let project = self
                    .project
                    .borrow()
                    .clone()
                    .unwrap_or_else(|| crate::core::go_nil_dereference());
                let logger = self.logger.borrow().clone();
                builder
                    .config_file_registry_builder
                    .acquire_config_for_project(file_name, path, &project, logger)
            }
        }
    }

    // Go: project/compilerhost.go:103 compilerHost.GetSourceFile
    // GetSourceFile implements compiler.CompilerHost. Files are cached in parseCache
    // and acquired immediately for the in-progress program.
    // PORT: the parse cache holds `HashedSourceFile` (the file and Go's
    // `file.Hash`); the program gets the file.
    fn get_source_file(
        &self,
        opts: &parser::SourceFileParseOptions,
    ) -> Option<Rc<parser::ParsedSourceFile>> {
        self.ensure_alive();
        if let Some(fh) = self.source_fs.get_file_by_path(&opts.file_name, &opts.path) {
            let key = new_parse_cache_key(opts, fh.hash(), fh.kind());
            let builder = self
                .builder
                .borrow()
                .clone()
                .unwrap_or_else(|| crate::core::go_nil_dereference());
            return Some(builder.parse_cache.acquire(key, fh).file);
        }
        None
    }

    // Go: project/compilerhost.go:113 compilerHost.GetContentMappedSourceFiles (tsgo#4712)
    // GetContentMappedSourceFile implements compiler.CompilerHost.
    // PORT: a file that cannot be read is `Ok` with no canonical file (Go
    // returns the zero value and a nil error). Go `file.Hash = key.Hash` is
    // `set_source_file_hash` (project/parsecache.rs).
    fn get_content_mapped_source_files(
        &self,
        parse_options: &parser::SourceFileParseOptions,
        mapper: &Rc<contentmapper::Mapper>,
    ) -> Result<contentmapper::SourceFiles, GoError> {
        self.ensure_alive();
        let Some(fh) = self
            .source_fs
            .get_file_by_path(&parse_options.file_name, &parse_options.path)
        else {
            return Ok(contentmapper::SourceFiles::default());
        };
        let builder = self
            .builder
            .borrow()
            .clone()
            .unwrap_or_else(|| crate::core::go_nil_dereference());
        // ts#64163: the locale comes from the builder context.
        let diagnostic_locale = locale::from_context(&builder.ctx);
        // ts#64221
        let Some(project) = compiler::CompilerHost::content_mapper_project(self) else {
            return Err(contentmapper::ERR_PROJECT_UNAVAILABLE.clone());
        };
        let identity = match project.identity(mapper) {
            Ok(identity) => identity,
            Err(err) => {
                return Err(contentmapper::new_transform_error(
                    contentmapper::TransformErrorKind::PROJECT,
                    Some(err),
                )
                .to_go_error());
            }
        };
        let transform_identity = xxh3_128(identity.as_bytes());
        let key = content_mapped_parse_cache_key(
            parse_options,
            fh.hash(),
            transform_identity,
            &diagnostic_locale,
        );
        let files = builder
            .content_mapped_parse_cache
            .acquire_or_error(key.clone(), || {
                let files = contentmapper::transform_and_parse(
                    parse_options,
                    &fh.content(),
                    mapper,
                    &*project,
                )?;
                // Go: binder.BindSourceFile on the canonical file and on each
                // supplemental file (ts#63952). PORT: not ported; the Rust
                // binder binds each program version in one arena
                // (`program::bind_all`), see `new_parse_cache`.
                if let Some(canonical) = &files.canonical {
                    set_source_file_hash(canonical, key.hash);
                }
                for supplemental in &files.supplemental {
                    set_source_file_hash(supplemental, key.hash);
                }
                Ok(files)
            })?;
        let fs = compiler::CompilerHost::fs(self);
        if let Err(err) =
            contentmapper::check_supplemental_file_name_collisions(&files, &|name: &str| {
                vfs::Fs::file_exists(&*fs, name)
            })
        {
            deref_content_mapped_file(&builder.content_mapped_parse_cache, &key);
            return Err(err);
        }
        Ok(files)
    }

    // Go: project/compilerhost.go:153 compilerHost.ContentMapperProject (tsgo#4712, ts#64221)
    // PORT: the body of Go `ensureContentMapperProject` moved here in ts#64221
    // (Go `contentMapperOnce.Do`).
    fn content_mapper_project(&self) -> Option<Rc<dyn contentmapper::Project>> {
        if !self.content_mapper_once.replace(true) {
            let content_mapper_host = self
                .builder
                .borrow()
                .as_ref()
                .and_then(|builder| builder.content_mapper_host.clone());
            if let Some(content_mapper_host) = content_mapper_host {
                let project = self
                    .project
                    .borrow()
                    .clone()
                    .unwrap_or_else(|| crate::core::go_nil_dereference());
                let command_line = project.borrow().get_command_line_with_typings_files();
                // Go `ContentMappers` is nil-safe: a nil command line has
                // none, so it returns before the other getters.
                if let Some(command_line) =
                    command_line.filter(|command_line| !command_line.content_mappers().is_empty())
                {
                    let content_mapper_project =
                        content_mapper_host.project(contentmapper::ProjectSpec {
                            config_file_name: command_line.config_name().to_string(),
                            mappers: command_line.content_mappers().to_vec(),
                            compiler_options: Some(command_line.compiler_options().clone()),
                        });
                    *self.content_mapper_project.borrow_mut() = content_mapper_project;
                }
            }
        }
        self.content_mapper_project.borrow().clone()
    }

    // Go: project/compilerhost.go:172 compilerHost.Trace
    // Trace implements compiler.CompilerHost.
    fn trace(&self, msg: &'static crate::diagnostics::Message, args: Vec<String>) {
        let logger = self.logger.borrow().clone();
        logger.log(&crate::diagnostics_loc::message_localize(
            msg,
            &locale::DEFAULT,
            &args,
        ));
    }

    // PORT: not in Go (see `compiler::CompilerHost::prefetch_parses`). A
    // rebuild gets almost every file from the parse cache, which uses a
    // worker parse only on a miss. Parse workers would parse the whole
    // program again for nothing, and those parses stay in the workers' AST
    // arenas (about 30 MiB for each Query core rebuild). The first load of
    // a project still parses ahead, except the files that the parse cache
    // has (`cached_source_file_refs`).
    fn prefetch_parses(&self) -> bool {
        self.first_load
    }

    // PORT: not in Go (see `compiler::CompilerHost::cached_source_file_refs`).
    // The parse cache files whose key `get_source_file` can make in this
    // project's load. The loader then takes them from the cache, so the
    // workers do not parse them. A project that one clone made and deleted
    // keeps its files in the cache (as in Go), so when a later open makes it
    // again (hono's tsconfig.spec.json), its load starts no worker parse.
    //
    // Go's key (project/parsecache.go:22 NewParseCacheKey) is the parse
    // options (file name, path, jsx, force), the script kind and the text
    // hash. Before the load the host knows only some of them:
    // - hash and script kind (Go `FileHandle.Kind`, compilerhost.go:106):
    //   when the snapshot knows the file with no read (`known_file`: an
    //   open file or a cached one), they must match. Else the file is read
    //   from disk, whose kind comes from the name (Go overlayfs.go:108
    //   `cachedFile.Kind`), and any hash passes. `NewParseCacheKey` makes an
    //   unknown kind TS.
    // - jsx and force: what this project's options give the name. force also
    //   depends on the file's package.json scope, which the load finds later
    //   (ast/parseoptions.go:46 isFileForcedToBeModuleByFormat). Only a
    //   `"type": "module"` scope can set force, and the load reads the type
    //   only where Go does (fileloader.go:398 to :401, node16 to nodenext
    //   resolution or a `/node_modules/` path): there both values pass, and
    //   elsewhere only the value with no scope. A project with references parses
    //   their source files with the reference's own options (Go
    //   projectreferencefilemapper.go:80 getCompilerOptionsForFile, from
    //   fileloader.go:418), which the host does not have yet, so then any
    //   jsx and force pass.
    //
    // A wrong guess costs only time or memory: the output always comes from
    // `get_source_file` with the full key (Go compilerhost.go:106).
    // - A file in the map whose key misses the cache: no worker parses it,
    //   so the loader parses it itself (time).
    // - A file not in the map whose key hits the cache: a worker parses it,
    //   the loader takes the cached file, and the worker's parse stays in
    //   its AST arena (memory). The matches above never leave out a key
    //   that the load can make.
    fn cached_source_file_refs(&self) -> FxHashMap<String, Arc<compiler::FileRefs>> {
        let mut refs = FxHashMap::default();
        let (Some(builder), Some(project)) =
            (self.builder.borrow().clone(), self.project.borrow().clone())
        else {
            return refs;
        };
        let Some(command_line) = project.borrow().get_command_line_with_typings_files() else {
            return refs;
        };
        let options = command_line.compiler_options();
        let has_references = !command_line.project_references().is_empty();
        let no_scope = SourceFileMetaData::default();
        let esm_scope = SourceFileMetaData {
            package_json_type: "module".to_string(),
            implied_node_format: ModuleKind::ES_NEXT,
            ..SourceFileMetaData::default()
        };
        let module_resolution_kind = options.get_module_resolution_kind();
        let options_pass = |key: &ParseCacheKey| {
            if has_references {
                return true;
            }
            let guess = |metadata| {
                parser::get_external_module_indicator_options(&key.file_name, &options, metadata)
            };
            let plain = guess(&no_scope);
            key.jsx == plain.jsx
                && (key.force == plain.force
                    || compiler::package_json_type_applies(&key.file_name, module_resolution_kind)
                        && key.force == guess(&esm_scope).force)
        };
        for (key, entry) in builder.parse_cache.entries.borrow().iter() {
            let (hash, kind) = match builder.fs.known_file(&key.path) {
                Some((hash, kind)) => (Some(hash), kind),
                None => (None, get_script_kind_from_file_name(&key.file_name)),
            };
            let kind = if kind == ScriptKind::UNKNOWN {
                ensure_script_kind_from_file_name(&key.file_name)
            } else {
                kind
            };
            if key.script_kind != kind
                || hash.is_some_and(|hash| hash != key.hash)
                || !options_pass(key)
            {
                continue;
            }
            if let Some(file) = &*entry.value.borrow() {
                refs.insert(key.file_name.clone(), file.refs());
            }
        }
        refs
    }

    // PORT: not in Go (see `compiler::CompilerHost::release`). Go frees the
    // host when the last program that uses it is freed. The port keeps the
    // program shell (multiprog M2), so the host drops its data here: the
    // snapshot file system (disk file map copy, overlays, cachedvfs
    // results), the seen files and missing directories, the config
    // registry, and the resolve-ahead keys and share. A later file read
    // panics, like a use after `freeze` does for the builder.
    fn release(&self) {
        self.source_fs.release();
        let config_file_registry = self.config_file_registry.borrow_mut().take();
        drop(config_file_registry);
        let resolution_keys = self.resolution_keys.borrow_mut().take();
        drop(resolution_keys);
        let kept_share = self.kept_share.borrow_mut().take();
        drop(kept_share);
    }

    // PORT: not in Go (perf, see `compiler::CompilerHost::resolve_ahead`).
    // Only while the host tracks the files that its program load sees
    // (before `freeze`), on a case-sensitive file system whose layers are
    // the open files over the OS file system: the workers then see what
    // the loader sees, except the snapshot's cached files, which the check
    // compares (`accept_ahead_answer`). A case-insensitive file system
    // could spell a read file name another way than the loader would.
    fn resolve_ahead(&self) -> Option<compiler::resolve_ahead::ResolveAheadHost> {
        if !self.source_fs.tracking.get() {
            return None;
        }
        let builder = self.builder.borrow().clone()?;
        let files = builder.fs.clone();
        // The loader reads `files` through `source_fs`.
        if !std::ptr::addr_eq(
            Rc::as_ptr(&*self.source_fs.source.borrow()),
            Rc::as_ptr(&files),
        ) {
            return None;
        }
        let use_case_sensitive_file_names =
            vfs::Fs::use_case_sensitive_file_names(&*self.source_fs);
        if !use_case_sensitive_file_names {
            return None;
        }
        // The workers make the paths as `source_fs` does.
        let current_directory = self.session_options.current_directory.clone();
        let probe = "a/B.ts";
        if (self.source_fs.to_path)(probe)
            != tspath::to_path(probe, &current_directory, use_case_sensitive_file_names)
        {
            return None;
        }
        let (open_files, open_directories) = files.open_files_over_os()?;
        let lookups = files.cached_fs()?;
        ahead_lookup_layer(&lookups)?;
        let job = Rc::new(Cell::new(usize::MAX));
        let attach = {
            let lookups = lookups.clone();
            let job = job.clone();
            Box::new(move |stats| {
                if let Some(layer) = ahead_lookup_layer(&lookups) {
                    job.set(layer.attach(stats));
                }
            }) as Box<dyn FnOnce(Arc<dyn compiler::resolve_ahead::AheadLookups>)>
        };
        let load = AheadCheck {
            lookups,
            job,
            reads: RefCell::new(FxHashMap::default()),
            groups: RefCell::new(FxHashSet::default()),
        };
        let accept = {
            let source_fs = self.source_fs.clone();
            let files = files.clone();
            Rc::new(move |_answer: AheadAnswer<'_>, calls: &[AheadCall]| {
                accept_ahead_answer(&source_fs, &files, &load, calls)
            })
        };
        let keys = self.resolution_keys.clone();
        let scratch = cfg!(debug_assertions).then(|| {
            let to_path = self.source_fs.to_path.clone();
            Rc::new(move || {
                let fs = new_source_fs(true, files.clone(), to_path.clone());
                let tracked = fs.clone();
                compiler::resolve_ahead::ScratchFs {
                    fs,
                    tracked: Box::new(move || {
                        let seen = tracked
                            .seen_files
                            .borrow()
                            .as_ref()
                            .map(|seen| seen.borrow().keys().cloned().collect());
                        let missing = tracked
                            .missing_directories
                            .as_ref()
                            .map(|missing| missing.borrow().clone());
                        (seen.unwrap_or_default(), missing.unwrap_or_default())
                    }),
                    to_path: to_path.clone(),
                }
            }) as Rc<dyn Fn() -> compiler::resolve_ahead::ScratchFs>
        });
        Some(compiler::resolve_ahead::ResolveAheadHost {
            previous_keys: self.resolution_keys.borrow().clone(),
            view: compiler::resolve_ahead::WorkerView {
                current_directory,
                use_case_sensitive_file_names,
                open_files,
                open_directories,
            },
            accept,
            attach,
            keep_keys: Box::new(move |new_keys| *keys.borrow_mut() = Some(new_keys)),
            share: self.kept_share.borrow().clone()?,
            scratch,
        })
    }
}

/// Checks the file system calls of a resolve-ahead answer on this host's
/// file system, as the loader's own resolution of the key would make them
/// at this point of the load, and replays their side effects
/// (`compiler::CompilerHost::resolve_ahead`). False: a call would give
/// another answer here, and the loader resolves the key itself.
///
/// - `file_exists`: the snapshot's cached file of the path decides, if it
///   has one (`SnapshotFSBuilder::cached_file_state`). A cached file that
///   needs a reload fails the check: the loader's lookup would read it.
///   Else the snapshot's lookup cache decides, if it has an answer (Go
///   `cachedvfs` of the layered file system, which the worker read the same
///   way: open files over the OS), and else the lookups of an earlier
///   resolve-ahead job of the snapshot, if one has an answer
///   (`AheadLookupLayer`). With neither, the answer passes: the cache takes
///   this load's job answer, which is the worker's, when it is asked
///   (`ResolveAheadHost::attach`). A `known` answer (not found in this load)
///   is asked of the layered file system through the cache.
/// - `directory_exists` and `realpath`: the same, and a call that the job's
///   answers share (`AheadCall::Shared`) is checked once per load.
/// - a read is the loader's own read (`SourceFS::get_file`: it tracks the
///   file, caches it and notes a `node_modules` realpath alias), made at
///   the moment the loader would make it, since every call before it gave
///   the same answer. Its text must have the worker's hash. `reads` keeps
///   the reads of this load, so a later answer that reads the file again
///   only compares the hash. A worker read that failed fails the check.
/// - then each `file_exists` path becomes a seen file and each missing
///   directory a missing directory, as the loader's calls would note them.
/// - the calls of a package.json cache entry (`AheadCall::PackageJson`)
///   are checked and replayed once per load: the snapshot does not change
///   its answers during the load, and the replay notes the same paths
///   again.
/// What `accept_ahead_answer` keeps during one load.
struct AheadCheck {
    /// The snapshot's lookup cache (`SnapshotFSBuilder::cached_fs`).
    lookups: Rc<vfs::CachedFs>,
    /// The index of this load's job in the layer under `lookups`
    /// (`AheadLookupLayer`; `usize::MAX` until the load gives the workers a
    /// job).
    job: Rc<Cell<usize>>,
    /// The hash of each file that the check read, by name.
    reads: RefCell<FxHashMap<String, Option<u128>>>,
    /// The shared calls (`AheadCall::PackageJson`, `AheadCall::Shared`)
    /// that passed the check and were replayed, by address (the answers
    /// keep them for the whole load).
    groups: RefCell<FxHashSet<usize>>,
}

impl AheadCheck {
    /// True when `answer`, a worker's answer for a lookup, is the answer of
    /// the snapshot: `cached`, the answer of its lookup cache, or else the
    /// answer of an earlier job (`before`), if it has one.
    fn agrees<T: PartialEq>(
        &self,
        cached: Option<T>,
        before: impl Fn(&dyn compiler::resolve_ahead::AheadLookups) -> Option<T>,
        answer: &T,
    ) -> bool {
        cached
            .or_else(|| ahead_lookup_layer(&self.lookups)?.before(self.job.get(), before))
            .is_none_or(|snapshot| snapshot == *answer)
    }
}

fn accept_ahead_answer(
    source_fs: &SourceFS,
    files: &SnapshotFSBuilder,
    load: &AheadCheck,
    calls: &[AheadCall],
) -> bool {
    if !calls
        .iter()
        .all(|call| check_ahead_call(source_fs, files, load, call))
    {
        return false;
    }
    for call in calls {
        replay_ahead_call(source_fs, load, call);
    }
    true
}

/// True when `call` gives the same answer on this host's file system
/// (`accept_ahead_answer`).
fn check_ahead_call(
    source_fs: &SourceFS,
    files: &SnapshotFSBuilder,
    load: &AheadCheck,
    call: &AheadCall,
) -> bool {
    match call {
        AheadCall::FileExists {
            path,
            exists,
            known,
        } => match files.cached_file_state(path) {
            // The worker asked the OS through the open files, as the
            // layered file system does; a known answer is checked here
            // (the same call as the loader's own lookup).
            CachedFileState::Absent if *known => {
                vfs::Fs::file_exists(&*load.lookups, path.as_str()) == *exists
            }
            CachedFileState::Absent => load.agrees(
                load.lookups.cached_file_exists(path.as_str()),
                |lookups| lookups.file_exists(path.as_str()),
                exists,
            ),
            CachedFileState::Live => *exists,
            CachedFileState::NoValue => !*exists,
            CachedFileState::NeedsReload => false,
        },
        AheadCall::DirectoryExists { path, exists } => load.agrees(
            load.lookups.cached_directory_exists(path.as_str()),
            |lookups| lookups.directory_exists(path.as_str()),
            exists,
        ),
        AheadCall::Realpath { name, real } => load.agrees(
            load.lookups.cached_realpath(name),
            |lookups| lookups.realpath(name),
            real,
        ),
        // A read that failed is not taken: the disk changed during the load
        // (a package.json that `file_exists` found was gone), or the file
        // cannot be read. The snapshot does not cache a failed read, so a
        // later read in the load can find the file, and then the worker's
        // resolution without its text is not the loader's. A worker logs a
        // failed read only for a file that it knows from an earlier job, so
        // the rejection drops the known files; any other failed read makes
        // the answer unshareable (compiler/resolve_ahead.rs
        // `AheadFs::read_file`).
        AheadCall::Read { hash: None, .. } => false,
        AheadCall::Read {
            file_name,
            hash: Some(hash),
        } => {
            let known = load.reads.borrow().get(file_name).copied();
            let read = known.unwrap_or_else(|| {
                let read = source_fs.get_file(file_name).map(|file| file.hash());
                load.reads.borrow_mut().insert(file_name.clone(), read);
                read
            });
            read == Some(*hash)
        }
        AheadCall::PackageJson(group) => {
            load.groups.borrow().contains(&group_key(group))
                || group
                    .iter()
                    .all(|call| check_ahead_call(source_fs, files, load, call))
        }
        AheadCall::Shared(call) => {
            load.groups.borrow().contains(&shared_key(call))
                || check_ahead_call(source_fs, files, load, call)
        }
    }
}

/// Notes the side effects of `call` (`accept_ahead_answer`).
fn replay_ahead_call(source_fs: &SourceFS, load: &AheadCheck, call: &AheadCall) {
    match call {
        // The call was noted only when the name was its path
        // (`resolve_ahead::AheadFs::note_call`).
        AheadCall::FileExists { path, .. } => source_fs.track_path(path.as_str(), path),
        AheadCall::DirectoryExists {
            path,
            exists: false,
        } => source_fs.note_missing_directory(path),
        AheadCall::DirectoryExists { exists: true, .. }
        | AheadCall::Realpath { .. }
        | AheadCall::Read { .. } => {}
        AheadCall::PackageJson(group) => {
            if load.groups.borrow_mut().insert(group_key(group)) {
                for call in group.iter() {
                    replay_ahead_call(source_fs, load, call);
                }
            }
        }
        AheadCall::Shared(call) => {
            if load.groups.borrow_mut().insert(shared_key(call)) {
                replay_ahead_call(source_fs, load, call);
            }
        }
    }
}

fn group_key(group: &Arc<[AheadCall]>) -> usize {
    Arc::as_ptr(group).cast::<AheadCall>() as usize
}

fn shared_key(call: &Arc<AheadCall>) -> usize {
    Arc::as_ptr(call) as usize
}
