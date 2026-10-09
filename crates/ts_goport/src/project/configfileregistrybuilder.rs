//! Go `internal/project/configfileregistrybuilder.go`.
//!
//! PORT: one thread (project/dirty/interfaces.rs). The builder is shared as
//! `Rc<ConfigFileRegistryBuilder>` (Go `*configFileRegistryBuilder`). Go
//! passes the builder itself as the `tsoptions.ParseConfigHost` and
//! `tsoptions.ExtendedConfigCache` of the extended config cache arguments,
//! which own their values, so the builder keeps a `Weak` to itself
//! (`this`). `dirty` values are `Rc<RefCell<..>>` handles; `change` and
//! `change_if` give the handle to `apply`, which changes it through
//! `borrow_mut`. Go `map[tspath.Path]struct{}` is `FxHashSet<tspath::Path>`;
//! a local Go map that can stay nil is an `Option`.

use crate::project::prelude::*;

use crate::frontend::core_ls_ext::copy_map_into;
use crate::frontend::vfs::Fs as _;
use std::cell::Cell;
use std::rc::Weak;

// Go: project/configfileregistrybuilder.go:18 interface assertions
// PORT: the `tsoptions::ParseConfigHost` and `tsoptions::ExtendedConfigCache`
// impls at the end of this file.

// Go: project/configfileregistrybuilder.go:27 configFileRegistryBuilder
// configFileRegistryBuilder tracks changes made on top of a previous
// configFileRegistry, producing a new clone with `finalize()` after
// all changes have been made.
pub struct ConfigFileRegistryBuilder {
    pub has_relative_pattern_capability: bool,
    pub fs: Rc<SourceFS>,
    pub is_open_file: Rc<dyn Fn(&tspath::Path) -> bool>,
    pub extended_config_cache: Rc<ExtendedConfigCache>,
    pub snapshot_id: u64,
    pub session_options: Rc<SessionOptions>,
    pub custom_config_file_name: String,

    pub base: Rc<ConfigFileRegistry>,
    pub configs: Rc<dirty::SyncMap<tspath::Path, Rc<RefCell<ConfigFileEntry>>>>,
    pub config_file_names: Rc<dirty::Map<tspath::Path, Rc<RefCell<ConfigFileNames>>>>,
    pub custom_config_file_name_changed: bool,
    // tsgo#4712. PORT: `contentMappersMu` is dropped (one thread); a Go nil
    // pointer is `None`.
    pub all_configured_content_mappers: RefCell<Option<Rc<ConfiguredContentMappers>>>,

    /// The Go pointer `c` (see the file comment).
    this: Weak<ConfigFileRegistryBuilder>,
}

// Go: project/configfileregistrybuilder.go:44 newConfigFileRegistryBuilder
// PORT: the unused Go `logger` parameter is kept. The base maps are copied
// into the dirty maps (project/dirty/interfaces.rs decision 3).
#[allow(clippy::too_many_arguments)]
pub fn new_config_file_registry_builder(
    has_relative_pattern_capability: bool,
    fs: Rc<SnapshotFSBuilder>,
    // ts#64291
    is_open_file: Rc<dyn Fn(&tspath::Path) -> bool>,
    old_config_file_registry: Rc<ConfigFileRegistry>,
    extended_config_cache: Rc<ExtendedConfigCache>,
    snapshot_id: u64,
    session_options: Rc<SessionOptions>,
    custom_config_file_name: &str,
    _logger: Option<Rc<logging::LogTree>>,
) -> Rc<ConfigFileRegistryBuilder> {
    let to_path = fs.to_path.clone();
    let custom_config_file_name_changed =
        custom_config_file_name != old_config_file_registry.custom_config_file_name;
    let all_configured_content_mappers = old_config_file_registry.content_mappers();
    let configs = dirty::new_sync_map(old_config_file_registry.configs.clone());
    let config_file_names = dirty::new_map(old_config_file_registry.config_file_names.clone());
    Rc::new_cyclic(|this| ConfigFileRegistryBuilder {
        has_relative_pattern_capability,
        fs: new_source_fs(false, fs, to_path),
        is_open_file,
        base: old_config_file_registry,
        session_options,
        extended_config_cache,
        snapshot_id,
        custom_config_file_name: custom_config_file_name.to_string(),
        custom_config_file_name_changed,
        all_configured_content_mappers: RefCell::new(Some(all_configured_content_mappers)),

        configs,
        config_file_names,
        this: this.clone(),
    })
}

impl ConfigFileRegistryBuilder {
    /// Go `c` as a shared pointer.
    fn this_rc(&self) -> Rc<ConfigFileRegistryBuilder> {
        self.this
            .upgrade()
            .expect("configFileRegistryBuilder: builder dropped")
    }

    // Go: project/configfileregistrybuilder.go:74 configFileRegistryBuilder.Finalize
    // Finalize creates a new configFileRegistry based on the changes made in the builder.
    // If no changes were made, it returns the original base registry.
    pub fn finalize(&self) -> Rc<ConfigFileRegistry> {
        let mut changed = false;
        let mut new_registry = self.base.clone();
        let mut ensure_cloned = |new_registry: &mut Rc<ConfigFileRegistry>| {
            if !changed {
                *new_registry = Rc::new(new_registry.clone_());
                changed = true;
            }
        };

        let (configs, changed_configs) = self.configs.finalize_exported();
        if changed_configs {
            ensure_cloned(&mut new_registry);
            let all_configured_content_mappers = self.content_mappers();
            let registry = Rc::get_mut(&mut new_registry).expect("a fresh clone has one owner");
            registry.configs = configs;
            registry.all_configured_content_mappers = Some(all_configured_content_mappers);
        }

        let (config_file_names, changed_names) = self.config_file_names.finalize();
        if changed_names {
            ensure_cloned(&mut new_registry);
            Rc::get_mut(&mut new_registry)
                .expect("a fresh clone has one owner")
                .config_file_names = config_file_names;
        }

        if self.custom_config_file_name_changed {
            ensure_cloned(&mut new_registry);
            Rc::get_mut(&mut new_registry)
                .expect("a fresh clone has one owner")
                .custom_config_file_name = self.custom_config_file_name.clone();
        }

        new_registry
    }

    // Go: project/configfileregistrybuilder.go:103 configFileRegistryBuilder.contentMappers (tsgo#4712)
    pub fn content_mappers(&self) -> Rc<ConfiguredContentMappers> {
        if self.all_configured_content_mappers.borrow().is_none() {
            let mut command_lines: Vec<Rc<tsoptions::ParsedCommandLine>> = Vec::new();
            self.configs.range(&mut |entry: &Rc<
                dirty::SyncMapEntry<tspath::Path, Rc<RefCell<ConfigFileEntry>>>,
            >| {
                let command_line = entry
                    .value()
                    .unwrap_or_else(|| crate::core::go_nil_dereference())
                    .borrow()
                    .command_line
                    .clone();
                if let Some(command_line) = command_line {
                    command_lines.push(command_line);
                }
                true
            });
            *self.all_configured_content_mappers.borrow_mut() = Some(
                collect_configured_content_mappers(command_lines.iter().map(|c| &**c)),
            );
        }
        self.all_configured_content_mappers
            .borrow()
            .clone()
            .expect("set above")
    }

    // Go: project/configfileregistrybuilder.go:119 configFileRegistryBuilder.invalidateContentMappers (tsgo#4712)
    pub fn invalidate_content_mappers(&self) {
        *self.all_configured_content_mappers.borrow_mut() = None;
    }

    // Go: project/configfileregistrybuilder.go:125 configFileRegistryBuilder.findOrAcquireConfigForFile
    pub fn find_or_acquire_config_for_file(
        &self,
        config_file_name: &str,
        config_file_path: &tspath::Path,
        file_path: &tspath::Path,
        load_kind: ProjectLoadKind,
        logger: Option<Rc<logging::LogTree>>,
    ) -> Option<Rc<tsoptions::ParsedCommandLine>> {
        match load_kind {
            ProjectLoadKind::FIND => {
                if let (Some(entry), true) = self.configs.load(config_file_path) {
                    return entry
                        .value()
                        .unwrap_or_else(|| crate::core::go_nil_dereference())
                        .borrow()
                        .command_line
                        .clone();
                }
                None
            }
            ProjectLoadKind::CREATE => {
                self.acquire_config_for_file(config_file_name, config_file_path, file_path, logger)
            }
            #[allow(unreachable_patterns)]
            _ => crate::core::go_panic(format!("unknown project load kind: {}", load_kind.0)),
        }
    }

    // Go: project/configfileregistrybuilder.go:148 configFileRegistryBuilder.reloadIfNeeded
    // reloadIfNeeded updates the command line of the config file entry based on its
    // pending reload state. This function should only be called from within the
    // Change() method of a dirty map entry.
    // PORT: `entry` is the handle; each field access is a short borrow, so
    // the calls below can reach other entries. tsgo#4712 returns whether
    // the command line changed (Go compares the pointers).
    pub fn reload_if_needed(
        &self,
        entry: &Rc<RefCell<ConfigFileEntry>>,
        file_name: &str,
        path: &tspath::Path,
        logger: Option<Rc<logging::LogTree>>,
    ) -> bool {
        let old_command_line = entry.borrow().command_line.clone();
        let pending_reload = entry.borrow().pending_reload;
        if pending_reload == PendingReload::FILE_NAMES {
            logger.log(&format!("Reloading file names for config: {file_name}"));
            let command_line = entry
                .borrow()
                .command_line
                .clone()
                .unwrap_or_else(|| crate::core::go_nil_dereference());
            let reloaded = command_line.reload_file_names_of_parsed_command_line(&*self.fs);
            entry.borrow_mut().command_line = Some(Rc::new(reloaded));
        } else if pending_reload == PendingReload::FULL {
            logger.log(&format!("Loading config file: {file_name}"));
            // When the workspace is trusted, enable external content mappers so a config's contentMappers pass
            // the runExternalCode gate and register, as they would with the CLI flag.
            let existing_options =
                self.session_options
                    .run_external_code
                    .then(|| CompilerOptions {
                        run_external_code: Tristate::True,
                        ..Default::default()
                    });
            let (command_line, _) = tsoptions::get_parsed_command_line_of_config_file_path(
                file_name,
                path.clone(),
                existing_options.as_ref(),
                None, /*optionsRaw*/
                self,
                Some(self as &dyn tsoptions::ExtendedConfigCache),
            );
            entry.borrow_mut().command_line = command_line.map(Rc::new);
            let new_command_line = entry.borrow().command_line.clone();
            self.update_extending_configs(
                path,
                new_command_line.as_deref(),
                old_command_line.as_deref(),
            );
            self.update_root_files_watch(file_name, entry);
            logger.log("Finished loading config file");
        } else {
            return false;
        }
        entry.borrow_mut().pending_reload = PendingReload::NONE;
        let new_command_line = entry.borrow().command_line.clone();
        match (&old_command_line, &new_command_line) {
            (Some(old), Some(new)) => !Rc::ptr_eq(old, new),
            (None, None) => false,
            _ => true,
        }
    }

    // Go: project/configfileregistrybuilder.go:173 configFileRegistryBuilder.updateExtendingConfigs
    pub fn update_extending_configs(
        &self,
        extending_config_path: &tspath::Path,
        new_command_line: Option<&tsoptions::ParsedCommandLine>,
        old_command_line: Option<&tsoptions::ParsedCommandLine>,
    ) {
        let mut new_extended_config_paths: FxHashSet<tspath::Path> = FxHashSet::default();
        if let Some(new_command_line) = new_command_line {
            for extended_config in new_command_line.extended_source_files() {
                let extended_config_path = (self.fs.to_path)(extended_config);
                new_extended_config_paths.insert(extended_config_path.clone());
                let (entry, loaded) = self.configs.load_or_store(
                    extended_config_path,
                    new_extended_config_file_entry(extended_config, extending_config_path.clone()),
                );
                if loaded {
                    let entry = entry.expect("dirty.SyncMap.LoadOrStore returns an entry");
                    entry.change_if(
                        &mut |config: Option<&Rc<RefCell<ConfigFileEntry>>>| {
                            let config =
                                config.unwrap_or_else(|| crate::core::go_nil_dereference());
                            let already_retaining = config
                                .borrow()
                                .retaining_configs
                                .contains(extending_config_path);
                            !already_retaining
                        },
                        &mut |config: &Rc<RefCell<ConfigFileEntry>>| {
                            // Go: make the map if nil (PORT: the set always exists)
                            config
                                .borrow_mut()
                                .retaining_configs
                                .insert(extending_config_path.clone());
                        },
                    );
                }
            }
        }
        if let Some(old_command_line) = old_command_line {
            for extended_config in old_command_line.extended_source_files() {
                let extended_config_path = (self.fs.to_path)(extended_config);
                if new_extended_config_paths.contains(&extended_config_path) {
                    continue;
                }
                if let (Some(entry), true) = self.configs.load(&extended_config_path) {
                    entry.change_if(
                        &mut |config: Option<&Rc<RefCell<ConfigFileEntry>>>| {
                            let config =
                                config.unwrap_or_else(|| crate::core::go_nil_dereference());
                            let exists = config
                                .borrow()
                                .retaining_configs
                                .contains(extending_config_path);
                            exists
                        },
                        &mut |config: &Rc<RefCell<ConfigFileEntry>>| {
                            config
                                .borrow_mut()
                                .retaining_configs
                                .remove(extending_config_path);
                        },
                    );
                }
            }
        }
    }

    // Go: project/configfileregistrybuilder.go:217 configFileRegistryBuilder.updateRootFilesWatch
    // PORT: Go `ParsedCommandLine` methods with a nil receiver return nil
    // (`WildcardDirectories`, `LiteralFileNames`, `ExtendedSourceFiles`);
    // a `None` command line gives empty values here.
    pub fn update_root_files_watch(&self, file_name: &str, entry: &Rc<RefCell<ConfigFileEntry>>) {
        let root_files_watch = entry.borrow().root_files_watch.clone();
        if root_files_watch.is_none() {
            return;
        }

        let mut ignored: FxHashSet<String> = FxHashSet::default();
        let mut globs: Vec<String> = Vec::new();
        let mut external_directories: Vec<String> = Vec::new();
        let mut include_workspace = false;
        let mut include_tsconfig_dir = false;
        let tsconfig_dir = tspath::get_directory_path(file_name);
        let command_line = entry.borrow().command_line.clone();
        let no_wildcard_directories: FxHashMap<String, bool> = FxHashMap::default();
        let wildcard_directories = match &command_line {
            Some(command_line) => command_line.wildcard_directories(),
            None => &no_wildcard_directories,
        };
        let compare_paths_options = tspath::ComparePathsOptions {
            current_directory: self.session_options.current_directory.clone(),
            use_case_sensitive_file_names: self.fs().use_case_sensitive_file_names(),
        };
        // PORT: Go map order is random; the globs are sorted below.
        for dir in wildcard_directories.keys() {
            if tspath::contains_path(
                &self.session_options.current_directory,
                dir,
                &compare_paths_options,
            ) {
                include_workspace = true;
            } else if tspath::contains_path(&tsconfig_dir, dir, &compare_paths_options) {
                include_tsconfig_dir = true;
            } else {
                external_directories.push(dir.clone());
            }
        }
        let literal_file_names: &[String] = match &command_line {
            Some(command_line) => command_line.literal_file_names(),
            None => &[],
        };
        for file_name in literal_file_names {
            if tspath::contains_path(
                &self.session_options.current_directory,
                file_name,
                &compare_paths_options,
            ) {
                include_workspace = true;
            } else if tspath::contains_path(&tsconfig_dir, file_name, &compare_paths_options) {
                include_tsconfig_dir = true;
            } else {
                external_directories.push(tspath::get_directory_path(file_name));
            }
        }

        if include_workspace {
            globs.push(get_recursive_glob_pattern(
                &self.session_options.current_directory,
            ));
        }
        if include_tsconfig_dir {
            globs.push(get_recursive_glob_pattern(&tsconfig_dir));
        }
        let extended_source_files: &[String] = match &command_line {
            Some(command_line) => command_line.extended_source_files(),
            None => &[],
        };
        for file_name in extended_source_files {
            if include_workspace
                && tspath::contains_path(
                    &self.session_options.current_directory,
                    file_name,
                    &compare_paths_options,
                )
            {
                continue;
            }
            globs.push(file_name.clone());
        }
        if !external_directories.is_empty() {
            let (common_parents, ignored_external_dirs) = tspath::get_common_parents(
                &external_directories,
                MIN_WATCH_LOCATION_DEPTH,
                &get_path_components_for_watching,
                &compare_paths_options,
            );
            for parent in &common_parents {
                globs.push(get_recursive_glob_pattern(parent));
            }
            ignored = ignored_external_dirs;
        }

        globs.sort();
        let new_root_files_watch = WatchedFiles::clone_(
            root_files_watch.as_deref(),
            PatternsAndIgnored {
                patterns_inside_workspace: globs,
                ignored,
                ..Default::default()
            },
        );
        entry.borrow_mut().root_files_watch = new_root_files_watch;
    }

    // Go: project/configfileregistrybuilder.go:283 configFileRegistryBuilder.acquireConfigForProject
    // acquireConfigForProject loads a config file entry from the cache, or parses it if not already
    // cached, then adds the project (if provided) to `retainingProjects` to keep it alive
    // in the cache. Each `acquireConfigForProject` call that passes a `project` should be accompanied
    // by an eventual `releaseConfigForProject` call with the same project.
    // PORT: reads `project.configFilePath` with a short borrow, so the
    // caller must not hold a mutable borrow of `project`.
    pub fn acquire_config_for_project(
        &self,
        file_name: &str,
        path: &tspath::Path,
        project: &Rc<RefCell<Project>>,
        logger: Option<Rc<logging::LogTree>>,
    ) -> Option<Rc<tsoptions::ParsedCommandLine>> {
        let (entry, _) = self.configs.load_or_store(
            path.clone(),
            new_config_file_entry(self.has_relative_pattern_capability, file_name),
        );
        let entry = entry.expect("dirty.SyncMap.LoadOrStore returns an entry");
        let needs_retain_project = Cell::new(false);
        let content_mappers_changed = Cell::new(false);
        entry.change_if(
            &mut |config: Option<&Rc<RefCell<ConfigFileEntry>>>| {
                let config = config.unwrap_or_else(|| crate::core::go_nil_dereference());
                // ts#64319
                let project_id = project.borrow().id();
                let already_retaining = config.borrow().retaining_projects.contains(&project_id);
                needs_retain_project.set(!already_retaining);
                needs_retain_project.get() || config.borrow().pending_reload != PendingReload::NONE
            },
            &mut |config: &Rc<RefCell<ConfigFileEntry>>| {
                if needs_retain_project.get() {
                    let project_id = project.borrow().id();
                    config.borrow_mut().retaining_projects.insert(project_id);
                }
                content_mappers_changed.set(self.reload_if_needed(
                    config,
                    file_name,
                    path,
                    logger.clone(),
                ));
            },
        );
        if content_mappers_changed.get() {
            self.invalidate_content_mappers();
        }
        entry
            .value()
            .unwrap_or_else(|| crate::core::go_nil_dereference())
            .borrow()
            .command_line
            .clone()
    }

    // Go: project/configfileregistrybuilder.go:313 configFileRegistryBuilder.acquireConfigForFile
    // acquireConfigForFile loads a config file entry from the cache, or parses it if not already
    // cached, then adds the open file to `retainingOpenFiles` to keep it alive in the cache.
    // Each `acquireConfigForFile` call that passes an `openFilePath`
    // should be accompanied by an eventual `releaseConfigForOpenFile` call with the same open file.
    pub fn acquire_config_for_file(
        &self,
        config_file_name: &str,
        config_file_path: &tspath::Path,
        file_path: &tspath::Path,
        logger: Option<Rc<logging::LogTree>>,
    ) -> Option<Rc<tsoptions::ParsedCommandLine>> {
        let (entry, _) = self.configs.load_or_store(
            config_file_path.clone(),
            new_config_file_entry(self.has_relative_pattern_capability, config_file_name),
        );
        let entry = entry.expect("dirty.SyncMap.LoadOrStore returns an entry");
        let needs_retain_open_file = Cell::new(false);
        let content_mappers_changed = Cell::new(false);
        entry.change_if(
            &mut |config: Option<&Rc<RefCell<ConfigFileEntry>>>| {
                let config = config.unwrap_or_else(|| crate::core::go_nil_dereference());
                if (self.is_open_file)(file_path) {
                    let already_retaining =
                        config.borrow().retaining_open_files.contains(file_path);
                    needs_retain_open_file.set(!already_retaining);
                }
                needs_retain_open_file.get()
                    || config.borrow().pending_reload != PendingReload::NONE
            },
            &mut |config: &Rc<RefCell<ConfigFileEntry>>| {
                if needs_retain_open_file.get() {
                    config
                        .borrow_mut()
                        .retaining_open_files
                        .insert(file_path.clone());
                }
                content_mappers_changed.set(self.reload_if_needed(
                    config,
                    config_file_name,
                    config_file_path,
                    logger.clone(),
                ));
            },
        );
        if content_mappers_changed.get() {
            self.invalidate_content_mappers();
        }
        entry
            .value()
            .unwrap_or_else(|| crate::core::go_nil_dereference())
            .borrow()
            .command_line
            .clone()
    }

    // Go: project/configfileregistrybuilder.go:343 configFileRegistryBuilder.releaseConfigForProject
    // releaseConfigForProject removes the project from the config entry. Once no projects
    // or files are associated with the config entry, it will be removed on the next call to `cleanup`.
    // ts#64319: takes the project ID.
    pub fn release_config_for_project(&self, config_file_path: &tspath::Path, project_id: &ID) {
        if let (Some(entry), true) = self.configs.load(config_file_path) {
            entry.change_if(
                &mut |config: Option<&Rc<RefCell<ConfigFileEntry>>>| {
                    let config = config.unwrap_or_else(|| crate::core::go_nil_dereference());
                    let exists = config.borrow().retaining_projects.contains(project_id);
                    exists
                },
                &mut |config: &Rc<RefCell<ConfigFileEntry>>| {
                    config.borrow_mut().retaining_projects.remove(project_id);
                },
            );
        }
    }

    // Go: project/configfileregistrybuilder.go:357 configFileRegistryBuilder.retainConfigForProject (ts#63950)
    // PORT: a Go nil `retainingProjects` map is the empty set here, so the
    // Go `make` before the write is not needed.
    // ts#64319: takes the project ID.
    pub fn retain_config_for_project(&self, config_file_path: &tspath::Path, project_id: &ID) {
        if let (Some(entry), true) = self.configs.load(config_file_path) {
            entry.change_if(
                &mut |config: Option<&Rc<RefCell<ConfigFileEntry>>>| {
                    let config = config.unwrap_or_else(|| crate::core::go_nil_dereference());
                    let exists = config.borrow().retaining_projects.contains(project_id);
                    !exists
                },
                &mut |config: &Rc<RefCell<ConfigFileEntry>>| {
                    config
                        .borrow_mut()
                        .retaining_projects
                        .insert(project_id.clone());
                },
            );
        }
    }

    // Go: project/configfileregistrybuilder.go:376 configFileRegistryBuilder.didCloseFile
    // didCloseFile removes the open file from the config entry. Once no projects
    // or files are associated with the config entry, it will be removed on the next call to `cleanup`.
    pub fn did_close_file(&self, path: &tspath::Path) {
        if tspath::is_dynamic_file_name(path) {
            return;
        }
        self.config_file_names.delete(path);
        self.configs.range(&mut |entry: &Rc<
            dirty::SyncMapEntry<tspath::Path, Rc<RefCell<ConfigFileEntry>>>,
        >| {
            entry.change_if(
                &mut |config: Option<&Rc<RefCell<ConfigFileEntry>>>| {
                    let config = config.unwrap_or_else(|| crate::core::go_nil_dereference());
                    let ok = config.borrow().retaining_open_files.contains(path);
                    ok
                },
                &mut |config: &Rc<RefCell<ConfigFileEntry>>| {
                    config.borrow_mut().retaining_open_files.remove(path);
                },
            );
            true
        });
    }

    // Go: project/configfileregistrybuilder.go:404 configFileRegistryBuilder.DidChangeCustomConfigFileName
    pub fn did_change_custom_config_file_name(
        &self,
        _logger: Option<Rc<logging::LogTree>>,
    ) -> bool {
        if !self.custom_config_file_name_changed {
            return false;
        }

        self.config_file_names.clear();
        true
    }

    // Go: project/configfileregistrybuilder.go:413 configFileRegistryBuilder.invalidateCache
    pub fn invalidate_cache(&self, logger: Option<Rc<logging::LogTree>>) -> ChangeFileResult {
        let mut affected_projects: Option<FxHashSet<ID>> = None;
        let mut affected_files: Option<FxHashSet<tspath::Path>> = None;

        logger.log("Too many files changed; marking all configs for reload");
        self.config_file_names.range(&mut |entry: &Rc<
            dirty::MapEntry<tspath::Path, Rc<RefCell<ConfigFileNames>>>,
        >| {
            affected_files
                .get_or_insert_with(FxHashSet::default)
                .insert(entry.key());
            true
        });
        self.config_file_names.clear();

        self.configs.range(&mut |entry: &Rc<
            dirty::SyncMapEntry<tspath::Path, Rc<RefCell<ConfigFileEntry>>>,
        >| {
            entry.change(&mut |entry: &Rc<RefCell<ConfigFileEntry>>| {
                let retaining_projects = entry.borrow().retaining_projects.clone();
                affected_projects =
                    Some(copy_map_into(affected_projects.take(), &retaining_projects));
                let pending_reload = entry.borrow().pending_reload;
                if pending_reload != PendingReload::FULL {
                    let file_name = entry.borrow().file_name.clone();
                    let (text, ok) = self.fs().read_file(&file_name);
                    // Go: entry.commandLine.ConfigFile.SourceFile.Text()
                    let config_text = || -> FileText {
                        let entry = entry.borrow();
                        let command_line = entry
                            .command_line
                            .as_ref()
                            .unwrap_or_else(|| crate::core::go_nil_dereference());
                        let config_file = command_line
                            .config_file
                            .as_ref()
                            .unwrap_or_else(|| crate::core::go_nil_dereference());
                        source_file_text(config_file.source_file)
                    };
                    if !ok
                        || entry.borrow().command_line.is_none()
                        || text.as_str() != &*config_text()
                    {
                        entry.borrow_mut().pending_reload = PendingReload::FULL;
                    } else {
                        entry.borrow_mut().pending_reload = PendingReload::FILE_NAMES;
                    }
                }
            });
            true
        });

        ChangeFileResult {
            affected_projects: affected_projects.unwrap_or_default(),
            affected_files: affected_files.unwrap_or_default(),
        }
    }

    // Go: project/configfileregistrybuilder.go:448 configFileRegistryBuilder.isConfigBaseName
    pub fn is_config_base_name(&self, base_name: &str) -> bool {
        base_name == "tsconfig.json"
            || base_name == "jsconfig.json"
            || (!self.custom_config_file_name.is_empty()
                && base_name == self.custom_config_file_name)
    }

    // Go: project/configfileregistrybuilder.go:453 configFileRegistryBuilder.DidChangeFiles
    // PORT: Go passes the summary by value. The local Go maps are
    // `IndexMap`/`IndexSet` (insertion order; PORT: Go map order is random).
    pub fn did_change_files(
        &self,
        summary: &FileChangeSummary,
        logger: Option<Rc<logging::LogTree>>,
    ) -> ChangeFileResult {
        // ts#64115
        if summary.invalidate_all {
            return self.invalidate_cache(logger);
        }
        let mut affected_projects: Option<FxHashSet<ID>> = None;
        let mut affected_files: Option<FxHashSet<tspath::Path>> = None;
        let mut should_invalidate_cache = false;

        logger.log("Summarizing file changes");
        let has_excessive_changes = summary.has_excessive_watch_events()
            && summary.includes_watch_change_outside_node_modules;
        let mut created_files: IndexMap<tspath::Path, String> =
            IndexMap::with_capacity(summary.created.len());
        let mut deleted_files: IndexMap<tspath::Path, String> =
            IndexMap::with_capacity(summary.deleted.len());
        let mut created_or_deleted_config_files: IndexSet<tspath::Path> = IndexSet::new();
        let mut created_or_changed_or_deleted_files: IndexSet<tspath::Path> =
            IndexSet::with_capacity(
                summary.changed.len() + summary.created.len() + summary.deleted.len(),
            );
        for uri in summary.changed.iter() {
            // ts#64159: the file name is checked, not the URI text.
            let file_name = uri.file_name();
            if tspath::contains_ignored_path(&file_name) {
                continue;
            }
            let path = (self.fs.to_path)(&file_name);
            let base_name = tspath::get_base_file_name(&path);
            if self.is_config_base_name(&base_name) {
                created_or_deleted_config_files.insert(path.clone());
            }
            created_or_changed_or_deleted_files.insert(path);
        }
        for uri in summary.deleted.iter() {
            // ts#64159: the file name is checked, not the URI text.
            let file_name = uri.file_name();
            if tspath::contains_ignored_path(&file_name) {
                continue;
            }
            let path = (self.fs.to_path)(&file_name);
            deleted_files.insert(path.clone(), file_name);
            let base_name = tspath::get_base_file_name(&path);
            if self.is_config_base_name(&base_name) {
                created_or_deleted_config_files.insert(path.clone());
            }
            created_or_changed_or_deleted_files.insert(path);
        }
        for uri in summary.created.iter() {
            // ts#64159: the file name is checked, not the URI text.
            let file_name = uri.file_name();
            if tspath::contains_ignored_path(&file_name) {
                continue;
            }
            let path = (self.fs.to_path)(&file_name);
            created_files.insert(path.clone(), file_name);
            let base_name = tspath::get_base_file_name(&path);
            if self.is_config_base_name(&base_name) {
                created_or_deleted_config_files.insert(path.clone());
            }
            created_or_changed_or_deleted_files.insert(path);
        }

        // Handle closed files - this ranges over config entries and could be combined
        // with the file change handling, but a separate loop is simpler and a snapshot
        // change with both closing and watch changes seems rare.
        for uri in summary.closed.iter() {
            let file_name = uri.file_name();
            let path = (self.fs.to_path)(&file_name);
            self.did_close_file(&path);
        }

        // Handle changes to stored config files and their content mapper package manifests.
        logger.log("Checking if any changed files are configuration files");
        for path in &created_or_changed_or_deleted_files {
            if let (Some(entry), true) = self.configs.load(path) {
                if has_excessive_changes {
                    return self.invalidate_cache(logger.clone());
                }

                affected_projects = Some(copy_map_into(
                    affected_projects.take(),
                    &self.handle_config_change(&entry, logger.clone()),
                ));
                // PORT: the keys are copied before the loop changes entries.
                let retaining_configs: Vec<tspath::Path> = entry
                    .value()
                    .unwrap_or_else(|| crate::core::go_nil_dereference())
                    .borrow()
                    .retaining_configs
                    .iter()
                    .cloned()
                    .collect();
                for extending_config_path in &retaining_configs {
                    if let (Some(extending_config_entry), true) =
                        self.configs.load(extending_config_path)
                    {
                        affected_projects = Some(copy_map_into(
                            affected_projects.take(),
                            &self.handle_config_change(&extending_config_entry, logger.clone()),
                        ));
                    }
                }
                // This was a config file, so assume it's not also a root file
                created_files.shift_remove(path);
            } else if tspath::get_base_file_name(path) == "package.json" {
                let mut manifest_changed = false;
                self.configs.range(&mut |entry: &Rc<
                    dirty::SyncMapEntry<tspath::Path, Rc<RefCell<ConfigFileEntry>>>,
                >| {
                    let command_line = entry
                        .value()
                        .unwrap_or_else(|| crate::core::go_nil_dereference())
                        .borrow()
                        .command_line
                        .clone();
                    if content_mapper_manifest_path(
                        command_line.as_deref(),
                        &*self.fs.to_path,
                        path,
                    ) {
                        affected_projects = Some(copy_map_into(
                            affected_projects.take(),
                            &self.handle_config_change(entry, logger.clone()),
                        ));
                        manifest_changed = true;
                    }
                    true
                });
                if manifest_changed {
                    self.invalidate_content_mappers();
                }
            }
        }

        // Handle created/deleted files named "tsconfig.json" or "jsconfig.json"
        for path in &created_or_deleted_config_files {
            if has_excessive_changes {
                return self.invalidate_cache(logger.clone());
            }
            let directory_path = path.get_directory_path();
            self.config_file_names.range(&mut |entry: &Rc<
                dirty::MapEntry<tspath::Path, Rc<RefCell<ConfigFileNames>>>,
            >| {
                if directory_path.contains_path(&entry.key()) {
                    affected_files
                        .get_or_insert_with(FxHashSet::default)
                        .insert(entry.key());
                    entry.delete();
                }
                true
            });
        }

        // Handle deletions of wildcard-included root files
        for (path, file_name) in &deleted_files {
            self.configs.range(&mut |entry: &Rc<
                dirty::SyncMapEntry<tspath::Path, Rc<RefCell<ConfigFileEntry>>>,
            >| {
                entry.change_if(
                    &mut |config: Option<&Rc<RefCell<ConfigFileEntry>>>| {
                        let config = config
                            .unwrap_or_else(|| crate::core::go_nil_dereference())
                            .borrow();
                        if config.pending_reload != PendingReload::NONE {
                            return false;
                        }
                        let Some(command_line) = &config.command_line else {
                            return false;
                        };
                        if command_line.file_names_by_path().contains_key(path) {
                            // If the file is included in FileNames() but not matched by literal "files", it must be
                            // included via wildcard, which means a reload of filenames will remove it from the list.
                            // (Files explicitly specified in "files" are always included in the ParsedCommandLine,
                            // triggering a missing root file error during program construction.)
                            return command_line.get_matched_file_spec(file_name).is_empty();
                        }
                        false
                    },
                    &mut |config: &Rc<RefCell<ConfigFileEntry>>| {
                        config.borrow_mut().pending_reload = PendingReload::FILE_NAMES;
                        // Go: maps.Copy(affectedProjects, config.retainingProjects)
                        let retaining_projects = config.borrow().retaining_projects.clone();
                        affected_projects
                            .get_or_insert_with(FxHashSet::default)
                            .extend(retaining_projects);
                        logger.logf(&format!("Root files for config {} changed", entry.key()));
                        should_invalidate_cache = has_excessive_changes;
                    },
                );
                !should_invalidate_cache
            });
            if should_invalidate_cache {
                return self.invalidate_cache(logger.clone());
            }
        }

        // Handle possible root file creation
        if !created_files.is_empty() {
            self.configs.range(&mut |entry: &Rc<
                dirty::SyncMapEntry<tspath::Path, Rc<RefCell<ConfigFileEntry>>>,
            >| {
                entry.change_if(
                    &mut |config: Option<&Rc<RefCell<ConfigFileEntry>>>| {
                        let config = config
                            .unwrap_or_else(|| crate::core::go_nil_dereference())
                            .borrow();
                        if config.command_line.is_none()
                            || config.root_files_watch.is_none()
                            || config.pending_reload != PendingReload::NONE
                        {
                            return false;
                        }
                        let command_line = config.command_line.as_ref().expect("checked above");
                        logger.logf(&format!(
                            "Checking if any of {} created files match root files for config {}",
                            created_files.len(),
                            entry.key()
                        ));
                        for (path, file_name) in &created_files {
                            if command_line.possibly_matches_file_name(file_name) {
                                return true;
                            }
                            if command_line.possibly_matches_directory_name(path)
                                && self.fs.directory_exists(file_name)
                            {
                                // If we got a creation event for a directory, it's probably a symlink. We don't need to
                                // test realpath here; this is enough confidence to trigger a filename reload.
                                return true;
                            }
                        }
                        false
                    },
                    &mut |config: &Rc<RefCell<ConfigFileEntry>>| {
                        config.borrow_mut().pending_reload = PendingReload::FILE_NAMES;
                        // Go: maps.Copy(affectedProjects, config.retainingProjects)
                        let retaining_projects = config.borrow().retaining_projects.clone();
                        affected_projects
                            .get_or_insert_with(FxHashSet::default)
                            .extend(retaining_projects);
                        logger.logf(&format!("Root files for config {} changed", entry.key()));
                        should_invalidate_cache = has_excessive_changes;
                    },
                );
                !should_invalidate_cache
            });
            if should_invalidate_cache {
                return self.invalidate_cache(logger.clone());
            }
        }

        ChangeFileResult {
            affected_projects: affected_projects.unwrap_or_default(),
            affected_files: affected_files.unwrap_or_default(),
        }
    }

    // Go: project/configfileregistrybuilder.go:642 configFileRegistryBuilder.handleConfigChange
    // PORT: a nil result map is the empty set.
    pub fn handle_config_change(
        &self,
        entry: &dirty::SyncMapEntry<tspath::Path, Rc<RefCell<ConfigFileEntry>>>,
        logger: Option<Rc<logging::LogTree>>,
    ) -> FxHashSet<ID> {
        let mut affected_projects: FxHashSet<ID> = FxHashSet::default();
        let changed = entry.change_if(
            &mut |config: Option<&Rc<RefCell<ConfigFileEntry>>>| {
                let config = config.unwrap_or_else(|| crate::core::go_nil_dereference());
                let pending_reload = config.borrow().pending_reload;
                pending_reload != PendingReload::FULL
            },
            &mut |config: &Rc<RefCell<ConfigFileEntry>>| {
                config.borrow_mut().pending_reload = PendingReload::FULL;
            },
        );
        if changed {
            logger.logf(&format!("Config file {} changed", entry.key()));
            affected_projects = entry
                .value()
                .unwrap_or_else(|| crate::core::go_nil_dereference())
                .borrow()
                .retaining_projects
                .clone();
        }

        affected_projects
    }

    // Go: project/configfileregistrybuilder.go:669 configFileRegistryBuilder.computeConfigFileName
    pub fn compute_config_file_name(
        &self,
        file_name: &str,
        skip_search_in_directory_of_file: bool,
        logger: Option<Rc<logging::LogTree>>,
    ) -> String {
        let search_path = tspath::get_directory_path(file_name);
        // Prefer custom config file if provided; search ancestors with correct skip behavior.
        if !self.custom_config_file_name.is_empty() {
            let mut skip = skip_search_in_directory_of_file;
            let (result, _) = tspath::for_each_ancestor_directory(
                &search_path,
                |directory: &str| -> (String, bool) {
                    if !skip {
                        let custom_path =
                            tspath::combine_paths(directory, &[&self.custom_config_file_name]);
                        if self.fs().file_exists(&custom_path) {
                            return (custom_path, true);
                        }
                    }
                    if directory.ends_with("/node_modules") {
                        return (String::new(), true);
                    }
                    skip = false;
                    (String::new(), false)
                },
            );
            if !result.is_empty() {
                logger.logf(&format!(
                    "computeConfigFileName:: File: {file_name}:: Result: {result}"
                ));
                return result;
            }
        }

        // When searching for ancestor of a config file, determine which config types to skip
        // in the starting directory. This matches TSServer's forEachConfigFileLocation behavior:
        // - For ancestor of tsconfig.json: skip tsconfig.json but still check jsconfig.json
        // - For ancestor of jsconfig.json: skip both tsconfig.json and jsconfig.json
        let mut skip_tsconfig = skip_search_in_directory_of_file;
        let mut skip_jsconfig =
            skip_search_in_directory_of_file && !file_name.ends_with("/tsconfig.json");
        let (result, _) = tspath::for_each_ancestor_directory(
            &search_path,
            |directory: &str| -> (String, bool) {
                if !skip_tsconfig {
                    let tsconfig_path = tspath::combine_paths(directory, &["tsconfig.json"]);
                    if self.fs().file_exists(&tsconfig_path) {
                        return (tsconfig_path, true);
                    }
                }
                if !skip_jsconfig {
                    let jsconfig_path = tspath::combine_paths(directory, &["jsconfig.json"]);
                    if self.fs().file_exists(&jsconfig_path) {
                        return (jsconfig_path, true);
                    }
                }
                if directory.ends_with("/node_modules") {
                    return (String::new(), true);
                }
                skip_tsconfig = false;
                skip_jsconfig = false;
                (String::new(), false)
            },
        );
        logger.logf(&format!(
            "computeConfigFileName:: File: {file_name}:: Result: {result}"
        ));
        result
    }

    // Go: project/configfileregistrybuilder.go:722 configFileRegistryBuilder.getConfigFileNameForFile
    pub fn get_config_file_name_for_file(
        &self,
        file_name: &str,
        path: &tspath::Path,
        logger: Option<Rc<logging::LogTree>>,
    ) -> String {
        if tspath::is_dynamic_file_name(file_name) {
            return String::new();
        }

        if let (Some(entry), true) = self.config_file_names.get(path) {
            return entry
                .value()
                .unwrap_or_else(|| crate::core::go_nil_dereference())
                .borrow()
                .nearest_config_file_name
                .clone();
        }

        let config_name = self.compute_config_file_name(file_name, false, logger);
        if (self.is_open_file)(path) {
            self.config_file_names.add(
                path.clone(),
                Rc::new(RefCell::new(ConfigFileNames {
                    nearest_config_file_name: config_name.clone(),
                    ancestors: FxHashMap::default(),
                })),
            );
        }
        config_name
    }

    // Go: project/configfileregistrybuilder.go:740 configFileRegistryBuilder.forEachConfigFileNameFor
    pub fn for_each_config_file_name_for(&self, path: &tspath::Path, cb: &mut dyn FnMut(&str)) {
        if tspath::is_dynamic_file_name(path) {
            return;
        }

        if let (Some(entry), true) = self.config_file_names.get(path) {
            let mut config_file_name = entry
                .value()
                .unwrap_or_else(|| crate::core::go_nil_dereference())
                .borrow()
                .nearest_config_file_name
                .clone();
            while !config_file_name.is_empty() {
                cb(&config_file_name);
                let ancestor_config_name = entry
                    .value()
                    .unwrap_or_else(|| crate::core::go_nil_dereference())
                    .borrow()
                    .ancestors
                    .get(&config_file_name)
                    .cloned();
                if let Some(ancestor_config_name) = ancestor_config_name {
                    config_file_name = ancestor_config_name;
                } else {
                    return;
                }
            }
        }
    }

    // Go: project/configfileregistrybuilder.go:758 configFileRegistryBuilder.getAncestorConfigFileName
    pub fn get_ancestor_config_file_name(
        &self,
        file_name: &str,
        path: &tspath::Path,
        config_file_name: &str,
        logger: Option<Rc<logging::LogTree>>,
    ) -> String {
        if tspath::is_dynamic_file_name(file_name) {
            return String::new();
        }

        let (entry, ok) = self.config_file_names.get(path);
        if !ok {
            return String::new();
        }
        let entry = entry.expect("dirty.Map.Get: ok implies an entry");

        let ancestor_config_name = entry
            .value()
            .unwrap_or_else(|| crate::core::go_nil_dereference())
            .borrow()
            .ancestors
            .get(config_file_name)
            .cloned();
        if let Some(ancestor_config_name) = ancestor_config_name {
            return ancestor_config_name;
        }

        // Look for config in parent folders of config file
        let result = self.compute_config_file_name(config_file_name, true, logger);

        if (self.is_open_file)(path) {
            entry.change(&mut |value: &Rc<RefCell<ConfigFileNames>>| {
                // Go: make the map if nil (PORT: the map always exists)
                value
                    .borrow_mut()
                    .ancestors
                    .insert(config_file_name.to_string(), result.clone());
            });
        }
        result
    }

    // Go: project/configfileregistrybuilder.go:787 configFileRegistryBuilder.FS
    // FS implements tsoptions.ParseConfigHost.
    pub fn fs(&self) -> Rc<dyn vfs::Fs> {
        self.fs.clone()
    }

    // Go: project/configfileregistrybuilder.go:792 configFileRegistryBuilder.GetCurrentDirectory
    // GetCurrentDirectory implements tsoptions.ParseConfigHost.
    pub fn get_current_directory(&self) -> String {
        self.session_options.current_directory.clone()
    }

    // Go: project/configfileregistrybuilder.go:797 configFileRegistryBuilder.GetExtendedConfig
    // GetExtendedConfig implements tsoptions.ExtendedConfigCache.
    // PORT: the cache arguments own their host (`Rc<dyn ParseConfigHost>`).
    // Go passes `host` there; every caller passes this builder as `host`
    // (reloadIfNeeded passes `c` as host and cache, and tsoptions passes
    // the host it got through), so the port passes `this`.
    pub fn get_extended_config(
        &self,
        file_name: &str,
        path: &tspath::Path,
        resolution_stack: &[tspath::Path],
        _host: &dyn tsoptions::ParseConfigHost,
    ) -> Rc<tsoptions::ExtendedConfigCacheEntry> {
        let mut content = String::new();
        let fh = self.fs.get_file_by_path(file_name, path);
        if let Some(fh) = fh {
            content = fh.content();
        }

        let this = self.this_rc();
        let host: Rc<dyn tsoptions::ParseConfigHost> = this.clone();
        let cache: Rc<dyn tsoptions::ExtendedConfigCache> = this;
        let source = self.fs.source.borrow().clone();
        self.extended_config_cache
            .load_and_acquire(
                path.clone(),
                self.snapshot_id,
                ExtendedConfigParseArgs {
                    file_name: file_name.to_string(),
                    content,
                    fs: source,
                    resolution_stack: resolution_stack.to_vec(),
                    host,
                    cache: Some(cache),
                },
            )
            .extended_config_cache_entry
            .clone()
    }

    // Go: project/configfileregistrybuilder.go:814 configFileRegistryBuilder.Cleanup
    pub fn cleanup(&self) {
        let mut changed = false;
        self.configs.range(&mut |entry: &Rc<
            dirty::SyncMapEntry<tspath::Path, Rc<RefCell<ConfigFileEntry>>>,
        >| {
            entry.delete_if(&mut |value: Option<&Rc<RefCell<ConfigFileEntry>>>| {
                let value = value
                    .unwrap_or_else(|| crate::core::go_nil_dereference())
                    .borrow();
                let should_delete = value.retaining_projects.is_empty()
                    && value.retaining_open_files.is_empty()
                    && value.retaining_configs.is_empty();
                changed = changed || should_delete;
                should_delete
            });
            true
        });
        if changed {
            self.invalidate_content_mappers();
        }
    }
}

// Go: project/configfileregistrybuilder.go:656 contentMapperManifestPath (tsgo#4712)
pub fn content_mapper_manifest_path(
    command_line: Option<&tsoptions::ParsedCommandLine>,
    to_path: &dyn Fn(&str) -> tspath::Path,
    path: &tspath::Path,
) -> bool {
    let Some(command_line) = command_line else {
        return false;
    };
    for mapper in command_line.content_mappers() {
        if !mapper.definition.package.is_empty()
            && mapper.contribution_id.is_empty()
            && !mapper.package_directory.is_empty()
            && to_path(&tspath::combine_paths(
                &mapper.package_directory,
                &["package.json"],
            )) == *path
        {
            return true;
        }
    }
    false
}

// Go: project/configfileregistrybuilder.go:395 changeFileResult
// PORT: a nil Go map is the empty set.
#[derive(Clone, Debug, Default)]
pub struct ChangeFileResult {
    // ts#64319: project IDs.
    pub affected_projects: FxHashSet<ID>,
    pub affected_files: FxHashSet<tspath::Path>,
}

impl ChangeFileResult {
    // Go: project/configfileregistrybuilder.go:400 changeFileResult.IsEmpty
    pub fn is_empty(&self) -> bool {
        self.affected_projects.is_empty() && self.affected_files.is_empty()
    }
}

// Go: project/configfileregistrybuilder.go:19 `_ tsoptions.ParseConfigHost = (*configFileRegistryBuilder)(nil)`
impl tsoptions::ParseConfigHost for ConfigFileRegistryBuilder {
    fn fs(&self) -> Rc<dyn vfs::Fs> {
        ConfigFileRegistryBuilder::fs(self)
    }

    fn get_current_directory(&self) -> String {
        ConfigFileRegistryBuilder::get_current_directory(self)
    }
}

// Go: project/configfileregistrybuilder.go:20 `_ tsoptions.ExtendedConfigCache = (*configFileRegistryBuilder)(nil)`
impl tsoptions::ExtendedConfigCache for ConfigFileRegistryBuilder {
    fn get_extended_config(
        &self,
        file_name: &str,
        path: &tspath::Path,
        resolution_stack: &[tspath::Path],
        host: &dyn tsoptions::ParseConfigHost,
    ) -> Rc<tsoptions::ExtendedConfigCacheEntry> {
        ConfigFileRegistryBuilder::get_extended_config(
            self,
            file_name,
            path,
            resolution_stack,
            host,
        )
    }
}
