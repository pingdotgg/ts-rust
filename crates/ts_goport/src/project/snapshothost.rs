//! Go `internal/project/snapshothost.go` (ts#64163).
//!
//! PORT: one thread (project/dirty/interfaces.rs). Go `*SnapshotHost` is
//! `Rc<SnapshotHost>`: every snapshot keeps its host so `Deref` can release
//! the host's caches. The Go `atomic.Uint64` snapshot id is a `Cell`. Go
//! `Session` embeds `*SnapshotHost`; the Rust `Session` holds it in
//! `snapshot_host` and derefs to it (session.rs).

use crate::project::prelude::*;

use crate::contentmapper;
use crate::frontend::parser;
use std::cell::Cell;

// Go: project/snapshothost.go:20 SnapshotHost
// SnapshotHost owns the services shared by a collection of immutable snapshots.
pub struct SnapshotHost {
    pub options: Rc<SessionOptions>,
    pub to_path: Rc<dyn Fn(&str) -> tspath::Path>,
    pub fs: Rc<dyn vfs::Fs>,

    pub parse_cache: Rc<ParseCache>,
    pub content_mapped_parse_cache: Rc<ContentMappedParseCache>,
    pub extended_config_cache: Rc<ExtendedConfigCache>,
    pub program_counter: Rc<ProgramCounter>,
    pub content_mapper_host: Option<Rc<dyn contentmapper::Host>>,

    pub snapshot_id: Cell<u64>,

    // PORT: the parse cache references that auto-import registry clones
    // keep after the clone, one per path (see
    // `AutoImportRegistryCloneHost::dispose`). No Go counterpart. It moved
    // from `Session` with the caches it belongs to.
    pub auto_import_parse_keys: Rc<AutoImportParseKeys>,

    // PORT: not in Go (perf). The resolve-ahead keys of the projects that a
    // clone made and deleted (`ResolveAheadStash`).
    pub resolve_ahead_stash: Rc<ResolveAheadStash>,
}

// Go: project/snapshothost.go:34 SourceFileLease (ts#64434)
// PORT: Go `releaseOnce sync.Once` is a `Cell<bool>` (one thread).
pub struct SourceFileLease {
    cache: Rc<ParseCache>,
    key: ParseCacheKey,
    source_file: Rc<parser::ParsedSourceFile>,
    released: Cell<bool>,
}

impl SourceFileLease {
    // Go: project/snapshothost.go:41 SourceFileLease.SourceFile
    // PORT: Go returns the `*ast.SourceFile`. This is its root node; the
    // whole file is `parsed_source_file`.
    pub fn source_file(&self) -> Node {
        self.source_file.root
    }

    // PORT: the whole file of Go `SourceFile()` (project/snapshothost.go:41):
    // the leased parse, with the Go `SourceFile` fields that are not on the
    // root node (`Hash`, `ParseOptions()`). The API encoder reads them here
    // (`encoder::encode_parsed_source_file`), as Go reads them from the
    // leased file. A lookup by the root node fails once the program that
    // loaded the file is released, while the lease still holds it.
    pub fn parsed_source_file(&self) -> &Rc<parser::ParsedSourceFile> {
        &self.source_file
    }

    // Go: project/snapshothost.go:45 SourceFileLease.Release
    pub fn release(&self) {
        if !self.released.replace(true) {
            self.cache.deref(&self.key);
        }
    }
}

/// Not in Go: drops `lease` after its `release`. When it held the last
/// holder of a freeable parse (no cache entry, other lease or program has
/// it), the pins of the parse's version go after the answer
/// (`ast::release_file_version_pins_later`), so the version dies, as Go's GC
/// frees the leased `*ast.SourceFile`. Without this, the encoder's reads keep
/// it pinned on this thread until the next program release.
// PORT: the count sees the holders on this thread only. A version that a
// worker thread still holds (a program version's tables) dies later, with
// that holder.
pub fn drop_released_lease(lease: Rc<SourceFileLease>) {
    let parsed = Rc::clone(&lease.source_file);
    drop(lease);
    let dies = parsed.version.get().is_some() && Rc::strong_count(&parsed) == 1;
    drop(parsed);
    if dies {
        crate::ast::release_file_version_pins_later();
    }
}

/// Go `logging.Logger` as the session logger argument of `Snapshot.Clone`,
/// `cloneForProgram` and `CloneSnapshotWithAutoImports`.
// PORT: Go passes the session's logger (a non-nil interface, also for the nop
// logger) or nil. `None` is the Go nil interface; `Some(logger)` is the
// session logger, which is itself `None` for the nop logger (see
// `logging::new_nop_logger`).
pub type SessionLogger<'a> = Option<&'a Option<Rc<dyn logging::Logger>>>;

impl SnapshotHost {
    // Go: project/snapshothost.go:51 SnapshotHost.nextSnapshotID
    pub fn next_snapshot_id(&self) -> u64 {
        // Go: s.snapshotID.Add(1)
        let id = self.snapshot_id.get() + 1;
        self.snapshot_id.set(id);
        id
    }

    // Go: project/snapshothost.go:55 SnapshotHost.AcquireSourceFile (ts#64434)
    pub fn acquire_source_file(
        &self,
        options: parser::SourceFileParseOptions,
        text: &str,
        script_kind: ScriptKind,
    ) -> Rc<SourceFileLease> {
        let file_handle = new_cached_file_handle(&options.file_name, text.to_string());
        let key = new_parse_cache_key(&options, file_handle.hash(), script_kind);
        // Not in Go: a new parse of a leased text is a freeable file version
        // also for a path that no publish published yet (the freeable rule,
        // `ast::freeable_path`, keeps the first version of a path static), so
        // the release of its last lease frees it (`drop_released_lease`), as
        // Go's GC frees it. The publish of the parse notes the path anyway.
        crate::ast::note_published_path(&key.path.0);
        // PORT: `acquire_bound` is Go `Acquire`, whose parse cache binds.
        let source_file = acquire_bound(
            &self.parse_cache,
            key.clone(),
            file_handle,
            &self.options.current_directory,
        )
        .file;
        Rc::new(SourceFileLease {
            cache: self.parse_cache.clone(),
            key,
            source_file,
            released: Cell::new(false),
        })
    }

    // Go: project/snapshothost.go:65 SnapshotHost.AcquireExistingSourceFile (ts#64518)
    // PORT: Go's parse cache binds a file before it stores it, so a live entry
    // is bound. Here a program load binds its files later (see
    // `new_parse_cache`), so the file is bound here, as `acquire_bound` does
    // for `acquire_source_file`. A bound file is not bound again.
    pub fn acquire_existing_source_file(&self, key: ParseCacheKey) -> Option<Rc<SourceFileLease>> {
        let source_file = self.parse_cache.acquire_existing(&key)?.file;
        crate::program::publish_parsed_files(&self.options.current_directory);
        crate::program::bind_file_outside_program(source_file.root);
        Some(Rc::new(SourceFileLease {
            cache: self.parse_cache.clone(),
            key,
            source_file,
            released: Cell::new(false),
        }))
    }
}

// Go: project/snapshothost.go:65 NewSnapshotHost
pub fn new_snapshot_host(init: &SessionInit) -> Rc<SnapshotHost> {
    let current_directory = init.options.current_directory.clone();
    let use_case_sensitive_file_names = init.fs.use_case_sensitive_file_names();
    let to_path: Rc<dyn Fn(&str) -> tspath::Path> = Rc::new(move |file_name: &str| {
        tspath::to_path(file_name, &current_directory, use_case_sensitive_file_names)
    });
    let mut parse_cache = init.parse_cache.clone();
    if parse_cache.is_none() {
        parse_cache = Some(new_parse_cache(RefCountCacheOptions::default()));
    }
    let mut content_mapped_parse_cache = init.content_mapped_parse_cache.clone();
    if content_mapped_parse_cache.is_none() {
        content_mapped_parse_cache = Some(new_content_mapped_parse_cache(
            RefCountCacheOptions::default(),
        ));
    }

    Rc::new(SnapshotHost {
        options: init.options.clone(),
        to_path,
        fs: init.fs.clone(),
        parse_cache: parse_cache.expect("parse cache is set above"),
        content_mapped_parse_cache: content_mapped_parse_cache
            .expect("content mapped parse cache is set above"),
        extended_config_cache: new_extended_config_cache(),
        program_counter: Rc::new(ProgramCounter::default()),
        content_mapper_host: new_content_mapper_host(init),
        snapshot_id: Cell::new(0),
        auto_import_parse_keys: Rc::new(RefCell::new(FxHashMap::default())),
        resolve_ahead_stash: Rc::default(),
    })
}

impl SnapshotHost {
    // Go: project/snapshothost.go:93 NewRootSnapshot (ts#64204: was NewStandaloneRootSnapshot)
    // NewRootSnapshot creates an independent root snapshot.
    // PORT: `_exported`, because Go also has `newRootSnapshot` (PORTING "Names").
    pub fn new_root_snapshot_exported(self: &Rc<Self>) -> Rc<Snapshot> {
        self.new_root_snapshot(0, false)
    }

    // Go: project/snapshothost.go:98 RetainSnapshot
    // RetainSnapshot adds a reference to a snapshot owned by this host.
    pub fn retain_snapshot(&self, snapshot: &Snapshot) {
        snapshot.ref_();
    }

    // Go: project/snapshothost.go:104 CloneSnapshot
    // CloneSnapshot derives a snapshot from baseSnapshot without adopting it as any
    // canonical session state or performing session side effects.
    // PORT: Go returns `(*Snapshot, error)` and returns the snapshot also with
    // an error; the port returns both values. Go `*APISnapshotRequest` is
    // `Option<&APISnapshotRequest>`; the snapshot change holds a copy.
    pub fn clone_snapshot(
        &self,
        ctx: &Context,
        base_snapshot: &Rc<Snapshot>,
        file_changes: FileChangeSummary,
        api_request: Option<&APISnapshotRequest>,
    ) -> (Rc<Snapshot>, Option<GoError>) {
        let mut change = SnapshotChange {
            api_request: api_request.cloned(),
            file_changes,
            ..Default::default()
        };
        // ts#64115
        if let Some(api_request) = api_request {
            change.fs = api_request.file_system.clone();
            change.file_system_override = api_request.file_system.is_some();
            change.replace_file_system = api_request.replace_file_system;
        }
        let snapshot = self.update(ctx, base_snapshot, change);
        let api_error = snapshot.api_error.clone();
        (snapshot, api_error)
    }

    // Go: project/snapshothost.go:125 SnapshotHost.update
    // update derives a snapshot from baseSnapshot without adopting it as any
    // canonical session state or performing session side effects.
    pub fn update(
        &self,
        ctx: &Context,
        base_snapshot: &Rc<Snapshot>,
        change: SnapshotChange,
    ) -> Rc<Snapshot> {
        base_snapshot.clone_(ctx, change, &base_snapshot.overlays(), None, None)
    }

    // Go: project/snapshothost.go:131 CloneSnapshotWithAutoImports
    // CloneSnapshotWithAutoImports derives a snapshot with auto-import preparation without
    // adopting the clone in the background.
    pub fn clone_snapshot_with_auto_imports(
        &self,
        ctx: &Context,
        base_snapshot: &Rc<Snapshot>,
        uri: &lsproto::DocumentUri,
        logger: SessionLogger<'_>,
    ) -> Rc<Snapshot> {
        let mut change = SnapshotChange {
            reason: UpdateReason::REQUESTED_LANGUAGE_SERVICE_WITH_AUTO_IMPORTS,
            // ts#64291
            fs: Some(base_snapshot.fs.fs.clone() as Rc<dyn vfs::Fs>),
            file_system_override: base_snapshot.file_system_override,
            // ts#64204
            resource_request: base_snapshot.resource_request_for_document(uri),
            ..Default::default()
        };
        change.resource_request.auto_imports = uri.clone();
        base_snapshot.clone_(ctx, change, &base_snapshot.overlays(), logger, None)
    }

    // Go: project/snapshothost.go:142 SnapshotHost.newRootSnapshot
    pub fn new_root_snapshot(
        self: &Rc<Self>,
        id: u64,
        relative_pattern_support: bool,
    ) -> Rc<Snapshot> {
        // ts#64291
        let file_system = new_overlay_fs(
            self.fs.clone(),
            IndexMap::default(),
            self.options.position_encoding.clone(),
            self.to_path.clone(),
        );
        self.new_snapshot(
            id,
            Rc::new(SnapshotFS {
                to_path: self.to_path.clone(),
                fs: file_system,
                cache_files: Rc::new(FxHashMap::default()),
                cache_directories: Rc::new(FxHashMap::default()),
                read_files: RefCell::new(FxHashMap::default()),
                node_modules_realpath_aliases: Rc::new(FxHashMap::default()),
            }),
            Rc::new(ConfigFileRegistry::default()),
            None,
            lsutil::new_default_user_preferences(),
            None,
            Some(new_watched_files::<FxHashMap<tspath::Path, String>>(
                "auto-import",
                lsproto::WatchKind(
                    lsproto::WatchKind::CREATE.0
                        | lsproto::WatchKind::CHANGE.0
                        | lsproto::WatchKind::DELETE.0,
                ),
                relative_pattern_support,
                Rc::new(|node_modules_dirs: &FxHashMap<tspath::Path, String>| {
                    let mut patterns: Vec<String> = Vec::with_capacity(node_modules_dirs.len());
                    // PORT: Go map order is random; the patterns are sorted below.
                    for dir in node_modules_dirs.values() {
                        patterns.push(get_recursive_glob_pattern(dir));
                    }
                    patterns.sort();
                    PatternsAndIgnored {
                        patterns_inside_workspace: patterns,
                        ..Default::default()
                    }
                }),
            )),
        )
    }

    // Go: project/snapshothost.go:172 SnapshotHost.FS
    pub fn fs(&self) -> Rc<dyn vfs::Fs> {
        self.fs.clone()
    }

    // Go: project/snapshothost.go:176 SnapshotHost.GetCurrentDirectory
    pub fn get_current_directory(&self) -> String {
        self.options.current_directory.clone()
    }

    // Go: project/snapshothost.go:180 SnapshotHost.DefaultLibraryPath (ts#64158)
    pub fn default_library_path(&self) -> String {
        self.options.default_library_path.clone()
    }

    // Go: project/snapshothost.go:184 SnapshotHost.Close
    pub fn close(&self) {
        if let Some(content_mapper_host) = &self.content_mapper_host {
            let _ = content_mapper_host.close();
        }
    }
}
