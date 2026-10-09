//! Port of Go `internal/api/module_resolution.go` (ts#64299): module
//! resolution overrides for API programs.
//!
//! PORT: Go `module.Resolver` is an interface at N (the program lane ports it
//! as the trait `module::Resolver`); a resolver value is
//! `Rc<dyn module::Resolver>`. Go `project.ModuleResolverFactory` is the
//! trait `project::ModuleResolverFactory`. The api lane notes for the
//! program and server lanes give the exact signatures this file calls.
//!
//! PORT: Go keeps `session *Session` in the factory to register and release
//! program resolution contexts. A Rust `&Session` cannot be kept, so the
//! registry of contexts is a shared `Rc<ProgramResolutionContexts>` that the
//! session and the factories hold.

use crate::api::prelude::*;

use crate::api::proto;
use crate::frontend::json_ext::AnyValue;
use crate::frontend::module;
use crate::gostd::errors;
use crate::program::PackageId as ModulePackageId;
// PORT: the explicit import picks Go `module.ResolvedModule` over the api
// `proto::ResolvedModule` of the api prelude.
use crate::program::ResolvedModule;
use std::cell::Cell;
use std::sync::Arc;

// Go: api/session.go moduleResolverRegistration (ts#64299)
pub struct ModuleResolverRegistration {
    pub id: ModuleResolverID,
    pub compiler_options: Rc<CompilerOptions>,
    pub resolutions: Option<Rc<module::StaticResolutions>>,
    pub resolve_module_name_callback: String,
}

// Go: api/module_resolution.go moduleResolverFactory
pub struct ModuleResolverFactoryImpl {
    registration: Rc<ModuleResolverRegistration>,
    contexts: Rc<ProgramResolutionContexts>,
    conn: Option<Rc<dyn ipc::Conn>>,
    ctx: Context,
    current_directory: String,
}

// Go: api/module_resolution.go programResolutionContext
// PORT: Go `mu` is not ported (one thread).
pub struct ProgramResolutionContext {
    options: module::ResolverOptions,
    resolvers: RefCell<FxHashMap<ModuleResolverID, Rc<dyn module::Resolver>>>,
}

/// The Go session fields `nextProgramResolutionContextID` and
/// `programResolutionContexts` (see the file header).
#[derive(Default)]
pub struct ProgramResolutionContexts {
    pub next_id: Cell<u64>,
    pub contexts: RefCell<FxHashMap<u64, Rc<ProgramResolutionContext>>>,
}

// Go: api/module_resolution.go callbackModuleResolver
pub struct CallbackModuleResolver {
    registration: Rc<ModuleResolverRegistration>,
    conn: Option<Rc<dyn ipc::Conn>>,
    ctx: Context,
    current_directory: String,
    snapshot: SnapshotID,
    program_resolution_context_id: u64,
    fallback_resolver: Rc<dyn module::Resolver>,
}

// Go: api/module_resolution.go moduleResolverFactory.NewResolver
impl project::ModuleResolverFactory for ModuleResolverFactoryImpl {
    fn new_resolver(
        &self,
        mut options: module::ResolverOptions,
    ) -> (Rc<dyn module::Resolver>, Box<dyn FnOnce()>) {
        options.compiler_options = Some(self.registration.compiler_options.clone());
        let mut fallback: Rc<dyn module::Resolver> = Rc::new(module::new_resolver(options.clone()));
        if self.registration.resolve_module_name_callback.is_empty() {
            if let Some(resolutions) = &self.registration.resolutions {
                fallback = Rc::new(module::new_static_resolver(fallback, resolutions.clone()));
            }
            return (fallback, Box::new(|| {}));
        }
        let context_id = self.contexts.register_program_resolution_context(
            fallback.clone(),
            options,
            &self.registration,
        );
        let mut resolver: Rc<dyn module::Resolver> = Rc::new(CallbackModuleResolver {
            registration: self.registration.clone(),
            conn: self.conn.clone(),
            ctx: self.ctx.clone(),
            current_directory: self.current_directory.clone(),
            snapshot: SnapshotID(0),
            program_resolution_context_id: context_id,
            fallback_resolver: fallback,
        });
        if let Some(resolutions) = &self.registration.resolutions {
            resolver = Rc::new(module::new_static_resolver(resolver, resolutions.clone()));
        }
        let contexts = self.contexts.clone();
        (
            resolver,
            Box::new(move || contexts.release_program_resolution_context(context_id)),
        )
    }
}

// Go: api/module_resolution.go callbackModuleResolver (module.Resolver)
impl module::Resolver for CallbackModuleResolver {
    // Go: api/module_resolution.go callbackModuleResolver.ResolveModuleName
    fn resolve_module_name(
        &self,
        module_name: &str,
        containing_file: &str,
        resolution_mode: ModuleKind,
        redirected_reference: Option<&dyn module::ModuleResolvedProjectReference>,
    ) -> (
        Option<Arc<ResolvedModule>>,
        Vec<module::DiagAndArgs>,
        Option<GoError>,
    ) {
        self.resolve_module_name_worker(
            module_name,
            containing_file,
            &tspath::get_directory_path(containing_file),
            resolution_mode,
            redirected_reference,
        )
    }

    // Go: api/module_resolution.go callbackModuleResolver.ResolveModuleNameFromDirectory
    fn resolve_module_name_from_directory(
        &self,
        module_name: &str,
        containing_directory: &str,
        resolution_mode: ModuleKind,
    ) -> (
        Option<Arc<ResolvedModule>>,
        Vec<module::DiagAndArgs>,
        Option<GoError>,
    ) {
        self.resolve_module_name_worker(
            module_name,
            containing_directory,
            containing_directory,
            resolution_mode,
            None,
        )
    }

    // Go: api/module_resolution.go callbackModuleResolver.ResolveTypeReferenceDirective
    fn resolve_type_reference_directive(
        &self,
        type_reference_directive_name: &str,
        containing_file: &str,
        resolution_mode: ModuleKind,
        redirected_reference: Option<&dyn module::ModuleResolvedProjectReference>,
    ) -> (
        Rc<module::ResolvedTypeReferenceDirective>,
        Vec<module::DiagAndArgs>,
    ) {
        self.fallback_resolver.resolve_type_reference_directive(
            type_reference_directive_name,
            containing_file,
            resolution_mode,
            redirected_reference,
        )
    }

    // Go: api/module_resolution.go callbackModuleResolver.GetPackageScopeForPath
    fn get_package_scope_for_path(
        &self,
        directory: &str,
    ) -> Option<Rc<crate::frontend::packagejson::InfoCacheEntry>> {
        self.fallback_resolver.get_package_scope_for_path(directory)
    }

    // Go: api/module_resolution.go callbackModuleResolver.PackageJsonCacheEntries
    fn package_json_cache_entries(
        &self,
        f: &mut dyn FnMut(
            &tspath::Path,
            crate::frontend::module::cache::PackageJsonCacheEntry<'_>,
        ) -> bool,
    ) {
        self.fallback_resolver.package_json_cache_entries(f);
    }

    // Go: api/module_resolution.go callbackModuleResolver.ResolvePackageDirectory
    fn resolve_package_directory(
        &self,
        module_name: &str,
        containing_file: &str,
        resolution_mode: ModuleKind,
        redirected_reference: Option<&dyn module::ModuleResolvedProjectReference>,
    ) -> Option<ResolvedModule> {
        self.fallback_resolver.resolve_package_directory(
            module_name,
            containing_file,
            resolution_mode,
            redirected_reference,
        )
    }

    // PORT: the Rust-only `release_caches` of `module::Resolver` (program
    // lane): a wrapper forwards it.
    fn release_caches(&self) {
        self.fallback_resolver.release_caches();
    }
}

impl CallbackModuleResolver {
    // Go: api/module_resolution.go callbackModuleResolver.resolveModuleName
    // PORT: named `_worker`; the trait method takes the Go exported name.
    fn resolve_module_name_worker(
        &self,
        module_name: &str,
        _containing_file: &str,
        containing_directory: &str,
        resolution_mode: ModuleKind,
        _redirected_reference: Option<&dyn module::ModuleResolvedProjectReference>,
    ) -> (
        Option<Arc<ResolvedModule>>,
        Vec<module::DiagAndArgs>,
        Option<GoError>,
    ) {
        let mut params = ResolveModuleNameCallbackParams {
            module_name: module_name.to_string(),
            containing_directory: containing_directory.to_string(),
            ..Default::default()
        };
        if self.snapshot.0 != 0 {
            params.snapshot = Some(self.snapshot);
        }
        if self.program_resolution_context_id != 0 {
            params.in_progress_snapshot = Some(self.program_resolution_context_id);
        }
        params.resolution_mode = Some(proto::ResolutionMode(resolution_mode.0));
        // PORT: Go calls a nil `conn` and panics; the factory refuses to
        // make a callback resolver without one.
        let conn = self
            .conn
            .as_ref()
            .expect("runtime error: invalid memory address or nil pointer dereference");
        let callback_result = match conn.call(
            &self.ctx,
            &self.registration.resolve_module_name_callback,
            Some(Box::new(params)),
        ) {
            Ok(result) => result,
            Err(err) => {
                return (
                    None,
                    Vec::new(),
                    Some(errors::errorf(
                        format!("resolveModuleName callback failed: {}", err.error()),
                        vec![err],
                    )),
                );
            }
        };

        if callback_result.0.is_empty() || callback_result.0 == b"null" {
            return (None, Vec::new(), None);
        }
        let mut static_resolution = StaticModuleResolution::default();
        // Go `json.Unmarshal`, with the v2 error texts.
        if let Err(err) =
            crate::frontend::json_ext::unmarshal_root(&callback_result.0, &mut static_resolution)
        {
            let err = errors::from_value(err);
            return (
                None,
                Vec::new(),
                Some(errors::errorf(
                    format!("invalid resolveModuleName callback result: {}", err.error()),
                    vec![err],
                )),
            );
        }
        (
            static_module_resolution_to_resolved_module(
                Some(&static_resolution),
                &self.current_directory,
            )
            .map(Arc::new),
            Vec::new(),
            None,
        )
    }
}

// Go: api/module_resolution.go compileModuleResolutionSpec
pub fn compile_module_resolution_spec(
    spec: Option<&ModuleResolutionSpec>,
    current_directory: &str,
    use_case_sensitive: bool,
) -> Result<Option<Rc<module::StaticResolutions>>, GoError> {
    let Some(spec) = spec else {
        return Ok(None);
    };
    let fallback_to_resolution = if spec.fallback == ModuleResolutionFallback::RESOLVE {
        true
    } else if spec.fallback == ModuleResolutionFallback::UNRESOLVED {
        false
    } else {
        return Err(errors::errorf(
            format!(
                "{}: invalid module resolution fallback {}",
                *ERR_CLIENT_ERROR,
                gostd::strconv::quote(&spec.fallback.0)
            ),
            vec![ERR_CLIENT_ERROR.clone()],
        ));
    };

    let mut entries: Vec<module::StaticResolutionEntry> = Vec::with_capacity(spec.entries.len());
    for (i, entry) in spec.entries.iter().enumerate() {
        let Some(entry) = entry else {
            return Err(errors::errorf(
                format!("{}: module resolution entry {i} is null", *ERR_CLIENT_ERROR),
                vec![ERR_CLIENT_ERROR.clone()],
            ));
        };
        if entry.module_name.is_empty() {
            return Err(errors::errorf(
                format!(
                    "{}: module resolution entry {i} has an empty moduleName",
                    *ERR_CLIENT_ERROR
                ),
                vec![ERR_CLIENT_ERROR.clone()],
            ));
        }
        let Some(result) = &entry.result else {
            return Err(errors::errorf(
                format!(
                    "{}: module resolution entry {i} has no result",
                    *ERR_CLIENT_ERROR
                ),
                vec![ERR_CLIENT_ERROR.clone()],
            ));
        };

        let mut static_entry = module::StaticResolutionEntry {
            module_name: entry.module_name.clone(),
            ..Default::default()
        };
        if let Some(containing_directory) = &entry.containing_directory {
            static_entry.containing_directory = tspath::get_normalized_absolute_path(
                &containing_directory.to_file_name(current_directory),
                current_directory,
            );
        }
        if let Some(resolution_mode) = entry.resolution_mode {
            let mode = ModuleKind(resolution_mode.0);
            if mode != ModuleKind::NONE
                && mode != ModuleKind::COMMON_JS
                && mode != ModuleKind::ES_NEXT
            {
                return Err(errors::errorf(
                    format!(
                        "{}: module resolution entry {i} has invalid resolutionMode {}",
                        *ERR_CLIENT_ERROR,
                        mode.string()
                    ),
                    vec![ERR_CLIENT_ERROR.clone()],
                ));
            }
            static_entry.resolution_mode = Some(mode);
        }
        static_entry.result =
            static_module_resolution_to_resolved_module(Some(result), current_directory)
                .map(Arc::new);
        entries.push(static_entry);
    }

    match module::new_static_resolutions(
        &entries,
        fallback_to_resolution,
        current_directory,
        use_case_sensitive,
    ) {
        Ok(resolutions) => Ok(Some(Rc::new(resolutions))),
        Err(err) => Err(errors::errorf(
            format!("{}: {}", *ERR_CLIENT_ERROR, err.error()),
            vec![ERR_CLIENT_ERROR.clone(), err],
        )),
    }
}

// Go: api/module_resolution.go staticModuleResolutionToResolvedModule
pub fn static_module_resolution_to_resolved_module(
    static_resolution: Option<&StaticModuleResolution>,
    current_directory: &str,
) -> Option<ResolvedModule> {
    let static_resolution = static_resolution?;
    let resolved_file_name = static_resolution.resolved_file_name.as_ref()?;
    let mut result = ResolvedModule {
        resolved_file_name: tspath::get_normalized_absolute_path(
            &resolved_file_name.to_file_name(current_directory),
            current_directory,
        ),
        ..Default::default()
    };
    if let Some(original_path) = &static_resolution.original_path {
        result.original_path = tspath::get_normalized_absolute_path(
            &original_path.to_file_name(current_directory),
            current_directory,
        );
    }
    if let Some(package_id) = &static_resolution.package_id {
        result.package_id = ModulePackageId {
            name: package_id.name.clone(),
            sub_module_name: package_id.sub_module_name.clone(),
            version: package_id.version.clone(),
            peer_dependencies: package_id.peer_dependencies.clone(),
        };
    }
    let original_path = if result.original_path.is_empty() {
        result.resolved_file_name.clone()
    } else {
        result.original_path.clone()
    };
    result.extension = tspath::try_get_extension_from_path(&result.resolved_file_name).to_string();
    result.is_external_library_import = original_path.contains("/node_modules/");
    Some(result)
}

// Go: api/module_resolution.go moduleResolutionTraceToStrings
pub fn module_resolution_trace_to_strings(trace: &[module::DiagAndArgs]) -> Vec<String> {
    trace
        .iter()
        .map(|entry| {
            crate::diagnostics_loc::localize(&locale::DEFAULT, Some(entry.message), "", &entry.args)
        })
        .collect()
}

impl Session {
    // Go: api/module_resolution.go Session.moduleResolverFactory
    pub fn module_resolver_factory(
        &self,
        ctx: &Context,
        options: &CreateProgramOptions,
    ) -> Result<Option<Rc<dyn project::ModuleResolverFactory>>, GoError> {
        if options.module_resolver.0 == 0 {
            return Ok(None);
        }
        let data = self
            .module_resolvers
            .borrow()
            .get(&options.module_resolver)
            .cloned();
        let Some(data) = data else {
            return Err(errors::errorf(
                format!(
                    "{}: module resolver {} not found",
                    *ERR_CLIENT_ERROR, options.module_resolver.0
                ),
                vec![ERR_CLIENT_ERROR.clone()],
            ));
        };
        let conn = self.conn.borrow().clone();
        if !data.resolve_module_name_callback.is_empty() && conn.is_none() {
            return Err(errors::errorf(
                format!("{}: API connection is not initialized", *ERR_CLIENT_ERROR),
                vec![ERR_CLIENT_ERROR.clone()],
            ));
        }
        Ok(Some(Rc::new(ModuleResolverFactoryImpl {
            registration: data,
            contexts: self.program_resolution_contexts.clone(),
            conn,
            ctx: ctx.clone(),
            current_directory: self.get_current_directory(),
        })))
    }
}

impl ProgramResolutionContexts {
    // Go: api/module_resolution.go Session.registerProgramResolutionContext
    pub fn register_program_resolution_context(
        &self,
        resolver: Rc<dyn module::Resolver>,
        options: module::ResolverOptions,
        registration: &ModuleResolverRegistration,
    ) -> u64 {
        self.next_id.set(self.next_id.get() + 1);
        let id = self.next_id.get();
        let mut resolvers: FxHashMap<ModuleResolverID, Rc<dyn module::Resolver>> =
            FxHashMap::default();
        resolvers.insert(registration.id, resolver);
        self.contexts.borrow_mut().insert(
            id,
            Rc::new(ProgramResolutionContext {
                options,
                resolvers: RefCell::new(resolvers),
            }),
        );
        id
    }

    // Go: api/module_resolution.go Session.releaseProgramResolutionContext
    pub fn release_program_resolution_context(&self, id: u64) {
        self.contexts.borrow_mut().remove(&id);
    }
}

impl ProgramResolutionContext {
    // Go: api/module_resolution.go programResolutionContext.resolverFor
    pub fn resolver_for(
        &self,
        registration: &ModuleResolverRegistration,
    ) -> Rc<dyn module::Resolver> {
        if let Some(resolver) = self.resolvers.borrow().get(&registration.id) {
            return resolver.clone();
        }
        let mut options = self.options.clone();
        options.compiler_options = Some(registration.compiler_options.clone());
        let resolver: Rc<dyn module::Resolver> = Rc::new(module::new_resolver(options));
        self.resolvers
            .borrow_mut()
            .insert(registration.id, resolver.clone());
        resolver
    }
}

// Go: api/module_resolution.go moduleResolutionError
pub fn module_resolution_error(snapshot: &project::Snapshot) -> Option<GoError> {
    for project in snapshot.project_collection.projects() {
        let program = project.borrow().get_program();
        if let Some(program) = program
            && let Some(err) = program.module_resolution_error()
        {
            return Some(err);
        }
    }
    None
}

/// Go passes the api `Session` (`FS`, `GetCurrentDirectory`) or a
/// `*project.Snapshot` as a `module.ResolutionHost`.
// PORT: the Rust trait returns borrowed values, so this host keeps the file
// system and directory that the Go methods return at the call.
struct ApiResolutionHost {
    fs: Rc<dyn vfs::Fs>,
    current_directory: String,
}

impl module::ResolutionHost for ApiResolutionHost {
    fn fs(&self) -> &dyn vfs::Fs {
        &*self.fs
    }

    fn get_current_directory(&self) -> &str {
        &self.current_directory
    }
}

impl Session {
    // Go: api/module_resolution.go Session.handleCreateModuleResolver
    pub fn handle_create_module_resolver(
        &self,
        params: &CreateModuleResolverParams,
    ) -> Result<ModuleResolverID, GoError> {
        let provider = compile_module_resolution_spec(
            params.module_resolutions.as_ref(),
            &self.get_current_directory(),
            self.fs().use_case_sensitive_file_names(),
        )?;
        self.next_module_resolver_id
            .set(self.next_module_resolver_id.get() + 1);
        let id = ModuleResolverID(self.next_module_resolver_id.get());
        let data = Rc::new(ModuleResolverRegistration {
            id,
            compiler_options: Rc::new(params.compiler_options.clone()),
            resolutions: provider,
            resolve_module_name_callback: params.resolve_module_name_callback.clone(),
        });
        self.module_resolvers.borrow_mut().insert(id, data);
        Ok(id)
    }

    // Go: api/module_resolution.go Session.handleReleaseModuleResolver
    pub fn handle_release_module_resolver(
        &self,
        params: &ReleaseModuleResolverParams,
    ) -> Result<Option<Box<dyn AnyValue>>, GoError> {
        let ok = self
            .module_resolvers
            .borrow_mut()
            .remove(&params.resolver)
            .is_some();
        if !ok {
            return Err(errors::errorf(
                format!(
                    "{}: module resolver {} not found",
                    *ERR_CLIENT_ERROR, params.resolver.0
                ),
                vec![ERR_CLIENT_ERROR.clone()],
            ));
        }
        Ok(None)
    }

    // Go: api/module_resolution.go Session.handleResolveModuleName
    pub fn handle_resolve_module_name(
        &self,
        ctx: &Context,
        params: &ResolveModuleNameParams,
    ) -> Result<ResolveModuleNameResult, GoError> {
        if params.module_name.is_empty() {
            return Err(errors::errorf(
                format!("{}: moduleName is empty", *ERR_CLIENT_ERROR),
                vec![ERR_CLIENT_ERROR.clone()],
            ));
        }
        let data = self
            .module_resolvers
            .borrow()
            .get(&params.resolver)
            .cloned();
        let Some(data) = data else {
            return Err(errors::errorf(
                format!(
                    "{}: module resolver {} not found",
                    *ERR_CLIENT_ERROR, params.resolver.0
                ),
                vec![ERR_CLIENT_ERROR.clone()],
            ));
        };

        let mut mode = RESOLUTION_MODE_NONE;
        if let Some(resolution_mode) = params.resolution_mode {
            mode = ModuleKind(resolution_mode.0);
            if mode != RESOLUTION_MODE_NONE
                && mode != RESOLUTION_MODE_COMMON_JS
                && mode != RESOLUTION_MODE_ESM
            {
                return Err(errors::errorf(
                    format!(
                        "{}: invalid resolutionMode {}",
                        *ERR_CLIENT_ERROR,
                        mode.string()
                    ),
                    vec![ERR_CLIENT_ERROR.clone()],
                ));
            }
        }
        let cwd = self.get_current_directory();
        let containing_directory = tspath::get_normalized_absolute_path(
            &params.containing_directory.to_file_name(&cwd),
            &cwd,
        );

        let mut resolver: Rc<dyn module::Resolver>;
        if params.snapshot.0 != 0 && params.in_progress_snapshot != 0 {
            return Err(errors::errorf(
                format!(
                    "{}: snapshot and inProgressSnapshot are mutually exclusive",
                    *ERR_CLIENT_ERROR
                ),
                vec![ERR_CLIENT_ERROR.clone()],
            ));
        }
        if params.in_progress_snapshot != 0 {
            let resolution_context = self
                .program_resolution_contexts
                .contexts
                .borrow()
                .get(&params.in_progress_snapshot)
                .cloned();
            let Some(resolution_context) = resolution_context else {
                return Err(errors::errorf(
                    format!(
                        "{}: in-progress snapshot {} not found",
                        *ERR_CLIENT_ERROR, params.in_progress_snapshot
                    ),
                    vec![ERR_CLIENT_ERROR.clone()],
                ));
            };
            resolver = resolution_context.resolver_for(&data);
        } else if params.snapshot.0 != 0 {
            let sd = self.get_snapshot_data(params.snapshot)?;
            resolver = Rc::new(module::new_resolver(module::ResolverOptions {
                host: Some(Rc::new(ApiResolutionHost {
                    fs: sd.snapshot.fs(),
                    current_directory: sd.snapshot.get_current_directory(),
                })),
                compiler_options: Some(data.compiler_options.clone()),
                extra_extensions: sd.snapshot.content_mapper_extensions(),
                ..Default::default()
            }));
        } else {
            resolver = Rc::new(module::new_resolver(module::ResolverOptions {
                host: Some(Rc::new(ApiResolutionHost {
                    fs: self.fs(),
                    current_directory: cwd.clone(),
                })),
                compiler_options: Some(data.compiler_options.clone()),
                ..Default::default()
            }));
        }
        if !data.resolve_module_name_callback.is_empty() {
            resolver = Rc::new(CallbackModuleResolver {
                registration: data.clone(),
                conn: self.conn.borrow().clone(),
                ctx: ctx.clone(),
                current_directory: cwd.clone(),
                snapshot: params.snapshot,
                program_resolution_context_id: params.in_progress_snapshot,
                fallback_resolver: resolver,
            });
        }
        if let Some(resolutions) = &data.resolutions {
            resolver = Rc::new(module::new_static_resolver(resolver, resolutions.clone()));
        }
        let (result, trace, err) = resolver.resolve_module_name_from_directory(
            &params.module_name,
            &containing_directory,
            mode,
        );
        if let Some(err) = err {
            return Err(err);
        }
        Ok(ResolveModuleNameResult {
            resolved_module: new_resolved_module_response(result.as_deref()),
            trace: module_resolution_trace_to_strings(&trace),
        })
    }
}
