//! Go: execute/build/host.go, execute/build/compilerHost.go, and the
//! `ExtendedConfigCache` of execute/tsc/extendedconfigcache.go.
//!
//! PORT: Go `host` keeps a pointer to its `*Orchestrator` and reads
//! `opts.Sys`, `opts.Command` and `toPath` through it. Here the host keeps
//! those values itself, so it needs no reference back to the orchestrator.
//! The orchestrator owns the host as `Rc<BuildHost>` and passes clones
//! where Go passes `o.host`.
//!
//! PORT: Go `time.Time` is `Option<SystemTime>` (`None` = zero) and
//! `time.Duration` is `Duration`, as in build_task.rs.

use crate::contentmapper::{self, Mapper, Project, SourceFiles};
use crate::execute::build::command_line::ParsedBuildCommandLine;
use crate::execute::build::config_prefetch::PrefetchPool;
use crate::execute::build::parse_cache::ParseCache;
use crate::execute::incremental::incremental;
use crate::execute::tsc::compile::System;
use crate::frontend::prelude::*;
use crate::gostd::GoError;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime};

// Go: tsc/extendedconfigcache.go:16 ExtendedConfigCache
// PORT: the Go type is `tsc.ExtendedConfigCache`; the Rust name adds `Tsc`
// because the `tsoptions.ExtendedConfigCache` interface already has the
// plain name. The per-entry mutex is dropped (one thread, see
// parse_cache.rs). The map borrow is not held while the config parses, so
// a nested `extends` can use the cache.
#[derive(Default)]
pub struct TscExtendedConfigCache {
    m: RefCell<FxHashMap<Path, Rc<ExtendedConfigCacheEntry>>>,
}

impl ExtendedConfigCache for TscExtendedConfigCache {
    // Go: tsc/extendedconfigcache.go:28 (*ExtendedConfigCache).GetExtendedConfig
    fn get_extended_config(
        &self,
        file_name: &str,
        path: &Path,
        resolution_stack: &[Path],
        host: &dyn ParseConfigHost,
    ) -> Rc<ExtendedConfigCacheEntry> {
        if let Some(entry) = self.m.borrow().get(path) {
            return entry.clone();
        }
        let entry = Rc::new(parse_extended_config(
            file_name,
            path.clone(),
            resolution_stack,
            host,
            Some(self),
        ));
        self.m
            .borrow_mut()
            .entry(path.clone())
            .or_insert(entry)
            .clone()
    }
}

impl TscExtendedConfigCache {
    // PORT: Go assigns a new cache (`o.host.extendedConfigCache =
    // tsc.ExtendedConfigCache{}`, orchestrator.go:276). The programs of a
    // build keep the host `Rc`, so the cache is emptied in place.
    pub fn reset(&self) {
        self.m.borrow_mut().clear();
    }
}

// PORT: Go keys the source file cache by `ast.SourceFileParseOptions`, a
// comparable struct. The Rust struct has no `Hash`, so this key hashes the
// same fields.
#[derive(Clone, PartialEq, Eq)]
pub struct SourceFileCacheKey(pub SourceFileParseOptions);

/// PORT: not in Go. A parse that `BuildHost::watch_source_file` keeps, and
/// the modification time of its file at the parse.
pub struct WatchSource {
    file: Rc<ParsedSourceFile>,
    mod_time: Option<std::time::SystemTime>,
    /// `reads_module_indicator_options(file)`, found at the parse: a later
    /// node read of a freeable version pins it on this thread until the
    /// next program release (`ast::with_file_version`), so a test of a
    /// kept parse before a build would keep it through the build.
    reads_options: bool,
}

/// PORT: not in Go. What the module indicator options of a parse take from
/// the compiler options (Go `GetExternalModuleIndicatorOptions`,
/// parseoptions.go:19; the rest is the file name and its package.json):
/// the detection kind, and with `auto` only, whether `jsx` is react-jsx or
/// react-jsxdev and whether the module resolution is in Node16..NodeNext,
/// which reads the package.json `type` (`loadSourceFileMetaData`,
/// fileloader.go:384). `module` has an effect only through the two
/// default kinds here: with no `moduleDetection`, a module kind in
/// Node16..NodeNext gives `force` (`GetEmitModuleDetectionKind`,
/// compileroptions.go:243), and with no `moduleResolution` (or classic or
/// node10) the module kind picks the resolution kind
/// (`GetModuleResolutionKind`, compileroptions.go:227). The module kind
/// that `isFileForcedToBeModuleByFormat` passes on is not an input: once
/// the `type` is known, `GetImpliedNodeFormatForEmitWorker`
/// (utilities.go:2622) gives ESNext to the same files with any module
/// kind. Two configs with the same inputs
/// give each file the same module indicator options. See
/// `BuildHost::drop_kept_parses_whose_module_indicator_options_change`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ModuleIndicatorInputs {
    detection: ModuleDetectionKind,
    jsx: bool,
    node_resolution: bool,
}

impl ModuleIndicatorInputs {
    #[must_use]
    pub fn of(options: &CompilerOptions) -> Self {
        let detection = options.get_emit_module_detection_kind();
        let auto = detection == ModuleDetectionKind::AUTO;
        let resolution = options.get_module_resolution_kind();
        ModuleIndicatorInputs {
            detection,
            jsx: auto
                && (options.jsx == JsxEmit::REACT_JSX || options.jsx == JsxEmit::REACT_JSX_DEV),
            node_resolution: auto
                && ModuleResolutionKind::NODE16 <= resolution
                && resolution <= ModuleResolutionKind::NODE_NEXT,
        }
    }

    /// The module indicator options of `file_name` under these inputs
    /// (Go `GetExternalModuleIndicatorOptions`), or `None` when they take
    /// the package.json `type`, which is not read here: with `auto`, for a
    /// file that is not .mts, .cts, .mjs or .cjs, under a Node16..NodeNext
    /// resolution or in node_modules (fileloader.go:384).
    #[must_use]
    pub fn options_of(self, file_name: &str) -> Option<ExternalModuleIndicatorOptions> {
        if is_declaration_file_name(file_name) {
            return Some(ExternalModuleIndicatorOptions::default());
        }
        match self.detection {
            ModuleDetectionKind::FORCE => Some(ExternalModuleIndicatorOptions {
                jsx: false,
                force: true,
            }),
            ModuleDetectionKind::AUTO => {
                // `isFileForcedToBeModuleByFormat` (parseoptions.go:46).
                let force = file_extension_is_one_of(
                    file_name,
                    &[EXTENSION_CJS, EXTENSION_CTS, EXTENSION_MJS, EXTENSION_MTS],
                );
                if !force && (self.node_resolution || file_name.contains("/node_modules/")) {
                    return None;
                }
                Some(ExternalModuleIndicatorOptions {
                    jsx: self.jsx,
                    force,
                })
            }
            _ => Some(ExternalModuleIndicatorOptions::default()),
        }
    }
}

impl Hash for SourceFileCacheKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.file_name.hash(state);
        self.0.path.hash(state);
        self.0.external_module_indicator_options.jsx.hash(state);
        self.0.external_module_indicator_options.force.hash(state);
    }
}

// Go: vfs/cachedvfs/cachedvfs.go FS, the file system of the build host
// (`cachedvfs.From(sys.FS())`, orchestrator.go:1004).
// PORT: the same cache as `CachedFs` (always enabled: the build host
// never disables it), but the `FileExists`, `DirectoryExists`,
// `Realpath` and `GetAccessibleEntries` lookups live in a
// `BuildStatCache` that the parse workers of each program load read too
// (`CompilerHost::stat_cache`), as Go parse tasks share the host's
// cachedvfs. This cache lasts for the whole build, and a write does not
// update it (cachedvfs.go:144), so a later program can find a lookup here
// that an earlier program made before the build wrote that path. It holds
// only the lookups that Go makes: the workers' own lookups stay out of it
// unless the loader uses them (see `BuildStatCache`).
pub struct BuildCachedFs {
    fs: Rc<dyn Fs>,
    stats: Arc<BuildStatCache>,
    stat_cache: RefCell<FxHashMap<String, Option<FileInfo>>>,
}

impl BuildCachedFs {
    // Go: cachedvfs.go:25 From
    fn new(fs: Rc<dyn Fs>) -> BuildCachedFs {
        BuildCachedFs {
            fs,
            stats: Arc::default(),
            stat_cache: RefCell::default(),
        }
    }

    // Go: cachedvfs.go:41 ClearCache
    pub fn clear_cache(&self) {
        self.stats.clear();
        self.stat_cache.borrow_mut().clear();
    }
}

impl Fs for BuildCachedFs {
    fn use_case_sensitive_file_names(&self) -> bool {
        self.fs.use_case_sensitive_file_names()
    }

    fn file_exists(&self, path: &str) -> bool {
        self.stats.file_exists(path, || self.fs.file_exists(path))
    }

    fn read_file(&self, path: &str) -> (String, bool) {
        self.fs.read_file(path)
    }

    fn write_file(&self, path: &str, data: &str) -> Result<(), FsError> {
        self.fs.write_file(path, data)
    }

    fn append_file(&self, path: &str, data: &str) -> Result<(), FsError> {
        self.fs.append_file(path, data)
    }

    fn remove(&self, path: &str) -> Result<(), FsError> {
        self.fs.remove(path)
    }

    fn chtimes(
        &self,
        path: &str,
        a_time: Option<SystemTime>,
        m_time: Option<SystemTime>,
    ) -> Result<(), FsError> {
        self.fs.chtimes(path, a_time, m_time)
    }

    fn directory_exists(&self, path: &str) -> bool {
        self.stats
            .directory_exists(path, || self.fs.directory_exists(path))
    }

    fn get_accessible_entries(&self, path: &str) -> Entries {
        self.stats
            .entries(path, || self.fs.get_accessible_entries(path))
    }

    fn stat(&self, path: &str) -> Option<FileInfo> {
        if let Some(ret) = self.stat_cache.borrow().get(path) {
            return ret.clone();
        }
        let ret = self.fs.stat(path);
        self.stat_cache
            .borrow_mut()
            .insert(path.to_string(), ret.clone());
        ret
    }

    fn realpath(&self, path: &str) -> String {
        self.stats.realpath(path, || self.fs.realpath(path))
    }
}

// Go: build/host.go:18 host
pub struct BuildHost {
    // PORT: in place of Go `orchestrator *Orchestrator` (see top).
    sys: Rc<dyn System>,
    command: Rc<ParsedBuildCommandLine>,
    compare_paths_options: ComparePathsOptions,

    host: Rc<dyn CompilerHost>,
    // PORT: the `*cachedvfs.FS` of `host`, kept for `resetCaches`. Go
    // reaches it as `o.host.host.FS().(*cachedvfs.FS)` (orchestrator.go:272).
    pub cached_fs: Rc<BuildCachedFs>,

    // Caches that last only for build cycle and then cleared out
    pub extended_config_cache: TscExtendedConfigCache,
    pub source_files: ParseCache<SourceFileCacheKey, Rc<ParsedSourceFile>>,
    // PORT: not in Go. The references of each parse in `source_files`, by
    // file name, made when a program load first needs them
    // (`cached_source_file_refs`).
    cached_refs: RefCell<FxHashMap<String, (std::rc::Weak<ParsedSourceFile>, Arc<FileRefs>)>>,
    pub config_times: RefCell<FxHashMap<Path, Duration>>,

    // caches that stay as long as they are needed
    pub resolved_references: ParseCache<Path, Rc<ParsedCommandLine>>,
    // PORT: not in Go (perf). The threads that parse the configs of the
    // graph ahead of `get_resolved_project_reference` (config_prefetch.rs),
    // until the graph is made.
    pub config_prefetch: RefCell<Option<PrefetchPool>>,
    // PORT: not in Go (`CompilerHost::prefetch_parses`). In watch mode,
    // true only in the first build and in the cycles with a config change
    // or an overflow (set by `Orchestrator::watch` and `do_cycle`). Another
    // cycle takes almost every file from `watch_sources`: a parse of a
    // changed file on a worker saves less than the pool costs. The workers
    // skip the parses that `watch_source_file` keeps
    // (`cached_source_file_refs`), and a worker parse of a published path
    // is a freeable file version (`freeable_worker_parses`).
    pub prefetch: std::cell::Cell<bool>,
    // PORT: not in Go (`watch_source_file`). The parses of the files in
    // `tsc -b --watch`, with the modification time of each file at its
    // parse. `None` outside watch mode. A `.d.ts` or `.json` file comes
    // through `source_files` first, which keeps the first parse of a cycle
    // for the rest of the cycle (`get_source_file`).
    pub watch_sources: RefCell<Option<FxHashMap<SourceFileCacheKey, WatchSource>>>,
    // PORT: not in Go (`keep_watch_sources_for_config_change`). In a cycle
    // after a config change, the parses that `watch_sources` had before it
    // and that no build of the cycle took yet.
    watch_sources_before_config_change: RefCell<FxHashMap<SourceFileCacheKey, WatchSource>>,
    // PORT: Go `*collections.SyncMap`. The task `writeFile` stores into it
    // from the checker threads.
    pub m_times: Arc<Mutex<FxHashMap<Path, Option<SystemTime>>>>,
    // PORT: not in Go (perf). The paths that the tasks of this build wrote
    // (the task `writeFile`) or touched (`set_m_time`). A check reads such
    // a file again, as Go does, in place of what the build info prefetch
    // read before the write (orchestrator.rs `BuildInfoPrefetch`). The
    // orchestrator clears it at the start and the end of each build.
    pub written: Arc<WrittenPaths>,
}

/// PORT: not in Go (perf). A set of paths (`BuildHost::written`), with a
/// check that takes no lock while the set is empty (a build that writes
/// nothing, such as a noop build).
#[derive(Default)]
pub struct WrittenPaths {
    any: std::sync::atomic::AtomicBool,
    paths: Mutex<FxHashSet<Path>>,
}

impl WrittenPaths {
    pub fn insert(&self, path: Path) {
        self.paths
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(path);
        self.any.store(true, std::sync::atomic::Ordering::Release);
    }

    pub fn contains(&self, path: &Path) -> bool {
        self.any.load(std::sync::atomic::Ordering::Acquire)
            && self
                .paths
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .contains(path)
    }

    pub fn clear(&self) {
        self.any.store(false, std::sync::atomic::Ordering::Release);
        self.paths
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
    }
}

impl BuildHost {
    // PORT: Go builds the host inline in `NewOrchestrator`
    // (orchestrator.go:1004): `compiler.NewCachedFSCompilerHost(cwd, sys.FS(),
    // sys.DefaultLibraryPath(), nil, nil, nil)` and an empty mTimes map.
    // `NewCachedFSCompilerHost` is written out (compiler/host.go:44) to keep
    // the cached file system.
    pub fn new(
        sys: Rc<dyn System>,
        command: Rc<ParsedBuildCommandLine>,
        compare_paths_options: ComparePathsOptions,
    ) -> BuildHost {
        let base = sys.fs();
        let cached_fs = Rc::new(BuildCachedFs::new(base.clone()));
        // PORT: Go `NewCompilerHost`. The host sees through the cache to
        // `sys.FS()`, so on the OS file system the parse workers read and
        // resolve for it (`CompilerHost::is_plain_os_fs`), as for `tsc -p`.
        let host = new_compiler_host_over(
            &sys.get_current_directory(),
            cached_fs.clone(),
            &base,
            &sys.default_library_path(),
        );
        BuildHost {
            sys,
            command,
            compare_paths_options,
            host,
            cached_fs,
            extended_config_cache: TscExtendedConfigCache::default(),
            source_files: ParseCache::default(),
            cached_refs: RefCell::default(),
            config_times: RefCell::new(FxHashMap::default()),
            resolved_references: ParseCache::default(),
            config_prefetch: RefCell::new(None),
            prefetch: std::cell::Cell::new(true),
            watch_sources: RefCell::new(None),
            watch_sources_before_config_change: RefCell::default(),
            m_times: Arc::default(),
            written: Arc::default(),
        }
    }

    /// PORT: not in Go (memory and time). In `tsc -b --watch` a file keeps
    /// its parse while its modification time does not change, no watch
    /// event names it and no build of this cycle wrote it, as Go
    /// `tsc --watch` keeps its files (execute/watcher.go `sourceFileCache`).
    /// Go parses every file of each project that it builds again in each
    /// cycle (`resetCaches`, and `sourceFiles` keeps the `.d.ts` and `.json`
    /// files of one cycle). Here each such parse is a new file version, and
    /// with parse workers off after the first build (`prefetch`) the parses
    /// of a cycle ran on one thread (query-persist-client-core rebuilds took
    /// 2.7 times as long). A parse of the same text with the same options is
    /// the same file, so the output does not change. A bundled lib never
    /// changes. A `.d.ts` or `.json` file comes here through `source_files`
    /// (`get_source_file`), so in one cycle only its first parse comes here.
    fn watch_source_file(&self, opts: &SourceFileParseOptions) -> Option<Rc<ParsedSourceFile>> {
        let key = SourceFileCacheKey(opts.clone());
        let fixed = crate::frontend::bundled::is_bundled(&opts.file_name);
        // The OS file system, not the one this cycle caches: a build of this
        // cycle can write the file after a lookup of it.
        let mod_time = if fixed {
            None
        } else {
            self.sys
                .fs()
                .stat(&opts.file_name)
                .and_then(|info| info.mod_time())
        };
        let keep = fixed || (mod_time.is_some() && !self.written.contains(&opts.path));
        let cached = self
            .watch_sources
            .borrow()
            .as_ref()
            .and_then(|sources| sources.get(&key))
            .filter(|source| keep && source.mod_time == mod_time)
            .map(|source| source.file.clone());
        if cached.is_some() {
            return cached;
        }
        // A kept parse from before a config change of this cycle: one with
        // these parse options, or with other module indicator options that
        // its parse did not read (`parse_with_options`).
        let before = self
            .take_watch_source_before_config_change(opts)
            .filter(|source| keep && source.mod_time == mod_time);
        if let Some(source) = before {
            let file = source.file.clone();
            if let Some(sources) = self.watch_sources.borrow_mut().as_mut() {
                sources.insert(key, source);
            }
            return Some(file);
        }
        let file = self.host.get_source_file(opts);
        // Each parse here can be kept and be a program file of a later
        // build, also a `.ts` file that one program leaves out (see
        // `note_kept_parse`, and the note in `get_source_file`).
        if let Some(file) = &file {
            crate::execute::watcher::note_kept_parse(file);
        }
        if let Some(sources) = self.watch_sources.borrow_mut().as_mut() {
            match &file {
                Some(file) if fixed || mod_time.is_some() => {
                    sources.insert(
                        key,
                        WatchSource {
                            file: file.clone(),
                            mod_time,
                            reads_options: crate::frontend::parser::reads_module_indicator_options(
                                file,
                            ),
                        },
                    );
                }
                _ => {
                    sources.remove(&key);
                }
            }
        }
        file
    }

    /// PORT: not in Go (`watch_source_file`). Drops the kept parses of the
    /// files at `paths` (the paths of a cycle's watch events), or every
    /// kept parse when `paths` is `None` (an overflow).
    pub fn evict_watch_sources(&self, paths: Option<&FxHashSet<Path>>) {
        if let Some(sources) = self.watch_sources.borrow_mut().as_mut() {
            match paths {
                Some(paths) => sources.retain(|key, _| !paths.contains(&key.0.path)),
                None => sources.clear(),
            }
        }
    }

    /// PORT: not in Go (`watch_source_file`). A cycle with a config change
    /// keeps the parses too: the key of a parse holds its parse options, so
    /// a project whose options changed parses again only the files whose
    /// parse options changed and whose parses read the changed options
    /// (`take_watch_source_before_config_change`). Go parses every file
    /// again in each cycle, and the port did in a config change cycle, on
    /// one thread (a query-persist-client-core build after a tsconfig edit
    /// took 2 times as long as Go). The kept parses wait in
    /// `watch_sources_before_config_change`, and a build of the cycle takes
    /// back each one that it uses (`watch_source_file`). Then
    /// `end_config_change_cycle` drops the ones that it replaced.
    pub fn keep_watch_sources_for_config_change(&self) {
        if let Some(sources) = self.watch_sources.borrow_mut().as_mut() {
            self.watch_sources_before_config_change
                .borrow_mut()
                .extend(sources.drain());
        }
    }

    /// PORT: not in Go (`watch_source_file`). Takes the kept parse from
    /// before a config change that a parse with `opts` would give: the one
    /// with `opts`, else one whose parse options differ only in module
    /// indicator options that it did not read, as a copy with `opts`
    /// (`parse_with_options`). The key of a kept parse holds its options, so
    /// the other three module indicator options are looked up.
    fn take_watch_source_before_config_change(
        &self,
        opts: &SourceFileParseOptions,
    ) -> Option<WatchSource> {
        let mut before = self.watch_sources_before_config_change.borrow_mut();
        if before.is_empty() {
            return None;
        }
        let mut key = SourceFileCacheKey(opts.clone());
        if let Some(source) = before.remove(&key) {
            return Some(source);
        }
        for (jsx, force) in [(false, false), (false, true), (true, false), (true, true)] {
            key.0.external_module_indicator_options = ExternalModuleIndicatorOptions { jsx, force };
            if key.0.external_module_indicator_options == opts.external_module_indicator_options {
                continue;
            }
            let Some(source) = before.get(&key) else {
                continue;
            };
            // Such a parse fits only its own options (no node read here,
            // see `WatchSource::reads_options`).
            if source.reads_options {
                return None;
            }
            let file = crate::frontend::parser::parse_with_options(&source.file, opts)?;
            let mod_time = source.mod_time;
            before.remove(&key);
            return Some(WatchSource {
                file,
                mod_time,
                reads_options: false,
            });
        }
        None
    }

    /// PORT: not in Go (`keep_watch_sources_for_config_change`). Drops the
    /// kept parses from before the config change of this cycle that its
    /// builds will not take, so that they are freed before the builds parse
    /// these files again. `Orchestrator::do_cycle` gives each project whose
    /// change gave it other `ModuleIndicatorInputs`: its config from before
    /// the change, whose files are the parses it holds, and its new inputs
    /// (`None` when its config is gone). A kept parse that read its module
    /// indicator options (`WatchSource::reads_options`) fits only a parse
    /// with its own options. If it is a file of such a project, it goes
    /// unless the options of the file under the new inputs of one of these
    /// projects are its own (`ModuleIndicatorInputs::options_of`); when
    /// such a project parses the file again with other options, it would
    /// go at `end_config_change_cycle` all the same. The parses of the
    /// other projects stay: Go shares no such parse between the programs
    /// of a build (build/host.go:54 keeps only `.d.ts` and `.json` files,
    /// and orchestrator.go:511 resets that cache in each cycle). Before,
    /// the dropped parses stayed until `end_config_change_cycle`: on 600
    /// script files with 12 `moduleDetection` edits, `tsc -b -w` held 24%
    /// more RSS than R173, which dropped every kept parse before a config
    /// change build. A dropped version that no other holder keeps dies
    /// here, and its data is freed on the free thread, beside the build, as
    /// Go's GC frees the files of the old program
    /// (`file_version::take_data_of_dying_versions`).
    pub fn drop_kept_parses_whose_module_indicator_options_change(
        &self,
        changed: &[(Option<ModuleIndicatorInputs>, Rc<ParsedCommandLine>)],
    ) {
        let mut versions = Vec::new();
        self.watch_sources_before_config_change
            .borrow_mut()
            .retain(|key, source| {
                if !source.reads_options {
                    return true;
                }
                let mut held = false;
                for (inputs, config) in changed {
                    if !config.file_names_by_path().contains_key(&key.0.path) {
                        continue;
                    }
                    let options = inputs.and_then(|inputs| inputs.options_of(&key.0.file_name));
                    if options == Some(key.0.external_module_indicator_options) {
                        return true;
                    }
                    held = true;
                }
                if held {
                    versions.extend(source.file.version.get().cloned());
                }
                !held
            });
        let data = crate::ast::file_version::take_data_of_dying_versions(versions);
        if !data.is_empty() {
            crate::execute::build::build_task::drop_in_background(data);
        }
    }

    /// PORT: not in Go (`keep_watch_sources_for_config_change`). At the end
    /// of a cycle, a kept parse from before a config change that no build
    /// of the cycle took goes back to `watch_sources`, for a later build of
    /// its project, unless the cycle has a parse of the same path (with
    /// other parse options): that one replaced it, so it goes.
    pub fn end_config_change_cycle(&self) {
        let before = std::mem::take(&mut *self.watch_sources_before_config_change.borrow_mut());
        if before.is_empty() {
            return;
        }
        if let Some(sources) = self.watch_sources.borrow_mut().as_mut() {
            let parsed: FxHashSet<Path> = sources.keys().map(|key| key.0.path.clone()).collect();
            sources.extend(
                before
                    .into_iter()
                    .filter(|(key, _)| !parsed.contains(&key.0.path)),
            );
        }
    }

    // Go: build/host.go:72, the raw command line options of
    // `GetResolvedProjectReference`: wrapped in a "compilerOptions" key to
    // match the tsconfig.json structure.
    pub fn command_line_raw(&self) -> Option<IndexMap<String, CompilerOptionsValue>> {
        match &self.command.raw {
            CompilerOptionsValue::Map(raw) => {
                let mut wrapped = IndexMap::default();
                wrapped.insert(
                    "compilerOptions".to_string(),
                    CompilerOptionsValue::Map(raw.clone()),
                );
                Some(wrapped)
            }
            _ => None,
        }
    }

    // Go: orchestrator.go:97 (*Orchestrator).toPath, as the host reaches it (at
    // 673a5f17d713; removed by ts#64159: Go N' calls caseSensitivity.PathKey).
    pub fn to_path(&self, file_name: &str) -> Path {
        to_path(
            file_name,
            &self.compare_paths_options.current_directory,
            self.compare_paths_options.use_case_sensitive_file_names,
        )
    }

    // Go: build/host.go:90 (*host).GetMTime
    pub fn get_m_time(&self, file: &str) -> Option<SystemTime> {
        self.load_or_store_m_time(file, None, true)
    }

    /// PORT: not in Go (perf). `get_m_time` of `file`, whose `toPath` is
    /// `path`. `prefetched` is the mtime that a prefetch thread read for it
    /// (orchestrator.rs `BuildInfoPrefetch`), taken where this would read
    /// the file system.
    /// A task of this build wrote `file` after the prefetch read: then this
    /// reads the file system, as Go does (`written`).
    pub fn get_m_time_of_path(
        &self,
        file: &str,
        path: &Path,
        prefetched: Option<Option<SystemTime>>,
    ) -> Option<SystemTime> {
        let prefetched = prefetched.filter(|_| !self.was_written(path));
        self.load_or_store_m_time_of_path(file, path.clone(), None, true, prefetched)
    }

    /// PORT: not in Go (perf). True when a task of this build wrote or
    /// touched `path` (`written`).
    pub fn was_written(&self, path: &Path) -> bool {
        self.written.contains(path)
    }

    // Go: build/host.go:94 (*host).SetMTime
    // PORT: it also notes the file in `written`.
    pub fn set_m_time(&self, file: &str, m_time: Option<SystemTime>) -> Result<(), FsError> {
        self.written.insert(self.to_path(file));
        CompilerHost::fs(self).chtimes(file, None, m_time)
    }

    // Go: build/host.go:98 (*host).loadOrStoreMTime
    pub fn load_or_store_m_time(
        &self,
        file: &str,
        old_cache: Option<&FxHashMap<Path, Option<SystemTime>>>,
        store: bool,
    ) -> Option<SystemTime> {
        self.load_or_store_m_time_of_path(file, self.to_path(file), old_cache, store, None)
    }

    // Go: build/host.go:98 (*host).loadOrStoreMTime, with
    // `h.orchestrator.toPath(file)` computed by the caller, and the mtime
    // that a prefetch thread read (`prefetched`) in place of `GetMTime`.
    fn load_or_store_m_time_of_path(
        &self,
        file: &str,
        path: Path,
        old_cache: Option<&FxHashMap<Path, Option<SystemTime>>>,
        store: bool,
        prefetched: Option<Option<SystemTime>>,
    ) -> Option<SystemTime> {
        // PORT: Go `Load`, then `LoadOrStore` below. The lock is not held
        // while `get_m_time` reads the file system; else it is held from
        // the load to the store.
        let lock = || self.m_times.lock().unwrap_or_else(PoisonError::into_inner);
        let mut m_times = lock();
        if let Some(existing) = m_times.get(&path) {
            return *existing;
        }
        let mut found = false;
        let mut m_time = None;
        if let Some(old_cache) = old_cache {
            if let Some(old) = old_cache.get(&path) {
                m_time = *old;
                found = true;
            }
        }
        if !found {
            m_time = match prefetched {
                Some(m_time) => m_time,
                None => {
                    drop(m_times);
                    let m_time = incremental::get_m_time(&*self.host, file);
                    m_times = lock();
                    m_time
                }
            };
        }
        if store {
            m_time = *m_times.entry(path).or_insert(m_time);
        }
        m_time
    }

    // Go: build/host.go:117 (*host).storeMTime
    pub fn store_m_time(&self, file: &str, m_time: Option<SystemTime>) {
        let path = self.to_path(file);
        self.m_times
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(path, m_time);
    }

    // Go: build/host.go:122 (*host).storeMTimeFromOldCache
    pub fn store_m_time_from_old_cache(
        &self,
        file: &str,
        old_cache: &FxHashMap<Path, Option<SystemTime>>,
    ) {
        let path = self.to_path(file);
        if let Some(m_time) = old_cache.get(&path) {
            self.m_times
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(path, *m_time);
        }
    }

    // Go: build/host.go:87 (*host).ReadBuildInfo
    // PORT: Go reads the build info cache of the config's task
    // (`loadOrStoreBuildInfo`). Its only caller is `ReadBuildInfoProgram` in
    // `compileAndEmit`, with the config of the task that compiles, so
    // `BuildTask::compile_and_emit_start` reads its own cache (`build_info_program`)
    // and the host does not implement `incremental.BuildInfoReader`.
}

impl CompilerHost for BuildHost {
    // Go: build/host.go:38 (*host).FS
    fn fs(&self) -> Rc<dyn Fs> {
        self.host.fs()
    }

    // Go: build/host.go:42 (*host).DefaultLibraryPath
    fn default_library_path(&self) -> String {
        self.host.default_library_path()
    }

    // Go: build/host.go:46 (*host).GetCurrentDirectory (at 673a5f17d713; removed by ts#64159)
    fn get_current_directory(&self) -> String {
        self.host.get_current_directory()
    }

    // Go: build/host.go:46 (*host).Trace
    fn trace(&self, _msg: &'static Message, _args: Vec<String>) {
        panic!(
            "build.Orchestrator.host does not support tracing; use a different host for tracing"
        );
    }

    // PORT: not in Go (see `BuildHost::prefetch`).
    fn prefetch_parses(&self) -> bool {
        self.prefetch.get()
    }

    // Go: build/host.go:50 (*host).GetSourceFile
    fn get_source_file(&self, opts: &SourceFileParseOptions) -> Option<Rc<ParsedSourceFile>> {
        let watch = self.watch_sources.borrow().is_some();
        if is_declaration_file_name(&opts.file_name)
            || file_extension_is(&opts.file_name, EXTENSION_JSON)
        {
            // Cache dts and json files as they will be reused
            // PORT: a parse that the cache keeps can be left out of one
            // program (a deduplicated package, or a file that only such a
            // package imports) and be a program file of a later one. Go
            // keeps the whole `*ast.SourceFile`. The note makes the publish
            // of the first program give the store its complete Go file, so
            // the later program can use it.
            // PORT: in `tsc -b --watch` the parse comes from
            // `watch_source_file`, and this cache still gives the first
            // parse of the cycle to each later program of the cycle, as Go
            // does. A program that builds beside an upstream project (no
            // reference to it) can read its `.d.ts` before the upstream
            // build writes it; a downstream program of the same cycle then
            // gets that parse, not the new text (bwsig1: hono
            // `runtime-tests/*` build infos after a `removeComments` edit).
            return self.source_files.load_or_store(
                SourceFileCacheKey(opts.clone()),
                |key| {
                    if watch {
                        return self.watch_source_file(&key.0);
                    }
                    let file = self.host.get_source_file(&key.0);
                    if let Some(file) = &file {
                        crate::program::note_parsed_source_file(file);
                    }
                    file
                },
                false, /* allowZero */
            );
        }
        if watch {
            return self.watch_source_file(opts);
        }
        self.host.get_source_file(opts)
    }

    // Go: build/host.go:58 (*host).GetContentMappedSourceFiles (tsgo#4712)
    fn get_content_mapped_source_files(
        &self,
        _parse_options: &SourceFileParseOptions,
        _mapper: &Rc<Mapper>,
    ) -> Result<SourceFiles, GoError> {
        Err(contentmapper::ERR_PROJECT_UNAVAILABLE.clone())
    }

    // Go: build/host.go:62 (*host).ContentMapperProject (tsgo#4712)
    fn content_mapper_project(&self) -> Option<Rc<dyn Project>> {
        panic!(
            "build.Orchestrator.host does not support content mapper project; use an individual project's compiler host instead"
        );
    }

    // PORT: not in Go (see `CompilerHost::is_plain_os_fs`).
    fn is_plain_os_fs(&self) -> bool {
        self.host.is_plain_os_fs()
    }

    // PORT: not in Go (see `CompilerHost::stat_cache`).
    fn stat_cache(&self) -> Option<Arc<BuildStatCache>> {
        Some(self.cached_fs.stats.clone())
    }

    // PORT: not in Go (see `CompilerHost::cached_source_file_refs`). The
    // `.d.ts` and `.json` files that `get_source_file` keeps, and in
    // `tsc -b --watch` the parses that `watch_source_file` keeps, also from
    // before a config change. A kept parse is given only for the module
    // indicator options that its parse read, if any
    // (`FileRefs::of_kept_parse`; a `.d.ts` or `.json` parse reads none).
    // The references of each parse are made once (`cached_refs`).
    fn cached_source_file_refs(&self) -> FxHashMap<String, Arc<FileRefs>> {
        let mut refs = FxHashMap::default();
        let mut memo = self.cached_refs.borrow_mut();
        let mut add = |key: &SourceFileCacheKey, file: &Rc<ParsedSourceFile>| {
            let (parse, file_refs) = memo
                .entry(key.0.file_name.clone())
                .or_insert_with(|| (Rc::downgrade(file), Arc::new(FileRefs::of_kept_parse(file))));
            if !parse.ptr_eq(&Rc::downgrade(file)) {
                *parse = Rc::downgrade(file);
                *file_refs = Arc::new(FileRefs::of_kept_parse(file));
            }
            refs.insert(key.0.file_name.clone(), file_refs.clone());
        };
        self.source_files.for_each_stored(&mut add);
        if let Some(sources) = self.watch_sources.borrow().as_ref() {
            for (key, source) in sources {
                add(key, &source.file);
            }
        }
        for (key, source) in self.watch_sources_before_config_change.borrow().iter() {
            add(key, &source.file);
        }
        refs
    }

    // PORT: not in Go (see `CompilerHost::freeable_worker_parses`).
    fn freeable_worker_parses(&self) -> bool {
        self.watch_sources.borrow().is_some()
    }

    // Go: build/host.go:66 (*host).GetResolvedProjectReference
    fn get_resolved_project_reference(
        &self,
        file_name: &str,
        path: &Path,
    ) -> Option<Rc<ParsedCommandLine>> {
        self.resolved_references.load_or_store(
            path.clone(),
            |path| {
                let config_start = self.sys.now();
                // Wrap command line options in "compilerOptions" key to match tsconfig.json structure
                let command_line_raw = self.command_line_raw();
                let (command_line, _) = get_parsed_command_line_of_config_file_path(
                    file_name,
                    path.clone(),
                    Some(&self.command.compiler_options),
                    command_line_raw.as_ref(),
                    self,
                    Some(&self.extended_config_cache),
                );
                let config_time = self
                    .sys
                    .now()
                    .duration_since(config_start)
                    .unwrap_or_default();
                self.config_times
                    .borrow_mut()
                    .insert(path.clone(), config_time);
                command_line.map(Rc::new)
            },
            true, /* allowZero */
        )
    }
}

// PORT: Go passes the `*host` as a `tsoptions.ParseConfigHost` (it has
// `FS()` and `GetCurrentDirectory()`). Rust needs the explicit impl.
impl ParseConfigHost for BuildHost {
    fn fs(&self) -> Rc<dyn Fs> {
        self.host.fs()
    }

    fn get_current_directory(&self) -> String {
        self.host.get_current_directory()
    }

    // PORT: not in Go (perf). The file names that a prefetch thread matched
    // (config_prefetch.rs), else Go `getFileNamesFromConfigSpecs`.
    fn get_file_names_from_config_specs(
        &self,
        config_file_name: &str,
        config_file_specs: &ConfigFileSpecs,
        base_path: &str,
        options: Option<&CompilerOptions>,
        extra_extensions: &[String],
    ) -> (Vec<String>, i32) {
        let fs = ParseConfigHost::fs(self);
        match &*self.config_prefetch.borrow() {
            Some(prefetch) => prefetch.get_file_names_from_config_specs(
                config_file_name,
                config_file_specs,
                base_path,
                options,
                extra_extensions,
                &*fs,
                &self.cached_fs.stats,
            ),
            None => get_file_names_from_config_specs(
                config_file_specs,
                base_path,
                options,
                &*fs,
                extra_extensions,
            ),
        }
    }
}

// Go: build/host.go:35 `_ incremental.Host = (*host)(nil)`
impl incremental::Host for BuildHost {
    fn fs(&self) -> Rc<dyn Fs> {
        CompilerHost::fs(self)
    }

    fn get_m_time(&self, file_name: &str) -> Option<SystemTime> {
        BuildHost::get_m_time(self, file_name)
    }

    fn set_m_time(&self, file_name: &str, m_time: Option<SystemTime>) -> Result<(), FsError> {
        BuildHost::set_m_time(self, file_name, m_time)
    }
}

// Go: build/compilerHost.go:13 compilerHost
// PORT: the host that the build task gives `compiler.NewProgram`: the
// build host with the task's trace writer. Go nil `contentMapperProject`
// is `None`.
pub struct BuildCompilerHost {
    pub host: Rc<BuildHost>,
    pub trace: TraceFn,
    pub content_mapper_project: Option<Rc<dyn Project>>,
}

impl CompilerHost for BuildCompilerHost {
    // Go: build/compilerHost.go:21 (*compilerHost).FS
    fn fs(&self) -> Rc<dyn Fs> {
        CompilerHost::fs(&*self.host)
    }

    // Go: build/compilerHost.go:25 (*compilerHost).DefaultLibraryPath
    fn default_library_path(&self) -> String {
        self.host.default_library_path()
    }

    // Go: build/compilerHost.go:29 (*compilerHost).GetCurrentDirectory (at 673a5f17d713;
    // removed by ts#64159)
    fn get_current_directory(&self) -> String {
        CompilerHost::get_current_directory(&*self.host)
    }

    // Go: build/compilerHost.go:29 (*compilerHost).Trace
    fn trace(&self, msg: &'static Message, args: Vec<String>) {
        (self.trace)(msg, args);
    }

    // Go: build/compilerHost.go:33 (*compilerHost).GetSourceFile
    fn get_source_file(&self, opts: &SourceFileParseOptions) -> Option<Rc<ParsedSourceFile>> {
        self.host.get_source_file(opts)
    }

    // PORT: not in Go (see `BuildHost::prefetch`).
    fn prefetch_parses(&self) -> bool {
        self.host.prefetch_parses()
    }

    // Go: build/compilerHost.go:37 (*compilerHost).GetContentMappedSourceFiles (tsgo#4712)
    // PORT: Go returns `(files, err)`; a file that cannot be read is `Ok`
    // with no canonical file, as in the compiler host.
    fn get_content_mapped_source_files(
        &self,
        parse_options: &SourceFileParseOptions,
        mapper: &Rc<Mapper>,
    ) -> Result<SourceFiles, GoError> {
        let Some(project) = self.content_mapper_project() else {
            return Err(contentmapper::ERR_PROJECT_UNAVAILABLE.clone());
        };
        crate::frontend::compiler::content_mapped_source_files(
            &*CompilerHost::fs(self),
            &*project,
            parse_options,
            mapper,
        )
    }

    // PORT: not in Go (see `CompilerHost::prefetch_content_mapped`).
    fn prefetch_content_mapped(&self) -> bool {
        true
    }

    // Go: build/compilerHost.go:52 (*compilerHost).ContentMapperProject (tsgo#4712)
    fn content_mapper_project(&self) -> Option<Rc<dyn Project>> {
        self.content_mapper_project.clone()
    }

    // Go: build/compilerHost.go:56 (*compilerHost).GetResolvedProjectReference
    fn get_resolved_project_reference(
        &self,
        file_name: &str,
        path: &Path,
    ) -> Option<Rc<ParsedCommandLine>> {
        self.host.get_resolved_project_reference(file_name, path)
    }

    // PORT: not in Go (see `CompilerHost::is_plain_os_fs`).
    fn is_plain_os_fs(&self) -> bool {
        self.host.is_plain_os_fs()
    }

    // PORT: not in Go (see `CompilerHost::stat_cache`).
    fn stat_cache(&self) -> Option<Arc<BuildStatCache>> {
        self.host.stat_cache()
    }

    // PORT: not in Go (see `CompilerHost::cached_source_file_refs`).
    fn cached_source_file_refs(&self) -> FxHashMap<String, Arc<FileRefs>> {
        self.host.cached_source_file_refs()
    }

    // PORT: not in Go (see `CompilerHost::freeable_worker_parses`).
    fn freeable_worker_parses(&self) -> bool {
        self.host.freeable_worker_parses()
    }
}
