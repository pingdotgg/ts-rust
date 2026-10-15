//! PORT: not in Go. Multi-run wasm support (`ts_wasm`): one process serves
//! many tsc requests (`ts_run`) through the program-version machinery, the
//! way watch mode and the language server do. The first request loads a
//! program version (`Entry::Version`, not `core::set_prog`), and every
//! later request loads a new version that reuses the parses of unchanged
//! files: the parse cache below keys a parse by path and the hash of the
//! content the host file system serves, as the language server parse cache
//! keys an entry by content (`project::parsecache` `ParseCacheKey`). A
//! bundled lib file skips the hash: its text is embedded in the module and
//! cannot change within the process. The host needs no mtime contract — a
//! host file system that reports one mtime for every file (the npm
//! package's `memoryFileSystem`) is served correctly. The version that the
//! last run loaded stays loaded until the run after the next, so each
//! version is built from it (`new_program_version` `previous`), which
//! shares the unchanged frontend data (`GoSharedState::new`).
//!
//! The host enables this once (`enable`); `execute::execute_tsc`
//! `install_program` and `perform_compilation` then take the version paths
//! here. `end_run` releases the version of two runs ago (its checker pool
//! stops) and drops the parse cache entries of paths that left the program
//! (as the Go watcher drops the cache entries of files that left), so a
//! per-run path does not accumulate parses over the runs; the parse is
//! freed with the versions that held it. A parse whose path this process
//! never published is a freeable file version too (`note_published_path`
//! before the parse, `FileVersion` after), so the first parse of each new
//! path is freed with its holders as well and only the bookkeeping of the
//! live paths stays.

use std::cell::RefCell;
use std::rc::Rc;

use crate::contentmapper::{Mapper, Project, SourceFiles};
use crate::frontend::prelude::*;
use crate::gostd::GoError;
use xxhash_rust::xxh3::xxh3_128;

static ENABLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// True when the run that is ending installed a program version. A run can
/// end without one (an early return such as `--version`): its `end_run`
/// must leave `CURRENT` and `RETIRED` alone, or it would release the
/// version that `CURRENT` still names and store the released version in
/// `RETIRED` as the `previous` of the next version.
static INSTALLED_RUN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

thread_local! {
    /// The program version that the last run loaded. It is the current
    /// program of the thread (`core::set_thread_program`), as `set_prog`
    /// would make a one-program program, and the `previous` of the next
    /// version.
    static CURRENT: std::cell::Cell<Option<&'static crate::core::GoProgram>> =
        const { std::cell::Cell::new(None) };
    /// The version before `CURRENT`, kept loaded while the last run used it
    /// as its `previous`. `end_run` releases it.
    static RETIRED: std::cell::Cell<Option<&'static crate::core::GoProgram>> =
        const { std::cell::Cell::new(None) };
    /// The parses of the file versions that the loads of this process have
    /// seen, by path, with the content key the host file system served. A
    /// load that reads the same content takes the parse with no read of the
    /// parser (one read to hash) and no parse. Files change between runs
    /// when the host writes them; the changed content is a new key.
    static PARSE_CACHE: RefCell<FxHashMap<Path, CachedParse>> = RefCell::new(FxHashMap::default());
    /// The paths whose parses the program version of the last run keeps:
    /// the files of its program and its config file. `end_run` drops the
    /// other parse cache entries, so the parse of a path that left the
    /// program is freed with the versions that held it instead of staying
    /// for the life of the instance.
    static LIVE_PATHS: RefCell<FxHashSet<Path>> = RefCell::new(FxHashSet::default());
    /// The config file of the last installed version, by the host name the
    /// request named. Part of the file world (`file_world_fingerprint`):
    /// the config text is the file set of a `files` config, and the config
    /// parse does not go through the parse cache (the host reads it before
    /// the run installs a version).
    static CONFIG_STAMP: RefCell<Option<String>> = const { RefCell::new(None) };
}

struct CachedParse {
    /// True when the parse is of a bundled lib file: its text is embedded
    /// in the module, so the entry skips the content hash and matches by
    /// parse options alone.
    bundled: bool,
    /// The `xxh3`-128 of the text the host file system served for the
    /// parse. Part of the key with the path.
    content_hash: u128,
    file: Rc<ParsedSourceFile>,
}

/// Turns the multi-run flow on for the rest of the process, and turns on the
/// freeable file versions of a watch process (`ast::set_watch_process`), so
/// a re-parse of a changed file is freed with its last holder instead of
/// staying static (watcher.rs `note_kept_parse`, the 9 MiB per edit).
pub fn enable() {
    if ENABLED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    crate::ast::set_watch_process();
}

/// True when `enable` has run.
#[must_use]
pub fn is_enabled() -> bool {
    ENABLED.load(std::sync::atomic::Ordering::Relaxed)
}

/// `execute::execute_tsc::install_program` in the multi-run flow: loads the
/// program version of `host` and `config` (the tsc `ProgramOptions`) and
/// makes it the current program of the thread, in place of
/// `program::install_new_program` (`core::set_prog`), which a process can
/// call once. The caller runs the compile with no extra scope, like the
/// installed program.
pub(crate) fn install_program_version(host: Rc<dyn CompilerHost>, config: Rc<ParsedCommandLine>) {
    let config_path = config.config_file.as_ref().map(|c| c.path.clone());
    let config_file_name = config.config_file.as_ref().map(|c| c.file_name.clone());
    CONFIG_STAMP.with(|stamp| *stamp.borrow_mut() = config_file_name);
    let np = crate::execute::execute_tsc::new_frontend_program(host, config);
    crate::program::mark_freeable_parses(&np);
    // The paths whose parses this version keeps: the files of its program
    // and its config file. `end_run` drops the parse cache entries of the
    // other paths, so a path that left the program frees its parse with the
    // versions that held it.
    LIVE_PATHS.with(|live| {
        let mut live = live.borrow_mut();
        live.clear();
        live.extend(np.get_source_files().iter().map(|file| file.path().clone()));
        if let Some(config_path) = config_path {
            live.insert(config_path);
        }
    });
    let previous = CURRENT.with(|current| current.get());
    // The semantic-diagnostic reuse set of this run, from `np` and the cache
    // of the runs before (see `SemanticCache`). The options pointer is the
    // interned copy `build_program` leaks, so runs with the same options
    // share it.
    prepare_semantic_reuse(&np, super::intern_compiler_options(np.options()));
    let version = crate::program::new_program_version(&np, previous);
    drop(np);
    crate::core::set_thread_program(Some(version));
    CURRENT.with(|current| current.set(Some(version)));
    INSTALLED_RUN.store(true, std::sync::atomic::Ordering::Release);
}

/// The end of a run: releases the version that the run before the last
/// loaded, and moves the last version there. Its checker pool stops; the
/// published stores and the parse cache stay, so the next run reuses the
/// parses of its unchanged files. The parse cache entries of paths that
/// left the program are dropped, so their parses free with the versions
/// that held them. Releases nothing before the second run. A run that
/// installed no version (an early return such as `--version`) rotates
/// nothing: `CURRENT` and `RETIRED` keep their programs.
pub fn end_run() {
    if !INSTALLED_RUN.swap(false, std::sync::atomic::Ordering::AcqRel) {
        return;
    }
    let retired = RETIRED.with(|retired| retired.replace(None));
    if let Some(old) = retired {
        crate::program::release_program(old);
    }
    RETIRED.with(|retired| retired.set(CURRENT.with(|current| current.get())));
    LIVE_PATHS.with(|live| {
        let live = live.borrow();
        PARSE_CACHE.with(|cache| {
            cache.borrow_mut().retain(|path, _| live.contains(path));
        });
    });
}

/// The host of a run over the parse cache, in the multi-run flow. Every
/// method but `get_source_file` goes to the host of the run (a fresh
/// `CachedFs` compiler host, as a one-run process makes).
struct MultiRunHost {
    inner: Rc<dyn CompilerHost>,
}

impl MultiRunHost {
    /// The cache with a parse of `path` when its content key is the one the
    /// host file system serves now and the parse was made with the parse
    /// options of this run. A file whose content changed, one whose earlier
    /// parse was of other content, or one whose earlier parse used other
    /// compiler options (a later request can pass another tsc argument
    /// list, and the options choose `external_module_indicator_options`)
    /// is a miss. `None` (a failed read) is always a miss.
    fn cached(
        &self,
        path: &Path,
        key: Option<(bool, u128)>,
        opts: &SourceFileParseOptions,
    ) -> Option<Rc<ParsedSourceFile>> {
        let (bundled, content_hash) = key?;
        PARSE_CACHE.with(|cache| {
            let cache = cache.borrow();
            let cached = cache.get(path)?;
            (cached.bundled == bundled
                && cached.content_hash == content_hash
                && cached
                    .file
                    .parse_options()
                    .external_module_indicator_options
                    == opts.external_module_indicator_options)
                .then(|| cached.file.clone())
        })
    }
}

impl CompilerHost for MultiRunHost {
    fn fs(&self) -> Rc<dyn Fs> {
        self.inner.fs()
    }

    fn default_library_path(&self) -> String {
        self.inner.default_library_path()
    }

    fn get_current_directory(&self) -> String {
        self.inner.get_current_directory()
    }

    fn trace(&self, msg: &'static Message, args: Vec<String>) {
        self.inner.trace(msg, args);
    }

    // The language server parse cache's test (`project::parsecache`): the
    // content the host serves, hashed, then the parse of the cache when the
    // hash matches, else the parse of the run's host, cached with the new
    // hash. A bundled lib file skips the hash (its text is embedded in the
    // module); a failed read is a miss with no insert, as a failed stat
    // was in the watch host.
    fn get_source_file(&self, opts: &SourceFileParseOptions) -> Option<Rc<ParsedSourceFile>> {
        let bundled = crate::frontend::bundled::bundled_text(&opts.file_name).is_some();
        let key = if bundled {
            Some((true, 0))
        } else {
            let (text, ok) = self.inner.fs().read_file(&opts.file_name);
            ok.then(|| (false, xxh3_128(text.as_bytes())))
        };
        if let Some(file) = self.cached(&opts.path, key, opts) {
            return Some(file);
        }
        // A new parse of a path this process never published is freeable
        // too: noting the path published turns `freeable_path` on for the
        // parse, so its store owns its nodes, and the `FileVersion` below
        // (as `execute::watcher` `note_kept_parse`) frees it with its last
        // holder — the first parse of each new per-run path does not stay
        // in the leaked arena for the life of the instance. Program files
        // get their version from `mark_freeable_parses` at install; this
        // covers a parse the program did not take.
        crate::ast::note_published_path(&opts.path.0);
        let file = self.inner.get_source_file(opts)?;
        if file.version.get().is_none()
            && !crate::ast::is_published(file.store)
            && crate::ast::freeable_path(&file.path().0)
        {
            let _ = file.version.set(crate::ast::FileVersion::new(file.store));
        }
        if let Some((bundled, content_hash)) = key {
            PARSE_CACHE.with(|cache| {
                cache.borrow_mut().insert(
                    opts.path.clone(),
                    CachedParse {
                        bundled,
                        content_hash,
                        file: file.clone(),
                    },
                );
            });
        }
        Some(file)
    }

    fn get_content_mapped_source_files(
        &self,
        parse_options: &SourceFileParseOptions,
        mapper: &Rc<Mapper>,
    ) -> Result<SourceFiles, GoError> {
        self.inner
            .get_content_mapped_source_files(parse_options, mapper)
    }

    fn content_mapper_project(&self) -> Option<Rc<dyn Project>> {
        self.inner.content_mapper_project()
    }

    fn get_resolved_project_reference(
        &self,
        file_name: &str,
        path: &Path,
    ) -> Option<Rc<ParsedCommandLine>> {
        self.inner.get_resolved_project_reference(file_name, path)
    }

    // The watch host's test: a load with parses in the cache starts no
    // workers (they would parse the cached files again for nothing).
    fn prefetch_parses(&self) -> bool {
        PARSE_CACHE.with(|cache| cache.borrow().is_empty()) && self.inner.prefetch_parses()
    }

    // The watch hosts return true (see the trait): a worker parse of a path
    // that this process published before is a freeable file version.
    fn freeable_worker_parses(&self) -> bool {
        true
    }
}

/// The host file system as the parse cache sees it, hashed: every path the
/// cache has parsed, with the mtime the host reports now. A replayed
/// request (`ts_wasm` `FINGERPRINT`) is the same check only while this
/// value is the one stored with its reply: a changed or deleted file
/// changes a stat, and a new program file changes the stat of the config
/// that names it (the held flow's config text is a function of its file
/// set). This is the parse cache's own mtime-honesty contract.
/// The host file system as the parse cache sees it, hashed: every path the
/// cache has parsed, with the hash of the content the file system serves
/// now, and the config file of the last run (`CONFIG_STAMP`). A bundled
/// lib file hashes its path only: its text is embedded in the module and
/// cannot change within the process. A replayed request (`ts_wasm`
/// `FINGERPRINT`) is the same check only while this value is the one
/// stored with its reply: a changed or deleted file changes a read, and a
/// changed config changes the read of the file set it names (a new program
/// file of a `files` config changes the config text). This is the parse
/// cache's own content test (`cached`), so a fingerprint hit means the run
/// would serve the same parses.
pub fn file_world_fingerprint(fs: &dyn Fs) -> u64 {
    use std::hash::{Hash, Hasher};
    // The reads run with no cache borrow held: a host read can re-enter
    // (`HostFs` crosses the wasm boundary).
    let entries: Vec<(Path, String, bool)> = PARSE_CACHE.with(|cache| {
        let mut entries: Vec<(Path, String, bool)> = cache
            .borrow()
            .iter()
            .map(|(path, cached)| {
                (
                    path.clone(),
                    cached.file.parse_options().file_name.clone(),
                    cached.bundled,
                )
            })
            .collect();
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        entries
    });
    let config_file_name = CONFIG_STAMP.with(|stamp| stamp.borrow().clone());
    let mut hasher = std::hash::DefaultHasher::new();
    for (path, file_name, is_bundled) in &entries {
        path.0.hash(&mut hasher);
        if *is_bundled {
            continue;
        }
        let (text, ok) = fs.read_file(file_name);
        ok.hash(&mut hasher);
        text.hash(&mut hasher);
    }
    if let Some(file_name) = config_file_name {
        hasher.write(b"config");
        file_name.hash(&mut hasher);
        let (text, ok) = fs.read_file(&file_name);
        ok.hash(&mut hasher);
        text.hash(&mut hasher);
    }
    hasher.finish()
}

/// The compiler host of a run in the multi-run flow: the run's host over the
/// process parse cache. With the flow off, the host unchanged.
pub(crate) fn wrap_host(host: Rc<dyn CompilerHost>) -> Rc<dyn CompilerHost> {
    if !is_enabled() {
        return host;
    }
    Rc::new(MultiRunHost { inner: host })
}

// ---- semantic-diagnostic reuse (phase 9) ----
//
// The check phase of a warm run is the whole floor (the decomposition that
// drove this: `C_install_program` 1-3 ms, `D_check_emit` 48-54 ms of a
// 55-70 ms run, ~47-50 ms of it `get_semantic_diagnostics` over all files —
// mostly the 57 lib .d.ts files). The parse cache already makes the parses
// of unchanged files shared `Rc`s, so a file whose parse is the same `Rc`
// as the run before was checked under the same text — and when nothing it
// reads changed, its semantic diagnostics are the same list. This cache
// hands those lists back instead of re-checking the files, so a warm run
// checks only what a change can reach.
//
// Safety rules (a reused list must be the list a full check would compute):
//
// 1. Same program shape: the same path set as the cached run, and the same
//    interned compiler options (a pointer compare). Else everything is
//    re-checked.
// 2. Changed files (a new parse store for their path) must be external
//    modules with no `declare global`/ambient modules and no
//    augmentations: their declarations cannot reach the global scope that
//    every file shares. A changed script can (a `const console` in a guest
//    script reports TS2451 on guest-globals.d.ts too — measured), so a
//    changed script re-checks everything.
// 3. A file is reusable only when it imports no changed file, not even
//    transitively (the resolved module and type-reference edges of the
//    program) and carries no triple-slash references (those resolve
//    outside `resolved_modules`).
// 4. `GOPORT_SEMANTIC_CACHE=0` turns the reuse off (a kill switch for a
//    diagnosis).
//
// The cache holds no program, no tables and no checker: `Rc` diagnostics
// keyed by path, so `release_program` lifetime rules are untouched.

struct CachedDiags {
    store: usize,
    diags: Rc<Vec<crate::core::Diagnostic>>,
}

thread_local! {
    /// The per-path semantic diagnostics of the last run that computed
    /// them, with the parse store they belong to.
    static SEMANTIC_CACHE: RefCell<SemanticCache> = RefCell::new(SemanticCache {
        options: None,
        by_path: FxHashMap::default(),
        contributions: FxHashMap::default(),
    });
    /// Store id -> cached diagnostics that the check of the run that is
    /// running now returns instead of re-checking. Cleared at every
    /// `prepare_semantic_reuse`.
    static SEMANTIC_REUSE: RefCell<FxHashMap<usize, Rc<Vec<crate::core::Diagnostic>>>> =
        RefCell::new(FxHashMap::default());
    /// Store id -> path of every file of the run that is running now, for
    /// `note_semantic_diagnostics` (the check reads nodes, not parses).
    static NOTE_PATHS: RefCell<FxHashMap<usize, crate::frontend::tspath::Path>> =
        RefCell::new(FxHashMap::default());
}

struct SemanticCache {
    /// The interned options the cache was built under.
    options: Option<&'static crate::options::CompilerOptions>,
    by_path: FxHashMap<crate::frontend::tspath::Path, CachedDiags>,
    /// The global-scope contribution hash of each cached global-scope file
    /// (`global_contribution_hash`), from the run that cached its
    /// diagnostics. Cleared with `by_path`.
    contributions: FxHashMap<crate::frontend::tspath::Path, u64>,
}

/// True unless `GOPORT_SEMANTIC_CACHE=0`. Read once.
fn semantic_cache_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("GOPORT_SEMANTIC_CACHE").map_or(true, |v| v != "0"))
}

/// A file whose change can reach the global scope every file shares: a
/// script, or a module that augments or declares ambient modules.
fn reaches_global_scope(file: &ParsedSourceFile) -> bool {
    file.external_module_indicator.is_nil()
        || !file.module_augmentations.is_empty()
        || !file.ambient_module_names.is_empty()
}

/// What a script file contributes to the global scope, hashed: every
/// top-level statement's kind and source text — except a function
/// declaration with an explicit return type annotation, which contributes
/// its signature only (the text up to its body): a body-only edit cannot
/// change the type other files see. A changed script whose hash is the one
/// the cache saw (the common agent edit: a new body for the same signed
/// function) cannot contaminate any other file, so the run re-checks just
/// the script.
fn global_contribution_hash(file: &ParsedSourceFile) -> u64 {
    use std::hash::{Hash, Hasher};
    let full: &str = match &file.text {
        crate::ast::FileText::Static(text) => text,
        crate::ast::FileText::Shared(text) => text,
    };
    let mut hasher = std::hash::DefaultHasher::new();
    for stmt in file.root.iter_children() {
        stmt.kind().hash(&mut hasher);
        let end = if stmt.kind() == crate::astdata::SyntaxKind::FunctionDeclaration
            && !stmt.body().is_nil()
            && !stmt.type_().is_nil()
        {
            stmt.body().pos()
        } else {
            stmt.end()
        };
        let start = usize::try_from(stmt.pos()).unwrap_or(0);
        let end = usize::try_from(end).unwrap_or(full.len()).min(full.len());
        hasher.write(full.get(start..end).unwrap_or_default().as_bytes());
    }
    hasher.finish()
}

/// The program files that (transitively) import a changed file: the reverse
/// of the program's resolved module and type-reference edges, walked from
/// the changed paths.
fn influenced_by(
    np: &crate::frontend::compiler::NewProgram,
    changed: &FxHashMap<crate::frontend::tspath::Path, usize>,
) -> FxHashSet<crate::frontend::tspath::Path> {
    let mut reverse: FxHashMap<
        &crate::frontend::tspath::Path,
        Vec<&crate::frontend::tspath::Path>,
    > = FxHashMap::default();
    for (importer, cache) in np.processed_files.resolved_modules.iter() {
        for resolution in cache.values() {
            if resolution.is_resolved() {
                reverse
                    .entry(&resolution.resolved_path)
                    .or_default()
                    .push(importer);
            }
        }
    }
    for (importer, cache) in np.processed_files.type_resolutions_in_file.iter() {
        for resolution in cache.values() {
            if resolution.is_resolved() {
                reverse
                    .entry(&resolution.resolved_path)
                    .or_default()
                    .push(importer);
            }
        }
    }
    let mut influenced: FxHashSet<&crate::frontend::tspath::Path> = changed.keys().collect();
    let mut queue: Vec<&crate::frontend::tspath::Path> = influenced.iter().copied().collect();
    while let Some(path) = queue.pop() {
        if let Some(importers) = reverse.get(path) {
            for importer in importers {
                if influenced.insert(importer) {
                    queue.push(importer);
                }
            }
        }
    }
    influenced.into_iter().cloned().collect()
}

/// Computes the reuse set of the run that is loading now, from `np` and the
/// cache of the runs before. Called on the loading thread before
/// `new_program_version`; `options` is the interned copy of the run's
/// compiler options.
fn prepare_semantic_reuse(
    np: &crate::frontend::compiler::NewProgram,
    options: &'static crate::options::CompilerOptions,
) {
    SEMANTIC_REUSE.with(|reuse| reuse.borrow_mut().clear());
    let files = np.source_files();
    NOTE_PATHS.with(|note| {
        let mut note = note.borrow_mut();
        note.clear();
        note.extend(files.iter().map(|f| (f.store, f.path().clone())));
    });
    if !semantic_cache_enabled() {
        return;
    }
    SEMANTIC_CACHE.with(|cache_cell| {
        SEMANTIC_REUSE.with(|reuse_cell| {
            let mut cache = cache_cell.borrow_mut();
            let mut reuse = reuse_cell.borrow_mut();
            // Rule 1: the options changed since the cache was built.
            if !cache
                .options
                .is_some_and(|cached| std::ptr::eq(cached, options))
            {
                cache.options = Some(options);
                cache.by_path.clear();
            }
            // Rule 1: the path set changed.
            let same_set = cache.by_path.len() == files.len()
                && files.iter().all(|f| cache.by_path.contains_key(f.path()));
            if !same_set {
                cache.by_path.clear();
                return;
            }
            // The files whose parse changed (or are new to the cache, which
            // the same-set test rules out).
            let changed: FxHashMap<crate::frontend::tspath::Path, usize> = files
                .iter()
                .filter(|f| {
                    cache
                        .by_path
                        .get(f.path())
                        .is_none_or(|c| c.store != f.store)
                })
                .map(|f| (f.path().clone(), f.store))
                .collect();
            if changed.is_empty() {
                // Every parse is the run before's: every cached list stands.
                for file in files {
                    let cached = &cache.by_path[file.path()];
                    reuse.insert(file.store, Rc::clone(&cached.diags));
                }
                return;
            }
            // Rule 2: a changed file that can reach the global scope keeps
            // the rest reusable only when its contribution is the one the
            // cache saw (a body-only edit of an annotated function). A new
            // top-level name or signature re-checks everything.
            let mut global_change = false;
            for file in files {
                if !changed.contains_key(file.path()) || !reaches_global_scope(file) {
                    continue;
                }
                let hash = global_contribution_hash(file);
                global_change |= cache.contributions.get(file.path()) != Some(&hash);
                cache.contributions.insert(file.path().clone(), hash);
            }
            if global_change {
                // The stored contribution stays: it is the new parse's, and
                // the next change of this path compares against it.
                cache.by_path.clear();
                return;
            }
            // Rule 3: keep the files outside the influence of the changed
            // ones that carry no triple-slash references.
            let influenced = influenced_by(np, &changed);
            for file in files {
                if changed.contains_key(file.path()) || influenced.contains(file.path()) {
                    continue;
                }
                if !file.referenced_files.is_empty() || !file.type_reference_directives.is_empty() {
                    continue;
                }
                let cached = &cache.by_path[file.path()];
                reuse.insert(file.store, Rc::clone(&cached.diags));
            }
        });
    });
}
/// The path of a published source-file node of the run that is checking
/// now (`NOTE_PATHS`, from the same load).
fn noted_path(store: usize) -> Option<crate::frontend::tspath::Path> {
    NOTE_PATHS.with(|note| note.borrow().get(&store).cloned())
}

/// `program::get_semantic_diagnostics(Node::NIL)` of a multi-run process:
/// re-checks only the files outside `SEMANTIC_REUSE`, splices the cached
/// lists of the rest in file order, notes what it computed, and sorts once
/// at the end — the same calls, order and result as the full fan-out, with
/// the reused files skipped.
pub(crate) fn semantic_diagnostics_with_reuse() -> Vec<crate::core::Diagnostic> {
    let files = super::source_files();
    let reuse = SEMANTIC_REUSE.with(|reuse| std::mem::take(&mut *reuse.borrow_mut()));
    enum Slot {
        Cached(Rc<Vec<crate::core::Diagnostic>>),
        Check(usize),
    }
    let mut slots: Vec<Slot> = Vec::with_capacity(files.len());
    let mut to_check: Vec<crate::core::Node> = Vec::with_capacity(files.len());
    for file in &files {
        match reuse.get(&file.file_index()) {
            Some(diags) => slots.push(Slot::Cached(Rc::clone(diags))),
            None => {
                slots.push(Slot::Check(to_check.len()));
                to_check.push(*file);
            }
        }
    }
    let mut checked = if to_check.is_empty() {
        Vec::new()
    } else {
        super::collect_checker_diagnostics_from_files(&to_check, |c, f| {
            super::get_semantic_diagnostics_with_checker(&crate::gostd::context::background(), c, f)
        })
    };
    let mut fresh: Vec<(
        crate::frontend::tspath::Path,
        usize,
        Rc<Vec<crate::core::Diagnostic>>,
    )> = Vec::with_capacity(checked.len());
    let mut out = Vec::new();
    for (i, file) in files.iter().enumerate() {
        match &slots[i] {
            Slot::Cached(diags) => out.extend(diags.iter().cloned()),
            Slot::Check(k) => {
                let diags = std::mem::take(&mut checked[*k]);
                let diags = Rc::new(diags);
                if let Some(path) = noted_path(file.file_index()) {
                    fresh.push((path, file.file_index(), Rc::clone(&diags)));
                }
                out.extend(diags.iter().cloned());
            }
        }
    }
    if semantic_cache_enabled() {
        SEMANTIC_CACHE.with(|cache| {
            let mut cache = cache.borrow_mut();
            for (path, store, diags) in fresh {
                cache.by_path.insert(path, CachedDiags { store, diags });
            }
        });
    }
    super::filter_and_sort_diagnostics(out)
}
