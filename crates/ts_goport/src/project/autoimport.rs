//! Go `internal/project/autoimport.go`.
//!
//! PORT: one thread (project/dirty/interfaces.rs). Go
//! `collections.SyncMap` is a `RefCell<FxHashMap>`; `filesMu` is dropped.
//! Go `FileHandle` is `Option<Rc<dyn FileHandle>>` (nil is `None`). In this
//! package's prelude, `autoimport` is Go package `ls/autoimport`.

use crate::project::prelude::*;

use crate::frontend::module;
use crate::frontend::parser;
use crate::frontend::vfs::Fs as _;

// Go: project/autoimport.go:16 autoImportBuilderFS
// PORT: the untracked map stores Go nil handles too (Go `LoadOrStore` of a
// nil `fh` caches the miss), so its values are `Option`.
pub struct AutoImportBuilderFS {
    pub snapshot_fs_builder: Rc<SnapshotFSBuilder>,
    pub untracked_files: RefCell<FxHashMap<tspath::Path, Option<Rc<dyn FileHandle>>>>,
}

// Go: project/autoimport.go:20 `var _ FileSource = (*autoImportBuilderFS)(nil)`
impl FileSource for AutoImportBuilderFS {
    // Go: project/autoimport.go:23 FS
    // FS implements FileSource.
    fn fs(&self) -> Rc<dyn vfs::Fs> {
        self.snapshot_fs_builder.fs.clone()
    }

    // Go: project/autoimport.go:57 FileExists
    // FileExists implements FileSource.
    fn file_exists(&self, file_name: &str, path: &tspath::Path) -> bool {
        self.snapshot_fs_builder.file_exists(file_name, path)
    }

    // Go: project/autoimport.go:52 GetAccessibleEntries
    // PORT: after FileExists because the Rust trait lists it last.
    fn get_accessible_entries(&self, path: &str) -> vfs::Entries {
        self.snapshot_fs_builder.get_accessible_entries(path)
    }
}

impl FileHandleSource for AutoImportBuilderFS {
    // Go: project/autoimport.go:29 GetFile
    // GetFile implements FileSource.
    fn get_file(&self, file_name: &str) -> Option<Rc<dyn FileHandle>> {
        let path = (self.snapshot_fs_builder.to_path)(file_name);
        self.get_file_by_path(file_name, &path)
    }

    // Go: project/autoimport.go:35 GetFileByPath
    // GetFileByPath implements FileSource.
    fn get_file_by_path(&self, file_name: &str, path: &tspath::Path) -> Option<Rc<dyn FileHandle>> {
        // We want to avoid long-term caching of files referenced only by auto-imports, so we
        // override GetFileByPath to avoid collecting more files into the snapshotFSBuilder's
        // cacheFiles. (Note the reason we can't just use the finalized SnapshotFS is that changed
        // files not read during other parts of the snapshot clone will be marked as dirty, but
        // not yet refreshed from the source filesystem.)
        if let (Some(cached_file), true) = self.snapshot_fs_builder.cache_files.load(path) {
            return self
                .snapshot_fs_builder
                .reload_entry_if_needed(&cached_file);
        }
        if let Some(fh) = self.untracked_files.borrow().get(path) {
            return fh.clone();
        }
        // ts#64291
        let fh = self
            .snapshot_fs_builder
            .fs
            .get_file_by_path(file_name, path);
        // Go: fh, _ = a.untrackedFiles.LoadOrStore(path, fh)
        let fh = self
            .untracked_files
            .borrow_mut()
            .entry(path.clone())
            .or_insert(fh)
            .clone();
        fh
    }
}

/// The parse cache keys that auto-import registry clones acquired, one per
/// path, kept after each clone. The session owns it
/// (`Session::auto_import_parse_keys`). See
/// `AutoImportRegistryCloneHost::dispose`.
// PORT: no Go counterpart.
pub type AutoImportParseKeys = RefCell<FxHashMap<tspath::Path, ParseCacheKey>>;

// Go: project/autoimport.go:61 autoImportRegistryCloneHost
// PORT: `filesMu` is dropped; `files` is written after sharing, so it is a
// `RefCell`. `kept_files` has no Go counterpart (see `dispose`).
pub struct AutoImportRegistryCloneHost {
    pub project_collection: Rc<ProjectCollection>,
    pub parse_cache: Rc<ParseCache>,
    pub fs: Rc<SourceFS>,
    pub current_directory: String,

    pub files: RefCell<Vec<ParseCacheKey>>,
    pub kept_files: Rc<AutoImportParseKeys>,
}

// Go: project/autoimport.go:73 newAutoImportRegistryCloneHost
// PORT: `kept_files` is the session's `auto_import_parse_keys`.
pub fn new_auto_import_registry_clone_host(
    project_collection: Rc<ProjectCollection>,
    parse_cache: Rc<ParseCache>,
    snapshot_fs_builder: Rc<SnapshotFSBuilder>,
    current_directory: &str,
    to_path: Rc<dyn Fn(&str) -> tspath::Path>,
    kept_files: Rc<AutoImportParseKeys>,
) -> Rc<AutoImportRegistryCloneHost> {
    Rc::new(AutoImportRegistryCloneHost {
        project_collection,
        parse_cache,
        fs: new_source_fs(
            false,
            Rc::new(AutoImportBuilderFS {
                snapshot_fs_builder,
                untracked_files: RefCell::new(FxHashMap::default()),
            }),
            to_path,
        ),
        current_directory: current_directory.to_string(),
        files: RefCell::new(Vec::new()),
        kept_files,
    })
}

// PORT: Go `autoimport.RegistryCloneHost` embeds `module.ResolutionHost`;
// its `FS` and `GetCurrentDirectory` are this supertrait.
impl module::ResolutionHost for AutoImportRegistryCloneHost {
    // Go: project/autoimport.go:95 FS
    // FS implements autoimport.RegistryCloneHost.
    fn fs(&self) -> &dyn vfs::Fs {
        &*self.fs
    }

    // Go: project/autoimport.go:94 GetCurrentDirectory
    // GetCurrentDirectory implements autoimport.RegistryCloneHost.
    fn get_current_directory(&self) -> &str {
        &self.current_directory
    }
}

// Go: project/autoimport.go:77 `var _ autoimport.RegistryCloneHost = (*autoImportRegistryCloneHost)(nil)`
impl autoimport::RegistryCloneHost for AutoImportRegistryCloneHost {
    // Go: project/autoimport.go:99 GetDefaultProject
    // GetDefaultProject implements autoimport.RegistryCloneHost.
    // ts#64319: the project ID (Go nil is `None`).
    fn get_default_project(
        &self,
        path: &tspath::Path,
    ) -> (
        Option<autoimport::ProjectID>,
        Option<Rc<compiler::NewProgram>>,
    ) {
        let Some(project) = self.project_collection.get_default_project(path) else {
            return (None, None);
        };
        let project = project.borrow();
        // PORT: Go `project.GetProgram()` is the field read.
        (
            Some(project.id().as_auto_import_project_id()),
            project.program.clone(),
        )
    }

    // Go: project/autoimport.go:139 GetProgramForProject
    // GetProgramForProject implements autoimport.RegistryCloneHost.
    // ts#64319: Go asserts the `autoimport.ProjectID` to a project `ID`; every
    // Rust `ProjectID` holds an ID string.
    fn get_program_for_project(
        &self,
        project_id: &autoimport::ProjectID,
    ) -> Option<Rc<compiler::NewProgram>> {
        let id = ID(project_id.0.clone());
        let project = self.project_collection.get_project(&id)?;
        // PORT: Go `project.GetProgram()` is the field read.
        let program = project.borrow().program.clone();
        program
    }

    // Go: project/autoimport.go:108 GetPackageJson
    // GetPackageJson implements autoimport.RegistryCloneHost.
    fn get_package_json(&self, file_name: &str) -> Option<Rc<packagejson::InfoCacheEntry>> {
        // !!! ref-counted shared cache
        let fh = self.fs.get_file(file_name);
        let package_directory = tspath::get_directory_path(file_name);
        let Some(fh) = fh else {
            return Some(Rc::new(packagejson::InfoCacheEntry {
                directory_exists: self.fs.directory_exists(&package_directory),
                package_directory,
                contents: None,
            }));
        };
        let fields = match packagejson::parse(fh.content().as_bytes()) {
            Ok(fields) => fields,
            Err(_) => {
                return Some(Rc::new(packagejson::InfoCacheEntry {
                    directory_exists: true,
                    package_directory: tspath::get_directory_path(file_name),
                    contents: Some(Rc::new(packagejson::PackageJson {
                        parseable: false,
                        ..Default::default()
                    })),
                }));
            }
        };
        Some(Rc::new(packagejson::InfoCacheEntry {
            directory_exists: true,
            package_directory: tspath::get_directory_path(file_name),
            contents: Some(Rc::new(packagejson::PackageJson {
                fields,
                parseable: true,
                ..Default::default()
            })),
        }))
    }

    // Go: project/autoimport.go:152 GetSourceFile
    // GetSourceFile implements autoimport.RegistryCloneHost.
    // PORT: Go `*ast.SourceFile` is the file root `Node` (`file.root`).
    fn get_source_file(&self, file_name: &str, path: &tspath::Path) -> Node {
        let Some(fh) = self.fs.get_file(file_name) else {
            return Node::NIL;
        };
        // ts#64159: the file takes the handle's name (autoimport.go:149).
        let opts = parser::SourceFileParseOptions {
            file_name: fh.file_name(),
            path: path.clone(),
            ..Default::default()
        };
        let key = new_parse_cache_key(&opts, fh.hash(), fh.kind());
        let result = self.parse_cache.acquire(key.clone(), fh);

        // Go: a.filesMu.Lock() / Unlock() (PORT: no lock).
        self.files.borrow_mut().push(key);

        result.file.root
    }

    // Go: project/autoimport.go:172 Dispose
    // Dispose implements autoimport.RegistryCloneHost.
    // PORT: Go derefs every key, and its GC frees the parses. Here a file
    // that the clone parsed is kept for good anyway: the alias resolver
    // publishes it (`program::publish_parsed_files`) and binds it into the
    // binder lineage. A released entry only makes the next clone parse, bind
    // and keep the same file again. An idle warm that the next edit cancels
    // then kept about 870 node_modules files (about 155 MiB) per edit. So the
    // session keeps one reference per path, for the newest key, and a newer
    // key for the same path releases the older one. A later clone gets the
    // same parse from the cache. A parse depends only on its key, so no
    // result changes.
    fn dispose(&self) {
        // Go: a.filesMu.Lock(); defer a.filesMu.Unlock() (PORT: no lock).
        let mut kept = self.kept_files.borrow_mut();
        for key in self.files.take() {
            // For the same key, this releases the clone's extra reference.
            if let Some(older) = kept.insert(key.path.clone(), key) {
                self.parse_cache.deref(&older);
            }
        }
    }
}
