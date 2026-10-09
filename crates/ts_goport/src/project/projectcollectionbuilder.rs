//! Go `internal/project/projectcollectionbuilder.go`.
//!
//! PORT: one thread (project/dirty/interfaces.rs). The builder is shared
//! (`Rc<ProjectCollectionBuilder>`: projects and compiler hosts keep a
//! pointer to it until `freeze`), so its methods take `self: &Rc<Self>` and
//! the fields Go writes after construction are `Cell` / `RefCell`. Go
//! `dirty.Value[*Project]` is `&dyn dirty::Value<Rc<RefCell<Project>>>`
//! (a returned one is `Rc<dyn ..>`). Go `collections.Set` and
//! `map[K]struct{}` are `FxHashSet`; Go `collections.SyncSet` is a
//! `RefCell<FxHashSet>`. `core.BreadthFirstSearchParallelEx` and the
//! parallel `core.WorkGroup` run serially in queue order. Log text uses
//! `{:?}` for Go `%v` (log only).

use crate::project::prelude::*;

use crate::contentmapper;
use crate::frontend::core_ext::{ProjectReference, get_script_kind_from_file_name};
use crate::frontend::{core_bfs, core_ls_ext, core_workgroup};
use std::cell::Cell;
use std::collections::VecDeque;
use std::time::Instant;

// Go: project/projectcollectionbuilder.go:26 projectLoadKind
// PORT: Go `type projectLoadKind int` with iota consts; Go
// `projectLoadKindFind` is `ProjectLoadKind::FIND` (same values).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ProjectLoadKind(pub i32);

impl ProjectLoadKind {
    // Project is not created or updated, only looked up in cache
    pub const FIND: ProjectLoadKind = ProjectLoadKind(0);
    // Project is created and then its graph is updated
    pub const CREATE: ProjectLoadKind = ProjectLoadKind(1);
}

// Go: project/projectcollectionbuilder.go:35 ProjectCollectionBuilder
// PORT: tsgo#4712 adds the content-mapped parse cache, the content mapper
// host (Go nil interface is `None`) and the inferred project mappers.
pub struct ProjectCollectionBuilder {
    pub session_options: Rc<SessionOptions>,
    pub parse_cache: Rc<ParseCache>,
    pub content_mapped_parse_cache: Rc<ContentMappedParseCache>,
    pub extended_config_cache: Rc<ExtendedConfigCache>,
    pub content_mapper_host: Option<Rc<dyn contentmapper::Host>>,
    /// The session's stash of deleted projects' resolve-ahead keys, which
    /// the hosts of the projects that this clone makes again take.
    // PORT: not in Go (see `ResolveAheadStash`).
    pub resolve_ahead_stash: Rc<ResolveAheadStash>,
    pub to_path: Rc<dyn Fn(&str) -> tspath::Path>,

    pub ctx: Context,
    pub fs: Rc<SnapshotFSBuilder>,
    // ts#64291
    pub overlays: Rc<IndexMap<tspath::Path, Rc<Overlay>>>,
    pub base: Rc<ProjectCollection>,
    pub compiler_options_for_inferred_projects: Option<Rc<CompilerOptions>>,
    pub inferred_content_mappers: Vec<Rc<contentmapper::Mapper>>,
    pub inferred_content_mapper_extensions: Vec<String>,
    pub config_file_registry_builder: Rc<ConfigFileRegistryBuilder>,

    pub client: Option<Rc<dyn Client>>, // optional; used for project loading notifications

    pub new_snapshot_id: u64,
    pub program_structure_changed: Cell<bool>,
    pub default_projects_invalidated: Cell<bool>,
    pub open_files_changed: Cell<bool>,

    // PORT: a Go nil map is an empty map (Go only reads it, compares it
    // with `maps.Equal`, or makes it before a write).
    // ts#64319: typed project IDs.
    pub file_default_projects: RefCell<FxHashMap<tspath::Path, ID>>,
    pub configured_projects: Rc<dirty::SyncMap<ConfiguredProjectID, Rc<RefCell<Project>>>>,
    // ts#64204
    pub synthetic_projects: Rc<dirty::SyncMap<SyntheticProjectID, Rc<RefCell<Project>>>>,
    pub inferred_project: Rc<dirty::Box<Rc<RefCell<Project>>>>,
    pub created_programs: RefCell<Vec<Rc<RefCell<Project>>>>,
    /// Each program that `update_program` made in this clone, with its host
    /// and checker pool. The snapshot frees the ones that no project of the
    /// new collection has (`Snapshot::clone`).
    // PORT: not in Go. Go's GC frees a program that this clone made for a
    // project that it then deleted (hono: tsconfig.spec.json at each file
    // open) or updated again.
    pub made_programs: RefCell<Vec<MadeProgram>>,

    pub api_state: RefCell<APIState>,
}

/// A program that the builder made, with the host and checker pool that it
/// gave the project with it (`ProjectCollectionBuilder::made_programs`).
// PORT: not in Go.
pub struct MadeProgram {
    pub program: Rc<compiler::NewProgram>,
    pub host: Rc<CompilerHost>,
    pub checker_pool: Rc<CheckerPool>,
}

// Go: project/projectcollectionbuilder.go:68 newProjectCollectionBuilder
// PORT: Go `oldAPIState.clone()` copies the state, so the caller passes it by
// reference.
#[allow(clippy::too_many_arguments)]
pub fn new_project_collection_builder(
    ctx: &Context,
    new_snapshot_id: u64,
    fs: Rc<SnapshotFSBuilder>,
    overlays: Rc<IndexMap<tspath::Path, Rc<Overlay>>>,
    old_project_collection: Rc<ProjectCollection>,
    old_config_file_registry: Rc<ConfigFileRegistry>,
    old_api_state: &APIState,
    compiler_options_for_inferred_projects: Option<Rc<CompilerOptions>>,
    inferred_content_mappers: Vec<Rc<contentmapper::Mapper>>,
    inferred_content_mapper_extensions: Vec<String>,
    session_options: Rc<SessionOptions>,
    custom_config_file_name: &str,
    parse_cache: Rc<ParseCache>,
    content_mapped_parse_cache: Rc<ContentMappedParseCache>,
    extended_config_cache: Rc<ExtendedConfigCache>,
    content_mapper_host: Option<Rc<dyn contentmapper::Host>>,
    resolve_ahead_stash: Rc<ResolveAheadStash>,
    client: Option<Rc<dyn Client>>,
) -> Rc<ProjectCollectionBuilder> {
    let open_files = open_file_paths(&overlays);
    let is_open_file: Rc<dyn Fn(&tspath::Path) -> bool> = {
        let overlays = overlays.clone();
        Rc::new(move |path: &tspath::Path| overlays.contains_key(path))
    };
    let config_file_registry_builder = new_config_file_registry_builder(
        lsproto::get_client_capabilities(ctx)
            .workspace
            .did_change_watched_files
            .relative_pattern_support,
        fs.clone(),
        is_open_file,
        old_config_file_registry,
        extended_config_cache.clone(),
        new_snapshot_id,
        session_options.clone(),
        custom_config_file_name,
        None,
    );
    let open_files_changed = open_files != old_project_collection.open_files;
    Rc::new(ProjectCollectionBuilder {
        ctx: ctx.clone(),
        to_path: fs.to_path.clone(),
        fs,
        overlays,
        compiler_options_for_inferred_projects,
        inferred_content_mappers,
        inferred_content_mapper_extensions,
        session_options,
        parse_cache,
        content_mapped_parse_cache,
        extended_config_cache,
        content_mapper_host,
        resolve_ahead_stash,
        config_file_registry_builder,
        new_snapshot_id,
        configured_projects: dirty::new_sync_map(
            old_project_collection.configured_projects.clone(),
        ),
        synthetic_projects: dirty::new_sync_map(old_project_collection.synthetic_projects.clone()),
        inferred_project: dirty::new_box(old_project_collection.inferred_project.clone()),
        created_programs: RefCell::new(Vec::new()),
        made_programs: RefCell::new(Vec::new()),
        api_state: RefCell::new(old_api_state.clone()),
        client,
        base: old_project_collection,
        program_structure_changed: Cell::new(false),
        default_projects_invalidated: Cell::new(false),
        open_files_changed: Cell::new(open_files_changed),
        file_default_projects: RefCell::new(FxHashMap::default()),
    })
}

// PORT: Go `ensureCloned` (closure in Finalize). `None` is Go
// `changed == false` (the base collection is still the result).
fn ensure_cloned<'c>(
    new_project_collection: &'c mut Option<ProjectCollection>,
    base: &ProjectCollection,
) -> &'c mut ProjectCollection {
    if new_project_collection.is_none() {
        *new_project_collection = Some(ProjectCollection::clone(base));
    }
    new_project_collection.as_mut().expect("cloned above")
}

impl ProjectCollectionBuilder {
    // Go: project/projectcollectionbuilder.go:113 isOpenFile (ts#64291)
    pub fn is_open_file(&self, path: &tspath::Path) -> bool {
        self.overlays.contains_key(path)
    }

    // Go: project/projectcollectionbuilder.go:118 Finalize
    pub fn finalize(
        self: &Rc<Self>,
        _logger: Option<Rc<logging::LogTree>>,
    ) -> (Rc<ProjectCollection>, Rc<ConfigFileRegistry>) {
        // PORT: Go `changed` + `newProjectCollection := b.base`; the clone is
        // owned until it is returned.
        let mut new_project_collection: Option<ProjectCollection> = None;

        let (configured_projects, configured_projects_changed) =
            self.configured_projects.finalize_exported();
        if configured_projects_changed {
            ensure_cloned(&mut new_project_collection, &self.base).configured_projects =
                configured_projects;
        }
        // ts#64204
        let (synthetic_projects, synthetic_projects_changed) =
            self.synthetic_projects.finalize_exported();
        if synthetic_projects_changed {
            ensure_cloned(&mut new_project_collection, &self.base).synthetic_projects =
                synthetic_projects;
        }

        if self.open_files_changed.get() {
            ensure_cloned(&mut new_project_collection, &self.base).open_files =
                open_file_paths(&self.overlays);
        }

        if *self.file_default_projects.borrow() != self.base.file_default_projects {
            ensure_cloned(&mut new_project_collection, &self.base).file_default_projects =
                self.file_default_projects.borrow().clone();
        }

        let (new_inferred_project, inferred_project_changed) = self.inferred_project.finalize();
        if inferred_project_changed {
            ensure_cloned(&mut new_project_collection, &self.base).inferred_project =
                new_inferred_project;
        }

        let config_file_registry = self.config_file_registry_builder.finalize();
        let same_registry = matches!(
            &self.base.config_file_registry,
            Some(base) if Rc::ptr_eq(base, &config_file_registry)
        );
        if !same_registry {
            ensure_cloned(&mut new_project_collection, &self.base).config_file_registry =
                Some(config_file_registry.clone());
        }

        if *self.api_state.borrow() != self.base.api_state {
            ensure_cloned(&mut new_project_collection, &self.base).api_state =
                self.api_state.borrow().clone();
        }

        let new_project_collection = match new_project_collection {
            Some(cloned) => Rc::new(cloned),
            None => self.base.clone(),
        };
        (new_project_collection, config_file_registry)
    }

    // Go: project/projectcollectionbuilder.go:166 forEachProject
    pub fn for_each_project(
        self: &Rc<Self>,
        fn_: &mut dyn FnMut(&dyn dirty::Value<Rc<RefCell<Project>>>) -> bool,
    ) {
        let mut keep_going = true;
        self.configured_projects.range(&mut |entry| {
            keep_going = fn_(&**entry);
            keep_going
        });
        // ts#64204
        if keep_going {
            self.synthetic_projects.range(&mut |entry| {
                keep_going = fn_(&**entry);
                keep_going
            });
        }
        if !keep_going {
            return;
        }
        if self.inferred_project.value().is_some() {
            fn_(&*self.inferred_project);
        }
    }

    // Go: project/projectcollectionbuilder.go:186 HandleAPIRequest
    pub fn handle_api_request(
        self: &Rc<Self>,
        api_request: &APISnapshotRequest,
        logger: Option<Rc<logging::LogTree>>,
    ) -> Result<(), GoError> {
        // PORT: a Go nil map is an empty set.
        let mut projects_to_close: FxHashSet<tspath::Path> = FxHashSet::default();
        if let Some(close_projects) = &api_request.close_projects {
            let mut api_state = self.api_state.borrow_mut();
            let open_projects = &mut api_state.open_projects;
            for project_path in close_projects {
                // Ref-counted close: only actually close the project once the last
                // API client that opened it releases it.
                // PORT: a missing Go map key reads as 0.
                let count = open_projects.get(project_path).copied().unwrap_or(0);
                if count > 1 {
                    open_projects.insert(project_path.clone(), count - 1);
                } else if count == 1 {
                    open_projects.shift_remove(project_path);
                    projects_to_close.insert(project_path.clone());
                }
            }
        }

        if let Some(open_projects) = &api_request.open_projects {
            // PORT: Go map order is random; FxHashSet order here.
            for config_file_name in open_projects {
                let config_path = (self.to_path)(config_file_name);
                if let Some(entry) = self.find_or_create_project(
                    config_file_name,
                    &config_path,
                    ProjectLoadKind::CREATE,
                    logger.clone(),
                ) {
                    *self
                        .api_state
                        .borrow_mut()
                        .open_projects
                        .entry(config_path.clone())
                        .or_insert(0) += 1;
                    // A project re-opened in the same request shouldn't be closed.
                    projects_to_close.remove(&config_path);
                    // ts#64204
                    self.update_program(&*entry, logger.clone());
                } else {
                    return Err(gostd::errors::errorf(
                        format!("project not found for open: {}", config_file_name),
                        vec![],
                    ));
                }
            }
        }

        if let Some(close_files) = &api_request.close_files {
            let mut api_state = self.api_state.borrow_mut();
            let open_files = &mut api_state.open_files;
            for path in close_files {
                let Some(entry) = open_files.get_mut(path) else {
                    continue;
                };
                if entry.ref_count > 1 {
                    entry.ref_count -= 1;
                } else {
                    open_files.shift_remove(path);
                }
            }
        }

        if let Some(request_open_files) = &api_request.open_files {
            let mut api_state = self.api_state.borrow_mut();
            let open_files = &mut api_state.open_files;
            // ts#64391: the request maps each path to its file name.
            for (path, file_name) in request_open_files {
                // PORT: Go reads the zero entry for a missing key and stores
                // it back.
                let entry = open_files.entry(path.clone()).or_default();
                entry.file_name = file_name.clone();
                entry.ref_count += 1;
            }
        }

        for overlay in self.overlays.values() {
            let file_name = overlay.file_name();
            if let Some(entry) =
                self.find_default_configured_project(&file_name, &(self.to_path)(&file_name))
            {
                let config_file_path = entry
                    .value()
                    .unwrap_or_else(|| crate::core::go_nil_dereference())
                    .borrow()
                    .config_file_path
                    .clone();
                projects_to_close.remove(&config_file_path);
            }
        }

        for project_path in &projects_to_close {
            if let (Some(entry), true) = self
                .configured_projects
                .load(&ConfiguredProjectID(project_path.clone()))
            {
                self.delete_project(&*entry, logger.clone());
            }
        }

        // Place newly API-opened files like LSP's textDocument/didOpen, ensuring only
        // their target projects. Existing API-opened files are retained by cleanup below
        // without implicitly updating their programs.
        if let Some(request_open_files) = &api_request.open_files {
            let mut retain: FxHashSet<tspath::Path> = FxHashSet::default();
            let mut ensure_inferred_project = false;
            for (path, file_name) in request_open_files {
                if self.is_open_file(path) {
                    if self
                        .find_default_configured_project(file_name, path)
                        .is_none()
                        && !self.is_supported_in_inferred_project(file_name)
                    {
                        return Err(gostd::errors::errorf(
                            format!("no project found for opened file: {}", file_name),
                            vec![],
                        ));
                    }
                    continue;
                }
                let result = self.ensure_configured_project_and_ancestors_for_file(
                    file_name,
                    path,
                    logger.clone(),
                );
                retain.extend(result.retain);
                if result.project.is_none() {
                    if !self.is_supported_in_inferred_project(file_name) {
                        return Err(gostd::errors::errorf(
                            format!("no project found for opened file: {}", file_name),
                            vec![],
                        ));
                    }
                    ensure_inferred_project = true;
                }
            }
            self.cleanup_configured_projects(&retain, logger.clone());
            if ensure_inferred_project && self.inferred_project.value().is_some() {
                self.update_program(&*self.inferred_project, logger.clone());
            }
        } else if api_request.close_files.is_some() {
            // Go: b.cleanupConfiguredProjects(nil, logger)
            self.cleanup_configured_projects(&FxHashSet::default(), logger.clone());
        }
        let mut seen_reconfigured_programs: FxHashSet<SyntheticProjectID> = FxHashSet::default();
        for request in &api_request.reconfigure_programs {
            if seen_reconfigured_programs.contains(&request.program_id) {
                return Err(gostd::errors::errorf(
                    format!(
                        "synthetic program reconfigured more than once: {}",
                        request.program_id
                    ),
                    vec![],
                ));
            }
            seen_reconfigured_programs.insert(request.program_id.clone());
            if api_request
                .remove_programs
                .as_ref()
                .is_some_and(|remove_programs| remove_programs.contains(&request.program_id))
            {
                return Err(gostd::errors::errorf(
                    format!(
                        "synthetic program cannot be reconfigured and removed: {}",
                        request.program_id
                    ),
                    vec![],
                ));
            }
            let (_, ok) = self.synthetic_projects.load(&request.program_id);
            if !ok {
                return Err(gostd::errors::errorf(
                    format!(
                        "synthetic program not found for reconfiguration: {}",
                        request.program_id
                    ),
                    vec![],
                ));
            }
        }
        for program_id in api_request.remove_programs.iter().flatten() {
            let (Some(project), true) = self.synthetic_projects.load(program_id) else {
                return Err(gostd::errors::errorf(
                    format!("synthetic program not found for removal: {}", program_id),
                    vec![],
                ));
            };
            self.delete_project(&*project, logger.clone());
        }
        let mut created_entries: Vec<
            Rc<dirty::SyncMapEntry<SyntheticProjectID, Rc<RefCell<Project>>>>,
        > = Vec::with_capacity(api_request.create_programs.len());
        // PORT: an empty `project_references` is the Go nil slice.
        for request in &api_request.create_programs {
            let entry = self.update_or_create_synthetic_project(
                self.next_synthetic_project_id(),
                request.root_file_names.clone(),
                Some(request.compiler_options.clone()),
                (!request.project_references.is_empty())
                    .then(|| request.project_references.clone()),
                request.config_file_parsing_diagnostics.clone(),
                // ts#64299
                request.module_resolver_factory.clone(),
                request.module_resolver_id,
                self.inferred_content_mappers.clone(),
                logger.clone(),
            );
            created_entries.push(entry);
        }
        let mut reconfigured_entries: Vec<
            Rc<dirty::SyncMapEntry<SyntheticProjectID, Rc<RefCell<Project>>>>,
        > = Vec::with_capacity(api_request.reconfigure_programs.len());
        for request in &api_request.reconfigure_programs {
            let create_request = &request.api_create_program_request;
            reconfigured_entries.push(
                self.update_or_create_synthetic_project(
                    request.program_id.clone(),
                    create_request.root_file_names.clone(),
                    Some(create_request.compiler_options.clone()),
                    (!create_request.project_references.is_empty())
                        .then(|| create_request.project_references.clone()),
                    create_request.config_file_parsing_diagnostics.clone(),
                    // ts#64299
                    create_request.module_resolver_factory.clone(),
                    create_request.module_resolver_id,
                    self.inferred_content_mappers.clone(),
                    logger.clone(),
                ),
            );
        }
        // PORT: Go runs each update on its own goroutine (`sync.WaitGroup`);
        // the port runs them serially in Go start order (PORTING "Threads").
        // Go: projectcollectionbuilder.go:337,345 `wg.Go`. A `recover()`
        // sees only its own goroutine, so a Go panic in an update ends the
        // process whatever the API request recovers
        // (`go_wait_group_goroutine`).
        let mut created_programs: Vec<Rc<RefCell<Project>>> =
            Vec::with_capacity(created_entries.len());
        for entry in &created_entries {
            crate::core::go_wait_group_goroutine(|| {
                if entry
                    .value()
                    .unwrap_or_else(|| crate::core::go_nil_dereference())
                    .borrow()
                    .dirty
                {
                    self.update_program(&**entry, logger.clone());
                }
                created_programs.push(
                    entry
                        .value()
                        .unwrap_or_else(|| crate::core::go_nil_dereference()),
                );
            });
        }
        for entry in &reconfigured_entries {
            crate::core::go_wait_group_goroutine(|| {
                if entry
                    .value()
                    .unwrap_or_else(|| crate::core::go_nil_dereference())
                    .borrow()
                    .dirty
                {
                    self.update_program(&**entry, logger.clone());
                }
            });
        }
        *self.created_programs.borrow_mut() = created_programs;
        // ts#64391: the request maps each path to its file name.
        for (path, file_name) in api_request.ensure_files.iter().flatten() {
            self.did_request_file(
                file_name,
                path,
                false, /*configuredProjectsOnly*/
                logger.clone(),
            );
            // ts#64374
            if self.find_default_project(file_name, path).is_none() {
                return Err(gostd::errors::errorf(
                    format!("no project found for opened file: {}", file_name),
                    vec![],
                ));
            }
        }
        for project_id in api_request.ensure_programs.iter().flatten() {
            self.did_request_project(project_id, logger.clone());
        }
        if api_request.ensure_all_programs {
            self.for_each_project(
                &mut |entry: &dyn dirty::Value<Rc<RefCell<Project>>>| -> bool {
                    self.update_program(entry, logger.clone());
                    true
                },
            );
        }
        // ts#64299
        let mut module_resolution_error: Option<GoError> = None;
        self.for_each_project(
            &mut |entry: &dyn dirty::Value<Rc<RefCell<Project>>>| -> bool {
                let project = entry
                    .value()
                    .unwrap_or_else(|| crate::core::go_nil_dereference());
                if let Some(program) = &project.borrow().program {
                    module_resolution_error = program.module_resolution_error();
                }
                module_resolution_error.is_none()
            },
        );
        match module_resolution_error {
            Some(err) => Err(err),
            None => Ok(()),
        }
    }

    // Go: project/projectcollectionbuilder.go:379 nextSyntheticProjectID (ts#64319: was nextSyntheticProjectName)
    pub fn next_synthetic_project_id(&self) -> SyntheticProjectID {
        let mut id = 1;
        loop {
            let project_id = new_synthetic_project_id(id);
            let (_, ok) = self.synthetic_projects.load(&project_id);
            if !ok {
                return project_id;
            }
            id += 1;
        }
    }

    /// The API-opened files as (path, file name) pairs, in map order.
    // PORT: Go ranges over the live `apiState.openFiles`. The loop bodies do
    // not change it, so the port copies it first and holds no borrow.
    fn api_opened_files(&self) -> Vec<(tspath::Path, String)> {
        self.api_state
            .borrow()
            .open_files
            .iter()
            .map(|(path, file)| (path.clone(), file.file_name.clone()))
            .collect()
    }

    // Go: project/projectcollectionbuilder.go:388 DidChangeFiles
    // PORT: Go passes the summary by value; here by reference.
    pub fn did_change_files(
        self: &Rc<Self>,
        summary: &FileChangeSummary,
        logger: Option<Rc<logging::LogTree>>,
    ) {
        self.open_files_changed.set(
            self.open_files_changed.get()
                || !summary.opened.0.is_empty()
                || !summary.closed.is_empty(),
        );

        let to_paths = |uris: &FxHashSet<lsproto::DocumentUri>| -> Vec<tspath::Path> {
            let mut paths: Vec<tspath::Path> = Vec::with_capacity(uris.len());
            for uri in uris {
                paths.push((self.to_path)(&uri.file_name()));
            }
            paths
        };
        let changed_files = to_paths(&summary.changed);
        let deleted_files = to_paths(&summary.deleted);
        let created_files = to_paths(&summary.created);
        if self.content_mapper_host.is_some() {
            let all_watch_changes: Vec<tspath::Path> = changed_files
                .iter()
                .chain(&deleted_files)
                .chain(&created_files)
                .cloned()
                .collect();
            self.for_each_project(
                &mut |entry: &dyn dirty::Value<Rc<RefCell<Project>>>| -> bool {
                    self.refresh_content_mapper_project_for_changes(
                        entry,
                        &all_watch_changes,
                        summary.has_excessive_non_create_watch_events(),
                        logger.clone(),
                    );
                    true
                },
            );
        }

        let config_change_logger = logger.fork("Checking for changes affecting config files");
        let config_change_result = self
            .config_file_registry_builder
            .did_change_files(summary, config_change_logger.clone());
        log_change_file_result(&config_change_result, &config_change_logger);

        self.program_structure_changed.set(
            self.mark_projects_affected_by_config_changes(&config_change_result, logger.clone()),
        );

        self.for_each_project(
            &mut |entry: &dyn dirty::Value<Rc<RefCell<Project>>>| -> bool {
                // Only consider change/delete; creates are handled by the config file registry
                if summary.has_excessive_non_create_watch_events() {
                    entry.change(&mut |p: &Rc<RefCell<Project>>| {
                        let mut p = p.borrow_mut();
                        p.dirty = true;
                        p.dirty_file_path = tspath::Path::default();
                        if logger.is_some() {
                            logger.logf(&format!(
                                "Marking project as dirty due to excessive watch changes: {}",
                                p.id()
                            ));
                        }
                    });
                    return true;
                }

                // Handle closed and changed files
                self.mark_files_changed(
                    entry,
                    &changed_files,
                    lsproto::FileChangeType::CHANGED,
                    logger.clone(),
                );
                let value = entry
                    .value()
                    .unwrap_or_else(|| crate::core::go_nil_dereference());
                if value.borrow().kind == Kind::INFERRED && !summary.closed.is_empty() {
                    // PORT: Go `newRootFiles` aliases the command line's slice and
                    // `slices.Delete` edits it in place; the port edits a copy.
                    let (root_files_map, mut new_root_files) = {
                        let value = value.borrow();
                        let command_line = value
                            .command_line
                            .as_ref()
                            .unwrap_or_else(|| crate::core::go_nil_dereference());
                        (
                            command_line.file_names_by_path().clone(),
                            command_line.file_names().to_vec(),
                        )
                    };
                    for uri in &summary.closed {
                        let file_name = uri.file_name();
                        let path = (self.to_path)(&file_name);
                        if root_files_map.contains_key(&path) {
                            // Go: slices.Delete(newRootFiles, slices.Index(newRootFiles, fileName), slices.Index(newRootFiles, fileName)+1)
                            // A missing file is index -1. The bound check is the
                            // 3-index `s[i:j:len(s)]`, so the runtime text is `[-1::]`.
                            let Some(index) = new_root_files.iter().position(|f| *f == file_name)
                            else {
                                crate::core::go_panic(
                                    "runtime error: slice bounds out of range [-1::]".to_string(),
                                );
                            };
                            new_root_files.remove(index);
                        }
                    }
                    self.update_inferred_project_roots(new_root_files, logger.clone());
                }

                // Handle deleted files
                if !summary.deleted.is_empty() {
                    self.mark_files_changed(
                        entry,
                        &deleted_files,
                        lsproto::FileChangeType::DELETED,
                        logger.clone(),
                    );
                }

                // Handle created files
                if !summary.created.is_empty() {
                    self.mark_files_changed(
                        entry,
                        &created_files,
                        lsproto::FileChangeType::CREATED,
                        logger.clone(),
                    );
                }

                true
            },
        );

        // Handle opened file
        if !summary.opened.0.is_empty() || !summary.reopened.0.is_empty() {
            let file_name =
                core_ls_ext::first_non_zero([summary.opened.clone(), summary.reopened.clone()])
                    .file_name();
            let path = (self.to_path)(&file_name);
            let open_file_result = self.ensure_configured_project_and_ancestors_for_file(
                &file_name,
                &path,
                logger.clone(),
            );
            self.cleanup_configured_projects(&open_file_result.retain, logger);
        }
    }

    // Go: project/projectcollectionbuilder.go:465 refreshContentMapperProjectForChanges (tsgo#4712)
    pub fn refresh_content_mapper_project_for_changes(
        &self,
        entry: &dyn dirty::Value<Rc<RefCell<Project>>>,
        paths: &[tspath::Path],
        refresh_all: bool,
        logger: Option<Rc<logging::LogTree>>,
    ) {
        let project = entry
            .value()
            .unwrap_or_else(|| crate::core::go_nil_dereference());
        let (program, content_mapper_watched_files) = {
            let project = project.borrow();
            (
                project.program.clone(),
                project.content_mapper_watched_files.clone(),
            )
        };
        let (Some(program), Some(content_mapper_watched_files)) =
            (program, content_mapper_watched_files)
        else {
            return;
        };
        let mut affected = refresh_all;
        if !affected
            && paths
                .iter()
                .any(|path| content_mapper_watched_files.contains(path))
        {
            affected = true;
        }
        if !affected {
            return;
        }
        if let Some(content_mapper_project) = program.content_mapper_project() {
            let _ = content_mapper_project.refresh();
        }
        entry.change(&mut |p: &Rc<RefCell<Project>>| {
            let mut p = p.borrow_mut();
            p.dirty = true;
            p.dirty_file_path = tspath::Path::default();
            if logger.is_some() {
                logger.logf(&format!(
                    "Marking project as dirty due to content mapper configuration changes: {}",
                    p.id()
                ));
            }
        });
    }

    // Go: project/projectcollectionbuilder.go:500 cleanupConfiguredProjects
    // cleanupConfiguredProjects sweeps the loaded configured projects and unloads those
    // that are no longer needed. Starting from the set of all configured projects, it
    // retains any project that is the default project (along with its references and
    // ancestor configs) of an open overlay file or an API-opened file, any project
    // explicitly opened through the API, and any project in retain (e.g. the ancestor
    // solution tree built for a freshly opened overlay file). Every other configured
    // project is deleted, the inferred project roots are recomputed, and the config file
    // registry is cleaned up. This is the shared mechanism that keeps the set of loaded
    // projects minimal for both LSP file opens and API file opens/closes.
    pub fn cleanup_configured_projects(
        self: &Rc<Self>,
        retain: &FxHashSet<tspath::Path>,
        logger: Option<Rc<logging::LogTree>>,
    ) {
        // PORT: Go `collections.Set` (random order); `IndexSet` keeps the
        // insertion order so deletions run in a fixed order.
        let mut to_remove_projects: IndexSet<tspath::Path> = IndexSet::new();
        self.configured_projects.range(&mut |entry| {
            to_remove_projects.insert(entry.key().path());
            true
        });

        // Go: retainConfiguredProjectAndReferences (closure in cleanupConfiguredProjects)
        let retain_project_and_references =
            |to_remove_projects: &mut IndexSet<tspath::Path>, project: &Rc<RefCell<Project>>| {
                // Retain project
                // PORT: Go `project.GetProgram()` is the field read.
                let (config_file_path, program) = {
                    let project = project.borrow();
                    (project.config_file_path(), project.program.clone())
                };
                to_remove_projects.shift_remove(&config_file_path);
                if let Some(program) = program {
                    program.range_resolved_project_reference(
                        |reference_path: &tspath::Path, _, _, _| -> bool {
                            if let (_, true) = self
                                .configured_projects
                                .load(&ConfiguredProjectID(reference_path.clone()))
                            {
                                to_remove_projects.shift_remove(reference_path);
                            }
                            true
                        },
                    );
                }
            };

        // Go: retainDefaultConfiguredProject (closure in cleanupConfiguredProjects)
        let retain_default_configured_project =
            |to_remove_projects: &mut IndexSet<tspath::Path>,
             open_file_path: &tspath::Path,
             project: &Rc<RefCell<Project>>| {
                // Retain project and its references
                retain_project_and_references(&mut *to_remove_projects, project);

                // Retain all the ancestor projects
                self.config_file_registry_builder
                    .for_each_config_file_name_for(
                        open_file_path,
                        &mut |config_file_name: &str| {
                            if let Some(ancestor) = self.find_or_create_project(
                                config_file_name,
                                &(self.to_path)(config_file_name),
                                ProjectLoadKind::FIND,
                                logger.clone(),
                            ) {
                                retain_project_and_references(
                                    &mut *to_remove_projects,
                                    &ancestor
                                        .value()
                                        .unwrap_or_else(|| crate::core::go_nil_dereference()),
                                );
                            }
                        },
                    );
            };

        let mut inferred_project_files: Vec<String> = Vec::new();
        // PORT: Go map order is random; the overlay map is an IndexMap.
        for overlay in self.overlays.values() {
            let open_file = overlay.file_name();
            let open_file_path = (self.to_path)(&open_file);
            if let Some(p) = self.find_default_configured_project(&open_file, &open_file_path) {
                retain_default_configured_project(
                    &mut to_remove_projects,
                    &open_file_path,
                    &p.value()
                        .unwrap_or_else(|| crate::core::go_nil_dereference()),
                );
            } else {
                inferred_project_files.push(open_file);
            }
        }
        // Treat API-opened files like open files: retain their configured project (so
        // an LSP-driven open doesn't close it), or keep them as inferred project roots.
        for (path, file_name) in self.api_opened_files() {
            if self.is_open_file(&path) {
                continue;
            }
            if let Some(p) = self.find_default_configured_project(&file_name, &path) {
                retain_default_configured_project(
                    &mut to_remove_projects,
                    &path,
                    &p.value()
                        .unwrap_or_else(|| crate::core::go_nil_dereference()),
                );
            } else {
                inferred_project_files.push(file_name);
            }
        }

        for project_path in &to_remove_projects {
            if retain.contains(project_path) {
                continue;
            }
            if self
                .api_state
                .borrow()
                .open_projects
                .contains_key(project_path)
            {
                continue;
            }
            if let (Some(p), true) = self
                .configured_projects
                .load(&ConfiguredProjectID(project_path.clone()))
            {
                self.delete_project(&*p, logger.clone());
            }
        }
        self.update_inferred_project_roots(inferred_project_files, logger.clone());
        self.config_file_registry_builder.cleanup();
    }

    // Go: project/projectcollectionbuilder.go:571 cleanupAllConfiguredProjects (ts#63950)
    // cleanupAllConfiguredProjects removes all configured projects unconditionally.
    // PORT: Go deletes entries inside `Range`; the port copies the keys first
    // and loads each one again, as Go does.
    pub fn cleanup_all_configured_projects(self: &Rc<Self>, logger: Option<Rc<logging::LogTree>>) {
        let mut keys: Vec<ConfiguredProjectID> = Vec::new();
        self.configured_projects.range(&mut |entry| {
            keys.push(entry.key());
            true
        });
        for key in &keys {
            if let (Some(p), true) = self.configured_projects.load(key) {
                self.delete_project(&*p, logger.clone());
            }
        }
        self.config_file_registry_builder.cleanup();
    }

    // Go: project/projectcollectionbuilder.go:590 collectInferredProjectRoots
    fn collect_inferred_project_roots(self: &Rc<Self>) -> Vec<String> {
        let mut inferred_project_files: Vec<String> = Vec::new();
        for (path, overlay) in self.overlays.iter() {
            if self
                .find_default_configured_project(&overlay.file_name(), path)
                .is_none()
            {
                inferred_project_files.push(overlay.file_name());
            }
        }
        self.append_api_opened_inferred_roots(inferred_project_files)
    }

    // Go: project/projectcollectionbuilder.go:603 appendAPIOpenedInferredRoots
    // appendAPIOpenedInferredRoots appends API-opened files that aren't open in an
    // overlay and have no configured project, so they're kept as inferred project
    // roots and persist across snapshots.
    fn append_api_opened_inferred_roots(
        self: &Rc<Self>,
        mut inferred_project_files: Vec<String>,
    ) -> Vec<String> {
        for (path, file_name) in self.api_opened_files() {
            if self.is_open_file(&path) {
                continue;
            }
            if self
                .find_default_configured_project(&file_name, &path)
                .is_none()
            {
                inferred_project_files.push(file_name);
            }
        }
        inferred_project_files
    }

    // Go: project/projectcollectionbuilder.go:615 cleanupInferredProject
    pub fn cleanup_inferred_project(self: &Rc<Self>, logger: Option<Rc<logging::LogTree>>) {
        self.update_inferred_project_roots(self.collect_inferred_project_roots(), logger);
    }

    // Go: project/projectcollectionbuilder.go:619 DidChangeContentMapperContributions (tsgo#4712)
    pub fn did_change_content_mapper_contributions(
        self: &Rc<Self>,
        logger: Option<Rc<logging::LogTree>>,
    ) {
        self.cleanup_inferred_project(logger.clone());
        if self.inferred_project.value().is_some() {
            self.update_program(&*self.inferred_project, logger);
        }
    }

    // Go: project/projectcollectionbuilder.go:626 ensureInferredProjectIncludesClosedFile
    pub fn ensure_inferred_project_includes_closed_file(
        self: &Rc<Self>,
        file_name: &str,
        logger: Option<Rc<logging::LogTree>>,
    ) {
        // Collect existing inferred project roots (open files not in configured projects)
        // plus this closed file.
        let mut inferred_project_files = self.collect_inferred_project_roots();
        inferred_project_files.push(file_name.to_string());
        self.update_inferred_project_roots(inferred_project_files, logger.clone());
        if self.inferred_project.value().is_some() {
            self.update_program(&*self.inferred_project, logger);
        }
    }

    // Go: project/projectcollectionbuilder.go:639 DidRequestFile
    // DidRequestFile ensures projects are loaded for the given URI.
    // If configuredProjectsOnly is true, only configured projects are loaded; no inferred project is created
    // and it is not guaranteed that there will be any project containing the file in the resulting snapshot.
    // PORT: `_exported`, because Go also has `didRequestFile` (ts#64391,
    // PORTING "Names").
    pub fn did_request_file_exported(
        self: &Rc<Self>,
        uri: &lsproto::DocumentUri,
        configured_projects_only: bool,
        logger: Option<Rc<logging::LogTree>>,
    ) {
        let file_name = uri.file_name();
        let path = (self.to_path)(&file_name);
        self.did_request_file(&file_name, &path, configured_projects_only, logger);
    }

    // Go: project/projectcollectionbuilder.go:645 didRequestFile (ts#64391)
    pub fn did_request_file(
        self: &Rc<Self>,
        file_name: &str,
        path: &tspath::Path,
        configured_projects_only: bool,
        logger: Option<Rc<logging::LogTree>>,
    ) {
        let start_time = Instant::now();
        let file_name = file_name.to_string();
        let path = path.clone();
        if self.default_projects_invalidated.get() {
            self.ensure_configured_project_and_ancestors_for_file(
                &file_name,
                &path,
                logger.clone(),
            );
            if !self.is_open_file(&path) {
                return;
            }
        }
        if self.is_open_file(&path) {
            let mut has_changes = self.program_structure_changed.get();

            // See if we can find a default project without updating a bunch of stuff.
            if let Some(result) = self.find_default_project(&file_name, &path) {
                has_changes = self.update_program(&*result, logger.clone()) || has_changes;
                if result
                    .value()
                    .is_some_and(|project| project.borrow().contains_file(&path))
                {
                    if has_changes {
                        self.cleanup_inferred_project(logger.clone());
                        if self.inferred_project.value().is_some() {
                            self.update_program(&*self.inferred_project, logger.clone());
                        }
                    }
                    return;
                }
            }

            // Make sure all projects we know about are up to date...
            self.configured_projects.range(&mut |entry| {
                has_changes = self.update_program(&**entry, logger.clone()) || has_changes;
                true
            });
            if has_changes {
                // If the structure of other projects changed, we might need to move files
                // in/out of the inferred project.
                self.cleanup_inferred_project(logger.clone());
            }

            if self.inferred_project.value().is_some() {
                self.update_program(&*self.inferred_project, logger.clone());
            }

            // At this point we should be able to find the default project for the file without
            // creating anything else. Initially, I verified that and panicked if nothing was found,
            // but that panic was getting triggered by fourslash infrastructure when it told us to
            // open a package.json file. This is something the VS Code client would never do, but
            // it seems possible that another client would. There's no point in panicking; we don't
            // really even have an error condition until it tries to ask us language questions about
            // a non-TS-handleable file.
        } else {
            let result = self.ensure_configured_project_and_ancestors_for_file(
                &file_name,
                &path,
                logger.clone(),
            );
            if result.project.is_none() && !configured_projects_only {
                // No configured project found for this closed file.
                // Add it to the inferred project so language service requests can be served.
                self.ensure_inferred_project_includes_closed_file(&file_name, logger.clone());
            }
        }

        if logger.is_some() {
            let elapsed = start_time.elapsed();
            logger.log(&format!(
                "Completed file request for {} in {:?}",
                file_name, elapsed
            ));
        }
    }

    // Go: project/projectcollectionbuilder.go:707 DidRequestProject (ts#64319: takes the project ID)
    pub fn did_request_project(
        self: &Rc<Self>,
        project_id: &ID,
        logger: Option<Rc<logging::LogTree>>,
    ) {
        let start_time = Instant::now();
        if project_id.inferred().1 {
            // Update inferred project
            if self.inferred_project.value().is_some() {
                self.update_program(&*self.inferred_project, logger.clone());
            }
        } else if let (synthetic_id, true) = project_id.synthetic() {
            if let (Some(entry), true) = self.synthetic_projects.load(&synthetic_id) {
                self.update_program(&*entry, logger.clone());
            }
        } else if let (configured_id, true) = project_id.configured() {
            if let (Some(entry), true) = self.configured_projects.load(&configured_id) {
                self.update_program(&*entry, logger.clone());
            }
        }

        if logger.is_some() {
            let elapsed = start_time.elapsed();
            logger.log(&format!(
                "Completed project update request for {} in {:?}",
                project_id, elapsed
            ));
        }
    }

    // Go: project/projectcollectionbuilder.go:730 DidRequestProjectTrees
    pub fn did_request_project_trees(
        self: &Rc<Self>,
        project_tree_request: &ProjectTreeRequest,
        logger: Option<Rc<logging::LogTree>>,
    ) {
        let start_time = Instant::now();

        let mut current_projects: Vec<ConfiguredProjectID> = Vec::new();
        self.configured_projects.range(&mut |sme| {
            current_projects.push(sme.key());
            true
        });

        let seen_projects: RefCell<FxHashSet<ConfiguredProjectID>> =
            RefCell::new(FxHashSet::default());
        // PORT: Go `core.NewWorkGroup(false)` is the parallel group, which
        // starts every queued function at once. On the dispatch thread the
        // port runs them serially in queue order (first in, first out), so
        // it builds the port's FIFO group, not `new_work_group` (LIFO).
        let wg: Rc<dyn core_workgroup::WorkGroup<'_> + '_> =
            Rc::new(core_workgroup::ParallelWorkGroup {
                done: Cell::new(false),
                wg: RefCell::new(VecDeque::new()),
            });
        for project_id in current_projects {
            let b = self;
            let wg_inner = wg.clone();
            let seen_projects = &seen_projects;
            let logger = logger.clone();
            wg.queue(Box::new(move || {
                if let (Some(entry), true) = b.configured_projects.load(&project_id) {
                    // If this project has potential project reference for any of the project we are loading ancestor tree for
                    // load this project first
                    if let Some(project) = entry.value() {
                        if project_tree_request.is_all_projects()
                            || project
                                .borrow()
                                .has_potential_project_reference(project_tree_request)
                        {
                            b.update_program(&*entry, logger.clone());
                        }
                    }
                    b.ensure_project_tree(
                        &wg_inner,
                        &entry,
                        project_tree_request,
                        seen_projects,
                        logger.clone(),
                    );
                }
            }));
        }
        wg.run_and_wait();

        if logger.is_some() {
            let elapsed = start_time.elapsed();
            logger.log(&format!(
                "Completed project tree request for {:?} in {:?}",
                project_tree_request.projects(),
                elapsed
            ));
        }
    }

    // Go: project/projectcollectionbuilder.go:761 ensureProjectTree
    pub fn ensure_project_tree<'a>(
        self: &'a Rc<Self>,
        wg: &Rc<dyn core_workgroup::WorkGroup<'a> + 'a>,
        entry: &Rc<dirty::SyncMapEntry<ConfiguredProjectID, Rc<RefCell<Project>>>>,
        project_tree_request: &'a ProjectTreeRequest,
        seen_projects: &'a RefCell<FxHashSet<ConfiguredProjectID>>,
        logger: Option<Rc<logging::LogTree>>,
    ) {
        if !seen_projects.borrow_mut().insert(entry.key()) {
            return;
        }

        let Some(project) = entry.value() else {
            return;
        };

        // PORT: Go `project.GetProgram()` is the field read.
        let Some(program) = project.borrow().program.clone() else {
            return;
        };

        // If this project disables child load ignore it
        if program
            .command_line()
            .compiler_options()
            .disable_referenced_project_load
            .is_true()
        {
            return;
        }

        // PORT: Go returns nil when the program has no root config; that is
        // an empty `Vec` here, and the loop below then does nothing.
        let children = program.get_resolved_project_references();
        if children.is_empty() {
            return;
        }
        for child_config in children {
            let Some(child_config) = child_config else {
                continue;
            };
            let wg_inner = wg.clone();
            let logger = logger.clone();
            let program = Rc::clone(&program);
            wg.queue(Box::new(move || {
                if !project_tree_request.is_all_projects()
                    && program.range_resolved_project_reference_in_child_config(
                        &child_config,
                        |reference_path: &tspath::Path, _config, _, _| -> bool {
                            !project_tree_request.is_project_referenced(reference_path)
                        },
                    )
                {
                    return;
                }

                // Load this child project since this is referenced
                let child_config_path = child_config
                    .config_file
                    .as_ref()
                    .unwrap_or_else(|| crate::core::go_nil_dereference())
                    .path
                    .clone();
                let child_project_entry = self
                    .find_or_create_project(
                        child_config.config_name(),
                        &child_config_path,
                        ProjectLoadKind::CREATE,
                        logger.clone(),
                    )
                    .unwrap_or_else(|| crate::core::go_nil_dereference());
                self.update_program(&*child_project_entry, logger.clone());

                // Ensure children for this project
                self.ensure_project_tree(
                    &wg_inner,
                    &child_project_entry,
                    project_tree_request,
                    seen_projects,
                    logger.clone(),
                );
            }));
        }
    }

    // Go: project/projectcollectionbuilder.go:815 DidUpdateATAState
    // PORT: Go map order is random; FxHashMap order here (log order only).
    pub fn did_update_ata_state(
        self: &Rc<Self>,
        ata_changes: &FxHashMap<ID, Rc<ATAStateChange>>,
        logger: Option<Rc<logging::LogTree>>,
    ) {
        let update_project = |project: &dyn dirty::Value<Rc<RefCell<Project>>>,
                              ata_change: &ATAStateChange| {
            project.change_if(
                &mut |p: Option<&Rc<RefCell<Project>>>| -> bool {
                    let Some(p) = p else {
                        return false;
                    };
                    // Consistency check: the ATA demands (project options, unresolved imports) of this project
                    // has not changed since the time the ATA request was dispatched; the change can still be
                    // applied to this project in its current state.
                    let typings_info = p.borrow().compute_typings_info();
                    ata_change
                        .typings_info
                        .as_ref()
                        .unwrap_or_else(|| crate::core::go_nil_dereference())
                        .equals(&typings_info)
                },
                &mut |p: &Rc<RefCell<Project>>| {
                    let mut p = p.borrow_mut();
                    // We checked before triggering this change (in Session.triggerATAForUpdatedProjects) that
                    // the set of typings files is actually different.
                    p.installed_typings_info = ata_change.typings_info.clone();
                    p.typings_files = ata_change.typings_files.clone();
                    let typings_watch_globs = get_typings_locations_globs(
                        &ata_change.typings_files_to_watch,
                        &self.session_options.typings_location,
                        &self.session_options.current_directory,
                        &p.current_directory,
                        self.fs.fs.use_case_sensitive_file_names(),
                    );
                    let typings_watch =
                        WatchedFiles::clone_(p.typings_watch.as_deref(), typings_watch_globs);
                    p.typings_watch = typings_watch;
                    p.dirty = true;
                    p.dirty_file_path = tspath::Path::default();
                },
            );
        };

        // ts#64319: keyed by project ID.
        for (project_id, ata_change) in ata_changes {
            logger.embed(&ata_change.logs);
            if project_id.inferred().1 {
                update_project(
                    &*self.inferred_project as &dyn dirty::Value<Rc<RefCell<Project>>>,
                    &**ata_change,
                );
            } else if let (synthetic_project_id, true) = project_id.synthetic() {
                if let (Some(project), true) = self.synthetic_projects.load(&synthetic_project_id) {
                    update_project(
                        &*project as &dyn dirty::Value<Rc<RefCell<Project>>>,
                        &**ata_change,
                    );
                }
            } else if let (configured_id, true) = project_id.configured() {
                if let (Some(project), true) = self.configured_projects.load(&configured_id) {
                    update_project(
                        &*project as &dyn dirty::Value<Rc<RefCell<Project>>>,
                        &**ata_change,
                    );
                }
            }

            if logger.is_some() {
                logger.log(&format!("Updated ATA state for project {}", project_id));
            }
        }
    }

    // Go: project/projectcollectionbuilder.go:867 DidChangeCustomConfigFileName
    // if customConfigFileName changes, invalidate default projects.
    pub fn did_change_custom_config_file_name(
        self: &Rc<Self>,
        logger: Option<Rc<logging::LogTree>>,
    ) {
        if !self
            .config_file_registry_builder
            .did_change_custom_config_file_name(logger)
        {
            return;
        }

        // Go: b.fileDefaultProjects = nil
        *self.file_default_projects.borrow_mut() = FxHashMap::default();
        self.default_projects_invalidated.set(true);
        self.program_structure_changed.set(true);
    }

    // Go: project/projectcollectionbuilder.go:877 DidChangeUserPreferences (tsgo#4712)
    pub fn did_change_user_preferences(
        self: &Rc<Self>,
        old_preferences: &lsutil::UserPreferences,
        new_preferences: &lsutil::UserPreferences,
        logger: Option<Rc<logging::LogTree>>,
    ) {
        if old_preferences.locale == new_preferences.locale {
            return;
        }
        self.for_each_project(
            &mut |entry: &dyn dirty::Value<Rc<RefCell<Project>>>| -> bool {
                entry.change(&mut |p: &Rc<RefCell<Project>>| {
                    let mut p = p.borrow_mut();
                    p.dirty = true;
                    p.dirty_file_path = tspath::Path::default();
                    if logger.is_some() {
                        logger.logf(&format!(
                            "Marking project as dirty due to locale change: {}",
                            p.id()
                        ));
                    }
                });
                true
            },
        );
    }

    // Go: project/projectcollectionbuilder.go:893 markProjectsAffectedByConfigChanges
    pub fn mark_projects_affected_by_config_changes(
        self: &Rc<Self>,
        config_change_result: &ChangeFileResult,
        logger: Option<Rc<logging::LogTree>>,
    ) -> bool {
        // PORT: Go map order is random; FxHashSet order here.
        for project_id in &config_change_result.affected_projects {
            // ts#63950: the inferred project can retain a config (a program
            // made by createProgram with project references).
            // ts#64319: the affected projects are project IDs.
            let mut project: Option<Rc<dyn dirty::Value<Rc<RefCell<Project>>>>> = None;
            if project_id.inferred().1 {
                project = Some(
                    self.inferred_project.clone() as Rc<dyn dirty::Value<Rc<RefCell<Project>>>>
                );
            } else {
                if let (synthetic_project_id, true) = project_id.synthetic() {
                    let (synthetic_project, loaded) =
                        self.synthetic_projects.load(&synthetic_project_id);
                    if loaded {
                        project = synthetic_project
                            .map(|entry| entry as Rc<dyn dirty::Value<Rc<RefCell<Project>>>>);
                    }
                }
                if project.is_none() {
                    if let (configured_id, true) = project_id.configured() {
                        // PORT: Go stores the nil `*SyncMapEntry` of a missed
                        // `Load` in the interface, so `project == nil` is
                        // false and `project.Value()` reads the nil entry.
                        project = Some(
                            self.configured_projects
                                .load(&configured_id)
                                .0
                                .map(|entry| entry as Rc<dyn dirty::Value<Rc<RefCell<Project>>>>)
                                .unwrap_or_else(|| crate::core::go_nil_dereference()),
                        );
                    }
                }
            }
            let Some(project) = project.filter(|project| project.value().is_some()) else {
                crate::core::go_panic(format!(
                    "project {} affected by config change not found",
                    project_id
                ));
            };
            project.change_if(
                &mut |p: Option<&Rc<RefCell<Project>>>| -> bool {
                    let p = p
                        .unwrap_or_else(|| crate::core::go_nil_dereference())
                        .borrow();
                    !p.dirty || !p.dirty_file_path.is_empty()
                },
                &mut |p: &Rc<RefCell<Project>>| {
                    let mut p = p.borrow_mut();
                    p.dirty = true;
                    p.dirty_file_path = tspath::Path::default();
                    if logger.is_some() {
                        logger.logf(&format!(
                            "Marking project {} as dirty due to change affecting config",
                            project_id
                        ));
                    }
                },
            );
        }

        // Recompute default projects for open files that now have different config file presence.
        let mut has_changes = false;
        for path in &config_change_result.affected_files {
            // ts#64291
            let Some(overlay) = self.overlays.get(path) else {
                continue;
            };
            let file_name = overlay.file_name();
            let _ = self.ensure_configured_project_and_ancestors_for_file(
                &file_name,
                path,
                logger.clone(),
            );
            has_changes = true;
        }

        has_changes
    }

    // Go: project/projectcollectionbuilder.go:944 findDefaultProject
    pub fn find_default_project(
        self: &Rc<Self>,
        file_name: &str,
        path: &tspath::Path,
    ) -> Option<Rc<dyn dirty::Value<Rc<RefCell<Project>>>>> {
        if let Some(configured_project) = self.find_default_configured_project(file_name, path) {
            return Some(configured_project as Rc<dyn dirty::Value<Rc<RefCell<Project>>>>);
        }
        // ts#64319
        let key_is_inferred = matches!(
            self.file_default_projects.borrow().get(path),
            Some(key) if key.inferred().1
        );
        if key_is_inferred {
            return Some(
                self.inferred_project.clone() as Rc<dyn dirty::Value<Rc<RefCell<Project>>>>
            );
        }
        if let Some(inferred_project) = self.inferred_project.value() {
            if inferred_project.borrow().contains_file(path) {
                // Go: `if b.fileDefaultProjects == nil { make(...) }` (the port map always exists).
                self.file_default_projects
                    .borrow_mut()
                    .insert(path.clone(), inferred_project_id().as_id());
                return Some(
                    self.inferred_project.clone() as Rc<dyn dirty::Value<Rc<RefCell<Project>>>>
                );
            }
        }
        None
    }

    // Go: project/projectcollectionbuilder.go:963 findDefaultConfiguredProject
    pub fn find_default_configured_project(
        self: &Rc<Self>,
        file_name: &str,
        path: &tspath::Path,
    ) -> Option<Rc<dirty::SyncMapEntry<ConfiguredProjectID, Rc<RefCell<Project>>>>> {
        let key = self.file_default_projects.borrow().get(path).cloned();
        if let Some(key) = key {
            if let (configured_id, true) = key.configured() {
                if let (Some(entry), true) = self.configured_projects.load(&configured_id) {
                    return Some(entry);
                }
            }
        }
        // Sort configured projects so we can use a deterministic "first" as a last resort.
        let mut configured_project_paths: Vec<tspath::Path> = Vec::new();
        let mut configured_projects: FxHashMap<
            tspath::Path,
            Rc<dirty::SyncMapEntry<ConfiguredProjectID, Rc<RefCell<Project>>>>,
        > = FxHashMap::default();
        self.configured_projects.range(&mut |entry| {
            let configured_path = entry.key().path();
            configured_project_paths.push(configured_path.clone());
            configured_projects.insert(configured_path, entry.clone());
            true
        });
        configured_project_paths.sort();

        let (project, multiple_candidates) = find_default_configured_project_from_program_inclusion(
            file_name,
            path,
            &configured_project_paths,
            &mut |path: &tspath::Path| {
                configured_projects
                    .get(path)
                    .unwrap_or_else(|| crate::core::go_nil_dereference())
                    .value()
            },
        );

        if multiple_candidates {
            if let Some(p) = self
                .find_or_create_default_configured_project_for_file(
                    file_name,
                    path,
                    ProjectLoadKind::FIND,
                    None,
                )
                .project
            {
                return Some(p);
            }
        }

        configured_projects.get(&project).cloned()
    }

    // Go: project/projectcollectionbuilder.go:995 ensureConfiguredProjectAndAncestorsForFile
    pub fn ensure_configured_project_and_ancestors_for_file(
        self: &Rc<Self>,
        file_name: &str,
        path: &tspath::Path,
        logger: Option<Rc<logging::LogTree>>,
    ) -> SearchResult {
        let mut result = self.find_or_create_default_configured_project_for_file(
            file_name,
            path,
            ProjectLoadKind::CREATE,
            logger.clone(),
        );
        if result.project.is_some() && self.is_open_file(path) {
            self.create_ancestor_tree(file_name, path, &mut result, logger);
        }
        result
    }

    // Go: project/projectcollectionbuilder.go:1003 createAncestorTree
    pub fn create_ancestor_tree(
        self: &Rc<Self>,
        file_name: &str,
        path: &tspath::Path,
        open_result: &mut SearchResult,
        logger: Option<Rc<logging::LogTree>>,
    ) {
        let mut project = open_result
            .project
            .as_ref()
            .unwrap_or_else(|| crate::core::go_nil_dereference())
            .value()
            .unwrap_or_else(|| crate::core::go_nil_dereference());
        loop {
            // Skip if project is not composite and we are only looking for solution
            let (config_file_name, config_file_path, command_line) = {
                let p = project.borrow();
                (
                    p.config_file_name(),
                    p.config_file_path.clone(),
                    p.command_line.clone(),
                )
            };
            if let Some(command_line) = &command_line {
                if !command_line.compiler_options().composite.is_true()
                    || command_line
                        .compiler_options()
                        .disable_solution_searching
                        .is_true()
                {
                    return;
                }
            }

            // Get config file name
            let ancestor_config_name = self
                .config_file_registry_builder
                .get_ancestor_config_file_name(file_name, path, &config_file_name, logger.clone());
            if ancestor_config_name.is_empty() {
                return;
            }

            // find or delay load the project
            let ancestor_path = (self.to_path)(&ancestor_config_name);
            let Some(ancestor) = self.find_or_create_project(
                &ancestor_config_name,
                &ancestor_path,
                ProjectLoadKind::CREATE,
                logger.clone(),
            ) else {
                return;
            };

            open_result.retain.insert(ancestor_path);

            // If this ancestor is new and was not updated because we are just creating it for future loading
            // eg when invoking find all references or rename that could span multiple projects
            // we would make the current project as its potential project reference
            let ancestor_has_no_command_line = ancestor
                .value()
                .unwrap_or_else(|| crate::core::go_nil_dereference())
                .borrow()
                .command_line
                .is_none();
            if ancestor_has_no_command_line
                && command_line
                    .as_ref()
                    .is_none_or(|command_line| command_line.compiler_options().composite.is_true())
            {
                ancestor.change(&mut |ancestor_project: &Rc<RefCell<Project>>| {
                    ancestor_project
                        .borrow_mut()
                        .set_potential_project_reference(&config_file_path);
                });
            }

            project = ancestor
                .value()
                .unwrap_or_else(|| crate::core::go_nil_dereference());
        }
    }

    // Go: project/projectcollectionbuilder.go:1058 findOrCreateDefaultConfiguredProjectWorker
    // PORT: Go `*collections.SyncSet[searchNodeKey]` is a `RefCell<FxHashSet>`
    // and Go `collections.SyncMap` of configs is a `RefCell<FxHashMap>`.
    #[allow(clippy::too_many_arguments)]
    pub fn find_or_create_default_configured_project_worker(
        self: &Rc<Self>,
        file_name: &str,
        path: &tspath::Path,
        config_file_name: &str,
        load_kind: ProjectLoadKind,
        visited: Option<&RefCell<FxHashSet<SearchNodeKey>>>,
        fallback: Option<SearchResult>,
        logger: Option<Rc<logging::LogTree>>,
    ) -> SearchResult {
        let mut fallback = fallback;
        let configs: RefCell<FxHashMap<tspath::Path, Rc<tsoptions::ParsedCommandLine>>> =
            RefCell::new(FxHashMap::default());
        let new_visited: RefCell<FxHashSet<SearchNodeKey>>;
        let visited = match visited {
            Some(visited) => visited,
            None => {
                new_visited = RefCell::new(FxHashSet::default());
                &new_visited
            }
        };

        let search = core_bfs::breadth_first_search_parallel_ex(
            SearchNode {
                config_file_name: config_file_name.to_string(),
                load_kind,
                logger: logger.clone(),
            },
            &mut |node: &SearchNode| -> Vec<SearchNode> {
                let config = configs
                    .borrow()
                    .get(&(self.to_path)(&node.config_file_name))
                    .cloned();
                if let Some(config) = config {
                    if !config.project_references().is_empty() {
                        let mut reference_load_kind = node.load_kind;
                        if config
                            .compiler_options()
                            .disable_referenced_project_load
                            .is_true()
                        {
                            reference_load_kind = ProjectLoadKind::FIND;
                        }

                        let mut ref_logger: Option<Rc<logging::LogTree>> = None;
                        let references = config.resolved_project_reference_paths();
                        if !references.is_empty() && node.logger.is_some() {
                            ref_logger = node.logger.fork(&format!(
                                "Searching {} project references of {}",
                                references.len(),
                                node.config_file_name
                            ));
                        }
                        return references
                            .iter()
                            .map(|config_file_name| SearchNode {
                                config_file_name: config_file_name.clone(),
                                load_kind: reference_load_kind,
                                logger: ref_logger.fork(&format!(
                                    "Searching project reference {}",
                                    config_file_name
                                )),
                            })
                            .collect();
                    }
                }
                Vec::new()
            },
            &mut |node: &SearchNode| -> (bool, bool) {
                let config_file_path = (self.to_path)(&node.config_file_name);
                let config = self
                    .config_file_registry_builder
                    .find_or_acquire_config_for_file(
                        &node.config_file_name,
                        &config_file_path,
                        path,
                        node.load_kind,
                        node.logger.fork("Acquiring config for open file"),
                    );
                let Some(config) = config else {
                    node.logger
                        .log("Config file for project does not already exist");
                    return (false, false);
                };
                configs
                    .borrow_mut()
                    .insert(config_file_path.clone(), config.clone());
                if config.file_names().is_empty() {
                    // Likely a solution tsconfig.json - the search will fan out to its references.
                    node.logger
                        .log("Project does not contain file (no root files)");
                    return (false, false);
                }

                if config.compiler_options().composite == Tristate::True {
                    // For composite projects, we can get an early negative result.
                    // !!! what about declaration files in node_modules? wouldn't it be better to
                    //     check project inclusion if the project is already loaded?
                    if !config.file_names_by_path().contains_key(path) {
                        node.logger
                            .log("Project does not contain file (by composite config inclusion)");
                        return (false, false);
                    }
                }

                let Some(project) = self.find_or_create_project(
                    &node.config_file_name,
                    &config_file_path,
                    node.load_kind,
                    node.logger.clone(),
                ) else {
                    node.logger.log("Project does not already exist");
                    return (false, false);
                };

                if node.load_kind == ProjectLoadKind::CREATE {
                    // Ensure project is up to date before checking for file inclusion
                    self.update_program(&*project, node.logger.clone());
                }

                let value = project
                    .value()
                    .unwrap_or_else(|| crate::core::go_nil_dereference());
                if value.borrow().contains_file(path) {
                    let is_direct_inclusion =
                        !value.borrow().is_source_from_project_reference(path);
                    if node.logger.is_some() {
                        node.logger.logf(&format!(
                            "Project contains file {}",
                            if is_direct_inclusion {
                                "directly"
                            } else {
                                "as a source of a referenced project"
                            }
                        ));
                    }
                    return (true, is_direct_inclusion);
                }

                node.logger.log("Project does not contain file");
                (false, false)
            },
            core_bfs::BreadthFirstSearchOptions {
                visited: Some(visited),
                preprocess_level: Some(&mut |level: &core_bfs::BreadthFirstSearchLevel<
                    SearchNodeKey,
                    SearchNode,
                >| {
                    level.range(&mut |node: &SearchNode| -> bool {
                        if node.load_kind == ProjectLoadKind::FIND
                            && level.has(&SearchNodeKey {
                                config_file_name: node.config_file_name.clone(),
                                load_kind: ProjectLoadKind::CREATE,
                            })
                        {
                            // Remove find requests when a create request for the same project is already present.
                            level.delete(&SearchNodeKey {
                                config_file_name: node.config_file_name.clone(),
                                load_kind: node.load_kind,
                            });
                        }
                        true
                    });
                }),
            },
            &mut |node: &SearchNode| -> SearchNodeKey {
                SearchNodeKey {
                    config_file_name: node.config_file_name.clone(),
                    load_kind: node.load_kind,
                }
            },
        );

        let mut retain: FxHashSet<tspath::Path> = FxHashSet::default();
        let mut project: Option<
            Rc<dirty::SyncMapEntry<ConfiguredProjectID, Rc<RefCell<Project>>>>,
        > = None;
        if !search.path.is_empty() {
            project = self
                .configured_projects
                .load(&ConfiguredProjectID((self.to_path)(
                    &search.path[0].config_file_name,
                )))
                .0;
            // If we found a project, we retain each project along the BFS path.
            // We don't want to retain everything we visited since BFS can terminate
            // early, and we don't want to retain nondeterministically.
            for node in &search.path {
                retain.insert((self.to_path)(&node.config_file_name));
            }
        }

        if search.stopped {
            // Found a project that directly contains the file.
            return SearchResult { project, retain };
        }

        if project.is_some() {
            // If we found a project that contains the file, but it is a source from
            // a project reference, record it as a fallback.
            fallback = Some(SearchResult {
                project: project.clone(),
                retain: retain.clone(),
            });
        }

        // Look for tsconfig.json files higher up the directory tree and do the same. This handles
        // the common case where a higher-level "solution" tsconfig.json contains all projects in a
        // workspace.
        let disable_solution_searching = matches!(
            configs.borrow().get(&(self.to_path)(config_file_name)),
            Some(config) if config.compiler_options().disable_solution_searching.is_true()
        );
        if disable_solution_searching {
            if let Some(fallback) = fallback {
                return fallback;
            }
        }
        let ancestor_config_name = self
            .config_file_registry_builder
            .get_ancestor_config_file_name(file_name, path, config_file_name, logger.clone());
        if !ancestor_config_name.is_empty() {
            return self.find_or_create_default_configured_project_worker(
                file_name,
                path,
                &ancestor_config_name,
                load_kind,
                Some(visited),
                fallback,
                logger.fork(&format!(
                    "Searching ancestor config file at {}",
                    ancestor_config_name
                )),
            );
        }
        if let Some(fallback) = fallback {
            return fallback;
        }
        // If we didn't find anything, we can retain everything we visited,
        // since the whole graph must have been traversed (i.e., the set of
        // retained projects is guaranteed to be deterministic).
        for node in visited.borrow().iter() {
            retain.insert((self.to_path)(&node.config_file_name));
        }
        SearchResult {
            project: None,
            retain,
        }
    }

    // Go: project/projectcollectionbuilder.go:1216 findOrCreateDefaultConfiguredProjectForFile
    pub fn find_or_create_default_configured_project_for_file(
        self: &Rc<Self>,
        file_name: &str,
        path: &tspath::Path,
        load_kind: ProjectLoadKind,
        logger: Option<Rc<logging::LogTree>>,
    ) -> SearchResult {
        let key = self.file_default_projects.borrow().get(path).cloned();
        if let Some(key) = key {
            if key.inferred().1 {
                // The file belongs to the inferred project
                return SearchResult::default();
            }
            let (configured_id, _) = key.configured();
            let (entry, _) = self.configured_projects.load(&configured_id);
            return SearchResult {
                project: entry,
                retain: FxHashSet::default(),
            };
        }
        let config_file_name = self
            .config_file_registry_builder
            .get_config_file_name_for_file(file_name, path, logger.clone());
        if !config_file_name.is_empty() {
            let start_time = Instant::now();
            let result = self.find_or_create_default_configured_project_worker(
                file_name,
                path,
                &config_file_name,
                load_kind,
                None,
                None,
                logger.fork(&format!(
                    "Searching for default configured project for {}",
                    file_name
                )),
            );
            if let Some(project) = &result.project {
                // Go: `if b.fileDefaultProjects == nil { make(...) }` (the port map always exists).
                let project_id = project
                    .value()
                    .unwrap_or_else(|| crate::core::go_nil_dereference())
                    .borrow()
                    .id();
                self.file_default_projects
                    .borrow_mut()
                    .insert(path.clone(), project_id);
            }
            if logger.is_some() {
                let elapsed = start_time.elapsed();
                if let Some(project) = &result.project {
                    logger.log(&format!(
                        "Found default configured project for {}: {} (in {:?})",
                        file_name,
                        project
                            .value()
                            .unwrap_or_else(|| crate::core::go_nil_dereference())
                            .borrow()
                            .config_file_name(),
                        elapsed
                    ));
                } else {
                    logger.log(&format!(
                        "No default configured project found for {} (searched in {:?})",
                        file_name, elapsed
                    ));
                }
            }
            return result;
        }
        SearchResult::default()
    }

    // Go: project/projectcollectionbuilder.go:1261 findOrCreateProject
    pub fn find_or_create_project(
        self: &Rc<Self>,
        config_file_name: &str,
        config_file_path: &tspath::Path,
        load_kind: ProjectLoadKind,
        logger: Option<Rc<logging::LogTree>>,
    ) -> Option<Rc<dirty::SyncMapEntry<ConfiguredProjectID, Rc<RefCell<Project>>>>> {
        if load_kind == ProjectLoadKind::FIND {
            let (entry, _) = self
                .configured_projects
                .load(&ConfiguredProjectID(config_file_path.clone()));
            return entry;
        }
        // Go evaluates NewConfiguredProject before LoadOrStore, also when the
        // project exists (it logs and takes a watcher id).
        let (entry, _) = self.configured_projects.load_or_store(
            ConfiguredProjectID(config_file_path.clone()),
            new_configured_project(config_file_name, config_file_path, self, logger),
        );
        entry
    }

    // Go: project/projectcollectionbuilder.go:1275 updateInferredProjectRoots
    // PORT: Go reads `CommandLine.Errors` of an inferred or synthetic
    // project here, in `update_or_create_synthetic_project`, in
    // `update_or_create_inferred_project` and in snapshot.rs
    // `Snapshot::clone_body`. The port form of Go `Errors` is
    // `errors_with_common_source_directory_errors`, and these readers read
    // the plain `errors`, which are the same on these command lines:
    // - Go adds TS6059 to `Errors` only in `checkSourceFilesBelongToPath`
    //   (tsoptions/parsedcommandline.go:181). Only
    //   `(*ParsedCommandLine).CommonSourceDirectory` (:157) calls it. Its
    //   callers:
    //   - checker.go:15547, on `redirect`, the command line of a project
    //     reference (compiler/projectreferencefilemapper.go:90);
    //   - outputpaths (outputpaths.go:141, :167, :191), with the command
    //     line as its `OutputPathsHost` (parsedcommandline.go:117), from
    //     `getOutputDeclarationAndSourceFileNames` (:197) and
    //     `GetOutputFileNames` (:211). Only `ParseInputOutputNames` (:135)
    //     calls the first, on a reference
    //     (compiler/projectreferenceparser.go:28, ls/autoimport/util.go:189).
    //     Only tsc -b calls the second, on the command line of a config
    //     file (execute/build/buildtask.go:290, :520, :782, :797, :824,
    //     orchestrator.go:398).
    // - An inferred or synthetic command line comes from
    //   `newInferredProjectCommandLine` (project/project.go:258), not from a
    //   config file, so it is never a reference of a program. Its `Errors`
    //   hold only the `configFileParsingDiagnostics` that this builder sets.
    // - In the port, only the command line of a reference gets the flag
    //   that records the errors (program/go_frontend.rs
    //   `ProjectReferenceCopies`).
    // PORT: Go filters into a new slice; the port takes the `Vec` by value.
    pub fn update_inferred_project_roots(
        self: &Rc<Self>,
        root_file_names: Vec<String>,
        logger: Option<Rc<logging::LogTree>>,
    ) -> bool {
        let root_file_names: Vec<String> = root_file_names
            .into_iter()
            .filter(|file_name| self.is_supported_in_inferred_project(file_name))
            .collect();
        let mut project_references: Option<Vec<ProjectReference>> = None;
        let mut config_file_parsing_diagnostics: Vec<Diagnostic> = Vec::new();
        if let Some(project) = self.inferred_project.value() {
            let project = project.borrow();
            let command_line = project
                .command_line
                .as_ref()
                .unwrap_or_else(|| crate::core::go_nil_dereference());
            // PORT: Go `CommandLine.ProjectReferences()` keeps nil apart
            // from an empty list; that is the `ParsedOptions` field here.
            project_references = command_line.parsed_config.project_references.clone();
            // PORT: Go `CommandLine.Errors`; the plain `errors` (see the
            // PORT note above).
            config_file_parsing_diagnostics = command_line.errors.clone();
        }
        self.update_inferred_project(
            root_file_names,
            self.compiler_options_for_inferred_projects.clone(),
            project_references,
            config_file_parsing_diagnostics,
            self.inferred_content_mappers.clone(),
            logger,
        )
    }

    // Go: project/projectcollectionbuilder.go:1286 updateOrCreateSyntheticProject (ts#64204)
    #[allow(clippy::too_many_arguments)]
    // ts#64319: keyed by SyntheticProjectID. ts#64299: the module resolver
    // factory and its ID.
    pub fn update_or_create_synthetic_project(
        self: &Rc<Self>,
        project_id: SyntheticProjectID,
        root_file_names: Vec<String>,
        compiler_options: Option<Rc<CompilerOptions>>,
        project_references: Option<Vec<ProjectReference>>,
        config_file_parsing_diagnostics: Vec<Diagnostic>,
        module_resolver_factory: Option<Rc<dyn ModuleResolverFactory>>,
        module_resolver_id: u64,
        content_mappers: Vec<Rc<contentmapper::Mapper>>,
        logger: Option<Rc<logging::LogTree>>,
    ) -> Rc<dirty::SyncMapEntry<SyntheticProjectID, Rc<RefCell<Project>>>> {
        let (project, loaded) = self.synthetic_projects.load(&project_id);
        let Some(project) = project.filter(|_| loaded) else {
            let synthetic_project = new_synthetic_project(
                project_id.clone(),
                &self.session_options.current_directory,
                compiler_options,
                root_file_names,
                project_references,
                &content_mappers,
                self,
                logger,
            );
            {
                let mut p = synthetic_project.borrow_mut();
                // Go: syntheticProject.CommandLine.Errors = configFileParsingDiagnostics
                Rc::get_mut(
                    p.command_line
                        .as_mut()
                        .unwrap_or_else(|| crate::core::go_nil_dereference()),
                )
                .expect("the new command line is not shared yet")
                .errors = config_file_parsing_diagnostics;
                // ts#64299
                p.module_resolver_factory = module_resolver_factory;
                p.module_resolver_id = module_resolver_id;
            }
            let (project, _) = self
                .synthetic_projects
                .load_or_store(project_id, synthetic_project);
            return project.unwrap_or_else(|| crate::core::go_nil_dereference());
        };

        let (compiler_options, current_directory) = {
            let current_project = project
                .value()
                .unwrap_or_else(|| crate::core::go_nil_dereference());
            let current_project = current_project.borrow();
            // Go `CompilerOptions` is nil-safe: a nil command line gives nil.
            // PORT: the port command line cannot hold nil options. A
            // synthetic project always has a command line, so this is a port
            // assert, not a Go panic.
            let compiler_options = match compiler_options {
                Some(compiler_options) => compiler_options,
                None => current_project
                    .command_line
                    .as_ref()
                    .map(|command_line| command_line.compiler_options().clone())
                    .expect("port: a synthetic project has a command line"),
            };
            (compiler_options, current_project.current_directory.clone())
        };
        let mut new_command_line = new_inferred_project_command_line(
            compiler_options.clone(),
            root_file_names.clone(),
            project_references.clone(),
            &content_mappers,
            tspath::ComparePathsOptions {
                use_case_sensitive_file_names: self.fs.fs.use_case_sensitive_file_names(),
                current_directory,
            },
        );
        new_command_line.errors = config_file_parsing_diagnostics.clone();
        let new_command_line = Rc::new(new_command_line);
        project.change_if(
            &mut |p: Option<&Rc<RefCell<Project>>>| -> bool {
                let p = p.unwrap_or_else(|| crate::core::go_nil_dereference()).borrow();
                let command_line = p.command_line.as_ref().unwrap_or_else(|| crate::core::go_nil_dereference());
                command_line.file_names() != new_command_line.file_names()
                    // Go: !p.CommandLine.CompilerOptions().Equals(compilerOptions) (ts#64457,
                    // projectcollectionbuilder.go:1338 at fed0bf24149f; it was
                    // reflect.DeepEqual). PORT: the derived `PartialEq` compares every
                    // field, as the generated Equals does.
                    || **command_line.compiler_options() != *compiler_options
                    || !project_references_equal(
                        command_line.project_references(),
                        project_references.as_deref().unwrap_or_default(),
                    )
                    // PORT: Go `p.CommandLine.Errors`; the plain `errors`
                    // (see `update_inferred_project_roots`).
                    || !diagnostics_deep_equal(&command_line.errors, &config_file_parsing_diagnostics)
                    || !mappers_equal(command_line.content_mappers(), new_command_line.content_mappers())
                    // ts#64299
                    || p.module_resolver_id != module_resolver_id
            },
            &mut |p: &Rc<RefCell<Project>>| {
                if logger.is_some() {
                    logger.log(&format!(
                        "Updating synthetic project config with {} root files",
                        root_file_names.len()
                    ));
                }
                let mut p = p.borrow_mut();
                p.set_command_line(Some(new_command_line.clone()));
                // ts#64299
                p.module_resolver_factory = module_resolver_factory.clone();
                p.module_resolver_id = module_resolver_id;
            },
        );
        project
    }

    // Go: project/projectcollectionbuilder.go:1338 updateInferredProject (ts#63950)
    // updateInferredProject preserves the current command line when roots/options are unchanged.
    #[allow(clippy::too_many_arguments)]
    pub fn update_inferred_project(
        self: &Rc<Self>,
        root_file_names: Vec<String>,
        compiler_options: Option<Rc<CompilerOptions>>,
        project_references: Option<Vec<ProjectReference>>,
        config_file_parsing_diagnostics: Vec<Diagnostic>,
        content_mappers: Vec<Rc<contentmapper::Mapper>>,
        logger: Option<Rc<logging::LogTree>>,
    ) -> bool {
        if root_file_names.is_empty() {
            return self.delete_inferred_project(logger);
        }
        // Go: slices.Clone, then slices.Sort (the port owns the Vec).
        let mut root_file_names = root_file_names;
        root_file_names.sort();
        self.update_or_create_inferred_project(
            root_file_names,
            compiler_options,
            project_references,
            config_file_parsing_diagnostics,
            content_mappers,
            logger,
        )
    }

    // Go: project/projectcollectionbuilder.go:1354 deleteInferredProject (ts#64204)
    pub fn delete_inferred_project(self: &Rc<Self>, logger: Option<Rc<logging::LogTree>>) -> bool {
        let Some(project) = self.inferred_project.value() else {
            return false;
        };
        if logger.is_some() {
            logger.log("Deleting inferred project");
        }
        let (program, project_id) = {
            let project = project.borrow();
            (project.program.clone(), project.id())
        };
        if let Some(program) = program {
            program.range_resolved_project_reference(
                |reference_path: &tspath::Path, _, _, _| -> bool {
                    self.config_file_registry_builder
                        .release_config_for_project(reference_path, &project_id);
                    true
                },
            );
        }
        self.inferred_project.delete();
        true
    }

    // Go: project/projectcollectionbuilder.go:1374 updateOrCreateInferredProject (ts#63950)
    // updateOrCreateInferredProject always retains an inferred project, including when rootFileNames is empty.
    // The caller transfers ownership of rootFileNames.
    #[allow(clippy::too_many_arguments)]
    pub fn update_or_create_inferred_project(
        self: &Rc<Self>,
        root_file_names: Vec<String>,
        compiler_options: Option<Rc<CompilerOptions>>,
        project_references: Option<Vec<ProjectReference>>,
        config_file_parsing_diagnostics: Vec<Diagnostic>,
        content_mappers: Vec<Rc<contentmapper::Mapper>>,
        logger: Option<Rc<logging::LogTree>>,
    ) -> bool {
        let Some(project) = self.inferred_project.value() else {
            let project = new_inferred_project(
                &self.session_options.current_directory,
                compiler_options,
                &root_file_names,
                project_references,
                &content_mappers,
                self,
                logger,
            );
            {
                let mut p = project.borrow_mut();
                // Go: project.CommandLine.Errors = configFileParsingDiagnostics
                Rc::get_mut(
                    p.command_line
                        .as_mut()
                        .unwrap_or_else(|| crate::core::go_nil_dereference()),
                )
                .expect("the new command line is not shared yet")
                .errors = config_file_parsing_diagnostics;
            }
            self.inferred_project.set(project);
            return true;
        };

        let (compiler_options, current_directory) = {
            let project = project.borrow();
            // Go `CompilerOptions` is nil-safe: a nil command line gives nil.
            // PORT: the port command line cannot hold nil options. An
            // inferred project always has a command line, so this is a port
            // assert, not a Go panic.
            let compiler_options = match compiler_options {
                Some(compiler_options) => compiler_options,
                None => project
                    .command_line
                    .as_ref()
                    .map(|command_line| command_line.compiler_options().clone())
                    .expect("port: an inferred project has a command line"),
            };
            (compiler_options, project.current_directory.clone())
        };
        let mut new_command_line = new_inferred_project_command_line(
            compiler_options.clone(),
            root_file_names.clone(),
            project_references.clone(),
            &content_mappers,
            tspath::ComparePathsOptions {
                use_case_sensitive_file_names: self.fs.fs.use_case_sensitive_file_names(),
                current_directory,
            },
        );
        new_command_line.errors = config_file_parsing_diagnostics.clone();
        let new_command_line = Rc::new(new_command_line);
        let changed = self.inferred_project.change_if(
            &mut |p: Option<&Rc<RefCell<Project>>>| -> bool {
                let p = p.unwrap_or_else(|| crate::core::go_nil_dereference()).borrow();
                let command_line = p.command_line.as_ref().unwrap_or_else(|| crate::core::go_nil_dereference());
                command_line.file_names() != new_command_line.file_names()
                    // Go: !p.CommandLine.CompilerOptions().Equals(compilerOptions) (ts#64457,
                    // projectcollectionbuilder.go:1424 at fed0bf24149f; it was
                    // reflect.DeepEqual). PORT: the derived `PartialEq` compares every
                    // field, as the generated Equals does.
                    || **command_line.compiler_options() != *compiler_options
                    || !project_references_equal(
                        command_line.project_references(),
                        project_references.as_deref().unwrap_or_default(),
                    )
                    // PORT: Go `p.CommandLine.Errors`; the plain `errors`
                    // (see `update_inferred_project_roots`).
                    || !diagnostics_deep_equal(&command_line.errors, &config_file_parsing_diagnostics)
                    // PORT: Go `slices.Equal` on `[]*contentmapper.Mapper`
                    // compares the pointers.
                    || !mappers_equal(command_line.content_mappers(), new_command_line.content_mappers())
            },
            &mut |p: &Rc<RefCell<Project>>| {
                if logger.is_some() {
                    logger.log(&format!(
                        "Updating inferred project config with {} root files",
                        root_file_names.len()
                    ));
                }
                p.borrow_mut()
                    .set_command_line(Some(new_command_line.clone()));
            },
        );
        if !changed {
            return false;
        }
        true
    }

    // Go: project/projectcollectionbuilder.go:1428 isSupportedInInferredProject (tsgo#4712)
    pub fn is_supported_in_inferred_project(&self, file_name: &str) -> bool {
        if tspath::is_dynamic_file_name(file_name)
            || get_script_kind_from_file_name(file_name) != ScriptKind::UNKNOWN
        {
            return true;
        }
        if let Some(file) = self.fs.get_file(file_name)
            && file.is_overlay()
            && tspath::get_any_extension_from_path(file_name, &[], false).is_empty()
        {
            return true;
        }
        let extensions: Vec<&str> = self
            .inferred_content_mapper_extensions
            .iter()
            .map(String::as_str)
            .collect();
        tspath::file_extension_is_one_of(file_name, &extensions)
    }

    // Go: project/projectcollectionbuilder.go:1440 updateProgram
    // updateProgram updates the program for the given project entry if necessary. It returns
    // a boolean indicating whether the update could have caused any structure-affecting changes.
    pub fn update_program(
        self: &Rc<Self>,
        entry: &dyn dirty::Value<Rc<RefCell<Project>>>,
        logger: Option<Rc<logging::LogTree>>,
    ) -> bool {
        let mut update_program = false;
        let mut delete_project = false;
        let mut files_changed = false;
        // ts#64319
        let project_id = entry
            .value()
            .unwrap_or_else(|| crate::core::go_nil_dereference())
            .borrow()
            .id();
        let start_time = Instant::now();
        let mut notified_loading = false;
        let mut display_name = String::new();
        entry.locked(&mut |entry: &dyn dirty::Value<Rc<RefCell<Project>>>| {
            let value = entry.value().unwrap_or_else(|| crate::core::go_nil_dereference());
            if value.borrow().kind == Kind::CONFIGURED {
                let (value_config_file_name, value_config_file_path) = {
                    let value = value.borrow();
                    (value.config_file_name(), value.config_file_path.clone())
                };
                let command_line = self
                    .config_file_registry_builder
                    .acquire_config_for_project(
                        &value_config_file_name,
                        &value_config_file_path,
                        &value,
                        logger.fork("Acquiring config for project"),
                    );
                let Some(command_line) = command_line else {
                    delete_project = true;
                    files_changed = true;
                    return;
                };
                // Go: pointer compare `entry.Value().CommandLine != commandLine`.
                let same_command_line = matches!(
                    &entry.value().unwrap_or_else(|| crate::core::go_nil_dereference()).borrow().command_line,
                    Some(current) if Rc::ptr_eq(current, &command_line)
                );
                if !same_command_line {
                    update_program = true;
                    entry.change(&mut |p: &Rc<RefCell<Project>>| {
                        p.borrow_mut().set_command_line(Some(command_line.clone()));
                    });
                }
            }
            if !update_program {
                update_program = entry.value().unwrap_or_else(|| crate::core::go_nil_dereference()).borrow().dirty;
            }
            if update_program && self.client.is_some() {
                display_name = entry
                    .value()
                    .unwrap_or_else(|| crate::core::go_nil_dereference())
                    .borrow()
                    .display_name(&self.session_options.current_directory);
                notified_loading = true;
            }
        });
        if notified_loading {
            if let Some(client) = &self.client {
                client.progress_start(diag::Project_0, args![display_name]);
            }
        }
        if delete_project {
            self.delete_project(entry, logger.clone());
        }
        if update_program {
            entry.locked(&mut |entry: &dyn dirty::Value<Rc<RefCell<Project>>>| {
                entry.change(&mut |project: &Rc<RefCell<Project>>| {
                    let (old_host, old_program, old_checker_pool, current_directory) = {
                        let p = project.borrow();
                        (
                            p.host.clone(),
                            p.program.clone(),
                            p.checker_pool.clone(),
                            p.current_directory.clone(),
                        )
                    };
                    let host = new_compiler_host(
                        &current_directory,
                        project,
                        self,
                        logger.fork("CompilerHost"),
                    );
                    project.borrow_mut().host = Some(host);
                    // PORT: no mutable borrow is held while the program is
                    // built (the compiler host reads the project).
                    let result = project.borrow().create_program();
                    // tsgo#4712
                    let (new_host, content_mappers) = {
                        let p = project.borrow();
                        (
                            p.host
                                .clone()
                                .unwrap_or_else(|| crate::core::go_nil_dereference()),
                            // Go `ContentMappers` is nil-safe: a nil command
                            // line has none.
                            p.command_line
                                .as_ref()
                                .map_or_else(Vec::new, |c| c.content_mappers().to_vec()),
                        )
                    };
                    let mut watched_files: Vec<String> = Vec::new();
                    for mapper in &content_mappers {
                        if !mapper.definition.package.is_empty()
                            && mapper.contribution_id.is_empty()
                            && !mapper.package_directory.is_empty()
                        {
                            watched_files.push(tspath::combine_paths(
                                &mapper.package_directory,
                                &["package.json"],
                            ));
                        }
                    }
                    if result
                        .program
                        .source_files()
                        .iter()
                        .any(|file| !file.content_mapper().is_empty())
                    {
                        // ts#64221
                        if let Some(content_mapper_project) =
                            compiler::CompilerHost::content_mapper_project(&*new_host)
                        {
                            let dynamic_watched_files =
                                content_mapper_project.watched_files().unwrap_or_default();
                            watched_files.extend(dynamic_watched_files);
                        }
                    }
                    watched_files.sort();
                    watched_files.dedup();
                    let mut content_mapper_watched_files: FxHashSet<tspath::Path> =
                        FxHashSet::with_capacity_and_hasher(
                            watched_files.len(),
                            Default::default(),
                        );
                    for file_name in &watched_files {
                        content_mapper_watched_files.insert((self.to_path)(file_name));
                    }
                    // PORT: Go `result.Program.GetCheckerPool().(*checkerPool)`.
                    // `ls_program::CheckerPool` has no `Any` view, so
                    // CreateProgram returns the pool its CreateCheckerPool
                    // closure made for this program (project.rs). `None` is
                    // the failed Go type assertion.
                    let checker_pool = result.checker_pool.clone().unwrap_or_else(|| {
                        panic!(
                            "interface conversion: compiler.CheckerPool is not *project.checkerPool"
                        )
                    });
                    let mut p = project.borrow_mut();
                    let content_mapper_watch =
                        WatchedFiles::clone_(p.content_mapper_watch.as_deref(), watched_files);
                    p.content_mapper_watch = content_mapper_watch;
                    p.content_mapper_watched_files = Some(Rc::new(content_mapper_watched_files));
                    p.program = Some(Rc::clone(&result.program));
                    p.program_file_refs = Some(Rc::clone(&result.file_refs));
                    self.made_programs.borrow_mut().push(MadeProgram {
                        program: Rc::clone(&result.program),
                        host: Rc::clone(
                            p.host
                                .as_ref()
                                .unwrap_or_else(|| crate::core::go_nil_dereference()),
                        ),
                        checker_pool: Rc::clone(&checker_pool),
                    });
                    p.checker_pool = Some(checker_pool);
                    p.program_update_kind = result.update_kind;
                    p.program_last_update = self.new_snapshot_id;
                    if result.update_kind == ProgramUpdateKind::CLONED {
                        let seen_files = old_host
                            .as_ref()
                            .unwrap_or_else(|| crate::core::go_nil_dereference())
                            .source_fs
                            .seen_files
                            .borrow()
                            .clone();
                        *p.host
                            .as_ref()
                            .unwrap_or_else(|| crate::core::go_nil_dereference())
                            .source_fs
                            .seen_files
                            .borrow_mut() = seen_files;
                    }
                    if result.update_kind == ProgramUpdateKind::NEW_FILES {
                        files_changed = true;
                        let program_files_watch = p.clone_watchers();
                        p.program_files_watch = program_files_watch;
                    }
                    p.dirty = false;
                    p.dirty_file_path = tspath::Path::default();
                    let project_id = p.id();
                    drop(p);
                    self.release_dropped_project_references(
                        old_program.as_deref(),
                        Some(&*result.program),
                        &project_id,
                    );
                    if let Some(old_checker_pool) = old_checker_pool {
                        old_checker_pool.discard();
                    }
                });
            });
        }
        if notified_loading {
            if let Some(client) = &self.client {
                client.progress_finish(diag::Project_0, args![display_name]);
            }
        }
        if update_program && logger.is_some() {
            let elapsed = start_time.elapsed();
            logger.log(&format!(
                "Program update for {} completed in {:?}",
                project_id, elapsed
            ));
        }
        files_changed
    }

    // Go: project/projectcollectionbuilder.go:1543 markFilesChanged
    // PORT: the two Go closures share `dirty` and `dirtyFilePath`, so they
    // are a `Cell` and a `RefCell`.
    pub fn mark_files_changed(
        self: &Rc<Self>,
        entry: &dyn dirty::Value<Rc<RefCell<Project>>>,
        paths: &[tspath::Path],
        change_type: lsproto::FileChangeType,
        logger: Option<Rc<logging::LogTree>>,
    ) {
        let dirty = Cell::new(false);
        let dirty_file_path: RefCell<tspath::Path> = RefCell::new(tspath::Path::default());
        entry.change_if(
            &mut |p: Option<&Rc<RefCell<Project>>>| -> bool {
                let p = p
                    .unwrap_or_else(|| crate::core::go_nil_dereference())
                    .borrow();
                if p.program.is_none() || p.dirty && p.dirty_file_path.is_empty() {
                    return false;
                }

                *dirty_file_path.borrow_mut() = p.dirty_file_path.clone();
                for path in paths {
                    if p.contains_file(path) {
                        dirty.set(true);
                        if change_type == lsproto::FileChangeType::DELETED {
                            *dirty_file_path.borrow_mut() = tspath::Path::default();
                            break;
                        }
                        // package.json changes can affect module resolution and package
                        // identity (e.g. dedup decisions), so they must always trigger
                        // a full rebuild rather than a single-file clone.
                        if tspath::get_base_file_name(path) == "package.json" {
                            *dirty_file_path.borrow_mut() = tspath::Path::default();
                            break;
                        }
                        let current = dirty_file_path.borrow().clone();
                        if current.is_empty() {
                            *dirty_file_path.borrow_mut() = path.clone();
                        } else if current != *path {
                            *dirty_file_path.borrow_mut() = tspath::Path::default();
                            break;
                        }
                    } else if let Some(host) = &p.host {
                        if change_type == lsproto::FileChangeType::CREATED
                            && host.source_fs.seen_file_or_missing_parent_directory(path)
                            || change_type != lsproto::FileChangeType::CREATED
                                && host.source_fs.seen_file(path)
                        {
                            dirty.set(true);
                            *dirty_file_path.borrow_mut() = tspath::Path::default();
                            break;
                        }
                    }
                }
                dirty.get() || p.dirty_file_path != *dirty_file_path.borrow()
            },
            &mut |p: &Rc<RefCell<Project>>| {
                let mut p = p.borrow_mut();
                p.dirty = true;
                p.dirty_file_path = dirty_file_path.borrow().clone();
                if logger.is_some() {
                    let dirty_file_path = dirty_file_path.borrow();
                    if !dirty_file_path.is_empty() {
                        logger.logf(&format!(
                            "Marking project {} as dirty due to changes in {}",
                            p.id(),
                            dirty_file_path
                        ));
                    } else {
                        logger.logf(&format!("Marking project {} as dirty", p.id()));
                    }
                }
            },
        );
    }

    // Go: project/projectcollectionbuilder.go:1597 deleteProject (ts#64204: was deleteConfiguredProject)
    pub fn delete_project(
        self: &Rc<Self>,
        project: &dyn dirty::Value<Rc<RefCell<Project>>>,
        logger: Option<Rc<logging::LogTree>>,
    ) {
        // ts#64319: configs release by project ID.
        let (project_id, kind, program, config_file_path) = {
            let value = project
                .value()
                .unwrap_or_else(|| crate::core::go_nil_dereference());
            let value = value.borrow();
            (
                value.id(),
                value.kind,
                value.program.clone(),
                value.config_file_path.clone(),
            )
        };
        if logger.is_some() {
            logger.logf(&format!(
                "Deleting {} project: {}",
                kind.string(),
                project_id
            ));
        }
        if let Some(program) = program {
            program.range_resolved_project_reference(
                |reference_path: &tspath::Path, _, _, _| -> bool {
                    self.config_file_registry_builder
                        .release_config_for_project(reference_path, &project_id);
                    true
                },
            );
        }
        if kind == Kind::CONFIGURED {
            // Go: value.ConfigFilePath()
            self.config_file_registry_builder
                .release_config_for_project(&config_file_path, &project_id);
        }
        project.delete();
    }

    // Go: project/projectcollectionbuilder.go:1619 releaseDroppedProjectReferences
    // releaseDroppedProjectReferences releases the config entries for project references
    // that were present in oldProgram but are no longer referenced by newProgram. Creating
    // newProgram already re-acquires the config for every reference it still resolves, so
    // only the dropped references need to be released here.
    pub fn release_dropped_project_references(
        &self,
        old_program: Option<&compiler::NewProgram>,
        new_program: Option<&compiler::NewProgram>,
        project_id: &ID,
    ) {
        let Some(old_program) = old_program else {
            return;
        };
        if new_program.is_some_and(|new_program| std::ptr::eq(old_program, new_program)) {
            return;
        }
        let mut new_references: FxHashSet<tspath::Path> = FxHashSet::default();
        if let Some(new_program) = new_program {
            new_program.range_resolved_project_reference(
                |reference_path: &tspath::Path, _, _, _| -> bool {
                    new_references.insert(reference_path.clone());
                    true
                },
            );
        }
        old_program.range_resolved_project_reference(
            |reference_path: &tspath::Path, _, _, _| -> bool {
                if !new_references.contains(reference_path) {
                    self.config_file_registry_builder
                        .release_config_for_project(reference_path, project_id);
                }
                true
            },
        );
    }
}

// Go: project/projectcollectionbuilder.go:1419 projectReferencesEqual (ts#63950)
// PORT: Go `[]*core.ProjectReference` elements are never nil here.
pub fn project_references_equal(a: &[ProjectReference], b: &[ProjectReference]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|(a, b)| a.path == b.path && a.circular == b.circular)
}

/// Go `slices.Equal` on `[]*contentmapper.Mapper` (pointer compare).
fn mappers_equal(a: &[Rc<contentmapper::Mapper>], b: &[Rc<contentmapper::Mapper>]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(a, b)| Rc::ptr_eq(a, b))
}

/// Go `reflect.DeepEqual` on two `[]*ast.Diagnostic` (ts#63950).
// PORT: Go also tells a nil slice from an empty one; the port's
// `ParsedCommandLine.errors` is a `Vec`, so both are empty. The source file
// and the message are compared by identity (Go compares the pointees, which
// are equal only for the same file and message in practice), and
// `repopulateInfo` by pointer.
pub fn diagnostics_deep_equal(a: &[Diagnostic], b: &[Diagnostic]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(a, b)| {
            a.file == b.file
                && a.pos == b.pos
                && a.end == b.end
                && a.code == b.code
                && a.category == b.category
                && a.source == b.source
                && std::ptr::eq(a.message, b.message)
                && a.message_text == b.message_text
                && a.message_args == b.message_args
                && diagnostics_deep_equal(&a.message_chain, &b.message_chain)
                && diagnostics_deep_equal(&a.related_information, &b.related_information)
                && a.reports_unnecessary == b.reports_unnecessary
                && a.reports_deprecated == b.reports_deprecated
                && a.skipped_on_no_emit == b.skipped_on_no_emit
                && match (&a.repopulate_info, &b.repopulate_info) {
                    (None, None) => true,
                    (Some(a), Some(b)) => std::sync::Arc::ptr_eq(a, b),
                    _ => false,
                }
        })
}

// Go: project/projectcollectionbuilder.go:271 isReferencedBy (closure in DidChangeFiles)
// PORT: Go defines this closure and never calls it (it only calls itself);
// it is kept for the literal port. Go `*collections.Set[*Project]` is a set
// of project addresses.
fn is_referenced_by(
    b: &Rc<ProjectCollectionBuilder>,
    project: &Rc<RefCell<Project>>,
    ref_path: &tspath::Path,
    seen_projects: &mut FxHashSet<usize>,
) -> bool {
    if !seen_projects.insert(Rc::as_ptr(project) as usize) {
        return false;
    }

    let project = project.borrow();
    if let Some(potential_project_references) = &project.potential_project_references {
        for potential_ref in potential_project_references.iter() {
            if potential_ref == ref_path {
                return true;
            }
        }
        for potential_ref in potential_project_references.iter() {
            if let (Some(ref_project), true) = b
                .configured_projects
                .load(&ConfiguredProjectID(potential_ref.clone()))
            {
                if is_referenced_by(
                    b,
                    &ref_project
                        .value()
                        .unwrap_or_else(|| crate::core::go_nil_dereference()),
                    ref_path,
                    seen_projects,
                ) {
                    return true;
                }
            }
        }
    } else if let Some(program) = &project.program {
        // PORT: Go `project.GetProgram()` is the field read.
        if !program.range_resolved_project_reference(
            |reference_path: &tspath::Path, _, _, _| -> bool { reference_path != ref_path },
        ) {
            return true;
        }
    }
    false
}

// Go: project/projectcollectionbuilder.go:581 logChangeFileResult
// PORT: Go passes the result by value; here by reference.
pub fn log_change_file_result(result: &ChangeFileResult, logger: &Option<Rc<logging::LogTree>>) {
    if !result.affected_projects.is_empty() {
        logger.logf(&format!(
            "Config file change affected projects: {:?}",
            result.affected_projects.iter().collect::<Vec<_>>()
        ));
    }
    if !result.affected_files.is_empty() {
        logger.logf(&format!(
            "Config file change affected config file lookups for {} files",
            result.affected_files.len()
        ));
    }
}

// Go: project/projectcollectionbuilder.go:1042 searchNode
#[derive(Clone)]
pub struct SearchNode {
    pub config_file_name: String,
    pub load_kind: ProjectLoadKind,
    pub logger: Option<Rc<logging::LogTree>>,
}

// Go: project/projectcollectionbuilder.go:1048 searchNodeKey
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SearchNodeKey {
    pub config_file_name: String,
    pub load_kind: ProjectLoadKind,
}

// Go: project/projectcollectionbuilder.go:1053 searchResult
#[derive(Clone, Default)]
pub struct SearchResult {
    pub project: Option<Rc<dirty::SyncMapEntry<ConfiguredProjectID, Rc<RefCell<Project>>>>>,
    pub retain: FxHashSet<tspath::Path>,
}
