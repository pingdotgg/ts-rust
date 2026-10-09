//! Go `internal/project/snapshot.go`.
//!
//! PORT: one thread (project/dirty/interfaces.rs). Go `*Snapshot` is
//! `Rc<Snapshot>`; the Go `atomic.Int32` ref count is a manual `Cell<i32>`
//! because `dispose` releases parse cache and extended config cache
//! entries. Go `*Project` is `Rc<RefCell<Project>>`. Go
//! `collections.Set` is `FxHashSet`. Log text uses `{:?}` for Go `%v`
//! (log only).

use crate::project::prelude::*;

use crate::contentmapper;
use crate::frontend::core_ext::ProjectReference;
use std::cell::{Cell, OnceCell};
use std::panic::AssertUnwindSafe;
use std::time::Instant;

// Go: project/snapshot.go:31 Snapshot
pub struct Snapshot {
    pub host: Rc<SnapshotHost>,
    pub id: u64,
    pub parent_id: u64,
    pub ref_count: Cell<i32>,

    pub converters: Rc<lsconv::Converters>,

    // Immutable state, cloned between snapshots
    pub fs: Rc<SnapshotFS>,
    pub project_collection: Rc<ProjectCollection>,
    // PORT: Go can hold nil here only inside Clone, between NewSnapshot and
    // the assignment right after it; the port passes the final registry to
    // NewSnapshot, so the field is never nil.
    pub config_file_registry: Rc<ConfigFileRegistry>,
    pub auto_imports: Option<Rc<autoimport::Registry>>,
    pub auto_imports_watch: Option<Rc<WatchedFiles<FxHashMap<tspath::Path, String>>>>,
    pub compiler_options_for_inferred_projects: Option<Rc<CompilerOptions>>,
    pub inferred_project_content_mappers: Vec<Rc<contentmapper::Mapper>>,
    pub inferred_project_content_mapper_extensions: Vec<String>,
    pub user_preferences: lsutil::UserPreferences,
    // tsgo#4712. PORT: Go `contentMapperWatchStateOnce` with the fields
    // `contentMapperExtensions` and `contentMapperWatchedFiles` is one
    // `OnceCell` of both.
    pub content_mapper_watch_state: OnceCell<(Vec<String>, Rc<FxHashSet<tspath::Path>>)>,

    pub builder_logs: Option<Rc<logging::LogTree>>,
    pub api_error: Option<GoError>,
    // fileSystemOverride indicates that this snapshot was built from a filesystem
    // supplied by an API update rather than the session host filesystem.
    // ts#64115
    pub file_system_override: bool,

    // ts#64204
    pub created_programs: Vec<Rc<RefCell<Project>>>,
}

impl Snapshot {
    // Go: project/snapshot.go:62 contentMapperWatchState (tsgo#4712)
    pub fn content_mapper_watch_state(&self) -> (Vec<String>, Rc<FxHashSet<tspath::Path>>) {
        self.content_mapper_watch_state
            .get_or_init(|| {
                let configured = self.config_file_registry.content_mappers();
                let mut content_mapper_extensions = configured.extensions.clone();
                content_mapper_extensions.extend(
                    self.inferred_project_content_mapper_extensions
                        .iter()
                        .cloned(),
                );
                content_mapper_extensions.sort();
                content_mapper_extensions.dedup();

                let mut content_mapper_watched_files: FxHashSet<tspath::Path> =
                    FxHashSet::default();
                for project in self.project_collection.projects() {
                    let project = project.borrow();
                    if let Some(watched_files) = &project.content_mapper_watched_files {
                        for path in watched_files.iter() {
                            content_mapper_watched_files.insert(path.clone());
                        }
                    }
                }
                (
                    content_mapper_extensions,
                    Rc::new(content_mapper_watched_files),
                )
            })
            .clone()
    }
}

// Go: project/snapshot.go:218 (*Snapshot).LSPLineMap, as a function of the
// snapshot's file system.
// PORT: Go gives NewConverters the method value `s.LSPLineMap`, which keeps
// the snapshot alive. `s.fs` never changes after NewSnapshot, so the port's
// closure holds `s.fs` instead (no `Rc` cycle, same results).
fn lsp_line_map_of(fs: &SnapshotFS, file_name: &str) -> Option<Rc<lsconv::LSPLineMap>> {
    if let Some(file) = fs.get_file(file_name) {
        return Some(file.lsp_line_map());
    }
    None
}

// Go: project/snapshot.go:84 SnapshotHost.newSnapshot (ts#64163)
// PORT: Go `NewSnapshot` became this host method; the snapshot starts with
// refCount 1 and keeps the host.
impl SnapshotHost {
    #[allow(clippy::too_many_arguments)]
    pub fn new_snapshot(
        self: &Rc<Self>,
        id: u64,
        fs: Rc<SnapshotFS>,
        config_file_registry: Rc<ConfigFileRegistry>,
        compiler_options_for_inferred_projects: Option<Rc<CompilerOptions>>,
        user_preferences: lsutil::UserPreferences,
        auto_imports: Option<Rc<autoimport::Registry>>,
        auto_imports_watch: Option<Rc<WatchedFiles<FxHashMap<tspath::Path, String>>>>,
    ) -> Rc<Snapshot> {
        new_snapshot_with_host(
            self,
            id,
            fs,
            config_file_registry,
            compiler_options_for_inferred_projects,
            user_preferences,
            auto_imports,
            auto_imports_watch,
        )
    }
}

// PORT: the body of Go `SnapshotHost.newSnapshot`.
#[allow(clippy::too_many_arguments)]
fn new_snapshot_with_host(
    host: &Rc<SnapshotHost>,
    id: u64,
    fs: Rc<SnapshotFS>,
    config_file_registry: Rc<ConfigFileRegistry>,
    compiler_options_for_inferred_projects: Option<Rc<CompilerOptions>>,
    user_preferences: lsutil::UserPreferences,
    auto_imports: Option<Rc<autoimport::Registry>>,
    auto_imports_watch: Option<Rc<WatchedFiles<FxHashMap<tspath::Path, String>>>>,
) -> Rc<Snapshot> {
    let line_map_fs = fs.clone();
    let converters = lsconv::new_converters(
        host.options.position_encoding.clone(),
        move |file_name: &str| lsp_line_map_of(&line_map_fs, file_name),
    );
    let project_collection = Rc::new(ProjectCollection {
        to_path: host.to_path.clone(),
        config_file_registry: None,
        file_default_projects: FxHashMap::default(),
        configured_projects: FxHashMap::default(),
        synthetic_projects: FxHashMap::default(),
        open_files: open_file_paths(&snapshot_overlays(&fs)),
        inferred_project: None,
        api_state: APIState::default(),
        open_configured_projects: std::cell::OnceCell::new(),
    });
    Rc::new(Snapshot {
        host: host.clone(),
        id,
        parent_id: 0,
        // Go: s.refCount.Store(1)
        ref_count: Cell::new(1),

        converters,

        fs,
        config_file_registry,
        project_collection,
        compiler_options_for_inferred_projects,
        inferred_project_content_mappers: Vec::new(),
        inferred_project_content_mapper_extensions: Vec::new(),
        user_preferences,
        content_mapper_watch_state: OnceCell::new(),
        auto_imports,
        auto_imports_watch,

        builder_logs: None,
        api_error: None,
        file_system_override: false,
        created_programs: Vec::new(),
    })
}

// Go: project/snapshot.go:193 overlayFileHandles (ts#64291)
// PORT: Go `map[tspath.Path]FileHandle` keeps the overlay map's order here.
pub fn overlay_file_handles(
    overlays: &IndexMap<tspath::Path, Rc<Overlay>>,
) -> IndexMap<tspath::Path, Rc<dyn FileHandle>> {
    let mut files: IndexMap<tspath::Path, Rc<dyn FileHandle>> =
        IndexMap::with_capacity(overlays.len());
    for (path, overlay) in overlays {
        files.insert(path.clone(), overlay.clone());
    }
    files
}

// Go: project/snapshot.go:111 snapshotOverlays (ts#64291)
pub fn snapshot_overlays(fs: &SnapshotFS) -> Rc<IndexMap<tspath::Path, Rc<Overlay>>> {
    fs.fs.overlays()
}

impl Snapshot {
    // Go: project/snapshot.go:115 overlays (ts#64291)
    pub fn overlays(&self) -> Rc<IndexMap<tspath::Path, Rc<Overlay>>> {
        snapshot_overlays(&self.fs)
    }

    // Go: project/snapshot.go:119 CreatedPrograms (ts#64204)
    pub fn created_programs(&self) -> Vec<Rc<RefCell<Project>>> {
        self.created_programs.clone()
    }

    // Go: project/snapshot.go:123 resourceRequestForDocument (ts#64204)
    pub fn resource_request_for_document(&self, uri: &lsproto::DocumentUri) -> ResourceRequest {
        let path = uri.path(self.use_case_sensitive_file_names());
        let mut request = ResourceRequest {
            documents: vec![uri.clone()],
            ..Default::default()
        };
        for project in self.project_collection.synthetic_projects() {
            let project = project.borrow();
            if project.contains_file(&path)
                || project
                    .host
                    .as_ref()
                    .is_some_and(|host| host.source_fs.seen_file_or_missing_parent_directory(&path))
            {
                request.projects.push(project.id());
            }
        }
        request
    }

    // Go: project/snapshot.go:134 processFileChanges (ts#63950, ts#64291)
    pub fn process_file_changes(
        &self,
        fs: &Rc<SnapshotFSBuilder>,
        file_changes: FileChangeSummary,
        logger: &Option<Rc<logging::LogTree>>,
        content_mapper_contributions: Option<&ContentMapperContributions>,
        previous_overlays: &IndexMap<tspath::Path, Rc<Overlay>>,
        overlays: &IndexMap<tspath::Path, Rc<Overlay>>,
    ) -> FileChangeSummary {
        let mut file_changes = file_changes;
        if let Some(expander) = as_file_change_expander(&*fs.fs) {
            file_changes = expander.expand_file_changes(file_changes);
        }
        let previous_open_files = overlay_file_handles(previous_overlays);
        let open_files = overlay_file_handles(overlays);
        if file_changes.has_excessive_watch_events() {
            let invalidate_start = Instant::now();
            if file_changes.invalidate_all {
                fs.invalidate_cache();
                if logger.is_some() {
                    logger.logf(&format!(
                        "InvalidateAll: invalidated file cache in {:?}",
                        invalidate_start.elapsed()
                    ));
                }
            } else if !fs.watch_changes_overlap_cache(
                &file_changes,
                &previous_open_files,
                &open_files,
            ) {
                // All watch changes/deletes are files we haven't seen; should be irrelevant to us (probably an external tool's build or something)
                file_changes.changed = FxHashSet::default();
                file_changes.deleted = FxHashSet::default();
            } else if file_changes.includes_watch_change_outside_node_modules {
                fs.invalidate_cache();
                if logger.is_some() {
                    logger.logf(&format!(
                        "Excessive watch changes detected, invalidated file cache in {:?}",
                        invalidate_start.elapsed()
                    ));
                }
            } else {
                fs.invalidate_node_modules_cache();
                if logger.is_some() {
                    logger.logf(&format!(
                        "npm install detected, invalidated node_modules cache in {:?}",
                        invalidate_start.elapsed()
                    ));
                }
            }
        } else {
            let content_mapper_extensions = match content_mapper_contributions {
                None => self.content_mapper_watch_state().0,
                Some(contributions) => {
                    let mut content_mapper_extensions = self
                        .config_file_registry
                        .content_mappers()
                        .extensions
                        .clone();
                    content_mapper_extensions.extend(contributions.extensions.iter().cloned());
                    content_mapper_extensions
                }
            };
            let (_, content_mapper_watched_files) = self.content_mapper_watch_state();
            file_changes = fs.expand_and_filter_watch_events(
                file_changes,
                &content_mapper_extensions,
                Some(&*content_mapper_watched_files),
                &previous_open_files,
                &open_files,
            );
            file_changes = self.fs.expand_realpath_aliases(file_changes);
            file_changes = fs.mark_dirty_files(file_changes);
            file_changes = fs.convert_open_and_close_to_changes(
                file_changes,
                &previous_open_files,
                &open_files,
            );
        }
        for path in open_files.keys() {
            if let (Some(entry), true) = fs.cache_files.load(path) {
                fs.delete_cache_entry(&entry);
            }
        }
        file_changes
    }

    // Go: project/snapshot.go:201 GetDefaultProject
    pub fn get_default_project(&self, uri: &lsproto::DocumentUri) -> Option<Rc<RefCell<Project>>> {
        self.project_collection
            .get_default_project(&uri.path(self.use_case_sensitive_file_names()))
    }

    // Go: project/snapshot.go:207 GetLanguageServiceProjectsContainingFile (ts#64204: was GetProjectsContainingFile)
    // GetLanguageServiceProjectsContainingFile does not consider synthetic projects
    // (ones created by API via createProgram).
    pub fn get_language_service_projects_containing_file(
        &self,
        uri: &lsproto::DocumentUri,
    ) -> Vec<Rc<dyn ls::Project>> {
        let file_name = uri.file_name();
        let path = (self.host.to_path)(&file_name);
        // TODO!! sheetal may be change this to handle symlinks!!
        self.project_collection
            .get_language_service_projects_containing_file(&path)
    }

    // Go: project/snapshot.go:214 GetFile
    pub fn get_file(&self, file_name: &str) -> Option<Rc<dyn FileHandle>> {
        self.fs.get_file(file_name)
    }

    // Go: project/snapshot.go:218 LSPLineMap
    pub fn lsp_line_map(&self, file_name: &str) -> Option<Rc<lsconv::LSPLineMap>> {
        if let Some(file) = self.fs.get_file(file_name) {
            return Some(file.lsp_line_map());
        }
        None
    }

    // Go: project/snapshot.go:225 GetECMALineInfo
    pub fn get_ecma_line_info(
        &self,
        file_name: &str,
    ) -> Option<Rc<sourcemap::lineinfo::ECMALineInfo>> {
        if let Some(file) = self.fs.get_file(file_name) {
            return Some(file.ecma_line_info());
        }
        None
    }

    // Go: project/snapshot.go:232 GetPreferences
    pub fn get_preferences(&self, _active_file: &str) -> lsutil::UserPreferences {
        self.user_preferences.clone()
    }

    // Go: project/snapshot.go:236 UserPreferences
    pub fn user_preferences(&self) -> lsutil::UserPreferences {
        self.user_preferences.clone()
    }

    // Go: project/snapshot.go:240 Converters
    pub fn converters(&self) -> Rc<lsconv::Converters> {
        self.converters.clone()
    }

    // Go: project/snapshot.go:244 AutoImportRegistry
    pub fn auto_import_registry(&self) -> Option<Rc<autoimport::Registry>> {
        self.auto_imports.clone()
    }

    // Go: project/snapshot.go:248 ID
    pub fn id(&self) -> u64 {
        self.id
    }

    // Go: project/snapshot.go:252 toPath (ts#64163)
    pub fn to_path(&self, file_name: &str) -> tspath::Path {
        (self.host.to_path)(file_name)
    }

    // Go: project/snapshot.go:256 isOpenFile (ts#64291)
    pub fn is_open_file(&self, file_name: &str) -> bool {
        self.overlays().contains_key(&self.to_path(file_name))
    }

    // Go: project/snapshot.go:261 hasOverlayWithin (ts#64291)
    pub fn has_overlay_within(&self, path: &tspath::Path) -> bool {
        for overlay_path in self.overlays().keys() {
            if path.contains_path(overlay_path) {
                return true;
            }
        }
        false
    }

    // Go: project/snapshot.go:270 UseCaseSensitiveFileNames
    pub fn use_case_sensitive_file_names(&self) -> bool {
        self.fs.fs.use_case_sensitive_file_names()
    }

    // Go: project/snapshot.go:275 FileSystem (ts#64115)
    // FileSystem returns the filesystem backing this snapshot.
    pub fn file_system(&self) -> Rc<dyn vfs::Fs> {
        self.fs.fs.clone()
    }

    // Go: project/snapshot.go:281 HasFileSystemOverride (ts#64115)
    // HasFileSystemOverride reports whether this snapshot uses an API-supplied
    // filesystem instead of the session host filesystem.
    pub fn has_file_system_override(&self) -> bool {
        self.file_system_override
    }

    // Go: project/snapshot.go:285 ReadFile
    pub fn read_file(&self, file_name: &str) -> (String, bool) {
        let (text, ok) = self.read_file_shared(file_name);
        (text.to_string(), ok)
    }

    /// Go `ReadFile` without the copy: the text the file holds (the
    /// `ls::Host` read).
    pub fn read_file_shared(&self, file_name: &str) -> (FileText, bool) {
        let Some(handle) = self.get_file(file_name) else {
            return (FileText::default(), false);
        };
        (FileText::Shared(handle.shared_content()), true)
    }

    // Go: project/snapshot.go:293 DirectoryExists
    pub fn directory_exists(&self, path: &str) -> bool {
        self.fs.fs.directory_exists(path)
    }

    // Go: project/snapshot.go:297 FileExists
    pub fn file_exists(&self, path: &str) -> bool {
        self.fs.fs.file_exists(path)
    }

    // Go: project/snapshot.go:301 GetDirectories
    pub fn get_directories(&self, path: &str) -> Vec<String> {
        self.fs.fs.get_accessible_entries(path).directories
    }

    // Go: project/snapshot.go:305 ReadDirectory
    pub fn read_directory(
        &self,
        current_dir: &str,
        path: &str,
        extensions: &[String],
        excludes: &[String],
        includes: &[String],
        depth: i32,
    ) -> Vec<String> {
        vfs::vfsmatch::read_directory(
            &*self.fs.fs,
            current_dir,
            path,
            extensions,
            excludes,
            includes,
            depth,
        )
    }

    // Go: project/snapshot.go:309 Snapshot.FS (ts#64299)
    // PORT: the field `fs` is Go's `s.fs`; this method is Go `FS()`.
    pub fn fs(&self) -> Rc<dyn vfs::Fs> {
        new_source_fs(false, self.fs.clone(), self.host.to_path.clone())
    }

    // Go: project/snapshot.go:313 Snapshot.GetCurrentDirectory (ts#64299)
    pub fn get_current_directory(&self) -> String {
        self.host.get_current_directory()
    }

    // Go: project/snapshot.go:317 Snapshot.ContentMapperExtensions (ts#64299)
    pub fn content_mapper_extensions(&self) -> Vec<String> {
        let (extensions, _) = self.content_mapper_watch_state();
        extensions
    }
}

// Go: project/snapshot.go:13 (import of ls; Snapshot is the ls.Host of a
// language service)
impl ls::Host for Snapshot {
    fn use_case_sensitive_file_names(&self) -> bool {
        Snapshot::use_case_sensitive_file_names(self)
    }

    fn read_file(&self, path: &str) -> (FileText, bool) {
        Snapshot::read_file_shared(self, path)
    }

    fn converters(&self) -> Rc<lsconv::Converters> {
        Snapshot::converters(self)
    }

    fn get_preferences(&self, active_file: &str) -> lsutil::UserPreferences {
        Snapshot::get_preferences(self, active_file)
    }

    fn get_ecma_line_info(&self, file_name: &str) -> Option<Rc<sourcemap::lineinfo::ECMALineInfo>> {
        Snapshot::get_ecma_line_info(self, file_name)
    }

    fn auto_import_registry(&self) -> Option<Rc<autoimport::Registry>> {
        Snapshot::auto_import_registry(self)
    }

    fn read_directory(
        &self,
        current_dir: &str,
        path: &str,
        extensions: &[String],
        excludes: &[String],
        includes: &[String],
        depth: i32,
    ) -> Vec<String> {
        Snapshot::read_directory(
            self,
            current_dir,
            path,
            extensions,
            excludes,
            includes,
            depth,
        )
    }

    fn get_directories(&self, path: &str) -> Vec<String> {
        Snapshot::get_directories(self, path)
    }

    fn directory_exists(&self, path: &str) -> bool {
        Snapshot::directory_exists(self, path)
    }

    fn file_exists(&self, path: &str) -> bool {
        Snapshot::file_exists(self, path)
    }
}

// Go: project/snapshot.go:322 APICreateProgramRequest (ts#64204)
// PORT: Go `*core.CompilerOptions` is `Rc<CompilerOptions>`: every Go
// caller (the API session) sets it. A Go nil `[]*core.ProjectReference` is
// an empty `Vec`.
#[derive(Clone, Default)]
pub struct APICreateProgramRequest {
    pub root_file_names: Vec<String>,
    pub compiler_options: Rc<CompilerOptions>,
    pub project_references: Vec<ProjectReference>,
    pub config_file_parsing_diagnostics: Vec<Diagnostic>,
    // ts#64299. PORT: a Go nil factory is `None`.
    pub module_resolver_factory: Option<Rc<dyn ModuleResolverFactory>>,
    pub module_resolver_id: u64,
}

// Go: project/snapshot.go:331 ModuleResolverFactory (ts#64299)
pub trait ModuleResolverFactory {
    // Go: NewResolver(options module.ResolverOptions) (module.Resolver, func())
    fn new_resolver(
        &self,
        options: crate::frontend::module::ResolverOptions,
    ) -> (Rc<dyn crate::frontend::module::Resolver>, Box<dyn FnOnce()>);
}

// Go: project/snapshot.go:335 APIReconfigureProgramRequest (ts#64204)
// PORT: Go embeds `APICreateProgramRequest`; here it is the field
// `api_create_program_request`.
#[derive(Clone, Default)]
pub struct APIReconfigureProgramRequest {
    // ts#64319
    pub program_id: SyntheticProjectID,
    pub api_create_program_request: APICreateProgramRequest,
}

// Go: project/snapshot.go:340 APISnapshotRequest
// PORT: Go `*collections.Set[T]` is `Option<FxHashSet<T>>` (nil is `None`).
// Go `map[tspath.Path]string` (`open_files`, `ensure_files`, ts#64391) is
// `Option<IndexMap>`, so API-opened files enter the API state in request
// order (Go map order is random).
// PORT: Go nil `vfs.FS` is `None`. `Debug` skips the file system.
#[derive(Clone, Default)]
pub struct APISnapshotRequest {
    // ts#64554 (snapshot.go:337 at fed0bf24149f). PORT: Go
    // `*lsutil.UserPreferences` nil is `None`.
    pub user_preferences: Option<lsutil::UserPreferences>,
    pub prepare_auto_imports: lsproto::DocumentUri,
    pub open_projects: Option<FxHashSet<String>>,
    pub close_projects: Option<FxHashSet<tspath::Path>>,
    pub open_files: Option<IndexMap<tspath::Path, String>>,
    pub close_files: Option<FxHashSet<tspath::Path>>,
    // ts#64204
    pub create_programs: Vec<APICreateProgramRequest>,
    pub reconfigure_programs: Vec<APIReconfigureProgramRequest>,
    pub remove_programs: Option<FxHashSet<SyntheticProjectID>>,
    pub ensure_programs: Option<FxHashSet<ID>>,
    pub ensure_all_programs: bool,
    pub ensure_files: Option<IndexMap<tspath::Path, String>>,
    // ts#64115
    pub file_system: Option<Rc<dyn vfs::Fs>>,
    // ReplaceFileSystem indicates a total filesystem replacement. Layers use
    // per-path file changes instead of invalidating all inherited state.
    pub replace_file_system: bool,
}

impl std::fmt::Debug for APISnapshotRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("APISnapshotRequest")
            .field("user_preferences", &self.user_preferences.is_some())
            .field("prepare_auto_imports", &self.prepare_auto_imports)
            .field("open_projects", &self.open_projects)
            .field("close_projects", &self.close_projects)
            .field("open_files", &self.open_files)
            .field("close_files", &self.close_files)
            .field("create_programs", &self.create_programs.len())
            .field("reconfigure_programs", &self.reconfigure_programs.len())
            .field("remove_programs", &self.remove_programs)
            .field("ensure_programs", &self.ensure_programs)
            .field("ensure_all_programs", &self.ensure_all_programs)
            .field("ensure_files", &self.ensure_files)
            .field("file_system", &self.file_system.is_some())
            .field("replace_file_system", &self.replace_file_system)
            .finish()
    }
}

// Go: project/snapshot.go:357 ProjectTreeRequest
#[derive(Clone, Debug, Default)]
pub struct ProjectTreeRequest {
    // If null, all project trees need to be loaded, otherwise only those that are referenced
    pub referenced_projects: Option<FxHashSet<tspath::Path>>,
}

impl ProjectTreeRequest {
    // Go: project/snapshot.go:362 IsAllProjects
    pub fn is_all_projects(&self) -> bool {
        self.referenced_projects.is_none()
    }

    // Go: project/snapshot.go:366 IsProjectReferenced
    // PORT: Go `Set.Has` returns false on a nil set.
    pub fn is_project_referenced(&self, project_id: &tspath::Path) -> bool {
        self.referenced_projects
            .as_ref()
            .is_some_and(|referenced_projects| referenced_projects.contains(project_id))
    }

    // Go: project/snapshot.go:370 Projects
    // PORT: a Go nil slice is empty. Go map order is random; FxHashSet order here.
    pub fn projects(&self) -> Vec<tspath::Path> {
        let Some(referenced_projects) = &self.referenced_projects else {
            return Vec::new();
        };
        referenced_projects.iter().cloned().collect()
    }
}

// Go: project/snapshot.go:377 ResourceRequest
// PORT: Go `*ProjectTreeRequest` is `Option<ProjectTreeRequest>` (nil is `None`).
#[derive(Clone, Debug, Default)]
pub struct ResourceRequest {
    // Documents are URIs that were requested by the client.
    // The new snapshot should ensure projects for these URIs have loaded programs.
    pub documents: Vec<lsproto::DocumentUri>,
    // ConfiguredProjectDocuments are URIs for which configured projects should be loaded
    // (if disableSolutionSearching/disableReferencedProjectLoad settings allow),
    // but no inferred project should be created if no configured project is found.
    // This is used by cross-project operations like find-all-references.
    pub configured_project_documents: Vec<lsproto::DocumentUri>,
    // Update requested Projects.
    // this is used when we want to get LS and from all the Projects the file can be part of
    // ts#64319: project IDs.
    pub projects: Vec<ID>,
    // Update and ensure project trees that reference the projects
    // This is used to compute the solution and project tree so that
    // we can find references across all the projects in the solution irrespective of which project is open
    pub project_tree: Option<ProjectTreeRequest>,
    // AutoImports is the document URI for which auto imports should be prepared.
    pub auto_imports: lsproto::DocumentUri,
}

// Go: project/snapshot.go:397 SnapshotChange
// PORT: Go embeds `ResourceRequest`; here it is the field
// `resource_request`. Go `*core.CompilerOptions` is
// `Option<Rc<CompilerOptions>>`, Go `*lsutil.UserPreferences` is
// `Option<lsutil::UserPreferences>`, Go `*APISnapshotRequest` is
// `Option<APISnapshotRequest>` (nil is `None`).
#[derive(Clone, Default)]
pub struct SnapshotChange {
    pub resource_request: ResourceRequest,
    pub reason: UpdateReason,
    // fs overrides the session filesystem for this snapshot. It is used by API
    // snapshots that supply their own memory or cache filesystem.
    // ts#64115. PORT: Go nil `vfs.FS` is `None`.
    pub fs: Option<Rc<dyn vfs::Fs>>,
    pub file_system_override: bool,
    pub replace_file_system: bool,
    // fileChanges are the changes that have occurred since the last snapshot.
    pub file_changes: FileChangeSummary,
    // compilerOptionsForInferredProjects is the compiler options to use for inferred projects.
    // It should only be set the value in the next snapshot should be changed. If nil, the
    // value from the previous snapshot will be copied to the new snapshot.
    pub compiler_options_for_inferred_projects: Option<Rc<CompilerOptions>>,
    // tsgo#4712. PORT: Go nil pointer is `None`.
    pub content_mapper_contributions: Option<ContentMapperContributions>,
    pub new_config: Option<lsutil::UserPreferences>,
    // ataChanges contains ATA-related changes to apply to projects in the new snapshot.
    // ts#64319: keyed by project ID.
    pub ata_changes: FxHashMap<ID, Rc<ATAStateChange>>,
    pub api_request: Option<APISnapshotRequest>,
    // cleanFileCache triggers cleaning of cached files not referenced by any open project.
    // ts#64291: was cleanDiskCache.
    pub clean_file_cache: bool,
}

// Go: project/snapshot.go:421 ATAStateChange
// ATAStateChange represents a change to a project's ATA state.
// PORT: Go `*ata.TypingsInfo` is `Option<Rc<ata::TypingsInfo>>`.
#[derive(Clone, Default)]
pub struct ATAStateChange {
    // TypingsInfo is the new typings info for the project.
    pub typings_info: Option<Rc<ata::TypingsInfo>>,
    // TypingsFiles is the new list of typing files for the project.
    pub typings_files: Vec<String>,
    // TypingsFilesToWatch is the new list of typing files to watch for changes.
    pub typings_files_to_watch: Vec<String>,
    pub logs: Option<Rc<logging::LogTree>>,
}

/// Go `%v` of a slice in log text: `[a b c]`.
fn fmt_list<T: std::fmt::Display>(items: &[T]) -> String {
    let parts: Vec<String> = items.iter().map(|item| item.to_string()).collect();
    format!("[{}]", parts.join(" "))
}

/// Go `%v` of a `[]lsproto.DocumentUri` in log text.
fn fmt_uris(uris: &[lsproto::DocumentUri]) -> String {
    let parts: Vec<&str> = uris.iter().map(|uri| uri.0.as_str()).collect();
    fmt_list(&parts)
}

impl Snapshot {
    // Go: project/snapshot.go:431 Clone
    // PORT: Go `Clone` is `clone_` (Rust `Clone::clone` copies a value).
    // The deferred `recover()` is `catch_unwind` around the body
    // (`clone_body`); the panic is logged and raised again, as in Go
    // (`go_repanic`: the runtime line ends with `[recovered, repanicked]`).
    pub fn clone_(
        &self,
        ctx: &Context,
        change: SnapshotChange,
        overlays: &IndexMap<tspath::Path, Rc<Overlay>>,
        session_logger: SessionLogger<'_>,
        client: Option<Rc<dyn Client>>,
    ) -> Rc<Snapshot> {
        // ts#64204
        if let Some(api_error) = &self.api_error {
            crate::core::go_panic(format!(
                "cannot clone snapshot with API error: {}",
                api_error.error()
            ));
        }
        let store = &self.host;
        let mut logger: Option<Rc<logging::LogTree>> = None;

        // Print in-progress logs immediately if cloning fails
        if store.options.logging_enabled
            && let Some(session_logger) = session_logger
        {
            let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
                self.clone_body(ctx, change, overlays, true, client.clone(), &mut logger)
            }));
            return match result {
                Ok(new_snapshot) => new_snapshot,
                Err(r) => {
                    session_logger.log(&logger.string());
                    crate::core::go_repanic(r)
                }
            };
        }

        self.clone_body(ctx, change, overlays, false, client, &mut logger)
    }

    // Go: project/snapshot.go:431 Clone (the body after the deferred recover)
    // PORT: split out of `clone_` so the recover can wrap it; `logger` is
    // the Go local that the deferred function reads. `make_logger` is Go
    // `store.options.LoggingEnabled && sessionLogger != nil`.
    fn clone_body(
        &self,
        ctx: &Context,
        change: SnapshotChange,
        overlays: &IndexMap<tspath::Path, Rc<Overlay>>,
        make_logger: bool,
        client: Option<Rc<dyn Client>>,
        logger_out: &mut Option<Rc<logging::LogTree>>,
    ) -> Rc<Snapshot> {
        let store = &self.host;
        let mut change = change;

        if make_logger {
            *logger_out = logging::new_log_tree(&format!("Cloning snapshot {}", self.id));
            let logger = logger_out.clone();
            let get_details = || -> String {
                let mut details = String::new();
                if !change.resource_request.documents.is_empty() {
                    details += &format!(
                        " Documents: {}",
                        fmt_uris(&change.resource_request.documents)
                    );
                }
                if !change
                    .resource_request
                    .configured_project_documents
                    .is_empty()
                {
                    details += &format!(
                        " ConfiguredProjectDocuments: {}",
                        fmt_uris(&change.resource_request.configured_project_documents)
                    );
                }
                if !change.resource_request.projects.is_empty() {
                    details +=
                        &format!(" Projects: {}", fmt_list(&change.resource_request.projects));
                }
                if let Some(project_tree) = &change.resource_request.project_tree {
                    details += &format!(" ProjectTree: {}", fmt_list(&project_tree.projects()));
                }
                details
            };
            // PORT: Go `switch change.reason`; an `if` chain needs only `PartialEq`.
            let reason = &change.reason;
            if *reason == UpdateReason::DID_OPEN_FILE {
                logger.logf(&format!(
                    "Reason: DidOpenFile - {}",
                    change.file_changes.opened.0
                ));
            } else if *reason == UpdateReason::DID_CLOSE_FILE {
                logger.logf(&format!(
                    "Reason: DidCloseFile - {:?}",
                    change.file_changes.closed
                ));
            } else if *reason == UpdateReason::DID_CHANGE_COMPILER_OPTIONS_FOR_INFERRED_PROJECTS {
                logger.logf("Reason: DidChangeCompilerOptionsForInferredProjects");
            } else if *reason == UpdateReason::REQUESTED_LANGUAGE_SERVICE_PENDING_CHANGES {
                logger.logf(&format!(
                    "Reason: RequestedLanguageService (pending file changes) - {}",
                    get_details()
                ));
            } else if *reason == UpdateReason::REQUESTED_LANGUAGE_SERVICE_PROJECT_NOT_LOADED {
                logger.logf(&format!(
                    "Reason: RequestedLanguageService (project not loaded) - {}",
                    get_details()
                ));
            } else if *reason == UpdateReason::REQUESTED_LANGUAGE_SERVICE_FOR_FILE_NOT_OPEN {
                logger.logf(&format!(
                    "Reason: RequestedLanguageService (file not open) - {}",
                    get_details()
                ));
            } else if *reason == UpdateReason::REQUESTED_LANGUAGE_SERVICE_PROJECT_DIRTY {
                logger.logf(&format!(
                    "Reason: RequestedLanguageService (project dirty) - {}",
                    get_details()
                ));
            } else if *reason == UpdateReason::REQUESTED_LOAD_PROJECT_TREE {
                logger.logf(&format!(
                    "Reason: RequestedLoadProjectTree - {}",
                    get_details()
                ));
            } else if *reason == UpdateReason::IDLE_CLEAN_DISK_CACHE {
                logger.logf("Reason: IdleCleanDiskCache");
            } else if *reason == UpdateReason::DID_CHANGE_CONFIG_FILE {
                logger.logf(&format!("Reason: DidChangeConfigFile - {}", get_details()));
            } else if *reason == UpdateReason::DID_CHANGE_CONTENT_MAPPER_CONTRIBUTIONS {
                logger.logf(&format!(
                    "Reason: DidChangeContentMapperContributions - {}",
                    get_details()
                ));
            }
        }
        let logger = logger_out.clone();

        let start = Instant::now();
        let mut inferred_content_mappers = self.inferred_project_content_mappers.clone();
        let mut inferred_content_mapper_extensions =
            self.inferred_project_content_mapper_extensions.clone();
        if let Some(contributions) = &change.content_mapper_contributions {
            inferred_content_mappers = contributions.mappers.clone();
            inferred_content_mapper_extensions = contributions.extensions.clone();
        }
        let mut base_fs = store.fs.clone();
        if let Some(change_fs) = &change.fs {
            base_fs = change_fs.clone();
        }
        // Total replacements and returning to the session host must not retain files
        // from the previous filesystem. Layers invalidate only their per-path changes,
        // including the first layer over a host-backed snapshot.
        if change.replace_file_system || self.file_system_override && !change.file_system_override {
            change.file_changes.invalidate_all = true;
        }
        // ts#64291
        let layered_fs = layer_overlay_file_system(
            base_fs,
            overlays.clone(),
            store.options.position_encoding.clone(),
            store.to_path.clone(),
        );
        let overlays = layered_fs.overlays();
        let fs = new_snapshot_fs_builder_from_source(
            layered_fs,
            self.fs.cache_files.clone(),
            self.fs.cache_directories.clone(),
            self.fs.node_modules_realpath_aliases.clone(),
            store.to_path.clone(),
        );
        change.file_changes = self.process_file_changes(
            &fs,
            std::mem::take(&mut change.file_changes),
            &logger,
            change.content_mapper_contributions.as_ref(),
            &self.overlays(),
            &overlays,
        );

        let mut compiler_options_for_inferred_projects =
            self.compiler_options_for_inferred_projects.clone();
        if change.compiler_options_for_inferred_projects.is_some() {
            compiler_options_for_inferred_projects =
                change.compiler_options_for_inferred_projects.clone();
        }

        // Compute effective customConfigFileName from user preferences
        let mut custom_config_file_name = self.config_file_registry.custom_config_file_name.clone();
        if let Some(new_config) = &change.new_config {
            custom_config_file_name = new_config.custom_config_file_name.clone();
        }

        let new_snapshot_id = store.next_snapshot_id();
        let project_collection_builder = new_project_collection_builder(
            ctx,
            new_snapshot_id,
            fs.clone(),
            overlays.clone(),
            self.project_collection.clone(),
            self.config_file_registry.clone(),
            &self.project_collection.api_state,
            compiler_options_for_inferred_projects.clone(),
            inferred_content_mappers.clone(),
            inferred_content_mapper_extensions.clone(),
            store.options.clone(),
            &custom_config_file_name,
            store.parse_cache.clone(),
            store.content_mapped_parse_cache.clone(),
            store.extended_config_cache.clone(),
            store.content_mapper_host.clone(),
            store.resolve_ahead_stash.clone(),
            client,
        );

        if !change.ata_changes.is_empty() {
            project_collection_builder
                .did_update_ata_state(&change.ata_changes, logger.fork("DidUpdateATAState"));
        }

        project_collection_builder
            .did_change_custom_config_file_name(logger.fork("DidChangeCustomConfigFileName"));
        // ts#63950
        if let Some(compiler_options) = &change.compiler_options_for_inferred_projects
            && let Some(inferred_project) = project_collection_builder.inferred_project.value()
        {
            let (file_names, project_references, errors, content_mappers) = {
                let inferred_project = inferred_project.borrow();
                let command_line = inferred_project
                    .command_line
                    .as_ref()
                    .unwrap_or_else(|| crate::core::go_nil_dereference());
                (
                    command_line.file_names().to_vec(),
                    command_line.parsed_config.project_references.clone(),
                    // PORT: Go `CommandLine.Errors`; the plain `errors` (see
                    // projectcollectionbuilder.rs
                    // `update_inferred_project_roots`).
                    command_line.errors.clone(),
                    command_line.content_mappers().to_vec(),
                )
            };
            project_collection_builder.update_inferred_project(
                file_names,
                Some(compiler_options.clone()),
                project_references,
                errors,
                content_mappers,
                logger.fork("DidChangeCompilerOptionsForInferredProjects"),
            );
        }
        if change.content_mapper_contributions.is_some() {
            project_collection_builder.did_change_content_mapper_contributions(
                logger.fork("DidChangeContentMapperContributions"),
            );
        }
        if let Some(new_config) = &change.new_config {
            project_collection_builder.did_change_user_preferences(
                &self.user_preferences,
                new_config,
                logger.fork("DidChangeUserPreferences"),
            );
        }

        if !change.file_changes.is_empty() {
            project_collection_builder
                .did_change_files(&change.file_changes, logger.fork("DidChangeFiles"));
        }

        let mut api_error: Option<GoError> = None;
        if let Some(api_request) = &change.api_request {
            api_error = project_collection_builder
                .handle_api_request(api_request, logger.fork("HandleAPIRequest"))
                .err();
        }

        for uri in &change.resource_request.documents {
            project_collection_builder.did_request_file_exported(
                uri,
                false, /*configuredProjectsOnly*/
                logger.fork("DidRequestFile"),
            );
        }

        for uri in &change.resource_request.configured_project_documents {
            project_collection_builder.did_request_file_exported(
                uri,
                true, /*configuredProjectsOnly*/
                logger.fork("DidRequestFile (optional)"),
            );
        }

        for project_id in &change.resource_request.projects {
            project_collection_builder
                .did_request_project(project_id, logger.fork("DidRequestProject"));
        }

        if let Some(project_tree) = &change.resource_request.project_tree {
            project_collection_builder
                .did_request_project_trees(project_tree, logger.fork("DidRequestProjectTrees"));
        }

        let (project_collection, config_file_registry) =
            project_collection_builder.finalize(logger.clone());

        // ts#64319: keyed by project ID.
        let mut projects_with_new_program_structure: FxHashMap<autoimport::ProjectID, bool> =
            FxHashMap::default();
        for project in project_collection.projects() {
            let project = project.borrow();
            if project.program_last_update == new_snapshot_id
                && project.program_update_kind != ProgramUpdateKind::CLONED
            {
                projects_with_new_program_structure.insert(
                    project.id().as_auto_import_project_id(),
                    project.program_update_kind == ProgramUpdateKind::NEW_FILES,
                );
            }
        }

        // Clean cached files not touched by any open project on file open, close, delete,
        // or when explicitly requested (e.g. by an idle timer).
        let should_clean_file_cache = change.clean_file_cache
            || !change.file_changes.opened.0.is_empty()
            || !change.file_changes.reopened.0.is_empty()
            || !change.file_changes.closed.is_empty()
            || !change.file_changes.deleted.is_empty();
        if should_clean_file_cache {
            // The set of seen files can change only if a program was constructed (not cloned) during this snapshot.
            // When cleanFileCache is explicitly set, always attempt cleaning.
            if !projects_with_new_program_structure.is_empty() || change.clean_file_cache {
                let clean_files_start = Instant::now();
                let mut removed_files = 0;
                fs.cache_files.range(&mut |entry| {
                    for project in project_collection.projects() {
                        let project = project.borrow();
                        if let Some(host) = &project.host {
                            if host.source_fs.seen_file(&entry.key()) {
                                return true;
                            }
                        }
                    }
                    entry.delete();
                    removed_files += 1;
                    true
                });
                if logger.is_some() {
                    logger.logf(&format!(
                        "Removed {} cached file(s) in {:?}",
                        removed_files,
                        clean_files_start.elapsed()
                    ));
                }
            }
        }

        let mut config = self.user_preferences.clone();
        if let Some(new_config) = &change.new_config {
            config = new_config.clone();
        }

        let auto_import_host = new_auto_import_registry_clone_host(
            project_collection.clone(),
            store.parse_cache.clone(),
            fs.clone(),
            &store.options.current_directory,
            store.to_path.clone(),
            store.auto_import_parse_keys.clone(),
        );
        let mut open_files: FxHashMap<tspath::Path, String> =
            FxHashMap::with_capacity_and_hasher(overlays.len(), Default::default());
        for (path, overlay) in overlays.iter() {
            open_files.insert(path.clone(), overlay.file_name());
        }
        let mut prepare_auto_imports = tspath::Path::default();
        if !change.resource_request.auto_imports.0.is_empty() {
            prepare_auto_imports = change
                .resource_request
                .auto_imports
                .path(self.use_case_sensitive_file_names());
        }
        let mut old_auto_imports = self.auto_imports.clone();
        if old_auto_imports.is_none() {
            old_auto_imports = Some(Rc::new(autoimport::new_registry(
                store.to_path.clone(),
                self.user_preferences.clone(),
            )));
        }
        let mut auto_imports_watch: Option<Rc<WatchedFiles<FxHashMap<tspath::Path, String>>>> =
            None;
        let clone_result = old_auto_imports
            .unwrap_or_else(|| crate::core::go_nil_dereference())
            .clone_(
                ctx,
                autoimport::RegistryChange {
                    requested_file: prepare_auto_imports,
                    open_files,
                    changed: change.file_changes.changed.clone(),
                    created: change.file_changes.created.clone(),
                    deleted: change.file_changes.deleted.clone(),
                    rebuilt_programs: projects_with_new_program_structure,
                    user_preferences: change.new_config.clone(),
                },
                auto_import_host.clone() as Rc<dyn autoimport::RegistryCloneHost>,
                logger.fork("UpdateAutoImports"),
            );
        // PORT: Go `autoImports, err := ...`; on an error Go `autoImports` is nil.
        let auto_imports: Option<Rc<autoimport::Registry>> = match clone_result {
            Ok(auto_imports) => {
                auto_imports_watch = WatchedFiles::clone_(
                    self.auto_imports_watch.as_deref(),
                    auto_imports.node_modules_directories(),
                );
                Some(auto_imports)
            }
            Err(_) => None,
        };

        let (snapshot_fs, _) = fs.finalize();
        // PORT: Go passes nil for the config file registry and assigns it
        // right after; the port passes the final registry here.
        let mut new_snapshot = store.new_snapshot(
            new_snapshot_id,
            snapshot_fs.clone(),
            config_file_registry.clone(),
            compiler_options_for_inferred_projects,
            config,
            auto_imports,
            auto_imports_watch,
        );
        {
            // PORT: Go writes the new snapshot's fields before anyone else
            // sees it; the `Rc` is not shared yet.
            let s = Rc::get_mut(&mut new_snapshot).expect("new snapshot is not shared yet");
            s.parent_id = self.id;
            s.project_collection = project_collection;
            s.config_file_registry = config_file_registry;
            s.inferred_project_content_mappers = inferred_content_mappers;
            s.inferred_project_content_mapper_extensions = inferred_content_mapper_extensions;
            s.builder_logs = logger.clone();
            s.api_error = api_error;
            s.file_system_override = change.file_system_override;
            s.created_programs = project_collection_builder.created_programs.borrow().clone();
        }

        for project in new_snapshot.project_collection.projects() {
            let project = project.borrow();
            // PORT: Go `project.Program` (the field).
            if let Some(program) = &project.program {
                store.program_counter.ref_(program);
                if project.program_last_update == new_snapshot_id {
                    // If the program was updated during this clone, the project and its host are new
                    // and still retain references to the builder. Freezing clears the builder reference
                    // so it's GC'd and to ensure the project can't access any data not already in the
                    // snapshot during use. This is pretty kludgy, but it's an artifact of Program design:
                    // Program has a single host, which is expected to implement a full vfs.FS, among
                    // other things. That host is *mostly* only used during program *construction*, but a
                    // few methods may get exercised during program *use*. So, our compiler host is allowed
                    // to access caches and perform mutating effects (like acquire referenced project
                    // config files) during snapshot building, and then we call `freeze` to ensure those
                    // mutations don't happen afterwards. In the future, we might improve things by
                    // separating what it takes to build a program from what it takes to use a program,
                    // and only pass the former into NewProgram instead of retaining it indefinitely.
                    project
                        .host
                        .as_ref()
                        .unwrap_or_else(|| crate::core::go_nil_dereference())
                        .freeze(
                            snapshot_fs.clone(),
                            new_snapshot.config_file_registry.clone(),
                        );
                }
            }
        }
        // PORT: not in Go. A program that this clone made for a project that
        // it then deleted or updated again is in no snapshot, so `dispose`
        // never frees it, and its host still holds the builder (the
        // `Project -> host -> builder` cycle). Go's GC frees it. The port
        // freezes its host and frees its checker pool and program version,
        // as `dispose` does for a program that no snapshot holds. Its parse
        // cache counts stay, as in Go (no snapshot derefs them). The host of
        // a project that the clone deleted gives its resolve-ahead keys to
        // the stash, for a later clone that makes the project again
        // (`ResolveAheadStash`).
        let projects = new_snapshot.project_collection.projects();
        let kept: FxHashSet<*const compiler::NewProgram> = projects
            .iter()
            .filter_map(|project| project.borrow().program.as_ref().map(Rc::as_ptr))
            .collect();
        let kept_configs: FxHashSet<tspath::Path> = projects
            .iter()
            .map(|project| project.borrow().config_file_path.clone())
            .collect();
        for made in project_collection_builder.made_programs.take() {
            if kept.contains(&Rc::as_ptr(&made.program)) {
                continue;
            }
            if made.host.builder.borrow().is_some() {
                made.host.freeze(
                    snapshot_fs.clone(),
                    new_snapshot.config_file_registry.clone(),
                );
            }
            if !kept_configs.contains(&made.host.config_file_path) {
                store.resolve_ahead_stash.put(&made.host);
            }
            made.checker_pool.discard();
            crate::ls::release_search_thread(&made.program);
            crate::program::ls_program::release_program(&made.program);
        }
        // PORT: Go map order is random; the registry map's order here (the
        // owner adds do not depend on the order).
        for config in new_snapshot.config_file_registry.configs.values() {
            let config = config.borrow();
            if let Some(command_line) = &config.command_line {
                if let Some(config_file) = &command_line.config_file {
                    for file in &config_file.extended_source_files {
                        store
                            .extended_config_cache
                            .add_owner(&(store.to_path)(file), new_snapshot.id);
                    }
                }
            }
        }

        autoimport::RegistryCloneHost::dispose(&*auto_import_host);

        logger.logf(&format!(
            "Finished cloning snapshot {} into snapshot {} in {:?}",
            self.id,
            new_snapshot.id,
            start.elapsed()
        ));
        new_snapshot
    }

    // Go: project/snapshot.go:732 ref
    // ref increments the snapshot's reference count, preventing it from being
    // disposed until a corresponding Deref is called. The snapshot must still
    // be alive (refCount > 0) when ref is called.
    pub fn ref_(&self) {
        // Go: s.refCount.Add(1)
        let rc = self.ref_count.get() + 1;
        self.ref_count.set(rc);
        if rc <= 1 {
            crate::core::go_panic(format!(
                "snapshot {}: ref on disposed snapshot, parentId={}",
                self.id, self.parent_id
            ));
        }
    }

    // Go: project/snapshot.go:741 tryRef
    // tryRef attempts to increment the snapshot's reference count. If the
    // snapshot is already disposed (refCount == 0), it returns false without
    // modifying the count. On success the caller must eventually call Deref.
    // PORT: one thread, so the compare-and-swap always succeeds.
    pub fn try_ref(&self) -> bool {
        let rc = self.ref_count.get();
        if rc <= 0 {
            return false;
        }
        self.ref_count.set(rc + 1);
        true
    }

    // Go: project/snapshot.go:755 Deref
    // Deref decrements the snapshot's reference count. When the count reaches
    // zero, the snapshot is disposed and its store-owned resources are released.
    pub fn deref(&self) {
        // Go: s.refCount.Add(-1)
        let rc = self.ref_count.get() - 1;
        self.ref_count.set(rc);
        if rc < 0 {
            crate::core::go_panic(format!(
                "snapshot {}: ref count below zero, parentId={}",
                self.id, self.parent_id
            ));
        }
        if rc == 0 {
            self.dispose();
        }
    }

    // Go: project/snapshot.go:765 dispose
    pub fn dispose(&self) {
        let store = &self.host;
        for project in self.project_collection.projects() {
            let project = project.borrow();
            // PORT: Go `project.Program` (the field).
            if let Some(program) = &project.program {
                if store.program_counter.deref(program) {
                    if let Some(content_mapper_project) = program.content_mapper_project() {
                        let _ = content_mapper_project.close();
                    }
                    // This program is no longer referenced by any snapshot.
                    // Mark its checker pool as discarded so its idle-cleanup timer stops
                    // keeping the pool alive, allowing the pool and any idle checkers it
                    // still references to be reclaimed when the pool is garbage-collected.
                    if let Some(checker_pool) = &project.checker_pool {
                        checker_pool.discard();
                    }
                    // PORT: through the entries the program holds a count on
                    // (`ProgramFileRefs`), so no file builds a key unless its
                    // entry goes.
                    project
                        .program_file_refs
                        .as_ref()
                        .expect("a project program has its file refs")
                        .release(
                            &store.parse_cache,
                            &store.content_mapped_parse_cache,
                            program.source_files(),
                        );
                    for file in program.duplicate_source_files() {
                        if !file.is_content_mapper_failure_stub {
                            if !file.content_mapper.is_empty() {
                                deref_content_mapped_file(
                                    &store.content_mapped_parse_cache,
                                    &content_mapped_parse_cache_key_for_duplicate(file),
                                );
                            } else {
                                deref_program_file(
                                    &store.parse_cache,
                                    &file.parse_options,
                                    file.source_hash(),
                                    file.script_kind,
                                );
                            }
                        }
                    }
                    // PORT: Go frees the program when nothing references it.
                    // The port frees its checkers and its program version
                    // now, or when the last request on it ends; the program
                    // goes with its last `Rc` holder. Its cross-project
                    // search thread ends after its queued jobs. The search
                    // thread is found by the program version, so it is
                    // released first.
                    crate::ls::release_search_thread(program);
                    crate::program::ls_program::release_program(program);
                }
            }
        }
        // PORT: Go map order is random; the registry map's order here (the
        // releases do not depend on the order).
        for config in self.config_file_registry.configs.values() {
            let config = config.borrow();
            if let Some(command_line) = &config.command_line {
                for file in command_line.extended_source_files() {
                    store
                        .extended_config_cache
                        .release(&(store.to_path)(file), self.id);
                }
            }
        }
    }
}
