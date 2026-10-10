use crate::ls::autoimport::prelude::*;

// Port of Go `ls/autoimport/view.go`.
//
// PORT (whole file):
// - Go `*View` is shared (`Rc<View>` in the import adder) and fills three
//   caches lazily through the pointer. The caches use `RefCell` / `Cell`, so
//   every method takes `&self`.
// - Go passes `v.program` where a `modulespecifiers.ModuleSpecifierGenerationHost`
//   is needed. The host for the installed program is
//   `modulespecifiers::ProgramHost` (PORT: installed program, plan D-LS1).
// - Go `*ast.SourceFile` passed as an `ast.HasFileName` to a program method
//   is `source_file_has_file_name(file)`.

use crate::flags_macros::go_enum;
use crate::frontend::compiler;
use crate::frontend::core_ls_ext::{first_non_zero, min_all_func};
use crate::frontend::core_nodemodules::UNPREFIXED_NODE_CORE_MODULES;
use crate::frontend::module;
use crate::frontend::tspath;
use crate::ls::lsutil;
use crate::lsp::lsproto;
use crate::modulespecifiers;
use std::cell::Cell;

// Go: ls/autoimport/view.go:21 View
// PORT: Go `checker *checker.Checker` (ts#64178) is not a field; see
// `new_view`.
// PORT: Go `*collections.Set[string]` `conditions` is never nil after
// `NewView`, so it is a plain set. Go `*collections.MultiMap` is
// `IndexMap<K, Vec<V>>` behind an `Rc` (Go returns the pointer). The lazy
// Go fields (`allowedEndings`, `existingImports`, `shouldUseRequireForFixes`)
// are `RefCell<Option<..>>` / `Cell<Option<bool>>` (`None` is Go nil).
pub struct View {
    pub registry: Rc<Registry>,
    pub importing_file: Node,
    pub importing_file_path: tspath::Path,
    pub program: Rc<compiler::NewProgram>,
    pub preferences: modulespecifiers::UserPreferences,
    pub project_id: ProjectID,

    pub allowed_endings: RefCell<Option<Vec<modulespecifiers::ModuleSpecifierEnding>>>,
    pub conditions: FxHashSet<String>,
    pub should_use_uri_style_node_core_modules: Tristate,
    pub existing_imports: RefCell<Option<Rc<IndexMap<ModuleID, Vec<ExistingImport>>>>>,
    pub should_use_require_for_fixes: Cell<Option<bool>>,
}

/// Go passes an `*ast.SourceFile` where a program method takes an
/// `ast.HasFileName`.
// PORT: ts_goport program methods take `&dyn HasFileName`; a file root
// `Node` does not implement it, so its file name and path are copied
// (Go `ast.NewHasFileName`).
pub fn source_file_has_file_name(file: Node) -> HasFileNameImpl {
    new_has_file_name(source_file_file_name(file), &source_file_info(file).path)
}

// Go: ls/autoimport/view.go:37 NewView
// PORT: Go takes `typeChecker` and stores it in the view (ts#64178). The
// Rust checker is a `RefCell` that the caller already borrows, so the view
// does not keep it: the methods that read Go `v.checker` take `ch` (see
// `fix.rs`).
pub fn new_view(
    registry: Rc<Registry>,
    importing_file: Node,
    project_id: ProjectID,
    program: Rc<compiler::NewProgram>,
    preferences: modulespecifiers::UserPreferences,
) -> View {
    let mut importing_file_path = tspath::Path(source_file_info(importing_file).path.clone());
    // PORT: Go `importingFile.CanonicalSourceFile()` is read from the program's
    // `ParsedSourceFile` for the file root `Node` (see `ls/utilities.rs`).
    if let Some(canonical) = program
        .get_source_file_by_path(&importing_file_path)
        .and_then(|file| file.canonical_source_file())
    {
        importing_file_path = canonical.path().clone();
    }
    let conditions = module::get_conditions(
        program.options(),
        program.get_default_resolution_mode_for_file(&source_file_has_file_name(importing_file)),
    )
    .into_iter()
    .collect();
    let should_use_uri_style_node_core_modules =
        lsutil::should_use_uri_style_node_core_modules(importing_file, &program);
    View {
        registry,
        importing_file,
        importing_file_path,
        program,
        project_id,
        preferences,
        conditions,
        should_use_uri_style_node_core_modules,
        allowed_endings: RefCell::new(None),
        existing_imports: RefCell::new(None),
        should_use_require_for_fixes: Cell::new(None),
    }
}

impl View {
    // Go: ls/autoimport/view.go:58 getAllowedEndings
    pub fn get_allowed_endings(&self) -> Vec<modulespecifiers::ModuleSpecifierEnding> {
        if self.allowed_endings.borrow().is_none() {
            let resolution_mode =
                self.program
                    .get_default_resolution_mode_for_file(&source_file_has_file_name(
                        self.importing_file,
                    ));
            // PORT: Go passes `v.program` as the host; see the file header.
            let allowed_endings = modulespecifiers::get_allowed_endings_in_preferred_order(
                &self.preferences,
                &modulespecifiers::ProgramHost,
                self.program.options(),
                &self.importing_file,
                "",
                resolution_mode,
            );
            *self.allowed_endings.borrow_mut() = Some(allowed_endings);
        }
        self.allowed_endings
            .borrow()
            .clone()
            .expect("set above when nil")
    }
}

// Go: ls/autoimport/view.go:73 QueryKind
go_enum!(QueryKind, i32 {
    WORD_PREFIX = 0; // QueryKindWordPrefix
    EXACT_MATCH = 1; // QueryKindExactMatch
    CASE_INSENSITIVE_MATCH = 2; // QueryKindCaseInsensitiveMatch
});

impl View {
    // Go: ls/autoimport/view.go:81 Search
    // PORT: `search_exported`, because Go also has the unexported `search`.
    // Go `bucket.Index` is a pointer; a nil index panics as in Go.
    pub fn search_exported(&self, query: &str, kind: QueryKind) -> Vec<Rc<Export>> {
        let search_fn = |bucket: &RegistryBucket| -> Vec<Rc<Export>> {
            let index = bucket
                .index
                .as_ref()
                .unwrap_or_else(|| crate::core::go_nil_dereference())
                .borrow();
            match kind {
                QueryKind::WORD_PREFIX => index.search_word_prefix(query),
                QueryKind::EXACT_MATCH => index.find(query, true),
                QueryKind::CASE_INSENSITIVE_MATCH => index.find(query, false),
                _ => crate::core::go_panic("unreachable".to_string()),
            }
        };

        self.search(&search_fn)
    }

    // Go: ls/autoimport/view.go:98 SearchByExportID
    pub fn search_by_export_id(&self, id: &ExportID) -> Vec<Rc<Export>> {
        let search = |bucket: &RegistryBucket| -> Vec<Rc<Export>> {
            let index = bucket
                .index
                .as_ref()
                .unwrap_or_else(|| crate::core::go_nil_dereference())
                .borrow();
            index
                .entries
                .iter()
                .filter(|e| e.export_id == *id)
                .cloned()
                .collect()
        };

        self.search(&search)
    }

    // Go: ls/autoimport/view.go:108 search
    // PORT: Go `*collections.Set[string]` `allowedPackages` is
    // `Option<FxHashSet<String>>` (`None` is nil).
    pub fn search(
        &self,
        search_fn: &dyn Fn(&RegistryBucket) -> Vec<Rc<Export>>,
    ) -> Vec<Rc<Export>> {
        let mut results: Vec<Rc<Export>> = Vec::new();
        let importing_file_path = tspath::Path(source_file_info(self.importing_file).path.clone());

        if let Some(bucket) = self.registry.projects.get(&self.project_id) {
            let exports = search_fn(&**bucket);
            results.reserve(exports.len());
            for e in exports {
                // ts#64159: view.go:114, only a file module is the importing file.
                if e.module_id.as_path_key() == Some(&importing_file_path) {
                    // Don't auto-import from the importing file itself
                    continue;
                }
                results.push(e);
            }
        }

        // Compute the set of packages accessible to the importing file.
        // This includes packages from package.json dependencies (aggregated from ancestor directories)
        // plus packages that are directly imported by the project's program files.
        // If no package.json is found, allowedPackages remains nil and all packages are allowed.
        let mut allowed_packages: Option<FxHashSet<String>> = None;
        importing_file_path
            .get_directory_path()
            .for_each_ancestor_directory(|dir_path: tspath::Path| -> ((), bool) {
                if let Some(dir) = self.registry.directories.get(&dir_path) {
                    let dir = dir.borrow();
                    if let Some(pj) = dir.package_json.as_ref().filter(|pj| pj.exists())
                        && pj
                            .contents
                            .as_ref()
                            .unwrap_or_else(|| crate::core::go_nil_dereference())
                            .parseable
                    {
                        // Initialize to empty set if this is the first package.json we've seen
                        if allowed_packages.is_none() {
                            allowed_packages = Some(FxHashSet::default());
                        }
                        add_package_json_dependencies(
                            pj.contents
                                .as_ref()
                                .unwrap_or_else(|| crate::core::go_nil_dereference()),
                            allowed_packages.as_mut().expect("set above when nil"),
                        );
                    }
                }
                ((), false)
            });
        // If we found at least one package.json, also include packages directly imported by the project
        if let Some(allowed) = &allowed_packages {
            if let Some(bucket) = self.registry.projects.get(&self.project_id) {
                // Go: allowedPackages.UnionedWith(bucket.ResolvedPackageNames)
                let mut result = allowed.clone();
                if let Some(other) = &bucket.resolved_package_names {
                    result.extend(other.iter().cloned());
                }
                allowed_packages = Some(result);
            }
        }

        let mut exclude_packages: FxHashSet<String> = FxHashSet::default();
        importing_file_path
            .get_directory_path()
            .for_each_ancestor_directory(|dir_path: tspath::Path| -> ((), bool) {
                if let Some(node_modules_bucket) = self.registry.node_modules.get(&dir_path) {
                    let exports = search_fn(&**node_modules_bucket);
                    results.reserve(exports.len());
                    for e in exports {
                        // Exclude packages found in lower node_modules (shadowing)
                        if exclude_packages.contains(&e.package_name) {
                            continue;
                        }
                        // If allowedPackages is nil, no package.json was found, so include all packages.
                        // Otherwise, only include packages that are dependencies or directly imported.
                        if let Some(allowed) = &allowed_packages
                            && !allowed.contains(&e.package_name)
                        {
                            continue;
                        }
                        results.push(e);
                    }

                    // As we go up the directory tree, exclude packages found in lower node_modules
                    if let Some(package_files) = &node_modules_bucket.package_files {
                        for pkg_name in package_files.keys() {
                            exclude_packages.insert(pkg_name.clone());
                        }
                    }
                }
                ((), false)
            });
        results
    }
}

// Go: ls/autoimport/view.go:175 FixAndExport
#[derive(Clone, Debug)]
pub struct FixAndExport {
    pub fix: Rc<Fix>,
    pub export: Rc<Export>,
}

/// Go `unicode.IsUpper`: general category Lu.
// PORT: the Rust `Uppercase` property is Lu plus `Other_Uppercase`; the
// `Other_Uppercase` ranges are removed so the result is Go's category test
// (the same helper as in `util.rs`, which keeps it private).
fn unicode_is_upper(c: char) -> bool {
    if !c.is_uppercase() {
        return false;
    }
    !matches!(
        c as u32,
        0x2160..=0x216F | 0x24B6..=0x24CF | 0x1F130..=0x1F149 | 0x1F150..=0x1F169 | 0x1F170..=0x1F189
    )
}

impl View {
    // Go: ls/autoimport/view.go:180 GetCompletions
    // PORT: `ch` is the request checker, Go `v.checker` (ts#64178); the
    // caller passes it (as the pinned ImportAdder decision does).
    // Go `grouped` is a map: `IndexMap` in insertion order.
    // PORT: Go map order is random. It changes values, not only the order of
    // ties, through the per-file specifier cache (`specifiers.rs`): the cache
    // key is `export.path` but the value comes from `export.module_file_name`.
    // A relative module augmentation export (`declare module '../..'`) has the
    // augmented file as its module and the declaring file as its path, and
    // the merge below keeps the last path of equal export ids. If its group is
    // computed first, the declaring file's own exports get the augmented
    // module's specifier for the rest of the session. goport computes those
    // groups after the other groups (the common tsgo result: Hono goldens
    // poison no middleware in 240 of 296 sessions). `fixes` keeps the
    // insertion order, so the sort input and its ties do not change.
    // The cache lives as long as the importing file is open, in Go too: a
    // completion that computes only the augmentation (prefix `V` for
    // `Vars`) poisons the declaring file for the later completions (Go
    // every time when one file augments). When several files augment, Go
    // merges a random last path on each bucket build (map order in
    // buildProjectBucket), so a long session poisons a random set of them;
    // goport always merges the last in program order. knownprob1 S3, the
    // hono long plan at edit 440: Go gives "." to `requestId` in 7 of 13
    // runs and to other middleware in some; goport poisons none, Go's whole
    // answer in 3 of 10 runs (aispec1).
    pub fn get_completions(
        &self,
        ch: &mut Checker,
        prefix: &str,
        position: lsproto::Position,
        for_jsx: bool,
        is_type_only_location: bool,
    ) -> Vec<FixAndExport> {
        let results = self.search_exported(prefix, QueryKind::WORD_PREFIX);

        #[derive(Clone, Debug, PartialEq, Eq, Hash)]
        struct ExportGroupKey {
            target: ExportID,
            name: String,
            ambient_module_or_package_name: String,
        }
        let mut grouped: IndexMap<ExportGroupKey, Vec<Rc<Export>>> =
            IndexMap::with_capacity(results.len());
        'outer: for e in &results {
            let name = e.name();
            if !is_identifier_text(&name, LanguageVariant::STANDARD) {
                continue;
            }
            // PORT: Go `rune(name[0])` is the first byte as a rune.
            if for_jsx && !(unicode_is_upper(char::from(name.as_bytes()[0])) || e.is_renameable()) {
                continue;
            }
            let mut target = e.export_id.clone();
            if e.target != ExportID::default() {
                target = e.target.clone();
            }
            let mut key = ExportGroupKey {
                target,
                name,
                ambient_module_or_package_name: first_non_zero([
                    e.ambient_module_name(),
                    e.package_name.clone(),
                ]),
            };
            if e.package_name == "@types/node" || e.path.0.contains("/node_modules/@types/node/") {
                if UNPREFIXED_NODE_CORE_MODULES
                    .contains_key(key.ambient_module_or_package_name.as_str())
                {
                    // Group URI-style and non-URI style node core modules together so the ranking logic
                    // is allowed to drop one if an explicit preference is detected.
                    key.ambient_module_or_package_name =
                        format!("node:{}", key.ambient_module_or_package_name);
                }
            }
            if let Some(existing) = grouped.get_mut(&key) {
                for i in 0..existing.len() {
                    let ex = existing[i].clone();
                    if e.export_id == ex.export_id {
                        // Go: slices.Replace(existing, i, i+1, &Export{..})
                        existing[i] = Rc::new(Export {
                            export_id: e.export_id.clone(),
                            module_file_name: e.module_file_name.clone(),
                            // ts#64159: view.go:219
                            unresolved_module_specifier: e.unresolved_module_specifier.clone(),
                            package_name: e.package_name.clone(),
                            is_type_only: e.is_type_only || ex.is_type_only,
                            syntax: e.syntax.min(ex.syntax),
                            flags: e.flags | ex.flags,
                            script_element_kind: e.script_element_kind.min(ex.script_element_kind),
                            script_element_kind_modifiers: e.script_element_kind_modifiers
                                | ex.script_element_kind_modifiers,
                            local_name: e.local_name.clone(),
                            target: e.target.clone(),
                            path: e.path.clone(),
                            ..Default::default()
                        });
                        continue 'outer;
                    }
                }
            }
            grouped.entry(key).or_default().push(e.clone());
        }

        let mut fixes: Vec<FixAndExport> = Vec::with_capacity(results.len());

        // PORT: see the note above. A relative module augmentation export:
        // project file (no package), module is a file that is not its path.
        // ts#64159: the module ID kind says it is a file module.
        let is_augmentation = |e: &Export| {
            e.package_name.is_empty()
                && e.module_id
                    .as_path_key()
                    .is_some_and(|module_path| *module_path != e.path)
        };
        let groups: Vec<&Vec<Rc<Export>>> = grouped.values().collect();
        let (late, early): (Vec<usize>, Vec<usize>) =
            (0..groups.len()).partition(|&i| groups[i].iter().all(|e| is_augmentation(&**e)));
        let mut group_fixes: Vec<Vec<FixAndExport>> = vec![Vec::new(); groups.len()];

        for i in early.into_iter().chain(late) {
            let exps = groups[i];
            let mut fixes_for_group: Vec<FixAndExport> = Vec::with_capacity(exps.len());
            for e in exps {
                for fix in self.get_fixes(ch, e, for_jsx, is_type_only_location, Some(position)) {
                    fixes_for_group.push(FixAndExport {
                        fix,
                        export: e.clone(),
                    });
                }
            }
            // Go: compareFixes
            group_fixes[i] = min_all_func(&fixes_for_group, |a, b| {
                self.compare_fixes_for_ranking(&a.fix, &b.fix)
            });
        }
        for group in group_fixes {
            fixes.extend(group);
        }

        // The client will do additional sorting by SortText and Label, so we don't
        // need to consider the name in our sorting here; we only need to produce a
        // stable relative ordering between completions that the client will consider
        // equivalent.
        gostd::slices::sort_func(&mut fixes, |a, b| {
            self.compare_fixes_for_sorting(&a.fix, &b.fix)
        });

        fixes
    }
}
