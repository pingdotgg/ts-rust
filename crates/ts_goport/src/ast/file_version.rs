//! Rust-only: the owner and liveness of freeable file versions (lsshells
//! M3a, M3b, M3c).
//!
//! Go frees an `*ast.SourceFile` when no program and no parse cache entry
//! holds it (project/snapshot.go:537 `dispose`: `parseCache.Deref` for each
//! file of a released program). Here a static published file store is
//! leaked (`ast/store.rs`), so its id stays readable. A `FileVersion` gives
//! a file version the Go lifetime:
//! - its parse holds it (`ParsedSourceFile::version`), so each frontend
//!   program (`NewProgram`) and each parse cache entry that has the parse
//!   holds it;
//! - the tables of each program version that has the file hold it
//!   (`program::VersionTables`), so a checker, bind, emit or search thread
//!   that was seeded from that version holds it too (`WorkerSeed`);
//! - a thread that reads it pins it (`with_file_version`, `PINS`) until the
//!   next program release, or the next release of a source file lease that
//!   held the last parse holder (`release_file_version_pins`), or its end;
//! - a `FileRef` guard holds it while the guard lives;
//! - the registry here keeps only a `Weak`.
//!
//! M3b: at publish the version takes its `FileStore` and its `GoFile`
//! (info, `node_bind`, `file_bind`, `flow_nodes`)
//! (`ast::store::VersionStore`). Its node records and kids are in its node
//! shell, the registry block of its id, so the header and child reads stay
//! inline (`ast::store::node_shell`); its foreign parents are leaked there,
//! and the child link column is dropped. AST node records, step 4: the
//! records and kids are in a pooled block that the version gives back when
//! it dies; after two more pin releases (`pin_epoch`: program releases, and
//! lease releases that free a parse) a later node shell can take it
//! (`ast::store::BlockPool`).
//! M3c (owned nodes; on by default in a language server or API process,
//! see `owned_nodes_enabled`): its parse was a freeable parse
//! (`ast::enter_freeable_parse`), so its store owns its astdata nodes (node
//! structs and data boxes), its
//! pending lists and its parse lists (`ast::store::OwnedAst`), and they go
//! with the version. The node shell has no node column: a node data read
//! of the file is a scoped read of the pinned version
//! (`ast::with_scoped_store_node`), and a list of its node data is a handle
//! (`ast::StoreList`) that is read at each use.
//! M3d: the binder lineage binds the version into symbol and table chunks
//! of its own. After the version dies, the next bind frees them
//! (`program::Lineage`), and each program copy of the lineage lets go of
//! them when its release frees its tables (M2c, `program::bound_symbols`).
//! When the last holder lets go, the version is dead: its id goes to
//! `DEAD_FILES` and leaves the registry, its store and `GoFile` are freed
//! (after it, on a free thread, when a pin release of the stdio API or
//! `tsc -b -w` takes its data: `take_data`), and each per-file thread-local
//! map (`PerFileMap`) forgets the entries of that id when it is next
//! written. A later read of its store, `GoFile` or node data panics
//! with "file version N is released": ids are never reused, so a missed
//! holder panics there and never reads another file. A header or child
//! read of its node shell reads its pooled block: the data of that node
//! until another version takes the block, then that version's data. With
//! debug assertions that read panics with the same message (the owner
//! check, `ast::store::file_block`). A release build checks the owner only
//! in the binder field reads (`check_block_owner`), so there a stale
//! header or child read gives wrong data and no panic. The check in every
//! `file_block` read (one load, compare and branch) costs too much for a
//! release build (watchfix1 item 5, instructions:u of stable builds):
//! `goport -p --singleThreaded` query +7.9%, hono +7.9%, zod +7.1%, effect
//! +10.2% (+6.7% to +9.7% on the default threads), and the editor long
//! sessions query-core +9.6%, hono +4.5%, effect +9.3%. A correct reader
//! never sees a reuse: every holder of a node holds its version, and a
//! given-back block waits two pin releases (`BlockPool`). No standing run
//! has debug assertions: the protected tests run `--release`
//! (`build-goport-tests.sh`), and the corpus, editor and oracle runs use
//! release bins. A debug test build checks each `file_block` read.
//!
//! Freeable rule (`free_file_versions`, `freeable_path`): only a parse of
//! a path that a publish on this thread published before, in a language
//! server or API process (the parse cache, project/parsecache.rs) or a
//! `tsc --watch` or `tsc -b --watch` process (each build,
//! `program::mark_freeable_parses`; watchfree1). An API source file lease
//! notes its path before its parse (apimem1 fix C,
//! `project::SnapshotHost::acquire_source_file`), so the first version of a
//! leased path gets one. So the first publish, the first version of each
//! file (but a leased one) and every other CLI publish never get a
//! `FileVersion`.
//! `GOPORT_FREE_FILE_VERSIONS=0` turns it off (the behavior before M3a);
//! `=1` turns it on in any process, and then
//! `program::update_program_version` (`goport_multiprog`) applies the same
//! rule to its new parses (the compiler host opens the freeable parse scope
//! for them, M3c). A parse that a parse worker made (prefetch) keeps its
//! nodes in the leaked AST arena; its version still frees its store and
//! `GoFile`. Watch mode parses ahead in its first build and after a config
//! change or an overflow; there a worker parse of a published path owns
//! its nodes (`CompilerHost::freeable_worker_parses`, watchcfg1).

use super::store::VersionStore;
use crate::prelude::*;
use std::cell::Cell;
use std::ops::Deref;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError, Weak};

/// The owner of one freeable file version. It is `Send + Sync`: a program
/// version's tables carry it to worker threads.
pub struct FileVersion {
    /// The file id (the store id).
    file: usize,
    /// The store and `GoFile` of the version, set by the publish (M3b).
    published: OnceLock<VersionStore>,
    /// Go `SourceFile.nameTable` of this version (ast.go:2532 to 2543;
    /// `ast::source_file_get_name_table`). A static file keeps it in the
    /// thread-local `NAME_TABLES` (node.rs).
    pub(crate) name_table: OnceLock<FxHashMap<String, i32>>,
    /// Go `SourceFile.positionMap` (`ast::source_file_get_position_map`).
    pub(crate) position_map: OnceLock<PositionMap>,
    /// Go `SourceFile.declarationMap`
    /// (`ast::source_file_get_declaration_map`).
    pub(crate) declaration_map: OnceLock<FxHashMap<String, Vec<Node>>>,
    /// Go `SourceFile.identifiers` (`ast::source_file_has_identifier`).
    pub(crate) identifiers: OnceLock<FxHashSet<&'static str>>,
}

impl std::fmt::Debug for FileVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileVersion")
            .field("file", &self.file)
            .field("published", &self.published.get().is_some())
            .finish()
    }
}

impl FileVersion {
    /// The owner of file version `file`, registered in the registry. The
    /// language server parse cache and `program::mark_freeable_parses`
    /// make it (`freeable_path`).
    pub fn new(file: usize) -> Arc<Self> {
        let version = Arc::new(FileVersion {
            file,
            published: OnceLock::new(),
            name_table: OnceLock::new(),
            position_map: OnceLock::new(),
            declaration_map: OnceLock::new(),
            identifiers: OnceLock::new(),
        });
        lock(&VERSIONS).insert(file, Arc::downgrade(&version));
        MADE.fetch_add(1, Ordering::Relaxed);
        version
    }

    /// The file id (the store id) of this version.
    #[must_use]
    pub fn file(&self) -> usize {
        self.file
    }

    /// Gives the version its published store and `GoFile`
    /// (`publish_file_stores`). Panics when it has them already.
    pub(crate) fn set_published(&self, store: VersionStore) {
        FREEABLE_PUBLISHED.store(true, Ordering::Release);
        assert!(
            self.published.set(store).is_ok(),
            "file version {} is already published",
            self.file
        );
    }

    /// The published store and `GoFile`, or `None` before the publish.
    #[inline]
    pub(crate) fn published(&self) -> Option<&VersionStore> {
        self.published.get()
    }

    /// The node count of the version (its parser flags), 0 before the
    /// publish.
    fn node_count(&self) -> usize {
        self.published()
            .map_or(0, |store| store.go_file().parser_flags.len())
    }

    /// Takes the data out of a version that dies (`PinRelease`): its store
    /// and `GoFile`, name table, position map, declaration map and
    /// identifier set. The version then drops with nothing to free.
    fn take_data(&mut self) -> impl Send + use<> {
        (
            self.published.take(),
            self.name_table.take(),
            self.position_map.take(),
            self.declaration_map.take(),
            self.identifiers.take(),
        )
    }

    /// The `GoFile` of this version. Panics before the publish. A
    /// `FileRef::Pinned` getter starts here.
    #[inline]
    #[must_use]
    pub fn go_file(&self) -> &GoFile {
        match self.published.get() {
            Some(store) => store.go_file(),
            None => panic!("file version {} is not published", self.file),
        }
    }
}

impl Drop for FileVersion {
    // The store and `GoFile` (`published`) are freed after this, with the
    // fields, unless a `PinRelease` took them out (`take_data`).
    //
    // This runs after the strong count is 0, so a reader on another thread
    // can fail to upgrade the registry entry before the id is in
    // `DEAD_FILES`. The reader does not ask `DEAD_FILES` then: a registry
    // entry that does not upgrade is a dead version (`pin_file_version`).
    // After the entry goes, `DEAD_FILES` has the id. So a read of this
    // version panics (`released`) from the moment the count is 0.
    fn drop(&mut self) {
        {
            let mut dead = lock(&DEAD_FILES);
            dead.push(self.file);
            DEAD_COUNT.store(dead.len(), Ordering::Release);
        }
        // File ids are never reused, so the entry is this version's.
        lock(&VERSIONS).remove(&self.file);
    }
}

/// The live file versions by file id. A `Weak`, so the registry does not
/// keep a version alive.
static VERSIONS: Mutex<FxHashMap<usize, Weak<FileVersion>>> =
    Mutex::new(FxHashMap::with_hasher(rustc_hash::FxBuildHasher));

/// The ids of the dead file versions, in the order they died. `PerFileMap`
/// reads the ids after the length it saw last.
// PERF: one id per edit, so it grows by 8 bytes per edit.
static DEAD_FILES: Mutex<Vec<usize>> = Mutex::new(Vec::new());

/// The length of `DEAD_FILES` (the dead-file epoch). A map that saw this
/// length has nothing to forget.
static DEAD_COUNT: AtomicUsize = AtomicUsize::new(0);

/// The number of file versions made in this process.
static MADE: AtomicUsize = AtomicUsize::new(0);

/// Set by `project::new_session`: this process runs the language server or
/// the API.
static EDITOR_PROCESS: AtomicBool = AtomicBool::new(false);

/// Set by `Watcher::start` and `Orchestrator::start`: this process runs
/// `tsc --watch`, with or without `--build`.
static WATCH_PROCESS: AtomicBool = AtomicBool::new(false);

/// Set when the first freeable version is published. Until then a registry
/// read (`with_version_store` in `ast/store.rs`) never looks for a version.
static FREEABLE_PUBLISHED: AtomicBool = AtomicBool::new(false);

/// Raised by each pin release (`release_file_version_pins`). A thread
/// whose pins are from an older epoch drops them at its next pinned read.
static PIN_EPOCH: AtomicUsize = AtomicUsize::new(0);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Marks this process as a language server or API process, where
/// `free_file_versions` and owned nodes are on by default.
/// `project::new_session` and `api::new_standalone_session` call it.
pub fn set_editor_process() {
    EDITOR_PROCESS.store(true, Ordering::Relaxed);
}

/// Marks this process as a `tsc --watch` process (with or without
/// `--build`), where `free_file_versions` and owned nodes are on by
/// default, as in a language server. `Watcher::start` and
/// `Orchestrator::start` call it before the first build.
// PORT: not in Go. Go frees an old `*ast.SourceFile` of a watch rebuild
// with its GC (execute/watcher.go:482, :567). Every tsc command of the
// go_baselines tests runs in a child process of its own, so the flag never
// reaches another test.
pub fn set_watch_process() {
    WATCH_PROCESS.store(true, Ordering::Relaxed);
}

/// True in a process where `free_file_versions` and owned nodes are on by
/// default: a language server or API process (`set_editor_process`) or a
/// `tsc --watch` process (`set_watch_process`).
pub(crate) fn frees_file_versions_by_default() -> bool {
    EDITOR_PROCESS.load(Ordering::Relaxed) || WATCH_PROCESS.load(Ordering::Relaxed)
}

/// True when this process frees the file versions that it publishes again.
/// `GOPORT_FREE_FILE_VERSIONS` is read once: `0` is off, `1` is on; else it
/// is on in a language server, API or `tsc --watch` process only
/// (`frees_file_versions_by_default`).
pub fn free_file_versions() -> bool {
    static FLAG: OnceLock<Option<bool>> = OnceLock::new();
    let flag = *FLAG.get_or_init(
        || match std::env::var("GOPORT_FREE_FILE_VERSIONS").as_deref() {
            Ok("0") => Some(false),
            Ok("1") => Some(true),
            _ => None,
        },
    );
    flag.unwrap_or_else(frees_file_versions_by_default)
}

thread_local! {
    /// The paths that a publish on this thread published, while
    /// `free_file_versions` is on (`note_published_path`). Shared with the
    /// parse workers of a load (`published_paths`); a new path copies the
    /// set while a load holds it.
    static PUBLISHED_PATHS: RefCell<Arc<FxHashSet<String>>> = RefCell::default();
}

/// Records that a publish on this thread published a file at `path`. Does
/// nothing while `free_file_versions` is off.
pub fn note_published_path(path: &str) {
    if !free_file_versions() {
        return;
    }
    PUBLISHED_PATHS.with(|paths| {
        let mut paths = paths.borrow_mut();
        if !paths.contains(path) {
            Arc::make_mut(&mut paths).insert(path.to_string());
        }
    });
}

/// The paths whose new parse on this thread is a freeable file version
/// (`freeable_path`), for the parse workers of a program load, which
/// cannot read this thread's state. Empty while `free_file_versions` is off.
pub fn published_paths() -> Arc<FxHashSet<String>> {
    if !free_file_versions() {
        return Arc::default();
    }
    PUBLISHED_PATHS.with(|paths| paths.borrow().clone())
}

/// The freeable rule: true when a new parse of `path` gets a
/// `FileVersion`, because `free_file_versions` is on and a publish on this
/// thread published `path` before.
pub fn freeable_path(path: &str) -> bool {
    free_file_versions() && PUBLISHED_PATHS.with(|paths| paths.borrow().contains(path))
}

/// The live versions of `files` (file ids). Static files and dead versions
/// have none.
pub(crate) fn live_file_versions(files: impl Iterator<Item = usize>) -> Vec<Arc<FileVersion>> {
    let weak: Vec<Weak<FileVersion>> = {
        let versions = lock(&VERSIONS);
        if versions.is_empty() {
            return Vec::new();
        }
        files
            .filter_map(|file| versions.get(&file).cloned())
            .collect()
    };
    // Upgraded after the lock ends: the last drop of a version locks it.
    weak.iter().filter_map(Weak::upgrade).collect()
}

/// The live file versions that `diagnostics` point at: the file of each
/// diagnostic, of its related information and of its message chain. Empty
/// until a freeable version is published (`any_freeable_published`).
/// `tsc --watch` keeps them with a diagnostic that it reads after the
/// program that made it is released (a copied diagnostic, a `tsc -b` task
/// error), as Go's GC keeps an `*ast.SourceFile` that an `*ast.Diagnostic`
/// points at.
pub(crate) fn diagnostic_file_versions<'a>(
    diagnostics: impl IntoIterator<Item = &'a crate::core::Diagnostic>,
) -> Vec<Arc<FileVersion>> {
    fn note(d: &crate::core::Diagnostic, files: &mut Vec<usize>) {
        if !d.file.is_nil() {
            files.push(d.file.file_index());
        }
        for d in d.message_chain.iter().chain(&d.related_information) {
            note(d, files);
        }
    }
    if !any_freeable_published() {
        return Vec::new();
    }
    let mut files = Vec::new();
    for d in diagnostics {
        note(d, &mut files);
    }
    files.sort_unstable();
    files.dedup();
    live_file_versions(files.into_iter())
}

/// True once a freeable version is published in this process. A CLI
/// process never sets it, so its registry reads never look for a version.
#[inline]
pub(crate) fn any_freeable_published() -> bool {
    FREEABLE_PUBLISHED.load(Ordering::Acquire)
}

/// True when `file` was a freeable version that died.
// PERF: a scan, on the path of a read that found no live version only.
fn is_dead_file(file: usize) -> bool {
    lock(&DEAD_FILES).contains(&file)
}

/// The pin epoch: the number of pin releases so far
/// (`release_file_version_pins`). A pooled node block given back at epoch
/// `e` is free from epoch `e + 2` (`ast::store::BlockPool`).
#[inline]
pub(crate) fn pin_epoch() -> usize {
    PIN_EPOCH.load(Ordering::Acquire)
}

/// Panics for a read of dead file version `file`.
#[cold]
#[inline(never)]
pub(crate) fn released(file: usize) -> ! {
    panic!("file version {file} is released")
}

/// A pin of a file version on one thread: the `Arc` in a thread-local
/// `Rc`. The pins of a thread (`PINS`) and the `FileRef` guards that it
/// made share it.
// PERF: lsshells M3 repair. A guard clones and drops the `Rc` (no atomic
// write); a flow walk of the edited file makes one guard per flow node, and
// an `Arc` clone per guard made query-core edits about 0.3 ms slower.
pub type VersionPin = Rc<Arc<FileVersion>>;

/// The pins of one thread: the versions it read since the last program
/// release (`PIN_EPOCH`), by file id.
struct Pins {
    epoch: usize,
    // PERF: a thread reads few freeable versions (the edited files), so a
    // scan is faster than a map.
    list: Vec<(usize, VersionPin)>,
    /// The index in `list` of the last hit, tested before the scan.
    // PERF: lsshells M3 repair. Most pinned reads in a row are of one file
    // (the edited file).
    last: Cell<usize>,
}

impl Pins {
    /// The pinned version `file` of the current epoch.
    #[inline]
    fn find(&self, file: usize) -> Option<&VersionPin> {
        if self.epoch != PIN_EPOCH.load(Ordering::Acquire) {
            return None;
        }
        match self.list.get(self.last.get()) {
            Some((id, version)) if *id == file => Some(version),
            _ => self.find_scan(file),
        }
    }

    /// `find` after a miss of the last hit.
    #[inline(never)]
    fn find_scan(&self, file: usize) -> Option<&VersionPin> {
        let index = self.list.iter().position(|(id, _)| *id == file)?;
        self.last.set(index);
        Some(&self.list[index].1)
    }
}

thread_local! {
    /// The file versions this thread pinned (`with_file_version`).
    static PINS: RefCell<Pins> = const {
        RefCell::new(Pins {
            epoch: 0,
            list: Vec::new(),
            last: Cell::new(0),
        })
    };
}

/// Runs `read` on the live version `file`, pinned on this thread. `None`
/// when `file` is no live version (a static, synthetic or unknown id).
/// Panics when `file` was a freeable version that died: a stale read never
/// reads other data. `ast::store` calls it only after the static blocks
/// missed.
// PERF: a hit is a thread-local borrow, an epoch compare and a short scan,
// with no atomic write. `read` runs while the pins are borrowed, so a read
// inside `read` that misses runs with its own `Arc` and pins nothing.
#[inline]
pub(crate) fn with_file_version<R>(file: usize, read: impl FnOnce(&FileVersion) -> R) -> Option<R> {
    let version = pinned_file_version(file)?;
    make_hot(&version);
    Some(read(&**version))
}

/// The live version `file`, pinned on this thread, as a pin that a
/// `FileRef` guard keeps. `None` and panics as `with_file_version`.
pub(crate) fn pinned_file_version(file: usize) -> Option<VersionPin> {
    let hit = PINS
        .try_with(|pins| {
            let pins = pins.try_borrow().ok()?;
            pins.find(file).map(Rc::clone)
        })
        .ok()
        .flatten();
    match hit {
        Some(version) => Some(version),
        None => pin_file_version(file),
    }
}

/// The pin miss: the version from the registry, added to this thread's
/// pins (after the pins of an older epoch are dropped).
#[cold]
#[inline(never)]
fn pin_file_version(file: usize) -> Option<VersionPin> {
    let weak = lock(&VERSIONS).get(&file).cloned();
    // Upgraded after the lock ends: the last drop of a version locks it.
    let version = match weak {
        Some(weak) => match weak.upgrade() {
            Some(version) => version,
            // The strong count is 0: the version is dead, and its `Drop`
            // may not have put its id in `DEAD_FILES` yet.
            None => released(file),
        },
        None if is_dead_file(file) => released(file),
        None => return None,
    };
    let version = Rc::new(version);
    // A hot version of an older epoch goes with the expired pins.
    if HOT_KEY.with(Cell::get).1 != PIN_EPOCH.load(Ordering::Relaxed) {
        drop_hot();
    }
    let expired = PINS
        .try_with(|pins| {
            let mut pins = pins.try_borrow_mut().ok()?;
            let epoch = PIN_EPOCH.load(Ordering::Acquire);
            let expired = if pins.epoch == epoch {
                Vec::new()
            } else {
                pins.epoch = epoch;
                std::mem::take(&mut pins.list)
            };
            pins.list.push((file, Rc::clone(&version)));
            pins.last.set(pins.list.len() - 1);
            Some(expired)
        })
        .ok()
        .flatten();
    // Dropped after the borrow ends: the last pin of a version frees it.
    drop(expired);
    Some(version)
}

thread_local! {
    /// The key of `HOT`: the file id and the pin epoch of its version, or
    /// `usize::MAX` for no version. It has no `Drop`, so a test of it is one
    /// thread-local load (`is_hot`).
    static HOT_KEY: Cell<(usize, usize)> = const { Cell::new((usize::MAX, usize::MAX)) };
    /// The hot version of this thread (lsshells M3f): the published
    /// freeable version of its last pinned read, pinned (a clone of its pin
    /// in `PINS`). The reads of that version test `is_hot` and read it with
    /// `with_hot`, with no pin lookup. It is borrowed only inside this
    /// module, never while other code runs.
    static HOT: HotSlot = const { HotSlot(RefCell::new(None)) };
}

/// The value of `HOT`. Its thread-local destructor clears `HOT_KEY`, which
/// has none, so a read during the thread's end does not see a hot version
/// that is gone.
struct HotSlot(RefCell<Option<VersionPin>>);

impl Drop for HotSlot {
    fn drop(&mut self) {
        let _ = HOT_KEY.try_with(|key| key.set((usize::MAX, usize::MAX)));
    }
}

/// True when published freeable version `file` is the hot version of this
/// thread, pinned in the current pin epoch: `with_hot` reads it.
// PERF: lsshells M3f. Two thread-local loads, one atomic load and two
// compares. A static file misses it with no further load.
#[inline(always)]
pub(crate) fn is_hot(file: usize) -> bool {
    // `try_with` is `#[inline]`; `LocalKey::with` is not, and it was not
    // inlined at the hot read sites (m3f notes, h3).
    HOT_KEY
        .try_with(Cell::get)
        .is_ok_and(|key| key == (file, PIN_EPOCH.load(Ordering::Relaxed)))
}

/// `read` on the published store of the hot version. Call it only after
/// `is_hot` gave true on this thread, with no pinned read in between.
/// `read` runs inside a shared borrow of `HOT`: a read inside it can read
/// the hot version again, and a pinned read inside it of another version
/// does not make that version hot (`make_hot_slow` skips a borrowed `HOT`).
// PERF: lsshells M3f. `LocalKey::try_with` is `#[inline]`, so a hit is two
// thread-local tests, a borrow and three loads to the store, with no call
// and no pin clone. `LocalKey::with` is not `#[inline]`: with it the reads
// called it out of line, and the calls cost more than the pin lookups they
// saved (m3f notes, h1 and h3).
#[inline(always)]
pub(crate) fn with_hot<R>(read: impl FnOnce(&VersionStore) -> R) -> R {
    let result = HOT.try_with(|hot| {
        let hot = hot.0.borrow();
        hot.as_deref()
            .and_then(|version| version.published())
            .map(read)
    });
    match result {
        Ok(Some(result)) => result,
        _ => no_hot_version(),
    }
}

/// A clone of the pin of the hot version (`with_hot`), for a guard.
#[inline(always)]
pub(crate) fn hot_pin() -> VersionPin {
    match HOT.try_with(|hot| hot.0.borrow().clone()) {
        Ok(Some(version)) => version,
        _ => no_hot_version(),
    }
}

#[cold]
#[inline(never)]
fn no_hot_version() -> ! {
    panic!("no hot file version on this thread")
}

/// Makes `version` the hot version of this thread when it is published.
#[inline]
fn make_hot(version: &VersionPin) {
    if is_hot(version.file) {
        return;
    }
    make_hot_slow(version);
}

#[cold]
#[inline(never)]
fn make_hot_slow(version: &VersionPin) {
    if version.published().is_none() {
        return;
    }
    let previous = HOT
        .try_with(|hot| {
            let mut hot = hot.0.try_borrow_mut().ok()?;
            let previous = hot.replace(Rc::clone(version));
            HOT_KEY.with(|key| key.set((version.file, PIN_EPOCH.load(Ordering::Relaxed))));
            Some(previous)
        })
        .ok()
        .flatten();
    // Dropped after the borrow ends: the last pin of a version frees it.
    drop(previous);
}

/// Drops the hot version of this thread, when `HOT` is not borrowed.
fn drop_hot() {
    let previous = HOT
        .try_with(|hot| {
            let mut hot = hot.0.try_borrow_mut().ok()?;
            HOT_KEY.with(|key| key.set((usize::MAX, usize::MAX)));
            hot.take()
        })
        .ok()
        .flatten();
    drop(previous);
}

/// Drops the pins of this thread and makes every other thread drop its
/// pins at its next pinned read. A program release calls it
/// (`program::ReleasedProgram`), so a version that only the released
/// program read dies with its other holders, and so does the release of a
/// source file lease that held the last holder of a freeable parse
/// (`release_file_version_pins_later`). The versions that the live
/// programs read are pinned again on their next read.
pub fn release_file_version_pins() {
    // Dropped after the borrow ends.
    drop(take_file_version_pins());
}

/// The pins of this thread, taken out as `release_file_version_pins` drops
/// them.
fn take_file_version_pins() -> Vec<(usize, VersionPin)> {
    // A test waits while another test reads the pin epoch.
    #[cfg(test)]
    let _turn = crate::test_lock::pin_release();
    PIN_EPOCH.fetch_add(1, Ordering::AcqRel);
    drop_hot();
    PINS.try_with(|pins| {
        let mut pins = pins.try_borrow_mut().ok()?;
        Some(std::mem::take(&mut pins.list))
    })
    .ok()
    .flatten()
    .unwrap_or_default()
}

thread_local! {
    /// True while a `PinRelease` of this thread waits for its drop
    /// (`release_file_version_pins_later`).
    static PIN_RELEASE_QUEUED: Cell<bool> = const { Cell::new(false) };
    /// `Some(min_nodes)` on a thread whose `PinRelease` frees the data of a
    /// dying version of `min_nodes` or more nodes on the free thread
    /// (`free_released_versions_in_background`).
    static FREE_IN_BACKGROUND: Cell<Option<usize>> = const { Cell::new(None) };
}

/// The node count from which the free of a dying version's data goes to the
/// free thread (`free_released_versions_in_background`): about a 60 KB file.
// PERF: (apiperf1) mini-743d. The free of a version of a 260 KB file (about
// 70,000 nodes) took about 1 ms of each releaseSourceFile. For small texts
// (about 150 nodes) the free thread made a loop of 3,000 leases 3 to 6%
// slower: the next parse reused memory that the other thread freed.
const BACKGROUND_FREE_MIN_NODES: usize = 10_000;

/// `release_file_version_pins` when the value drops.
struct PinRelease;

impl Drop for PinRelease {
    // Go has no counterpart: Go's GC frees the leased `*ast.SourceFile`
    // after api/session.go:1906 `lease.Release()` (handleReleaseSourceFile,
    // :1893; project/snapshothost.go:47 `cache.Deref`). The free thread
    // and the free at the thread's end are Rust-only.
    fn drop(&mut self) {
        let _ = PIN_RELEASE_QUEUED.try_with(|queued| queued.set(false));
        let pins = take_file_version_pins();
        // At the thread's end (the release waited in the garbage of
        // `gostd::local`, and the thread ended before `drop_garbage`), the
        // pins drop here, so the end does not start the free thread.
        let min_nodes = match FREE_IN_BACKGROUND.try_with(Cell::get) {
            Ok(Some(min_nodes)) if !crate::gostd::local::is_ending() => min_nodes,
            _ => {
                drop(pins);
                return;
            }
        };
        let mut garbage = Vec::new();
        for (_, pin) in pins {
            // A pin that a `FileRef` guard of this thread shares drops here:
            // the guard keeps the version.
            let Ok(version) = Rc::try_unwrap(pin) else {
                continue;
            };
            // The version dies here, before the send.
            let Some((nodes, data)) = kill_and_take_data(version) else {
                continue;
            };
            if nodes >= min_nodes {
                garbage.push(data);
            }
            // Else its data drops here.
        }
        if !garbage.is_empty() {
            free_in_background(Box::new(garbage));
        }
    }
}

/// Not in Go (perf): when this is the last holder of `version`, the version
/// dies here and gives its node count and its data (`take_data`), for a
/// free on a free thread. `try_unwrap` takes the strong count from 1 to 0
/// in one step, so after it a registry upgrade fails on every thread, and
/// the version's `Drop` here puts its id in `DEAD_FILES` and takes it out
/// of the registry before the caller sends the data: a read of it panics
/// (`released`) from now on. `None` when another holder keeps the version
/// (a guard, or the tables of a program on another thread): the `Arc`
/// drops here as usual, and the version dies with its last holder.
fn kill_and_take_data(version: Arc<FileVersion>) -> Option<(usize, impl Send + use<>)> {
    let mut version = Arc::try_unwrap(version).ok()?;
    // Before `take_data`, which takes the store that it reads.
    let nodes = version.node_count();
    let data = version.take_data();
    drop(version);
    Some((nodes, data))
}

/// Not in Go (perf, followups32): the data of each version of `versions`
/// whose last holder this is, for a free on a free thread. Each such
/// version dies here (`kill_and_take_data`), so its id is dead before the
/// send, as in a pin release (`PinRelease`). `tsc -b -w` frees the kept
/// parses that a config change drops this way
/// (`BuildHost::drop_kept_parses_whose_module_indicator_options_change`),
/// as Go's GC frees the files of the old program.
pub(crate) fn take_data_of_dying_versions(
    versions: Vec<Arc<FileVersion>>,
) -> Vec<impl Send + use<>> {
    versions
        .into_iter()
        .filter_map(kill_and_take_data)
        .map(|(_, data)| data)
        .collect()
}

/// Not in Go (perf, apiperf1): on this thread, the pin release of a source
/// file lease release (`release_file_version_pins_later`) gives the data of
/// each version that dies in it with `BACKGROUND_FREE_MIN_NODES` or more
/// nodes to the free thread (`free_in_background`), so it is freed there,
/// beside the next requests, as Go's GC frees the leased
/// `*ast.SourceFile`. The version itself dies in the release: its id is
/// dead before the send, as with a free in the release. The stdio API
/// server calls it with `gostd::local::keep_garbage`. Elsewhere the free
/// runs in the release, so a test sees the data go at `drop_garbage`.
pub fn free_released_versions_in_background() {
    FREE_IN_BACKGROUND.with(|min_nodes| min_nodes.set(Some(BACKGROUND_FREE_MIN_NODES)));
}

/// Not in Go (perf, apiperf1): drops `garbage` on a thread that drops the
/// values sent to it in order, or here when that thread cannot start
/// (wasm32-wasip1 has no threads). A value still queued at exit is not
/// dropped. `execute::build::build_task::drop_in_background` is the same
/// kind of thread (`goport-free`, a thread of its own) for the data of a
/// build.
fn free_in_background(garbage: Box<dyn Send>) {
    if cfg!(target_family = "wasm") {
        drop(garbage);
        return;
    }
    static QUEUE: OnceLock<Option<std::sync::mpsc::Sender<Box<dyn Send>>>> = OnceLock::new();
    let queue = QUEUE.get_or_init(|| {
        let (send, receive) = std::sync::mpsc::channel::<Box<dyn Send>>();
        std::thread::Builder::new()
            .name("goport-free-versions".to_string())
            .spawn(move || receive.into_iter().for_each(drop))
            .ok()
            .map(|_| send)
    });
    let unsent = match queue {
        Some(send) => send.send(garbage).err().map(|err| err.0),
        None => Some(garbage),
    };
    drop(unsent);
}

/// Not in Go: `release_file_version_pins` after the answer
/// (`gostd::local::drop_later`; at once on a thread that does not keep
/// garbage). The release of a source file lease whose freeable parse has no
/// other holder calls it (`project::drop_released_lease`), so the version
/// dies with the lease, as Go's GC frees the leased `*ast.SourceFile`; its
/// data is freed on the free thread when this thread frees in the
/// background (`free_released_versions_in_background`). One release waits
/// at a time: the leases that one message releases (a session close) bump
/// the pin epoch once.
pub fn release_file_version_pins_later() {
    if PIN_RELEASE_QUEUED.replace(true) {
        return;
    }
    crate::gostd::local::drop_later(Box::new(PinRelease));
}

/// A borrow of per-file data (lsshells M3b), the return type of the file
/// data accessors (`ast::go_file`, `ast::source_file_info`,
/// `FlowNodeId::get_flow`, ...). It derefs to the data.
/// - `Static`: a file that is never freed (a static publish, synthetic or
///   leaked data). `as_static` gives the `'static` borrow.
/// - `Pinned`: a freeable file version. The guard holds the version (a
///   thread-local pin, `VersionPin`, so a guard stays on its thread), and
///   the data lives while the guard does. `get(version, key)` finds the
///   data; it is a plain function (no captures), `key` is its argument (a
///   slot or flow index, or 0).
///
/// Keep a guard only as long as the data is needed: a kept guard keeps its
/// file version alive. To keep a value past the guard, copy or clone it.
pub enum FileRef<T: ?Sized + 'static> {
    Static(&'static T),
    Pinned {
        version: VersionPin,
        key: usize,
        get: fn(&FileVersion, usize) -> &T,
    },
}

impl<T: ?Sized + 'static> FileRef<T> {
    /// The `'static` borrow of a static file, or `None` for a pinned
    /// version.
    #[inline]
    #[must_use]
    pub fn as_static(&self) -> Option<&'static T> {
        match self {
            FileRef::Static(value) => Some(value),
            FileRef::Pinned { .. } => None,
        }
    }

    /// The file version that a pinned guard holds, or `None` for a static
    /// file.
    #[must_use]
    pub fn version(&self) -> Option<&VersionPin> {
        match self {
            FileRef::Static(_) => None,
            FileRef::Pinned { version, .. } => Some(version),
        }
    }
}

impl<T: ?Sized + 'static> Deref for FileRef<T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &T {
        match self {
            FileRef::Static(value) => value,
            FileRef::Pinned { version, key, get } => get(&**version, *key),
        }
    }
}

impl<T: ?Sized + 'static> Clone for FileRef<T> {
    fn clone(&self) -> Self {
        match self {
            FileRef::Static(value) => FileRef::Static(value),
            FileRef::Pinned { version, key, get } => FileRef::Pinned {
                version: Rc::clone(version),
                key: *key,
                get: *get,
            },
        }
    }
}

impl<T: 'static> Default for FileRef<[T]> {
    /// An empty static list.
    fn default() -> Self {
        FileRef::Static(&[])
    }
}

impl<T: ?Sized + std::fmt::Debug + 'static> std::fmt::Debug for FileRef<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        (**self).fmt(f)
    }
}

/// `FileRef` to `$part` of the `GoFile` of published file `$file` (a file
/// id). `$part` is a place expression of `$g` (the `GoFile`) and `$k` (the
/// value of `$key`; name it `_key` when `$part` does not use it). It is
/// written once and used for both variants, so it must not capture anything
/// else. Panics when `$file` is not published.
// PORT: a `FileRef::Pinned` getter must be a plain function, so the part is
// repeated in a non-capturing closure.
macro_rules! go_file_ref {
    ($file:expr, $key:expr, |$g:ident, $k:ident| $part:expr) => {{
        let key: usize = $key;
        match $crate::ast::go_file($file) {
            $crate::ast::FileRef::Static($g) => {
                let $k = key;
                $crate::ast::FileRef::Static(&$part)
            }
            $crate::ast::FileRef::Pinned { version, .. } => $crate::ast::FileRef::Pinned {
                version,
                key,
                get: |version, $k| {
                    let $g = version.go_file();
                    &$part
                },
            },
        }
    }};
}
pub(crate) use go_file_ref;

/// The text of one file (Go `SourceFile.text`). It derefs to `str`.
/// - `Static`: a file that is never freed (every CLI file, a lib, the first
///   version of an edited file, a synthetic file): the leaked text, as
///   before textleak1.
/// - `Shared`: a freeable file version (lsshells M3a). Its store, its parse
///   and its program inputs share the text, and it goes with the last of
///   them.
///
/// A parse borrows the text (`Parser<'a>`, `Scanner<'a>`), and a reader
/// holds a clone only as long as it reads: a kept clone keeps the text of a
/// dead version alive, but never reads freed memory.
// PORT: Go strings are GC values. No node data points into the text: the
// node texts are owned copies or interned names.
#[derive(Clone)]
pub enum FileText {
    Static(&'static str),
    Shared(Arc<str>),
}

impl FileText {
    /// The text of a new parse: `Shared` for a parse of a freeable file
    /// version (`freeable`, see `freeable_path`), else leaked (`Static`),
    /// as every CLI text is.
    #[must_use]
    pub fn new(text: String, freeable: bool) -> Self {
        if freeable {
            FileText::Shared(Arc::from(text))
        } else {
            FileText::Static(Box::leak(text.into_boxed_str()))
        }
    }

    /// The `'static` text of a static file, or `None` for a shared text.
    #[inline]
    #[must_use]
    pub fn as_static(&self) -> Option<&'static str> {
        match self {
            FileText::Static(text) => Some(text),
            FileText::Shared(_) => None,
        }
    }

    /// A weak handle to a shared text, or `None` for a static text. Tests
    /// use it to see that a text goes with its file version.
    #[must_use]
    pub fn weak(&self) -> Option<Weak<str>> {
        match self {
            FileText::Static(_) => None,
            FileText::Shared(text) => Some(Arc::downgrade(text)),
        }
    }
}

impl Default for FileText {
    /// The empty static text.
    fn default() -> Self {
        FileText::Static("")
    }
}

impl Deref for FileText {
    type Target = str;

    #[inline]
    fn deref(&self) -> &str {
        match self {
            FileText::Static(text) => text,
            FileText::Shared(text) => text,
        }
    }
}

impl From<&'static str> for FileText {
    #[inline]
    fn from(text: &'static str) -> Self {
        FileText::Static(text)
    }
}

impl std::fmt::Debug for FileText {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        (**self).fmt(f)
    }
}

impl std::fmt::Display for FileText {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        (**self).fmt(f)
    }
}

impl PartialEq for FileText {
    fn eq(&self, other: &Self) -> bool {
        **self == **other
    }
}

impl Eq for FileText {}

impl PartialEq<str> for FileText {
    fn eq(&self, other: &str) -> bool {
        &**self == other
    }
}

impl PartialEq<&str> for FileText {
    fn eq(&self, other: &&str) -> bool {
        &**self == *other
    }
}

/// A weak handle to a file version. Tests use it to see when the version
/// dies.
pub struct FileVersionProbe(Weak<FileVersion>);

impl FileVersionProbe {
    /// True once no holder has the version.
    #[must_use]
    pub fn is_freed(&self) -> bool {
        self.0.strong_count() == 0
    }
}

/// A probe of the version of the file of `node`. None for a static file
/// (the first publish, a first version, a CLI publish) or a dead version.
#[must_use]
pub fn file_version_probe(node: Node) -> Option<FileVersionProbe> {
    let weak = lock(&VERSIONS).get(&node.file_index()).cloned()?;
    (weak.strong_count() > 0).then_some(FileVersionProbe(weak))
}

/// The number of file versions made in this process.
#[must_use]
pub fn file_versions_made() -> usize {
    MADE.load(Ordering::Relaxed)
}

/// The number of dead file versions in this process.
#[must_use]
pub fn dead_file_versions() -> usize {
    DEAD_COUNT.load(Ordering::Acquire)
}

/// The ids of the file versions that died after the first `seen` dead ones,
/// in the order they died, and the number of dead versions now. The binder
/// lineage frees their symbol chunks (`program::Lineage`, lsshells M3d).
pub(crate) fn dead_files_since(seen: usize) -> (Vec<usize>, usize) {
    let dead = lock(&DEAD_FILES);
    (dead[seen.min(dead.len())..].to_vec(), dead.len())
}

/// A thread-local map with per-file entries: each key is a node, and the
/// entry belongs to the file of that node. It forgets the entries of dead
/// file versions when it is next written (`write`). A read (`Deref`) can
/// still find such an entry before that; its value is owned by the map, so
/// it is still valid, but a key of a dead version is never read again.
#[derive(Clone, Debug)]
pub(crate) struct PerFileMap<V> {
    /// `DEAD_COUNT` when the map last forgot dead entries.
    seen: usize,
    map: FxHashMap<Node, V>,
    /// The ids of the dead file versions that the map knows of (the first
    /// `seen` of `DEAD_FILES`), so a miss can tell a node of a dead version
    /// (`is_dead`).
    // PERF: one id per edit.
    dead: FxHashSet<usize>,
}

impl<V> PerFileMap<V> {
    pub(crate) const fn new() -> Self {
        Self {
            seen: 0,
            map: FxHashMap::with_hasher(rustc_hash::FxBuildHasher),
            dead: FxHashSet::with_hasher(rustc_hash::FxBuildHasher),
        }
    }

    /// True when `node` is a node of a dead file version. Call it after
    /// `write`, which brings the dead ids up to date.
    // PERF: no hash in a process that frees no file version.
    #[inline]
    pub(crate) fn is_dead(&self, node: Node) -> bool {
        !self.dead.is_empty() && self.dead.contains(&node.file_index())
    }

    /// The map for a write, after it forgets the entries of the file
    /// versions that died since its last write.
    // PERF: one atomic load. A process that frees no file version never
    // takes the cold path.
    #[inline]
    pub(crate) fn write(&mut self) -> &mut FxHashMap<Node, V> {
        if self.seen != DEAD_COUNT.load(Ordering::Acquire) {
            self.forget_dead();
        }
        &mut self.map
    }

    #[cold]
    #[inline(never)]
    fn forget_dead(&mut self) {
        let new: Vec<usize> = {
            let dead = lock(&DEAD_FILES);
            let new = dead[self.seen.min(dead.len())..].to_vec();
            self.seen = dead.len();
            new
        };
        self.dead.extend(new);
        // After the lock ends: a dropped entry can hold a file version.
        if !self.map.is_empty() {
            let dead = &self.dead;
            self.map
                .retain(|node, _| !dead.contains(&node.file_index()));
        }
    }
}

impl<V> Default for PerFileMap<V> {
    fn default() -> Self {
        Self::new()
    }
}

impl<V> Deref for PerFileMap<V> {
    type Target = FxHashMap<Node, V>;

    fn deref(&self) -> &FxHashMap<Node, V> {
        &self.map
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The first node of file `file`. The tests with a fixed id (`DYING`,
    /// `OTHER`) do not publish it; the others publish the file first
    /// (`publish_pinned`).
    fn node(file: usize) -> Node {
        Node(((file as u64) << 32) | 1)
    }

    // A map keeps the entries of a dead version until its next write, then
    // forgets them and keeps the entries of other files.
    #[test]
    fn per_file_map_forgets_dead_file_versions() {
        const DYING: usize = (1 << 22) - 1;
        const OTHER: usize = DYING - 1;
        let mut map = PerFileMap::new();
        map.write().insert(node(DYING), 1);
        map.write().insert(node(OTHER), 2);
        let version = FileVersion::new(DYING);
        let probe = file_version_probe(node(DYING)).expect("the registry has the version");

        drop(version);
        assert!(probe.is_freed());
        assert!(file_version_probe(node(DYING)).is_none());
        assert_eq!(map.get(&node(DYING)), Some(&1));
        map.write();
        assert_eq!(map.get(&node(DYING)), None);
        assert_eq!(map.get(&node(OTHER)), Some(&2));
    }

    /// Holds the free thread until its sender sends or drops.
    struct Blocker(std::sync::mpsc::Receiver<()>);

    impl Drop for Blocker {
        fn drop(&mut self) {
            let _ = self.0.recv();
        }
    }

    /// Starts the free thread when it has not started and blocks it: the
    /// values sent to it after this wait until the returned sender sends or
    /// drops.
    fn block_free_thread() -> std::sync::mpsc::Sender<()> {
        let (go, wait) = std::sync::mpsc::channel();
        free_in_background(Box::new(Blocker(wait)));
        go
    }

    /// Publishes on this thread a freeable version of a file of one
    /// identifier per name and pins it. Gives the version (its other
    /// holder) and a weak handle to its text, which goes with its data (its
    /// store). The thread holds the store build lock to its end
    /// (`crate::test_lock::hold`): the tests read the pin epoch and the free
    /// thread, which other tests also use.
    fn publish_pinned(name: &'static str, names: &[&str]) -> (Arc<FileVersion>, Weak<str>) {
        crate::test_lock::hold();
        let text = FileText::Shared(Arc::from(names.join(";")));
        let weak = text.weak().expect("a shared text");
        let file = super::super::store::new_file_store(name, text);
        let factory = crate::ast::NodeFactory::for_file(file);
        for identifier in names {
            factory.new_identifier(*identifier);
        }
        super::super::store::freeze_file_store(file);
        let version = FileVersion::new(file);
        crate::program::publish_parsed_files("/");
        assert!(version.node_count() > 0, "the version is published");
        drop(pinned_file_version(file).expect("the version is live"));
        (version, weak)
    }

    // apiperf1, followups30 item 3 (a): on a thread that frees in the
    // background (the stdio API server), a version dies in the pin release
    // of a lease release, also when the free thread is busy: its id is dead
    // and a read of it panics. The data of a version of `min_nodes` or more
    // nodes goes to the free thread, which frees it after the values queued
    // before it. The data of a smaller version is freed in the release. A
    // version with another holder lives until that holder lets go. Its
    // thread holds the store build lock (`publish_pinned`).
    #[test]
    fn background_pin_release_frees_on_the_free_thread() {
        // Its own thread, so the background mode stays there.
        std::thread::spawn(|| {
            let (small_version, small_text) = publish_pinned("/fv/small.ts", &["a"]);
            let (big_version, big_text) = publish_pinned("/fv/big.ts", &["a", "b", "c", "d"]);
            let (kept, kept_text) = publish_pinned("/fv/kept.ts", &["a", "b", "c", "d"]);
            let big_nodes = big_version.node_count();
            assert!(small_version.node_count() < big_nodes);
            let [small, big] = [&small_version, &big_version].map(|version| version.file());
            let probes = [small, big, kept.file()].map(|file| {
                file_version_probe(node(file)).expect("the pin of this thread holds the version")
            });
            // Only the pins of this thread hold small and big.
            drop((small_version, big_version));
            let go = block_free_thread();
            FREE_IN_BACKGROUND.with(|min_nodes| min_nodes.set(Some(big_nodes)));
            let dead = dead_file_versions();
            release_file_version_pins_later();
            assert!(
                probes[..2].iter().all(FileVersionProbe::is_freed),
                "the versions die in the release"
            );
            let (died, _) = dead_files_since(dead);
            assert!(died.contains(&small) && died.contains(&big), "{died:?}");
            assert!(!died.contains(&kept.file()), "{died:?}");
            assert!(!probes[2].is_freed(), "its other holder keeps a version");
            drop(kept);
            assert!(probes[2].is_freed(), "the last holder frees the version");
            assert!(
                kept_text.upgrade().is_none(),
                "the last holder frees the data"
            );
            let read = std::panic::catch_unwind(|| pinned_file_version(big).is_some());
            let message = read.expect_err("a read of a dead version panics");
            assert_eq!(
                message.downcast_ref::<String>(),
                Some(&format!("file version {big} is released"))
            );
            assert!(
                small_text.upgrade().is_none(),
                "the release frees the data of a small version"
            );
            assert!(
                big_text.upgrade().is_some(),
                "the data of a big version waits for the free thread"
            );
            go.send(()).expect("the free thread waits");
            let deadline = std::time::Instant::now() + std::time::Duration::from_mins(1);
            while big_text.upgrade().is_some() {
                assert!(
                    std::time::Instant::now() < deadline,
                    "the free thread frees the data"
                );
                std::thread::yield_now();
            }
        })
        .join()
        .expect("the test thread ends");
    }

    // followups30 item 3 (b): a `PinRelease` that waits in the garbage of
    // `gostd::local` when its thread ends drops its pins there, so the
    // thread's end frees the data and does not send it to the free thread
    // (a thread-local destructor must not start that thread). Its inner
    // thread holds the store build lock (`crate::test_lock::hold`) and
    // blocks the free thread. The test thread does not block it: it would
    // then wait for the inner thread, which waits for the lock, while a test
    // that holds the lock waits for the free thread.
    #[test]
    fn a_pin_release_at_the_thread_end_frees_in_place() {
        let (probe, text, go) = std::thread::spawn(|| {
            // First, so its thread-local drops last, after the version dies.
            crate::test_lock::hold();
            // Then the free thread, so it exists and the end does not start it.
            let go = block_free_thread();
            // `PINS` before `LOCAL` (`keep_garbage`): the thread-local
            // destructors run in the reverse order of the first uses, so
            // `LOCAL` drops the release while the pins are still there.
            PINS.with(|_| ());
            let (version, text) = publish_pinned("/fv/end.ts", &["a"]);
            let file = version.file();
            drop(version);
            crate::gostd::local::keep_garbage();
            FREE_IN_BACKGROUND.with(|min_nodes| min_nodes.set(Some(0)));
            release_file_version_pins_later();
            assert_eq!(
                crate::gostd::local::garbage_len(),
                1,
                "the release waits for `drop_garbage`"
            );
            let probe = file_version_probe(node(file)).expect("the pin holds the version");
            // The thread ends with no `drop_garbage`.
            (probe, text, go)
        })
        .join()
        .expect("the test thread ends");
        assert!(probe.is_freed(), "the end of the thread frees the version");
        assert!(
            text.upgrade().is_none(),
            "the end of the thread frees the data, not the free thread"
        );
        go.send(()).expect("the free thread waits");
    }

    // followups32 (followups30 item 3 (a)): a version that dies in a pin
    // release on a thread that frees in the background leaves the registry
    // in the release (`FileVersion::drop`), before its data goes to the free
    // thread, so a busy free thread leaves no entry of a dead version. Its
    // thread holds the store build lock (`publish_pinned`).
    #[test]
    fn a_pin_release_takes_a_dying_version_out_of_the_registry() {
        std::thread::spawn(|| {
            let (version, text) = publish_pinned("/fv/registry.ts", &["a"]);
            let file = version.file();
            drop(version);
            assert!(lock(&VERSIONS).contains_key(&file), "a live version");
            let go = block_free_thread();
            FREE_IN_BACKGROUND.with(|min_nodes| min_nodes.set(Some(0)));
            release_file_version_pins_later();
            assert!(
                !lock(&VERSIONS).contains_key(&file),
                "the registry has no entry of the dead version"
            );
            assert!(
                text.upgrade().is_some(),
                "the data waits for the free thread"
            );
            go.send(()).expect("the free thread waits");
            wait_for_free(&text);
        })
        .join()
        .expect("the test thread ends");
    }

    // followups32 (followups30 item 3 (a)): a pin that a `FileRef` guard of
    // this thread shares does not unwrap in a pin release (`Rc::try_unwrap`
    // fails): the pin drops there, and the guard keeps the version, which
    // stays live and in the registry. When the guard drops, the version
    // dies there, and its data is freed with it, not on the free thread. Its
    // thread holds the store build lock (`publish_pinned`).
    #[test]
    fn a_pin_release_keeps_a_version_that_a_guard_shares() {
        std::thread::spawn(|| {
            let (version, text) = publish_pinned("/fv/guard.ts", &["a"]);
            let file = version.file();
            let probe = file_version_probe(node(file)).expect("a live version");
            let guard = pinned_file_version(file).expect("the pin of this thread");
            drop(version);
            let go = block_free_thread();
            FREE_IN_BACKGROUND.with(|min_nodes| min_nodes.set(Some(0)));
            let dead = dead_file_versions();
            release_file_version_pins_later();
            assert!(!probe.is_freed(), "the guard keeps the version");
            assert!(lock(&VERSIONS).contains_key(&file), "a live version");
            // Versions of other tests can die meanwhile.
            assert!(
                !dead_files_since(dead).0.contains(&file),
                "the version did not die"
            );
            assert_eq!(guard.file(), file);
            drop(guard);
            assert!(probe.is_freed(), "the version dies with the guard");
            assert!(
                text.upgrade().is_none(),
                "the data is freed with the version, not on the free thread"
            );
            go.send(()).expect("the free thread waits");
        })
        .join()
        .expect("the test thread ends");
    }

    /// Starts the build's free thread (`build_task::drop_in_background`)
    /// when it has not started and blocks it, as `block_free_thread`.
    fn block_build_free_thread() -> std::sync::mpsc::Sender<()> {
        let (go, wait) = std::sync::mpsc::channel();
        crate::execute::build::build_task::drop_in_background(Blocker(wait));
        go
    }

    /// Waits up to 60 s for a free thread to free the data of `text`.
    fn wait_for_free(text: &Weak<str>) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_mins(1);
        while text.upgrade().is_some() {
            assert!(
                std::time::Instant::now() < deadline,
                "the free thread frees the data"
            );
            std::thread::yield_now();
        }
    }

    // followups32 item 6: `tsc -b -w` frees the kept parses that a config
    // change drops with `take_data_of_dying_versions`
    // (`BuildHost::drop_kept_parses_whose_module_indicator_options_change`):
    // a version whose last holder that is dies there, before its data goes
    // to the build's free thread, so its id is dead, it is not in the
    // registry and a read of it panics while the free thread is busy. The
    // free thread frees its data later. A version with another holder lives
    // until that holder lets go. Its thread holds the store build lock
    // (`publish_pinned`).
    #[test]
    fn a_dropped_kept_parse_dies_before_its_data_goes_to_the_free_thread() {
        std::thread::spawn(|| {
            let (version, text) = publish_pinned("/fv/kept-parse.ts", &["a"]);
            let (shared, shared_text) = publish_pinned("/fv/kept-shared.ts", &["a"]);
            let file = version.file();
            let probes = [file, shared.file()]
                .map(|file| file_version_probe(node(file)).expect("a live version"));
            // The pins of this thread go, so `version` is its last holder.
            release_file_version_pins();
            let go = block_build_free_thread();
            let dead = dead_file_versions();
            let data = take_data_of_dying_versions(vec![version, shared.clone()]);
            assert_eq!(data.len(), 1, "only a version with no other holder dies");
            crate::execute::build::build_task::drop_in_background(data);
            assert!(probes[0].is_freed(), "the version dies before the send");
            assert!(
                !lock(&VERSIONS).contains_key(&file),
                "the registry has no entry of the dead version"
            );
            // Versions of other tests can die meanwhile.
            let (died, _) = dead_files_since(dead);
            assert!(
                died.contains(&file) && !died.contains(&shared.file()),
                "{died:?}"
            );
            let read = std::panic::catch_unwind(|| pinned_file_version(file).is_some());
            let message = read.expect_err("a read of a dead version panics");
            assert_eq!(
                message.downcast_ref::<String>(),
                Some(&format!("file version {file} is released"))
            );
            assert!(
                text.upgrade().is_some(),
                "the data waits for the free thread"
            );
            assert!(!probes[1].is_freed(), "its other holder keeps a version");
            drop(shared);
            assert!(probes[1].is_freed(), "the last holder frees the version");
            assert!(
                shared_text.upgrade().is_none(),
                "the last holder frees the data"
            );
            go.send(()).expect("the free thread waits");
            wait_for_free(&text);
        })
        .join()
        .expect("the test thread ends");
    }

    // A node of a dead version whose id the map forgot gets no new id: the
    // id read panics (skeptic tripwire 1).
    #[test]
    #[should_panic(expected = "file version 4194301 is released")]
    fn node_id_of_a_dead_file_version_panics() {
        const DYING: usize = (1 << 22) - 3;
        const OTHER: usize = DYING - 1;
        let version = FileVersion::new(DYING);
        let id = crate::ast::get_node_id(node(DYING));
        drop(version);
        // Before the next write the map still has the id.
        assert_eq!(crate::ast::get_node_id(node(DYING)), id);
        // This write forgets the ids of the dead version.
        crate::ast::get_node_id(node(OTHER));
        crate::ast::get_node_id(node(DYING));
    }

    // A reader that finds a registry entry that does not upgrade (the
    // strong count is 0, but `Drop` has not put the id in `DEAD_FILES`
    // yet) panics, and does not read the id as no version (skeptic
    // tripwire 2).
    #[test]
    #[should_panic(expected = "file version 4194299 is released")]
    fn a_dying_file_version_is_released() {
        const DYING: usize = (1 << 22) - 5;
        // The state between the last strong drop and `FileVersion::drop`.
        lock(&VERSIONS).insert(DYING, Weak::new());
        assert!(!is_dead_file(DYING));
        with_file_version(DYING, |_| ());
    }
}
