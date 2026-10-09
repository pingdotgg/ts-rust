//! Go `internal/module/resolver.go` lines 1 to 1152 (unit U18).
//!
//! The `resolved` result, the tracer, `resolutionState`, `Resolver` and the
//! `resolutionState` methods from `resolveTypeReferenceDirective` to
//! `loadModuleFromSpecificNodeModulesDirectory`. `resolver_p2.rs` has the rest.
//!
//! PORT: shapes shared with `resolver_p2.rs`:
//! - Go `*resolved` is `Option<Resolved>`. `nil` (`continueSearching()`) is
//!   `None`. `shouldContinueSearching()` is `is_none()`.
//! - Go `*tracer` is `Option<Rc<RefCell<Tracer>>>`. The resolver shares it
//!   with the state. `trace_write!` keeps the Go `if r.tracer != nil` form.
//! - Go `*packagejson.InfoCacheEntry` is `Option<Rc<InfoCacheEntry>>`. Go
//!   embedding is nesting (`contents.fields.path_fields.exports.json_value`).
//! - Go `*core.CompilerOptions` is `Rc<CompilerOptions>`. The state compares
//!   it by pointer with the resolver options.
//! - Go `*ResolvedModule` results are owned values. The resolver caches
//!   hold them as `Rc`.

use crate::frontend::prelude::*;
use crate::gostd::GoError;
use std::borrow::Cow;
use std::sync::Arc;

/// Go `if r.tracer != nil { r.tracer.write(diag, args...) }`.
macro_rules! trace_write {
    ($state:expr, $msg:expr $(, $arg:expr)* $(,)?) => {
        if let Some(tracer) = &$state.tracer {
            tracer.borrow_mut().write($msg, args![$($arg),*]);
        }
    };
}

/// Go `(*resolved).isResolved` on a possibly nil `*resolved`.
fn resolved_is_resolved(resolved: &Option<Resolved>) -> bool {
    resolved.as_ref().is_some_and(|r| !r.path.is_empty())
}

/// Go `(*packagejson.InfoCacheEntry).Exists` on a possibly nil entry.
fn entry_exists(info: &Option<Rc<InfoCacheEntry>>) -> bool {
    info.as_ref().is_some_and(|p| p.exists())
}

// Go: module/resolver.go:20 resolved
#[derive(Clone, Debug, Default)]
pub struct Resolved {
    pub path: String,
    pub extension: String,
    pub package_id: PackageId,
    pub original_path: String,
    pub resolved_using_ts_extension: bool,
    // tsgo#4712
    pub resolved_using_extra_extensions: bool,
}

// Go: module/resolver.go:29 resolved.shouldContinueSearching
// PORT: `Option::is_none` on `Option<Resolved>`.

// Go: module/resolver.go:33 resolved.isResolved
// PORT: the private `resolved_is_resolved` helper.

// Go: module/resolver.go:37 continueSearching
#[must_use]
pub fn continue_searching() -> Option<Resolved> {
    None
}

// Go: module/resolver.go:41 unresolved
#[must_use]
pub fn unresolved() -> Option<Resolved> {
    Some(Resolved::default())
}

// Go: module/resolver.go:130 pathForDynamicResolution (ts#64544)
// A relative module name under an encoded dynamic directory, with its
// segments encoded as the dynamic file names are.
pub(crate) fn path_for_dynamic_resolution<'a>(
    directory: &str,
    path: &'a str,
    directory_only: bool,
) -> Cow<'a, str> {
    if is_encoded_dynamic_file_name(directory) && !path_is_absolute(path) {
        if directory_only {
            return Cow::Owned(encode_dynamic_directory_specifier(path));
        }
        return Cow::Owned(encode_dynamic_module_specifier(path));
    }
    Cow::Borrowed(path)
}

// Go: module/resolver.go:54 resolvePathForModule (ts#64544 at 59f5b0233; ts#64159
// replaces it with resolutionCandidate)
// `path` resolved against `directory`; a directory path ends with "/".
pub(crate) fn resolve_path_for_module(directory: &str, path: &str, directory_only: bool) -> String {
    let resolved = normalize_path(&combine_paths(
        directory,
        &[&path_for_dynamic_resolution(
            directory,
            path,
            directory_only,
        )],
    ));
    if directory_only {
        return ensure_trailing_directory_separator(&resolved);
    }
    resolved
}

// Go: module/resolver.go:62 resolveDynamicLogicalPath (ts#64544 at 59f5b0233; removed by
// ts#64159)
pub(crate) fn resolve_dynamic_logical_path(
    directory: &str,
    path: &str,
    directory_only: bool,
) -> String {
    let path = if !path.is_empty() && directory_only {
        remove_trailing_directory_separator(path)
    } else {
        path
    };
    let encoded = if directory_only {
        encode_dynamic_relative_uri_directory_path(path)
    } else {
        encode_dynamic_relative_uri_path(path)
    };
    let resolved = normalize_path(&combine_paths(directory, &[&encoded]));
    if directory_only {
        return ensure_trailing_directory_separator(&resolved);
    }
    resolved
}

// Go: module/resolver.go:140 resolutionCandidateFromRelativePath and :100
// resolutionCandidateFromDiskLogicalPath (ts#64159)
// `path` relative to `directory`, appended as written (a rooted-looking
// segment such as "c:" stays a segment). An empty path is the directory. A
// directory candidate ends with "/".
pub(crate) fn candidate_from_relative_path(
    directory: &str,
    path: &str,
    directory_only: bool,
) -> String {
    let path = if !path.is_empty() && directory_only {
        remove_trailing_directory_separator(path)
    } else {
        path
    };
    if path.is_empty() {
        return ensure_trailing_directory_separator(directory);
    }
    // Go RootedDirectoryPath.ResolveRelativeFile: a parent-relative path is
    // resolved, any other is appended.
    let resolved = if path == ".." || path.starts_with("../") {
        get_normalized_absolute_path(path, directory)
    } else if has_trailing_directory_separator(directory) {
        format!("{directory}{path}")
    } else {
        format!("{directory}/{path}")
    };
    if directory_only {
        return ensure_trailing_directory_separator(&resolved);
    }
    resolved
}

// Go: module/resolver.go:83 resolutionCandidateFromDynamicLogicalPath (ts#64159)
// `resolve_dynamic_logical_path`, except that an empty path is the directory.
pub(crate) fn candidate_from_dynamic_logical_path(directory: &str, path: &str) -> String {
    if path.is_empty() {
        return ensure_trailing_directory_separator(directory);
    }
    resolve_dynamic_logical_path(directory, path, has_trailing_directory_separator(path))
}

// Go: module/resolver.go:77 dynamicDirectoryCandidate (ts#64544 at 59f5b0233; ts#64159
// replaces it with resolutionCandidate.directoryPath)
// A dynamic candidate whose last segment is encoded as a directory segment
// (a file segment keeps its extension outside the escape).
pub(crate) fn dynamic_directory_candidate(candidate: &str) -> Cow<'_, str> {
    if !is_encoded_dynamic_file_name(candidate) {
        return Cow::Borrowed(candidate);
    }
    let directory = get_directory_path(candidate);
    let base = get_base_file_name(candidate);
    let logical_base = decode_dynamic_uri_path_segment(&base);
    let encoded_base = encode_dynamic_uri_directory_path(&logical_base);
    if encoded_base == base {
        return Cow::Borrowed(candidate);
    }
    Cow::Owned(combine_paths(&directory, &[&encoded_base]))
}

// Go: module/resolver.go:48 resolutionCandidate (ts#64159)
// PORT: the port keeps a candidate as one string, and a trailing separator
// is Go's `directoryOnly`. `candidate_path` is Go's `path` field: the name
// without that separator (a root keeps it, resolutionCandidateFromNormalized
// :54).
pub(crate) fn candidate_path(candidate: &str) -> &str {
    if candidate.len() > get_root_length(candidate) {
        return remove_trailing_directory_separator(candidate);
    }
    candidate
}

// Go: module/resolver.go:165 resolutionCandidate.AsDirectoryPath (ts#64159)
// A directory-only candidate is already encoded as a directory; another
// candidate uses its dynamic directory spelling (`dynamic_directory_candidate`).
pub(crate) fn candidate_directory_path(candidate: &str) -> Cow<'_, str> {
    if has_trailing_directory_separator(candidate) {
        return Cow::Borrowed(candidate_path(candidate));
    }
    dynamic_directory_candidate(candidate)
}

// Go: module/resolver.go:223 resolutionKindSpecificLoader
// PORT: Go closures capture `r`. Here the loader gets the state as its first
// argument, so the caller can keep `&mut self`.
pub type ResolutionKindSpecificLoader<'s> =
    dyn FnMut(&mut ResolutionState<'s>, Extensions, &str) -> Option<Resolved>;

// Go: module/resolver.go:225 tracer
#[derive(Clone, Debug, Default)]
pub struct Tracer {
    pub traces: Vec<DiagAndArgs>,
}

// Go: module/resolver.go:229 DiagAndArgs
// PORT: Go `Args []any` is `Vec<String>`. Values are formatted with
// `ToString` when written (`args!`), like the Go `%v` formatting later.
#[derive(Clone, Debug)]
pub struct DiagAndArgs {
    pub message: &'static crate::diagnostics::Message,
    pub args: Vec<String>,
}

impl Tracer {
    // Go: module/resolver.go:234 tracer.write
    // PORT: the Go nil check is on the caller side (`trace_write!`).
    pub fn write(&mut self, diag: &'static crate::diagnostics::Message, args: Vec<String>) {
        self.traces.push(DiagAndArgs {
            message: diag,
            args,
        });
    }

    // Go: module/resolver.go:240 tracer.getTraces
    // PORT: a nil tracer gives an empty `Vec` (see `traces_of`).
    #[must_use]
    pub fn get_traces(&self) -> Vec<DiagAndArgs> {
        self.traces.clone()
    }
}

/// Go `traceBuilder.getTraces()` on a possibly nil `*tracer`.
fn traces_of(tracer: &Option<Rc<RefCell<Tracer>>>) -> Vec<DiagAndArgs> {
    match tracer {
        Some(tracer) => tracer.borrow().get_traces(),
        None => Vec::new(),
    }
}

// Go: module/resolver.go:247 resolutionState
pub struct ResolutionState<'a> {
    pub resolver: &'a DefaultResolver,
    pub tracer: Option<Rc<RefCell<Tracer>>>,

    // request fields
    pub name: String,
    pub containing_directory: String,
    pub is_config_lookup: bool,
    pub features: NodeResolutionFeatures,
    pub esm_mode: bool,
    pub conditions: Vec<String>,
    pub extensions: Extensions,
    pub compiler_options: Rc<CompilerOptions>,
    pub resolve_package_directory_only: bool,

    // state fields
    // candidateEndingIsFromConfig is set when the candidate file extension originated from
    // configuration (package.json fields, tsconfig.json paths entries, or wildcard substitutions)
    // rather than from the module specifier written in source code. When true, resolvedUsingTsExtension
    // is suppressed so the checker does not attempt to extract a TS extension from the original specifier.
    pub candidate_ending_is_from_config: bool,
    pub resolved_package_directory: bool,
    pub diagnostics: Vec<Diagnostic>,
}

impl<'a> ResolutionState<'a> {
    // PORT: Go `&resolutionState{compilerOptions: ..., resolver: r}` and the
    // zero fields of the other composite literals.
    fn zero(
        resolver: &'a DefaultResolver,
        compiler_options: Rc<CompilerOptions>,
    ) -> ResolutionState<'a> {
        ResolutionState {
            resolver,
            tracer: None,
            name: String::new(),
            containing_directory: String::new(),
            is_config_lookup: false,
            features: NodeResolutionFeatures::NONE,
            esm_mode: false,
            conditions: Vec::new(),
            extensions: Extensions::default(),
            compiler_options,
            resolve_package_directory_only: false,
            candidate_ending_is_from_config: false,
            resolved_package_directory: false,
            diagnostics: Vec::new(),
        }
    }
}

// Go: module/resolver.go:273 newResolutionState
#[allow(clippy::too_many_arguments)]
#[must_use]
pub fn new_resolution_state<'a>(
    name: &str,
    containing_directory: &str,
    is_type_reference_directive: bool,
    resolution_mode: ResolutionMode,
    compiler_options: &Rc<CompilerOptions>,
    redirected_reference: Option<&dyn ModuleResolvedProjectReference>,
    resolver: &'a DefaultResolver,
    trace_builder: Option<Rc<RefCell<Tracer>>>,
) -> ResolutionState<'a> {
    let mut state = ResolutionState::zero(
        resolver,
        get_compiler_options_with_redirect(compiler_options, redirected_reference),
    );
    state.name = name.to_string();
    state.containing_directory = containing_directory.to_string();
    state.tracer = trace_builder;

    if is_type_reference_directive {
        state.extensions = Extensions::DECLARATION;
    } else if compiler_options.no_dts_resolution == Tristate::True {
        state.extensions = Extensions::IMPLEMENTATION_FILES;
    } else {
        state.extensions =
            Extensions::TYPE_SCRIPT | Extensions::JAVA_SCRIPT | Extensions::DECLARATION;
    }

    if !is_type_reference_directive && compiler_options.get_resolve_json_module() {
        state.extensions |= Extensions::JSON;
    }

    let module_resolution = compiler_options.get_module_resolution_kind();
    if module_resolution == ModuleResolutionKind::NODE16 {
        state.features = NodeResolutionFeatures::NODE16_DEFAULT;
        state.esm_mode = resolution_mode == ModuleKind::ES_NEXT;
        state.conditions = get_conditions(compiler_options, resolution_mode);
    } else if module_resolution == ModuleResolutionKind::NODE_NEXT {
        state.features = NodeResolutionFeatures::NODE_NEXT_DEFAULT;
        state.esm_mode = resolution_mode == ModuleKind::ES_NEXT;
        state.conditions = get_conditions(compiler_options, resolution_mode);
    } else if module_resolution == ModuleResolutionKind::BUNDLER {
        state.features = get_node_resolution_features(compiler_options);
        state.conditions = get_conditions(compiler_options, resolution_mode);
    }
    state
}

// Go: module/resolver.go:325 GetCompilerOptionsWithRedirect
#[must_use]
pub fn get_compiler_options_with_redirect(
    compiler_options: &Rc<CompilerOptions>,
    redirected_reference: Option<&dyn ModuleResolvedProjectReference>,
) -> Rc<CompilerOptions> {
    let Some(redirected_reference) = redirected_reference else {
        return compiler_options.clone();
    };
    if let Some(options_from_redirect) = redirected_reference.compiler_options() {
        return options_from_redirect;
    }
    compiler_options.clone()
}

// Go: module/resolver.go:335 DefaultResolver
// PORT: Go embeds `caches`; here it is the `caches` field.
pub struct DefaultResolver {
    pub caches: Caches,
    pub host: Rc<dyn ResolutionHost>,
    pub compiler_options: Rc<CompilerOptions>,
    pub typings_location: String,
    pub project_name: String,
    // tsgo#4712: the content mapper extensions.
    pub extra_extensions: Vec<String>,
    // reportDiagnostic: DiagnosticReporter
}

// Go: module/resolver.go:350 ResolverOptions
// PORT: the Go nil `Host` and `CompilerOptions` are `None`. Go copies the
// struct by value; this is `Clone`.
#[derive(Clone, Default)]
pub struct ResolverOptions {
    pub host: Option<Rc<dyn ResolutionHost>>,
    pub compiler_options: Option<Rc<CompilerOptions>>,
    pub typings_location: String,
    pub project_name: String,
    pub extra_extensions: Vec<String>,
    pub package_json_cache: Option<Rc<InfoCache>>,
}

// Go: module/resolver.go:363 NewResolver
// PORT: Go keeps a nil `Host` or `CompilerOptions` and panics at the first
// use; `DefaultResolver` has no nil for them, so this panics now.
#[must_use]
pub fn new_resolver(opts: ResolverOptions) -> DefaultResolver {
    let host = opts.host.expect("module.NewResolver: nil Host");
    let compiler_options = opts
        .compiler_options
        .expect("module.NewResolver: nil CompilerOptions");
    // PORT: Go sets the fields one by one on a zero `caches`.
    let caches = match opts.package_json_cache {
        Some(package_json_cache) => Caches::with_package_json_info_cache(package_json_cache),
        None => new_caches(
            host.get_current_directory(),
            host.fs().use_case_sensitive_file_names(),
            &compiler_options,
        ),
    };
    DefaultResolver {
        host,
        compiler_options,
        typings_location: opts.typings_location,
        project_name: opts.project_name,
        extra_extensions: opts.extra_extensions,
        caches,
    }
}

impl DefaultResolver {
    // Go: module/resolver.go:401 newTraceBuilder
    #[must_use]
    pub fn new_trace_builder(&self) -> Option<Rc<RefCell<Tracer>>> {
        if self.compiler_options.trace_resolution == Tristate::True {
            return Some(Rc::new(RefCell::new(Tracer::default())));
        }
        None
    }

    // Go: module/resolver.go:408 GetPackageScopeForPath
    pub fn get_package_scope_for_path(&self, directory: &str) -> Option<Rc<InfoCacheEntry>> {
        ResolutionState::zero(self, self.compiler_options.clone())
            .get_package_scope_for_path(directory)
    }

    /// A resolve-ahead worker's resolver (compiler/resolve_ahead.rs): finds
    /// the package scope of `directory` (`get_package_scope_for_path`) and
    /// publishes it with the package.json lookups and the file system calls
    /// that found it, for the loader (`Caches::package_scope_ahead`). A
    /// scope that the loader cannot check is not published.
    // PORT: not in Go (perf). Go `loadSourceFileMetaData` finds the scope
    // in each parse task.
    pub fn publish_package_scope(&self, directory: &str) {
        let Some(shared) = self.caches.shared.as_ref().filter(|shared| shared.publish) else {
            return;
        };
        self.caches.start_package_json_log();
        let scope = PackageScope::of(self.get_package_scope_for_path(directory).as_deref());
        let package_jsons = self.caches.take_package_json_log();
        let lookups = self.caches.take_worker_lookup_log();
        if let AheadLogEnd::Logged(calls) = self.caches.take_ahead_log() {
            shared.cache.set_scope(
                directory.to_string(),
                SharedResolution {
                    value: scope,
                    package_jsons,
                    lookups,
                    ahead: Some(calls),
                },
            );
        }
    }

    // Go: module/resolver.go:412 PackageJsonCacheEntries (tsgo#4301)
    // PORT: the entries include the package.json lookups of the parse
    // worker answers that this resolver took (`Caches::worker_package_jsons`),
    // so that they are what the one Go cache of all parse tasks holds: after
    // the cache's own entries, each lookup whose package.json the cache and
    // the lookups before it do not have, as the worker saw it. Go's lookup
    // reads each package.json that exists; the worker read it, and its
    // `exists` flag is `InfoCacheEntry::exists` of what it read, so the
    // entry is not read again (`PackageJsonCacheEntry`). The order of the
    // entries is not defined, as in Go.
    pub fn package_json_cache_entries(
        &self,
        mut f: impl FnMut(&Path, PackageJsonCacheEntry<'_>) -> bool,
    ) {
        let cache = &self.caches.package_json_info_cache;
        let mut go_on = true;
        cache.range(|key, entry| {
            go_on = f(
                key,
                PackageJsonCacheEntry {
                    package_directory: &entry.package_directory,
                    directory_exists: entry.directory_exists,
                    exists: entry.exists(),
                },
            );
            go_on
        });
        if !go_on {
            return;
        }
        let worker_package_jsons = self.caches.worker_package_jsons.borrow();
        let mut seen: FxHashSet<Path> = FxHashSet::default();
        for lookup in worker_package_jsons
            .iter()
            .flat_map(|lookups| lookups.iter())
        {
            let key = cache.key(&combine_paths(&lookup.package_directory, &["package.json"]));
            if cache.contains_key(&key) || !seen.insert(key.clone()) {
                continue;
            }
            let entry = PackageJsonCacheEntry {
                package_directory: &lookup.package_directory,
                directory_exists: lookup.directory_exists,
                exists: lookup.exists,
            };
            if !f(&key, entry) {
                return;
            }
        }
    }

    /// Takes the file system lookups of the parse worker answers that this
    /// resolver took (`Caches::worker_lookups`).
    // PORT: not in Go (see `BuildStatCache`).
    pub fn take_worker_lookups(&self) -> Vec<Arc<[StatLookup]>> {
        std::mem::take(&mut *self.caches.worker_lookups.borrow_mut())
    }
}

impl Tracer {
    // Go: module/resolver.go:416 tracer.traceResolutionUsingProjectReference
    pub fn trace_resolution_using_project_reference(
        &mut self,
        redirected_reference: Option<&dyn ModuleResolvedProjectReference>,
    ) {
        if let Some(redirected_reference) = redirected_reference {
            if redirected_reference.compiler_options().is_some() {
                self.write(
                    diag::Using_compiler_options_of_project_reference_redirect_0,
                    args![redirected_reference.config_name()],
                );
            }
        }
    }
}

impl DefaultResolver {
    // Go: module/resolver.go:422 ResolveTypeReferenceDirective
    pub fn resolve_type_reference_directive(
        &self,
        type_reference_directive_name: &str,
        containing_file: &str,
        resolution_mode: ResolutionMode,
        redirected_reference: Option<&dyn ModuleResolvedProjectReference>,
    ) -> (Rc<ResolvedTypeReferenceDirective>, Vec<DiagAndArgs>) {
        let containing_directory = get_directory_path(containing_file);
        let trace_builder = self.new_trace_builder();

        let from_inferred_types_containing_file =
            containing_file.ends_with(INFERRED_TYPES_CONTAINING_FILE);

        let cache_key = TypeRefDirectiveResolutionCacheKey {
            containing_directory: containing_directory.clone(),
            type_reference_name: type_reference_directive_name.to_string(),
            resolution_mode,
            redirect_config_name: get_redirect_config_name(redirected_reference),
            from_inferred_types_containing_file,
        };

        if trace_builder.is_none() {
            if let Some(cached) = self
                .caches
                .type_ref_directive_resolution_cache
                .get(&cache_key)
            {
                return (cached, Vec::new());
            }
            // PERF: a parse worker may have resolved this key already (Go
            // shares one cache between all parse tasks). The answer is the
            // same, so it goes into this resolver's cache as its own.
            if let Some(shared) = &self.caches.shared
                && let Some(found) = shared.cache.get_type_ref_directive(&cache_key)
            {
                self.caches.note_worker_package_jsons(&found.package_jsons);
                self.caches.note_worker_lookups(&found.lookups);
                let found = Rc::new((*found.value).clone());
                self.caches
                    .type_ref_directive_resolution_cache
                    .set(cache_key, found.clone());
                return (found, Vec::new());
            }
        }

        self.caches.start_package_json_log();
        let compiler_options =
            get_compiler_options_with_redirect(&self.compiler_options, redirected_reference);

        let (type_roots, from_config) =
            compiler_options.get_effective_type_roots(self.host.get_current_directory());
        if let Some(trace_builder) = &trace_builder {
            let mut trace_builder = trace_builder.borrow_mut();
            trace_builder.write(
                diag::Resolving_type_reference_directive_0_containing_file_1_root_directory_2,
                args![
                    type_reference_directive_name,
                    containing_file,
                    type_roots.join(",")
                ],
            );
            trace_builder.trace_resolution_using_project_reference(redirected_reference);
        }

        let mut state = new_resolution_state(
            type_reference_directive_name,
            &containing_directory,
            true, /*isTypeReferenceDirective*/
            resolution_mode,
            &compiler_options,
            redirected_reference,
            self,
            trace_builder.clone(),
        );
        let result = Rc::new(state.resolve_type_reference_directive(
            &type_roots,
            from_config,
            from_inferred_types_containing_file,
        ));

        if let Some(trace_builder) = &trace_builder {
            trace_builder
                .borrow_mut()
                .trace_type_reference_directive_result(type_reference_directive_name, &result);
        }

        if let Some(shared) = self.caches.shared.as_ref().filter(|shared| shared.publish) {
            shared.cache.set_type_ref_directive(
                cache_key.clone(),
                SharedResolution {
                    value: Arc::new((*result).clone()),
                    package_jsons: self.caches.take_package_json_log(),
                    lookups: self.caches.take_worker_lookup_log(),
                    // Resolve-ahead workers resolve only module names.
                    ahead: None,
                },
            );
        }
        self.caches
            .type_ref_directive_resolution_cache
            .set(cache_key, result.clone());

        (result, traces_of(&trace_builder))
    }

    // Go: module/resolver.go:467 ResolveModuleName
    // PORT: the `module.Resolver` form is `Resolver::resolve_module_name`,
    // whose result can be nil. This one is never nil and never fails (Go
    // returns a nil error).
    pub fn resolve_module_name(
        &self,
        module_name: &str,
        containing_file: &str,
        resolution_mode: ResolutionMode,
        redirected_reference: Option<&dyn ModuleResolvedProjectReference>,
    ) -> (Arc<ResolvedModule>, Vec<DiagAndArgs>, Option<GoError>) {
        let (result, trace) = self.resolve_module_name_worker(
            module_name,
            containing_file,
            &get_directory_path(containing_file),
            resolution_mode,
            redirected_reference,
        );
        (result, trace, None)
    }

    // Go: module/resolver.go:472 ResolveModuleNameFromDirectory (ts#64299)
    // PORT: see `resolve_module_name`.
    pub fn resolve_module_name_from_directory(
        &self,
        module_name: &str,
        containing_directory: &str,
        resolution_mode: ResolutionMode,
    ) -> (Arc<ResolvedModule>, Vec<DiagAndArgs>, Option<GoError>) {
        let (result, trace) = self.resolve_module_name_worker(
            module_name,
            containing_directory,
            containing_directory,
            resolution_mode,
            None,
        );
        (result, trace, None)
    }

    // Go: module/resolver.go:477 resolveModuleName
    // PORT: named `resolve_module_name_worker`; the Go names
    // `ResolveModuleName` and `resolveModuleName` share a snake name.
    fn resolve_module_name_worker(
        &self,
        module_name: &str,
        containing_file: &str,
        containing_directory: &str,
        resolution_mode: ResolutionMode,
        redirected_reference: Option<&dyn ModuleResolvedProjectReference>,
    ) -> (Arc<ResolvedModule>, Vec<DiagAndArgs>) {
        let trace_builder = self.new_trace_builder();

        let redirect_config_name = get_redirect_config_name(redirected_reference);
        // PORT: the cache lookups use the key's parts; the key itself is
        // made only for a new entry (`ModuleKey`).
        let key_parts = (
            containing_directory,
            module_name,
            resolution_mode,
            redirect_config_name.as_str(),
        );

        if trace_builder.is_none() {
            if let Some(cached) = self.caches.module_resolution_cache.get(&key_parts) {
                return (cached, Vec::new());
            }
            // PERF: a parse worker may have resolved this key already (see
            // `resolve_type_reference_directive`). A resolve-ahead worker
            // resolves each key that no other worker takes
            // (`AheadQueue::take_next`), so it does not look.
            if let Some(shared) = &self.caches.shared
                && !(shared.publish && on_ahead_thread())
                && let Some(found) = shared.cache.get_module(&key_parts)
            {
                self.caches.note_worker_package_jsons(&found.package_jsons);
                self.caches.note_worker_lookups(&found.lookups);
                self.caches.module_resolution_cache.set(
                    ModuleResolutionCacheKey::from_parts(key_parts),
                    found.value.clone(),
                );
                return (found.value, Vec::new());
            }
            // PERF: a resolve-ahead worker may have resolved this key
            // (compiler/resolve_ahead.rs). The loader takes the answer only
            // after it checked it on its own file system.
            if let Some(found) = self.caches.take_resolved_ahead(key_parts) {
                self.caches.module_resolution_cache.set(
                    ModuleResolutionCacheKey::from_parts(key_parts),
                    found.clone(),
                );
                return (found, Vec::new());
            }
        }
        let cache_key = ModuleResolutionCacheKey::from_parts(key_parts);

        self.caches.start_package_json_log();
        let compiler_options =
            get_compiler_options_with_redirect(&self.compiler_options, redirected_reference);
        if let Some(trace_builder) = &trace_builder {
            let mut trace_builder = trace_builder.borrow_mut();
            trace_builder.write(
                diag::Resolving_module_0_from_1,
                args![module_name, containing_file],
            );
            trace_builder.trace_resolution_using_project_reference(redirected_reference);
        }

        let module_resolution = compiler_options.get_module_resolution_kind();
        if compiler_options.module_resolution != module_resolution {
            if let Some(trace_builder) = &trace_builder {
                trace_builder.borrow_mut().write(
                    diag::Module_resolution_kind_is_not_specified_using_0,
                    args![module_resolution.string()],
                );
            }
        } else if let Some(trace_builder) = &trace_builder {
            trace_builder.borrow_mut().write(
                diag::Explicitly_specified_module_resolution_kind_Colon_0,
                args![module_resolution.string()],
            );
        }

        let result: ResolvedModule = if module_resolution == ModuleResolutionKind::NODE16
            || module_resolution == ModuleResolutionKind::NODE_NEXT
            || module_resolution == ModuleResolutionKind::BUNDLER
        {
            let mut state = new_resolution_state(
                module_name,
                containing_directory,
                false, /*isTypeReferenceDirective*/
                resolution_mode,
                &compiler_options,
                redirected_reference,
                self,
                trace_builder.clone(),
            );
            state.resolve_node_like()
        } else {
            panic!("Unexpected moduleResolution: {}", module_resolution.0);
        };

        if let Some(trace_builder) = &trace_builder {
            let mut trace_builder = trace_builder.borrow_mut();
            if result.is_resolved() {
                if !result.package_id.name.is_empty() {
                    trace_builder.write(
                        diag::Module_name_0_was_successfully_resolved_to_1_with_Package_ID_2,
                        args![
                            module_name,
                            result.resolved_file_name,
                            result.package_id.string()
                        ],
                    );
                } else {
                    trace_builder.write(
                        diag::Module_name_0_was_successfully_resolved_to_1,
                        args![module_name, result.resolved_file_name],
                    );
                }
            } else {
                trace_builder.write(diag::Module_name_0_was_not_resolved, args![module_name]);
            }
        }

        let final_result = Arc::new(self.try_resolve_from_typings_location(
            module_name,
            containing_directory,
            result,
            &trace_builder,
        ));
        if let Some(shared) = self.caches.shared.as_ref().filter(|shared| shared.publish) {
            let package_jsons = self.caches.take_package_json_log();
            let lookups = self.caches.take_worker_lookup_log();
            // A resolve-ahead answer that the loader cannot check is not
            // published (`AheadLogEnd::Unshareable`).
            let ahead = match self.caches.take_ahead_log() {
                AheadLogEnd::NotLogged => Some(None),
                AheadLogEnd::Unshareable => None,
                AheadLogEnd::Logged(calls) => Some(Some(calls)),
            };
            if let Some(ahead) = ahead {
                shared.cache.set_module(
                    cache_key.clone(),
                    SharedResolution {
                        value: Arc::clone(&final_result),
                        package_jsons,
                        lookups,
                        ahead,
                    },
                );
            }
        }
        self.caches
            .module_resolution_cache
            .set(cache_key, final_result.clone());

        (final_result, traces_of(&trace_builder))
    }

    // Go: module/resolver.go:537 ResolvePackageDirectory
    // PORT: a Go nil `*ResolvedModule` is `None`.
    pub fn resolve_package_directory(
        &self,
        module_name: &str,
        containing_file: &str,
        resolution_mode: ResolutionMode,
        redirected_reference: Option<&dyn ModuleResolvedProjectReference>,
    ) -> Option<ResolvedModule> {
        let compiler_options =
            get_compiler_options_with_redirect(&self.compiler_options, redirected_reference);
        let containing_directory = get_directory_path(containing_file);
        let mut state = new_resolution_state(
            module_name,
            &containing_directory,
            false, /*isTypeReferenceDirective*/
            resolution_mode,
            &compiler_options,
            redirected_reference,
            self,
            None,
        );
        state.resolve_package_directory_only = true;
        let result =
            state.load_module_from_nearest_node_modules_directory(false /*typesScopeOnly*/);
        if resolved_is_resolved(&result) {
            return Some(state.create_resolved_module_handling_symlink(result));
        }
        None
    }

    // Go: module/resolver.go:548 tryResolveFromTypingsLocation
    // PORT: takes the original result by value and returns it or a new one.
    pub fn try_resolve_from_typings_location(
        &self,
        module_name: &str,
        containing_directory: &str,
        original_result: ResolvedModule,
        trace_builder: &Option<Rc<RefCell<Tracer>>>,
    ) -> ResolvedModule {
        if self.typings_location.is_empty()
            || is_external_module_name_relative(module_name)
            || (!original_result.resolved_file_name.is_empty()
                && extension_is_one_of(
                    &original_result.extension,
                    SUPPORTED_TS_EXTENSIONS_WITH_JSON_FLAT,
                ))
        {
            return original_result;
        }

        let mut state = new_resolution_state(
            module_name,
            containing_directory,
            false,            /*isTypeReferenceDirective*/
            ModuleKind::NONE, // resolutionMode,
            &self.compiler_options,
            None, // redirectedReference,
            self,
            trace_builder.clone(),
        );
        if let Some(trace_builder) = trace_builder {
            trace_builder.borrow_mut().write(
                diag::Auto_discovery_for_typings_is_enabled_in_project_0_Running_extra_resolution_pass_for_module_1_using_cache_location_2,
                args![self.project_name, module_name, self.typings_location],
            );
        }
        let global_resolved = state.load_module_from_immediate_node_modules_directory(
            Extensions::DECLARATION,
            &self.typings_location,
            false,
        );
        if global_resolved.is_none() {
            return original_result;
        }
        let mut result = state.create_resolved_module(global_resolved, true);
        let mut resolution_diagnostics = original_result.resolution_diagnostics;
        resolution_diagnostics.append(&mut result.resolution_diagnostics);
        result.resolution_diagnostics = resolution_diagnostics;
        result
    }

    // Go: module/resolver.go:577 resolveConfig
    pub fn resolve_config(&self, module_name: &str, containing_file: &str) -> ResolvedModule {
        let containing_directory = get_directory_path(containing_file);
        let mut state = new_resolution_state(
            module_name,
            &containing_directory,
            false, /*isTypeReferenceDirective*/
            ModuleKind::COMMON_JS,
            &self.compiler_options,
            None,
            self,
            None,
        );
        state.is_config_lookup = true;
        state.extensions = Extensions::JSON;
        state.resolve_node_like()
    }
}

impl Tracer {
    // Go: module/resolver.go:585 tracer.traceTypeReferenceDirectiveResult
    pub fn trace_type_reference_directive_result(
        &mut self,
        type_reference_directive_name: &str,
        result: &ResolvedTypeReferenceDirective,
    ) {
        if !result.is_resolved() {
            self.write(
                diag::Type_reference_directive_0_was_not_resolved,
                args![type_reference_directive_name],
            );
        } else if !result.package_id.name.is_empty() {
            self.write(
                diag::Type_reference_directive_0_was_successfully_resolved_to_1_with_Package_ID_2_primary_Colon_3,
                args![
                    type_reference_directive_name,
                    result.resolved_file_name,
                    result.package_id.string(),
                    result.primary
                ],
            );
        } else {
            self.write(
                diag::Type_reference_directive_0_was_successfully_resolved_to_1_primary_Colon_2,
                args![
                    type_reference_directive_name,
                    result.resolved_file_name,
                    result.primary
                ],
            );
        }
    }
}

impl ResolutionState<'_> {
    // Go: module/resolver.go:606 resolveTypeReferenceDirective
    pub fn resolve_type_reference_directive(
        &mut self,
        type_roots: &[String],
        from_config: bool,
        from_inferred_types_containing_file: bool,
    ) -> ResolvedTypeReferenceDirective {
        // Primary lookup
        if !type_roots.is_empty() {
            trace_write!(
                self,
                diag::Resolving_with_primary_search_path_0,
                type_roots.join(", ")
            );
            for type_root in type_roots {
                let candidate = self.get_candidate_from_type_root(type_root);
                let directory_exists = self.resolver.host.fs().directory_exists(type_root);
                if !directory_exists {
                    trace_write!(
                        self,
                        diag::Directory_0_does_not_exist_skipping_all_lookups_in_it,
                        type_root
                    );
                    continue;
                }
                // ts#64159: a name with a trailing separator is only a
                // directory.
                if from_config && !has_trailing_directory_separator(&candidate) {
                    // Custom typeRoots resolve as file or directory just like we do modules
                    let resolved_from_file =
                        self.load_module_from_file(Extensions::DECLARATION, &candidate);
                    if let Some(mut resolved_from_file) = resolved_from_file {
                        let package_directory =
                            node_module_package_root_for_file(&resolved_from_file.path);
                        if !package_directory.is_empty() {
                            let package_info = self.get_package_json_info(&package_directory);
                            resolved_from_file.package_id =
                                self.get_package_id(&resolved_from_file.path, &package_info);
                        }
                        return self.create_resolved_type_reference_directive(
                            Some(resolved_from_file),
                            true, /*primary*/
                        );
                    }
                }
                let resolved_from_directory = self.load_node_module_from_directory(
                    Extensions::DECLARATION,
                    &candidate,
                    true, /*considerPackageJson*/
                );
                if resolved_from_directory.is_some() {
                    return self.create_resolved_type_reference_directive(
                        resolved_from_directory,
                        true, /*primary*/
                    );
                }
            }
        } else {
            trace_write!(
                self,
                diag::Root_directory_cannot_be_determined_skipping_primary_search_paths
            );
        }

        // Secondary lookup
        let mut resolved: Option<Resolved> = None;
        if !from_config || !from_inferred_types_containing_file {
            trace_write!(
                self,
                diag::Looking_up_in_node_modules_folder_initial_location_0,
                self.containing_directory
            );
            if !is_external_module_name_relative(&self.name) {
                resolved = self
                    .load_module_from_nearest_node_modules_directory(false /*typesScopeOnly*/);
            } else {
                let candidate =
                    normalize_path_for_cjs_resolution(&self.containing_directory, &self.name);
                resolved = self.node_load_module_by_relative_name(
                    Extensions::DECLARATION,
                    &candidate,
                    true, /*considerPackageJson*/
                );
            }
        } else {
            trace_write!(
                self,
                diag::Resolving_type_reference_directive_for_program_that_specifies_custom_typeRoots_skipping_lookup_in_node_modules_folder
            );
        }
        self.create_resolved_type_reference_directive(resolved, false /*primary*/)
    }

    // Go: module/resolver.go:659 getCandidateFromTypeRoot
    pub fn get_candidate_from_type_root(&mut self, type_root: &str) -> String {
        let mut name_for_lookup = self.name.clone();
        if type_root.ends_with("/node_modules/@types")
            || type_root.ends_with("/node_modules/@types/")
        {
            let name = self.name.clone();
            name_for_lookup = self.mangle_scoped_package_name(&name);
        }
        // ts#64159: the candidate is normalized and keeps the directory
        // intent of a trailing separator (Go resolutionCandidateFromDirectoryPath).
        resolve_path_for_module(
            type_root,
            &name_for_lookup,
            has_trailing_directory_separator(&name_for_lookup),
        )
    }

    // Go: module/resolver.go:667 resolutionState.mangleScopedPackageName
    pub fn mangle_scoped_package_name(&mut self, name: &str) -> String {
        let mangled = mangle_scoped_package_name(name);
        if self.tracer.is_some() && mangled != name {
            trace_write!(self, diag::Scoped_package_detected_looking_in_0, mangled);
        }
        mangled
    }

    // Go: module/resolver.go:678 resolveFromTypeRoot
    // resolveFromTypeRoot tries to resolve a module name from the configured typeRoots.
    // This is used as a fallback after node_modules resolution fails, for declaration file lookups.
    // Returns nil if typeRoots is not configured or if no matching module is found in any typeRoot directory.
    pub fn resolve_from_type_root(&mut self) -> Option<Resolved> {
        let Some(type_roots) = self.compiler_options.type_roots.clone() else {
            return None;
        };
        for type_root in &type_roots {
            let candidate = self.get_candidate_from_type_root(type_root);
            let directory_exists = self.resolver.host.fs().directory_exists(type_root);
            if !directory_exists {
                trace_write!(
                    self,
                    diag::Directory_0_does_not_exist_skipping_all_lookups_in_it,
                    type_root
                );
                continue;
            }
            // ts#64159: a name with a trailing separator is only a directory.
            if !has_trailing_directory_separator(&candidate) {
                let resolved_from_file =
                    self.load_module_from_file(Extensions::DECLARATION, &candidate);
                if let Some(mut resolved_from_file) = resolved_from_file {
                    let package_directory =
                        node_module_package_root_for_file(&resolved_from_file.path);
                    if !package_directory.is_empty() {
                        let package_info = self.get_package_json_info(&package_directory);
                        resolved_from_file.package_id =
                            self.get_package_id(&resolved_from_file.path, &package_info);
                    }
                    return Some(resolved_from_file);
                }
            }
            let resolved = self.load_node_module_from_directory(
                Extensions::DECLARATION,
                &candidate,
                true, /*considerPackageJson*/
            );
            if resolved.is_some() {
                return resolved;
            }
        }
        None
    }

    // Go: module/resolver.go:707 getPackageScopeForPath
    pub fn get_package_scope_for_path(&mut self, directory: &str) -> Option<Rc<InfoCacheEntry>> {
        let resolver = self.resolver;
        for_each_ancestor_directory_stopping_at_global_cache(
            &resolver.typings_location,
            directory,
            |directory: &str| -> (Option<Rc<InfoCacheEntry>>, bool) {
                let result = self.get_package_json_info(directory);
                if result.is_some() {
                    return (result, true);
                }
                (None, false)
            },
        )
    }

    // Go: module/resolver.go:719 resolveNodeLike
    pub fn resolve_node_like(&mut self) -> ResolvedModule {
        if self.tracer.is_some() {
            let conditions = self
                .conditions
                .iter()
                .map(|c| format!("'{c}'"))
                .collect::<Vec<_>>()
                .join(", ");
            if self.esm_mode {
                trace_write!(
                    self,
                    diag::Resolving_in_0_mode_with_conditions_1,
                    "ESM",
                    conditions
                );
            } else {
                trace_write!(
                    self,
                    diag::Resolving_in_0_mode_with_conditions_1,
                    "CJS",
                    conditions
                );
            }
        }
        let mut result = self.resolve_node_like_worker();
        if self.resolved_package_directory
            && !self.is_config_lookup
            && self.features.intersects(NodeResolutionFeatures::EXPORTS)
            && self
                .extensions
                .intersects(Extensions::TYPE_SCRIPT | Extensions::DECLARATION)
            && !is_external_module_name_relative(&self.name)
            && result.is_resolved()
            && result.is_external_library_import
            && !extension_is_ok(
                Extensions::TYPE_SCRIPT | Extensions::DECLARATION,
                &result.extension,
            )
            && self.conditions.iter().any(|c| c == "import")
        {
            trace_write!(
                self,
                diag::Resolution_of_non_relative_name_failed_trying_with_modern_Node_resolution_features_disabled_to_see_if_npm_library_needs_configuration_update
            );
            self.features = self.features.without(NodeResolutionFeatures::EXPORTS);
            self.extensions = self.extensions & (Extensions::TYPE_SCRIPT | Extensions::DECLARATION);
            let diagnostics_count = self.diagnostics.len();
            let diagnostic_result = self.resolve_node_like_worker();
            if diagnostic_result.is_resolved() && diagnostic_result.is_external_library_import {
                result.alternate_result = diagnostic_result.resolved_file_name;
            }
            self.diagnostics.truncate(diagnostics_count);
        }
        result
    }

    // Go: module/resolver.go:752 resolveNodeLikeWorker
    pub fn resolve_node_like_worker(&mut self) -> ResolvedModule {
        let resolved = self.try_load_module_using_optional_resolution_settings();
        if resolved.is_some() {
            return self.create_resolved_module_handling_symlink(resolved);
        }

        if !is_external_module_name_relative(&self.name) {
            if self.features.intersects(NodeResolutionFeatures::IMPORTS)
                && self.name.starts_with('#')
            {
                let resolved = self.load_module_from_imports();
                if resolved.is_some() {
                    return self.create_resolved_module_handling_symlink(resolved);
                }
            }
            if self.features.intersects(NodeResolutionFeatures::SELF_NAME) {
                let resolved = self.load_module_from_self_name_reference();
                if resolved.is_some() {
                    return self.create_resolved_module_handling_symlink(resolved);
                }
            }
            if self.name.contains(':') {
                trace_write!(
                    self,
                    diag::Skipping_module_0_that_looks_like_an_absolute_URI_target_file_types_Colon_1,
                    self.name,
                    self.extensions.string()
                );
                return self.create_resolved_module(None, false);
            }
            trace_write!(
                self,
                diag::Loading_module_0_from_node_modules_folder_target_file_types_Colon_1,
                self.name,
                self.extensions.string()
            );
            let resolved =
                self.load_module_from_nearest_node_modules_directory(false /*typesScopeOnly*/);
            if resolved.is_some() {
                return self.create_resolved_module_handling_symlink(resolved);
            }
            if self.extensions.intersects(Extensions::DECLARATION) {
                let resolved = self.resolve_from_type_root();
                if resolved.is_some() {
                    return self.create_resolved_module_handling_symlink(resolved);
                }
            }
        } else {
            let candidate =
                normalize_path_for_cjs_resolution(&self.containing_directory, &self.name);
            let extensions = self.extensions;
            let resolved = self.node_load_module_by_relative_name(extensions, &candidate, true);
            let is_external_library_import = resolved
                .as_ref()
                .is_some_and(|r| r.path.contains("/node_modules/"));
            return self.create_resolved_module(resolved, is_external_library_import);
        }
        self.create_resolved_module(None, false)
    }

    // Go: module/resolver.go:796 loadModuleFromSelfNameReference
    pub fn load_module_from_self_name_reference(&mut self) -> Option<Resolved> {
        let directory_path = get_normalized_absolute_path(
            &self.containing_directory,
            self.resolver.host.get_current_directory(),
        );
        let scope = self.get_package_scope_for_path(&directory_path);
        if !entry_exists(&scope)
            || scope
                .as_ref()
                .unwrap()
                .contents
                .as_ref()
                .unwrap()
                .fields
                .path_fields
                .exports
                .json_value
                .is_falsy()
        {
            // !!! falsy check seems wrong?
            return continue_searching();
        }
        let contents = scope.as_ref().unwrap().contents.clone().unwrap();
        let (name, ok) = contents.fields.header_fields.name.get_value();
        if !ok {
            return continue_searching();
        }
        let parts = get_path_components(&self.name, "");
        let name_parts = get_path_components(&name, "");
        if parts.len() < name_parts.len() || name_parts[..] != parts[..name_parts.len()] {
            return continue_searching();
        }
        let trailing_parts: Vec<&str> = parts[name_parts.len()..]
            .iter()
            .map(String::as_str)
            .collect();
        let subpath = if !trailing_parts.is_empty() {
            combine_paths(".", &trailing_parts)
        } else {
            ".".to_string()
        };
        // Maybe TODO: splitting extensions into two priorities should be unnecessary, except
        // https://github.com/microsoft/TypeScript/issues/50762 makes the behavior different.
        // As long as that bug exists, we need to do two passes here in self-name loading
        // in order to be consistent with (non-self) library-name loading in
        // `loadModuleFromNearestNodeModulesDirectoryWorker`, which uses two passes in order
        // to prioritize `@types` packages higher up the directory tree over untyped
        // implementation packages. See the selfNameModuleAugmentation.ts test for why this
        // matters.
        //
        // However, there's an exception. If the user has `allowJs` and `declaration`, we need
        // to ensure that self-name imports of their own package can resolve back to their
        // input JS files via `tryLoadInputFileForPath` at a higher priority than their output
        // declaration files, so we need to do a single pass with all extensions for that case.
        if self.compiler_options.get_allow_js()
            && !self.containing_directory.contains("/node_modules/")
        {
            let extensions = self.extensions;
            return self.load_module_from_exports(&scope, extensions, &subpath);
        }
        let priority_extensions =
            self.extensions & (Extensions::TYPE_SCRIPT | Extensions::DECLARATION);
        let secondary_extensions = self
            .extensions
            .without(Extensions::TYPE_SCRIPT | Extensions::DECLARATION);
        let resolved = self.load_module_from_exports(&scope, priority_extensions, &subpath);
        if resolved.is_some() {
            return resolved;
        }
        self.load_module_from_exports(&scope, secondary_extensions, &subpath)
    }

    // Go: module/resolver.go:842 loadModuleFromImports
    pub fn load_module_from_imports(&mut self) -> Option<Resolved> {
        if self.name == "#"
            || (self.name.starts_with("#/")
                && !self
                    .features
                    .intersects(NodeResolutionFeatures::IMPORTS_PATTERN_ROOT))
        {
            trace_write!(
                self,
                diag::Invalid_import_specifier_0_has_no_possible_resolutions,
                self.name
            );
            return continue_searching();
        }
        let directory_path = get_normalized_absolute_path(
            &self.containing_directory,
            self.resolver.host.get_current_directory(),
        );
        let scope = self.get_package_scope_for_path(&directory_path);
        if !entry_exists(&scope) {
            trace_write!(
                self,
                diag::Directory_0_has_no_containing_package_json_scope_Imports_will_not_resolve,
                directory_path
            );
            return continue_searching();
        }
        let scope = scope.unwrap();
        let contents = scope.contents.clone().unwrap();
        let imports = &contents.fields.path_fields.imports;
        if imports.json_value.type_ != JSONValueType::OBJECT {
            // !!! Old compiler only checks for undefined, but then assumes `imports` is an object if present.
            // Maybe should have a new diagnostic for imports of an invalid type. Also, array should be handled?
            trace_write!(
                self,
                diag::X_package_json_scope_0_has_no_imports_defined,
                scope.package_directory
            );
            return continue_searching();
        }

        let extensions = self.extensions;
        let name = self.name.clone();
        let result = self.load_module_from_exports_or_imports(
            extensions,
            &name,
            imports.as_object(),
            &scope,
            true, /*isImports*/
        );
        if result.is_some() {
            return result;
        }

        trace_write!(
            self,
            diag::Import_specifier_0_does_not_exist_in_package_json_scope_at_path_1,
            self.name,
            scope.package_directory
        );
        continue_searching()
    }

    // Go: module/resolver.go:876 loadModuleFromExports
    pub fn load_module_from_exports(
        &mut self,
        package_info: &Option<Rc<InfoCacheEntry>>,
        ext: Extensions,
        subpath: &str,
    ) -> Option<Resolved> {
        // !!! This is ported exactly, but the falsy check seems wrong
        if !entry_exists(package_info)
            || package_info
                .as_ref()
                .unwrap()
                .contents
                .as_ref()
                .unwrap()
                .fields
                .path_fields
                .exports
                .json_value
                .is_falsy()
        {
            return continue_searching();
        }
        let package_info = package_info.clone().unwrap();
        let contents = package_info.contents.clone().unwrap();
        let exports = &contents.fields.path_fields.exports;

        if subpath == "." {
            let mut main_export = ExportsOrImports::default();
            match exports.json_value.type_ {
                JSONValueType::STRING | JSONValueType::ARRAY => {
                    main_export = exports.clone();
                }
                JSONValueType::OBJECT => {
                    if exports.is_conditions() {
                        main_export = exports.clone();
                    } else if let Some(dot) = exports.as_object().get(".") {
                        main_export = dot.clone();
                    }
                }
                _ => {}
            }
            if main_export.json_value.type_ != JSONValueType::NOT_PRESENT {
                return self.load_module_from_target_export_or_import(
                    ext,
                    subpath,
                    &package_info,
                    false, /*isImports*/
                    &main_export,
                    "",
                    false, /*isPattern*/
                    ".",
                );
            }
        } else if exports.json_value.type_ == JSONValueType::OBJECT && exports.is_subpaths() {
            let result = self.load_module_from_exports_or_imports(
                ext,
                subpath,
                exports.as_object(),
                &package_info,
                false, /*isImports*/
            );
            if result.is_some() {
                return result;
            }
        }

        trace_write!(
            self,
            diag::Export_specifier_0_does_not_exist_in_package_json_scope_at_path_1,
            subpath,
            package_info.package_directory
        );
        continue_searching()
    }

    // Go: module/resolver.go:909 loadModuleFromExportsOrImports
    pub fn load_module_from_exports_or_imports(
        &mut self,
        extensions: Extensions,
        module_name: &str,
        lookup_table: &IndexMap<String, ExportsOrImports>,
        scope: &Rc<InfoCacheEntry>,
        is_imports: bool,
    ) -> Option<Resolved> {
        if !module_name.ends_with('/') && !module_name.contains('*') {
            if let Some(target) = lookup_table.get(module_name) {
                return self.load_module_from_target_export_or_import(
                    extensions,
                    module_name,
                    scope,
                    is_imports,
                    target,
                    "",
                    false, /*isPattern*/
                    module_name,
                );
            }
        }

        let mut expanding_keys: Vec<&String> = Vec::with_capacity(lookup_table.len());
        for key in lookup_table.keys() {
            if key.matches('*').count() == 1 || key.ends_with('/') {
                expanding_keys.push(key);
            }
        }
        // Go: module/resolver.go:725 slices.SortFunc(expandingKeys, ComparePatternKeys)
        crate::gostd::slices::sort_func(&mut expanding_keys, |a, b| compare_pattern_keys(a, b));

        // PORT: Go matches and slices bytes. The names are port forms, so
        // this works on their Go bytes (see `scanner_util::GO_STRING_MARKER`).
        let module_name_len = go_len(module_name);
        for potential_target in expanding_keys {
            if self
                .features
                .intersects(NodeResolutionFeatures::EXPORTS_PATTERN_TRAILERS)
                && matches_pattern_with_trailer(potential_target, module_name)
            {
                let target = &lookup_table[potential_target];
                let star = potential_target.find('*').unwrap();
                let star_pos = go_len(&potential_target[..star]);
                let subpath = go_slice(
                    module_name,
                    star_pos,
                    module_name_len - (go_len(potential_target) - 1 - star_pos),
                );
                return self.load_module_from_target_export_or_import(
                    extensions,
                    module_name,
                    scope,
                    is_imports,
                    target,
                    &subpath,
                    true,
                    potential_target,
                );
            } else if potential_target.ends_with('*')
                && go_has_prefix(module_name, &potential_target[..potential_target.len() - 1])
            {
                let target = &lookup_table[potential_target];
                let subpath = go_slice(module_name, go_len(potential_target) - 1, module_name_len);
                return self.load_module_from_target_export_or_import(
                    extensions,
                    module_name,
                    scope,
                    is_imports,
                    target,
                    &subpath,
                    true,
                    potential_target,
                );
            } else if go_has_prefix(module_name, potential_target) {
                let target = &lookup_table[potential_target];
                let subpath = go_slice(module_name, go_len(potential_target), module_name_len);
                return self.load_module_from_target_export_or_import(
                    extensions,
                    module_name,
                    scope,
                    is_imports,
                    target,
                    &subpath,
                    false,
                    potential_target,
                );
            }
        }

        continue_searching()
    }

    // Go: module/resolver.go:950 loadModuleFromTargetExportOrImport
    #[allow(clippy::too_many_arguments)]
    pub fn load_module_from_target_export_or_import(
        &mut self,
        extensions: Extensions,
        module_name: &str,
        scope: &Rc<InfoCacheEntry>,
        is_imports: bool,
        target: &ExportsOrImports,
        subpath: &str,
        is_pattern: bool,
        key: &str,
    ) -> Option<Resolved> {
        match target.json_value.type_ {
            JSONValueType::STRING => {
                let target_string: String = target.json_value.as_string().to_string();
                if !is_pattern && !subpath.is_empty() && !target_string.ends_with('/') {
                    trace_write!(
                        self,
                        diag::X_package_json_scope_0_has_invalid_type_for_target_of_specifier_1,
                        scope.package_directory,
                        module_name
                    );
                    return continue_searching();
                }
                if !target_string.starts_with("./") {
                    if is_imports
                        && !target_string.starts_with("../")
                        && !target_string.starts_with('/')
                        && !is_rooted_disk_path(&target_string)
                    {
                        let combined_lookup = if is_pattern {
                            target_string.replace('*', subpath)
                        } else {
                            format!("{target_string}{subpath}")
                        };
                        let scope_containing_directory =
                            ensure_trailing_directory_separator(&scope.package_directory);
                        trace_write!(
                            self,
                            diag::Using_0_subpath_1_with_target_2,
                            "imports",
                            key,
                            combined_lookup
                        );
                        trace_write!(
                            self,
                            diag::Resolving_module_0_from_1,
                            combined_lookup,
                            scope_containing_directory
                        );
                        // PORT: Go restores `name` and `containingDirectory` with `defer`.
                        let name = std::mem::replace(&mut self.name, combined_lookup);
                        let containing_directory = std::mem::replace(
                            &mut self.containing_directory,
                            scope_containing_directory,
                        );
                        let result = self.resolve_node_like();
                        self.name = name;
                        self.containing_directory = containing_directory;
                        if result.is_resolved() {
                            return Some(Resolved {
                                path: result.resolved_file_name,
                                extension: result.extension,
                                package_id: result.package_id,
                                original_path: result.original_path,
                                resolved_using_ts_extension: result.resolved_using_ts_extension,
                                ..Default::default()
                            });
                        }
                        return continue_searching();
                    }
                    trace_write!(
                        self,
                        diag::X_package_json_scope_0_has_invalid_type_for_target_of_specifier_1,
                        scope.package_directory,
                        module_name
                    );
                    return continue_searching();
                }
                let parts: Vec<String> = if path_is_relative(&target_string) {
                    get_path_components(&target_string, "")[1..].to_vec()
                } else {
                    get_path_components(&target_string, "")
                };
                let parts_after_first = &parts[1..];
                if parts_after_first
                    .iter()
                    .any(|p| p == ".." || p == "." || p == "node_modules")
                {
                    trace_write!(
                        self,
                        diag::X_package_json_scope_0_has_invalid_type_for_target_of_specifier_1,
                        scope.package_directory,
                        module_name
                    );
                    return continue_searching();
                }
                // TODO: Assert that `resolvedTarget` is actually within the package directory? That's what the spec says.... but I'm not sure we need
                // to be in the business of validating everyone's import and export map correctness.
                let subpath_parts = get_path_components(subpath, "");
                if subpath_parts
                    .iter()
                    .any(|p| p == ".." || p == "." || p == "node_modules")
                {
                    trace_write!(
                        self,
                        diag::X_package_json_scope_0_has_invalid_type_for_target_of_specifier_1,
                        scope.package_directory,
                        module_name
                    );
                    return continue_searching();
                }

                if self.tracer.is_some() {
                    let message_target = if is_pattern {
                        target_string.replace('*', subpath)
                    } else {
                        format!("{target_string}{subpath}")
                    };
                    trace_write!(
                        self,
                        diag::Using_0_subpath_1_with_target_2,
                        if is_imports { "imports" } else { "exports" },
                        key,
                        message_target
                    );
                }
                // ts#64544: the target is resolved against the package
                // directory, and a target that ends with "/" stays a
                // directory path.
                let target_path = if is_pattern {
                    target_string.replace('*', subpath)
                } else {
                    format!("{target_string}{subpath}")
                };
                let final_path = resolve_path_for_module(
                    &scope.package_directory,
                    &target_path,
                    has_trailing_directory_separator(&target_path),
                );
                let scope_info = Some(scope.clone());
                let input_link = self.try_load_input_file_for_path(
                    &final_path,
                    subpath,
                    &combine_paths(&scope.package_directory, &["package.json"]),
                    is_imports,
                );
                if let Some(mut input_link) = input_link {
                    input_link.package_id = self.get_package_id(&input_link.path, &scope_info);
                    return Some(input_link);
                }
                let result = self.load_file_name_from_package_json_field(
                    extensions,
                    &final_path,
                    &target_string,
                );
                if let Some(mut result) = result {
                    result.package_id = self.get_package_id(&result.path, &scope_info);
                    return Some(result);
                }
                return continue_searching();
            }
            JSONValueType::OBJECT => {
                trace_write!(self, diag::Entering_conditional_exports);
                for (condition, sub_target) in target.as_object() {
                    if self.condition_matches(condition) {
                        trace_write!(
                            self,
                            diag::Matched_0_condition_1,
                            if is_imports { "imports" } else { "exports" },
                            condition
                        );
                        let result = self.load_module_from_target_export_or_import(
                            extensions,
                            module_name,
                            scope,
                            is_imports,
                            sub_target,
                            subpath,
                            is_pattern,
                            key,
                        );
                        if result.is_some() {
                            if resolved_is_resolved(&result) {
                                trace_write!(self, diag::Resolved_under_condition_0, condition);
                            }
                            trace_write!(self, diag::Exiting_conditional_exports);
                            return result;
                        } else {
                            trace_write!(
                                self,
                                diag::Failed_to_resolve_under_condition_0,
                                condition
                            );
                        }
                    } else {
                        trace_write!(self, diag::Saw_non_matching_condition_0, condition);
                    }
                }
                trace_write!(self, diag::Exiting_conditional_exports);
                return continue_searching();
            }
            JSONValueType::ARRAY => {
                if target.as_array().is_empty() {
                    trace_write!(
                        self,
                        diag::X_package_json_scope_0_has_invalid_type_for_target_of_specifier_1,
                        scope.package_directory,
                        module_name
                    );
                    return continue_searching();
                }
                for elem in target.as_array() {
                    let result = self.load_module_from_target_export_or_import(
                        extensions,
                        module_name,
                        scope,
                        is_imports,
                        elem,
                        subpath,
                        is_pattern,
                        key,
                    );
                    if result.is_some() {
                        return result;
                    }
                }
            }
            JSONValueType::NULL => {
                trace_write!(
                    self,
                    diag::X_package_json_scope_0_explicitly_maps_specifier_1_to_null,
                    scope.package_directory,
                    module_name
                );
                return unresolved();
            }
            _ => {}
        }

        trace_write!(
            self,
            diag::X_package_json_scope_0_has_invalid_type_for_target_of_specifier_1,
            scope.package_directory,
            module_name
        );
        continue_searching()
    }

    // Go: module/resolver.go:1094 tryLoadInputFileForPath
    pub fn try_load_input_file_for_path(
        &mut self,
        final_path: &str,
        entry: &str,
        package_path: &str,
        is_imports: bool,
    ) -> Option<Resolved> {
        // ts#64159: a directory-only target is never a file.
        if has_trailing_directory_separator(final_path) {
            return continue_searching();
        }
        let options = self.compiler_options.clone();
        let compare_paths_options = ComparePathsOptions {
            use_case_sensitive_file_names: self.resolver.host.fs().use_case_sensitive_file_names(),
            current_directory: self.resolver.host.get_current_directory().to_string(),
        };
        // Replace any references to outputs for files in the program with the input files to support package self-names used with outDir
        if !self.is_config_lookup
            && (!options.declaration_dir.is_empty() || !options.out_dir.is_empty())
            && !final_path.contains("/node_modules/")
            && (options.config_file_path.is_empty()
                || contains_path(
                    &get_directory_path(package_path),
                    &options.config_file_path,
                    &compare_paths_options,
                ))
        {
            // Note: this differs from Strada's tryLoadInputFileForPath in that it
            // does not attempt to perform "guesses", instead requring a clear root indicator.

            let root_dir: String = if !options.root_dir.is_empty() {
                // A `rootDir` compiler option strongly indicates the root location
                options.root_dir.clone()
            } else if !options.config_file_path.is_empty() {
                // When no explicit rootDir is set, treat the config file's directory as the project root, which establishes the common source directory, so no other locations need to be checked.
                get_directory_path(&options.config_file_path)
            } else {
                let diagnostic = new_diagnostic(
                    Node::NIL,
                    TextRange::default(),
                    if is_imports {
                        diag::The_project_root_is_ambiguous_but_is_required_to_resolve_import_map_entry_0_in_file_1_Supply_the_rootDir_compiler_option_to_disambiguate
                    } else {
                        diag::The_project_root_is_ambiguous_but_is_required_to_resolve_export_map_entry_0_in_file_1_Supply_the_rootDir_compiler_option_to_disambiguate
                    },
                    // replace empty string with `.` - the reverse of the operation done when entries are built - so main entrypoint errors don't look weird
                    args![if entry.is_empty() { "." } else { entry }, package_path],
                );
                self.diagnostics.push(diagnostic);
                return unresolved();
            };

            let candidate_directories = self.get_output_directories_for_base_directory(&root_dir);
            for candidate_dir in &candidate_directories {
                if contains_path(candidate_dir, final_path, &compare_paths_options) {
                    // The matched export is looking up something in either the out declaration or js dir, now map the written path back into the source dir and source extension
                    let path_fragment = if final_path.len() > candidate_dir.len() {
                        &final_path[candidate_dir.len() + 1..] // +1 to also remove directory separator
                    } else {
                        ""
                    };
                    let possible_input_base = combine_paths(&root_dir, &[path_fragment]);
                    let js_and_dts_extensions = [
                        EXTENSION_MJS,
                        EXTENSION_CJS,
                        EXTENSION_JS,
                        EXTENSION_JSON,
                        EXTENSION_DMTS,
                        EXTENSION_DCTS,
                        EXTENSION_DTS,
                    ];
                    for ext in js_and_dts_extensions {
                        if file_extension_is(&possible_input_base, ext) {
                            let input_exts = get_possible_original_input_extension_for_extension(
                                &possible_input_base,
                            );
                            for possible_ext in &input_exts {
                                if !extension_is_ok(self.extensions, possible_ext) {
                                    continue;
                                }
                                let possible_input_with_input_extension =
                                    change_extension(&possible_input_base, possible_ext);
                                if self
                                    .resolver
                                    .host
                                    .fs()
                                    .file_exists(&possible_input_with_input_extension)
                                {
                                    let extensions = self.extensions;
                                    let resolved = self.load_file_name_from_package_json_field(
                                        extensions,
                                        &possible_input_with_input_extension,
                                        "",
                                    );
                                    if resolved.is_some() {
                                        return resolved;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        continue_searching()
    }

    // Go: module/resolver.go:955 getOutputDirectoriesForBaseDirectory (at 673a5f17d713;
    // ts#64159 makes it getOutputDirectories, module/resolver.go:1165)
    #[must_use]
    pub fn get_output_directories_for_base_directory(
        &self,
        common_source_dir_guess: &str,
    ) -> Vec<String> {
        // Config file output paths are processed to be relative to the host's current directory, while
        // otherwise the paths are resolved relative to the common source dir the compiler puts together
        let current_directory = self.resolver.host.get_current_directory();
        let current_dir = if !self.compiler_options.config_file_path.is_empty() {
            current_directory
        } else {
            common_source_dir_guess
        };
        let mut candidate_directories: Vec<String> = Vec::new();
        if !self.compiler_options.declaration_dir.is_empty() {
            candidate_directories.push(get_normalized_absolute_path(
                &combine_paths(current_dir, &[&self.compiler_options.declaration_dir]),
                current_directory,
            ));
        }
        if !self.compiler_options.out_dir.is_empty()
            && self.compiler_options.out_dir != self.compiler_options.declaration_dir
        {
            candidate_directories.push(get_normalized_absolute_path(
                &combine_paths(current_dir, &[&self.compiler_options.out_dir]),
                current_directory,
            ));
        }
        candidate_directories
    }

    // Go: module/resolver.go:1176 loadModuleFromNearestNodeModulesDirectory
    pub fn load_module_from_nearest_node_modules_directory(
        &mut self,
        types_scope_only: bool,
    ) -> Option<Resolved> {
        let mut mode = RESOLUTION_MODE_COMMON_JS;
        if self.esm_mode || self.condition_matches("import") {
            mode = RESOLUTION_MODE_ESM;
        }
        // Do (up to) two passes through node_modules:
        //   1. For each ancestor node_modules directory, try to find:
        //      i.  TS/DTS files in the implementation package
        //      ii. DTS files in the @types package
        //   2. For each ancestor node_modules directory, try to find:
        //      i.  JS files in the implementation package
        let priority_extensions =
            self.extensions & (Extensions::TYPE_SCRIPT | Extensions::DECLARATION);
        let secondary_extensions = self
            .extensions
            .without(Extensions::TYPE_SCRIPT | Extensions::DECLARATION);
        // (1)
        if priority_extensions != Extensions::default() {
            trace_write!(
                self,
                diag::Searching_all_ancestor_node_modules_directories_for_preferred_extensions_Colon_0,
                priority_extensions.string()
            );
            let result = self.load_module_from_nearest_node_modules_directory_worker(
                priority_extensions,
                mode,
                types_scope_only,
            );
            if result.is_some() {
                return result;
            }
        }
        // (2)
        if secondary_extensions != Extensions::default() && !types_scope_only {
            trace_write!(
                self,
                diag::Searching_all_ancestor_node_modules_directories_for_fallback_extensions_Colon_0,
                secondary_extensions.string()
            );
            return self.load_module_from_nearest_node_modules_directory_worker(
                secondary_extensions,
                mode,
                types_scope_only,
            );
        }
        continue_searching()
    }

    // Go: module/resolver.go:1208 loadModuleFromNearestNodeModulesDirectoryWorker
    // PORT: Go does not read `mode`.
    pub fn load_module_from_nearest_node_modules_directory_worker(
        &mut self,
        ext: Extensions,
        _mode: ResolutionMode,
        types_scope_only: bool,
    ) -> Option<Resolved> {
        let containing_directory = self.containing_directory.clone();
        let (result, _) = for_each_ancestor_directory(
            &containing_directory,
            |directory: &str| -> (Option<Resolved>, bool) {
                // !!! stop at global cache
                if get_base_file_name(directory) != "node_modules" {
                    let result = self.load_module_from_immediate_node_modules_directory(
                        ext,
                        directory,
                        types_scope_only,
                    );
                    let stop = result.is_some();
                    return (result, stop);
                }
                (continue_searching(), false)
            },
        );
        result
    }

    // Go: module/resolver.go:1222 loadModuleFromImmediateNodeModulesDirectory
    pub fn load_module_from_immediate_node_modules_directory(
        &mut self,
        extensions: Extensions,
        directory: &str,
        types_scope_only: bool,
    ) -> Option<Resolved> {
        let node_modules_folder = combine_paths(directory, &["node_modules"]);
        if !self
            .resolver
            .host
            .fs()
            .directory_exists(&node_modules_folder)
        {
            trace_write!(
                self,
                diag::Directory_0_does_not_exist_skipping_all_lookups_in_it,
                node_modules_folder
            );
            return continue_searching();
        }

        if !types_scope_only {
            let name = self.name.clone();
            let package_result = self.load_module_from_specific_node_modules_directory(
                extensions,
                &name,
                &node_modules_folder,
            );
            if package_result.is_some() {
                return package_result;
            }
        }

        if extensions.intersects(Extensions::DECLARATION) {
            let node_modules_at_types = combine_paths(&node_modules_folder, &["@types"]);
            if !self
                .resolver
                .host
                .fs()
                .directory_exists(&node_modules_at_types)
            {
                trace_write!(
                    self,
                    diag::Directory_0_does_not_exist_skipping_all_lookups_in_it,
                    node_modules_at_types
                );
                return continue_searching();
            }
            let name = self.name.clone();
            let mangled = self.mangle_scoped_package_name(&name);
            return self.load_module_from_specific_node_modules_directory(
                Extensions::DECLARATION,
                &mangled,
                &node_modules_at_types,
            );
        }

        continue_searching()
    }

    // Go: module/resolver.go:1251 loadModuleFromSpecificNodeModulesDirectory
    pub fn load_module_from_specific_node_modules_directory(
        &mut self,
        ext: Extensions,
        module_name: &str,
        node_modules_directory: &str,
    ) -> Option<Resolved> {
        // Strip any trailing directory separator so that imports like `pkg/` and `pkg`
        // produce identical `candidate` and `packageDirectory` strings. Otherwise the
        // `package.json` info cache (which is keyed by normalized path but stores the
        // caller's `PackageDirectory` verbatim) can hand back, under concurrent
        // inserts, an entry whose `PackageDirectory` doesn't match `candidate`,
        // causing `loadNodeModuleFromDirectoryWorker`'s `ComparePaths(candidate, ...)`
        // check to fail and skip loading the package's `main`/`types` entry.
        // https://github.com/microsoft/typescript-go/issues/3526
        // ts#64544: the candidate and the package directory resolve as
        // module paths, so the segments of a dynamic directory stay encoded.
        // ts#64159: Go `resolutionCandidateFromDirectoryPath(nodeModulesDirectory,
        // moduleName)` (resolver.go:1259) keeps the directory intent of "pkg/"
        // in the candidate (N: the separator was removed); the candidate
        // directory has none.
        let candidate = resolve_path_for_module(
            node_modules_directory,
            module_name,
            has_trailing_directory_separator(module_name),
        );
        let candidate_directory = candidate_directory_path(&candidate).into_owned();
        let (package_name, rest) = parse_package_name(module_name);
        let mut package_directory = remove_trailing_directory_separator(&resolve_path_for_module(
            node_modules_directory,
            &package_name,
            true,
        ))
        .to_string();
        if package_name.is_empty() {
            package_directory = candidate_directory.clone();
        }

        if self.resolve_package_directory_only {
            if self.resolver.host.fs().directory_exists(&package_directory) {
                return Some(Resolved {
                    path: package_directory,
                    ..Default::default()
                });
            }
            return continue_searching();
        }

        let mut root_package_info: Option<Rc<InfoCacheEntry>> = None;
        // First look for a nested package.json, as in `node_modules/foo/bar/package.json`
        let mut package_info = self.get_package_json_info(&candidate_directory);
        // But only if we're not respecting export maps (if we are, we might redirect around this location)
        if !rest.is_empty() && entry_exists(&package_info) {
            if self.features.intersects(NodeResolutionFeatures::EXPORTS) {
                root_package_info = self.get_package_json_info(&package_directory);
            }
            if !entry_exists(&root_package_info)
                || root_package_info
                    .as_ref()
                    .unwrap()
                    .contents
                    .as_ref()
                    .unwrap()
                    .fields
                    .path_fields
                    .exports
                    .json_value
                    .type_
                    == JSONValueType::NOT_PRESENT
            {
                let from_file = self.load_module_from_file(ext, &candidate);
                if from_file.is_some() {
                    return from_file;
                }

                let from_directory = self.load_node_module_from_directory_worker(
                    ext,
                    &candidate_directory,
                    &package_info,
                );
                if let Some(mut from_directory) = from_directory {
                    from_directory.package_id =
                        self.get_package_id(&from_directory.path, &package_info);
                    return Some(from_directory);
                }
            }
        }

        // PORT: Go defines `loader` here and it reads `packageInfo` by
        // reference. `packageInfo` is only reassigned below, before any call
        // of `loader`, so the Rust closure is defined after the reassignment.
        if !rest.is_empty() {
            package_info = root_package_info;
            if package_info.is_none() {
                // Previous `packageInfo` may have been from a nested package.json; ensure we have the one from the package root now.
                package_info = self.get_package_json_info(&package_directory);
            }
        }

        let mut loader =
            |state: &mut Self, extensions: Extensions, candidate: &str| -> Option<Resolved> {
                let loader_candidate_directory = candidate_directory_path(candidate);
                if !rest.is_empty() || !state.esm_mode {
                    let from_file = state.load_module_from_file(extensions, candidate);
                    if let Some(mut from_file) = from_file {
                        from_file.package_id = state.get_package_id(&from_file.path, &package_info);
                        return Some(from_file);
                    }
                }
                let from_directory = state.load_node_module_from_directory_worker(
                    extensions,
                    &loader_candidate_directory,
                    &package_info,
                );
                if let Some(mut from_directory) = from_directory {
                    from_directory.package_id =
                        state.get_package_id(&from_directory.path, &package_info);
                    return Some(from_directory);
                }
                if rest.is_empty()
                    && entry_exists(&package_info)
                    && {
                        let exports_type = package_info
                            .as_ref()
                            .unwrap()
                            .contents
                            .as_ref()
                            .unwrap()
                            .fields
                            .path_fields
                            .exports
                            .json_value
                            .type_;
                        exports_type == JSONValueType::NOT_PRESENT
                            || exports_type == JSONValueType::NULL
                    }
                    && state.esm_mode
                {
                    // EsmMode disables index lookup in `loadNodeModuleFromDirectoryWorker` generally, however non-relative package resolutions still assume
                    // a default `index.js` entrypoint if no `main` or `exports` are present
                    let index_result = state.load_module_from_file(
                        extensions,
                        &combine_paths(&loader_candidate_directory, &["index.js"]),
                    );
                    if let Some(mut index_result) = index_result {
                        index_result.package_id =
                            state.get_package_id(&index_result.path, &package_info);
                        return Some(index_result);
                    }
                }
                continue_searching()
            };

        if package_info.is_some() {
            self.resolved_package_directory = true;
            if self.features.intersects(NodeResolutionFeatures::EXPORTS)
                && entry_exists(&package_info)
                && !package_info
                    .as_ref()
                    .unwrap()
                    .contents
                    .as_ref()
                    .unwrap()
                    .fields
                    .path_fields
                    .exports
                    .json_value
                    .is_falsy()
            {
                // package exports are higher priority than file/directory/typesVersions lookups and (and, if there's exports present*, blocks them)
                // *Well, weirdly enough a top-level `"exports": null` does NOT block fallback resolution.
                // https://github.com/microsoft/TypeScript/pull/49327
                return self.load_module_from_exports(
                    &package_info,
                    ext,
                    &combine_paths(".", &[&rest]),
                );
            }
            if !rest.is_empty() && entry_exists(&package_info) {
                let contents = package_info.as_ref().unwrap().contents.clone().unwrap();
                let version_paths = contents.get_version_paths(self.get_trace_func());
                if version_paths.exists() {
                    trace_write!(
                        self,
                        diag::X_package_json_has_a_typesVersions_entry_0_that_matches_compiler_version_1_looking_for_a_pattern_to_match_module_name_2,
                        version_paths.version,
                        version(),
                        rest
                    );
                    let paths = version_paths.get_paths();
                    let path_patterns = try_parse_patterns(paths);
                    let from_paths = self.try_load_module_using_paths(
                        ext,
                        &rest,
                        &package_directory,
                        paths,
                        &path_patterns,
                        &mut loader,
                    );
                    if from_paths.is_some() {
                        return from_paths;
                    }
                }
            }
        }
        loader(self, ext, &candidate)
    }
}

// Go: module/resolver_internal_test.go (ts#64159)
// PORT: Go uses `vfstest.FromMap`; these tests write the files to a
// temporary directory and use the OS file system, as `cache.rs` tests do.
#[cfg(test)]
mod internal_tests {
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

    /// A temporary directory `name` with `files` in it; its path with "/".
    fn write_files(name: &str, files: &[(&str, &str)]) -> String {
        let root = std::env::temp_dir().join(format!("ts_goport_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (path, text) in files {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        root.to_string_lossy().replace('\\', "/")
    }

    fn test_resolver(dir: &str, options: CompilerOptions) -> DefaultResolver {
        let host: Rc<dyn ResolutionHost> = Rc::new(TestHost {
            fs: osvfs_fs(),
            current_directory: dir.to_string(),
        });
        new_resolver(ResolverOptions {
            host: Some(host),
            compiler_options: Some(Rc::new(options)),
            ..Default::default()
        })
    }

    // Go: module/resolver_internal_test.go:85 TestPackageJSONPathWithTrailingSeparatorDoesNotResolveAsFile
    #[test]
    fn package_json_path_with_trailing_separator_does_not_resolve_as_file() {
        let dir = write_files(
            "pjson_trailing_separator",
            &[("project/index.d.ts", "export {};")],
        );
        let resolver = test_resolver(&dir, CompilerOptions::default());
        let mut state = ResolutionState::zero(&resolver, resolver.compiler_options.clone());
        // Go resolveResolutionCandidate("/project", "index.d.ts/").
        let candidate = resolve_path_for_module(&format!("{dir}/project"), "index.d.ts/", true);
        let result = state.load_file_name_from_package_json_field(
            Extensions::DECLARATION,
            &candidate,
            "./index.d.ts/",
        );
        assert!(
            result.is_none(),
            "directory-only candidate resolved as {:?}",
            result.map(|r| r.path)
        );
    }

    // Go: module/resolver_internal_test.go:102 TestExtensionReplacementPreservesEmptyAndDotStems
    #[test]
    fn extension_replacement_preserves_empty_and_dot_stems() {
        let mut errors = Vec::new();
        for (index, (import_name, expected)) in [
            (".js", "project/.native.ts"),
            ("..js", "project/..native.ts"),
            ("...js", "project/...native.ts"),
            (".js", "project/.native.d.ts"),
        ]
        .into_iter()
        .enumerate()
        {
            let dir = write_files(
                &format!("extension_replacement_stems_{index}"),
                &[(expected, "export {};")],
            );
            let resolver = test_resolver(
                &dir,
                CompilerOptions {
                    module_suffixes: Some(vec![".native".to_string(), String::new()]),
                    ..Default::default()
                },
            );
            let mut state = ResolutionState::zero(&resolver, resolver.compiler_options.clone());
            // Go resolveResolutionCandidate("/project", importName).
            let candidate = resolve_path_for_module(&format!("{dir}/project"), import_name, false);
            let result = state.load_module_from_file_no_implicit_extensions(
                Extensions::TYPE_SCRIPT | Extensions::DECLARATION,
                &candidate,
            );
            let expected = format!("{dir}/{expected}");
            if result.as_ref().map(|r| r.path.as_str()) != Some(expected.as_str()) {
                errors.push(format!(
                    "{import_name:?}: got {:?}, expected {expected:?}",
                    result.map(|r| r.path)
                ));
            }
        }
        assert!(errors.is_empty(), "{}", errors.join("\n"));
    }

    // Go: module/resolver_internal_test.go:132 TestOutputDirectoriesRemoveTrailingSeparators
    // PORT: the port keeps N's `getOutputDirectoriesForBaseDirectory`; with
    // rooted options the guess directory is not read.
    #[test]
    fn output_directories_remove_trailing_separators() {
        let resolver = test_resolver(
            "/",
            CompilerOptions {
                declaration_dir: "/project/types/".to_string(),
                out_dir: "/project/dist/".to_string(),
                ..Default::default()
            },
        );
        let state = ResolutionState::zero(&resolver, resolver.compiler_options.clone());
        assert_eq!(
            state.get_output_directories_for_base_directory(""),
            ["/project/types", "/project/dist"]
        );
    }
}
