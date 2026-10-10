//! Go `internal/project/project.go`.
//!
//! PORT: one thread (project/dirty/interfaces.rs). Go `*Project` is
//! `Rc<RefCell<Project>>` (the dirty maps change it through the pointer).
//! Go `*compiler.Program` is `Rc<compiler::NewProgram>` (nil is
//! `None`). Go `*tsoptions.ParsedCommandLine` is
//! `Option<Rc<tsoptions::ParsedCommandLine>>`; Go pointer equality is
//! `Rc::ptr_eq`.

use crate::project::prelude::*;

use crate::contentmapper;
use crate::frontend::compiler::CompilerHost as _;
use crate::frontend::core_ext::{ProjectReference, TypeAcquisition};
use crate::frontend::module;
use crate::frontend::vfs::Fs as _;
use crate::program::ls_program;
use std::cell::Cell;

// Go: project/project.go:27 inferredProjectName
pub const INFERRED_PROJECT_NAME: &str = "/dev/null/inferred"; // lowercase so toPath is a no-op regardless of settings
// Go: project/project.go:28 syntheticProjectPrefix (ts#64204)
pub const SYNTHETIC_PROJECT_PREFIX: &str = "/dev/null/synthetic/";
// Go: project/project.go:29 hr
pub const HR: &str = "-----------------------------------------------";

// Go: project/project.go:32 ID (ts#64319)
// PORT: Go `type ID string`. The derived `Ord` is Go's string order
// (`cmp.Compare`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ID(pub String);

// Go: project/project.go:34 ConfiguredProjectID (ts#64319)
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ConfiguredProjectID(pub tspath::Path);

impl ConfiguredProjectID {
    // Go: project/project.go:36 ConfiguredProjectID.Path (at 673a5f17d713; ts#64159
    // renames it PathKey, project.go:38)
    pub fn path(&self) -> tspath::Path {
        self.0.clone()
    }

    // Go: project/project.go:57 ConfiguredProjectID.AsID
    pub fn as_id(&self) -> ID {
        ID(self.0.0.clone())
    }
}

// Go: project/project.go:42 InferredProjectID (ts#64319)
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct InferredProjectID(pub String);

// Go: project/project.go:44 inferredProjectID
// PORT: Go `const inferredProjectID InferredProjectID = inferredProjectName`.
pub fn inferred_project_id() -> InferredProjectID {
    InferredProjectID(INFERRED_PROJECT_NAME.to_string())
}

impl InferredProjectID {
    // Go: project/project.go:58 InferredProjectID.AsID
    pub fn as_id(&self) -> ID {
        ID(self.0.clone())
    }
}

// Go: project/project.go:46 SyntheticProjectID (ts#64319)
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SyntheticProjectID(pub String);

// Go: project/project.go:48 NewSyntheticProjectID (ts#64319)
pub fn new_synthetic_project_id(id: i32) -> SyntheticProjectID {
    if id <= 0 {
        crate::core::go_panic(format!("invalid synthetic project ID: {id}"));
    }
    SyntheticProjectID(format!("{SYNTHETIC_PROJECT_PREFIX}{id}"))
}

impl SyntheticProjectID {
    // Go: project/project.go:59 SyntheticProjectID.AsID
    pub fn as_id(&self) -> ID {
        ID(self.0.clone())
    }
}

/// Go `%s` of an ID type is its string.
impl std::fmt::Display for ID {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::fmt::Display for ConfiguredProjectID {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0.0)
    }
}

impl std::fmt::Display for SyntheticProjectID {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl ID {
    // Go: project/project.go:55 ID.String
    pub fn string(&self) -> String {
        self.0.clone()
    }

    // Go: project/project.go:61 ID.Configured
    pub fn configured(&self) -> (ConfiguredProjectID, bool) {
        parse_configured_project_id(&tspath::Path(self.0.clone()))
    }

    // Go: project/project.go:91 ID.Inferred
    pub fn inferred(&self) -> (InferredProjectID, bool) {
        (
            inferred_project_id(),
            *self == inferred_project_id().as_id(),
        )
    }

    // Go: project/project.go:95 ID.Synthetic
    pub fn synthetic(&self) -> (SyntheticProjectID, bool) {
        parse_synthetic_project_id(&self.0)
    }

    /// Go: an `ID` passed as an `autoimport.ProjectID` (the Go interface holds
    /// the `ID`; the Rust type holds its string).
    // PORT: not in Go.
    pub fn as_auto_import_project_id(&self) -> autoimport::ProjectID {
        autoimport::ProjectID(self.0.clone())
    }
}

// PORT: Go passes an `ID` as an `ata.ProjectID` (`fmt.Stringer`).
impl ata::ProjectID for ID {
    fn string(&self) -> String {
        self.0.clone()
    }
}

// Go: project/project.go:69 ParseConfiguredProjectID (ts#64319)
pub fn parse_configured_project_id(value: &tspath::Path) -> (ConfiguredProjectID, bool) {
    let id = ID(value.0.clone());
    if id.0.is_empty() {
        return (ConfiguredProjectID::default(), false);
    }
    if id.inferred().1 {
        return (ConfiguredProjectID::default(), false);
    }
    if id.synthetic().1 {
        return (ConfiguredProjectID::default(), false);
    }
    (ConfiguredProjectID(value.clone()), true)
}

// Go: project/project.go:99 SyntheticProjectID.UnmarshalJSONFrom (ts#64319)
// PORT: the JSON impls of the project ID types are in src/api/proto.rs, with
// the other API JSON impls.

// Go: project/project.go:112 ParseSyntheticProjectID (ts#64319)
// PORT: Go `strconv.Atoi` accepts a leading sign; `str::parse::<i32>` does
// too.
pub fn parse_synthetic_project_id(value: &str) -> (SyntheticProjectID, bool) {
    let Some(suffix) = value.strip_prefix(SYNTHETIC_PROJECT_PREFIX) else {
        return (SyntheticProjectID::default(), false);
    };
    match suffix.parse::<i32>() {
        Ok(id) if id > 0 => (new_synthetic_project_id(id), true),
        _ => (SyntheticProjectID::default(), false),
    }
}

// Go: project/project.go:126 Kind
// PORT: Go `type Kind int` with iota consts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Kind(pub i32);

impl Kind {
    // Go: project/project.go:129 KindInferred
    pub const INFERRED: Kind = Kind(0);
    // Go: project/project.go:130 KindConfigured
    pub const CONFIGURED: Kind = Kind(1);
    // Go: project/project.go:131 KindSynthetic (ts#64204)
    pub const SYNTHETIC: Kind = Kind(2);
}

// Go: project/project.go:134 ProgramUpdateKind
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ProgramUpdateKind(pub i32);

impl ProgramUpdateKind {
    // Go: project/project.go:137 ProgramUpdateKindNone
    pub const NONE: ProgramUpdateKind = ProgramUpdateKind(0);
    // Go: project/project.go:138 ProgramUpdateKindCloned
    pub const CLONED: ProgramUpdateKind = ProgramUpdateKind(1);
    // Go: project/project.go:139 ProgramUpdateKindSameFileNames
    pub const SAME_FILE_NAMES: ProgramUpdateKind = ProgramUpdateKind(2);
    // Go: project/project.go:140 ProgramUpdateKindNewFiles
    pub const NEW_FILES: ProgramUpdateKind = ProgramUpdateKind(3);
}

// Go: project/project.go:143 PendingReload
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PendingReload(pub i32);

impl PendingReload {
    // Go: project/project.go:146 PendingReloadNone
    pub const NONE: PendingReload = PendingReload(0);
    // Go: project/project.go:147 PendingReloadFileNames
    pub const FILE_NAMES: PendingReload = PendingReload(1);
    // Go: project/project.go:148 PendingReloadFull
    pub const FULL: PendingReload = PendingReload(2);
}

// Go: project/project.go:153 Project
// Project represents a TypeScript project.
// If changing struct fields, also update the Clone method.
// PORT: `commandLineWithTypingsFiles` and its `sync.Once` are written by
// `getCommandLineWithTypingsFiles`, which `CreateProgram` calls; they are a
// `RefCell` and a `Cell<bool>` so both take `&self`. Go
// `*collections.Set[tspath.Path]` is `Option<Rc<FxHashSet<..>>>` (Go
// replaces it with a clone before adding). Go
// `*collections.SyncSet[tspath.Path]` (the program files watch input) is
// `Option<Rc<RefCell<FxHashSet<..>>>>`, the sourceFS seen-files set.
#[derive(Default)]
pub struct Project {
    pub kind: Kind,
    // ts#64319
    pub id: ID,
    pub current_directory: String,
    pub config_file_name: String,
    pub config_file_path: tspath::Path,

    pub dirty: bool,
    pub dirty_file_path: tspath::Path,

    pub host: Option<Rc<CompilerHost>>,
    pub command_line: Option<Rc<tsoptions::ParsedCommandLine>>,
    pub command_line_with_typings_files: RefCell<Option<Rc<tsoptions::ParsedCommandLine>>>,
    pub command_line_with_typings_files_once: Cell<bool>,
    pub program: Option<Rc<compiler::NewProgram>>,
    // The kind of update that was performed on the program last time it was updated.
    pub program_update_kind: ProgramUpdateKind,
    // The ID of the snapshot that created the program stored in this project.
    pub program_last_update: u64,
    // Set of projects that this project could be referencing.
    // Only set before actually loading config file to get actual project references
    pub potential_project_references: Option<Rc<FxHashSet<tspath::Path>>>,

    pub program_files_watch: Option<Rc<WatchedFiles<Option<SeenFiles>>>>,
    pub typings_watch: Option<Rc<WatchedFiles<PatternsAndIgnored>>>,
    // tsgo#4712. PORT: Go `*collections.Set[tspath.Path]` (nil until the
    // first program) is `Option<Rc<FxHashSet<..>>>`.
    pub content_mapper_watch: Option<Rc<WatchedFiles<Vec<String>>>>,
    pub content_mapper_watched_files: Option<Rc<FxHashSet<tspath::Path>>>,

    pub checker_pool: Option<Rc<CheckerPool>>,
    // Not in Go: the parse cache entries that `program` holds a count on
    // (`ProgramFileRefs`). Set with `program`.
    pub program_file_refs: Option<Rc<ProgramFileRefs>>,

    // ts#64299. PORT: a Go nil factory is `None`.
    pub module_resolver_factory: Option<Rc<dyn ModuleResolverFactory>>,
    pub module_resolver_id: u64,

    // installedTypingsInfo is the value of `project.ComputeTypingsInfo()` that was
    // used during the most recently completed typings installation.
    pub installed_typings_info: Option<Rc<ata::TypingsInfo>>,
    // typingsFiles are the root files added by the typings installer.
    pub typings_files: Vec<String>,
}

// Go: project/project.go:195 NewConfiguredProject
// PORT: Go `*ProjectCollectionBuilder` is only read here, so it is a borrow.
pub fn new_configured_project(
    config_file_name: &str,
    config_file_path: &tspath::Path,
    builder: &ProjectCollectionBuilder,
    logger: Option<Rc<logging::LogTree>>,
) -> Rc<RefCell<Project>> {
    // ts#64319
    let (configured_project_id, ok) = parse_configured_project_id(config_file_path);
    if !ok {
        crate::core::go_panic(format!("invalid configured project ID: {config_file_path}"));
    }
    let project = new_project(
        configured_project_id.as_id(),
        Kind::CONFIGURED,
        &tspath::get_directory_path(config_file_name),
        builder,
        logger,
    );
    {
        let mut p = project.borrow_mut();
        p.config_file_name = config_file_name.to_string();
        p.config_file_path = config_file_path.clone();
    }
    project
}

// Go: project/project.go:211 NewInferredProject
// PORT: Go `*core.CompilerOptions` (nil-able) is `Option<Rc<CompilerOptions>>`.
// PORT: Go `[]*core.ProjectReference` is `Option<Vec<ProjectReference>>`
// (the `ParsedOptions` field type; nil is `None`).
#[allow(clippy::too_many_arguments)]
pub fn new_inferred_project(
    current_directory: &str,
    compiler_options: Option<Rc<CompilerOptions>>,
    root_file_names: &[String],
    project_references: Option<Vec<ProjectReference>>,
    content_mappers: &[Rc<contentmapper::Mapper>],
    builder: &ProjectCollectionBuilder,
    logger: Option<Rc<logging::LogTree>>,
) -> Rc<RefCell<Project>> {
    let p = new_project(
        inferred_project_id().as_id(),
        Kind::INFERRED,
        current_directory,
        builder,
        logger,
    );
    let compiler_options = match compiler_options {
        Some(compiler_options) => compiler_options,
        None => Rc::new(CompilerOptions {
            allow_js: Tristate::True,
            module: ModuleKind::ES_NEXT,
            module_resolution: ModuleResolutionKind::BUNDLER,
            target: ScriptTarget::LATEST_STANDARD,
            jsx: JsxEmit::REACT_JSX,
            allow_importing_ts_extensions: Tristate::True,
            strict_null_checks: Tristate::True,
            strict_function_types: Tristate::True,
            source_map: Tristate::True,
            allow_non_ts_extensions: Tristate::True,
            resolve_json_module: Tristate::True,
            ..Default::default()
        }),
    };
    let command_line = new_inferred_project_command_line(
        compiler_options,
        root_file_names.to_vec(),
        project_references,
        content_mappers,
        tspath::ComparePathsOptions {
            use_case_sensitive_file_names: builder.fs.fs.use_case_sensitive_file_names(),
            current_directory: current_directory.to_string(),
        },
    );
    p.borrow_mut().command_line = Some(Rc::new(command_line));
    p
}

// Go: project/project.go:247 newSyntheticProject (ts#64204)
// PORT: Go `*core.CompilerOptions` can be nil; the Rust command line needs a
// value, so a nil one is the zero options.
#[allow(clippy::too_many_arguments)]
pub fn new_synthetic_project(
    id: SyntheticProjectID,
    current_directory: &str,
    compiler_options: Option<Rc<CompilerOptions>>,
    root_file_names: Vec<String>,
    project_references: Option<Vec<ProjectReference>>,
    content_mappers: &[Rc<contentmapper::Mapper>],
    builder: &ProjectCollectionBuilder,
    logger: Option<Rc<logging::LogTree>>,
) -> Rc<RefCell<Project>> {
    let project = new_project(
        id.as_id(),
        Kind::SYNTHETIC,
        current_directory,
        builder,
        logger,
    );
    let command_line = new_inferred_project_command_line(
        compiler_options.unwrap_or_default(),
        root_file_names,
        project_references,
        content_mappers,
        tspath::ComparePathsOptions {
            use_case_sensitive_file_names: builder.fs.fs.use_case_sensitive_file_names(),
            current_directory: current_directory.to_string(),
        },
    );
    project.borrow_mut().command_line = Some(Rc::new(command_line));
    project
}

// Go: project/project.go:269 newInferredProjectCommandLine (tsgo#4712)
pub fn new_inferred_project_command_line(
    compiler_options: Rc<CompilerOptions>,
    root_file_names: Vec<String>,
    project_references: Option<Vec<ProjectReference>>,
    content_mappers: &[Rc<contentmapper::Mapper>],
    compare_paths_options: tspath::ComparePathsOptions,
) -> tsoptions::ParsedCommandLine {
    let mut command_line = tsoptions::new_parsed_command_line(
        compiler_options,
        root_file_names,
        project_references,
        compare_paths_options,
    );
    command_line.parsed_config.content_mappers = content_mappers.to_vec();
    command_line
}

// Go: project/project.go:282 NewProject
// ts#64319: takes the project ID; a configured project sets its config file
// name and path after (NewConfiguredProject).
pub fn new_project(
    id: ID,
    kind: Kind,
    current_directory: &str,
    builder: &ProjectCollectionBuilder,
    logger: Option<Rc<logging::LogTree>>,
) -> Rc<RefCell<Project>> {
    if logger.is_some() {
        logger.log(&format!(
            "Creating {}Project: {}, currentDirectory: {}",
            kind.string(),
            id,
            current_directory
        ));
    }
    let mut project = Project {
        kind,
        id: id.clone(),
        current_directory: current_directory.to_string(),
        dirty: true,
        ..Default::default()
    };

    project.program_files_watch = Some(new_watched_files(
        &format!("program files for {id}"),
        lsproto::WatchKind(
            lsproto::WatchKind::CREATE.0
                | lsproto::WatchKind::CHANGE.0
                | lsproto::WatchKind::DELETE.0,
        ),
        lsproto::get_client_capabilities(&builder.ctx)
            .workspace
            .did_change_watched_files
            .relative_pattern_support,
        create_resolution_lookup_glob_mapper(
            &builder.session_options.current_directory,
            &builder.session_options.default_library_path,
            &project.current_directory,
            builder.fs.fs.use_case_sensitive_file_names(),
        ),
    ));
    if !builder.session_options.typings_location.is_empty() {
        // Go: core.Identity
        let identity: Rc<dyn Fn(&PatternsAndIgnored) -> PatternsAndIgnored> =
            Rc::new(|p: &PatternsAndIgnored| p.clone());
        project.typings_watch = Some(new_watched_files(
            "typings installer files",
            lsproto::WatchKind(
                lsproto::WatchKind::CREATE.0
                    | lsproto::WatchKind::CHANGE.0
                    | lsproto::WatchKind::DELETE.0,
            ),
            lsproto::get_client_capabilities(&builder.ctx)
                .workspace
                .did_change_watched_files
                .relative_pattern_support,
            identity,
        ));
    }
    project.content_mapper_watch = Some(new_watched_files_for_paths(
        &format!("content mapper configuration files for {id}"),
        lsproto::WatchKind(
            lsproto::WatchKind::CREATE.0
                | lsproto::WatchKind::CHANGE.0
                | lsproto::WatchKind::DELETE.0,
        ),
        lsproto::get_client_capabilities(&builder.ctx)
            .workspace
            .did_change_watched_files
            .relative_pattern_support,
        &builder.session_options.current_directory,
        &builder.session_options.current_directory,
        builder.fs.fs.use_case_sensitive_file_names(),
    ));
    Rc::new(RefCell::new(project))
}

impl Project {
    // Go: project/project.go:328 Project.CurrentDirectory (ts#63935)
    pub fn current_directory(&self) -> String {
        self.current_directory.clone()
    }

    // Go: project/project.go:336 Project.DisplayName
    // DisplayName returns a short, human-readable name for the project,
    // relative to the given workspace root directory.
    // For configured projects, this is the config file path made relative.
    // For inferred projects, this is the last component of the current directory.
    pub fn display_name(&self, cwd: &str) -> String {
        if self.kind == Kind::INFERRED {
            return tspath::get_base_file_name(&self.current_directory);
        }
        // ts#64319
        let mut name = self.id().0;
        if self.kind == Kind::CONFIGURED {
            name = self.config_file_name();
        }
        // ts#64159: a name on another root stays absolute (R4).
        match tspath::relative_path_from_directory(cwd, &name, false) {
            Some(relative_path) => relative_path,
            None => name,
        }
    }

    // Go: project/project.go:350 Project.ID (ts#64319: the project ID)
    // PORT: Go also has `Id()` (the `ls.Project` method, same snake name);
    // it is the `ls::Project` impl below and returns the ID's string.
    pub fn id(&self) -> ID {
        self.id.clone()
    }

    // Go: project/project.go:355 Project.ConfigFileName
    // ConfigFileName panics if Kind() is not KindConfigured.
    pub fn config_file_name(&self) -> String {
        if self.kind != Kind::CONFIGURED {
            crate::core::go_panic("ConfigFileName called on non-configured project".to_string());
        }
        self.config_file_name.clone()
    }

    // Go: project/project.go:363 Project.ConfigFilePath
    // ConfigFilePath panics if Kind() is not KindConfigured.
    pub fn config_file_path(&self) -> tspath::Path {
        if self.kind != Kind::CONFIGURED {
            crate::core::go_panic("ConfigFilePath called on non-configured project".to_string());
        }
        self.config_file_path.clone()
    }

    // Go: project/project.go:374 Project.GetProgram
    pub fn get_program(&self) -> Option<Rc<compiler::NewProgram>> {
        self.program.clone()
    }

    // Go: project/project.go:378 Project.IsDirty (ts#64204)
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    // Go: project/project.go:385 Project.GetProjectDiagnostics
    // GetProjectDiagnostics returns program diagnostics combined with any global
    // diagnostics discovered during checking. These are the diagnostics reported on
    // the tsconfig.json file.
    pub fn get_project_diagnostics(&self, _ctx: &Context) -> Vec<Diagnostic> {
        let mut global_diags: Vec<Diagnostic> = Vec::new();
        if let Some(checker_pool) = &self.checker_pool {
            global_diags = checker_pool.get_global_diagnostics();
        }
        let program = self
            .program
            .as_deref()
            .unwrap_or_else(|| crate::core::go_nil_dereference());
        // Go: slices.Concat
        let mut diagnostics = program.get_config_file_parsing_diagnostics();
        diagnostics.extend(ls_program::get_program_diagnostics(program));
        diagnostics.extend(global_diags);
        sort_and_deduplicate_diagnostics(diagnostics)
    }

    // Go: project/project.go:397 Project.HasFile
    pub fn has_file(&self, file_name: &str) -> bool {
        self.contains_file(&self.to_path(file_name))
    }

    // Go: project/project.go:401 Project.containsFile
    pub fn contains_file(&self, path: &tspath::Path) -> bool {
        self.program
            .as_ref()
            .is_some_and(|program| program.get_source_file_by_path(path).is_some())
    }

    // Go: project/project.go:405 Project.IsSourceFromProjectReference
    pub fn is_source_from_project_reference(&self, path: &tspath::Path) -> bool {
        self.program
            .as_ref()
            .is_some_and(|program| program.is_source_from_project_reference(path))
    }

    // Go: project/project.go:409 Project.Clone
    // PORT: Go `Clone()` is `clone_` (dirty decision 3). The `sync.Once` is
    // not copied (Go leaves the zero value).
    pub fn clone_(&self) -> Rc<RefCell<Project>> {
        Rc::new(RefCell::new(Project {
            kind: self.kind,
            id: self.id.clone(),
            current_directory: self.current_directory.clone(),
            config_file_name: self.config_file_name.clone(),
            config_file_path: self.config_file_path.clone(),

            dirty: self.dirty,
            dirty_file_path: self.dirty_file_path.clone(),

            host: self.host.clone(),
            command_line: self.command_line.clone(),
            command_line_with_typings_files: RefCell::new(
                self.command_line_with_typings_files.borrow().clone(),
            ),
            command_line_with_typings_files_once: Cell::new(false),
            program: self.program.clone(),
            program_update_kind: ProgramUpdateKind::NONE,
            program_last_update: self.program_last_update,
            potential_project_references: self.potential_project_references.clone(),

            program_files_watch: self.program_files_watch.clone(),
            typings_watch: self.typings_watch.clone(),
            content_mapper_watch: self.content_mapper_watch.clone(),
            content_mapper_watched_files: self.content_mapper_watched_files.clone(),

            checker_pool: self.checker_pool.clone(),
            program_file_refs: self.program_file_refs.clone(),

            module_resolver_factory: self.module_resolver_factory.clone(),
            module_resolver_id: self.module_resolver_id,

            installed_typings_info: self.installed_typings_info.clone(),
            typings_files: self.typings_files.clone(),
        }))
    }

    // Go: project/project.go:450 Project.SetCommandLine
    // SetCommandLine reassigns the project's command line and resets all state derived
    // from it. Changing the command line always requires a full program rebuild, so the
    // project is marked fully dirty. It also resets:
    //   - the memoized command line augmented with typings files (and its sync.Once, so
    //     the augmented command line is rebuilt from the new command line on next access);
    //   - potentialProjectReferences, the pre-load placeholder derived from the old
    //     command line (always nil for inferred projects, which have no project references).
    pub fn set_command_line(&mut self, command_line: Option<Rc<tsoptions::ParsedCommandLine>>) {
        self.command_line = command_line;
        *self.command_line_with_typings_files.borrow_mut() = None;
        self.command_line_with_typings_files_once = Cell::new(false);
        self.potential_project_references = None;
        self.dirty = true;
        self.dirty_file_path = tspath::Path::default();
    }

    // Go: project/project.go:460 Project.getCommandLineWithTypingsFiles
    // getCommandLineWithTypingsFiles returns the command line augmented with typing files if ATA is enabled.
    pub fn get_command_line_with_typings_files(&self) -> Option<Rc<tsoptions::ParsedCommandLine>> {
        if self.typings_files.is_empty() {
            return self.command_line.clone();
        }

        // Check if ATA is enabled for this project
        let type_acquisition = self.get_type_acquisition();
        match &type_acquisition {
            Some(type_acquisition) if type_acquisition.enable.is_true() => {}
            _ => return self.command_line.clone(),
        }

        // Go: p.commandLineWithTypingsFilesOnce.Do(..)
        if !self.command_line_with_typings_files_once.replace(true)
            && self.command_line_with_typings_files.borrow().is_none()
        {
            let command_line = self
                .command_line
                .as_ref()
                .unwrap_or_else(|| crate::core::go_nil_dereference());
            // Create an augmented command line that includes typing files
            let original_root_names = command_line.file_names();
            let mut new_root_names: Vec<String> =
                Vec::with_capacity(original_root_names.len() + self.typings_files.len());
            new_root_names.extend_from_slice(original_root_names);
            new_root_names.extend_from_slice(&self.typings_files);

            // tsgo#4712
            let augmented = command_line.with_file_names(new_root_names);
            *self.command_line_with_typings_files.borrow_mut() = Some(Rc::new(augmented));
        }
        self.command_line_with_typings_files.borrow().clone()
    }

    // Go: project/project.go:485 Project.setPotentialProjectReference
    pub fn set_potential_project_reference(&mut self, config_file_path: &tspath::Path) {
        let mut potential_project_references = match &self.potential_project_references {
            None => FxHashSet::default(),
            // Go: p.potentialProjectReferences.Clone()
            Some(references) => (**references).clone(),
        };
        potential_project_references.insert(config_file_path.clone());
        self.potential_project_references = Some(Rc::new(potential_project_references));
    }

    // Go: project/project.go:494 Project.hasPotentialProjectReference
    pub fn has_potential_project_reference(
        &self,
        project_tree_request: &ProjectTreeRequest,
    ) -> bool {
        if let Some(command_line) = &self.command_line {
            for path in command_line.resolved_project_reference_paths() {
                if project_tree_request.is_project_referenced(&self.to_path(path)) {
                    return true;
                }
            }
        } else if let Some(potential_project_references) = &self.potential_project_references {
            for path in potential_project_references.iter() {
                if project_tree_request.is_project_referenced(path) {
                    return true;
                }
            }
        }
        false
    }

    // Go: project/project.go:516 Project.CreateProgram
    // PORT: Go `compiler.NewProgram(opts)` is `ls_program::new_program(opts,
    // create_checker_pool)` and `p.Program.UpdateProgram(..)` is
    // `ls_program::update_program(p.Program, ..)` (contract C3). Go
    // `ProgramOptions.CreateCheckerPool` is their last argument.
    pub fn create_program(&self) -> CreateProgramResult {
        let mut update_kind = ProgramUpdateKind::NEW_FILES;
        let mut program_cloned = false;
        let new_program: Rc<compiler::NewProgram>;
        let mut file_refs: Option<ProgramFileRefs> = None;

        let host = self
            .host
            .clone()
            .unwrap_or_else(|| crate::core::go_nil_dereference());

        // PORT: Go reads `p.host.sessionOptions.CheckerPoolOptions` when the
        // closure runs; the session options never change, so the value is
        // copied here. Go passes the method value `p.log`, whose body is
        // empty (`// !!!`); a closure that holds the project would make a
        // reference cycle (project -> program -> pool -> project), so the
        // pool gets an empty closure. Go reads the pool back with
        // `result.Program.GetCheckerPool().(*checkerPool)`; a Rust trait
        // object can not be downcast, so the closure also keeps the pool it
        // made and `CreateProgramResult.checker_pool` returns it.
        let created_checker_pool: Rc<RefCell<Option<Rc<CheckerPool>>>> =
            Rc::new(RefCell::new(None));
        let create_checker_pool: ls_program::CreateCheckerPool = {
            let checker_pool_options = host.session_options.checker_pool_options.clone();
            let created_checker_pool = created_checker_pool.clone();
            Rc::new(
                move |program: &Rc<compiler::NewProgram>| -> Rc<dyn ls_program::CheckerPool> {
                    let log: Rc<dyn Fn(&str)> = Rc::new(|_msg: &str| {
                        // Go: p.log(msg) (empty body)
                    });
                    let pool = new_checker_pool(
                        checker_pool_options.clone(),
                        Rc::clone(program),
                        Some(log),
                    );
                    *created_checker_pool.borrow_mut() = Some(pool.clone());
                    pool
                },
            )
        };

        // ts#64299
        // PORT: Go always passes `createModuleResolver`; without a factory it
        // returns `module.NewResolver(options)`, which is the loader's own
        // default. The port passes `None` then, so the loader keeps its
        // default resolver and its parse-worker resolution (PERF, see
        // `file_loader.rs`). Go `cleanupModuleResolver` is the shared cell;
        // Go's `defer` runs it at the end of this function.
        let cleanup_module_resolver: Rc<RefCell<Option<Box<dyn FnOnce()>>>> =
            Rc::new(RefCell::new(None));
        let create_module_resolver: Option<
            Rc<dyn Fn(module::ResolverOptions) -> Rc<dyn module::Resolver>>,
        > = self.module_resolver_factory.clone().map(|factory| {
            let cleanup_module_resolver = cleanup_module_resolver.clone();
            // Go: p.host.builder.ctx (ts#64519, project.go:529 at fed0bf24149f)
            // PORT: read once here; the builder does not change while this
            // function runs, and the closure runs only inside it.
            let ctx = host
                .builder
                .borrow()
                .as_ref()
                .unwrap_or_else(|| crate::core::go_nil_dereference())
                .ctx
                .clone();
            let create: Rc<dyn Fn(module::ResolverOptions) -> Rc<dyn module::Resolver>> =
                Rc::new(move |options: module::ResolverOptions| {
                    let (resolver, cleanup) = factory.new_resolver(&ctx, options);
                    *cleanup_module_resolver.borrow_mut() = Some(cleanup);
                    resolver
                });
            create
        });

        // Create the command line, potentially augmented with typing files
        let command_line = self.get_command_line_with_typings_files();

        let same_command_line = match (&self.program, &command_line) {
            (Some(program), Some(command_line)) => Rc::ptr_eq(program.command_line(), command_line),
            _ => false,
        };
        if !self.dirty_file_path.is_empty() && self.program.is_some() && same_command_line {
            let program = self.program.as_deref().expect("checked above");
            let host_rc: Rc<dyn compiler::CompilerHost> = host.clone();
            let (updated_program, dirty_file, cloned) = ls_program::update_program(
                program,
                &self.dirty_file_path,
                host_rc,
                Some(create_checker_pool.clone()),
                create_module_resolver.clone(),
            );
            new_program = updated_program;
            program_cloned = cloned;
            // Go: p.host.builder (read in each branch below)
            let builder = || {
                host.builder
                    .borrow()
                    .clone()
                    .unwrap_or_else(|| crate::core::go_nil_dereference())
            };
            if program_cloned {
                update_kind = ProgramUpdateKind::CLONED;
                let builder = builder();
                // UpdateProgram acquired the changed file only, so we need to ref everything else
                // PORT: through the old program's entries (`ProgramFileRefs`),
                // so only the changed files build a key.
                let old = self
                    .program_file_refs
                    .as_deref()
                    .map(|old_refs| (program.source_files(), old_refs));
                file_refs = Some(ProgramFileRefs::new(
                    &builder.parse_cache,
                    &builder.content_mapped_parse_cache,
                    new_program.source_files(),
                    true,
                    dirty_file.as_ref(),
                    old,
                ));
                for file in new_program.duplicate_source_files() {
                    if !file.is_content_mapper_failure_stub {
                        if !file.content_mapper.is_empty() {
                            builder
                                .content_mapped_parse_cache
                                .ref_(&content_mapped_parse_cache_key_for_duplicate(file));
                        } else {
                            ref_program_file(
                                &builder.parse_cache,
                                &file.parse_options,
                                file.source_hash(),
                                file.script_kind,
                            );
                        }
                    }
                }
            } else if let Some(dirty_file) = &dirty_file {
                // UpdateProgram always acquires the dirty file before deciding whether it can
                // reuse the old program. If it falls back to a full rebuild, release that
                // speculative acquire so the rebuilt program is the only remaining owner.
                if !dirty_file.content_mapper().is_empty() {
                    deref_content_mapped_file(
                        &builder().content_mapped_parse_cache,
                        &content_mapped_parse_cache_key_for_file(dirty_file),
                    );
                } else {
                    deref_program_file(
                        &builder().parse_cache,
                        dirty_file.parse_options(),
                        dirty_file.source_hash(),
                        dirty_file.script_kind,
                    );
                }
            }
        } else {
            let mut typings_location = String::new();
            let type_acquisition = self
                .get_type_acquisition()
                .unwrap_or_else(|| crate::core::go_nil_dereference());
            if type_acquisition.enable.is_true() {
                typings_location = host.session_options.typings_location.clone();
            }
            let host_rc: Rc<dyn compiler::CompilerHost> = host.clone();
            new_program = ls_program::new_program(
                compiler::ProgramOptions {
                    host: host_rc,
                    config: command_line.unwrap_or_else(|| crate::core::go_nil_dereference()),
                    use_source_of_project_reference: true,
                    single_threaded: Tristate::Unknown,
                    typings_location,
                    project_name: String::new(),
                    // ts#64299
                    create_module_resolver: create_module_resolver.clone(),
                    // ts#64024: Go leaves `SkipModuleResolution` zero here.
                    skip_module_resolution: false,
                },
                Some(create_checker_pool),
            );
        }

        if !program_cloned
            && self
                .program
                .as_ref()
                .is_some_and(|program| program.has_same_file_names(&new_program))
        {
            update_kind = ProgramUpdateKind::SAME_FILE_NAMES;
        }

        ls_program::bind_source_files(&new_program);

        // Go: defer cleanupModuleResolver() (ts#64299)
        let cleanup = cleanup_module_resolver.borrow_mut().take();
        if let Some(cleanup) = cleanup {
            cleanup();
        }

        // Not in Go: a program that is not cloned holds its counts through
        // its own loads; collect their entries.
        let file_refs = file_refs.unwrap_or_else(|| {
            let builder = host
                .builder
                .borrow()
                .clone()
                .unwrap_or_else(|| crate::core::go_nil_dereference());
            ProgramFileRefs::new(
                &builder.parse_cache,
                &builder.content_mapped_parse_cache,
                new_program.source_files(),
                false,
                None,
                None,
            )
        });

        let checker_pool = created_checker_pool.borrow().clone();
        CreateProgramResult {
            program: new_program,
            update_kind,
            checker_pool,
            file_refs: Rc::new(file_refs),
        }
    }

    // Go: project/project.go:606 Project.CloneWatchers
    pub fn clone_watchers(&self) -> Option<Rc<WatchedFiles<Option<SeenFiles>>>> {
        let host = self
            .host
            .as_ref()
            .unwrap_or_else(|| crate::core::go_nil_dereference());
        let seen_files = host.source_fs.seen_files.borrow().clone();
        WatchedFiles::clone_(self.program_files_watch.as_deref(), seen_files)
    }

    // Go: project/project.go:610 Project.log
    pub fn log(&self, _msg: &str) {
        // !!!
    }

    // Go: project/project.go:601 Project.toPath (at 673a5f17d713; removed by ts#64159)
    pub fn to_path(&self, file_name: &str) -> tspath::Path {
        let host = self
            .host
            .as_ref()
            .unwrap_or_else(|| crate::core::go_nil_dereference());
        tspath::to_path(
            file_name,
            &self.current_directory,
            host.fs().use_case_sensitive_file_names(),
        )
    }

    // Go: project/project.go:614 Project.print
    pub fn print(
        &self,
        write_file_names: bool,
        _write_file_explanation: bool,
        builder: &mut String,
    ) -> String {
        builder.push_str(&format!("\nProject '{}'\n", self.id()));
        match &self.program {
            None => {
                builder.push_str("\tFiles (0) NoProgram\n");
            }
            Some(program) => {
                let source_files = program.get_source_files();
                builder.push_str(&format!("\tFiles ({})\n", source_files.len()));
                if write_file_names {
                    for source_file in source_files {
                        builder.push_str("\t\t");
                        builder.push_str(source_file.file_name());
                        builder.push('\n');
                    }
                    // !!!
                    // if writeFileExplanation {}
                }
            }
        }
        builder.push_str(HR);
        builder.clone()
    }

    // Go: project/project.go:636 Project.GetTypeAcquisition
    // GetTypeAcquisition returns the type acquisition settings for this project.
    // PORT: Go returns a pointer (nil-able); the command line's value is
    // copied into a new `Rc`.
    pub fn get_type_acquisition(&self) -> Option<Rc<TypeAcquisition>> {
        if self.kind == Kind::INFERRED || self.kind == Kind::SYNTHETIC {
            // For inferred and synthetic projects, use default settings.
            return Some(Rc::new(TypeAcquisition {
                enable: Tristate::True,
                include: Vec::new(),
                exclude: Vec::new(),
                disable_filename_based_type_acquisition: Tristate::False,
            }));
        }

        if let Some(command_line) = &self.command_line {
            return command_line
                .type_acquisition()
                .map(|type_acquisition| Rc::new(type_acquisition.clone()));
        }

        None
    }

    // Go: project/project.go:655 Project.GetUnresolvedImports
    // GetUnresolvedImports extracts unresolved imports from this project's program.
    // PORT: Go returns the program's cached set; the port copies it into an `Rc`.
    pub fn get_unresolved_imports(&self) -> Option<Rc<FxHashSet<String>>> {
        let program = self.program.as_ref()?;

        Some(Rc::new(program.get_unresolved_imports().clone()))
    }

    // Go: project/project.go:664 Project.ShouldTriggerATA
    // ShouldTriggerATA determines if ATA should be triggered for this project.
    pub fn should_trigger_ata(&self, snapshot_id: u64) -> bool {
        if self.program.is_none() || self.command_line.is_none() {
            return false;
        }

        let type_acquisition = self.get_type_acquisition();
        match &type_acquisition {
            Some(type_acquisition) if type_acquisition.enable.is_true() => {}
            _ => return false,
        }

        let Some(installed_typings_info) = &self.installed_typings_info else {
            return true;
        };
        if self.program_last_update == snapshot_id
            && self.program_update_kind == ProgramUpdateKind::NEW_FILES
        {
            return true;
        }

        !installed_typings_info.equals(&self.compute_typings_info())
    }

    // Go: project/project.go:681 Project.ComputeTypingsInfo
    pub fn compute_typings_info(&self) -> ata::TypingsInfo {
        ata::TypingsInfo {
            // Go: p.CommandLine.CompilerOptions() (nil receiver gives nil)
            compiler_options: self
                .command_line
                .as_ref()
                .map(|command_line| command_line.compiler_options().clone()),
            type_acquisition: self.get_type_acquisition(),
            unresolved_imports: self.get_unresolved_imports(),
        }
    }
}

// Go: project/project.go:409 Project.Clone (dirty.Cloneable)
impl dirty::Cloneable for Rc<RefCell<Project>> {
    fn clone_(&self) -> Self {
        self.borrow().clone_()
    }
}

// Go: project/project.go:89 `var _ ls.Project = (*Project)(nil)`
impl ls::Project for Project {
    // Go: project/project.go:370 Project.Id (ts#64319: the ID string)
    fn id(&self) -> String {
        Project::id(self).0
    }

    // Go: project/project.go:374 Project.GetProgram
    fn get_program(&self) -> Option<Rc<compiler::NewProgram>> {
        Project::get_program(self)
    }

    // Go: project/project.go:397 Project.HasFile
    fn has_file(&self, file_name: &str) -> bool {
        Project::has_file(self, file_name)
    }
}

// PORT: projects are shared as `Rc<RefCell<Project>>`; this impl lets them
// coerce to `Rc<dyn ls::Project>` (Go passes the `*Project`).
impl ls::Project for RefCell<Project> {
    fn id(&self) -> String {
        ls::Project::id(&*self.borrow())
    }

    fn get_program(&self) -> Option<Rc<compiler::NewProgram>> {
        ls::Project::get_program(&*self.borrow())
    }

    fn has_file(&self, file_name: &str) -> bool {
        ls::Project::has_file(&*self.borrow(), file_name)
    }
}

// Go: project/project.go:511 CreateProgramResult
// PORT: `checker_pool` is the pool that the CreateCheckerPool closure made
// for `program` (Go `result.Program.GetCheckerPool().(*checkerPool)`).
#[derive(Clone)]
pub struct CreateProgramResult {
    pub program: Rc<compiler::NewProgram>,
    pub update_kind: ProgramUpdateKind,
    pub checker_pool: Option<Rc<CheckerPool>>,
    // Not in Go: the parse cache entries that `program` holds a count on.
    pub file_refs: Rc<ProgramFileRefs>,
}
