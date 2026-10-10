//! Go `internal/project/session.go`.
//!
//! PORT: one thread (see `project/dirty/interfaces.rs`). Every Go mutex of
//! `Session` is dropped; fields that Go changes after construction are
//! `Cell` / `RefCell`. Go `*Session` is `Rc<Session>`: methods that queue
//! work capturing the session (background tasks, timers) or that hand the
//! session to snapshot code take `self: &Rc<Self>`.
//!
//! Background tasks (`backgroundQueue.Enqueue`) run through
//! `background::Queue`, which posts them to `gostd::local::go` in Go
//! enqueue order. Debounce sleeps, the idle cache clean timer and the
//! telemetry ticker are `gostd::local::after_func` timers, so their
//! functions run on the dispatch thread; a debounced task keeps a
//! `background::TaskHold` until its timer has run. `WaitForBackgroundTasks`
//! drains `gostd::local` through `Queue::wait`. The one exception is the
//! clone of the auto-import warm, which is `gostd::local` idle work: the
//! LSP server runs it only when no message waits, and the reader thread can
//! cancel it or make it yield (`WarmAutoImportPreempt`).
//!
//! Go runtime metrics (`runtime/metrics`) exist only in the Go runtime.
//! Performance telemetry reads them as `KindBad` (`metrics_read`), so its
//! Go runtime fields are 0; the system memory fields come from
//! `/proc/meminfo` (`osmemory_get`). The log-only runtime metrics and
//! `runtime.GC()` are PORT skips.

use crate::project::prelude::*;

use crate::contentmapper;
use std::cell::Cell;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

// Go: project/session.go:36 UpdateReason
// PORT: Go `type UpdateReason int` with iota consts. Go
// `UpdateReasonDidOpenFile` is `UpdateReason::DID_OPEN_FILE` (same values).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct UpdateReason(pub i32);

impl UpdateReason {
    pub const UNKNOWN: UpdateReason = UpdateReason(0);
    pub const DID_OPEN_FILE: UpdateReason = UpdateReason(1);
    pub const DID_CLOSE_FILE: UpdateReason = UpdateReason(2);
    pub const DID_CHANGE_COMPILER_OPTIONS_FOR_INFERRED_PROJECTS: UpdateReason = UpdateReason(3);
    pub const REQUESTED_LANGUAGE_SERVICE_PENDING_CHANGES: UpdateReason = UpdateReason(4);
    pub const REQUESTED_LANGUAGE_SERVICE_PROJECT_NOT_LOADED: UpdateReason = UpdateReason(5);
    pub const REQUESTED_LANGUAGE_SERVICE_FOR_FILE_NOT_OPEN: UpdateReason = UpdateReason(6);
    pub const REQUESTED_LANGUAGE_SERVICE_PROJECT_DIRTY: UpdateReason = UpdateReason(7);
    pub const REQUESTED_LOAD_PROJECT_TREE: UpdateReason = UpdateReason(8);
    pub const REQUESTED_LANGUAGE_SERVICE_WITH_AUTO_IMPORTS: UpdateReason = UpdateReason(9);
    pub const IDLE_CLEAN_DISK_CACHE: UpdateReason = UpdateReason(10);
    pub const DID_CHANGE_CONFIG_FILE: UpdateReason = UpdateReason(11);
    // tsgo#4712
    pub const DID_CHANGE_CONTENT_MAPPER_CONTRIBUTIONS: UpdateReason = UpdateReason(12);
}

// Go: project/session.go:39 ErrNoProjectForUnknownScriptKind (tsgo#4712)
// ErrNoProjectForUnknownScriptKind identifies requests for otherwise unsupported files.
pub static ERR_NO_PROJECT_FOR_UNKNOWN_SCRIPT_KIND: std::sync::LazyLock<GoError> =
    std::sync::LazyLock::new(|| gostd::errors::new("no project for unknown script kind"));

// Go: project/session.go:57 ContentMapperContributions (tsgo#4712)
// PORT: Go `[]*contentmapper.Mapper` is `Vec<Rc<Mapper>>`.
#[derive(Clone, Debug, Default)]
pub struct ContentMapperContributions {
    pub mappers: Vec<Rc<contentmapper::Mapper>>,
    pub extensions: Vec<String>,
}

// Go: project/session.go:64 watchRequestTimeout
// watchRequestTimeout is the maximum time to wait for the client to respond to
// a WatchFiles or UnwatchFiles request while holding the watches mutex.
pub const WATCH_REQUEST_TIMEOUT: Duration = Duration::from_secs(1);

// Go: project/session.go:68 SessionOptions
// SessionOptions are the immutable initialization options for a session.
// Snapshots may reference them as a pointer since they never change.
// PORT: Go `*SessionOptions` is `Rc<SessionOptions>`.
pub struct SessionOptions {
    pub current_directory: String,
    pub default_library_path: String,
    pub typings_location: String,
    pub position_encoding: lsproto::PositionEncodingKind,
    pub watch_enabled: bool,
    pub logging_enabled: bool,
    pub telemetry_enabled: bool,
    pub push_diagnostics_enabled: bool,
    // RunExternalCode allows configured content mappers to run their (external) processes,
    // gated on workspace trust by the client. It corresponds to the --runExternalCode CLI flag.
    pub run_external_code: bool,
    pub debounce_delay: Duration,
    pub checker_pool_options: CheckerPoolOptions,
}

// Go: project/session.go:84 SessionInit
// PORT: Go nil interfaces and pointers are `None`.
pub struct SessionInit {
    pub background_ctx: Context,
    pub options: Rc<SessionOptions>,
    pub fs: Rc<dyn vfs::Fs>,
    pub client: Option<Rc<dyn Client>>,
    pub logger: Option<Rc<dyn logging::Logger>>,
    pub npm_executor: Option<Rc<dyn ata::NpmExecutor>>,
    // Spawner launches content mapper processes. It is nil when the host cannot spawn processes.
    pub spawner: Option<Rc<dyn contentmapper::Spawner>>,
    pub content_mapper_logger: Option<contentmapper::Logger>,
    pub parse_cache: Option<Rc<ParseCache>>,
    pub content_mapped_parse_cache: Option<Rc<ContentMappedParseCache>>,
}

// Go: project/session.go:104 Session
// Session manages the state of an LSP session. It receives textDocument
// events and requests for LanguageService objects from the LPS server
// and processes them into immutable snapshots as the data source for
// LanguageServices. When Session transitions from one snapshot to the
// next, it diffs them and updates file watchers and Automatic Type
// Acquisition (ATA) state accordingly.
// PORT: the Go mutexes (`snapshotMu`, `snapshotUpdateMu`,
// `scheduledSnapshotUpdateMu`, `userConfigRWMu`, `pendingFileChangesMu`,
// `pendingATAChangesMu`, `diagnosticsRefreshMu`, `warmAutoImportMu`,
// `idleCacheCleanMu`) are dropped. Go `atomic.*` fields are `Cell`.
// PORT: Go embeds `*SnapshotHost` (ts#64163); here it is the field
// `snapshot_host`, and `Session` derefs to it, so the host's fields and
// methods are promoted as in Go. A `Session` method with the same name
// (`fs`, `get_current_directory`, `close`) wins, as in Go.
pub struct Session {
    pub snapshot_host: Rc<SnapshotHost>,
    pub options: Rc<SessionOptions>,
    pub logger: Option<Rc<dyn logging::Logger>>,
    pub background_ctx: Context,
    pub to_path: Rc<dyn Fn(&str) -> tspath::Path>,
    pub client: Option<Rc<dyn Client>>,
    pub start_time: Instant,
    pub npm_executor: Option<Rc<dyn ata::NpmExecutor>>,
    pub fs: Rc<OverlayFS>,
    // contentMapperTimings is the cumulative host snapshot at the most recent session snapshot adoption.
    // PORT: `contentMapperTimingsMu` is dropped (one thread).
    pub content_mapper_timings: RefCell<contentmapper::Timings>,

    // registeredContentMapperSnapshotID is the ID of the newest snapshot whose registration has been
    // applied. Registration runs from background tasks that may finish out of order, so
    // contentMapperRegistrationMu serializes updates and the snapshot ID keeps a stale task from
    // overwriting a newer snapshot's registration.
    // PORT: `contentMapperRegistrationMu` is dropped (one thread).
    pub registered_content_mapper_extensions: RefCell<Vec<String>>,
    pub registered_content_mapper_snapshot_id: Cell<u64>,

    // read-only after initialization
    pub initial_user_preferences: RefCell<lsutil::UserPreferences>,
    // current preferences
    pub workspace_user_preferences: RefCell<lsutil::UserPreferences>,
    pub compiler_options_for_inferred_projects: RefCell<Option<Rc<CompilerOptions>>>,
    // PORT: set right after the session `Rc` exists (Go assigns it after
    // the composite literal). The installer holds the session as its host,
    // so the pair is a reference cycle that lives as long as the process.
    pub typings_installer: RefCell<Option<Rc<ata::TypingsInstaller>>>,
    pub background_queue: Rc<background::Queue>,

    // snapshot is the current immutable state of all projects.
    pub snapshot: RefCell<Rc<Snapshot>>,

    // scheduledSnapshotUpdateCancel is the cancelation function for a scheduled
    // snapshot update. Snapshot updates are scheduled and debounced after file closes.
    pub scheduled_snapshot_update_cancel: RefCell<Option<gostd::context::CancelFunc>>,
    pub scheduled_snapshot_update_generation: Cell<u64>,

    pub pending_user_config_changes: Cell<bool>,

    // pendingFileChanges are accumulated from textDocument/* events delivered
    // by the LSP server through DidOpenFile(), DidChangeFile(), etc. They are
    // applied to the next snapshot update.
    pub pending_file_changes: RefCell<Vec<FileChange>>,

    // pendingATAChanges are produced by Automatic Type Acquisition (ATA)
    // installations and applied to the next snapshot update.
    pub pending_ata_changes: RefCell<FxHashMap<ID, Rc<ATAStateChange>>>,

    // diagnosticsRefreshCancel is the cancelation function for a scheduled
    // diagnostics refresh. Diagnostics refreshes are scheduled and debounced
    // after file watch changes and ATA updates.
    pub diagnostics_refresh_cancel: RefCell<Option<gostd::context::CancelFunc>>,
    pub diagnostics_refresh_generation: Cell<u64>,

    // warmAutoImportCancel is the cancelation function for a running
    // auto-import cache warming task. It is cancelled on file opens,
    // closes, changes, watched-file changes, new auto-import warming
    // requests, and when the session closes.
    // PORT: Go stores its own closure (it logs through the session logger),
    // which is not `Send`, so this is an `Rc<dyn Fn()>`, not a
    // `gostd::context::CancelFunc`.
    pub warm_auto_import_cancel: RefCell<Option<Rc<dyn Fn()>>>,
    // PORT: the `Send` copy of `warm_auto_import_cancel` for the LSP reader
    // thread. See `WarmAutoImportPreempt`.
    pub warm_auto_import_preempt: WarmAutoImportPreempt,
    // PORT: the auto-import warm whose clone waits for idle time (see
    // `warm_auto_import_cache`), and whether an idle job for it is queued.
    pub warm_auto_import_pending: RefCell<Option<PendingWarm>>,
    pub warm_auto_import_queued: Cell<bool>,
    // PORT: set when an eager attempt of the warm ends without its clone or
    // a clone takes longer than `WARM_AUTO_IMPORT_HOLD_CAP`, cleared when a
    // clone ends sooner. While it is set, a warm waits for a quiet period
    // (see `run_pending_warm`).
    pub warm_auto_import_slow: Cell<bool>,

    // idleCacheCleanTimer is a resettable timer for scheduling idle disk
    // cache cleans. The timer resets on any file event (open, close,
    // change, save, watch) and fires after 30 seconds of inactivity.
    pub idle_cache_clean_timer: RefCell<Option<gostd::local::LocalTimer>>,

    // performanceTelemetryCancel cancels the periodic performance telemetry ticker.
    pub performance_telemetry_cancel: RefCell<Option<gostd::context::CancelFunc>>,

    // seenProjects tracks projects that have already had telemetry sent.
    pub seen_projects: RefCell<FxHashSet<ID>>,

    // watches tracks the current watch globs and how many individual WatchedFiles
    // are using each glob.
    pub watches: Rc<WatchRegistry>,

    // globalDiagPublishPending is set to true when a global diagnostics publish
    // task should be enqueued. It is reset when the task runs, coalescing multiple
    // requests into a single background task.
    pub global_diag_publish_pending: Cell<bool>,
}

// Go: project/session.go:199 newContentMapperHost (tsgo#4712)
// newContentMapperHost creates the session's shared content mapper host when the workspace is trusted and
// a spawner is available; otherwise it returns nil, and configured content mappers are rejected by the
// config-file gate.
pub fn new_content_mapper_host(init: &SessionInit) -> Option<Rc<dyn contentmapper::Host>> {
    let spawner = match &init.spawner {
        Some(spawner) if init.options.run_external_code => spawner.clone(),
        _ => return None,
    };
    let mut diagnostic_locale = locale::DEFAULT;
    if let Some(client) = &init.client {
        diagnostic_locale = client.get_locale();
    }
    Some(contentmapper::new_host_with_options(
        &init.background_ctx,
        spawner,
        diagnostic_locale,
        contentmapper::HostOptions {
            logger: init.content_mapper_logger.clone(),
        },
    ))
}

// Go: project/session.go:210 NewSession
pub fn new_session(init: &SessionInit) -> Rc<Session> {
    // Not in Go: a process with a session is a language server or API
    // process, which frees the file versions it publishes again
    // (`ast::free_file_versions`).
    crate::ast::set_editor_process();
    let snapshot_host = new_snapshot_host(init);
    let mut session_logger = init.logger.clone();
    if session_logger.is_none() {
        session_logger = logging::new_nop_logger();
    }
    let session = Rc::new(Session {
        snapshot_host: snapshot_host.clone(),
        options: init.options.clone(),
        logger: session_logger,
        background_ctx: init.background_ctx.clone(),
        to_path: snapshot_host.to_path.clone(),
        client: init.client.clone(),
        npm_executor: init.npm_executor.clone(),
        fs: new_overlay_fs(
            snapshot_host.fs.clone(),
            IndexMap::default(),
            init.options.position_encoding.clone(),
            snapshot_host.to_path.clone(),
        ),
        content_mapper_timings: RefCell::new(contentmapper::Timings::default()),
        registered_content_mapper_extensions: RefCell::new(Vec::new()),
        registered_content_mapper_snapshot_id: Cell::new(0),
        background_queue: background::new_queue(),
        start_time: Instant::now(),
        snapshot: RefCell::new(
            snapshot_host.new_root_snapshot(
                0,
                lsproto::get_client_capabilities(&init.background_ctx)
                    .workspace
                    .did_change_watched_files
                    .relative_pattern_support,
            ),
        ),
        initial_user_preferences: RefCell::new(lsutil::new_default_user_preferences()),
        workspace_user_preferences: RefCell::new(lsutil::new_default_user_preferences()),
        compiler_options_for_inferred_projects: RefCell::new(None),
        typings_installer: RefCell::new(None),
        scheduled_snapshot_update_cancel: RefCell::new(None),
        scheduled_snapshot_update_generation: Cell::new(0),
        pending_user_config_changes: Cell::new(false),
        pending_file_changes: RefCell::new(Vec::new()),
        pending_ata_changes: RefCell::new(FxHashMap::default()),
        diagnostics_refresh_cancel: RefCell::new(None),
        diagnostics_refresh_generation: Cell::new(0),
        warm_auto_import_cancel: RefCell::new(None),
        warm_auto_import_preempt: WarmAutoImportPreempt::default(),
        warm_auto_import_pending: RefCell::new(None),
        warm_auto_import_queued: Cell::new(false),
        warm_auto_import_slow: Cell::new(false),
        idle_cache_clean_timer: RefCell::new(None),
        performance_telemetry_cancel: RefCell::new(None),
        seen_projects: RefCell::new(FxHashSet::default()),
        watches: new_watch_registry(),
        global_diag_publish_pending: Cell::new(false),
    });

    if !init.options.typings_location.is_empty() && init.npm_executor.is_some() {
        let typings_installer = ata::new_typings_installer(
            &ata::TypingsInstallerOptions {
                typings_location: init.options.typings_location.clone(),
                throttle_limit: 5,
            },
            session.clone(),
        );
        *session.typings_installer.borrow_mut() = Some(typings_installer);
    }
    if let Some(content_mapper_host) = &snapshot_host.content_mapper_host {
        *session.content_mapper_timings.borrow_mut() = content_mapper_host.timings();
    }

    session
}

// PORT: Go `Session` embeds `*SnapshotHost` (ts#64163).
impl std::ops::Deref for Session {
    type Target = SnapshotHost;

    fn deref(&self) -> &SnapshotHost {
        &self.snapshot_host
    }
}

// PORT: Go `FS()` and `GetCurrentDirectory()` implement
// `module.ResolutionHost`, whose Rust form returns borrowed values. The
// inherent methods keep the Go results for other callers (api session).
impl crate::frontend::module::ResolutionHost for Session {
    // Go: project/session.go:251 FS (ts#64291: the overlay file system)
    fn fs(&self) -> &dyn vfs::Fs {
        &*self.fs
    }

    // Go: project/session.go:256 GetCurrentDirectory
    fn get_current_directory(&self) -> &str {
        &self.options.current_directory
    }
}

// Go: project/session.go:1827 NpmInstall
// PORT: Go `NpmInstall` implements `ata.NpmExecutor`. With this impl and
// `module::ResolutionHost`, `Session` is an `ata::TypingsInstallerHost`.
impl ata::NpmExecutor for Session {
    fn npm_install(&self, cwd: &str, npm_install_args: &[String]) -> (Vec<u8>, Option<GoError>) {
        self.npm_executor
            .as_ref()
            .unwrap_or_else(|| crate::core::go_nil_dereference())
            .npm_install(cwd, npm_install_args)
    }

    // PORT: see `ata::NpmExecutor::npm_install_func`.
    fn npm_install_func(&self) -> Option<ata::NpmInstallFunc> {
        self.npm_executor.as_ref()?.npm_install_func()
    }
}

impl Session {
    // Go: project/session.go:251 FS
    // FS implements module.ResolutionHost
    pub fn fs(&self) -> Rc<dyn vfs::Fs> {
        self.fs.clone()
    }

    // Go: project/session.go:256 GetCurrentDirectory
    // GetCurrentDirectory implements module.ResolutionHost
    pub fn get_current_directory(&self) -> String {
        self.options.current_directory.clone()
    }

    // Go: project/session.go:260 DefaultLibraryPath (ts#64158)
    pub fn default_library_path(&self) -> String {
        self.options.default_library_path.clone()
    }

    // Go: project/session.go:265 Config
    // Gets copy of current configuration
    pub fn config(&self) -> lsutil::UserPreferences {
        self.workspace_user_preferences.borrow().clone()
    }

    // Go: project/session.go:271 backgroundContext
    fn background_context(&self) -> Context {
        self.with_current_locale(&self.background_ctx)
    }

    // Go: project/session.go:275 WithCurrentLocale (exported by ts#64163)
    pub fn with_current_locale(&self, ctx: &Context) -> Context {
        let Some(client) = &self.client else {
            return ctx.clone();
        };
        locale::with_locale(ctx, client.get_locale())
    }

    // Go: project/session.go:283 Trace
    // Trace implements module.ResolutionHost
    pub fn trace(&self, _msg: &str) {
        crate::core::go_panic("ATA module resolution should not use tracing".to_string());
    }

    // Go: project/session.go:287 Configure
    // PORT: `configureMu` and `userConfigRWMu` are dropped (one thread).
    pub fn configure(self: &Rc<Self>, config: lsutil::UserPreferences) {
        self.pending_user_config_changes.set(true);
        let old_config = self.workspace_user_preferences.replace(config.clone());

        if !config.locale.is_empty() {
            let client = self
                .client
                .as_ref()
                .unwrap_or_else(|| crate::core::go_nil_dereference());
            let old_locale = client.get_locale();
            client.set_locale(&config.locale);
            let new_locale = client.get_locale();
            if old_locale.string() != new_locale.string()
                && let Some(content_mapper_host) = &self.content_mapper_host
            {
                content_mapper_host.set_locale(new_locale);
            }
        }

        // Tell the client to re-request certain commands depending on user preference changes.
        self.refresh_inlay_hints_if_needed(&old_config, &config);
        self.refresh_code_lens_if_needed(&old_config, &config);
        self.refresh_diagnostics_if_needed(&old_config, &config);
        self.refresh_ata_if_needed(&old_config, &config);
    }

    // Go: project/session.go:314 InitializeWithUserConfig
    pub fn initialize_with_user_config(self: &Rc<Self>, config: lsutil::UserPreferences) {
        *self.initial_user_preferences.borrow_mut() = config.clone();
        self.configure(config);
    }

    // Go: project/session.go:319 DidOpenFile
    pub fn did_open_file(
        self: &Rc<Self>,
        ctx: &Context,
        uri: &lsproto::DocumentUri,
        version: i32,
        content: &str,
        language_kind: &lsproto::LanguageKind,
    ) {
        self.cancel_warm_auto_import_cache();
        self.schedule_idle_cache_clean();
        self.cancel_scheduled_snapshot_update();
        self.pending_file_changes.borrow_mut().push(FileChange {
            kind: FileChangeKind::OPEN,
            uri: uri.clone(),
            version,
            content: content.to_string(),
            language_kind: language_kind.clone(),
            ..Default::default()
        });
        let (changes, overlays) = self.flush_changes_locked(ctx);
        self.update_snapshot_exported(
            ctx,
            overlays,
            SnapshotChange {
                reason: UpdateReason::DID_OPEN_FILE,
                file_changes: changes,
                resource_request: ResourceRequest {
                    documents: vec![uri.clone()],
                    ..Default::default()
                },
                ..Default::default()
            },
        );
    }

    // Go: project/session.go:344 SetContentMapperContributions (tsgo#4712)
    // SetContentMapperContributions atomically replaces extension-provided inferred-project mappers and
    // discovers configured projects for matching open documents. Configured projects never consume these mappers.
    // PORT: `snapshotUpdateMu` and `pendingFileChangesMu` are dropped (one
    // thread).
    pub fn set_content_mapper_contributions(
        self: &Rc<Self>,
        ctx: &Context,
        contributions: ContentMapperContributions,
        document_uris: Vec<lsproto::DocumentUri>,
    ) {
        if !self.options.run_external_code {
            return;
        }
        self.cancel_scheduled_snapshot_update();
        let (changes, overlays) = self.flush_changes_locked(ctx);
        self.update_snapshot_exported(
            ctx,
            overlays,
            SnapshotChange {
                reason: UpdateReason::DID_CHANGE_CONTENT_MAPPER_CONTRIBUTIONS,
                file_changes: changes,
                content_mapper_contributions: Some(contributions),
                resource_request: ResourceRequest {
                    configured_project_documents: document_uris,
                    ..Default::default()
                },
                ..Default::default()
            },
        );
        let snapshot = self.snapshot();
        let _ = self.update_content_mapper_registrations(ctx, &snapshot);
    }

    // Go: project/session.go:363 DidCloseFile
    pub fn did_close_file(self: &Rc<Self>, _ctx: &Context, uri: &lsproto::DocumentUri) {
        self.cancel_warm_auto_import_cache();
        self.schedule_idle_cache_clean();
        self.pending_file_changes.borrow_mut().push(FileChange {
            kind: FileChangeKind::CLOSE,
            uri: uri.clone(),
            ..Default::default()
        });
        self.schedule_snapshot_update(UpdateReason::DID_CLOSE_FILE);
    }

    // Go: project/session.go:375 DidChangeFile
    pub fn did_change_file(
        self: &Rc<Self>,
        _ctx: &Context,
        uri: &lsproto::DocumentUri,
        version: i32,
        changes: &[lsproto::TextDocumentContentChangePartialOrWholeDocument],
    ) {
        self.cancel_warm_auto_import_cache();
        self.schedule_idle_cache_clean();
        self.pending_file_changes.borrow_mut().push(FileChange {
            kind: FileChangeKind::CHANGE,
            uri: uri.clone(),
            version,
            changes: changes.to_vec(),
            ..Default::default()
        });

        // Editing a content-mapped file changes the program like any source edit, but the client's
        // pull-diagnostics machinery won't re-request diagnostics for dependent files: the content-mapped file is not
        // in the diagnostic provider's document selector, so a change to it never triggers the client's
        // inter-file re-pull. Prompt a workspace refresh so dependents update. We skip the debounce here so the
        // edit doesn't feel sluggish (normal source edits are pulled per-keystroke client-side); the refresh is
        // still coalesced. Ordinary source files are handled entirely client-side, so we cancel any pending
        // refresh for them as before.
        if self.is_content_mapper_file(uri) {
            self.schedule_diagnostics_refresh(Duration::ZERO);
        } else {
            self.cancel_diagnostics_refresh();
        }
    }

    // Go: project/session.go:403 isContentMapperFile (tsgo#4712)
    // isContentMapperFile reports whether uri is a content-mapped file handled by a configured content mapper, based
    // on the extensions currently registered with the client for text document synchronization.
    pub fn is_content_mapper_file(&self, uri: &lsproto::DocumentUri) -> bool {
        let snapshot = self.snapshot();
        let configured = snapshot.config_file_registry.content_mappers();
        let mut extensions: Vec<&str> = configured.extensions.iter().map(String::as_str).collect();
        extensions.extend(
            snapshot
                .inferred_project_content_mapper_extensions
                .iter()
                .map(String::as_str),
        );
        tspath::file_extension_is_one_of(&uri.file_name(), &extensions)
    }

    // Go: project/session.go:410 DidSaveFile
    pub fn did_save_file(self: &Rc<Self>, _ctx: &Context, uri: &lsproto::DocumentUri) {
        self.schedule_idle_cache_clean();
        self.pending_file_changes.borrow_mut().push(FileChange {
            kind: FileChangeKind::SAVE,
            uri: uri.clone(),
            ..Default::default()
        });
    }

    // Go: project/session.go:420 DidChangeWatchedFiles
    // PORT: Go `[]*lsproto.FileEvent` is `&[lsproto::FileEvent]`.
    pub fn did_change_watched_files(
        self: &Rc<Self>,
        _ctx: &Context,
        changes: &[Option<lsproto::FileEvent>],
    ) {
        let mut file_changes: Vec<FileChange> = Vec::with_capacity(changes.len());
        let mut has_relevant_change = false;
        let mut has_config_change = false;
        let snapshot = self.snapshot();
        let config_file_registry = snapshot.config_file_registry.clone();
        let (content_mapper_extensions, content_mapper_watched_files) =
            snapshot.content_mapper_watch_state();
        let content_mapper_extensions: Vec<&str> = content_mapper_extensions
            .iter()
            .map(String::as_str)
            .collect();
        for change in changes {
            // Go: change.Type (a nil element panics)
            let change = change
                .as_ref()
                .unwrap_or_else(|| crate::core::go_nil_dereference());
            let kind = match change.type_ {
                lsproto::FileChangeType::CREATED => FileChangeKind::WATCH_CREATE,
                lsproto::FileChangeType::CHANGED => FileChangeKind::WATCH_CHANGE,
                lsproto::FileChangeType::DELETED => FileChangeKind::WATCH_DELETE,
                _ => continue, // Ignore unknown change types.
            };
            file_changes.push(FileChange {
                kind,
                uri: change.uri.clone(),
                ..Default::default()
            });

            if !has_config_change
                && config_file_registry.is_tracked(&(self.to_path)(&change.uri.file_name()))
            {
                has_config_change = true;
            }

            if !has_relevant_change {
                let file_name = change.uri.file_name();
                let path = (self.to_path)(&file_name).remove_trailing_directory_separator();
                if content_mapper_watched_files.contains(&path) {
                    has_relevant_change = true;
                    continue;
                }
                let path_str = path.as_str();
                let i = path_str.rfind('.');
                if i.is_none_or(|i| path_str.rfind('/').is_some_and(|slash| slash > i)) {
                    // Extensionless paths might be directories.
                    // For creations/changes, we can check the file system.
                    // For deletions, consult the current snapshot cache to avoid treating extensionless file deletions as relevant.
                    if kind != FileChangeKind::WATCH_DELETE {
                        has_relevant_change = vfs::Fs::directory_exists(&*self.fs, &file_name);
                    } else {
                        let snapshot = self.snapshot.borrow().clone();
                        if snapshot.fs.cache_directories.contains_key(&path)
                            || snapshot.has_overlay_within(&path)
                            || is_node_modules_path(&path)
                        {
                            has_relevant_change = true;
                        }
                    }
                } else if let Some(i) = i {
                    if is_relevant_extension(&path_str[i..])
                        || tspath::file_extension_is_one_of(path_str, &content_mapper_extensions)
                    {
                        has_relevant_change = true;
                    }
                }
            }
        }

        self.pending_file_changes.borrow_mut().extend(file_changes);

        if has_relevant_change {
            // Schedule a debounced diagnostics refresh only for paths
            // that can affect the TypeScript program (relevant extensions or directories).
            self.schedule_diagnostics_refresh_exported();
        }
        if has_config_change {
            // Config file diagnostics are pushed on snapshot updates rather than pulled,
            // so they must not depend on the client re-pulling diagnostics in response to
            // the refresh request above.
            self.schedule_snapshot_update(UpdateReason::DID_CHANGE_CONFIG_FILE);
        }
        self.cancel_warm_auto_import_cache();
        self.schedule_idle_cache_clean();
    }

    // Go: project/session.go:499 DidChangeCompilerOptionsForInferredProjects
    pub fn did_change_compiler_options_for_inferred_projects(
        self: &Rc<Self>,
        ctx: &Context,
        options: Option<Rc<CompilerOptions>>,
    ) {
        *self.compiler_options_for_inferred_projects.borrow_mut() = options.clone();
        self.update_snapshot_exported(
            ctx,
            (*self.fs.overlays()).clone(),
            SnapshotChange {
                reason: UpdateReason::DID_CHANGE_COMPILER_OPTIONS_FOR_INFERRED_PROJECTS,
                compiler_options_for_inferred_projects: options,
                ..Default::default()
            },
        );
    }

    // Go: project/session.go:507 ScheduleDiagnosticsRefresh
    // PORT: `_exported`, because Go also has `scheduleDiagnosticsRefresh`.
    pub fn schedule_diagnostics_refresh_exported(self: &Rc<Self>) {
        self.schedule_diagnostics_refresh(self.options.debounce_delay);
    }

    // Go: project/session.go:514 scheduleDiagnosticsRefresh (tsgo#4712)
    // scheduleDiagnosticsRefresh schedules a coalesced workspace diagnostics refresh after delay. A delay of
    // 0 refreshes as soon as the background queue runs the task; it is used for interactive edits (e.g. a
    // content-mapped file) where the debounce would make dependent-file diagnostics feel sluggish.
    // PORT: Go sleeps inside the background task with
    // `select { case <-time.After(delay): case <-ctx.Done(): }`. Here the
    // task arms a `gostd::local::after_func` timer for the delay, and the
    // rest of the task runs when it fires; a cancelled context makes it
    // return then (Go returns at once; nothing else differs). The task
    // keeps a `background::TaskHold` until then, so `Queue::wait` (Go
    // `WaitForBackgroundTasks`) waits for the refresh, as Go's does. A zero
    // delay runs the rest of the task at once, as Go does.
    pub fn schedule_diagnostics_refresh(self: &Rc<Self>, delay: Duration) {
        // Cancel any existing scheduled diagnostics refresh
        let existing_cancel = self.diagnostics_refresh_cancel.borrow().clone();
        if let Some(existing_cancel) = existing_cancel {
            existing_cancel();
            self.logger.log("Delaying scheduled diagnostics refresh...");
        } else {
            self.logger.log("Scheduling new diagnostics refresh...");
        }

        // Create a new cancellable context for the debounce task
        let (debounce_ctx, cancel) = gostd::context::with_cancel(&self.background_context());
        self.diagnostics_refresh_generation
            .set(self.diagnostics_refresh_generation.get() + 1);
        let generation = self.diagnostics_refresh_generation.get();
        *self.diagnostics_refresh_cancel.borrow_mut() = Some(cancel.clone());

        // Enqueue the (optionally debounced) diagnostics refresh
        let s = self.clone();
        self.background_queue.enqueue(&debounce_ctx, move |ctx| {
            let ctx = ctx.clone();
            let hold = s.background_queue.hold();
            let mut task = Some(move || {
                let run = || {
                    // Wait out the debounce window; a newer event cancels this one.
                    if ctx.err().is_some() {
                        // Context was cancelled, newer events arrived
                        return;
                    }
                    // Delay completed, proceed with refresh

                    // Clear the cancel function since we're about to execute the refresh
                    if s.diagnostics_refresh_generation.get() != generation {
                        return;
                    }
                    *s.diagnostics_refresh_cancel.borrow_mut() = None;

                    if s.options.logging_enabled {
                        s.logger.log("Running scheduled diagnostics refresh");
                    }
                    if let Err(err) = s
                        .client
                        .as_ref()
                        .unwrap_or_else(|| crate::core::go_nil_dereference())
                        .refresh_diagnostics(&s.background_context())
                    {
                        if s.options.logging_enabled {
                            s.logger
                                .logf(&format!("Error refreshing diagnostics: {}", err.error()));
                        }
                    }
                };
                // Go: the rest of the same wg.Go goroutine (`TaskHold`).
                crate::core::go_wait_group_task(run);
                // Go: defer cancel()
                cancel();
                drop(hold);
            });
            if delay > Duration::ZERO {
                gostd::local::after_func(
                    delay,
                    Box::new(move || {
                        if let Some(task) = task.take() {
                            task();
                        }
                    }),
                );
            } else if let Some(task) = task.take() {
                task();
            }
        });
    }

    // Go: project/session.go:566 cancelDiagnosticsRefresh
    pub fn cancel_diagnostics_refresh(&self) {
        let cancel = self.diagnostics_refresh_cancel.borrow().clone();
        if let Some(cancel) = cancel {
            cancel();
            self.logger.log("Canceled scheduled diagnostics refresh");
            *self.diagnostics_refresh_cancel.borrow_mut() = None;
            self.diagnostics_refresh_generation
                .set(self.diagnostics_refresh_generation.get() + 1);
        }
    }

    // Go: project/session.go:577 ScheduleSnapshotUpdate
    // PORT: the debounce sleep is a `gostd::local::after_func` timer, and
    // the task keeps a `background::TaskHold` until the update has run, as
    // in `schedule_diagnostics_refresh`.
    pub fn schedule_snapshot_update(self: &Rc<Self>, reason: UpdateReason) {
        // Cancel any existing scheduled snapshot update
        let existing_cancel = self.scheduled_snapshot_update_cancel.borrow().clone();
        if let Some(existing_cancel) = existing_cancel {
            existing_cancel();
            if self.options.logging_enabled {
                self.logger.log("Delaying scheduled snapshot update...");
            }
        } else if self.options.logging_enabled {
            self.logger.log("Scheduling new snapshot update...");
        }

        // Create a new cancellable context for the debounce task
        let (debounce_ctx, cancel) = gostd::context::with_cancel(&self.background_context());
        self.scheduled_snapshot_update_generation
            .set(self.scheduled_snapshot_update_generation.get() + 1);
        let generation = self.scheduled_snapshot_update_generation.get();
        *self.scheduled_snapshot_update_cancel.borrow_mut() = Some(cancel.clone());

        // Enqueue the debounced snapshot update
        let s = self.clone();
        self.background_queue.enqueue(&debounce_ctx, move |ctx| {
            let ctx = ctx.clone();
            let delay = s.options.debounce_delay;
            let hold = s.background_queue.hold();
            let mut task = Some(move || {
                let run = || {
                    // Sleep for the debounce delay
                    if ctx.err().is_some() {
                        // Context was cancelled, newer events arrived or another snapshot update ran
                        return;
                    }
                    // Delay completed, proceed with update

                    // Clear the cancel function since we're about to execute the update
                    if s.scheduled_snapshot_update_generation.get() != generation {
                        return;
                    }
                    *s.scheduled_snapshot_update_cancel.borrow_mut() = None;

                    if s.options.logging_enabled {
                        s.logger.log("Running scheduled snapshot update");
                    }

                    let (file_changes, overlays, ata_changes, new_config) = s.flush_changes(&ctx);
                    if file_changes.is_empty() && ata_changes.is_empty() && new_config.is_none() {
                        return;
                    }

                    s.update_snapshot_exported(
                        &ctx,
                        overlays,
                        SnapshotChange {
                            reason,
                            file_changes,
                            ata_changes,
                            new_config,
                            ..Default::default()
                        },
                    );
                };
                // Go: the rest of the same wg.Go goroutine (`TaskHold`).
                crate::core::go_wait_group_task(run);
                // Go: defer cancel()
                cancel();
                drop(hold);
            });
            gostd::local::after_func(
                delay,
                Box::new(move || {
                    if let Some(task) = task.take() {
                        task();
                    }
                }),
            );
        });
    }

    // Go: project/session.go:639 cancelScheduledSnapshotUpdate
    pub fn cancel_scheduled_snapshot_update(&self) {
        let cancel = self.scheduled_snapshot_update_cancel.borrow().clone();
        if let Some(cancel) = cancel {
            cancel();
            if self.options.logging_enabled {
                self.logger.log("Canceled scheduled snapshot update");
            }
            *self.scheduled_snapshot_update_cancel.borrow_mut() = None;
            self.scheduled_snapshot_update_generation
                .set(self.scheduled_snapshot_update_generation.get() + 1);
        }
    }

    // Go: project/session.go:652 cancelWarmAutoImportCache
    pub fn cancel_warm_auto_import_cache(&self) {
        let cancel = self.warm_auto_import_cancel.borrow().clone();
        if let Some(cancel) = cancel {
            cancel();
            *self.warm_auto_import_cancel.borrow_mut() = None;
            self.warm_auto_import_preempt.clear();
        }
        // PORT: a cancelled warm whose clone has not started lets its
        // snapshot go now (see `run_pending_warm`).
        match self.warm_auto_import_pending.take() {
            Some(warm) if warm.ctx.err().is_some() => self.end_pending_warm(warm),
            pending => *self.warm_auto_import_pending.borrow_mut() = pending,
        }
    }
}

/// PORT: the state that Go's `warmAutoImportCache` keeps for its clone,
/// while the clone waits for idle time. It holds a reference on
/// `new_snapshot` (Go `tryRef`).
pub struct PendingWarm {
    ctx: Context,
    cancel: gostd::context::CancelFunc,
    changed_file: lsproto::DocumentUri,
    new_snapshot: Rc<Snapshot>,
    /// The id that Go's clone takes when the warm starts.
    snapshot_id: u64,
    /// For an eager attempt (it starts as soon as no message waits): the
    /// time from which the attempt's hold counts (see `run_pending_warm`).
    /// `None` for an attempt that waits for a quiet period.
    hold_from: Option<Instant>,
    /// Whether an attempt yielded before (see `run_pending_warm`).
    retry: bool,
}

/// PORT: the longest time that a message waits for an eager attempt of the
/// auto-import warm (see `run_pending_warm`). Go's head start is about 2 ms
/// in the lswarm1 repros.
pub const WARM_AUTO_IMPORT_HOLD_CAP: Duration = Duration::from_millis(5);

/// PORT: how a clone attempt of the auto-import warm ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WarmAttemptEnd {
    /// The clone ended. Go adopts it.
    Done,
    /// The warm was cancelled (Go `warmCtx`). Go discards the clone.
    Cancelled,
    /// Only the attempt was cancelled: a message did not wait for it. The
    /// warm tries again.
    Yielded,
}

/// PORT: lets the LSP reader thread cancel the auto-import warm, or make it
/// wait or yield.
///
/// Go runs the warm on a goroutine, and the dispatch goroutine cancels it
/// when it handles didOpen, didChange, didClose or didChangeWatchedFiles.
/// Other messages run at the same time as the warm. Here the clone runs on
/// the dispatch thread (`Session::run_pending_warm`), so the dispatch thread
/// cannot get to a message until the clone ends. The reader thread calls
/// `on_message` for each message that it queues:
///
/// - While an eager attempt runs, the message first waits up to the
///   attempt's hold for the clone to end (Go's head start).
/// - Then a file event cancels the warm, and the warm stops at its next
///   context check. The handler's own `cancel_warm_auto_import_cache` then
///   finds the warm done, as Go's does when the warm has ended.
/// - Any other message yields an eager attempt: the clone stops at its next
///   context check and the warm tries again later. During an attempt that
///   is not eager, such a message waits for the whole clone.
///
/// It holds the same context and cancel function as
/// `Session::warm_auto_import_cancel`, and is set and cleared with it. One
/// mutex covers the warm, its attempt and the queueing of a message, so an
/// attempt does not start while a message waits (`set_busy`), and after an
/// attempt ends the reader neither cancels nor yields it.
#[derive(Clone, Default)]
pub struct WarmAutoImportPreempt(Arc<WarmAutoImportPreemptShared>);

#[derive(Default)]
struct WarmAutoImportPreemptShared {
    entry: Mutex<Option<WarmAutoImportPreemptEntry>>,
    /// Signalled when an attempt ends.
    attempt_ended: Condvar,
    /// Whether a message waits for the dispatch thread (`set_busy`).
    busy: OnceLock<Box<dyn Fn() -> bool + Send + Sync>>,
}

struct WarmAutoImportPreemptEntry {
    ctx: Context,
    cancel: gostd::context::CancelFunc,
    file_name: String,
    /// The clone attempt that runs now: its cancel function and its hold
    /// (`None` when it is not eager).
    attempt: Option<(gostd::context::CancelFunc, Option<Duration>)>,
    /// Whether the clone ended. Go's warm has then ended, and its cancel
    /// does nothing.
    done: bool,
}

impl WarmAutoImportPreempt {
    fn entry(&self) -> MutexGuard<'_, Option<WarmAutoImportPreemptEntry>> {
        // PORT: Go mutexes do not poison.
        self.0.entry.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn set(&self, ctx: Context, cancel: gostd::context::CancelFunc, file_name: String) {
        *self.entry() = Some(WarmAutoImportPreemptEntry {
            ctx,
            cancel,
            file_name,
            attempt: None,
            done: false,
        });
    }

    fn clear(&self) {
        *self.entry() = None;
    }

    /// Sets the check that a message waits for the dispatch thread (the LSP
    /// server's request queue). An attempt does not start while it is true.
    pub fn set_busy(&self, busy: Box<dyn Fn() -> bool + Send + Sync>) {
        let _ = self.0.busy.set(busy);
    }

    /// Whether a message waits for the dispatch thread (`set_busy`).
    fn busy(&self) -> bool {
        self.0.busy.get().is_some_and(|busy| busy())
    }

    /// Go `cancelWarmAutoImportCache`, with the log line of the stored
    /// cancel function. Safe to call from any thread.
    pub fn cancel(&self, logger: &dyn logging::Logger) {
        Self::cancel_entry(&mut self.entry(), logger);
    }

    fn cancel_entry(entry: &mut Option<WarmAutoImportPreemptEntry>, logger: &dyn logging::Logger) {
        let Some(entry) = entry.take() else {
            return;
        };
        if entry.done || entry.ctx.err().is_some() {
            return;
        }
        logger.logf(&format!(
            "Cancelling auto-import warming for file {}",
            entry.file_name
        ));
        (entry.cancel)();
    }

    /// The LSP reader thread calls this for each message that it queues for
    /// the dispatch thread, with `queue`, which queues it. `file_event`: the
    /// message's Go handler cancels the warm. See the type doc.
    pub fn on_message<R>(
        &self,
        file_event: bool,
        logger: &dyn logging::Logger,
        queue: impl FnOnce() -> R,
    ) -> R {
        let mut entry = self.entry();
        let hold = entry
            .as_ref()
            .and_then(|e| e.attempt.as_ref())
            .and_then(|(_, hold)| *hold);
        let Some(hold) = hold else {
            if file_event {
                Self::cancel_entry(&mut entry, logger);
            }
            return queue();
        };
        let result = queue();
        let deadline = Instant::now() + hold;
        loop {
            let now = Instant::now();
            if now >= deadline || entry.as_ref().is_none_or(|e| e.attempt.is_none()) {
                break;
            }
            entry = self
                .0
                .attempt_ended
                .wait_timeout(entry, deadline - now)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        if file_event {
            Self::cancel_entry(&mut entry, logger);
        } else if let Some((yield_attempt, Some(_))) =
            entry.as_ref().and_then(|e| e.attempt.as_ref())
        {
            yield_attempt();
        }
        result
    }

    /// Starts a clone attempt, unless a message waits.
    fn start_attempt(&self, cancel: gostd::context::CancelFunc, hold: Option<Duration>) -> bool {
        let mut entry = self.entry();
        if self.busy() {
            return false;
        }
        if let Some(entry) = entry.as_mut() {
            entry.attempt = Some((cancel, hold));
        }
        true
    }

    /// Ends the clone attempt that runs, and says how it ended.
    fn end_attempt(&self, warm_ctx: &Context, attempt_ctx: &Context) -> WarmAttemptEnd {
        let mut entry = self.entry();
        let end = if warm_ctx.err().is_some() {
            WarmAttemptEnd::Cancelled
        } else if attempt_ctx.err().is_some() {
            WarmAttemptEnd::Yielded
        } else {
            WarmAttemptEnd::Done
        };
        if let Some(entry) = entry.as_mut() {
            entry.attempt = None;
            entry.done = end == WarmAttemptEnd::Done;
        }
        self.0.attempt_ended.notify_all();
        end
    }
}

// Go: project/session.go:661 idleCacheCleanDelay
pub const IDLE_CACHE_CLEAN_DELAY: Duration = Duration::from_secs(30);

impl Session {
    // Go: project/session.go:663 scheduleIdleCacheClean
    pub fn schedule_idle_cache_clean(self: &Rc<Self>) {
        if let Some(timer) = self.idle_cache_clean_timer.borrow().as_ref() {
            timer.stop();
        }

        let s = self.clone();
        let timer = gostd::local::after_func(
            IDLE_CACHE_CLEAN_DELAY,
            Box::new(move || {
                *s.idle_cache_clean_timer.borrow_mut() = None;

                s.cancel_scheduled_snapshot_update();

                let ctx = s.background_context();
                let (file_changes, overlays, ata_changes, new_config) = s.flush_changes(&ctx);
                s.update_snapshot_exported(
                    &ctx,
                    overlays,
                    SnapshotChange {
                        reason: UpdateReason::IDLE_CLEAN_DISK_CACHE,
                        file_changes,
                        ata_changes,
                        new_config,
                        clean_file_cache: true,
                        ..Default::default()
                    },
                );

                // Go: go func() { runtime.GC() }()
                // PORT: skipped. Rust has no garbage collector; memory the
                // new snapshot released is freed when its last owner drops.
            }),
        );
        *self.idle_cache_clean_timer.borrow_mut() = Some(timer);
    }

    // Go: project/session.go:694 cancelIdleCacheClean
    pub fn cancel_idle_cache_clean(&self) {
        let timer = self.idle_cache_clean_timer.borrow_mut().take();
        if let Some(timer) = timer {
            timer.stop();
        }
    }
}

// Go: project/session.go:703 performanceTelemetryInterval
pub const PERFORMANCE_TELEMETRY_INTERVAL: Duration = Duration::from_secs(5 * 60);

// PORT: Go `runtime/metrics.Sample` and `metrics.Value` (Go standard
// library). Only the shape is ported: the values describe the Go runtime.
#[derive(Clone, Debug, Default)]
struct MetricsSample {
    name: &'static str,
    value: MetricsValue,
}

// PORT: Go `metrics.Value` by `Kind()`.
#[derive(Clone, Copy, Debug, Default)]
enum MetricsValue {
    #[default]
    Bad,
    Uint64(u64),
    Float64(f64),
    Float64Histogram,
}

// Go: runtime/metrics/sample.go:45 Read (go1.27.1), which runs
// runtime/metrics.go:1029 readMetricsLocked. go1.27.1 returns early for no
// samples; the loop below does nothing then too.
// PORT: Go computes each metric from Go runtime statistics (heap, GC,
// scheduler, goroutines). The port has no Go runtime, so it has only
// `/sched/gomaxprocs:threads` (`gostd::runtime::gomaxprocs`, as Go
// runtime/metrics.go `gomaxprocs`). Go gives `KindBad` for a name it does
// not have, so every other sample is `KindBad` and the other Go runtime
// fields of the telemetry event are 0 (`memoryUsedBytes`, `goMemLimit`,
// `goGCPercent`, the heap and GC fields, `goroutineCount`, `gcCpuSeconds`,
// `userCpuSeconds`).
fn metrics_read(samples: &mut [MetricsSample]) {
    // Sample.
    for sample in samples.iter_mut() {
        sample.value = match sample.name {
            "/sched/gomaxprocs:threads" => {
                MetricsValue::Uint64(crate::gostd::runtime::gomaxprocs() as u64)
            }
            _ => MetricsValue::Bad,
        };
    }
}

// PORT: Go `go-osstat/memory.Stats` (the two fields session.go reads).
struct OsMemoryStats {
    total: u64,
    used: u64,
}

// Go: github.com/mackerelio/go-osstat@v0.2.7 memory/memory_linux.go:15 Get
// Get memory statistics
fn osmemory_get() -> Result<OsMemoryStats, GoError> {
    // Reference: man 5 proc, Documentation/filesystems/proc.txt in Linux source code
    let mut file = match std::fs::File::open("/proc/meminfo") {
        Ok(file) => file,
        Err(err) => return Err(crate::pprof::path_error("open", "/proc/meminfo", &err)),
    };
    // Go: defer file.Close(). The file closes when it drops.
    collect_memory_stats(&mut file)
}

// Go: github.com/mackerelio/go-osstat@v0.2.7 memory/memory_linux.go:33 collectMemoryStats
// PORT: Go fills every `Stats` field. The port keeps the fields that
// `Total` and `Used` need (`OsMemoryStats`); the other `memStats` names set
// only fields that nobody reads. Go reads lines with `bufio.Scanner`, which
// stops at the first read error; the port reads the whole file first. A
// meminfo line is never longer than the 64 KiB scanner limit.
fn collect_memory_stats(out: &mut dyn std::io::Read) -> Result<OsMemoryStats, GoError> {
    let mut data = Vec::new();
    if let Err(err) = out.read_to_end(&mut data) {
        return Err(gostd::errors::new(format!(
            "scan error for /proc/meminfo: {err}"
        )));
    }
    let (mut total, mut free, mut available, mut buffers, mut cached) =
        (0u64, 0u64, 0u64, 0u64, 0u64);
    let mut mem_available_enabled = false;
    // Go: bufio.ScanLines. A line ends at "\n" and loses a final "\r"; the
    // empty piece after the last "\n" has no ':' and is skipped.
    for line in data.split(|&c| c == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        let Some(i) = line.iter().position(|&c| c == b':') else {
            continue;
        };
        let fld = &line[..i];
        let ptr = match fld {
            b"MemTotal" => &mut total,
            b"MemFree" => &mut free,
            b"MemAvailable" => &mut available,
            b"Buffers" => &mut buffers,
            b"Cached" => &mut cached,
            _ => continue,
        };
        // Go: strings.TrimSpace(strings.TrimRight(line[i+1:], "kB"))
        let mut val = &line[i + 1..];
        while let [rest @ .., b'k' | b'B'] = val {
            val = rest;
        }
        // PORT: Go TrimSpace also trims non-ASCII spaces, which meminfo does
        // not have.
        let is_space = |c: &u8| matches!(c, b'\t' | b'\n' | 0x0b | 0x0c | b'\r' | b' ');
        while let [c, rest @ ..] = val
            && is_space(c)
        {
            val = rest;
        }
        while let [rest @ .., c] = val
            && is_space(c)
        {
            val = rest;
        }
        // Go: strconv.ParseUint(val, 10, 64). Rust also takes a leading '+'.
        if val.first() != Some(&b'+')
            && let Some(v) = std::str::from_utf8(val)
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
        {
            *ptr = v.wrapping_mul(1024);
        }
        if fld == b"MemAvailable" {
            mem_available_enabled = true;
        }
    }

    let used = if mem_available_enabled {
        total.wrapping_sub(available)
    } else {
        total
            .wrapping_sub(free)
            .wrapping_sub(buffers)
            .wrapping_sub(cached)
    };

    Ok(OsMemoryStats { total, used })
}

impl Session {
    // Go: project/session.go:707 StartPerformanceTelemetry
    // StartPerformanceTelemetry begins periodic collection and sending of performance
    // telemetry. It should be called once after the session is initialized.
    // PORT: Go loops over a `time.Ticker` in the background task. Here the
    // task arms a `gostd::local::after_func` timer that re-arms itself for
    // the next tick until the context is done (checked on each tick).
    pub fn start_performance_telemetry(self: &Rc<Self>) {
        if !self.options.telemetry_enabled {
            return;
        }
        let (ctx, cancel) = gostd::context::with_cancel(&self.background_context());
        *self.performance_telemetry_cancel.borrow_mut() = Some(cancel);
        let s = self.clone();
        self.background_queue.enqueue(&ctx, move |ctx| {
            let ctx = ctx.clone();
            let ticker: Rc<RefCell<Option<gostd::local::LocalTimer>>> = Rc::new(RefCell::new(None));
            let tick_ticker = ticker.clone();
            let timer = gostd::local::after_func(
                PERFORMANCE_TELEMETRY_INTERVAL,
                Box::new(move || {
                    // Go: each tick runs in the same wg.Go goroutine.
                    crate::core::go_wait_group_task(|| {
                        // Go: case <-ctx.Done(): return (defer ticker.Stop())
                        if ctx.err().is_some() {
                            let stopped = tick_ticker.borrow_mut().take();
                            if let Some(stopped) = stopped {
                                stopped.stop();
                            }
                            return;
                        }
                        // Go: case <-ticker.C (the ticker sends the next tick after the interval)
                        if let Some(t) = tick_ticker.borrow().as_ref() {
                            t.reset(PERFORMANCE_TELEMETRY_INTERVAL);
                        }
                        if s.client.is_none()
                            || !s
                                .client
                                .as_ref()
                                .unwrap_or_else(|| crate::core::go_nil_dereference())
                                .is_active()
                        {
                            return; // Go: continue
                        }
                        s.send_performance_telemetry(&ctx);
                    })
                }),
            );
            *ticker.borrow_mut() = Some(timer);
        });
    }

    // Go: project/session.go:730 stopPerformanceTelemetry
    pub fn stop_performance_telemetry(&self) {
        let cancel = self.performance_telemetry_cancel.borrow().clone();
        if let Some(cancel) = cancel {
            cancel();
            *self.performance_telemetry_cancel.borrow_mut() = None;
        }
    }

    // Go: project/session.go:737 sendPerformanceTelemetry
    pub fn send_performance_telemetry(&self, ctx: &Context) {
        if self.client.is_none() || !self.options.telemetry_enabled {
            return;
        }
        let snapshot = self.snapshot.borrow().clone();

        // Read Go runtime metrics in a single call
        const S_MEMORY_USED_BYTES: usize = 0;
        const S_GO_MEM_LIMIT: usize = 1;
        const S_GO_GC_PERCENT: usize = 2;
        const S_HEAP_GOAL_BYTES: usize = 3;
        const S_HEAP_LIVE_BYTES: usize = 4;
        const S_HEAP_OBJECT_COUNT: usize = 5;
        const S_HEAP_STACK_BYTES: usize = 6;
        const S_HEAP_RELEASED_BYTES: usize = 7;
        const S_HEAP_FREE_BYTES: usize = 8;
        const S_GC_SCAN_HEAP_BYTES: usize = 9;
        const S_GO_MAX_PROCS: usize = 10;
        const S_GOROUTINE_COUNT: usize = 11;
        const S_GC_CYCLES_TOTAL: usize = 12;
        const S_GC_CPU_SECONDS: usize = 13;
        const S_USER_CPU_SECONDS: usize = 14;
        const S_METRIC_COUNT: usize = 15;
        let mut samples: Vec<MetricsSample> = vec![MetricsSample::default(); S_METRIC_COUNT];
        samples[S_MEMORY_USED_BYTES].name = "/memory/classes/total:bytes";
        samples[S_GO_MEM_LIMIT].name = "/gc/gomemlimit:bytes";
        samples[S_GO_GC_PERCENT].name = "/gc/gogc:percent";
        samples[S_HEAP_GOAL_BYTES].name = "/gc/heap/goal:bytes";
        samples[S_HEAP_LIVE_BYTES].name = "/gc/heap/live:bytes";
        samples[S_HEAP_OBJECT_COUNT].name = "/gc/heap/objects:objects";
        samples[S_HEAP_STACK_BYTES].name = "/memory/classes/heap/stacks:bytes";
        samples[S_HEAP_RELEASED_BYTES].name = "/memory/classes/heap/released:bytes";
        samples[S_HEAP_FREE_BYTES].name = "/memory/classes/heap/free:bytes";
        samples[S_GC_SCAN_HEAP_BYTES].name = "/gc/scan/heap:bytes";
        samples[S_GO_MAX_PROCS].name = "/sched/gomaxprocs:threads";
        samples[S_GOROUTINE_COUNT].name = "/sched/goroutines:goroutines";
        samples[S_GC_CYCLES_TOTAL].name = "/gc/cycles/total:gc-cycles";
        samples[S_GC_CPU_SECONDS].name = "/cpu/classes/gc/total:cpu-seconds";
        samples[S_USER_CPU_SECONDS].name = "/cpu/classes/user:cpu-seconds";
        metrics_read(&mut samples);

        let mut measurements = lsproto::PerformanceStatsTelemetryMeasurements {
            open_file_count: snapshot.overlays().len() as f64,
            uptime_seconds: self.start_time.elapsed().as_secs_f64(),
            project_count: snapshot.project_collection.projects().len() as f64,
            config_count: snapshot.config_file_registry.configs.len() as f64,
            cached_disk_file_count: snapshot.fs.cache_files.len() as f64,
            ..Default::default()
        };

        let read_uint64 = |s: &MetricsSample| -> f64 {
            if let MetricsValue::Uint64(v) = s.value {
                return v as f64;
            }
            0.0
        };
        let read_float64 = |s: &MetricsSample| -> f64 {
            if let MetricsValue::Float64(v) = s.value {
                return v;
            }
            0.0
        };

        measurements.memory_used_bytes = read_uint64(&samples[S_MEMORY_USED_BYTES]);
        if let MetricsValue::Uint64(v) = samples[S_GO_MEM_LIMIT].value {
            if v < i64::MAX as u64 {
                measurements.go_mem_limit = v as f64;
            }
            // else: default (MaxInt64) exceeds MAX_SAFE_INTEGER; leave as 0 to indicate unconfigured
        }
        measurements.go_gc_percent = read_uint64(&samples[S_GO_GC_PERCENT]);
        measurements.heap_goal_bytes = read_uint64(&samples[S_HEAP_GOAL_BYTES]);
        measurements.heap_live_bytes = read_uint64(&samples[S_HEAP_LIVE_BYTES]);
        measurements.heap_object_count = read_uint64(&samples[S_HEAP_OBJECT_COUNT]);
        measurements.heap_stack_bytes = read_uint64(&samples[S_HEAP_STACK_BYTES]);
        measurements.heap_released_bytes = read_uint64(&samples[S_HEAP_RELEASED_BYTES]);
        measurements.heap_free_bytes = read_uint64(&samples[S_HEAP_FREE_BYTES]);
        measurements.gc_scan_heap_bytes = read_uint64(&samples[S_GC_SCAN_HEAP_BYTES]);
        measurements.go_max_procs = read_uint64(&samples[S_GO_MAX_PROCS]);
        measurements.goroutine_count = read_uint64(&samples[S_GOROUTINE_COUNT]);
        measurements.gc_cycles_total = read_uint64(&samples[S_GC_CYCLES_TOTAL]);
        measurements.gc_cpu_seconds = read_float64(&samples[S_GC_CPU_SECONDS]);
        measurements.user_cpu_seconds = read_float64(&samples[S_USER_CPU_SECONDS]);

        // Read system memory stats
        if let Ok(sys_mem) = osmemory_get() {
            measurements.system_mem_total = sys_mem.total as f64;
            measurements.system_mem_used = sys_mem.used as f64;
        }

        // Read auto-import registry stats
        if let Some(registry) = snapshot.auto_import_registry() {
            let auto_import_stats = registry.get_cache_stats();
            measurements.auto_import_project_bucket_count =
                auto_import_stats.project_buckets.len() as f64;
            measurements.auto_import_node_modules_bucket_count =
                auto_import_stats.node_modules_buckets.len() as f64;
            measurements.auto_import_unique_package_count =
                auto_import_stats.unique_package_count as f64;
            for b in &auto_import_stats.project_buckets {
                measurements.auto_import_project_export_count += b.export_count as f64;
                measurements.auto_import_project_file_count += b.file_count as f64;
            }
            for b in &auto_import_stats.node_modules_buckets {
                measurements.auto_import_node_modules_export_count += b.export_count as f64;
                measurements.auto_import_node_modules_file_count += b.file_count as f64;
                if b.dependency_names.is_none() {
                    measurements.auto_import_node_modules_unfiltered_bucket_count += 1.0;
                }
            }
        }

        if let Err(err) = self
            .client
            .as_ref()
            .unwrap_or_else(|| crate::core::go_nil_dereference())
            .send_telemetry(
                ctx,
                lsproto::TelemetryEvent {
                    performance_stats_telemetry_event: Some(
                        lsproto::PerformanceStatsTelemetryEvent {
                            measurements: Some(measurements),
                            ..Default::default()
                        },
                    ),
                    ..Default::default()
                },
            )
        {
            if self.options.logging_enabled {
                self.logger.logf(&format!(
                    "Error sending performance telemetry: {}",
                    err.error()
                ));
            }
        }
    }

    // Go: project/session.go:859 sendProjectInfoTelemetryForNewProjects
    pub fn send_project_info_telemetry_for_new_projects(
        &self,
        old_snapshot: &Rc<Snapshot>,
        new_snapshot: &Rc<Snapshot>,
    ) {
        if !self.options.telemetry_enabled {
            return;
        }
        let ctx = self.background_context();
        crate::frontend::core_ls_ext::diff_ordered_maps(
            &old_snapshot.project_collection.projects_by_id(),
            &new_snapshot.project_collection.projects_by_id(),
            |_: &ID, added_project| {
                self.send_project_info_telemetry(&ctx, added_project);
            },
            |_: &ID, _| {},
            |_: &ID, _, _| {},
        );
    }

    // Go: project/session.go:875 sendProjectInfoTelemetry
    pub fn send_project_info_telemetry(&self, ctx: &Context, project: &Rc<RefCell<Project>>) {
        if self.client.is_none() || !self.options.telemetry_enabled {
            return;
        }
        let project = project.borrow();
        if self.seen_projects.borrow().contains(&project.id()) {
            return;
        }

        if project.program.is_none() || project.command_line.is_none() {
            return;
        }

        let info = self.collect_project_info_telemetry(&project);
        if let Err(err) = self
            .client
            .as_ref()
            .unwrap_or_else(|| crate::core::go_nil_dereference())
            .send_telemetry(ctx, info)
        {
            if self.options.logging_enabled {
                self.logger.logf(&format!(
                    "Error sending project info telemetry: {}",
                    err.error()
                ));
            }
            return;
        }

        self.seen_projects.borrow_mut().insert(project.id());
    }

    // Go: project/session.go:898 collectProjectInfoTelemetry
    // PORT: Go `map[string]string` and `map[string]any` are `IndexMap`s in
    // insertion order (PORT: Go map order is random, also in its JSON).
    pub fn collect_project_info_telemetry(&self, project: &Project) -> lsproto::TelemetryEvent {
        // Go `CompilerOptions` is nil-safe, and a nil result becomes an empty
        // `core.CompilerOptions`.
        let opts = project.command_line.as_ref().map_or_else(
            || Rc::new(CompilerOptions::default()),
            |command_line| command_line.compiler_options().clone(),
        );

        let mut config_file_name = "other".to_string();
        if project.kind == Kind::CONFIGURED {
            let base_name = tspath::get_base_file_name(&project.config_file_name());
            if base_name == "tsconfig.json" || base_name == "jsconfig.json" {
                config_file_name = base_name;
            }
        }

        let mut project_type = "inferred";
        if project.kind == Kind::CONFIGURED {
            project_type = "configured";
        }

        let mut props: IndexMap<String, String> = IndexMap::default();
        props.insert("configFileName".to_string(), config_file_name);
        props.insert("projectType".to_string(), project_type.to_string());
        props.insert("version".to_string(), crate::core::version().to_string());

        // Compiler options — same approach as Strada's convertCompilerOptionsForTelemetry:
        // booleans and enum string names, no paths.
        let mut compiler_options: IndexMap<String, LspAny> = IndexMap::default();
        set_tristate(&mut compiler_options, "strict", opts.strict);
        set_tristate(&mut compiler_options, "noImplicitAny", opts.no_implicit_any);
        set_tristate(
            &mut compiler_options,
            "noImplicitThis",
            opts.no_implicit_this,
        );
        set_tristate(
            &mut compiler_options,
            "strictNullChecks",
            opts.strict_null_checks,
        );
        set_tristate(
            &mut compiler_options,
            "strictFunctionTypes",
            opts.strict_function_types,
        );
        set_tristate(
            &mut compiler_options,
            "strictBindCallApply",
            opts.strict_bind_call_apply,
        );
        set_tristate(
            &mut compiler_options,
            "strictPropertyInitialization",
            opts.strict_property_initialization,
        );
        set_tristate(
            &mut compiler_options,
            "strictBuiltinIteratorReturn",
            opts.strict_builtin_iterator_return,
        );
        set_tristate(
            &mut compiler_options,
            "useUnknownInCatchVariables",
            opts.use_unknown_in_catch_variables,
        );
        set_tristate(
            &mut compiler_options,
            "exactOptionalPropertyTypes",
            opts.exact_optional_property_types,
        );
        set_tristate(&mut compiler_options, "allowJs", opts.allow_js);
        set_tristate(&mut compiler_options, "checkJs", opts.check_js);
        set_tristate(&mut compiler_options, "noEmit", opts.no_emit);
        set_tristate(&mut compiler_options, "declaration", opts.declaration);
        set_tristate(&mut compiler_options, "composite", opts.composite);
        set_tristate(
            &mut compiler_options,
            "isolatedModules",
            opts.isolated_modules,
        );
        set_tristate(&mut compiler_options, "skipLibCheck", opts.skip_lib_check);
        set_tristate(&mut compiler_options, "incremental", opts.incremental);
        if opts.target != ScriptTarget::NONE {
            compiler_options.insert("target".to_string(), LspAny::String(opts.target.string()));
        }
        if opts.module != ModuleKind::NONE {
            compiler_options.insert("module".to_string(), LspAny::String(opts.module.string()));
        }
        if opts.module_resolution != ModuleResolutionKind::UNKNOWN {
            compiler_options.insert(
                "moduleResolution".to_string(),
                LspAny::String(opts.module_resolution.string()),
            );
        }
        if opts.jsx != JsxEmit::NONE {
            compiler_options.insert("jsx".to_string(), LspAny::String(opts.jsx.string()));
        }
        if let Ok(b) = crate::frontend::json::json_marshal(&compiler_options, &[]) {
            props.insert("compilerOptions".to_string(), b);
        }

        // Config file shape
        // PORT: Go `Raw.(*collections.OrderedMap[string, any])` is the
        // `CompilerOptionsValue::Map` form of `raw`. Go reads the field of a
        // nil command line here.
        let command_line = project
            .command_line
            .as_ref()
            .unwrap_or_else(|| crate::core::go_nil_dereference());
        if let tsoptions::CompilerOptionsValue::Map(raw) = &command_line.raw {
            props.insert(
                "extends".to_string(),
                bool_telemetry(raw.contains_key("extends")),
            );
            props.insert(
                "files".to_string(),
                bool_telemetry(raw.contains_key("files")),
            );
            props.insert(
                "include".to_string(),
                bool_telemetry(raw.contains_key("include")),
            );
            props.insert(
                "exclude".to_string(),
                bool_telemetry(raw.contains_key("exclude")),
            );
        }

        lsproto::TelemetryEvent {
            project_info_telemetry_event: Some(lsproto::ProjectInfoTelemetryEvent {
                properties: props,
                measurements: Some(count_file_stats(
                    project
                        .program
                        .as_ref()
                        .unwrap_or_else(|| crate::core::go_nil_dereference())
                        .get_source_files(),
                )),
                ..Default::default()
            }),
            ..Default::default()
        }
    }
}

// Go: project/session.go:976 setTristate
pub fn set_tristate(m: &mut IndexMap<String, LspAny>, key: &str, v: Tristate) {
    if v == Tristate::True {
        m.insert(key.to_string(), LspAny::Bool(true));
    } else if v == Tristate::False {
        m.insert(key.to_string(), LspAny::Bool(false));
    }
}

// Go: project/session.go:984 boolTelemetry
pub fn bool_telemetry(v: bool) -> String {
    if v {
        return "true".to_string();
    }
    "false".to_string()
}

// Go: project/session.go:991 countFileStats
// PORT: Go returns `*lsproto.ProjectInfoTelemetryMeasurements`; the event
// field holds the value.
pub fn count_file_stats(
    source_files: &[Rc<crate::frontend::parser::ParsedSourceFile>],
) -> lsproto::ProjectInfoTelemetryMeasurements {
    let mut stats = lsproto::ProjectInfoTelemetryMeasurements::default();
    for sf in source_files {
        let size = sf.root.end() as f64;
        match sf.script_kind {
            ScriptKind::JS => {
                stats.js_file_count += 1.0;
                stats.js_file_size += size;
            }
            ScriptKind::JSX => {
                stats.jsx_file_count += 1.0;
                stats.jsx_file_size += size;
            }
            ScriptKind::TS => {
                if tspath::is_declaration_file_name(sf.file_name()) {
                    stats.dts_file_count += 1.0;
                    stats.dts_file_size += size;
                } else {
                    stats.ts_file_count += 1.0;
                    stats.ts_file_size += size;
                }
            }
            ScriptKind::TSX => {
                stats.tsx_file_count += 1.0;
                stats.tsx_file_size += size;
            }
            _ => {}
        }
    }
    stats
}

impl Session {
    // Go: project/session.go:1018 Snapshot
    pub fn snapshot(&self) -> Rc<Snapshot> {
        self.snapshot.borrow().clone()
    }

    // Go: project/session.go:1028 getSnapshot
    // getSnapshot flushes pending changes and updates the session's snapshot
    // if needed for the given request. When callerRef is true, the returned
    // snapshot has an extra reference for the caller (taken atomically under
    // snapshotMu), guaranteeing it stays alive until the caller calls Deref.
    pub fn get_snapshot(
        self: &Rc<Self>,
        ctx: &Context,
        request: ResourceRequest,
        caller_ref: bool,
    ) -> Rc<Snapshot> {
        self.cancel_scheduled_snapshot_update();

        let (file_changes, overlays, ata_changes, new_config) = self.flush_changes(ctx);
        let update_snapshot =
            !file_changes.is_empty() || !ata_changes.is_empty() || new_config.is_some();
        if update_snapshot {
            // If there are pending file changes, we need to update the snapshot.
            // Sending the requested URI ensures that the project for this URI is loaded.
            return self
                .update_snapshot(
                    ctx,
                    overlays,
                    SnapshotChange {
                        reason: UpdateReason::REQUESTED_LANGUAGE_SERVICE_PENDING_CHANGES,
                        file_changes,
                        ata_changes,
                        new_config,
                        resource_request: request,
                        ..Default::default()
                    },
                    caller_ref,
                )
                // PORT: no API request here, so the result is never nil (ts#64204).
                .expect("updateSnapshot without an API request returns the snapshot");
        }
        // If there are no pending file changes, we can try to use the current snapshot.
        let snapshot = self.snapshot.borrow().clone();
        let mut update_reason = UpdateReason::UNKNOWN;
        if !request.projects.is_empty() {
            update_reason = UpdateReason::REQUESTED_LANGUAGE_SERVICE_PROJECT_DIRTY;
        } else if request.project_tree.is_some() {
            update_reason = UpdateReason::REQUESTED_LOAD_PROJECT_TREE;
        } else if !request.auto_imports.0.is_empty() {
            update_reason = UpdateReason::REQUESTED_LANGUAGE_SERVICE_WITH_AUTO_IMPORTS;
        } else {
            for document in &request.documents {
                match snapshot.get_default_project(document) {
                    None => {
                        update_reason = UpdateReason::REQUESTED_LANGUAGE_SERVICE_PROJECT_NOT_LOADED;
                    }
                    Some(project) => {
                        if project.borrow().dirty {
                            update_reason = UpdateReason::REQUESTED_LANGUAGE_SERVICE_PROJECT_DIRTY;
                        }
                    }
                }
            }
            if update_reason == UpdateReason::UNKNOWN {
                for document in &request.configured_project_documents {
                    if snapshot.is_open_file(&document.file_name()) {
                        match snapshot.get_default_project(document) {
                            None => {
                                update_reason =
                                    UpdateReason::REQUESTED_LANGUAGE_SERVICE_PROJECT_NOT_LOADED;
                            }
                            Some(project) => {
                                if project.borrow().dirty {
                                    update_reason =
                                        UpdateReason::REQUESTED_LANGUAGE_SERVICE_PROJECT_DIRTY;
                                }
                            }
                        }
                    } else {
                        update_reason = UpdateReason::REQUESTED_LANGUAGE_SERVICE_FOR_FILE_NOT_OPEN;
                    }
                }
            }
        }
        if update_reason == UpdateReason::UNKNOWN {
            if caller_ref {
                snapshot.ref_();
            }
            return snapshot;
        }

        self.update_snapshot(
            ctx,
            overlays,
            SnapshotChange {
                reason: update_reason,
                resource_request: request,
                ..Default::default()
            },
            caller_ref,
        )
        // PORT: no API request here, so the result is never nil (ts#64204).
        .expect("updateSnapshot without an API request returns the snapshot")
    }

    // Go: project/session.go:1099 getSnapshotAndDefaultProject
    // PORT: Go `project.GetProgram()` may be nil and `ls.NewLanguageService`
    // keeps it; the Rust language service needs a program, so a nil one
    // panics here (Go panics on first use).
    pub fn get_snapshot_and_default_project(
        self: &Rc<Self>,
        ctx: &Context,
        uri: &lsproto::DocumentUri,
        caller_ref: bool,
    ) -> Result<(Rc<Snapshot>, Rc<RefCell<Project>>, ls::LanguageService), GoError> {
        let snapshot = self.get_snapshot(
            ctx,
            ResourceRequest {
                documents: vec![uri.clone()],
                ..Default::default()
            },
            caller_ref,
        );
        let Some(project) = snapshot.get_default_project(uri) else {
            // tsgo#4712
            if caller_ref {
                snapshot.deref();
            }
            if let Some(file) = snapshot.get_file(&uri.file_name())
                && file.kind() == ScriptKind::UNKNOWN
            {
                return Err(gostd::errors::errorf(
                    format!(
                        "{}: no project found for URI {}",
                        ERR_NO_PROJECT_FOR_UNKNOWN_SCRIPT_KIND.error(),
                        uri
                    ),
                    vec![ERR_NO_PROJECT_FOR_UNKNOWN_SCRIPT_KIND.clone()],
                ));
            }
            return Err(gostd::errors::errorf(
                format!("no project found for URI {}", uri),
                vec![],
            ));
        };
        let language_service = {
            let p = project.borrow();
            ls::new_language_service(
                p.id().as_auto_import_project_id(),
                p.program
                    .clone()
                    .unwrap_or_else(|| crate::core::go_nil_dereference()),
                snapshot.clone(),
                &uri.file_name(),
            )
        };
        Ok((snapshot, project, language_service))
    }

    // Go: project/session.go:1118 GetLanguageService
    pub fn get_language_service(
        self: &Rc<Self>,
        ctx: &Context,
        uri: &lsproto::DocumentUri,
    ) -> Result<ls::LanguageService, GoError> {
        let (_, _, language_service) =
            self.get_snapshot_and_default_project(ctx, uri, false /*callerRef*/)?;
        Ok(language_service)
    }

    // Go: project/session.go:1126 GetLanguageServiceAndProjectsForFile
    // PORT: Go `[]ls.Project` is `Vec<Rc<dyn ls::Project>>`.
    pub fn get_language_service_and_projects_for_file(
        self: &Rc<Self>,
        ctx: &Context,
        uri: &lsproto::DocumentUri,
    ) -> Result<
        (
            Rc<RefCell<Project>>,
            ls::LanguageService,
            Vec<Rc<dyn ls::Project>>,
        ),
        GoError,
    > {
        let (snapshot, project, default_ls) =
            self.get_snapshot_and_default_project(ctx, uri, false /*callerRef*/)?;
        // !!! TODO: sheetal:  Get other projects that contain the file with symlink
        let all_projects = snapshot.get_language_service_projects_containing_file(uri);
        Ok((project, default_ls, all_projects))
    }

    // Go: project/session.go:1136 GetProjectsForFile
    pub fn get_projects_for_file(
        self: &Rc<Self>,
        ctx: &Context,
        uri: &lsproto::DocumentUri,
    ) -> Result<Vec<Rc<dyn ls::Project>>, GoError> {
        let snapshot = self.get_snapshot(
            ctx,
            ResourceRequest {
                configured_project_documents: vec![uri.clone()],
                ..Default::default()
            },
            false, /*callerRef*/
        );

        // !!! TODO: sheetal:  Get other projects that contain the file with symlink
        let all_projects = snapshot.get_language_service_projects_containing_file(uri);
        Ok(all_projects)
    }

    // Go: project/session.go:1153 GetLanguageServicesForDocumentsLoadingProjectTree
    // GetLanguageServicesForDocumentsLoadingProjectTree returns language services for
    // every project in the snapshot, loading all project trees first so that projects
    // that were never opened but reference the given documents are included. Loading the
    // trees is expensive, so this should only be used by operations that need to touch
    // every project in a solution, like file rename.
    pub fn get_language_services_for_documents_loading_project_tree(
        self: &Rc<Self>,
        ctx: &Context,
        uris: &[lsproto::DocumentUri],
    ) -> Vec<ls::LanguageService> {
        let snapshot = self.get_snapshot(
            ctx,
            ResourceRequest {
                documents: uris.to_vec(),
                project_tree: Some(ProjectTreeRequest::default()),
                ..Default::default()
            },
            false, /*callerRef*/
        );

        let mut active_file = String::new();
        if !uris.is_empty() {
            active_file = uris[0].file_name();
        }

        let projects = snapshot.project_collection.language_service_projects();
        let mut services: Vec<ls::LanguageService> = Vec::with_capacity(projects.len());
        for project in &projects {
            let project = project.borrow();
            let Some(program) = project.program.clone() else {
                continue;
            };

            services.push(ls::new_language_service(
                project.id().as_auto_import_project_id(),
                program,
                snapshot.clone(),
                &active_file,
            ));
        }
        services
    }

    // Go: project/session.go:1181 GetLanguageServiceForProjectWithFile
    // PORT: the Go server passes `p.(*project.Project)` (a type assertion on
    // an `ls.Project`). Only `Id()` of the argument is read, so the port
    // takes the `ls::Project` interface itself.
    pub fn get_language_service_for_project_with_file(
        self: &Rc<Self>,
        ctx: &Context,
        project: &dyn ls::Project,
        uri: &lsproto::DocumentUri,
    ) -> Option<ls::LanguageService> {
        let snapshot = self.get_snapshot(
            ctx,
            ResourceRequest {
                projects: vec![ID(project.id())],
                ..Default::default()
            },
            false, /*callerRef*/
        );
        // Ensure we have updated project
        let project = snapshot.project_collection.get_project(&ID(project.id()))?;
        let project = project.borrow();
        // if program doesnt contain this file any more ignore it
        if !project.has_file(&uri.file_name()) {
            return None;
        }
        Some(ls::new_language_service(
            project.id().as_auto_import_project_id(),
            project
                .program
                .clone()
                .unwrap_or_else(|| crate::core::go_nil_dereference()),
            snapshot,
            &uri.file_name(),
        ))
    }

    // Go: project/session.go:1202 WithSnapshotLoadingProjectTree
    // WithSnapshotLoadingProjectTree acquires a ref'd snapshot with the
    // requested project trees loaded, then calls fn. The snapshot stays alive
    // for the duration of fn.
    // PORT: Go `*collections.Set[tspath.Path]` is `Option<&FxHashSet<..>>`
    // (nil loads all project trees).
    pub fn with_snapshot_loading_project_tree(
        self: &Rc<Self>,
        ctx: &Context,
        requested_project_trees: Option<&FxHashSet<tspath::Path>>,
        fn_: &mut dyn FnMut(&Rc<Snapshot>),
    ) {
        let snapshot = self.get_snapshot(
            ctx,
            ResourceRequest {
                project_tree: Some(ProjectTreeRequest {
                    referenced_projects: requested_project_trees.cloned(),
                }),
                ..Default::default()
            },
            true, /*callerRef*/
        );
        fn_(&snapshot);
        // Go: defer snapshot.Deref()
        snapshot.deref();
    }

    // Go: project/session.go:1216 WithSnapshotForDocument
    pub fn with_snapshot_for_document(
        self: &Rc<Self>,
        ctx: &Context,
        uri: &lsproto::DocumentUri,
        fn_: &mut dyn FnMut(&Rc<Snapshot>),
    ) {
        let snapshot = self.get_snapshot(
            ctx,
            ResourceRequest {
                documents: vec![uri.clone()],
                ..Default::default()
            },
            true, /*callerRef*/
        );
        fn_(&snapshot);
        // Go: defer snapshot.Deref()
        snapshot.deref();
    }

    // Go: project/session.go:1235 GetCurrentLanguageServiceWithAutoImports
    // GetCurrentLanguageServiceWithAutoImports flushes pending file changes, clones the
    // current snapshot with auto-import preparation for the given URI, then returns a
    // LanguageService for the default project. Use this only outside of request handling
    // (e.g. cache warming). For request handlers, use GetLanguageServiceWithAutoImports
    // with the request-level snapshot instead.
    pub fn get_current_language_service_with_auto_imports(
        self: &Rc<Self>,
        ctx: &Context,
        uri: &lsproto::DocumentUri,
    ) -> Result<ls::LanguageService, GoError> {
        let snapshot = self.get_snapshot(
            ctx,
            ResourceRequest {
                documents: vec![uri.clone()],
                auto_imports: uri.clone(),
                ..Default::default()
            },
            false, /*callerRef*/
        );
        let Some(project) = snapshot.get_default_project(uri) else {
            return Err(gostd::errors::errorf(
                format!("no project found for URI {}", uri),
                vec![],
            ));
        };
        let project = project.borrow();
        Ok(ls::new_language_service(
            project.id().as_auto_import_project_id(),
            project
                .program
                .clone()
                .unwrap_or_else(|| crate::core::go_nil_dereference()),
            snapshot,
            &uri.file_name(),
        ))
    }

    // Go: project/session.go:1257 WithLanguageServiceAndSnapshot
    // WithLanguageServiceAndSnapshot synchronously acquires a ref'd snapshot and
    // creates a language service for the given URI. fn receives both the language
    // service and the backing snapshot so it can clone the snapshot (e.g. to
    // enable auto-imports). The snapshot is kept alive until the async work
    // completes.
    //
    // Only use this method when the callback needs direct access to the snapshot.
    // For handlers that only need a LanguageService, use GetLanguageService
    // directly—language services continue to work even after their backing
    // snapshot has been disposed.
    // PORT: Go `(func() error, error)` is `Result<Option<..>, GoError>`
    // (`Ok(None)` is a nil function with a nil error). `fn` takes the
    // language service by value so the async work can own it.
    pub fn with_language_service_and_snapshot(
        self: &Rc<Self>,
        ctx: &Context,
        uri: &lsproto::DocumentUri,
        fn_: impl FnOnce(
            ls::LanguageService,
            Rc<Snapshot>,
        ) -> Result<Option<Box<dyn FnOnce() -> Result<(), GoError>>>, GoError>,
    ) -> Result<Option<Box<dyn FnOnce() -> Result<(), GoError>>>, GoError> {
        let (snapshot, _, language_service) =
            self.get_snapshot_and_default_project(ctx, uri, true /*callerRef*/)?;
        let async_work = fn_(language_service, snapshot.clone());
        let async_work = match async_work {
            Ok(Some(async_work)) => async_work,
            Ok(None) => {
                snapshot.deref();
                return Ok(None);
            }
            Err(err) => {
                snapshot.deref();
                return Err(err);
            }
        };
        Ok(Some(Box::new(move || {
            let result = async_work();
            // Go: defer snapshot.Deref()
            snapshot.deref();
            result
        })))
    }

    // Go: project/session.go:1281 GetLanguageServiceWithAutoImports
    // GetLanguageServiceWithAutoImports clones the given snapshot with auto-import
    // preparation for the given URI, without flushing pending file changes.
    // The cloned snapshot will be adopted as the session's current snapshot in the background
    // if other changes haven't been adopted in the meantime.
    pub fn get_language_service_with_auto_imports(
        self: &Rc<Self>,
        ctx: &Context,
        base_snapshot: &Rc<Snapshot>,
        uri: &lsproto::DocumentUri,
    ) -> Result<ls::LanguageService, GoError> {
        let new_snapshot =
            self.clone_snapshot_with_auto_imports(ctx, base_snapshot, uri, Some(&self.logger));
        let Some(project) = new_snapshot.get_default_project(uri) else {
            // Clone's initial ref (1) is released since we won't use this snapshot.
            new_snapshot.deref();
            return Err(gostd::errors::errorf(
                format!("no project found for URI {}", uri),
                vec![],
            ));
        };

        self.try_adopt_snapshot_change_in_background(base_snapshot, &new_snapshot);

        let project = project.borrow();
        Ok(ls::new_language_service(
            project.id().as_auto_import_project_id(),
            project
                .program
                .clone()
                .unwrap_or_else(|| crate::core::go_nil_dereference()),
            new_snapshot,
            &uri.file_name(),
        ))
    }

    // Go: project/session.go:1295 tryAdoptSnapshotChangeInBackground
    // PORT: renamed from `adoptSnapshotChangeInBackground` by ts#64163.
    pub fn try_adopt_snapshot_change_in_background(
        self: &Rc<Self>,
        base_snapshot: &Rc<Snapshot>,
        new_snapshot: &Rc<Snapshot>,
    ) {
        // The clone's initial ref (1) is transferred to adoptSnapshotChange,
        // which will either promote it as the session's current snapshot or
        // release it if the session has moved on.
        let s = self.clone();
        let task_base_snapshot = base_snapshot.clone();
        let task_new_snapshot = new_snapshot.clone();
        self.background_queue
            .enqueue(&self.background_context(), move |_ctx| {
                s.adopt_snapshot_change(&task_base_snapshot, &task_new_snapshot);
            });
    }

    // Go: project/session.go:1308 adoptSnapshotChange
    // adoptSnapshotChange promotes a cloned snapshot as the session's current
    // snapshot so future requests benefit from the work already done. If the
    // session has moved on, the snapshot is discarded; the next request needing
    // auto-imports will redo the work on the latest snapshot.
    pub fn adopt_snapshot_change(
        self: &Rc<Self>,
        base_snapshot: &Rc<Snapshot>,
        new_snapshot: &Rc<Snapshot>,
    ) {
        let old_snapshot = self.snapshot.borrow().clone();
        if Rc::ptr_eq(&old_snapshot, base_snapshot) {
            // Session hasn't moved on; adopt the new snapshot. The clone's initial
            // ref is transferred to become the session's ref for its current snapshot.
            *self.snapshot.borrow_mut() = new_snapshot.clone();
            old_snapshot.deref();
            let content_mapper_timings = self.take_content_mapper_timing_delta();
            if self.options.logging_enabled {
                self.logger.logf(&format!(
                    "Adopted snapshot {} (parent {}) as current session snapshot (replacing {})",
                    new_snapshot.id, new_snapshot.parent_id, old_snapshot.id
                ));
                if new_snapshot.builder_logs.is_some() {
                    self.logger.log(&new_snapshot.builder_logs.string());
                }
                self.log_content_mapper_timings(&content_mapper_timings);
            }
        } else {
            // Session has moved on to a newer snapshot; discard this one.
            // Release the clone's initial ref. If a handler is still using
            // the snapshot, its own ref keeps it alive.
            if self.options.logging_enabled {
                self.logger.logf(&format!(
                    "Discarded snapshot {} (parent {}); session has moved on to snapshot {}",
                    new_snapshot.id, new_snapshot.parent_id, old_snapshot.id
                ));
                if new_snapshot.builder_logs.is_some() {
                    let logs = new_snapshot.builder_logs.string();
                    if !logs.is_empty() {
                        self.logger.logf(&format!(
                            "--- Discarded snapshot {} builder logs (NOT adopted) ---",
                            new_snapshot.id
                        ));
                        self.logger.log(&logs);
                        self.logger.logf(&format!(
                            "--- End discarded snapshot {} builder logs ---",
                            new_snapshot.id
                        ));
                    }
                }
            }
            new_snapshot.deref();
        }
    }

    // Go: project/session.go:1344 UpdateSnapshot
    // PORT: Go has `UpdateSnapshot` and `updateSnapshot`; the exported one
    // ends in `_exported` (PORTING "Names").
    pub fn update_snapshot_exported(
        self: &Rc<Self>,
        ctx: &Context,
        overlays: IndexMap<tspath::Path, Rc<Overlay>>,
        change: SnapshotChange,
    ) {
        self.update_snapshot(ctx, overlays, change, false);
    }

    // Go: project/session.go:1352 updateSnapshotRef
    // updateSnapshotRef is like UpdateSnapshot but returns the created snapshot
    // with an extra reference for the caller. The ref is taken atomically with
    // the snapshot assignment under snapshotMu, so the snapshot is guaranteed
    // to be alive when returned. The caller must call snapshot.Deref() when done.
    pub fn update_snapshot_ref(
        self: &Rc<Self>,
        ctx: &Context,
        overlays: IndexMap<tspath::Path, Rc<Overlay>>,
        change: SnapshotChange,
    ) -> Rc<Snapshot> {
        // PORT: with `callerRef` Go always returns the snapshot (ts#64204).
        self.update_snapshot(ctx, overlays, change, true)
            .expect("updateSnapshot with callerRef returns the snapshot")
    }

    // Go: project/session.go:1356 updateSnapshot
    // PORT: Go passes `change` by value to `Clone` and keeps its own copy
    // for the background task, so the port clones it. A Go nil result (an
    // API error without `callerRef`, ts#64204) is `None`.
    pub fn update_snapshot(
        self: &Rc<Self>,
        ctx: &Context,
        overlays: IndexMap<tspath::Path, Rc<Overlay>>,
        change: SnapshotChange,
        caller_ref: bool,
    ) -> Option<Rc<Snapshot>> {
        let old_snapshot = self.snapshot.borrow().clone();
        // ts#64163
        let locale_ctx;
        let mut ctx = ctx;
        if !locale::has_locale(ctx) {
            locale_ctx = self.with_current_locale(ctx);
            ctx = &locale_ctx;
        }
        let new_snapshot = old_snapshot.clone_(
            ctx,
            change.clone(),
            &overlays,
            Some(&self.logger),
            self.client.clone(),
        );
        // A failed API request may have mutated only a prefix of its clone. Such a
        // snapshot is returned to the caller for inspection and cleanup, but must
        // never become canonical session state or trigger adoption side effects.
        // ts#64204
        if new_snapshot.api_error.is_some() {
            if caller_ref {
                return Some(new_snapshot);
            }
            new_snapshot.deref();
            return None;
        }
        *self.snapshot.borrow_mut() = new_snapshot.clone();
        if caller_ref {
            new_snapshot.ref_();
        }
        let mut content_mapper_timings = contentmapper::Timings::default();
        if !Rc::ptr_eq(&new_snapshot, &old_snapshot) {
            // Release the session's reference to the old snapshot. The new snapshot's
            // clone ref (1) is transferred to become the session's ref for its current
            // snapshot. Other holders (e.g. active handlers) keep the old snapshot alive
            // via their own refs until they complete.
            old_snapshot.deref();
            content_mapper_timings = self.take_content_mapper_timing_delta();
        }

        // Enqueue ATA updates if needed
        if self.typings_installer.borrow().is_some() && !self.config().is_ata_disabled() {
            self.trigger_ata_for_updated_projects(&new_snapshot);
        }

        // Enqueue logging, watch updates, and diagnostic refresh tasks
        // !!! userPreferences/configuration updates
        let s = self.clone();
        let task_old_snapshot = old_snapshot;
        let task_new_snapshot = new_snapshot.clone();
        // PORT: with push diagnostics, the task reads the programs of the new
        // snapshot (`publish_program_diagnostics`). Go's pointers keep them
        // readable when the next snapshot change disposes it before the task
        // runs; here a hold keeps each one registered until the task ends.
        // No other part of the task reads a program through the registry.
        let program_holds: Vec<crate::program::ls_program::ProgramHold> =
            if self.options.push_diagnostics_enabled {
                new_snapshot
                    .project_collection
                    .projects()
                    .iter()
                    .filter_map(|project| {
                        project
                            .borrow()
                            .program
                            .as_deref()
                            .and_then(crate::program::ls_program::hold_program)
                    })
                    .collect()
            } else {
                Vec::new()
            };
        self.background_queue
            .enqueue(&self.background_context(), move |ctx| {
                // PORT: for the hold of the warm (see `run_pending_warm`).
                let task_start = Instant::now();
                let _program_holds = program_holds;
                let old_snapshot = &task_old_snapshot;
                let new_snapshot = &task_new_snapshot;
                if s.options.logging_enabled {
                    s.logger.logf(&format!(
                        "Adopted snapshot {} (parent {}) as current session snapshot (replacing {})",
                        new_snapshot.id, new_snapshot.parent_id, old_snapshot.id
                    ));
                    if new_snapshot.builder_logs.is_some() {
                        s.logger.log(&new_snapshot.builder_logs.string());
                    }
                    s.log_project_changes(old_snapshot, new_snapshot);
                    s.log_content_mapper_timings(&content_mapper_timings);
                    s.logger.log("");
                }
                if s.options.watch_enabled {
                    if let Err(err) = s.update_watches(old_snapshot, new_snapshot) {
                        if s.options.logging_enabled {
                            s.logger.log(&err.error());
                        }
                    }
                }
                let _ = s.update_content_mapper_registrations(ctx, new_snapshot);
                s.publish_program_diagnostics(old_snapshot, new_snapshot);
                s.send_project_info_telemetry_for_new_projects(old_snapshot, new_snapshot);
                s.warm_auto_import_cache(ctx, &change, old_snapshot, new_snapshot, task_start);
            });

        Some(new_snapshot)
    }

    // Go: project/session.go:1420 takeContentMapperTimingDelta (tsgo#4712)
    pub fn take_content_mapper_timing_delta(&self) -> contentmapper::Timings {
        let Some(content_mapper_host) = &self.content_mapper_host else {
            return contentmapper::Timings::default();
        };
        let current = content_mapper_host.timings();
        let delta = current.since(&self.content_mapper_timings.borrow());
        *self.content_mapper_timings.borrow_mut() = current;
        delta
    }

    // Go: project/session.go:1432 logContentMapperTimings (tsgo#4712)
    // PORT: Go `%v` of a `time.Duration` is `{:?}` (log only), as in the
    // other session logs. Go sorts the map keys.
    pub fn log_content_mapper_timings(&self, timings: &contentmapper::Timings) {
        if timings.request_wait.is_zero() && !has_content_mapper_operation_timings(&timings.mappers)
        {
            return;
        }
        self.logger
            .log("Content mapper timings since previous snapshot adoption:");
        if !timings.request_wait.is_zero() {
            self.logger
                .logf(&format!("  Request wait time: {:?}", timings.request_wait));
        }
        let mut identities: Vec<&String> = timings.mappers.keys().collect();
        identities.sort();
        for identity in identities {
            let mapper = &timings.mappers[identity];
            if !has_content_mapper_operation_timing(mapper) {
                continue;
            }
            self.logger.logf(&format!("  {identity}:"));
            if mapper.spawn.count != 0 {
                self.logger.logf(&format!(
                    "    Initializations: {} ({:?})",
                    mapper.spawn.count,
                    mapper.spawn.duration + mapper.initialize.duration
                ));
            }
            if mapper.open_project.count != 0 {
                self.logger.logf(&format!(
                    "    openProject requests: {} ({:?})",
                    mapper.open_project.count, mapper.open_project.duration
                ));
            }
            if mapper.close_project.count != 0 {
                self.logger.logf(&format!(
                    "    closeProject requests: {} ({:?})",
                    mapper.close_project.count, mapper.close_project.duration
                ));
            }
            if mapper.transform.count != 0 {
                self.logger.logf(&format!(
                    "    Transforms: {} ({:?})",
                    mapper.transform.count, mapper.transform.duration
                ));
            }
        }
    }

    // Go: project/session.go:1476 WaitForBackgroundTasks
    // WaitForBackgroundTasks waits for all background tasks to complete.
    // This is intended to be used only for testing purposes.
    // PORT: `Queue::wait` runs `gostd::local::run_pending` until the
    // queue's tasks have finished, including the debounced ones, which
    // count until their timer has run. The auto-import warm clone is idle work
    // (see `warm_auto_import_cache`); Go waits for it as part of its task,
    // so this runs the idle work too, retries included. While a message
    // waits for the dispatch thread (`WarmAutoImportPreempt::set_busy`), a
    // warm attempt does not start and queues itself again, and the message
    // cannot run while this waits, so the wait stops there instead of
    // running that attempt forever.
    pub fn wait_for_background_tasks(&self) {
        self.cancel_idle_cache_clean();
        self.background_queue.wait();
        while !self.warm_auto_import_preempt.busy() && gostd::local::run_idle() {
            self.background_queue.wait();
        }
    }
}

// Go: project/session.go:1461 hasContentMapperOperationTimings (ts#64015)
// PORT: Go map order is random; the result does not depend on it.
pub fn has_content_mapper_operation_timings(
    timings: &IndexMap<String, contentmapper::MapperTimings>,
) -> bool {
    for timing in timings.values() {
        if has_content_mapper_operation_timing(timing) {
            return true;
        }
    }
    false
}

// Go: project/session.go:1470 hasContentMapperOperationTiming (ts#64015)
pub fn has_content_mapper_operation_timing(timing: &contentmapper::MapperTimings) -> bool {
    timing.spawn.count != 0
        || timing.open_project.count != 0
        || timing.close_project.count != 0
        || timing.transform.count != 0
}

// Go: project/session.go:1481 updateWatch
// PORT: Go `*WatchedFiles[T]` arguments are `Option<&WatchedFiles<T>>`.
// Go `logger != nil` compares the interface, which always holds the
// session logger (a nil `*logger` for the nop logger), so it is always
// true; the nop logger is `None` here and its calls do nothing.
pub fn update_watch<T>(
    ctx: &Context,
    session: &Session,
    logger: &Option<Rc<dyn logging::Logger>>,
    old_watcher: Option<&WatchedFiles<T>>,
    new_watcher: Option<&WatchedFiles<T>>,
) -> Vec<GoError> {
    let mut errors: Vec<GoError> = Vec::new();
    if let Some(new_watcher) = new_watcher {
        let w = new_watcher.watchers();
        let mut watchers = w.workspace_watchers.clone();
        watchers.extend(w.outside_workspace_watchers.iter().cloned());
        if !watchers.is_empty() {
            let mut new_watchers: IndexMap<WatcherID, lsproto::FileSystemWatcher> =
                IndexMap::default();
            for (i, watcher) in watchers.iter().enumerate() {
                let glob_id = WatcherID(format!("{}.{}", w.watcher_id, i));
                if session.watches.acquire(watcher, glob_id.clone()) {
                    new_watchers.insert(glob_id, watcher.clone());
                }
            }
            let mut watch_errors: Vec<GoError> = Vec::new();
            for (id, watcher) in &new_watchers {
                // Create a fresh timeout per client call so earlier calls
                // don't consume the deadline for later ones.
                let (call_ctx, call_cancel) =
                    gostd::context::with_timeout(ctx, WATCH_REQUEST_TIMEOUT);
                let err = session
                    .client
                    .as_ref()
                    .unwrap_or_else(|| crate::core::go_nil_dereference())
                    .watch_files(&call_ctx, id.clone(), std::slice::from_ref(watcher));
                call_cancel();
                match err {
                    Err(err) => watch_errors.push(err),
                    Ok(()) => {
                        if old_watcher.is_none() {
                            logger.log(&format!("Added new watch: {}", id));
                        } else {
                            logger.log(&format!("Updated watch: {}", id));
                        }
                        logger.log(&format!("\t{}", file_system_watcher_glob_string(watcher)));
                        logger.log("");
                    }
                }
            }
            if !watch_errors.is_empty() {
                // Roll back ALL newly-acquired watchers on any failure to keep
                // refcounts clean. On retry, Acquire will see them as new again.
                // Re-registering an already-registered watcher with the client
                // is harmless (registerCapability with the same ID replaces it).
                for watcher in new_watchers.values() {
                    session.watches.release(watcher);
                }
                session.watches.mark_pending(w.watcher_id.clone());
                errors.extend(watch_errors);
            } else {
                session.watches.clear_pending(&w.watcher_id);
            }
            if !w.ignored_paths.is_empty() {
                logger.logf(&format!(
                    "{} paths ineligible for watching",
                    w.ignored_paths.len()
                ));
                if logger.is_verbose() {
                    // PORT: Go map order is random (log text only).
                    for path in &w.ignored_paths {
                        logger.log(&format!("\t{}", path));
                    }
                }
            }
        }
    }
    if let Some(old_watcher) = old_watcher {
        let w = old_watcher.watchers();
        let mut watchers = w.workspace_watchers.clone();
        watchers.extend(w.outside_workspace_watchers.iter().cloned());
        if !watchers.is_empty() {
            let mut removed_ids: Vec<WatcherID> = Vec::new();
            for watcher in &watchers {
                let (id, removed) = session.watches.release(watcher);
                if removed {
                    removed_ids.push(id);
                }
            }
            for id in removed_ids {
                let (call_ctx, call_cancel) =
                    gostd::context::with_timeout(ctx, WATCH_REQUEST_TIMEOUT);
                let err = session
                    .client
                    .as_ref()
                    .unwrap_or_else(|| crate::core::go_nil_dereference())
                    .unwatch_files(&call_ctx, id.clone());
                call_cancel();
                match err {
                    Err(err) => errors.push(err),
                    Ok(()) => {
                        if new_watcher.is_none() {
                            logger.log(&format!("Removed watch: {}", id));
                        }
                    }
                }
            }
        }
    }
    errors
}

impl Session {
    // Go: project/session.go:1565 updateContentMapperRegistrations (tsgo#4712)
    // updateContentMapperRegistrations computes the union of content mapper extensions across all loaded
    // configs in the new snapshot and, when the set changes, asks the client to synchronize text documents
    // with those extensions. This is how an otherwise unsupported file (e.g. a `.vue`) begins flowing to the server once a
    // config that maps it is discovered.
    // PORT: `contentMapperRegistrationMu` is dropped (one thread).
    pub fn update_content_mapper_registrations(
        &self,
        ctx: &Context,
        snapshot: &Snapshot,
    ) -> Result<(), GoError> {
        let Some(client) = &self.client else {
            return Ok(());
        };
        let content_mappers = snapshot.config_file_registry.content_mappers();
        let mut extensions = content_mappers.extensions.clone();
        extensions.extend(
            snapshot
                .inferred_project_content_mapper_extensions
                .iter()
                .cloned(),
        );
        extensions.sort();
        extensions.dedup();

        // Background tasks may finish out of order; never let an older snapshot's task overwrite the
        // registration derived from a newer one.
        if snapshot.id() <= self.registered_content_mapper_snapshot_id.get() {
            return Ok(());
        }
        if extensions == *self.registered_content_mapper_extensions.borrow() {
            self.registered_content_mapper_snapshot_id
                .set(snapshot.id());
            return Ok(());
        }
        // RegisterContentMapperExtensions replaces the prior registration wholesale (unregistering extensions
        // that are no longer mapped and registering the current set), so an empty set removes the registration
        // once the last mapping config unloads. On failure we leave the state unadvanced so the next snapshot
        // update retries.
        if let Err(err) = client.register_content_mapper_extensions(ctx, &extensions) {
            if self.options.logging_enabled {
                self.logger.log(&err.error());
            }
            return Err(err);
        }
        *self.registered_content_mapper_extensions.borrow_mut() = extensions;
        self.registered_content_mapper_snapshot_id
            .set(snapshot.id());
        Ok(())
    }

    // Go: project/session.go:1600 updateWatches
    // PORT: the Go closures all append to `errors`, so it is a `RefCell`.
    // Go ranges over the config map (random order); the port uses the
    // `FxHashMap` order, which only changes the order of watch requests.
    pub fn update_watches(
        &self,
        old_snapshot: &Rc<Snapshot>,
        new_snapshot: &Rc<Snapshot>,
    ) -> Result<(), GoError> {
        let errors: RefCell<Vec<GoError>> = RefCell::new(Vec::new());
        let start = Instant::now();
        let ctx = self.background_context();
        crate::frontend::core_ls_ext::diff_maps_func(
            &old_snapshot.config_file_registry.configs,
            &new_snapshot.config_file_registry.configs,
            |a: &Rc<RefCell<ConfigFileEntry>>, b: &Rc<RefCell<ConfigFileEntry>>| {
                WatchedFiles::id(a.borrow().root_files_watch.as_deref())
                    == WatchedFiles::id(b.borrow().root_files_watch.as_deref())
            },
            Some(
                &mut |_: &tspath::Path, added_entry: &Rc<RefCell<ConfigFileEntry>>| {
                    let added = update_watch(
                        &ctx,
                        self,
                        &self.logger,
                        None,
                        added_entry.borrow().root_files_watch.as_deref(),
                    );
                    errors.borrow_mut().extend(added);
                },
            ),
            Some(
                &mut |_: &tspath::Path, removed_entry: &Rc<RefCell<ConfigFileEntry>>| {
                    let removed = update_watch(
                        &ctx,
                        self,
                        &self.logger,
                        removed_entry.borrow().root_files_watch.as_deref(),
                        None,
                    );
                    errors.borrow_mut().extend(removed);
                },
            ),
            Some(&mut |_: &tspath::Path,
                       old_entry: &Rc<RefCell<ConfigFileEntry>>,
                       new_entry: &Rc<RefCell<ConfigFileEntry>>| {
                let changed = update_watch(
                    &ctx,
                    self,
                    &self.logger,
                    old_entry.borrow().root_files_watch.as_deref(),
                    new_entry.borrow().root_files_watch.as_deref(),
                );
                errors.borrow_mut().extend(changed);
            }),
        );
        // Retry config watchers whose IDs didn't change but whose previous registration failed.
        for (path, new_entry) in &new_snapshot.config_file_registry.configs {
            if let Some(old_entry) = old_snapshot.config_file_registry.configs.get(path) {
                let new_id = WatchedFiles::id(new_entry.borrow().root_files_watch.as_deref());
                if WatchedFiles::id(old_entry.borrow().root_files_watch.as_deref()) == new_id
                    && self.watches.is_pending(&new_id)
                {
                    let retried = update_watch(
                        &ctx,
                        self,
                        &self.logger,
                        None,
                        new_entry.borrow().root_files_watch.as_deref(),
                    );
                    errors.borrow_mut().extend(retried);
                }
            }
        }

        crate::frontend::core_ls_ext::diff_ordered_maps(
            &old_snapshot.project_collection.projects_by_id(),
            &new_snapshot.project_collection.projects_by_id(),
            |_: &ID, added_project| {
                let added_project = added_project.borrow();
                let program_files = update_watch(
                    &ctx,
                    self,
                    &self.logger,
                    None,
                    added_project.program_files_watch.as_deref(),
                );
                errors.borrow_mut().extend(program_files);
                let typings = update_watch(
                    &ctx,
                    self,
                    &self.logger,
                    None,
                    added_project.typings_watch.as_deref(),
                );
                errors.borrow_mut().extend(typings);
                let content_mapper = update_watch(
                    &ctx,
                    self,
                    &self.logger,
                    None,
                    added_project.content_mapper_watch.as_deref(),
                );
                errors.borrow_mut().extend(content_mapper);
            },
            |_: &ID, removed_project| {
                let removed_project = removed_project.borrow();
                let program_files = update_watch(
                    &ctx,
                    self,
                    &self.logger,
                    removed_project.program_files_watch.as_deref(),
                    None,
                );
                errors.borrow_mut().extend(program_files);
                let typings = update_watch(
                    &ctx,
                    self,
                    &self.logger,
                    removed_project.typings_watch.as_deref(),
                    None,
                );
                errors.borrow_mut().extend(typings);
                let content_mapper = update_watch(
                    &ctx,
                    self,
                    &self.logger,
                    removed_project.content_mapper_watch.as_deref(),
                    None,
                );
                errors.borrow_mut().extend(content_mapper);
            },
            |_: &ID, old_project, new_project| {
                let old_project = old_project.borrow();
                let new_project = new_project.borrow();
                if WatchedFiles::id(old_project.program_files_watch.as_deref())
                    != WatchedFiles::id(new_project.program_files_watch.as_deref())
                {
                    let changed = update_watch(
                        &ctx,
                        self,
                        &self.logger,
                        old_project.program_files_watch.as_deref(),
                        new_project.program_files_watch.as_deref(),
                    );
                    errors.borrow_mut().extend(changed);
                } else if self.watches.is_pending(&WatchedFiles::id(
                    new_project.program_files_watch.as_deref(),
                )) {
                    let retried = update_watch(
                        &ctx,
                        self,
                        &self.logger,
                        None,
                        new_project.program_files_watch.as_deref(),
                    );
                    errors.borrow_mut().extend(retried);
                }
                if WatchedFiles::id(old_project.typings_watch.as_deref())
                    != WatchedFiles::id(new_project.typings_watch.as_deref())
                {
                    let changed = update_watch(
                        &ctx,
                        self,
                        &self.logger,
                        old_project.typings_watch.as_deref(),
                        new_project.typings_watch.as_deref(),
                    );
                    errors.borrow_mut().extend(changed);
                } else if self
                    .watches
                    .is_pending(&WatchedFiles::id(new_project.typings_watch.as_deref()))
                {
                    let retried = update_watch(
                        &ctx,
                        self,
                        &self.logger,
                        None,
                        new_project.typings_watch.as_deref(),
                    );
                    errors.borrow_mut().extend(retried);
                }
                if WatchedFiles::id(old_project.content_mapper_watch.as_deref())
                    != WatchedFiles::id(new_project.content_mapper_watch.as_deref())
                {
                    let changed = update_watch(
                        &ctx,
                        self,
                        &self.logger,
                        old_project.content_mapper_watch.as_deref(),
                        new_project.content_mapper_watch.as_deref(),
                    );
                    errors.borrow_mut().extend(changed);
                } else if self.watches.is_pending(&WatchedFiles::id(
                    new_project.content_mapper_watch.as_deref(),
                )) {
                    let retried = update_watch(
                        &ctx,
                        self,
                        &self.logger,
                        None,
                        new_project.content_mapper_watch.as_deref(),
                    );
                    errors.borrow_mut().extend(retried);
                }
            },
        );

        if WatchedFiles::id(old_snapshot.auto_imports_watch.as_deref())
            != WatchedFiles::id(new_snapshot.auto_imports_watch.as_deref())
        {
            let changed = update_watch(
                &ctx,
                self,
                &self.logger,
                old_snapshot.auto_imports_watch.as_deref(),
                new_snapshot.auto_imports_watch.as_deref(),
            );
            errors.borrow_mut().extend(changed);
        } else if self.watches.is_pending(&WatchedFiles::id(
            new_snapshot.auto_imports_watch.as_deref(),
        )) {
            let retried = update_watch(
                &ctx,
                self,
                &self.logger,
                None,
                new_snapshot.auto_imports_watch.as_deref(),
            );
            errors.borrow_mut().extend(retried);
        }

        let errors = errors.into_inner();
        if !errors.is_empty() {
            // Go: fmt.Errorf("errors updating watches: %v", errors) (no %w).
            let texts: Vec<String> = errors.iter().map(|err| err.error()).collect();
            return Err(gostd::errors::errorf(
                format!("errors updating watches: [{}]", texts.join(" ")),
                vec![],
            ));
        } else if self.options.logging_enabled {
            // PORT: `%v` of a Go `time.Duration`; log text only.
            self.logger
                .log(&format!("Updated watches in {:?}", start.elapsed()));
        }
        Ok(())
    }

    // Go: project/session.go:1683 Close
    pub fn close(&self) {
        // Cancel any pending scheduled snapshot update
        self.cancel_scheduled_snapshot_update();
        // Cancel any pending diagnostics refresh
        self.cancel_diagnostics_refresh();
        // Cancel any pending auto-import cache warming
        self.cancel_warm_auto_import_cache();
        // Cancel any pending idle cache clean
        self.cancel_idle_cache_clean();
        // Cancel periodic performance telemetry
        self.stop_performance_telemetry();
        self.background_queue.close();
        self.snapshot_host.close();
    }

    // Go: project/session.go:1698 flushChanges
    // PORT: Go `*lsutil.UserPreferences` is `Option<lsutil::UserPreferences>`.
    pub fn flush_changes(
        &self,
        ctx: &Context,
    ) -> (
        FileChangeSummary,
        IndexMap<tspath::Path, Rc<Overlay>>,
        FxHashMap<ID, Rc<ATAStateChange>>,
        Option<lsutil::UserPreferences>,
    ) {
        let pending_ata_changes = std::mem::take(&mut *self.pending_ata_changes.borrow_mut());
        let (file_changes, overlays) = self.flush_changes_locked(ctx);
        let mut new_prefs: Option<lsutil::UserPreferences> = None;
        if self.pending_user_config_changes.get() {
            let p = self.workspace_user_preferences.borrow().clone();
            new_prefs = Some(p);
        }
        self.pending_user_config_changes.set(false);
        (file_changes, overlays, pending_ata_changes, new_prefs)
    }

    // Go: project/session.go:1718 flushChangesLocked
    // flushChangesLocked should only be called with s.pendingFileChangesMu held.
    pub fn flush_changes_locked(
        &self,
        _ctx: &Context,
    ) -> (FileChangeSummary, IndexMap<tspath::Path, Rc<Overlay>>) {
        if self.pending_file_changes.borrow().is_empty() {
            return (FileChangeSummary::default(), (*self.fs.overlays()).clone());
        }

        let start = Instant::now();
        let pending_file_changes = std::mem::take(&mut *self.pending_file_changes.borrow_mut());
        let (changes, overlays) = self.fs.process_changes(&pending_file_changes);
        if self.options.logging_enabled {
            // PORT: `%v` of a Go `time.Duration`; log text only.
            self.logger.log(&format!(
                "Processed {} file changes in {:?}",
                pending_file_changes.len(),
                start.elapsed()
            ));
        }
        // Go: s.pendingFileChanges = nil (taken above)
        (changes, overlays)
    }

    // Go: project/session.go:1733 logProjectChanges
    // logProjectChanges logs information about projects that have changed between snapshots
    pub fn log_project_changes(&self, old_snapshot: &Rc<Snapshot>, new_snapshot: &Rc<Snapshot>) {
        let logged_project_changes = Cell::new(false);
        let log_project = |project: &Rc<RefCell<Project>>| {
            let mut builder = String::new();
            project.borrow().print(
                self.logger.is_verbose(), /*writeFileNames*/
                self.logger.is_verbose(), /*writeFileExplanation*/
                &mut builder,
            );
            self.logger.log(&builder);
            logged_project_changes.set(true);
        };
        crate::frontend::core_ls_ext::diff_ordered_maps(
            &old_snapshot.project_collection.projects_by_id(),
            &new_snapshot.project_collection.projects_by_id(),
            |_: &ID, added_project| {
                // New project added
                log_project(added_project);
            },
            |_: &ID, removed_project| {
                // Project removed
                self.logger.logf(&format!(
                    "\nProject '{}' removed\n{}",
                    removed_project.borrow().id(),
                    HR
                ));
            },
            |_: &ID, _old_project, new_project| {
                // Project updated
                if new_project.borrow().program_update_kind == ProgramUpdateKind::NEW_FILES {
                    log_project(new_project);
                }
            },
        );

        if logged_project_changes.get() || self.logger.is_verbose() {
            self.log_cache_stats(new_snapshot);
        }
    }
}

impl Session {
    // Go: project/session.go:1765 logCacheStats
    pub fn log_cache_stats(&self, snapshot: &Rc<Snapshot>) {
        let mut parse_cache_size = 0;
        let mut extended_config_count = 0;
        if self.logger.is_verbose() {
            for _ in self.parse_cache.entries.borrow().iter() {
                parse_cache_size += 1;
            }
            for _ in self.extended_config_cache.entries.borrow().iter() {
                extended_config_count += 1;
            }
        }
        self.logger.log("\n======== Cache Statistics ========");
        self.logger.logf(&format!(
            "Open file count:   {:6}",
            snapshot.overlays().len()
        ));
        self.logger.logf(&format!(
            "Cached disk files: {:6}",
            snapshot.fs.cache_files.len()
        ));
        self.logger.logf(&format!(
            "Realpath aliases:  {:6}",
            snapshot.fs.node_modules_realpath_aliases.len()
        ));
        self.logger.logf(&format!(
            "Project count:     {:6}",
            snapshot.project_collection.projects().len()
        ));
        self.logger.logf(&format!(
            "Config count:      {:6}",
            snapshot.config_file_registry.configs.len()
        ));
        if self.logger.is_verbose() {
            self.logger.logf(&format!(
                "Parse cache size:           {:6}",
                parse_cache_size
            ));
            self.logger.logf(&format!(
                "Program count:              {:6}",
                self.program_counter.len()
            ));
            self.logger.logf(&format!(
                "Extended config cache size: {:6}",
                extended_config_count
            ));

            self.logger.log("Auto Imports:");
            let auto_import_stats = snapshot
                .auto_import_registry()
                .unwrap_or_else(|| crate::core::go_nil_dereference())
                .get_cache_stats();
            self.logger.logf(&format!(
                "\tUnique packages (by realpath): {}",
                auto_import_stats.unique_package_count
            ));
            if !auto_import_stats.project_buckets.is_empty() {
                self.logger.log("\tProject buckets:");
                for bucket in &auto_import_stats.project_buckets {
                    self.logger.logf(&format!(
                        "\t\t{}{}:",
                        bucket.name,
                        if bucket.state.dirty() { " (dirty)" } else { "" }
                    ));
                    self.logger
                        .logf(&format!("\t\t\tFiles: {}", bucket.file_count));
                    self.logger
                        .logf(&format!("\t\t\tExports: {}", bucket.export_count));
                }
            }
            if !auto_import_stats.node_modules_buckets.is_empty() {
                self.logger.log("\tnode_modules buckets:");
                for bucket in &auto_import_stats.node_modules_buckets {
                    self.logger.logf(&format!(
                        "\t\t{}{}:",
                        bucket.name,
                        if bucket.state.dirty() { " (dirty)" } else { "" }
                    ));
                    // PORT: Go map order is random (log text only).
                    if let Some(dirty_packages) = bucket.state.dirty_packages_exported() {
                        for package_name in dirty_packages {
                            self.logger
                                .logf(&format!("\t\t\tNeeds granular update: {}", package_name));
                        }
                    }
                    if let Some(dependency_names) = &bucket.dependency_names {
                        self.logger.logf(&format!(
                            "\t\t\tCollected packages: {}",
                            dependency_names.len()
                        ));
                    } else {
                        self.logger
                            .logf("\t\t\tCollected packages: all, due to no package.json!");
                    }
                    // Go: bucket.PackageNames.Len() (0 for a nil set)
                    self.logger.logf(&format!(
                        "\t\t\tTotal packages: {}",
                        bucket.package_names.as_ref().map_or(0, |names| names.len())
                    ));
                    self.logger
                        .logf(&format!("\t\t\tFiles: {}", bucket.file_count));
                    self.logger
                        .logf(&format!("\t\t\tExports: {}", bucket.export_count));
                    match bucket.state.recursive_search_packages_exported() {
                        None => {
                            self.logger.log("\t\t\tRecursive search: all");
                        }
                        Some(packages) if !packages.is_empty() => {
                            self.logger.logf(&format!(
                                "\t\t\tRecursive search: {} packages",
                                packages.len()
                            ));
                        }
                        Some(_) => {
                            self.logger.log("\t\t\tRecursive search: none");
                        }
                    }
                }
            }
        }
    }

    // Go: project/session.go:1831 refreshInlayHintsIfNeeded
    pub fn refresh_inlay_hints_if_needed(
        &self,
        old_prefs: &lsutil::UserPreferences,
        new_prefs: &lsutil::UserPreferences,
    ) {
        if old_prefs.inlay_hints != new_prefs.inlay_hints {
            if let Err(err) = self
                .client
                .as_ref()
                .unwrap_or_else(|| crate::core::go_nil_dereference())
                .refresh_inlay_hints(&self.background_context())
            {
                if self.options.logging_enabled {
                    self.logger
                        .logf(&format!("Error refreshing inlay hints: {}", err.error()));
                }
            }
        }
    }

    // Go: project/session.go:1839 refreshCodeLensIfNeeded
    pub fn refresh_code_lens_if_needed(
        &self,
        old_prefs: &lsutil::UserPreferences,
        new_prefs: &lsutil::UserPreferences,
    ) {
        if old_prefs.code_lens != new_prefs.code_lens {
            if let Err(err) = self
                .client
                .as_ref()
                .unwrap_or_else(|| crate::core::go_nil_dereference())
                .refresh_code_lens(&self.background_context())
            {
                if self.options.logging_enabled {
                    self.logger
                        .logf(&format!("Error refreshing code lens: {}", err.error()));
                }
            }
        }
    }

    // Go: project/session.go:1847 refreshDiagnosticsIfNeeded
    pub fn refresh_diagnostics_if_needed(
        self: &Rc<Self>,
        old_prefs: &lsutil::UserPreferences,
        new_prefs: &lsutil::UserPreferences,
    ) {
        if old_prefs.custom_config_file_name != new_prefs.custom_config_file_name
            || old_prefs.report_style_checks_as_warnings
                != new_prefs.report_style_checks_as_warnings
            || old_prefs.enable_validation != new_prefs.enable_validation
        {
            self.schedule_diagnostics_refresh_exported();
        }
    }

    // Go: project/session.go:1855 refreshATAIfNeeded
    pub fn refresh_ata_if_needed(
        self: &Rc<Self>,
        old_prefs: &lsutil::UserPreferences,
        new_prefs: &lsutil::UserPreferences,
    ) {
        if old_prefs.is_ata_disabled() && !new_prefs.is_ata_disabled() {
            // ATA was re-enabled; schedule a diagnostics refresh so the next snapshot update
            // re-triggers ATA for existing projects with the new setting.
            self.schedule_diagnostics_refresh_exported();
        }
    }

    // Go: project/session.go:1863 publishProgramDiagnostics
    pub fn publish_program_diagnostics(
        &self,
        old_snapshot: &Rc<Snapshot>,
        new_snapshot: &Rc<Snapshot>,
    ) {
        if !self.options.push_diagnostics_enabled {
            return;
        }
        if new_snapshot.user_preferences().enable_validation.is_false() {
            if old_snapshot.user_preferences().enable_validation.is_false() {
                return;
            }
            let old_open_projects = old_snapshot
                .project_collection
                .get_open_configured_projects();
            for old_project in old_snapshot.project_collection.projects_by_id().values() {
                let (configured_id, configured) = old_project.borrow().id().configured();
                if configured && old_open_projects.contains(&configured_id) {
                    let config_file_path = old_project.borrow().config_file_path();
                    self.publish_project_diagnostics(
                        &self.background_context(),
                        &config_file_path,
                        &[],
                        &old_snapshot.converters,
                    );
                }
            }
            return;
        }

        let ctx = self.background_context();
        let old_projects = old_snapshot.project_collection.projects_by_id();
        let new_projects = new_snapshot.project_collection.projects_by_id();
        let old_open_projects = old_snapshot
            .project_collection
            .get_open_configured_projects();
        let new_open_projects = new_snapshot
            .project_collection
            .get_open_configured_projects();
        crate::frontend::core_ls_ext::diff_ordered_maps(
            &old_projects,
            &new_projects,
            |_: &ID, added_project| {
                let (configured_id, configured) = added_project.borrow().id().configured();
                if !should_publish_program_diagnostics(&added_project.borrow(), new_snapshot.id())
                    || !configured
                    || !new_open_projects.contains(&configured_id)
                {
                    return;
                }
                let config_file_path = added_project.borrow().config_file_path();
                let diagnostics = added_project.borrow().get_project_diagnostics(&ctx);
                self.publish_project_diagnostics(
                    &ctx,
                    &config_file_path,
                    &diagnostics,
                    &new_snapshot.converters,
                );
            },
            |_: &ID, removed_project| {
                if removed_project.borrow().kind != Kind::CONFIGURED {
                    return;
                }
                let config_file_path = removed_project.borrow().config_file_path();
                self.publish_project_diagnostics(
                    &ctx,
                    &config_file_path,
                    &[],
                    &old_snapshot.converters,
                );
            },
            |_: &ID, _old_project, new_project| {
                let (configured_id, configured) = new_project.borrow().id().configured();
                if !should_publish_program_diagnostics(&new_project.borrow(), new_snapshot.id())
                    || !configured
                    || !new_open_projects.contains(&configured_id)
                {
                    return;
                }
                let config_file_path = new_project.borrow().config_file_path();
                let diagnostics = new_project.borrow().get_project_diagnostics(&ctx);
                self.publish_project_diagnostics(
                    &ctx,
                    &config_file_path,
                    &diagnostics,
                    &new_snapshot.converters,
                );
            },
        );
        // Sync diagnostics for projects whose open-file state changed without a program update.
        for (project_id, new_project) in &new_projects {
            if new_project.borrow().kind != Kind::CONFIGURED {
                continue;
            }
            if !old_projects.contains_key(project_id) {
                continue; // Handled by added project case above
            }
            let (configured_id, _) = new_project.borrow().id().configured();
            let config_file_path = new_project.borrow().config_file_path();
            let old_project = old_projects.get(project_id);
            let new_has_open_files = new_open_projects.contains(&configured_id);
            let old_has_open_files = old_open_projects.contains(&configured_id);
            if new_has_open_files
                && !old_has_open_files
                && (old_project.is_some_and(|old_project| Rc::ptr_eq(new_project, old_project))
                    || !should_publish_program_diagnostics(
                        &new_project.borrow(),
                        new_snapshot.id(),
                    ))
            {
                // Project reopened without a program update
                let diagnostics = new_project.borrow().get_project_diagnostics(&ctx);
                self.publish_project_diagnostics(
                    &ctx,
                    &config_file_path,
                    &diagnostics,
                    &new_snapshot.converters,
                );
            } else if !new_has_open_files && old_has_open_files {
                // Project closed
                self.publish_project_diagnostics(
                    &ctx,
                    &config_file_path,
                    &[],
                    &new_snapshot.converters,
                );
            }
        }
    }
}

// Go: project/session.go:1937 shouldPublishProgramDiagnostics
pub fn should_publish_program_diagnostics(p: &Project, snapshot_id: u64) -> bool {
    if p.kind != Kind::CONFIGURED || p.program.is_none() || p.program_last_update != snapshot_id {
        return false;
    }
    p.program_update_kind > ProgramUpdateKind::CLONED
}

impl Session {
    // Go: project/session.go:1944 publishProjectDiagnostics
    // PORT: Go `[]*ast.Diagnostic` is `&[Diagnostic]` (nil is empty).
    pub fn publish_project_diagnostics(
        &self,
        ctx: &Context,
        config_file_path: &str,
        diagnostics: &[Diagnostic],
        converters: &lsconv::Converters,
    ) {
        let diagnostics: &[Diagnostic] = if self.config().enable_validation.is_false() {
            &[]
        } else {
            diagnostics
        };
        let ctx = &self.with_current_locale(ctx);
        let mut lsp_diagnostics: Vec<lsproto::Diagnostic> = Vec::with_capacity(diagnostics.len());
        for diag in diagnostics {
            lsp_diagnostics.push(lsconv::diagnostic_to_lsp_push(ctx, converters, diag));
        }

        if let Err(err) = self
            .client
            .as_ref()
            .unwrap_or_else(|| crate::core::go_nil_dereference())
            .publish_diagnostics(
                ctx,
                lsproto::PublishDiagnosticsParams {
                    uri: lsconv::file_name_to_document_uri(config_file_path),
                    diagnostics: lsp_diagnostics,
                    ..Default::default()
                },
            )
        {
            if self.options.logging_enabled {
                self.logger
                    .logf(&format!("Error publishing diagnostics: {}", err.error()));
            }
        }
    }

    // Go: project/session.go:1965 EnqueuePublishGlobalDiagnostics
    // EnqueuePublishGlobalDiagnostics schedules a background check for new accumulated
    // global diagnostics from checker pools, re-publishing tsconfig diagnostics if changed.
    // Multiple calls are coalesced into a single background task.
    pub fn enqueue_publish_global_diagnostics(self: &Rc<Self>) {
        if !self.options.push_diagnostics_enabled || self.config().enable_validation.is_false() {
            return;
        }
        // Go: s.globalDiagPublishPending.CompareAndSwap(false, true)
        if !self.global_diag_publish_pending.get() {
            self.global_diag_publish_pending.set(true);
            let s = self.clone();
            self.background_queue
                .enqueue(&self.background_context(), move |ctx| {
                    s.publish_global_diagnostics(ctx);
                });
        }
    }

    // Go: project/session.go:1974 publishGlobalDiagnostics
    pub fn publish_global_diagnostics(self: &Rc<Self>, ctx: &Context) {
        let snapshot = self.snapshot.borrow().clone();
        snapshot.ref_();

        for project in snapshot.project_collection.projects() {
            let project = project.borrow();
            if project.kind != Kind::CONFIGURED || project.checker_pool.is_none() {
                continue;
            }
            if project
                .checker_pool
                .as_ref()
                .unwrap_or_else(|| crate::core::go_nil_dereference())
                .take_new_global_diagnostics()
            {
                let diagnostics = project.get_project_diagnostics(ctx);
                self.publish_project_diagnostics(
                    ctx,
                    &project.config_file_path,
                    &diagnostics,
                    &snapshot.converters,
                );
            }
        }

        // Go: defer snapshot.Deref(); defer s.globalDiagPublishPending.Store(false)
        snapshot.deref();
        self.global_diag_publish_pending.set(false);
    }

    // Go: project/session.go:1993 triggerATAForUpdatedProjects
    pub fn trigger_ata_for_updated_projects(self: &Rc<Self>, new_snapshot: &Rc<Snapshot>) {
        for project in new_snapshot.project_collection.projects() {
            if !project.borrow().should_trigger_ata(new_snapshot.id()) {
                continue;
            }
            let s = self.clone();
            self.background_queue
                .enqueue(&self.background_context(), move |_ctx| {
                    let mut log_tree: Option<Rc<logging::LogTree>> = None;
                    if s.options.logging_enabled {
                        log_tree = logging::new_log_tree(&format!(
                            "Triggering ATA for project {}",
                            project.borrow().id().string()
                        ));
                    }

                    let typings_info = Rc::new(project.borrow().compute_typings_info());
                    let (request, project_id, project_display_name) = {
                        let p = project.borrow();
                        let request = ata::TypingsInstallRequest {
                            project_id: Rc::new(p.id()),
                            typings_info: typings_info.clone(),
                            file_names: p
                                .program
                                .as_ref()
                                .unwrap_or_else(|| crate::core::go_nil_dereference())
                                .get_source_files()
                                .iter()
                                .map(|file| file.file_name().to_string())
                                .collect(),
                            project_root_path: p.current_directory.clone(),
                            // Go `CompilerOptions` is nil-safe: a nil
                            // command line gives nil.
                            compiler_options: p
                                .command_line
                                .as_ref()
                                .map(|c| c.compiler_options().clone()),
                            current_directory: s.options.current_directory.clone(),
                            get_script_kind: Rc::new(|file_name: &str| {
                                crate::frontend::core_ext::get_script_kind_from_file_name(file_name)
                            }),
                            fs: s.fs.clone(),
                            logger: log_tree.clone().map(|t| t as Rc<dyn logging::Logger>),
                        };
                        (
                            request,
                            p.id(),
                            p.display_name(&s.options.current_directory),
                        )
                    };

                    if let Some(client) = s.client.as_ref() {
                        client.progress_start(
                            diag::Installing_types_for_0,
                            args![project_display_name],
                        );
                    }
                    let typings_installer = s
                        .typings_installer
                        .borrow()
                        .clone()
                        .unwrap_or_else(|| crate::core::go_nil_dereference());
                    // PORT: the rest of the goroutine is a future that waits
                    // for npm off the dispatch thread (`ata::run_task`). The
                    // hold keeps this task running until the future ends.
                    let hold = s.background_queue.hold();
                    ata::run_task(Box::pin(async move {
                        let result = typings_installer.install_typings_exported(&request).await;
                        if let Some(client) = s.client.as_ref() {
                            client.progress_finish(
                                diag::Installing_types_for_0,
                                args![project_display_name],
                            );
                        }
                        match result {
                            Err(err) => {
                                if log_tree.is_some() {
                                    s.logger.log(&format!(
                                        "ATA installation failed for project {}: {}",
                                        project_id,
                                        err.error()
                                    ));
                                    s.logger.log(&log_tree.string());
                                }
                            }
                            Ok(result) => {
                                if result.typings_files != project.borrow().typings_files {
                                    s.pending_ata_changes.borrow_mut().insert(
                                        project_id,
                                        Rc::new(ATAStateChange {
                                            typings_info: Some(typings_info),
                                            typings_files: result.typings_files,
                                            typings_files_to_watch: result.files_to_watch,
                                            logs: log_tree,
                                        }),
                                    );
                                    s.schedule_diagnostics_refresh_exported();
                                }
                            }
                        }
                        drop(hold);
                    }));
                });
        }
    }

    // Go: project/session.go:2046 warmAutoImportCache
    // PORT: Go `defer cancel()` and `defer newSnapshot.Deref(s)` run on
    // every return; the port calls them on each path, in Go's defer order.
    //
    // PORT: the clone (the export extraction, which can take hundreds of
    // ms) runs as idle work (`gostd::local::go_idle`, `run_pending_warm`),
    // after the checks and the cancel setup that Go does first. Go runs the
    // whole warm on a goroutine, so a request never waits for it. The LSP
    // dispatch loop starts the clone only when no message waits, and the
    // reader thread makes a message wait for it only up to a short hold
    // (`run_pending_warm`). A file event or a newer warm cancels the context
    // before or during the clone (`WarmAutoImportPreempt`). A clone that has
    // started runs to its next cancel point, and its result is discarded,
    // as Go's is. Its registry build has more cancel points than Go's
    // (`autoimport::registry::DISCARD_ON_CANCEL_KEY`). A clone that has not
    // started is skipped: Go's would run and be discarded, and the only
    // trace it leaves is its snapshot id, which is taken here, where Go's
    // clone takes it. `task_start` is when the background task that calls
    // this started.
    pub fn warm_auto_import_cache(
        self: &Rc<Self>,
        ctx: &Context,
        change: &SnapshotChange,
        _old_snapshot: &Rc<Snapshot>,
        new_snapshot: &Rc<Snapshot>,
        task_start: Instant,
    ) {
        if change.file_changes.changed.len() == 1 {
            let mut changed_file = lsproto::DocumentUri::default();
            for uri in &change.file_changes.changed {
                changed_file = uri.clone();
            }
            if !new_snapshot.is_open_file(&changed_file.file_name()) {
                return;
            }
            let prefs = new_snapshot.user_preferences();
            if prefs.include_completions_for_module_exports.is_false() {
                return;
            }
            let Some(project) = new_snapshot.get_default_project(&changed_file) else {
                return;
            };
            if crate::ls::autoimport::Registry::is_prepared_for_importing_file(
                new_snapshot.auto_imports.as_deref(),
                &changed_file.file_name(),
                &project.borrow().id().as_auto_import_project_id(),
                &prefs,
            ) {
                return;
            }

            // Cancel any previous auto-import warming and create a new cancellable context.
            // Only publish the new cancel func if the derived context is still active,
            // and make the stored cancel func a no-op once that warming task is done.
            let previous_cancel = self.warm_auto_import_cancel.borrow().clone();
            if let Some(previous_cancel) = previous_cancel {
                previous_cancel();
            }
            let (warm_ctx, cancel) = gostd::context::with_cancel(ctx);
            if warm_ctx.err().is_none() {
                // PORT: the stored closure keeps the session logger (Go reads
                // `s.logger`, which never changes) instead of the session, so
                // it does not make a reference cycle through the session.
                let stored_ctx = warm_ctx.clone();
                let stored_cancel = cancel.clone();
                let logger = self.logger.clone();
                let file_name = changed_file.file_name();
                self.warm_auto_import_preempt.set(
                    warm_ctx.clone(),
                    cancel.clone(),
                    file_name.clone(),
                );
                *self.warm_auto_import_cancel.borrow_mut() = Some(Rc::new(move || {
                    if stored_ctx.err().is_some() {
                        return;
                    }
                    logger.logf(&format!(
                        "Cancelling auto-import warming for file {}",
                        file_name
                    ));
                    stored_cancel();
                }));
            }

            if warm_ctx.err().is_some() {
                cancel();
                return;
            }
            // Go: defer cancel()

            // Clone the snapshot with auto-imports using warmCtx so the expensive
            // extraction work is cancelled if a file change arrives.
            if !new_snapshot.try_ref() {
                cancel();
                return;
            }
            // Go: defer newSnapshot.Deref(s)

            // PORT: Go's clone would take its snapshot id now.
            let snapshot_id = self.snapshot_id.get() + 1;
            self.snapshot_id.set(snapshot_id);
            // PORT: Go's warm starts now, and the part of this task before
            // it ran for `prefix`. The hold counts from `queued_at + prefix`
            // (see `run_pending_warm`).
            let queued_at = Instant::now();
            let prefix = queued_at.saturating_duration_since(task_start);
            let warm = PendingWarm {
                ctx: warm_ctx,
                cancel,
                changed_file,
                new_snapshot: new_snapshot.clone(),
                snapshot_id,
                hold_from: (!self.warm_auto_import_slow.get()).then(|| queued_at + prefix),
                retry: false,
            };
            // A pending warm here was cancelled above (`previous_cancel`).
            if let Some(previous) = self.warm_auto_import_pending.replace(Some(warm)) {
                self.end_pending_warm(previous);
            }
            self.queue_pending_warm();
        }
    }

    /// PORT: queues the idle job that runs the pending warm, unless one is
    /// queued. An eager warm starts as soon as no message waits, others after
    /// a quiet period.
    fn queue_pending_warm(self: &Rc<Self>) {
        let start = match self.warm_auto_import_pending.borrow().as_ref() {
            Some(warm) if warm.hold_from.is_some() => gostd::local::IdleStart::AtOnce,
            _ => gostd::local::IdleStart::AfterQuiet,
        };
        if !self.warm_auto_import_queued.replace(true) {
            let s = self.clone();
            // Go: the rest of the same wg.Go goroutine.
            gostd::local::go_idle(
                start,
                Box::new(move || crate::core::go_wait_group_task(|| s.run_pending_warm())),
            );
        }
    }

    /// PORT: the part of Go `warmAutoImportCache` after `tryRef`: one clone
    /// attempt, run as idle work for the pending warm (see
    /// `warm_auto_import_cache`).
    ///
    /// Go's warm starts on a goroutine before the async part of the request
    /// that made the snapshot, and survives if it ends before the next file
    /// event: p + T_w < D + gap, where p is the part of the background task
    /// before the warm (session.go:1397-1413), T_w the warm, D the async
    /// part and gap the client's turnaround. The port runs p before the async
    /// part, so its answer comes p later, and the clone runs only after the
    /// answer. So the first attempt is eager: it starts as soon as no message
    /// waits, and a message that comes during it waits up to its hold
    /// h = D - p = (start - queued_at) - p, at most
    /// `WARM_AUTO_IMPORT_HOLD_CAP` (`WarmAutoImportPreempt::on_message`).
    /// Then a file event cancels the warm, and any other message yields the
    /// attempt: the warm goes back to the pending slot and tries again after
    /// a quiet period, when such a message waits for the whole clone, so the
    /// warm cannot starve.
    ///
    /// An eager attempt that ends without its clone marks the session slow
    /// (`warm_auto_import_slow`), and so does a clone that takes longer than
    /// the hold can be. A clone that ends sooner clears the mark. While it is
    /// set, a new warm also waits for a quiet period, so on a project whose
    /// warm is long, a message pays the hold only before the first warm ends.
    pub fn run_pending_warm(self: &Rc<Self>) {
        self.warm_auto_import_queued.set(false);
        let Some(warm) = self.warm_auto_import_pending.take() else {
            return;
        };
        if warm.ctx.err().is_some() {
            self.end_pending_warm(warm);
            return;
        }
        // Go's adopt (session.go:1311) discards the clone when the session
        // has moved past its snapshot.
        if warm.retry && !Rc::ptr_eq(&*self.snapshot.borrow(), &warm.new_snapshot) {
            self.end_pending_warm(warm);
            return;
        }
        let (attempt_ctx, attempt_cancel) = gostd::context::with_cancel(&warm.ctx);
        let hold = warm.hold_from.map(|from| {
            Instant::now()
                .saturating_duration_since(from)
                .min(WARM_AUTO_IMPORT_HOLD_CAP)
        });
        if !self
            .warm_auto_import_preempt
            .start_attempt(attempt_cancel.clone(), hold)
        {
            // A message came after the dispatch loop saw none. It goes first.
            attempt_cancel();
            self.put_back_warm(warm);
            return;
        }

        let warm_change = SnapshotChange {
            reason: UpdateReason::REQUESTED_LANGUAGE_SERVICE_WITH_AUTO_IMPORTS,
            resource_request: ResourceRequest {
                documents: vec![warm.changed_file.clone()],
                auto_imports: warm.changed_file.clone(),
                ..Default::default()
            },
            ..Default::default()
        };
        // PORT: a cancelled warm drops its clone below (Go session.go:2111),
        // so its registry build may stop at more points than Go's
        // (`autoimport::registry::DISCARD_ON_CANCEL_KEY`). `build_ctx` is a
        // value child of `attempt_ctx`, a child of `warm.ctx`, so it is
        // cancelled exactly when one of them is.
        let build_ctx = gostd::context::with_value(
            &attempt_ctx,
            &crate::ls::autoimport::registry::DISCARD_ON_CANCEL_KEY,
            (),
        );
        // PORT: the clone takes the id kept for it when the warm started.
        let next_snapshot_id = self.snapshot_id.replace(warm.snapshot_id - 1);
        let clone_start = Instant::now();
        let cloned_snapshot = warm.new_snapshot.clone_(
            &build_ctx,
            warm_change,
            &warm.new_snapshot.overlays(),
            Some(&self.logger),
            self.client.clone(),
        );
        self.snapshot_id.set(next_snapshot_id);
        let end = self
            .warm_auto_import_preempt
            .end_attempt(&warm.ctx, &attempt_ctx);
        attempt_cancel();
        if end == WarmAttemptEnd::Done {
            self.warm_auto_import_slow
                .set(clone_start.elapsed() > WARM_AUTO_IMPORT_HOLD_CAP);
        } else if hold.is_some() {
            self.warm_auto_import_slow.set(true);
        }

        match end {
            WarmAttemptEnd::Yielded => {
                cloned_snapshot.deref();
                self.put_back_warm(PendingWarm {
                    hold_from: None,
                    retry: true,
                    ..warm
                });
            }
            // If cancelled during clone, discard the incomplete result.
            WarmAttemptEnd::Cancelled => {
                cloned_snapshot.deref();
                warm.new_snapshot.deref();
                (warm.cancel)();
            }
            WarmAttemptEnd::Done => {
                // Conditionally adopt: if the session hasn't moved past newSnapshot,
                // promote the clone so future requests benefit from the warmed cache.
                self.adopt_snapshot_change(&warm.new_snapshot, &cloned_snapshot);
                warm.new_snapshot.deref();
                (warm.cancel)();
            }
        }
    }

    /// PORT: puts a warm whose attempt did not run or yielded back in the
    /// pending slot (with its snapshot reference and its snapshot id), and
    /// queues its next attempt.
    fn put_back_warm(self: &Rc<Self>, warm: PendingWarm) {
        // The slot is empty: only `warm_auto_import_cache` fills it, and it
        // does not run during an attempt.
        if let Some(previous) = self.warm_auto_import_pending.replace(Some(warm)) {
            self.end_pending_warm(previous);
        }
        self.queue_pending_warm();
    }

    /// PORT: Go's deferred `newSnapshot.Deref(s)` and `cancel()` for a
    /// pending warm that ends without its clone.
    fn end_pending_warm(&self, warm: PendingWarm) {
        warm.new_snapshot.deref();
        (warm.cancel)();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOP: Option<Rc<dyn logging::Logger>> = None;

    /// A preempt with a live warm, and the warm's context.
    fn preempt_with_warm() -> (WarmAutoImportPreempt, Context) {
        let preempt = WarmAutoImportPreempt::default();
        let (ctx, cancel) = gostd::context::with_cancel(&gostd::context::background());
        preempt.set(ctx.clone(), cancel, "/a.ts".to_string());
        (preempt, ctx)
    }

    /// Runs `on_message` on another thread, and returns once the message is
    /// queued. The thread gives its wait, from the queue call, and whether it
    /// queued the message. The wait does not count the thread's start, so a
    /// slow start under load does not shorten it.
    fn message(
        preempt: &WarmAutoImportPreempt,
        file_event: bool,
    ) -> std::thread::JoinHandle<(Duration, bool)> {
        let preempt = preempt.clone();
        let (queued_tx, queued_rx) = std::sync::mpsc::channel();
        let reader = std::thread::spawn(move || {
            let mut start = None;
            let queued = preempt.on_message(file_event, &NOP, || {
                start = Some(Instant::now());
                queued_tx.send(()).expect("test thread");
                true
            });
            (start.expect("message queued").elapsed(), queued)
        });
        queued_rx.recv().expect("reader thread");
        reader
    }

    // A message that comes during an eager attempt waits for the clone, at
    // most for the hold. A clone that ends in time is neither cancelled nor
    // yielded. The clone ends 20 ms after the message is queued, so the
    // message waits at least that long, whatever the load.
    #[test]
    fn a_message_waits_for_an_eager_attempt_until_the_clone_ends() {
        let (preempt, warm_ctx) = preempt_with_warm();
        for file_event in [false, true] {
            let (attempt_ctx, attempt_cancel) = gostd::context::with_cancel(&warm_ctx);
            assert!(preempt.start_attempt(attempt_cancel, Some(Duration::from_secs(60))));
            let reader = message(&preempt, file_event);
            std::thread::sleep(Duration::from_millis(20));
            assert_eq!(
                preempt.end_attempt(&warm_ctx, &attempt_ctx),
                WarmAttemptEnd::Done
            );
            let (waited, queued) = reader.join().expect("reader thread");
            assert!(queued);
            assert!(waited >= Duration::from_millis(20), "{waited:?}");
            assert!(waited < Duration::from_secs(60), "{waited:?}");
            assert!(attempt_ctx.err().is_none() && warm_ctx.err().is_none());
        }
    }

    // After the hold, a message other than a file event yields the eager
    // attempt, and a file event cancels the warm.
    #[test]
    fn after_the_hold_a_message_yields_the_attempt_and_a_file_event_cancels_the_warm() {
        let hold = Duration::from_millis(20);
        let (preempt, warm_ctx) = preempt_with_warm();
        let (attempt_ctx, attempt_cancel) = gostd::context::with_cancel(&warm_ctx);
        assert!(preempt.start_attempt(attempt_cancel, Some(hold)));
        let (waited, queued) = message(&preempt, false).join().expect("reader thread");
        assert!(queued && waited >= hold, "{waited:?}");
        assert!(attempt_ctx.err().is_some() && warm_ctx.err().is_none());
        assert_eq!(
            preempt.end_attempt(&warm_ctx, &attempt_ctx),
            WarmAttemptEnd::Yielded
        );

        let (attempt_ctx, attempt_cancel) = gostd::context::with_cancel(&warm_ctx);
        assert!(preempt.start_attempt(attempt_cancel, Some(hold)));
        let (waited, queued) = message(&preempt, true).join().expect("reader thread");
        assert!(queued && waited >= hold, "{waited:?}");
        assert!(warm_ctx.err().is_some());
        assert_eq!(
            preempt.end_attempt(&warm_ctx, &attempt_ctx),
            WarmAttemptEnd::Cancelled
        );
    }

    // During an attempt that is not eager, a message does not wait: a file
    // event cancels the warm at once, and other messages leave it running.
    #[test]
    fn a_message_does_not_wait_for_an_attempt_that_is_not_eager() {
        let (preempt, warm_ctx) = preempt_with_warm();
        let (attempt_ctx, attempt_cancel) = gostd::context::with_cancel(&warm_ctx);
        assert!(preempt.start_attempt(attempt_cancel, None));
        let (_, queued) = message(&preempt, false).join().expect("reader thread");
        assert!(queued && attempt_ctx.err().is_none() && warm_ctx.err().is_none());
        let (_, queued) = message(&preempt, true).join().expect("reader thread");
        assert!(queued && warm_ctx.err().is_some());
        assert_eq!(
            preempt.end_attempt(&warm_ctx, &attempt_ctx),
            WarmAttemptEnd::Cancelled
        );
    }

    // An attempt does not start while a message waits for the dispatch
    // thread.
    #[test]
    fn an_attempt_does_not_start_while_a_message_waits() {
        let (preempt, warm_ctx) = preempt_with_warm();
        let busy = Arc::new(std::sync::atomic::AtomicBool::new(true));
        {
            let busy = busy.clone();
            preempt.set_busy(Box::new(move || {
                busy.load(std::sync::atomic::Ordering::SeqCst)
            }));
        }
        let (_, attempt_cancel) = gostd::context::with_cancel(&warm_ctx);
        assert!(!preempt.start_attempt(attempt_cancel.clone(), Some(Duration::ZERO)));
        busy.store(false, std::sync::atomic::Ordering::SeqCst);
        assert!(preempt.start_attempt(attempt_cancel, Some(Duration::ZERO)));
    }
}
