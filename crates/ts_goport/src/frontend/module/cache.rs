//! Port of module/cache.go.

use crate::frontend::prelude::*;
use std::cell::Cell;
use std::sync::Arc;

// Go: module/cache.go:10 ModeAwareCache
pub type ModeAwareCache<T> = FxHashMap<ModeAwareCacheKey, T>;

// Go: module/cache.go:12 moduleResolutionCacheKey
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModuleResolutionCacheKey {
    pub containing_directory: String,
    pub module_name: String,
    pub resolution_mode: ResolutionMode,
    pub redirect_config_name: String,
}

/// The fields of a `ModuleResolutionCacheKey`: containing directory,
/// module name, resolution mode and redirect config name.
pub type ModuleKeyParts<'a> = (&'a str, &'a str, ResolutionMode, &'a str);

/// A module resolution cache key as its parts, so a lookup needs no new
/// key (`ModuleResolutionCache::get`).
// PORT: not in Go (perf). A Go key is a struct of string headers, which
// costs no allocation; a Rust key owns its strings.
pub trait ModuleKey {
    fn parts(&self) -> ModuleKeyParts<'_>;
}

impl ModuleKey for ModuleResolutionCacheKey {
    fn parts(&self) -> ModuleKeyParts<'_> {
        (
            &self.containing_directory,
            &self.module_name,
            self.resolution_mode,
            &self.redirect_config_name,
        )
    }
}

impl ModuleKey for ModuleKeyParts<'_> {
    fn parts(&self) -> ModuleKeyParts<'_> {
        *self
    }
}

impl<'a> std::borrow::Borrow<dyn ModuleKey + 'a> for ModuleResolutionCacheKey {
    fn borrow(&self) -> &(dyn ModuleKey + 'a) {
        self
    }
}

// The same hash as a derived one (the fields in order), so the map order
// does not change.
impl std::hash::Hash for ModuleResolutionCacheKey {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.parts().hash(state);
    }
}

impl std::hash::Hash for dyn ModuleKey + '_ {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.parts().hash(state);
    }
}

impl PartialEq for dyn ModuleKey + '_ {
    fn eq(&self, other: &Self) -> bool {
        self.parts() == other.parts()
    }
}

impl Eq for dyn ModuleKey + '_ {}

impl ModuleResolutionCacheKey {
    /// The key of `parts`.
    #[must_use]
    pub fn from_parts(parts: ModuleKeyParts<'_>) -> Self {
        let (containing_directory, module_name, resolution_mode, redirect_config_name) = parts;
        ModuleResolutionCacheKey {
            containing_directory: containing_directory.to_string(),
            module_name: module_name.to_string(),
            resolution_mode,
            redirect_config_name: redirect_config_name.to_string(),
        }
    }
}

// Go: module/cache.go:19 moduleResolutionCache
// PORT: Go `collections.SyncMap` is a plain map behind a `RefCell`, so the
// `DefaultResolver` methods can take `&self`. Cached Go pointers are `Arc`: parse
// workers share them (`SharedResolutionCache`), and checker threads read
// the program's resolutions with no copy (`GoSharedState`).
#[derive(Default)]
pub struct ModuleResolutionCache {
    pub cache: RefCell<FxHashMap<ModuleResolutionCacheKey, Arc<ResolvedModule>>>,
}

impl ModuleResolutionCache {
    // Go: module/cache.go:23 moduleResolutionCache.Get
    #[must_use]
    pub fn get(&self, key: &dyn ModuleKey) -> Option<Arc<ResolvedModule>> {
        self.cache.borrow().get(key).cloned()
    }

    // Go: module/cache.go:27 moduleResolutionCache.Set
    // PORT: Go `LoadOrStore`: the first stored value wins.
    pub fn set(&self, key: ModuleResolutionCacheKey, value: Arc<ResolvedModule>) {
        self.cache.borrow_mut().entry(key).or_insert(value);
    }
}

// Go: module/cache.go:31 typeRefDirectiveResolutionCacheKey
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct TypeRefDirectiveResolutionCacheKey {
    pub containing_directory: String,
    pub type_reference_name: String,
    pub resolution_mode: ResolutionMode,
    pub redirect_config_name: String,
    pub from_inferred_types_containing_file: bool,
}

// Go: module/cache.go:39 typeRefDirectiveResolutionCache
// PORT: see `ModuleResolutionCache`.
#[derive(Default)]
pub struct TypeRefDirectiveResolutionCache {
    pub cache:
        RefCell<FxHashMap<TypeRefDirectiveResolutionCacheKey, Rc<ResolvedTypeReferenceDirective>>>,
}

impl TypeRefDirectiveResolutionCache {
    // Go: module/cache.go:43 typeRefDirectiveResolutionCache.Get
    #[must_use]
    pub fn get(
        &self,
        key: &TypeRefDirectiveResolutionCacheKey,
    ) -> Option<Rc<ResolvedTypeReferenceDirective>> {
        self.cache.borrow().get(key).cloned()
    }

    // Go: module/cache.go:47 typeRefDirectiveResolutionCache.Set
    // PORT: Go `Store`: the last stored value wins.
    pub fn set(
        &self,
        key: TypeRefDirectiveResolutionCacheKey,
        value: Rc<ResolvedTypeReferenceDirective>,
    ) {
        self.cache.borrow_mut().insert(key, value);
    }
}

// Go: module/cache.go:51 parsedPatternsCache
// PORT: Go keys the `SyncMap` by the `*OrderedMap` of `paths`, and the key
// keeps that map alive. The Rust `paths` map is a field of the
// `CompilerOptions`, so the key is the address of that field (0 for a nil
// map), and the entry holds the options `Rc` that owns it.
#[derive(Default)]
pub struct ParsedPatternsCache {
    pub cache: RefCell<FxHashMap<usize, (Rc<CompilerOptions>, Rc<ParsedPatterns>)>>,
}

impl ParsedPatternsCache {
    // Go: module/cache.go:55 parsedPatternsCache.Get
    // PORT: Go takes `compilerOptions.Paths`; this takes the options that
    // own it (see the type).
    pub fn get(&self, compiler_options: &Rc<CompilerOptions>) -> Rc<ParsedPatterns> {
        let path_mappings = compiler_options.paths.as_ref();
        let key = path_mappings.map_or(0, |path_mappings| {
            std::ptr::from_ref(path_mappings) as usize
        });
        if let Some((_, patterns)) = self.cache.borrow().get(&key) {
            return patterns.clone();
        }
        let patterns = Rc::new(try_parse_patterns(path_mappings));
        self.cache
            .borrow_mut()
            .entry(key)
            .or_insert_with(|| (compiler_options.clone(), patterns))
            .1
            .clone()
    }

    /// Drops the entries (`Caches::release`).
    // PORT: not in Go.
    fn clear(&self) {
        let entries = std::mem::take(&mut *self.cache.borrow_mut());
        drop(entries);
    }
}

// Go: module/cache.go:63 ResolutionData (ts#64519)
/// The part of a resolver that a program keeps and that later resolvers
/// share (`new_resolver`): the options and the package.json cache. It holds
/// no host and no resolution caches.
// PORT: Go `*packagejson.InfoCache` is shared between resolvers
// (`ResolverOptions.PackageJsonCache`), so it is `Rc<InfoCache>`. `InfoCache`
// has interior mutability, like the Go `SyncMap`. Go shares the
// `*ResolutionData`; here it is `Rc<ResolutionData>`.
pub struct ResolutionData {
    pub compiler_options: Rc<CompilerOptions>,
    pub typings_location: String,
    pub project_name: String,
    // tsgo#4712: the content mapper extensions.
    pub extra_extensions: Vec<String>,

    pub package_json_info_cache: Rc<InfoCache>,
}

// Go: module/cache.go:72 newResolutionData (ts#64519)
// PORT: Go keeps a nil `CompilerOptions` and panics at its first use; this
// panics now (as `new_resolver` did).
pub(crate) fn new_resolution_data(opts: &ResolverOptions) -> ResolutionData {
    let host = opts.host.as_ref().expect("module.NewResolver: nil Host");
    ResolutionData {
        compiler_options: opts
            .compiler_options
            .clone()
            .expect("module.NewResolver: nil CompilerOptions"),
        typings_location: opts.typings_location.clone(),
        project_name: opts.project_name.clone(),
        extra_extensions: opts.extra_extensions.clone(),
        package_json_info_cache: opts.package_json_cache.clone().unwrap_or_else(|| {
            Rc::new(new_info_cache(
                host.get_current_directory(),
                host.fs().use_case_sensitive_file_names(),
            ))
        }),
    }
}

// Go: module/cache.go:86 (*ResolutionData).Clone (ts#64519)
/// The same data with a copy of the package.json cache table
/// (`InfoCache::clone`).
impl Clone for ResolutionData {
    fn clone(&self) -> Self {
        ResolutionData {
            compiler_options: self.compiler_options.clone(),
            typings_location: self.typings_location.clone(),
            project_name: self.project_name.clone(),
            extra_extensions: self.extra_extensions.clone(),
            package_json_info_cache: Rc::new(self.package_json_info_cache.as_ref().clone()),
        }
    }
}

impl ResolutionData {
    // Go: module/cache.go:97 (*ResolutionData).PackageJsonCacheEntries (ts#64519)
    // PORT: the loader's resolver also lists the package.json lookups of the
    // parse worker answers that it took
    // (`DefaultResolver::package_json_cache_entries`). They are in its
    // `Caches`, not here.
    pub fn package_json_cache_entries(
        &self,
        mut f: impl FnMut(&Path, PackageJsonCacheEntry<'_>) -> bool,
    ) {
        self.package_json_info_cache.range(|key, entry| {
            f(
                key,
                PackageJsonCacheEntry {
                    package_directory: &entry.package_directory,
                    directory_exists: entry.directory_exists,
                    exists: entry.exists(),
                },
            )
        });
    }
}

// Go: module/resolver.go:335 DefaultResolver (the caches; ts#64519 moves
// them there from module/cache.go:62 caches at 673a5f17d713)
// PORT: the Go fields `moduleResolutionCache`, `typeRefDirectiveResolutionCache`
// and `parsedPatternsForPaths` of `DefaultResolver`, with the Rust-only
// caches of the parallel load. Each resolver has its own; a resolver that
// `ResolutionData::new_resolver` makes starts empty.
#[derive(Default)]
pub struct Caches {
    pub module_resolution_cache: ModuleResolutionCache,
    pub type_ref_directive_resolution_cache: TypeRefDirectiveResolutionCache,

    // Cached representations for `core.CompilerOptions.paths`, keyed by the
    // path mappings themselves. This does not handle other path patterns such
    // as `typesVersions`.
    pub parsed_patterns_for_paths: ParsedPatternsCache,

    /// The resolution caches that this resolver shares with the other
    /// resolvers of one program load (see `SharedResolutionCache`). `None`
    /// for a resolver that shares nothing.
    pub shared: Option<SharedResolutionLink>,

    /// A parse worker's resolver: the package.json lookups of the
    /// resolution that it runs now, published with its answer.
    pub package_json_log: RefCell<Vec<PackageJsonLookup>>,

    /// The loader's resolver: the package.json lookups of the worker
    /// answers that it took from `shared`. A read of all the package.json
    /// cache entries lists them too
    /// (`DefaultResolver::package_json_cache_entries`).
    pub worker_package_jsons: RefCell<Vec<Arc<[PackageJsonLookup]>>>,

    /// A parse worker's resolver (files_parser.rs `WorkerResolver`): the
    /// reads of its package.json cache entries (`WorkerPackageJsonReads`).
    /// `None` for other resolvers.
    pub worker_package_json_reads: Option<WorkerPackageJsonReads>,

    /// The loader's resolver in `tsc -b`: the file system lookups of the
    /// worker answers that it took from `shared` (`SharedResolution::lookups`).
    /// The load adds them to the build host's cache at its end
    /// (`DefaultResolver::take_worker_lookups`, `BuildStatCache::end_load`).
    pub worker_lookups: RefCell<Vec<Arc<[StatLookup]>>>,

    /// The loader's resolver during a resolve-ahead program load
    /// (compiler/resolve_ahead.rs). The load removes it at its end.
    pub ahead: RefCell<Option<AheadLink>>,
}

/// The loader's resolver in a resolve-ahead program load
/// (compiler/resolve_ahead.rs): the answers of the workers, the check before
/// the loader takes one, and the keys of the load.
// PORT: not in Go (perf).
pub struct AheadLink {
    /// The answers of the resolve-ahead workers of this load. `None` when
    /// no worker runs; the load then only records its keys.
    pub answers: Option<Arc<SharedResolutionCache>>,
    /// Checks the calls of a worker answer for a key on the loader's file
    /// system and replays their side effects. False: a call gives another
    /// answer there, and the loader resolves the key itself.
    pub accept: AheadAccept,
    /// The keys that the loader resolved or took in this load, in its
    /// order: the keys for the workers of the next load.
    pub keys: RefCell<KeyList>,
    /// The workers' queue of the previous load's keys. `None` when no
    /// worker runs.
    pub queue: Option<Arc<AheadQueue>>,
    /// The index in the queue after the last key of the loader that was
    /// found there (`AheadQueue::find`).
    pub cursor: Cell<usize>,
    pub stats: Cell<AheadStats>,
    /// The package scope of each directory that the load asked for, by
    /// directory (`Caches::package_scope_ahead`).
    pub scopes: RefCell<FxHashMap<String, Option<PackageScope>>>,
}

/// The keys of the previous load, which the resolve-ahead workers resolve
/// in order, and who resolves each one: the loader takes a key that no
/// worker has started, so no key is resolved twice, and it waits for a key
/// that a worker resolves now.
// PORT: not in Go (perf).
pub struct AheadQueue {
    pub keys: Arc<KeyList>,
    /// The index of the next key for a worker.
    pub next: std::sync::atomic::AtomicUsize,
    /// Per key: `KEY_FREE`, `KEY_WORKER`, `KEY_DONE` or `KEY_LOADER`.
    states: Box<[std::sync::atomic::AtomicU8]>,
    /// The key that the loader waits for on `done_cv`, or `usize::MAX`.
    waiting: std::sync::atomic::AtomicUsize,
    done_lock: std::sync::Mutex<()>,
    done_cv: std::sync::Condvar,
    /// A worker panicked outside a resolution (`fail`).
    failed: std::sync::atomic::AtomicBool,
}

const KEY_FREE: u8 = 0;
/// A worker resolves the key now.
const KEY_WORKER: u8 = 1;
/// A worker ended the key: its answer is published, unless the
/// resolution could not be shared or panicked.
const KEY_DONE: u8 = 2;
/// The loader resolves the key itself.
const KEY_LOADER: u8 = 3;

/// How many times the loader checks a key that a worker resolves before
/// it sleeps (`AheadQueue::wait_or_take`): about 2 to 4 microseconds.
///
/// A longer spin does not make the waits shorter. On import edits (effect
/// and query-core, two hosts, 200 loads each) the loader waited 14 to 28
/// times per load, 30 to 40 microseconds each, 0.4 to 1.0 ms per load in
/// all. A spin of up to 50 or 200 microseconds took the sleeps away but
/// not the wait time: the loader has caught up with the workers there, so
/// it waits for their work, not for the wake (loadcuts1, cut 4).
const WAIT_SPINS: u32 = 64;

/// How many keys after the cursor `AheadQueue::find` looks at. The loader
/// meets the keys of the previous load in its order, less removed keys
/// and with new ones between them.
const FIND_AHEAD: usize = 4;

impl AheadQueue {
    #[must_use]
    pub fn new(keys: Arc<KeyList>) -> Self {
        let states = (0..keys.len())
            .map(|_| std::sync::atomic::AtomicU8::new(KEY_FREE))
            .collect();
        AheadQueue {
            keys,
            next: std::sync::atomic::AtomicUsize::new(0),
            states,
            waiting: std::sync::atomic::AtomicUsize::new(usize::MAX),
            done_lock: std::sync::Mutex::new(()),
            done_cv: std::sync::Condvar::new(),
            failed: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// A worker panicked outside a resolution (compiler/resolve_ahead.rs
    /// `run_task`): the loader takes no more answers of this load and
    /// resolves the rest of its keys itself, and the workers take no more
    /// keys. It does not close the job: the workers still store their
    /// lookups until the load ends.
    pub fn fail(&self) {
        self.failed
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }

    /// `fail` was called.
    #[must_use]
    pub fn failed(&self) -> bool {
        self.failed.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn lock_done(&self) -> std::sync::MutexGuard<'_, ()> {
        self.done_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// A worker takes the next key that the loader did not take: its index
    /// and parts. `None` when no key is left.
    pub fn take_next(&self) -> Option<(usize, (&str, &str, ResolutionMode))> {
        use std::sync::atomic::Ordering;
        loop {
            let index = self.next.fetch_add(1, Ordering::Relaxed);
            let key = self.keys.get(index)?;
            if self.states[index]
                .compare_exchange(KEY_FREE, KEY_WORKER, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return Some((index, key));
            }
        }
    }

    /// A worker ended key `index` (after it published the answer), and
    /// wakes the loader when it waits for the key.
    pub fn done(&self, index: usize) {
        use std::sync::atomic::Ordering;
        self.states[index].store(KEY_DONE, Ordering::SeqCst);
        if self.waiting.load(Ordering::SeqCst) == index {
            let _done = self.lock_done();
            self.done_cv.notify_all();
        }
    }

    /// The index of the loader's key `key`, from `cursor` on, and moves
    /// the cursor after it.
    fn find(&self, cursor: &Cell<usize>, key: (&str, &str, ResolutionMode)) -> Option<usize> {
        let start = cursor.get();
        let index = (start..self.keys.len().min(start + FIND_AHEAD))
            .find(|&index| self.keys.get(index) == Some(key))?;
        cursor.set(index + 1);
        Some(index)
    }

    /// For the loader's key `index` that has no answer: true when a worker
    /// resolved it (it waits while a worker resolves it now), so the answer
    /// may be published now. False: the loader resolves it itself, and no
    /// worker starts it. A wait counts in `stats`.
    fn wait_or_take(&self, index: usize, stats: &mut AheadStats) -> bool {
        use std::sync::atomic::Ordering;
        let state = &self.states[index];
        match state.compare_exchange(KEY_FREE, KEY_LOADER, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) | Err(KEY_LOADER) => false,
            Err(_) => {
                // Most resolutions end within a few microseconds: spin a
                // little, then sleep until the worker ends the key (`done`).
                let start = std::time::Instant::now();
                let mut ended = false;
                for _ in 0..WAIT_SPINS {
                    if state.load(Ordering::Acquire) != KEY_WORKER {
                        ended = true;
                        break;
                    }
                    std::hint::spin_loop();
                }
                if !ended {
                    stats.slept += 1;
                    self.waiting.store(index, Ordering::SeqCst);
                    let mut done = self.lock_done();
                    while state.load(Ordering::SeqCst) == KEY_WORKER {
                        done = self
                            .done_cv
                            .wait(done)
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                    }
                    drop(done);
                    self.waiting.store(usize::MAX, Ordering::SeqCst);
                }
                stats.waited += 1;
                let waited = u64::try_from(start.elapsed().as_nanos()).unwrap_or(u64::MAX);
                stats.wait_ns = stats.wait_ns.saturating_add(waited);
                true
            }
        }
    }
}

/// The check of a worker answer (`AheadLink::accept`): the answer and the
/// file system calls that made it.
pub type AheadAccept = Rc<dyn Fn(AheadAnswer<'_>, &[AheadCall]) -> bool>;

/// A worker answer that the loader checks (`AheadAccept`).
#[derive(Clone, Copy)]
pub enum AheadAnswer<'a> {
    /// The answer for a module resolution key.
    Module(ModuleKeyParts<'a>, &'a ResolvedModule),
    /// The package scope of a directory (`Caches::package_scope_ahead`).
    Scope(&'a str, Option<&'a PackageScope>),
}

impl std::fmt::Debug for AheadAnswer<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AheadAnswer::Module(key, _) => write!(f, "module key {key:?}"),
            AheadAnswer::Scope(directory, _) => write!(f, "package scope of {directory:?}"),
        }
    }
}

/// The parts of a package scope (Go `GetPackageScopeForPath`) that Go
/// `loadSourceFileMetaData` reads: the directory of the package.json and
/// its `type` field. `None` (no scope) when no package.json exists in the
/// directory or above it.
// PORT: not in Go (resolve ahead). A worker sends these parts to the loader,
// as its package.json parses stay on the worker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageScope {
    /// Go `GetDirectory()` of the scope.
    pub package_directory: String,
    /// The `type` field, when it is there and valid.
    pub type_: Option<String>,
}

impl PackageScope {
    /// The parts of `entry`, a result of `get_package_scope_for_path`.
    #[must_use]
    pub fn of(entry: Option<&InfoCacheEntry>) -> Option<PackageScope> {
        let entry = entry.filter(|entry| entry.exists())?;
        let contents = entry
            .contents
            .as_ref()
            .expect("an existing package.json scope has contents");
        let (value, ok) = contents.fields.header_fields.type_.get_value();
        Some(PackageScope {
            package_directory: entry.package_directory.clone(),
            type_: ok.then_some(value),
        })
    }
}

/// The module resolution keys of one program load that have no redirect,
/// and the directories whose package scope it found, in the load's order
/// (`AheadLink::keys`). A package scope key has an empty module name (a
/// module key never has one) and no mode (`push_scope`). The keys share
/// one text, so the loader records a key with no allocation of its own,
/// and the list frees in two.
// PORT: not in Go (perf).
#[derive(Default)]
pub struct KeyList {
    text: String,
    /// Per key: the end of its containing directory and of its module name
    /// in `text`, and its resolution mode. A key starts where the one
    /// before it ends.
    ends: Vec<(usize, usize, ResolutionMode)>,
    /// How many of the keys are package scope keys.
    scopes: usize,
}

impl KeyList {
    /// An empty list with room for the keys of `like`.
    #[must_use]
    pub fn with_capacity_of(like: &KeyList) -> Self {
        KeyList {
            text: String::with_capacity(like.text.len()),
            ends: Vec::with_capacity(like.ends.len()),
            scopes: 0,
        }
    }

    pub fn push(&mut self, containing_directory: &str, module_name: &str, mode: ResolutionMode) {
        self.text.push_str(containing_directory);
        let directory_end = self.text.len();
        self.text.push_str(module_name);
        self.ends.push((directory_end, self.text.len(), mode));
    }

    /// Records the package scope key of `directory`.
    pub fn push_scope(&mut self, directory: &str) {
        self.push(directory, "", RESOLUTION_MODE_NONE);
        self.scopes += 1;
    }

    /// The number of keys, package scope keys included.
    #[must_use]
    pub fn len(&self) -> usize {
        self.ends.len()
    }

    /// The number of package scope keys.
    #[must_use]
    pub fn scopes(&self) -> usize {
        self.scopes
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.ends.is_empty()
    }

    /// The containing directory, module name and mode of key `index`.
    #[must_use]
    pub fn get(&self, index: usize) -> Option<(&str, &str, ResolutionMode)> {
        let &(directory_end, end, mode) = self.ends.get(index)?;
        let start = index.checked_sub(1).map_or(0, |before| self.ends[before].1);
        Some((
            &self.text[start..directory_end],
            &self.text[directory_end..end],
            mode,
        ))
    }
}

/// What the loader did with the keys of a resolve-ahead load that its own
/// cache did not have.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AheadStats {
    /// Worker answers that passed the check.
    pub taken: usize,
    /// Worker answers that failed the check.
    pub rejected: usize,
    /// Keys with no worker answer (not resolved yet, new or not shareable),
    /// or after a worker panic (`AheadQueue::fail`).
    pub missing: usize,
    /// Keys whose worker answer the loader waited for.
    pub waited: usize,
    /// Of `waited`: the waits that slept after the spin.
    pub slept: usize,
    /// The time of all the waits, in nanoseconds.
    pub wait_ns: u64,
    /// `taken`, `rejected` and `missing` of the package scope keys (the
    /// other counts have both kinds of keys).
    pub scopes_taken: usize,
    pub scopes_rejected: usize,
    pub scopes_missing: usize,
}

impl Caches {
    /// Drops the cached resolutions when no program uses this resolver any
    /// more (`DefaultResolver::release_caches`).
    // PORT: not in Go. Go's GC frees the resolver with its last program.
    pub fn release(&self) {
        let modules = std::mem::take(&mut *self.module_resolution_cache.cache.borrow_mut());
        let type_ref_directives =
            std::mem::take(&mut *self.type_ref_directive_resolution_cache.cache.borrow_mut());
        self.parsed_patterns_for_paths.clear();
        let worker_package_jsons = std::mem::take(&mut *self.worker_package_jsons.borrow_mut());
        let worker_lookups = std::mem::take(&mut *self.worker_lookups.borrow_mut());
        drop(type_ref_directives);
        // PERF: in the language server, resolve-ahead workers made most of
        // these answers; they free them (with none, they drop here).
        crate::frontend::compiler::resolve_ahead::drop_on_worker(Box::new((
            modules,
            worker_package_jsons,
            worker_lookups,
        )));
    }
}

// Go: module/cache.go:19 moduleResolutionCache and :40
// typeRefDirectiveResolutionCache, the `SyncMap`s themselves.
// PORT: Go shares one resolver, and so these maps, between all parse tasks
// of a program. The Rust loader resolves on one thread with `Rc` values
// (`Caches`), and each parse worker has its own resolver. This is the part
// they share: the loader's resolver reads what the workers resolved ahead
// of it. A resolution is a function of its key (the key has the redirect
// config) and of the file system, so any resolver stores the same answer
// that the loader would find; first answer wins, as in Go. Only a program
// that resolves on the plain OS file system, with no project references
// and no traced resolution, shares one (`process_all_program_files`).
//
// Go also shares one package.json cache, which the build info lists
// (`Program::package_json_cache_entries`). So each answer carries the
// package.json lookups that made it (`SharedResolution`), and the loader
// adds them to its own package.json cache.
#[derive(Default)]
pub struct SharedResolutionCache {
    modules: std::sync::Mutex<
        FxHashMap<ModuleResolutionCacheKey, SharedResolution<Arc<ResolvedModule>>>,
    >,
    type_ref_directives: std::sync::Mutex<
        FxHashMap<
            TypeRefDirectiveResolutionCacheKey,
            SharedResolution<Arc<ResolvedTypeReferenceDirective>>,
        >,
    >,
    /// The package scopes that resolve-ahead workers found, by directory
    /// (`DefaultResolver::publish_package_scope`), or that parse workers
    /// found for the metadata of the files they parse (`store_scope`).
    scopes: std::sync::Mutex<FxHashMap<String, SharedResolution<Option<PackageScope>>>>,
    /// The first read of each package.json that a parse worker made in this
    /// load, by file name (`store_package_json_read`).
    package_json_reads: std::sync::Mutex<FxHashMap<String, Arc<WorkerPackageJsonRead>>>,
}

/// A parse worker's answer in the `SharedResolutionCache`.
#[derive(Clone)]
pub struct SharedResolution<T> {
    pub value: T,
    /// The package.json lookups of the resolution (`package_json_log`).
    pub package_jsons: Arc<[PackageJsonLookup]>,
    /// The file system lookups of the resolution that the `tsc -b` host's
    /// cache did not have (`note_worker_lookup`). `None` when the worker
    /// does not log them (no `tsc -b` host).
    pub lookups: Option<Arc<[StatLookup]>>,
    /// The file system calls of a resolve-ahead worker's resolution, with
    /// their answers (`AheadCall`). `None` from other parse workers. The
    /// loader takes a resolve-ahead answer only with them.
    pub ahead: Option<Arc<[AheadCall]>>,
}

/// One file system call of a resolve-ahead worker's resolution
/// (compiler/resolve_ahead.rs), with its answer. Before the loader takes
/// the answer, it checks each call on its own file system and replays the
/// side effects of the call (`AheadLink::accept`). The name of a
/// `FileExists` or `DirectoryExists` call is its path (the worker does not
/// share an answer with another name), so the loader's caches by name and
/// by path both find it.
// PORT: not in Go (perf). Go resolves in each parse task on the host's
// file system, which tracks the calls itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AheadCall {
    /// `file_exists` of `path`. `known`: the worker took the answer from
    /// what the workers found in earlier loads, not from the OS during
    /// this load, so the loader asks its own file system.
    FileExists {
        path: Path,
        exists: bool,
        known: bool,
    },
    /// `directory_exists` of `path`.
    DirectoryExists { path: Path, exists: bool },
    /// `realpath` of `name`.
    Realpath { name: String, real: String },
    /// `read_file`: the xxh3 hash of the text, `None` when the file could
    /// not be read.
    Read {
        file_name: String,
        hash: Option<u128>,
    },
    /// The calls of Go `getPackageJsonInfo` for one package.json cache
    /// entry (no `PackageJson` in them). One worker's answers that read the
    /// entry share them, so the loader checks them once per load.
    PackageJson(Arc<[AheadCall]>),
    /// A `DirectoryExists` or `Realpath` that all the answers of the job
    /// share (compiler/resolve_ahead.rs `WorkerStats`), so the loader checks
    /// it once per load.
    Shared(Arc<AheadCall>),
}

impl AheadCall {
    /// Calls `f` with each call of `calls`, the calls of a `PackageJson`
    /// or `Shared` in its place.
    pub fn each(calls: &[AheadCall], f: &mut impl FnMut(&AheadCall)) {
        for call in calls {
            match call {
                AheadCall::PackageJson(group) => AheadCall::each(group, f),
                AheadCall::Shared(call) => AheadCall::each(std::slice::from_ref(&**call), f),
                call => f(call),
            }
        }
    }
}

/// How the resolve-ahead call log of a resolution ended
/// (`Caches::take_ahead_log`).
pub enum AheadLogEnd {
    /// This thread does not log (it is no resolve-ahead worker).
    NotLogged,
    /// The resolution made a call that the loader cannot check (a read of
    /// an open file, a directory listing, a stat). Its answer is not
    /// published.
    Unshareable,
    Logged(Arc<[AheadCall]>),
}

/// The state of a resolve-ahead worker thread.
struct AheadThread {
    current_directory: String,
    use_case_sensitive_file_names: bool,
    /// The calls of the resolution that runs now. `None` between
    /// resolutions.
    calls: Option<Vec<AheadCall>>,
    /// False when the resolution made a call that the loader cannot check.
    shareable: bool,
    /// The hash of each file that this worker read, by name. The
    /// package.json cache of the worker's resolver keeps the parses of
    /// these texts (from job to job, compiler/resolve_ahead.rs), and a
    /// later resolution that reads the cache logs the read again
    /// (`Caches::log_package_json`).
    reads: FxHashMap<String, Option<u128>>,
    /// The calls of each package.json cache entry of this job, by the
    /// entry's address (the cache keeps each entry for the whole job).
    /// `None`: the entry's file was not read from a file that the loader
    /// can check.
    package_jsons: FxHashMap<usize, Option<Arc<[AheadCall]>>>,
    /// The package.json files whose cache entries the worker kept from an
    /// earlier job (`begin_ahead_thread`).
    kept: FxHashSet<String>,
    /// Whether a directory exists now on the worker's file system, with
    /// no log (`begin_ahead_thread`).
    directory_exists: Rc<dyn Fn(&str) -> bool>,
}

thread_local! {
    /// Set on a resolve-ahead worker thread (`begin_ahead_thread`).
    static AHEAD: RefCell<Option<AheadThread>> = const { RefCell::new(None) };
}

/// Makes this thread a resolve-ahead worker: its resolutions log their file
/// system calls (`note_ahead_call`). `current_directory` and
/// `use_case_sensitive_file_names` make the paths, as the host's `to_path`.
/// `reads` has the hash of the text of each package.json that the
/// worker's package.json cache has already; `kept` names those that the
/// cache kept from an earlier job. `directory_exists` answers whether a
/// directory exists now, as the worker's file system does, with no log.
pub fn begin_ahead_thread(
    current_directory: &str,
    use_case_sensitive_file_names: bool,
    reads: FxHashMap<String, Option<u128>>,
    kept: FxHashSet<String>,
    directory_exists: Rc<dyn Fn(&str) -> bool>,
) {
    AHEAD.with(|ahead| {
        *ahead.borrow_mut() = Some(AheadThread {
            current_directory: current_directory.to_string(),
            use_case_sensitive_file_names,
            calls: None,
            shareable: true,
            reads,
            package_jsons: FxHashMap::default(),
            kept,
            directory_exists,
        });
    });
}

/// True on a resolve-ahead worker (`begin_ahead_thread`).
#[must_use]
pub fn on_ahead_thread() -> bool {
    AHEAD.with(|ahead| ahead.borrow().is_some())
}

/// Ends `begin_ahead_thread`: the hashes of the files that the worker read
/// (`AheadThread::reads`).
#[must_use]
pub fn end_ahead_thread() -> FxHashMap<String, Option<u128>> {
    AHEAD
        .with(|ahead| ahead.borrow_mut().take())
        .map(|state| state.reads)
        .unwrap_or_default()
}

/// Logs `call` in the resolution that runs on this thread, if it logs.
pub fn note_ahead_call(call: AheadCall) {
    AHEAD.with(|ahead| {
        if let Some(calls) = ahead
            .borrow_mut()
            .as_mut()
            .and_then(|state| state.calls.as_mut())
        {
            calls.push(call);
        }
    });
}

/// Logs a read of `file_name` (`hash` of its text, `None` when it could not
/// be read) in the resolution that runs on this thread, and keeps the hash
/// for later reads of the same parse.
pub fn note_ahead_read(file_name: &str, hash: Option<u128>) {
    AHEAD.with(|ahead| {
        if let Some(state) = ahead.borrow_mut().as_mut() {
            state.reads.insert(file_name.to_string(), hash);
            if let Some(calls) = state.calls.as_mut() {
                calls.push(AheadCall::Read {
                    file_name: file_name.to_string(),
                    hash,
                });
            }
        }
    });
}

/// Logs the file system calls of Go `getPackageJsonInfo` for `entry` in
/// the resolve-ahead resolution that runs on this thread. A resolution that
/// finds the entry in the worker's package.json cache makes no call, but
/// the loader's own resolution makes them when its cache does not have the
/// entry. So each resolution that reads the entry lists them, as
/// `Caches::log_package_json` does for the `tsc -b` lookups, as one
/// `AheadCall::PackageJson` that the worker's answers share. A call that
/// the worker made for this entry is then listed twice, which the loader's
/// check and replay allow.
fn log_ahead_package_json(entry: &InfoCacheEntry) {
    AHEAD.with(|ahead| {
        let mut ahead = ahead.borrow_mut();
        let Some(state) = ahead.as_mut().filter(|state| state.calls.is_some()) else {
            return;
        };
        let key = std::ptr::from_ref(entry) as usize;
        let group = match state.package_jsons.get(&key) {
            Some(group) => group.clone(),
            None => {
                let group = package_json_calls(entry, state);
                state.package_jsons.insert(key, group.clone());
                group
            }
        };
        match (group, state.calls.as_mut()) {
            (Some(group), Some(calls)) => calls.push(AheadCall::PackageJson(group)),
            // The worker did not read the package.json from a file that the
            // loader can check.
            _ => state.shareable = false,
        }
    });
}

/// The calls of Go `getPackageJsonInfo` for `entry`. `None` when the worker
/// did not read its package.json from a file that the loader can check,
/// when a name is not its path (`AheadCall`), or when the entry was kept
/// from an earlier job and its directory is gone now: Go's lookup asks
/// whether the directory exists. The next job drops such an entry. The
/// worker knows the package.json of a kept entry from an earlier job, so
/// the loader asks its own file system for it (`known`).
fn package_json_calls(entry: &InfoCacheEntry, state: &mut AheadThread) -> Option<Arc<[AheadCall]>> {
    let to_path = |name: &str| {
        Some(to_path(
            name,
            &state.current_directory,
            state.use_case_sensitive_file_names,
        ))
        .filter(|path| path.as_str() == name)
    };
    let directory = AheadCall::DirectoryExists {
        path: to_path(&entry.package_directory)?,
        exists: entry.directory_exists,
    };
    if !entry.directory_exists {
        return Some(Arc::new([directory]));
    }
    let file_name = combine_paths(&entry.package_directory, &["package.json"]);
    let kept = state.kept.contains(&file_name);
    let exists = AheadCall::FileExists {
        path: to_path(&file_name)?,
        exists: entry.exists(),
        known: kept,
    };
    if !entry.exists() {
        return Some(Arc::new([directory, exists]));
    }
    if kept && !(state.directory_exists)(&entry.package_directory) {
        state.reads.remove(&file_name);
        return None;
    }
    let hash = *state.reads.get(&file_name)?;
    Some(Arc::new([
        directory,
        exists,
        AheadCall::Read { file_name, hash },
    ]))
}

/// Marks the resolution that runs on this thread as not shareable: it made
/// a call that the loader cannot check. With `file_name`, it read that
/// file from a source that the loader cannot check (an open file), so a
/// later resolution that uses its parse is not shareable either.
pub fn note_ahead_unshareable(file_name: Option<&str>) {
    AHEAD.with(|ahead| {
        if let Some(state) = ahead.borrow_mut().as_mut() {
            state.shareable = false;
            if let Some(file_name) = file_name {
                state.reads.remove(file_name);
            }
        }
    });
}

/// A lookup that Go `cachedvfs` caches (all but `Stat`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatKind {
    FileExists,
    DirectoryExists,
    Realpath,
    Entries,
}

/// One cached lookup of a parse worker's resolution, without its value.
/// The value is in the worker cache of the program load
/// (`BuildStatCache`).
#[derive(Clone, Debug)]
pub struct StatLookup {
    pub kind: StatKind,
    pub path: String,
}

thread_local! {
    /// True on a parse worker thread whose file system reads the `tsc -b`
    /// host's cache (`set_worker_lookup_log`).
    static WORKER_LOOKUP_LOG: Cell<bool> = const { Cell::new(false) };
    /// The lookups of the parse worker resolution that runs on this thread
    /// now (`Caches::start_package_json_log` to `take_package_json_log`).
    static WORKER_LOOKUPS: RefCell<Option<Vec<StatLookup>>> = const { RefCell::new(None) };
}

/// Makes the resolutions of this parse worker thread log their file system
/// lookups (`note_worker_lookup`) or not.
// PORT: not in Go. Go parse tasks share the host's cachedvfs, so the
// lookups of each resolution are in it. A `tsc -b` parse worker keeps its
// lookups out of the host's cache, and the loader adds the lookups of the
// answers it takes (`BuildStatCache`).
pub fn set_worker_lookup_log(on: bool) {
    WORKER_LOOKUP_LOG.with(|log| log.set(on));
}

/// Records a lookup of the parse worker resolution that runs on this
/// thread, if one runs and this thread logs lookups.
pub fn note_worker_lookup(kind: StatKind, path: &str) {
    WORKER_LOOKUPS.with(|log| {
        if let Some(log) = log.borrow_mut().as_mut() {
            log.push(StatLookup {
                kind,
                path: path.to_string(),
            });
        }
    });
}

/// One Go `getPackageJsonInfo` call of a parse worker's resolution: the
/// package.json cache entry that it read or stored. The parsed contents stay
/// on the worker; a lookup of the metadata scope walk carries the text of
/// the first read (`read`).
#[derive(Clone, Debug)]
pub struct PackageJsonLookup {
    pub package_directory: String,
    pub directory_exists: bool,
    /// The package.json file exists (`InfoCacheEntry::exists`).
    pub exists: bool,
    /// The first read of the load for the entry's package.json, which the
    /// loader keeps in its own cache when it takes the lookup
    /// (`Caches::adopt_worker_package_jsons`). Only the lookups of a parse
    /// worker's package scope walk for the metadata of a file have it
    /// (`WorkerPackageJsonReads::attach`); the loader keeps the other reads
    /// at the end of the load (`SharedResolutionCache::end_package_json_reads`).
    pub read: Option<Arc<WorkerPackageJsonRead>>,
}

/// A parse worker's reads of its package.json cache entries.
// PORT: not in Go (see `WorkerPackageJsonRead`).
#[derive(Default)]
pub struct WorkerPackageJsonReads {
    /// The read of the load for each entry
    /// (`SharedResolutionCache::store_package_json_read`), by the address of
    /// the entry. The worker's cache keeps each entry while the resolver
    /// lives, so an address names one entry.
    reads: RefCell<FxHashMap<usize, Arc<WorkerPackageJsonRead>>>,
    /// True while the worker finds the package scope of a file's directory
    /// for its metadata (Go `loadSourceFileMetaData`, which the loader runs
    /// in Go): the lookups then carry their reads, for the loader's cache.
    pub attach: Cell<bool>,
}

/// The first read of one package.json by the parse workers of a load (Go
/// `getPackageJsonInfo`, module/resolver.go:1755): the parts of the
/// package.json cache entry that it made, with the text of the file. The
/// lookups of the file that carry a read share it. A worker's entry has `Rc`
/// contents, which stay on the worker's thread, so the loader takes the
/// text (`adopt`), and its cache parses the text when a lookup first asks
/// for the entry (`InfoCache::get`).
// PORT: not in Go. Go's parse tasks share the resolver's one package.json
// cache, which keeps the first entry of each file (packagejson/cache.go:190
// `Set`).
pub struct WorkerPackageJsonRead {
    package_directory: String,
    directory_exists: bool,
    /// Set when the loader has taken the read.
    taken: std::sync::atomic::AtomicBool,
    /// The text of the package.json, until the loader takes it. `None`:
    /// the package.json does not exist.
    text: std::sync::Mutex<Option<Box<str>>>,
}

impl std::fmt::Debug for WorkerPackageJsonRead {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("WorkerPackageJsonRead")
    }
}

impl WorkerPackageJsonRead {
    /// The text of the read, the first time: `Some(None)` when the
    /// package.json does not exist. `None` after the first time.
    fn take(&self) -> Option<Option<Box<str>>> {
        use std::sync::atomic::Ordering;
        if self.taken.load(Ordering::Relaxed) || self.taken.swap(true, Ordering::Relaxed) {
            return None;
        }
        Some(lock_shared(&self.text).take())
    }

    /// Gives the read to `cache`, the loader's package.json cache, the
    /// first time: the cache keeps it unless it has the entry
    /// (`InfoCache::add_pending`), and parses the text only when a lookup
    /// asks for the entry.
    fn adopt(&self, cache: &InfoCache) {
        let Some(text) = self.take() else {
            return;
        };
        cache.add_pending(
            &combine_paths(&self.package_directory, &["package.json"]),
            PendingInfo {
                package_directory: self.package_directory.clone(),
                directory_exists: self.directory_exists,
                text,
            },
        );
        #[cfg(test)]
        ADOPTED_PACKAGE_JSONS.with(|count| count.set(count.get() + 1));
    }
}

thread_local! {
    /// The last package.json that the resolver of the parse worker on this
    /// thread read (`note_worker_package_json_read`): its file name and
    /// text. `log_package_json` takes it for the entry that the read makes.
    static WORKER_PACKAGE_JSON_READ: RefCell<Option<(String, Box<str>)>> =
        const { RefCell::new(None) };
}

#[cfg(test)]
thread_local! {
    /// The reads that loaders on this thread took from parse workers
    /// (`WorkerPackageJsonRead::adopt`), for tests.
    static ADOPTED_PACKAGE_JSONS: Cell<usize> = const { Cell::new(0) };
}

/// The reads that loaders on this thread took from parse workers so far
/// (`WorkerPackageJsonRead::adopt`), for tests.
#[cfg(test)]
pub(crate) fn adopted_package_jsons() -> usize {
    ADOPTED_PACKAGE_JSONS.with(Cell::get)
}

#[cfg(test)]
thread_local! {
    /// A test's choice to make the loads on this thread wait for the
    /// answers of the parse workers (`set_answer_wait`).
    static ANSWER_WAIT: Cell<bool> = const { Cell::new(false) };
}

/// Makes the loader's resolver on this thread wait in
/// `SharedResolutionCache::get_module` until a parse worker stores the
/// answer of the key (up to 60 s), so the loader takes the worker's answer
/// whatever the timing (tests). Only for keys that a worker resolves: a
/// key that none resolves waits the 60 s, then the loader resolves it.
#[cfg(test)]
pub(crate) fn set_answer_wait(on: bool) {
    ANSWER_WAIT.with(|wait| wait.set(on));
}

/// Keeps `text`, the text of the package.json `file_name` that the
/// resolver of the parse worker on this thread just read, for the entry
/// that the read makes (`Caches::worker_package_json_read`).
// PORT: not in Go (see `WorkerPackageJsonRead`).
pub fn note_worker_package_json_read(file_name: &str, text: &str) {
    WORKER_PACKAGE_JSON_READ
        .with(|read| *read.borrow_mut() = Some((file_name.to_string(), text.into())));
}

/// One entry of `DefaultResolver::package_json_cache_entries`: the parts
/// of a package.json cache entry (Go `*packagejson.InfoCacheEntry`) that the
/// build info reads.
// PORT: Go yields the cache entries. The entries of the parse workers'
// lookups (`Caches::worker_package_jsons`) have no contents here, so the
// callback gets these parts of each entry instead.
#[derive(Clone, Copy, Debug)]
pub struct PackageJsonCacheEntry<'e> {
    /// Go `GetDirectory()`.
    pub package_directory: &'e str,
    pub directory_exists: bool,
    /// Go `Exists()`: the package.json file was read.
    pub exists: bool,
}

/// A resolver's link to a `SharedResolutionCache`.
#[derive(Clone)]
pub struct SharedResolutionLink {
    pub cache: Arc<SharedResolutionCache>,
    /// True for a parse worker's resolver: it stores each answer it makes.
    /// The loader's resolver only reads, so its serial path does no extra
    /// copies.
    pub publish: bool,
}

fn lock_shared<T>(mutex: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl SharedResolutionCache {
    /// Go `moduleResolutionCache.Get`.
    #[must_use]
    pub fn get_module(&self, key: &dyn ModuleKey) -> Option<SharedResolution<Arc<ResolvedModule>>> {
        #[cfg(test)]
        if ANSWER_WAIT.with(Cell::get) {
            return self.wait_for_module(key);
        }
        lock_shared(&self.modules).get(key).cloned()
    }

    /// `get_module` on a thread that waits for the answers of the parse
    /// workers (`set_answer_wait`): up to 60 s for a worker to store the
    /// answer of `key`.
    #[cfg(test)]
    fn wait_for_module(
        &self,
        key: &dyn ModuleKey,
    ) -> Option<SharedResolution<Arc<ResolvedModule>>> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        loop {
            let found = lock_shared(&self.modules).get(key).cloned();
            if found.is_some() || std::time::Instant::now() > deadline {
                return found;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    /// Calls `f` with the resolve-ahead calls of each module answer and
    /// package scope (`SharedResolution::ahead`).
    // PORT: not in Go (resolve ahead, compiler/resolve_ahead.rs).
    pub fn for_each_ahead_calls(&self, mut f: impl FnMut(&[AheadCall])) {
        for value in lock_shared(&self.modules).values() {
            f(value.ahead.as_deref().unwrap_or_default());
        }
        for value in lock_shared(&self.scopes).values() {
            f(value.ahead.as_deref().unwrap_or_default());
        }
    }

    /// The package scope of `directory` that a worker found.
    // PORT: not in Go (resolve ahead).
    #[must_use]
    pub fn get_scope(&self, directory: &str) -> Option<SharedResolution<Option<PackageScope>>> {
        lock_shared(&self.scopes).get(directory).cloned()
    }

    /// Publishes the package scope of `directory` (the first value wins).
    // PORT: not in Go (resolve ahead).
    pub fn set_scope(&self, directory: String, value: SharedResolution<Option<PackageScope>>) {
        lock_shared(&self.scopes).entry(directory).or_insert(value);
    }

    /// Stores `read()`, a parse worker's read of the package.json
    /// `file_name`, unless the load has one, and returns the one that the
    /// cache keeps: the first, as Go `InfoCache.Set` keeps the first entry.
    // PORT: not in Go (see `WorkerPackageJsonRead`).
    pub fn store_package_json_read(
        &self,
        file_name: &str,
        read: impl FnOnce() -> WorkerPackageJsonRead,
    ) -> Arc<WorkerPackageJsonRead> {
        lock_shared(&self.package_json_reads)
            .entry(file_name.to_string())
            .or_insert_with(|| Arc::new(read()))
            .clone()
    }

    /// The read of the package.json `file_name` that the load keeps
    /// (`store_package_json_read`).
    // PORT: not in Go (see `WorkerPackageJsonRead`).
    #[must_use]
    pub fn package_json_read(&self, file_name: &str) -> Option<Arc<WorkerPackageJsonRead>> {
        lock_shared(&self.package_json_reads)
            .get(file_name)
            .cloned()
    }

    /// True when the load has a read of the package.json `file_name`, so a
    /// worker that reads the file again need not keep its text.
    // PORT: not in Go (see `WorkerPackageJsonRead`).
    #[must_use]
    pub fn has_package_json_read(&self, file_name: &str) -> bool {
        lock_shared(&self.package_json_reads).contains_key(file_name)
    }

    /// Gives the package.json reads of the load that the loader did not
    /// take to `cache`, the program resolver's package.json cache, when the
    /// load ends: the reads of the worker module and type reference
    /// answers. Go's parse tasks put them into the program resolver's one
    /// cache, so a later lookup (Go `Program.GetPackageJsonInfo`) finds
    /// what the load read. The cache parses a text only when a lookup asks
    /// for it (`InfoCache::get`).
    // PORT: not in Go (see `WorkerPackageJsonRead`).
    pub fn end_package_json_reads(&self, cache: &InfoCache) {
        let reads = std::mem::take(&mut *lock_shared(&self.package_json_reads));
        for read in reads.values() {
            read.adopt(cache);
        }
    }

    /// Publishes the package scope of `directory` (the first value wins),
    /// and returns the value that the cache keeps. A parse worker finds
    /// the metadata of a file with it, so every file of a directory gets
    /// one value and one lookup log (the loader notes each log once).
    // PORT: not in Go (loadpar1). Go's parse tasks share one package.json
    // cache, so the scope walk of a directory reads it from there.
    pub fn store_scope(
        &self,
        directory: &str,
        value: SharedResolution<Option<PackageScope>>,
    ) -> SharedResolution<Option<PackageScope>> {
        lock_shared(&self.scopes)
            .entry(directory.to_string())
            .or_insert(value)
            .clone()
    }

    /// Go `moduleResolutionCache.Set` (`LoadOrStore`: the first value wins).
    pub fn set_module(
        &self,
        key: ModuleResolutionCacheKey,
        value: SharedResolution<Arc<ResolvedModule>>,
    ) {
        lock_shared(&self.modules).entry(key).or_insert(value);
    }

    /// Go `typeRefDirectiveResolutionCache.Get`.
    #[must_use]
    pub fn get_type_ref_directive(
        &self,
        key: &TypeRefDirectiveResolutionCacheKey,
    ) -> Option<SharedResolution<Arc<ResolvedTypeReferenceDirective>>> {
        lock_shared(&self.type_ref_directives).get(key).cloned()
    }

    /// Go `typeRefDirectiveResolutionCache.Set` (`Store`: the last value
    /// wins).
    pub fn set_type_ref_directive(
        &self,
        key: TypeRefDirectiveResolutionCacheKey,
        value: SharedResolution<Arc<ResolvedTypeReferenceDirective>>,
    ) {
        lock_shared(&self.type_ref_directives).insert(key, value);
    }
}

impl Caches {
    /// True for a parse worker's resolver, which publishes its answers.
    #[must_use]
    pub fn publishes(&self) -> bool {
        self.shared.as_ref().is_some_and(|shared| shared.publish)
    }

    /// Records a package.json lookup of a parse worker's resolution
    /// (`publishes`). `entry` is the package.json cache entry that the
    /// lookup read or stored.
    ///
    /// The file system lookups of Go `getPackageJsonInfo` for the entry go
    /// into the resolution's lookup log too: an earlier resolution of the
    /// worker made them, but Go makes them in whichever resolution of the
    /// load reads the entry first, so each one that reads it lists them.
    pub fn log_package_json(&self, entry: &InfoCacheEntry) {
        if self.publishes() {
            self.package_json_log.borrow_mut().push(PackageJsonLookup {
                package_directory: entry.package_directory.clone(),
                directory_exists: entry.directory_exists,
                exists: entry.exists(),
                read: self.worker_package_json_read(entry),
            });
            if WORKER_LOOKUPS.with(|log| log.borrow().is_some()) {
                note_worker_lookup(StatKind::DirectoryExists, &entry.package_directory);
                if entry.directory_exists {
                    note_worker_lookup(
                        StatKind::FileExists,
                        &combine_paths(&entry.package_directory, &["package.json"]),
                    );
                }
            }
            log_ahead_package_json(entry);
        }
    }

    /// The load's read of the package.json of `entry`, an entry of this
    /// parse worker's package.json cache (`worker_package_json_reads`), for
    /// a lookup of the package scope walk (`WorkerPackageJsonReads::attach`).
    /// The lookup that made an entry of a package.json file comes right
    /// after the worker read the file, in any resolution: when the worker
    /// kept the text (`note_worker_package_json_read`), the read goes into
    /// the shared cache, which keeps the first read of the load. Else the
    /// load had a read of the file already, and the scope walk takes that
    /// one. An entry of a missing package.json needs no text, so the scope
    /// walk stores its read when it first meets it. `None` for another
    /// lookup or resolver.
    fn worker_package_json_read(
        &self,
        entry: &InfoCacheEntry,
    ) -> Option<Arc<WorkerPackageJsonRead>> {
        let reads = self.worker_package_json_reads.as_ref()?;
        let address = std::ptr::from_ref(entry) as usize;
        let made = WORKER_PACKAGE_JSON_READ.with(|read| {
            let mut read = read.borrow_mut();
            if read.is_none() || !entry.exists() {
                return None;
            }
            let file_name = combine_paths(&entry.package_directory, &["package.json"]);
            read.take_if(|(read_name, _)| *read_name == file_name)
        });
        let read = match made {
            Some((file_name, text)) => self.store_package_json_read(entry, &file_name, Some(text)),
            None if !reads.attach.get() => return None,
            None => match reads.reads.borrow().get(&address) {
                Some(read) => return Some(read.clone()),
                None => {
                    let file_name = combine_paths(&entry.package_directory, &["package.json"]);
                    if entry.exists() {
                        self.shared
                            .as_ref()
                            .and_then(|shared| shared.cache.package_json_read(&file_name))
                    } else {
                        self.store_package_json_read(entry, &file_name, None)
                    }
                }
            },
        }?;
        reads.reads.borrow_mut().insert(address, read.clone());
        reads.attach.get().then_some(read)
    }

    /// Stores the read of `entry`, an entry of this parse worker's
    /// package.json cache for `file_name`, in the shared cache, and returns
    /// the read that the shared cache keeps.
    fn store_package_json_read(
        &self,
        entry: &InfoCacheEntry,
        file_name: &str,
        text: Option<Box<str>>,
    ) -> Option<Arc<WorkerPackageJsonRead>> {
        let shared = self.shared.as_ref()?;
        Some(
            shared
                .cache
                .store_package_json_read(file_name, || WorkerPackageJsonRead {
                    package_directory: entry.package_directory.clone(),
                    directory_exists: entry.directory_exists,
                    taken: std::sync::atomic::AtomicBool::new(false),
                    text: std::sync::Mutex::new(text),
                }),
        )
    }

    /// Starts the package.json log of a parse worker's resolution. The
    /// lookups before it (such as `source_file_meta_data`) are not part of
    /// a resolution; the loader makes them itself. It starts the file
    /// system lookup log too, on a thread that logs them
    /// (`set_worker_lookup_log`).
    pub fn start_package_json_log(&self) {
        if self.publishes() {
            self.package_json_log.borrow_mut().clear();
            if WORKER_LOOKUP_LOG.with(Cell::get) {
                WORKER_LOOKUPS.with(|log| *log.borrow_mut() = Some(Vec::new()));
            }
            AHEAD.with(|ahead| {
                if let Some(state) = ahead.borrow_mut().as_mut() {
                    state.calls = Some(Vec::new());
                    state.shareable = true;
                }
            });
        }
    }

    /// Takes the resolve-ahead call log of the resolution that just ended
    /// (`start_package_json_log` to here), and stops it.
    pub fn take_ahead_log(&self) -> AheadLogEnd {
        AHEAD.with(|ahead| {
            let mut ahead = ahead.borrow_mut();
            let Some(state) = ahead.as_mut() else {
                return AheadLogEnd::NotLogged;
            };
            match state.calls.take() {
                None => AheadLogEnd::NotLogged,
                Some(calls) if state.shareable => AheadLogEnd::Logged(calls.into()),
                Some(_) => AheadLogEnd::Unshareable,
            }
        })
    }

    /// The loader's resolver in a resolve-ahead load: records `key` for
    /// the next load (`AheadLink::keys`), and gives the worker answer for
    /// `key` when there is one and it passes the check
    /// (`AheadLink::accept`). `None`: the loader resolves the key itself.
    pub fn take_resolved_ahead(&self, key: ModuleKeyParts<'_>) -> Option<Arc<ResolvedModule>> {
        let ahead = self.ahead.borrow();
        let ahead = ahead.as_ref()?;
        let (containing_directory, module_name, mode, redirect_config_name) = key;
        // An empty name is a package scope key (`KeyList`). The loader
        // resolves no empty name (`resolveImportsAndModuleAugmentations`
        // skips it).
        if module_name.is_empty() {
            return None;
        }
        // A key with a redirect needs its project reference; only the
        // loader resolves it.
        if redirect_config_name.is_empty() {
            ahead
                .keys
                .borrow_mut()
                .push(containing_directory, module_name, mode);
        }
        let answers = ahead.answers.as_ref()?;
        let mut stats = ahead.stats.get();
        if ahead.queue.as_ref().is_some_and(|queue| queue.failed()) {
            stats.missing += 1;
            ahead.stats.set(stats);
            return None;
        }
        let mut found = answers.get_module(&key);
        if redirect_config_name.is_empty()
            && let Some(queue) = &ahead.queue
            && let Some(index) =
                queue.find(&ahead.cursor, (containing_directory, module_name, mode))
            && found.is_none()
            && queue.wait_or_take(index, &mut stats)
        {
            found = answers.get_module(&key);
        }
        let accepted = match found
            .as_ref()
            .and_then(|found| Some((found, found.ahead.as_ref()?)))
        {
            None => {
                stats.missing += 1;
                false
            }
            Some((found, calls)) => {
                let accepted = (ahead.accept)(AheadAnswer::Module(key, &found.value), calls);
                if accepted {
                    stats.taken += 1;
                } else {
                    stats.rejected += 1;
                }
                accepted
            }
        };
        ahead.stats.set(stats);
        let found = found.filter(|_| accepted)?;
        self.note_worker_package_jsons(&found.package_jsons);
        Some(found.value)
    }

    /// The loader's resolver in a resolve-ahead load: the package scope of
    /// `directory`, as Go `loadSourceFileMetaData` reads it
    /// (`GetPackageScopeForPath`). `None` when the load does not resolve
    /// ahead.
    ///
    /// The first file of a directory in the load records the directory for
    /// the workers of the next load (`KeyList::push_scope`). It takes the
    /// workers' scope when the answer passes the check (`AheadLink::accept`),
    /// as a module answer; else `find` finds the scope. The later files of
    /// the directory get the same scope with no call: Go's lookups for them
    /// find each package.json of the first lookup in the cache.
    // PORT: not in Go (perf). Go finds the scope in each parse task.
    pub fn package_scope_ahead(
        &self,
        directory: &str,
        find: impl FnOnce() -> Option<PackageScope>,
    ) -> Option<Option<PackageScope>> {
        let ahead = self.ahead.borrow();
        let ahead = ahead.as_ref()?;
        if let Some(scope) = ahead.scopes.borrow().get(directory) {
            return Some(scope.clone());
        }
        ahead.keys.borrow_mut().push_scope(directory);
        let scope = self.take_scope_ahead(ahead, directory).unwrap_or_else(find);
        ahead
            .scopes
            .borrow_mut()
            .insert(directory.to_string(), scope.clone());
        Some(scope)
    }

    /// The workers' package scope of `directory`, when there is one and it
    /// passes the check (`package_scope_ahead`).
    fn take_scope_ahead(&self, ahead: &AheadLink, directory: &str) -> Option<Option<PackageScope>> {
        let answers = ahead.answers.as_ref()?;
        let mut stats = ahead.stats.get();
        let mut found = None;
        if !ahead.queue.as_ref().is_some_and(|queue| queue.failed()) {
            found = answers.get_scope(directory);
            if let Some(queue) = &ahead.queue
                && let Some(index) =
                    queue.find(&ahead.cursor, (directory, "", RESOLUTION_MODE_NONE))
                && found.is_none()
                && queue.wait_or_take(index, &mut stats)
            {
                found = answers.get_scope(directory);
            }
        }
        let taken = match found
            .as_ref()
            .and_then(|found| Some((found, found.ahead.as_ref()?)))
        {
            None => {
                stats.scopes_missing += 1;
                false
            }
            Some((found, calls)) => {
                let accepted =
                    (ahead.accept)(AheadAnswer::Scope(directory, found.value.as_ref()), calls);
                if accepted {
                    stats.scopes_taken += 1;
                } else {
                    stats.scopes_rejected += 1;
                }
                accepted
            }
        };
        ahead.stats.set(stats);
        let found = found.filter(|_| taken)?;
        self.note_worker_package_jsons(&found.package_jsons);
        Some(found.value)
    }

    /// Takes the package.json lookups of the resolution that just ended.
    pub fn take_package_json_log(&self) -> Arc<[PackageJsonLookup]> {
        std::mem::take(&mut *self.package_json_log.borrow_mut()).into()
    }

    /// Takes the file system lookups of the resolution that just ended
    /// (`note_worker_lookup`), and stops the log.
    pub fn take_worker_lookup_log(&self) -> Option<Arc<[StatLookup]>> {
        WORKER_LOOKUPS
            .with(|log| log.borrow_mut().take())
            .map(Into::into)
    }

    /// Keeps the package.json lookups of a worker answer that the loader's
    /// resolver took (`worker_package_jsons`).
    pub fn note_worker_package_jsons(&self, package_jsons: &Arc<[PackageJsonLookup]>) {
        if !package_jsons.is_empty() {
            self.worker_package_jsons
                .borrow_mut()
                .push(package_jsons.clone());
        }
    }

    /// Keeps the file system lookups of a worker answer that the loader's
    /// resolver took (`worker_lookups`).
    pub fn note_worker_lookups(&self, lookups: &Option<Arc<[StatLookup]>>) {
        if let Some(lookups) = lookups.as_ref().filter(|lookups| !lookups.is_empty()) {
            self.worker_lookups.borrow_mut().push(lookups.clone());
        }
    }
}

// PORT: the loader's resolver; the package.json cache is in its
// `ResolutionData` (ts#64519).
impl DefaultResolver {
    /// The loader's resolver: keeps the read of each lookup of
    /// `package_jsons` that has one, the lookups of the package scope walk
    /// of a file's metadata that a parse worker made, in its package.json
    /// cache, as the first read of the load made it
    /// (`PackageJsonLookup::read`), unless the cache has the entry: Go
    /// `InfoCache.Set` keeps the first. In Go the loader finds the metadata
    /// (fileloader.go:391 `loadSourceFileMetaData`) with the program's
    /// resolver, so a later lookup on the loading thread (the loader's own
    /// resolutions, Go `Program.GetPackageJsonInfo`) finds what the load
    /// read and does not read the file again. The cache parses a text only
    /// when a lookup asks for it (`InfoCache::get`), so the load does no
    /// extra parse.
    // PORT: not in Go (see `WorkerPackageJsonRead`). The reads of the other
    // worker answers go into the cache at the end of the load
    // (`SharedResolutionCache::end_package_json_reads`). Until then the
    // loader's own lookup of such a file reads it, and that read stays.
    pub fn adopt_worker_package_jsons(&self, package_jsons: &[PackageJsonLookup]) {
        for read in package_jsons
            .iter()
            .filter_map(|lookup| lookup.read.as_ref())
        {
            read.adopt(&self.package_json_info_cache);
        }
    }
}

// Go: module/cache.go:74 newCaches (at 673a5f17d713; removed by ts#64519,
// which makes it newResolutionData, module/cache.go:72 `new_resolution_data`)

// Go: module/cache.go:101 getRedirectConfigName
#[must_use]
pub fn get_redirect_config_name(redirect: Option<&dyn ModuleResolvedProjectReference>) -> String {
    match redirect {
        None => String::new(),
        Some(redirect) => redirect.config_name().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::vfs::osvfs_fs;

    struct TestHost {
        fs: Rc<dyn Fs>,
        current_directory: String,
    }

    impl ResolutionHost for TestHost {
        fn fs(&self) -> &dyn Fs {
            &*self.fs
        }

        fn get_current_directory(&self) -> &str {
            &self.current_directory
        }
    }

    /// A bundler resolver on the OS file system of this thread, linked to
    /// `shared` when given.
    fn test_resolver(dir: &str, shared: Option<SharedResolutionLink>) -> DefaultResolver {
        let host: Rc<dyn ResolutionHost> = Rc::new(TestHost {
            fs: osvfs_fs(),
            current_directory: dir.to_string(),
        });
        let options = Rc::new(CompilerOptions {
            module: ModuleKind::ES_NEXT,
            module_resolution: ModuleResolutionKind::BUNDLER,
            ..Default::default()
        });
        let mut resolver = new_resolver(ResolverOptions {
            host: Some(host),
            compiler_options: Some(options),
            ..Default::default()
        });
        resolver.caches.shared = shared;
        resolver
    }

    /// The imports of `src/deep/index.ts`. `@types/node` is a type
    /// reference directive.
    const NAMES: &[&str] = &[
        "pkg-a",
        "pkg-a/sub",
        "@scope/pkg-b",
        "missing-pkg",
        "@scope/missing",
        "./local",
        "@types/node",
    ];

    fn resolve(resolver: &DefaultResolver, dir: &str, names: &[&str]) {
        let containing_file = format!("{dir}/src/deep/index.ts");
        for name in names {
            if let Some(type_reference) = name.strip_prefix("@types/") {
                let _ = resolver.resolve_type_reference_directive(
                    type_reference,
                    &containing_file,
                    ModuleKind::ES_NEXT,
                    None,
                );
            } else {
                let _ =
                    resolver.resolve_module_name(name, &containing_file, ModuleKind::ES_NEXT, None);
            }
        }
    }

    /// The package.json cache entries that the build info reads, sorted.
    fn package_json_entries(resolver: &DefaultResolver) -> Vec<(String, String, bool, bool)> {
        let mut entries = Vec::new();
        resolver.package_json_cache_entries(|key, entry| {
            entries.push((
                key.0.clone(),
                entry.package_directory.to_string(),
                entry.directory_exists,
                entry.exists,
            ));
            true
        });
        entries.sort();
        entries
    }

    /// The loader's resolver takes every answer from parse workers on
    /// other threads, and its package.json cache then holds the same
    /// entries as a resolver that resolved all names itself (the one Go
    /// cache of all parse tasks).
    #[test]
    fn loader_takes_worker_package_json_lookups() {
        let root = std::env::temp_dir().join(format!(
            "ts_goport_shared_package_jsons_{}",
            std::process::id()
        ));
        let files = [
            ("src/deep/local.ts", "export {};"),
            (
                "node_modules/pkg-a/package.json",
                r#"{ "name": "pkg-a", "version": "1.0.0", "types": "index.d.ts" }"#,
            ),
            ("node_modules/pkg-a/index.d.ts", "export {};"),
            ("node_modules/pkg-a/sub/index.d.ts", "export {};"),
            (
                "node_modules/@scope/pkg-b/package.json",
                r#"{ "name": "@scope/pkg-b", "version": "2.0.0", "types": "lib/b.d.ts" }"#,
            ),
            ("node_modules/@scope/pkg-b/lib/b.d.ts", "export {};"),
            (
                "node_modules/@types/node/package.json",
                r#"{ "name": "@types/node", "version": "20.0.0" }"#,
            ),
            ("node_modules/@types/node/index.d.ts", "export {};"),
        ];
        for (path, text) in files {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        let dir = root.to_string_lossy().replace('\\', "/");

        let alone = test_resolver(&dir, None);
        resolve(&alone, &dir, NAMES);
        let expected = package_json_entries(&alone);
        assert!(expected.iter().any(|entry| entry.3), "{expected:?}");
        assert!(
            expected
                .iter()
                .any(|entry| !entry.3 && entry.0.contains("/node_modules/")),
            "{expected:?}"
        );

        let shared = Arc::new(SharedResolutionCache::default());
        std::thread::scope(|scope| {
            for names in NAMES.chunks(3) {
                let link = SharedResolutionLink {
                    cache: shared.clone(),
                    publish: true,
                };
                let dir = dir.as_str();
                scope.spawn(move || resolve(&test_resolver(dir, Some(link)), dir, names));
            }
        });
        let loader = test_resolver(
            &dir,
            Some(SharedResolutionLink {
                cache: shared,
                publish: false,
            }),
        );
        resolve(&loader, &dir, NAMES);
        // Every answer came from a worker, so the loader made no lookup.
        let mut own = 0;
        loader.package_json_info_cache.range(|_, _| {
            own += 1;
            true
        });
        assert_eq!(own, 0);
        let entries = package_json_entries(&loader);
        std::fs::remove_dir_all(&root).unwrap();
        assert_eq!(entries, expected);
    }

    /// Go `InfoCache.Set` keeps the first entry of each package.json
    /// (packagejson/cache.go:190), and Go's parse tasks and loader share
    /// that one cache. The parse workers' shared cache keeps the first read
    /// of each file (`store_package_json_read`). The loader's cache keeps the
    /// entry that it has when it takes worker reads
    /// (`adopt_worker_package_jsons`): its own earlier read, or the first
    /// worker read that it took, in lookup order.
    #[test]
    fn package_json_reads_keep_the_first() {
        let root = std::env::temp_dir().join(format!(
            "ts_goport_package_json_first_{}",
            std::process::id()
        ));
        std::fs::create_dir_all(root.join("b")).unwrap();
        std::fs::write(root.join("b/package.json"), r#"{ "name": "b-loader" }"#).unwrap();
        let dir = root.to_string_lossy().replace('\\', "/");
        let read = |package: &str, name: &str| WorkerPackageJsonRead {
            package_directory: format!("{dir}/{package}"),
            directory_exists: true,
            taken: std::sync::atomic::AtomicBool::new(false),
            text: std::sync::Mutex::new(Some(format!(r#"{{ "name": "{name}" }}"#).into())),
        };
        let lookup = |read: Arc<WorkerPackageJsonRead>| PackageJsonLookup {
            package_directory: read.package_directory.clone(),
            directory_exists: true,
            exists: true,
            read: Some(read),
        };

        let shared = SharedResolutionCache::default();
        let a = format!("{dir}/a/package.json");
        let first = shared.store_package_json_read(&a, || read("a", "a-first"));
        let second = shared.store_package_json_read(&a, || read("a", "a-second"));
        assert!(
            Arc::ptr_eq(&first, &second),
            "the workers keep the first read"
        );

        let loader = test_resolver(&dir, None);
        let name = |package: &str| {
            loader
                .get_package_scope_for_path(&format!("{dir}/{package}"))
                .and_then(|entry| entry.contents.clone())
                .map(|contents| contents.fields.header_fields.name.get_value().0)
        };
        // The loader's own read of b comes before the adoption.
        assert_eq!(name("b").as_deref(), Some("b-loader"));
        std::fs::remove_dir_all(&root).unwrap();
        loader.adopt_worker_package_jsons(&[
            lookup(first),
            lookup(second),
            lookup(Arc::new(read("b", "b-worker"))),
            lookup(Arc::new(read("c", "c-first"))),
            lookup(Arc::new(read("c", "c-second"))),
        ]);
        assert_eq!(
            [name("a"), name("b"), name("c")],
            [
                Some("a-first".to_string()),
                Some("b-loader".to_string()),
                Some("c-first".to_string())
            ]
        );
    }

    /// The loader's wait for a key in a worker's hands (`wait_or_take`): a
    /// free key is the loader's, an ended key needs no sleep, and a key
    /// that a worker holds past the spin makes the loader sleep until the
    /// worker's `done`. The load stats count the waits, the sleeps and the
    /// time.
    #[test]
    fn loader_waits_spin_then_sleep() {
        let mut keys = KeyList::default();
        for name in ["./a", "./b", "./c"] {
            keys.push("/p/", name, ResolutionMode::NONE);
        }
        let queue = Arc::new(AheadQueue::new(Arc::new(keys)));
        let mut stats = AheadStats::default();

        // Key 0 is free: the loader takes it, and no worker starts it.
        assert!(!queue.wait_or_take(0, &mut stats));
        assert_eq!(stats, AheadStats::default());
        assert_eq!(queue.take_next().map(|(index, _)| index), Some(1));

        // Key 1 ended: the answer is there, with no sleep.
        queue.done(1);
        assert!(queue.wait_or_take(1, &mut stats));
        assert_eq!((stats.waited, stats.slept), (1, 0));

        // Key 2 stays with a worker until the loader sleeps: the loader
        // spins, then sleeps until the worker ends it. The worker waits for
        // the loader's `waiting` mark, which the loader sets only after the
        // spin, so the order does not depend on the host's load. The 5 ms
        // after the mark is the least time that the wait can count. If the
        // mark does not come in 60 s (the loader did not sleep), the worker
        // still ends the key, so the loader cannot hang, and the test fails.
        assert_eq!(queue.take_next().map(|(index, _)| index), Some(2));
        let worker = {
            let queue = queue.clone();
            std::thread::spawn(move || {
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
                let mut marked = true;
                while queue.waiting.load(std::sync::atomic::Ordering::SeqCst) != 2 {
                    if std::time::Instant::now() > deadline {
                        marked = false;
                        break;
                    }
                    std::thread::yield_now();
                }
                if marked {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                queue.done(2);
                marked
            })
        };
        assert!(queue.wait_or_take(2, &mut stats));
        assert!(
            worker.join().unwrap(),
            "the loader set no waiting mark in 60 s"
        );
        assert_eq!((stats.waited, stats.slept), (2, 1));
        assert!(stats.wait_ns >= 5_000_000, "{stats:?}");
    }
}
