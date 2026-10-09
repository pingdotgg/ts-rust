use crate::api::prelude::*;

// Port of Go `internal/api/session.go`, lines 1-1271: the snapshot
// registries, `Session`, `NewSession`, the checker and language service
// setup, the `HandleRequest` dispatch and the snapshot, project, symbol and
// type handlers up to `handleGetTargetOfSignature`, the transpile handlers
// (tsgo#4849), then `handleGetImportAdderEdits` and `toAPITextEdits`
// (tsgo#3881) and `originalTextOffset` (tsgo#4712). Lines 1272-2484 are
// `session_p2.rs`.
//
// PORT notes for both files:
// - Go `*ast.Symbol`, `*checker.Type` and `*checker.Signature` carry their
//   data. A Rust `SymbolId`, `TypeId` or `SignatureId` is an index into the
//   arenas of one checker, so a registry entry keeps the checker that made
//   the handle: `(Rc<RefCell<Checker>>, handle)`. Reads borrow that checker.
// - Handlers call a checker method in its own statement
//   (`setup.checker.borrow_mut().x(..)`), so no borrow is held when a
//   `new_*_response` borrows the checker again to read the result.
// - A handle from the registry is used with the setup checker only through
//   `checker_symbol`, `checker_type` and `checker_signature`. Go can hand a
//   pointer of one checker to another checker. For a symbol, the setup
//   checker first catches up to the binder lineage, so it has every binder
//   symbol of a live file version under the same index, then uses the same
//   symbol, or a shadow of a symbol that the other checker made: a copy in
//   the setup checker's arena with the same id and no links
//   (`import_symbol`). A type or signature of another checker calls
//   `unported!`.
// - Go `defer setup.done()`: `CheckerSetup::done` is a `Release` guard. It
//   releases the checker when `setup` drops at the end of the handler.
// - Current program: node handles (`node_handle_from`, `resolve_node_handle`)
//   and the source file encoder read lazy JSDoc through `prog()`. So every
//   handler that reads a program keeps it current for its whole body.
//   `setup_checker` does it through `done`. The handlers with no checker
//   call `ls_program::enter` after `get_program`. The `resolve*PropertyOf*`
//   helpers have no project, so they enter the program of the checker that
//   owns the handle.
// - Go mutexes (`snapshotsMu`, the registry mutexes) are dropped: the
//   session runs on the dispatch thread (PORTING "Threads").
// - The profile handlers run `crate::pprof`, which writes profiles with no
//   samples (see its module comment).

use crate::api::encoder;
use crate::api::proto;
use crate::api::requestfilesystem;
use crate::astnav;
use crate::emitter::emitter::EmitOnly;
use crate::frontend::compiler;
use crate::frontend::core_context::{self, CheckerLifetime};
use crate::frontend::json_ext::{AnyValue, JsonValue};
use crate::frontend::tsoptions;
use crate::frontend::tspath;
use crate::frontend::vfs;
use crate::frontend::vfs::Fs as _;
use crate::gostd::{self, Context, GoError, errors};
use crate::ls;
use crate::ls::autoimport;
use crate::program::ls_program;
use crate::project;
use crate::transpile;
use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};

// Go: api/session.go:51 sessionIDCounter
pub static SESSION_ID_COUNTER: AtomicU64 = AtomicU64::new(0);

// Go: api/session.go:52 sourceFileSymbolIndexKey (ts#64518)
static SOURCE_FILE_SYMBOL_INDEX_KEY: std::sync::LazyLock<
    crate::ast::source_file_ls::SourceFileDataKey<Rc<FxHashMap<SymbolID, SymbolId>>>,
> = std::sync::LazyLock::new(crate::ast::source_file_ls::new_source_file_data_key);

// Go: api/session.go:55 getSourceFileSymbolIndex (ts#64518)
// The binder symbols that a source file owns, by handle.
// PORT: `symbols` is an arena that holds the binder symbols of `file` (a
// checker of a program that has the file, or a binder lineage copy). The
// binder gives each lineage symbol the same index in every such arena, so
// one index per file serves them all.
pub fn get_source_file_symbol_index(
    symbols: &SymbolArena,
    source_file: Node,
) -> Rc<FxHashMap<SymbolID, SymbolId>> {
    crate::ast::source_file_ls::source_file_get_or_compute_data(
        source_file,
        &*SOURCE_FILE_SYMBOL_INDEX_KEY,
        |file: Node| {
            let mut index: FxHashMap<SymbolID, SymbolId> = FxHashMap::default();
            // PORT: Go's recursive addSymbol; a work list here (parents and
            // members form long chains).
            let mut work: Vec<SymbolId> = Vec::new();
            let mut add_symbols = |work: &mut Vec<SymbolId>| {
                while let Some(symbol) = work.pop() {
                    if symbol.is_nil()
                        || symbols.sym(symbol).flags.intersects(SymbolFlags::TRANSIENT)
                    {
                        continue;
                    }
                    go_assert!(get_source_file_of_symbol(symbols, symbol) == file);
                    let id = symbol_handle(symbols, symbol);
                    if let Some(&existing) = index.get(&id) {
                        go_assert!(symbols.id_slot(existing) == symbols.id_slot(symbol));
                        continue;
                    }
                    index.insert(id, symbol);
                    let sym = symbols.sym(symbol);
                    let (parent, export_symbol, members, exports) =
                        (sym.parent, sym.export_symbol, sym.members, sym.exports);
                    // Go visits the parent, the export symbol, then the
                    // members and exports; the stack pops them in that order.
                    let mut children: Vec<SymbolId> = Vec::new();
                    for table in [members, exports] {
                        if !table.is_nil() {
                            children.extend(symbols.values(table));
                        }
                    }
                    work.extend(children.into_iter().rev());
                    work.push(export_symbol);
                    work.push(parent);
                }
            };
            for &node in encoder::get_node_index_table(file).nodes.iter() {
                if node.is_nil() {
                    continue;
                }
                work.push(node.symbol());
                add_symbols(&mut work);
                work.push(node.local_symbol());
                add_symbols(&mut work);
                let locals = node.locals();
                if !locals.is_nil() {
                    for symbol in symbols.values(locals) {
                        work.push(symbol);
                        add_symbols(&mut work);
                    }
                }
            }
            let bind = crate::ast::file_bind_data(file);
            if !bind.global_exports.is_nil() {
                for symbol in symbols.values(bind.global_exports) {
                    work.push(symbol);
                    add_symbols(&mut work);
                }
            }
            for module in bind.pattern_ambient_modules.iter() {
                work.push(module.symbol);
                add_symbols(&mut work);
            }
            Rc::new(index)
        },
    )
}

// Go: api/session.go:53 snapshotData
// snapshotData holds the per-snapshot state including the snapshot itself
// and symbol/type registries scoped to this snapshot.
// Multiple clients may hold references to the same snapshot via ref counting;
// the registries are cleaned up when refCount reaches zero.
// PORT: registry values keep the checker that owns the handle (file header).
pub struct SnapshotData {
    // ts#64518
    pub handle: SnapshotID,
    pub snapshot: Rc<project::Snapshot>,
    // ts#64115: the request file system the snapshot was made with (Go nil
    // is `None`).
    pub file_system: Option<Rc<dyn vfs::Fs>>,
    pub ref_count: Cell<i32>,

    // ts#64204
    pub open_projects: FxHashSet<tspath::Path>,
    pub open_files: FxHashSet<tspath::Path>,

    // Symbol IDs come from ast.GetSymbolId, a global atomic counter, so the same
    // *ast.Symbol pointer always has the same unique ID across all projects in the
    // snapshot. Symbols are registered snapshot-wide to ensure identity semantics:
    // querying the same symbol from two different projects returns the same handle.
    pub symbol_registry: RefCell<FxHashMap<SymbolID, (Rc<RefCell<Checker>>, SymbolId)>>,

    // symbolCanonicalProjects records, for each registered symbol, the project it was
    // first observed in. Because symbols are shared snapshot-wide (binder symbols are
    // attached to source files, which can be shared across projects), lookups that need
    // a project context (e.g. member/export ordering, node handle resolution) but don't
    // receive one from the caller default to this canonical project. First-writer wins so
    // the choice is stable. Guarded by symbolRegistryMu.
    pub symbol_canonical_projects: RefCell<FxHashMap<SymbolID, project::ID>>,

    pub project_registries: RefCell<FxHashMap<project::ID, Rc<ProjectRegistryData>>>,
}

// Go: api/session.go:91 projectRegistryData
// projectRegistryData holds per-project type and signature registries.
// Types and signatures use per-checker sequential IDs, so the same local ID
// can appear in multiple projects. Separate maps per project prevent collisions
// and allow clean teardown when a project is removed.
pub struct ProjectRegistryData {
    pub type_registry: RefCell<FxHashMap<TypeID, (Rc<RefCell<Checker>>, TypeId)>>,

    pub signature_registry: RefCell<FxHashMap<SignatureID, (Rc<RefCell<Checker>>, SignatureId)>>,
}

impl SnapshotData {
    // Go: api/session.go:100 getProgram
    // getProgram looks up a program from a project handle within this snapshot.
    pub fn get_program(
        &self,
        project_handle: &project::ID,
    ) -> Result<Rc<compiler::NewProgram>, GoError> {
        let proj = self.get_project(project_handle)?;

        let program = proj.borrow().get_program();
        let Some(program) = program else {
            return Err(errors::errorf(
                format!("{}: project has no program", *ERR_CLIENT_ERROR),
                vec![ERR_CLIENT_ERROR.clone()],
            ));
        };

        Ok(program)
    }

    // Go: api/session.go:115 getProject
    // getProject looks up a project from a project handle within this snapshot.
    // ts#64319: looked up by project ID.
    pub fn get_project(
        &self,
        project_handle: &project::ID,
    ) -> Result<Rc<RefCell<project::Project>>, GoError> {
        let proj = self.snapshot.project_collection.get_project(project_handle);
        let Some(proj) = proj else {
            return Err(errors::errorf(
                format!(
                    "{}: project {} not found",
                    *ERR_CLIENT_ERROR, project_handle.0
                ),
                vec![ERR_CLIENT_ERROR.clone()],
            ));
        };
        Ok(proj)
    }

    // Go: api/session.go:175 snapshotData.nodeHandleFrom
    // nodeHandleFrom creates an index-based node handle (index.kind.path), building a node index table
    // for the file on-demand if needed.
    pub fn node_handle_from(&self, node: Node) -> NodeHandle {
        node_handle_from(node)
    }

    // Go: api/session.go:134 getOrCreateProjectRegistry
    // getOrCreateProjectRegistry returns the registry for the given project, creating it if needed.
    pub fn get_or_create_project_registry(
        &self,
        project_id: &project::ID,
    ) -> Rc<ProjectRegistryData> {
        if project_id.0.is_empty() {
            panic!("getOrCreateProjectRegistry: empty project ID");
        }
        self.project_registries
            .borrow_mut()
            .entry(project_id.clone())
            .or_insert_with(|| {
                Rc::new(ProjectRegistryData {
                    type_registry: RefCell::new(FxHashMap::default()),
                    signature_registry: RefCell::new(FxHashMap::default()),
                })
            })
            .clone()
    }

    // Go: api/session.go:222 snapshotData.newSymbolResponse
    // newSymbolResponse classifies a symbol's ownership before exposing its identity to a client.
    // Only snapshot-owned symbols are registered in the snapshot; file-owned symbols are resolved
    // through their source file.
    // PORT: `checker` owns `symbol`; its arena holds the symbol data.
    pub fn new_symbol_response(
        &self,
        checker: &Rc<RefCell<Checker>>,
        symbol: SymbolId,
        canonical_project: &project::ID,
    ) -> Option<SymbolResponse> {
        if symbol.is_nil() {
            return None;
        }
        // ts#64518
        if symbol_owner_file(&checker.borrow().symbols, symbol).is_some() {
            return Some(new_file_symbol_response(&checker.borrow().symbols, symbol));
        }
        let (id, project) = self.register_symbol(checker, symbol, canonical_project);
        let reference = SymbolReference {
            id,
            kind: SymbolOwnerKind::SNAPSHOT,
            snapshot: self.handle,
            project,
            file: None,
        };
        Some(build_symbol_response(
            &checker.borrow().symbols,
            symbol,
            reference,
            Node::NIL,
        ))
    }

    // Go: api/session.go:202 registerSymbol
    // registerSymbol registers a symbol in the snapshot's registry and returns its handle along with
    // its canonical project. The canonical project is the project the symbol was first observed in
    // (first writer wins for stability) and is always non-empty: every symbol handed to a client must
    // carry a project so that project-scoped follow-up lookups (members/exports, parent, node
    // resolution) have a default context. Callers must supply a non-empty project.
    pub fn register_symbol(
        &self,
        checker: &Rc<RefCell<Checker>>,
        symbol: SymbolId,
        canonical_project: &project::ID,
    ) -> (SymbolID, project::ID) {
        if symbol.is_nil() {
            return (SymbolID(0), project::ID::default());
        }
        if canonical_project.0.is_empty() {
            panic!("registerSymbol requires a non-empty canonical project");
        }
        let (id, slot) = {
            let c = checker.borrow();
            (symbol_handle(&c.symbols, symbol), c.symbols.id_slot(symbol))
        };
        let mut registry = self.symbol_registry.borrow_mut();
        if let Some(existing) = registry.get(&id) {
            // PORT: Go compares `*ast.Symbol` pointers. The id slot stands
            // for the Go symbol (`SymbolArena::id_slot`): a binder symbol has
            // one slot in every checker that has it, and a shadow has the
            // slot of its origin (`import_symbol`).
            let same = existing.0.borrow().symbols.id_slot(existing.1) == slot;
            if !same {
                panic!("duplicate symbol");
            }
        } else {
            registry.insert(id, (checker.clone(), symbol));
        }
        let project = self
            .symbol_canonical_projects
            .borrow_mut()
            .entry(id)
            .or_insert_with(|| canonical_project.clone())
            .clone();
        (id, project)
    }

    // Go: api/session.go:229 newTypeResponse
    // newTypeResponse registers a type in the project's registry and returns the response.
    pub fn new_type_response(
        &self,
        project_id: &project::ID,
        checker: &Rc<RefCell<Checker>>,
        t: TypeId,
    ) -> Option<TypeResponse> {
        if t.is_nil() {
            return None;
        }
        let id = self.register_type(project_id, checker, t);
        let mut resp = new_type_response(&checker.borrow(), t, id);
        // ts#64518
        {
            let c = checker.borrow();
            resp.symbol = new_symbol_reference(&c.symbols, c.ty(t).symbol());
            if let Some(alias) = c.ty(t).alias() {
                resp.alias_symbol = new_symbol_reference(&c.symbols, alias.symbol());
            }
        }
        // ts#64397
        let is_mapped = checker
            .borrow()
            .ty(t)
            .object_flags()
            .intersects(ObjectFlags::MAPPED);
        if is_mapped {
            // Go `mapped.ResolveComponents(c, t)` (checker/types.go).
            {
                let mut c = checker.borrow_mut();
                c.get_type_parameter_from_mapped_type(t);
                c.get_constraint_type_from_mapped_type(t);
                c.get_name_type_from_mapped_type(t);
                c.get_template_type_from_mapped_type(t);
            }
            let (type_parameter, constraint_type, name_type, template_type) = {
                let c = checker.borrow();
                let mapped = c.ty(t).as_mapped_type();
                (
                    mapped.type_parameter,
                    mapped.constraint_type,
                    mapped.name_type,
                    mapped.template_type,
                )
            };
            resp.type_parameter = self.register_type(project_id, checker, type_parameter);
            resp.constraint_type = self.register_type(project_id, checker, constraint_type);
            resp.name_type = self.register_type(project_id, checker, name_type);
            resp.template_type = self.register_type(project_id, checker, template_type);
        }
        // ts#64109
        // PORT: the labeled declarations are copied out of the checker arena
        // before the node handles are built.
        let labeled_declarations: Option<Vec<Node>> = {
            let c = checker.borrow();
            if c.is_tuple_type_target(t) {
                Some(
                    c.ty(t)
                        .as_tuple_type()
                        .element_infos()
                        .iter()
                        .map(|info| info.labeled_declaration())
                        .collect(),
                )
            } else {
                None
            }
        };
        if let Some(element_infos) = labeled_declarations {
            for (i, &declaration) in element_infos.iter().enumerate() {
                if declaration.is_some() {
                    if resp.labeled_element_declarations.is_empty() {
                        resp.labeled_element_declarations =
                            vec![NodeHandle::default(); element_infos.len()];
                    }
                    resp.labeled_element_declarations[i] = self.node_handle_from(declaration);
                }
            }
        }
        Some(resp)
    }

    // Go: api/session.go:256 registerType
    pub fn register_type(
        &self,
        project_id: &project::ID,
        checker: &Rc<RefCell<Checker>>,
        t: TypeId,
    ) -> TypeID {
        if t.is_nil() {
            return TypeID(0);
        }
        let id = type_handle(t);
        let reg = self.get_or_create_project_registry(project_id);
        let mut registry = reg.type_registry.borrow_mut();
        let existing = registry.get(&id);

        if let Some(existing) = existing {
            // PORT: Go compares `*checker.Type` pointers: same checker and
            // same arena index.
            if !(Rc::ptr_eq(&existing.0, checker) && existing.1 == t) {
                panic!("duplicate type");
            }
            return id;
        }
        registry.insert(id, (checker.clone(), t));
        id
    }

    // Go: api/session.go:157 resolveSymbolHandle
    // resolveSymbolHandle resolves a symbol handle within the snapshot's registry.
    // PORT: returns the checker that owns the symbol with it (file header).
    pub fn resolve_symbol_handle(
        &self,
        handle: SymbolID,
    ) -> Result<(Rc<RefCell<Checker>>, SymbolId), GoError> {
        if handle.0 == 0 {
            return Err(errors::errorf(
                format!("{}: empty symbol handle", *ERR_CLIENT_ERROR),
                vec![ERR_CLIENT_ERROR.clone()],
            ));
        }

        let symbol = self.symbol_registry.borrow().get(&handle).cloned();

        let Some(symbol) = symbol else {
            return Err(errors::errorf(
                format!(
                    "{}: symbol handle {} not found in snapshot registry",
                    *ERR_CLIENT_ERROR, handle.0
                ),
                vec![ERR_CLIENT_ERROR.clone()],
            ));
        };

        Ok(symbol)
    }

    // Go: api/session.go:174 resolveTypeHandle
    // resolveTypeHandle resolves a type handle within the project's registry.
    // PORT: returns the checker that owns the type with it (file header).
    pub fn resolve_type_handle(
        &self,
        project_id: &project::ID,
        handle: TypeID,
    ) -> Result<(Rc<RefCell<Checker>>, TypeId), GoError> {
        if handle.0 == 0 {
            return Err(errors::errorf(
                format!("{}: empty type handle", *ERR_CLIENT_ERROR),
                vec![ERR_CLIENT_ERROR.clone()],
            ));
        }
        if project_id.0.is_empty() {
            return Err(errors::errorf(
                format!(
                    "{}: empty project ID for type handle {}",
                    *ERR_CLIENT_ERROR, handle.0
                ),
                vec![ERR_CLIENT_ERROR.clone()],
            ));
        }

        let reg = self.project_registries.borrow().get(project_id).cloned();

        let Some(reg) = reg else {
            return Err(errors::errorf(
                format!(
                    "{}: type handle {} not found (no registry for project {})",
                    *ERR_CLIENT_ERROR, handle.0, project_id.0
                ),
                vec![ERR_CLIENT_ERROR.clone()],
            ));
        };

        let t = reg.type_registry.borrow().get(&handle).cloned();

        let Some(t) = t else {
            return Err(errors::errorf(
                format!(
                    "{}: type handle {} not found in project registry",
                    *ERR_CLIENT_ERROR, handle.0
                ),
                vec![ERR_CLIENT_ERROR.clone()],
            ));
        };

        Ok(t)
    }

    // Go: api/session.go:191 resolveSignatureHandle
    // resolveSignatureHandle resolves a signature handle within the project's registry.
    // PORT: returns the checker that owns the signature with it (file header).
    pub fn resolve_signature_handle(
        &self,
        project_id: &project::ID,
        handle: SignatureID,
    ) -> Result<(Rc<RefCell<Checker>>, SignatureId), GoError> {
        if handle.0 == 0 {
            return Err(errors::errorf(
                format!("{}: empty signature handle", *ERR_CLIENT_ERROR),
                vec![ERR_CLIENT_ERROR.clone()],
            ));
        }
        if project_id.0.is_empty() {
            return Err(errors::errorf(
                format!(
                    "{}: empty project ID for signature handle {}",
                    *ERR_CLIENT_ERROR, handle.0
                ),
                vec![ERR_CLIENT_ERROR.clone()],
            ));
        }

        let reg = self.project_registries.borrow().get(project_id).cloned();

        let Some(reg) = reg else {
            return Err(errors::errorf(
                format!(
                    "{}: signature handle {} not found (no registry for project {})",
                    *ERR_CLIENT_ERROR, handle.0, project_id.0
                ),
                vec![ERR_CLIENT_ERROR.clone()],
            ));
        };

        let sig = reg.signature_registry.borrow().get(&handle).cloned();

        let Some(sig) = sig else {
            return Err(errors::errorf(
                format!(
                    "{}: signature handle {} not found in project registry",
                    *ERR_CLIENT_ERROR, handle.0
                ),
                vec![ERR_CLIENT_ERROR.clone()],
            ));
        };

        Ok(sig)
    }

    // Go: api/session.go:208 newSignatureResponse
    // newSignatureResponse registers a signature in the project's registry and returns the response.
    pub fn new_signature_response(
        &self,
        project_id: &project::ID,
        checker: &Rc<RefCell<Checker>>,
        sig: SignatureId,
    ) -> Option<SignatureResponse> {
        if sig.is_nil() {
            return None;
        }
        let c = checker.borrow();
        let s = c.sig(sig);
        let mut resp = SignatureResponse {
            id: self.register_signature(project_id, checker, sig),
            flags: s.flags().0,
            ..Default::default()
        };

        if s.declaration().is_some() {
            resp.declaration = self.node_handle_from(s.declaration());
        }

        if !s.type_parameters().is_empty() {
            resp.type_parameters = type_handles(s.type_parameters());
        }

        // ts#64518
        if !s.parameters().is_empty() {
            resp.parameters = s
                .parameters()
                .iter()
                .filter_map(|&parameter| new_symbol_reference(&c.symbols, parameter))
                .collect();
        }

        if s.this_parameter().is_some() {
            resp.this_parameter = new_symbol_reference(&c.symbols, s.this_parameter());
        }

        if s.target().is_some() {
            resp.target = signature_handle(s.target());
        }

        Some(resp)
    }

    // Go: api/session.go:382 registerSignature
    pub fn register_signature(
        &self,
        project_id: &project::ID,
        checker: &Rc<RefCell<Checker>>,
        sig: SignatureId,
    ) -> SignatureID {
        if sig.is_nil() {
            return SignatureID(0);
        }
        let id = signature_handle(sig);
        let reg = self.get_or_create_project_registry(project_id);
        let mut registry = reg.signature_registry.borrow_mut();
        let existing = registry.get(&id);

        if let Some(existing) = existing {
            // PORT: Go compares `*checker.Signature` pointers: same checker
            // and same arena index.
            if !(Rc::ptr_eq(&existing.0, checker) && existing.1 == sig) {
                panic!("duplicate signature");
            }
            return id;
        }
        registry.insert(id, (checker.clone(), sig));
        id
    }
}

/// Go `checkerSetup.resolveSymbolHandle` (api/session.go:777, ts#64518)
/// with the setup fields that it reads. `handleGetImportAdderEdits` calls
/// it with a setup literal that has no checker lease.
pub fn resolve_symbol_reference_for_checker(
    sd: &SnapshotData,
    snapshot: SnapshotID,
    program: &compiler::NewProgram,
    checker: &Rc<RefCell<Checker>>,
    reference: &SymbolReference,
) -> Result<(Rc<RefCell<Checker>>, SymbolId), GoError> {
    let client_error = |text: String| {
        Err(errors::errorf(
            format!("{}: {text}", *ERR_CLIENT_ERROR),
            vec![ERR_CLIENT_ERROR.clone()],
        ))
    };
    if reference.kind == SymbolOwnerKind::SNAPSHOT {
        if reference.snapshot != snapshot || reference.file.is_some() {
            return client_error(
                "snapshot symbol reference does not match the requested checker".to_string(),
            );
        }
        sd.resolve_symbol_handle(reference.id)
    } else if reference.kind == SymbolOwnerKind::FILE {
        let Some(file) = reference
            .file
            .as_ref()
            .filter(|_| reference.snapshot.0 == 0 && reference.project.0.is_empty())
        else {
            return client_error("invalid file symbol reference".to_string());
        };
        let source_file = program.get_source_file_by_path(&tspath::Path(file.path.clone()));
        let Some(source_file) =
            source_file.filter(|parsed| new_parsed_source_file_descriptor(parsed) == *file)
        else {
            return client_error("source file is not part of the requested program".to_string());
        };
        let symbol = {
            let c = checker.borrow();
            get_source_file_symbol_index(&c.symbols, source_file.root)
                .get(&reference.id)
                .copied()
        };
        let Some(symbol) = symbol else {
            return client_error(format!(
                "symbol handle {} not found in source file",
                reference.id.0
            ));
        };
        Ok((checker.clone(), symbol))
    } else {
        client_error(format!(
            "invalid symbol reference kind {}",
            reference.kind.0
        ))
    }
}

// Go: api/session.go:206 symbolOwnerFile (ts#64518)
// symbolOwnerFile returns the source file that owns a symbol's client identity, or nil when the
// symbol is owned by its snapshot. Content-mapped outputs live in a cache that cannot yet be
// addressed by file key, so their binder symbols remain snapshot-owned.
// PORT: `symbols` is the arena that holds `symbol`; nil is `Node::NIL`.
pub fn symbol_owner_file(symbols: &SymbolArena, symbol: SymbolId) -> Option<Node> {
    if symbols.sym(symbol).flags.intersects(SymbolFlags::TRANSIENT) {
        return None;
    }
    let file = get_source_file_of_symbol(symbols, symbol);
    if crate::ast::source_file_is_content_mapped(file) {
        return None;
    }
    Some(file)
}

// Go: api/session.go:241 newFileSymbolResponse (ts#64518)
pub fn new_file_symbol_response(symbols: &SymbolArena, symbol: SymbolId) -> SymbolResponse {
    let file = symbol_owner_file(symbols, symbol);
    go_assert!(file.is_some(), "Expected a file-owned symbol");
    let file = file.unwrap_or(Node::NIL);
    let descriptor = new_source_file_descriptor(file);
    let reference = SymbolReference {
        id: symbol_handle(symbols, symbol),
        kind: SymbolOwnerKind::FILE,
        file: Some(descriptor),
        ..Default::default()
    };
    build_symbol_response(symbols, symbol, reference, file)
}

// Go: api/session.go:253 buildSymbolResponse (ts#64518)
// PORT: Go nil `owner` is `Node::NIL`.
pub fn build_symbol_response(
    symbols: &SymbolArena,
    symbol: SymbolId,
    reference: SymbolReference,
    owner: Node,
) -> SymbolResponse {
    let sym = symbols.sym(symbol);
    let mut resp = SymbolResponse {
        reference,
        // PORT: Go `ast.EscapeSymbolName(symbol.Name)`. A private name
        // first gets the Go class id (`go_symbol_name`).
        name: escape_symbol_name(&go_symbol_name(symbols, symbol)),
        flags: sym.flags.0,
        check_flags: sym.check_flags.0,
        parent: new_symbol_reference(symbols, sym.parent),
        export_symbol: new_symbol_reference(symbols, sym.export_symbol),
        ..Default::default()
    };
    if owner.is_some() {
        // A client resolves a file-owned symbol's relationships through its own source file.
        go_assert!(
            sym.parent.is_nil() || symbol_owner_file(symbols, sym.parent) == Some(owner),
            "File-owned symbol parent belongs to another owner"
        );
        go_assert!(
            sym.export_symbol.is_nil()
                || symbol_owner_file(symbols, sym.export_symbol) == Some(owner),
            "File-owned export symbol belongs to another owner"
        );
    }
    if !sym.declarations.is_empty() {
        resp.declarations = sym
            .declarations
            .iter()
            .map(|&decl| symbol_node_handle_from(decl, owner))
            .collect();
    }
    if sym.value_declaration.is_some() {
        resp.value_declaration = symbol_node_handle_from(sym.value_declaration, owner);
    }
    resp
}

// Go: api/session.go:279 newSymbolReference (ts#64518)
// newSymbolReference creates a compact reference to a symbol without registering it. Clients resolve
// it from their caches or fetch the full response through the corresponding property method.
pub fn new_symbol_reference(
    symbols: &SymbolArena,
    symbol: SymbolId,
) -> Option<CompactSymbolReference> {
    if symbol.is_nil() {
        return None;
    }
    let mut reference = CompactSymbolReference {
        id: symbol_handle(symbols, symbol),
        file: String::new(),
    };
    if let Some(file) = symbol_owner_file(symbols, symbol) {
        reference.file = source_file_node_id(file).to_string();
    }
    Some(reference)
}

// Go: api/session.go:291 symbolNodeHandleFrom (ts#64518)
pub fn symbol_node_handle_from(node: Node, owner: Node) -> NodeHandle {
    if owner.is_some() {
        go_assert!(
            get_source_file_of_node(node) == owner,
            "File-owned symbol declaration belongs to another source file"
        );
    }
    node_handle_from(node)
}

// Go: api/session.go:298 nodeHandleFrom (ts#64518)
pub fn node_handle_from(node: Node) -> NodeHandle {
    let source_file = get_source_file_of_node(node);
    let table = encoder::get_node_index_table(source_file);
    let idx = table.get_index(node);
    let path = source_file_info(source_file).path.clone();
    NodeHandle(format!("{}.{}.{}", idx, node.kind() as i16, path))
}

// Go: api/session.go:2180 newSourceFileDescriptor (ts#64518)
// PORT: Go reads `ParseOptions()` and `ScriptKind` from the
// `*ast.SourceFile`; here they come from its parsed-file record.
pub fn new_source_file_descriptor(source_file: Node) -> SourceFileDescriptor {
    let parsed = ls_program::parsed_source_file(source_file)
        .unwrap_or_else(|| unported!("newSourceFileDescriptor of a file with no parse record"));
    new_parsed_source_file_descriptor(&parsed)
}

/// `new_source_file_descriptor` of the file of a parse record that the
/// caller holds (a lease).
pub fn new_parsed_source_file_descriptor(
    parsed: &crate::frontend::parser::ParsedSourceFile,
) -> SourceFileDescriptor {
    let parse_options = &parsed.parse_options;
    let mut parse_options_key: u32 = 0;
    if parse_options.external_module_indicator_options.jsx {
        parse_options_key |= 1;
    }
    if parse_options.external_module_indicator_options.force {
        parse_options_key |= 2;
    }
    SourceFileDescriptor {
        file_name: parse_options.file_name.clone(),
        path: parse_options.path.0.clone(),
        content_hash: encoder::parsed_source_file_hash(parsed),
        parse_options_key: parse_options_key.to_string(),
        script_kind: parsed.script_kind,
        node_id: source_file_node_id(parsed.root).to_string(),
    }
}

// Go: api/session.go:2201 sourceFileNodeID (ts#64518)
// sourceFileNodeID is stable for one Go AST and changes when an equal parse-cache key is
// recreated, making it suitable for validating remote references without introducing another
// source-file identity or ownership registry.
pub fn source_file_node_id(source_file: Node) -> u64 {
    crate::ast::get_node_id(source_file)
}

// Go `strconv.ParseUint(s, base, bitSize)` for base 10 or 16: no sign, no
// prefix, no underscores. The errors are Go's `*strconv.NumError` texts.
fn parse_uint(s: &str, base: u32, bit_size: u32) -> Result<u64, GoError> {
    let num_error = |reason: &str| {
        errors::new(format!(
            "strconv.ParseUint: parsing {}: {reason}",
            gostd::strconv::quote(s)
        ))
    };
    if s.is_empty() || !s.chars().all(|c| c.is_digit(base)) {
        return Err(num_error("invalid syntax"));
    }
    match u64::from_str_radix(s, base) {
        Ok(n) if bit_size >= 64 || n >> bit_size == 0 => Ok(n),
        _ => Err(num_error("value out of range")),
    }
}

// Go: api/session.go:2205 SourceFileDescriptor.parseCacheKey (ts#64518)
impl SourceFileDescriptor {
    pub fn parse_cache_key(&self) -> Result<project::ParseCacheKey, GoError> {
        if self.content_hash.len() != 32 {
            return Err(errors::new(
                "content hash must contain 32 hexadecimal digits",
            ));
        }
        let parse_hex = |digits: &str| {
            parse_uint(digits, 16, 64)
                .map_err(|err| errors::errorf(format!("invalid content hash: {err}"), vec![err]))
        };
        let hi = parse_hex(self.content_hash.get(..16).unwrap_or(""))?;
        let lo = parse_hex(self.content_hash.get(16..).unwrap_or(""))?;
        let parse_options_key = parse_uint(&self.parse_options_key, 10, 32);
        let parse_options_key = match parse_options_key {
            Ok(key) if key & !3 == 0 => key,
            _ => {
                return Err(errors::new(format!(
                    "invalid parse options key {:?}",
                    self.parse_options_key
                )));
            }
        };
        if !is_valid_create_source_file_script_kind(self.script_kind) {
            return Err(errors::new(format!(
                "invalid script kind {}",
                self.script_kind.0
            )));
        }
        let options = crate::frontend::parser::SourceFileParseOptions {
            file_name: self.file_name.clone(),
            path: tspath::Path(self.path.clone()),
            external_module_indicator_options:
                crate::frontend::parser::ExternalModuleIndicatorOptions {
                    jsx: parse_options_key & 1 != 0,
                    force: parse_options_key & 2 != 0,
                },
        };
        Ok(project::new_parse_cache_key(
            &options,
            (u128::from(hi) << 64) | u128::from(lo),
            self.script_kind,
        ))
    }
}

/// PORT: Go hands a `*ast.Symbol` of any checker to `setup.checker`. A Rust
/// symbol handle indexes the arena of `owner`. This returns the symbol of
/// `checker`'s arena that is the same Go symbol. Go's checker reads the
/// declarations, `node.Symbol()` and `node.Locals()` of any bound file. So
/// `checker` first catches up to the binder lineage
/// (`program::catch_up_checker`): then it holds every binder symbol and
/// table of a live file version under the index that `owner` has, also a
/// version bound after `checker` was made. A symbol that `owner` made
/// (`is_own_index`) becomes a shadow (`import_symbol`).
pub fn checker_symbol(
    checker: &Rc<RefCell<Checker>>,
    owner: &Rc<RefCell<Checker>>,
    symbol: SymbolId,
) -> SymbolId {
    if symbol.is_nil() || Rc::ptr_eq(checker, owner) {
        return symbol;
    }
    let owner = owner.borrow();
    let mut checker = checker.borrow_mut();
    crate::program::catch_up_checker(&mut checker.symbols);
    import_symbol(&owner.symbols, &mut checker.symbols, symbol)
}

/// The symbol of arena `to` that is Go symbol `symbol` of arena `from`: the
/// same symbol when `to` has it (a binder symbol, which `to` has after it
/// caught up), else a shadow in `to` (`SymbolArena::push_shadow`): a copy
/// with the same id. The checker of `to` has no links for a shadow, as a Go
/// checker has none for the symbol of another checker, so it computes the
/// type from the declarations or dereferences nil (an instantiated symbol
/// has no target). The symbol and table fields of a shadow point into `to`
/// in the same way. Nodes and names are global and stay.
// PORT: Go shares the object, so a later write to its fields is seen by
// both checkers. Checkers write those fields almost only while they make or
// merge the symbol. A work list, not recursion: parents and members form
// cycles and long chains.
fn import_symbol(from: &SymbolArena, to: &mut SymbolArena, symbol: SymbolId) -> SymbolId {
    let mut work = Vec::new();
    let result = shadow_of(from, to, symbol, &mut work);
    while let Some((shadow, origin)) = work.pop() {
        let s = from.sym(origin);
        let (parent, export_symbol, members, exports) =
            (s.parent, s.export_symbol, s.members, s.exports);
        let parent = shadow_of(from, to, parent, &mut work);
        let export_symbol = shadow_of(from, to, export_symbol, &mut work);
        let members = import_table(from, to, members, &mut work);
        let exports = import_table(from, to, exports, &mut work);
        let s = to.sym_mut(shadow);
        s.parent = parent;
        s.export_symbol = export_symbol;
        s.members = members;
        s.exports = exports;
    }
    result
}

/// `symbol` of `from` in `to`: the same symbol, or its shadow. A new shadow
/// still holds the symbol and table fields of `from`, so it goes on `work`
/// with its origin, and `import_symbol` maps them.
fn shadow_of(
    from: &SymbolArena,
    to: &mut SymbolArena,
    symbol: SymbolId,
    work: &mut Vec<(SymbolId, SymbolId)>,
) -> SymbolId {
    if symbol.is_nil() {
        return symbol;
    }
    let origin = from.id_slot(symbol);
    if let Some(found) = to.symbol_at_slot(origin) {
        return found;
    }
    let shadow = to.push_shadow(from.sym(symbol).clone(), origin);
    work.push((shadow, symbol));
    shadow
}

/// `table` of `from` in `to`: the same table for a binder lineage table,
/// which `to` has after it caught up, else a new table in `to` with the same
/// entries in the same order, each symbol in `to` (`shadow_of`).
fn import_table(
    from: &SymbolArena,
    to: &mut SymbolArena,
    table: SymbolTable,
    work: &mut Vec<(SymbolId, SymbolId)>,
) -> SymbolTable {
    if table.is_nil() || !is_own_index(table.0) {
        return table;
    }
    let entries: Vec<_> = from
        .iter_names(table)
        .map(|(name, symbol)| (name, shadow_of(from, to, symbol, work)))
        .collect();
    to.push_table_from_entries(entries.into_iter())
}

/// PORT: as `checker_symbol`, for a type. Every type belongs to one checker.
/// Go can mix types of two checkers (a canceled persistent checker, a second
/// project); the port can not. A copy, as for a symbol, does not give Go's
/// answers: Go's checkers write lazy results (resolved members, return
/// types) into the shared type and read each other's results, and they
/// number types each from 1, so ids collide (`duplicate type`). Only one
/// object heap for all checkers would match that.
pub fn checker_type(
    checker: &Rc<RefCell<Checker>>,
    owner: &Rc<RefCell<Checker>>,
    t: TypeId,
) -> TypeId {
    if !Rc::ptr_eq(checker, owner) {
        unported!("api: type of another checker");
    }
    t
}

/// PORT: as `checker_type`, for a signature, for the same reason.
pub fn checker_signature(
    checker: &Rc<RefCell<Checker>>,
    owner: &Rc<RefCell<Checker>>,
    sig: SignatureId,
) -> SignatureId {
    if !Rc::ptr_eq(checker, owner) {
        unported!("api: signature of another checker");
    }
    sig
}

// Go: api/session.go:407 Session
// Session represents an API session that provides programmatic access
// to TypeScript language services through the LSP server.
// It implements the Handler interface to process incoming API requests.
// The session supports multiple active snapshots, each with their own
// symbol and type registries for maintaining object identity.
// PORT: ts#64163 gives the session a snapshot host, and a project session
// only in LSP mode (Go nil is `None`).
pub struct Session {
    pub id: String,
    pub snapshot_host: Rc<project::SnapshotHost>,
    pub owns_snapshot_host: bool,
    // PORT: Go `withLocale func(context.Context) context.Context`.
    pub with_locale: Rc<dyn Fn(&Context) -> Context>,
    pub project_session: Option<Rc<project::Session>>,

    // PORT: Go `closeOnce sync.Once`.
    pub close_once: Cell<bool>,

    // This is set to true when using MessagePackProtocol.
    pub use_binary_responses: bool,
    // ts#64061
    // PORT: Go `sync.Map` and `atomic.Uint64`; one thread.
    pub batch_response_pages: RefCell<FxHashMap<String, BatchResponsePage>>,
    pub next_batch_response_page_id: Cell<u64>,

    // snapshots maps snapshot handles to their data. Each snapshot has its own
    // symbol/type registries.
    // PORT: the port is one thread, so the Go `snapshotsMu` lock is not
    // ported.
    pub snapshots: RefCell<FxHashMap<SnapshotID, Rc<SnapshotData>>>,

    // openProjects, openFiles, and createdPrograms are the canonical LSP-state resources
    // owned by this API client. Guarded by languageServerUpdateMu.
    // PORT: `languageServerUpdateMu` is not ported (one thread).
    pub open_projects: RefCell<FxHashSet<tspath::Path>>,
    pub open_files: RefCell<FxHashSet<tspath::Path>>,
    pub created_programs: RefCell<FxHashSet<project::SyntheticProjectID>>,

    // ts#64299
    // PORT: Go `atomic.Uint64` and the mutexes are not ported (one thread).
    // The program resolution contexts are shared with the resolver factories
    // (module_resolution.rs header).
    pub next_module_resolver_id: Cell<u64>,
    pub module_resolvers: RefCell<FxHashMap<ModuleResolverID, Rc<ModuleResolverRegistration>>>,
    pub program_resolution_contexts: Rc<ProgramResolutionContexts>,
    pub conn: RefCell<Option<Rc<dyn ipc::Conn>>>,
    // ts#64158
    // PORT: the Go `buildMu` lock is not ported (one thread).
    // PORT: the orchestrator methods take `&mut self`, so each is in a
    // `RefCell`.
    pub build_orchestrators: RefCell<
        FxHashMap<
            BuildOrchestratorID,
            Rc<RefCell<crate::execute::build::orchestrator::Orchestrator>>,
        >,
    >,
    // ts#64434
    // PORT: the Go `sourceFileLeasesMu` lock is not ported (one thread).
    pub source_file_leases: RefCell<FxHashMap<SourceFileLeaseID, Rc<project::SourceFileLease>>>,
    pub next_source_file_lease_id: Cell<u64>,

    pub cpu_profiler: crate::pprof::CpuProfiler,
}

// Go: api/session.go batchResponsePage (ts#64061)
#[derive(Debug, Default)]
pub struct BatchResponsePage {
    pub encoded_responses: Vec<JsonValue>,
}

// Go: api/session.go:284 `var _ Handler = (*Session)(nil)`
// Ensure Session implements Handler
// PORT: the `impl Handler for Session` below.

// Go: api/session.go:467 SessionOptions
// SessionOptions configures an API session.
#[derive(Clone, Debug, Default)]
pub struct SessionOptions {
    // UseBinaryResponses enables binary responses for msgpack protocol.
    pub use_binary_responses: bool,
}

// Go: api/session.go DefaultMaxResponseBytesPerPage (ts#64061)
// DefaultMaxResponseBytesPerPage leaves room for base64 expansion beneath V8's
// maximum string length while rounding down to an even decimal value.
pub const DEFAULT_MAX_RESPONSE_BYTES_PER_PAGE: i32 = 300_000_000;

// Go: api/session.go NewLSPSession (ts#64163)
// NewLSPSession creates a new API session with the given project session.
pub fn new_lsp_session(
    project_session: Rc<project::Session>,
    options: Option<&SessionOptions>,
) -> Rc<Session> {
    let with_locale_session = project_session.clone();
    let mut s = new_session(
        project_session.snapshot_host.clone(),
        Some(Rc::new(move |ctx: &Context| {
            with_locale_session.with_current_locale(ctx)
        })),
        options,
    );
    s.project_session = Some(project_session);
    Rc::new(s)
}

// Go: api/session.go NewStandaloneSession (ts#64163)
// NewStandaloneSession creates an API session with an independently owned snapshot host.
pub fn new_standalone_session(
    init: &project::SessionInit,
    options: Option<&SessionOptions>,
) -> Rc<Session> {
    // Not in Go: the API process frees the file versions that it publishes
    // again and owns the nodes of their parses, as `project::new_session`
    // sets for the language server (`ast::free_file_versions`), so a
    // released source file lease or program frees its parse, as Go's GC
    // does.
    crate::ast::set_editor_process();
    // Not in Go: a standalone API process answers as plain tsgo, without
    // the Effect rules, unless TSGO_EFFECT_API=1 (`effect::rulerunner`).
    crate::effect::rulerunner::set_api_process();
    let snapshot_host = project::new_snapshot_host(init);
    let mut s = new_session(snapshot_host, None, options);
    s.owns_snapshot_host = true;
    Rc::new(s)
}

// Go: api/session.go newSession (ts#64163)
// PORT: returns the session by value so the two constructors can set their
// fields before it is shared.
pub fn new_session(
    snapshot_host: Rc<project::SnapshotHost>,
    with_locale: Option<Rc<dyn Fn(&Context) -> Context>>,
    options: Option<&SessionOptions>,
) -> Session {
    let id = SESSION_ID_COUNTER.fetch_add(1, Ordering::SeqCst) + 1;
    let with_locale = with_locale.unwrap_or_else(|| Rc::new(|ctx: &Context| ctx.clone()));
    let mut s = Session {
        id: format_session_id(id),
        snapshot_host,
        owns_snapshot_host: false,
        with_locale,
        project_session: None,
        close_once: Cell::new(false),
        use_binary_responses: false,
        batch_response_pages: RefCell::new(FxHashMap::default()),
        next_batch_response_page_id: Cell::new(0),
        snapshots: RefCell::new(FxHashMap::default()),
        open_projects: RefCell::new(FxHashSet::default()),
        open_files: RefCell::new(FxHashSet::default()),
        created_programs: RefCell::new(FxHashSet::default()),
        next_module_resolver_id: Cell::new(0),
        module_resolvers: RefCell::new(FxHashMap::default()),
        program_resolution_contexts: Rc::new(ProgramResolutionContexts::default()),
        conn: RefCell::new(None),
        build_orchestrators: RefCell::new(FxHashMap::default()),
        source_file_leases: RefCell::new(FxHashMap::default()),
        next_source_file_lease_id: Cell::new(0),
        cpu_profiler: crate::pprof::CpuProfiler::default(),
    };
    if let Some(options) = options {
        s.use_binary_responses = options.use_binary_responses;
    }
    s
}

// PORT: Go `project.SnapshotHost` satisfies `tsoptions.ParseConfigHost`
// through its `FS` and `GetCurrentDirectory` methods; the config handlers
// pass it (ts#64163). Rust needs the impl; it forwards to the inherent
// methods.
impl tsoptions::ParseConfigHost for project::SnapshotHost {
    fn fs(&self) -> Rc<dyn vfs::Fs> {
        project::SnapshotHost::fs(self)
    }

    fn get_current_directory(&self) -> String {
        project::SnapshotHost::get_current_directory(self)
    }
}

// Go: api/session.go languageServerSnapshotUpdate (ts#64204)
pub struct LanguageServerSnapshotUpdate {
    pub request: project::APISnapshotRequest,
    pub open_state: SnapshotOpenState,
}

impl LanguageServerSnapshotUpdate {
    // Go: api/session.go languageServerSnapshotUpdate.commit (ts#64204)
    pub fn commit(&self, s: &Session, snapshot: &project::Snapshot) {
        *s.open_projects.borrow_mut() = self.open_state.open_projects.clone();
        *s.open_files.borrow_mut() = self.open_state.open_files.clone();
        if let Some(remove_programs) = &self.request.remove_programs {
            for program_id in remove_programs {
                s.created_programs.borrow_mut().remove(program_id);
            }
        }
        for program in snapshot.created_programs() {
            let id = program.borrow().id();
            let (program_id, ok) = id.synthetic();
            if !ok {
                panic!("created program has non-synthetic project ID: {}", id.0);
            }
            s.created_programs.borrow_mut().insert(program_id);
        }
    }
}

// Go: api/session.go snapshotOpenState (ts#64204)
#[derive(Clone, Debug, Default)]
pub struct SnapshotOpenState {
    pub open_projects: FxHashSet<tspath::Path>,
    pub open_files: FxHashSet<tspath::Path>,
}

// Go: api/session.go:544 snapshotHandle
// snapshotHandle creates a snapshot handle from a snapshot's ID.
pub fn snapshot_handle(snapshot: &project::Snapshot) -> SnapshotID {
    SnapshotID(snapshot.id())
}

// Go: api/session.go:592 checkerSetup
// checkerSetup holds the common context needed by handlers that require a type checker.
// PORT: Go `done func()` is the `Release` guard; it runs when the setup drops.
pub struct CheckerSetup {
    pub sd: Rc<SnapshotData>,
    // ts#64518
    pub snapshot: SnapshotID,
    pub program: Rc<compiler::NewProgram>,
    pub checker: Rc<RefCell<Checker>>,
    pub done: ls_program::Release,
    pub project_id: project::ID,
}

impl CheckerSetup {
    // ts#64397: Go `checkerSetup.newTypeResponse` is gone; callers use
    // `setup.sd.new_type_response(&setup.project_id, &setup.checker, t)`.

    // Go: api/session.go:600 checkerSetup.newSymbolResponse
    pub fn new_symbol_response(&self, sym: SymbolId) -> Option<SymbolResponse> {
        self.sd
            .new_symbol_response(&self.checker, sym, &self.project_id)
    }

    // Go: api/session.go:604 checkerSetup.newSignatureResponse
    pub fn new_signature_response(&self, sig: SignatureId) -> Option<SignatureResponse> {
        self.sd
            .new_signature_response(&self.project_id, &self.checker, sig)
    }

    // Go: api/session.go checkerSetup.newIndexInfoResponse (ts#64264)
    // PORT: Go dereferences the `*TypeResponse` (`*setup.newTypeResponse(..)`),
    // which panics on nil.
    pub fn new_index_info_response(&self, info: IndexInfoId) -> Option<IndexInfoResponse> {
        if info.is_nil() {
            return None;
        }
        let (key_type, value_type, is_readonly, declaration) = {
            let c = self.checker.borrow();
            let info = c.index_info(info);
            (
                info.key_type(),
                info.value_type(),
                info.is_readonly(),
                info.declaration(),
            )
        };
        let mut result = IndexInfoResponse {
            key_type: self
                .sd
                .new_type_response(&self.project_id, &self.checker, key_type)
                .expect("invalid memory address or nil pointer dereference"),
            value_type: self
                .sd
                .new_type_response(&self.project_id, &self.checker, value_type)
                .expect("invalid memory address or nil pointer dereference"),
            is_readonly,
            ..Default::default()
        };
        if declaration.is_some() {
            result.declaration = self.sd.node_handle_from(declaration);
        }
        Some(result)
    }

    // Go: api/session.go:623 checkerSetup.resolveTypeHandle
    pub fn resolve_type_handle(
        &self,
        id: TypeID,
    ) -> Result<(Rc<RefCell<Checker>>, TypeId), GoError> {
        self.sd.resolve_type_handle(&self.project_id, id)
    }

    // Go: api/session.go:777 checkerSetup.resolveSymbolHandle
    // ts#64518: a snapshot reference must name this checker's snapshot; a
    // file reference must name a source file of this program.
    // PORT: a file-owned symbol is returned with the setup checker, whose
    // arena holds the binder symbols of its program's files.
    pub fn resolve_symbol_handle(
        &self,
        reference: &SymbolReference,
    ) -> Result<(Rc<RefCell<Checker>>, SymbolId), GoError> {
        resolve_symbol_reference_for_checker(
            &self.sd,
            self.snapshot,
            &self.program,
            &self.checker,
            reference,
        )
    }

    // Go: api/session.go:631 checkerSetup.resolveSignatureHandle
    pub fn resolve_signature_handle(
        &self,
        id: SignatureID,
    ) -> Result<(Rc<RefCell<Checker>>, SignatureId), GoError> {
        self.sd.resolve_signature_handle(&self.project_id, id)
    }

    // Go: api/session.go:637 checkerSetup.resolveLocation
    // resolveLocation resolves an optional location, given either as a node handle or as a
    // file and position. Returns nil when neither is provided.
    pub fn resolve_location(
        &self,
        handle: &NodeHandle,
        file: Option<&DocumentIdentifier>,
        position: Option<u32>,
    ) -> Result<Node, GoError> {
        if !handle.0.is_empty() {
            return self.sd.resolve_node_handle(&self.program, handle);
        }
        if let (Some(file), Some(position)) = (file, position) {
            let source_file = self
                .program
                .get_source_file(&file.to_file_name())
                .map_or(Node::NIL, |f| f.root);
            if source_file.is_nil() {
                return Err(errors::errorf(
                    format!(
                        "{}: source file not found: {}",
                        *ERR_CLIENT_ERROR,
                        file.string()
                    ),
                    vec![ERR_CLIENT_ERROR.clone()],
                ));
            }
            return Ok(astnav::get_touching_property_name(
                source_file,
                source_file_get_position_map(source_file).utf16_to_utf8(position as i32),
            ));
        }
        Ok(Node::NIL)
    }
}

/// PORT: Go returns a typed handler result as `any`. A typed nil pointer or
/// slice is still a non-nil `any` (it marshals as `null` or `[]`), so every
/// typed result is `Some`.
pub fn to_any<T: AnyValue>(v: T) -> Option<Box<dyn AnyValue>> {
    Some(Box::new(v))
}

/// PORT: Go `parsed.(*T)`. `unmarshallerFor[T]` always returns a `*T`, so the
/// assertion holds; a failed one panics as in Go.
fn assert_params<T: 'static>(parsed: &Option<Box<dyn AnyValue>>) -> &T {
    match parsed.as_deref().and_then(|p| p.downcast_ref::<T>()) {
        Some(p) => p,
        None => panic!(
            "interface conversion: interface {{}} is not *{}",
            std::any::type_name::<T>()
        ),
    }
}

impl Session {
    // Go: api/session.go:513 ID
    // ID returns the unique identifier for this session.
    pub fn id(&self) -> String {
        self.id.clone()
    }

    // Go: api/session.go SetConnection (ts#64299)
    pub fn set_connection(&self, conn: Rc<dyn ipc::Conn>) {
        *self.conn.borrow_mut() = Some(conn);
    }

    // Go: api/session.go GetCurrentDirectory (ts#64163)
    pub fn get_current_directory(&self) -> String {
        self.snapshot_host.get_current_directory()
    }

    // Go: api/session.go FS (ts#64163)
    pub fn fs(&self) -> Rc<dyn vfs::Fs> {
        if let Some(project_session) = &self.project_session {
            return project_session.fs();
        }
        self.snapshot_host.fs()
    }

    // Go: api/session.go DefaultLibraryPath (ts#64158)
    pub fn default_library_path(&self) -> String {
        if let Some(project_session) = &self.project_session {
            return project_session.default_library_path();
        }
        self.snapshot_host.default_library_path()
    }

    // Go: api/session.go useCaseSensitiveFileNames (ts#64163)
    pub fn use_case_sensitive_file_names(&self) -> bool {
        self.snapshot_host.fs().use_case_sensitive_file_names()
    }

    // Go: api/session.go:549 getSnapshotData
    // getSnapshotData looks up snapshot data by handle.
    pub fn get_snapshot_data(&self, handle: SnapshotID) -> Result<Rc<SnapshotData>, GoError> {
        let sd = self.snapshots.borrow().get(&handle).cloned();
        let Some(sd) = sd else {
            return Err(errors::errorf(
                format!("{}: snapshot {} not found", *ERR_CLIENT_ERROR, handle.0),
                vec![ERR_CLIENT_ERROR.clone()],
            ));
        };
        Ok(sd)
    }

    // Go: api/session.go:560 retainSnapshotData (tsgo#4642)
    // retainSnapshotData pins snapshot data while an operation builds a derived snapshot.
    pub fn retain_snapshot_data(&self, handle: SnapshotID) -> Result<Rc<SnapshotData>, GoError> {
        let sd = self.snapshots.borrow().get(&handle).cloned();
        let Some(sd) = sd else {
            return Err(errors::errorf(
                format!("{}: snapshot {} not found", *ERR_CLIENT_ERROR, handle.0),
                vec![ERR_CLIENT_ERROR.clone()],
            ));
        };
        sd.ref_count.set(sd.ref_count.get() + 1);
        Ok(sd)
    }

    // Go: api/session.go:571 releaseSnapshot (tsgo#4642)
    pub fn release_snapshot(&self, handle: SnapshotID) -> Result<(), GoError> {
        let sd = self.snapshots.borrow().get(&handle).cloned();
        let Some(sd) = sd else {
            return Err(errors::errorf(
                format!("{}: snapshot {} not found", *ERR_CLIENT_ERROR, handle.0),
                vec![ERR_CLIENT_ERROR.clone()],
            ));
        };
        sd.ref_count.set(sd.ref_count.get() - 1);
        if sd.ref_count.get() <= 0 {
            self.snapshots.borrow_mut().remove(&handle);
            // ts#64163: the snapshot derefs without the project session.
            project::Snapshot::deref(&sd.snapshot);
        }
        Ok(())
    }

    // Go: api/session.go:653 setupChecker
    // setupChecker resolves snapshot, program, and type checker for a project.
    // Callers must defer setup.done() to release the checker.
    pub fn setup_checker(
        &self,
        ctx: &Context,
        snapshot: SnapshotID,
        project_handle: &project::ID,
    ) -> Result<CheckerSetup, GoError> {
        let sd = self.get_snapshot_data(snapshot)?;

        let program = sd.get_program(project_handle)?;

        let (c, done) = ls_program::get_type_checker(
            &program,
            &core_context::with_checker_lifetime(ctx, CheckerLifetime::API),
        );
        Ok(CheckerSetup {
            sd,
            snapshot,
            program,
            checker: c,
            done,
            project_id: project_handle.clone(),
        })
    }

    // Go: api/session.go:685 setupLanguageService
    // setupLanguageService creates a LanguageService for the given snapshot/project.
    // Unlike setupChecker, this does NOT acquire a checker from the pool, so callers that
    // only need an LS (and not a Checker) can avoid blocking on / holding a pooled checker.
    //
    // The LS acquires its own checker internally (keyed by the ctx's checker lifetime).
    // If a handler returns symbol/type/signature handles the client may later re-query
    // on the API checker (e.g. completion with IncludeSymbol -> GetTypeOfSymbol), wrap
    // ctx with core.WithCheckerLifetime(ctx, core.CheckerLifetimeAPI) so those handles
    // are produced on the persistent API checker and stay resolvable. Only safe when the
    // LS operation acquires a checker exactly once; nested acquisitions (e.g. find-all-
    // references) would deadlock on the single-slot persistent checker.
    // ts#64133: takes the snapshot, not the snapshot data.
    pub fn setup_language_service(
        &self,
        snapshot: &Rc<project::Snapshot>,
        program: Rc<compiler::NewProgram>,
        project_handle: &project::ID,
        active_file: &str,
    ) -> Result<ls::LanguageService, GoError> {
        // ts#64319: looked up by project ID.
        let proj = snapshot.project_collection.get_project(project_handle);
        let Some(proj) = proj else {
            return Err(errors::errorf(
                format!(
                    "{}: project {} not found",
                    *ERR_CLIENT_ERROR, project_handle.0
                ),
                vec![ERR_CLIENT_ERROR.clone()],
            ));
        };
        // ts#64319: Go passes `proj.ID()` as the `autoimport.ProjectID`
        // interface value; the Rust type is `autoimport::ProjectID`.
        let project_id = autoimport::ProjectID(proj.borrow().id().0);
        let host: Rc<dyn ls::Host> = snapshot.clone();
        Ok(ls::new_language_service(
            project_id,
            program,
            host,
            active_file,
        ))
    }
}

impl ipc::Handler for Session {
    // Go: api/session.go:694 HandleRequest
    // HandleRequest implements Handler.
    fn handle_request(
        &self,
        ctx: &Context,
        method: &str,
        params: JsonValue,
    ) -> Result<Option<Box<dyn AnyValue>>, GoError> {
        // ts#64163
        let ctx = &(self.with_locale)(ctx);
        // Handle simple methods that don't need param parsing
        match method {
            "echo" => {
                // Return raw binary for msgpack protocol compatibility
                if self.use_binary_responses {
                    return Ok(to_any(RawBinary(params.0)));
                }
                return Ok(to_any(params));
            }
            "ping" => {
                return Ok(to_any("pong".to_string()));
            }
            _ => {}
        }

        let parsed = match unmarshal_payload(method, &params) {
            Ok(parsed) => parsed,
            Err(err) => {
                return Err(errors::errorf(
                    format!("{}: {}", *ERR_INVALID_REQUEST, err),
                    vec![ERR_INVALID_REQUEST.clone(), err],
                ));
            }
        };

        match method {
            // ts#63937
            m if m == Method::BATCH_REQUESTS.0 => self
                .handle_batch_requests(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::RELEASE.0 => self.handle_release(ctx, Some(assert_params(&parsed))),
            // ts#64434
            m if m == Method::RELEASE_SOURCE_FILE.0 => {
                self.handle_release_source_file(Some(assert_params(&parsed)))
            }
            // ts#64518
            m if m == Method::RETAIN_SOURCE_FILE.0 => self
                .handle_retain_source_file(assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_CACHED_SOURCE_FILE.0 => {
                self.handle_get_cached_source_file(assert_params(&parsed))
            }
            m if m == Method::INITIALIZE.0 => self.handle_initialize(ctx).map(to_any),
            // ts#64204
            m if m == Method::CREATE_SNAPSHOT.0 => self
                .handle_create_snapshot(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::UPDATE_SNAPSHOT.0 => self
                .handle_update_snapshot(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_CURRENT_LANGUAGE_SERVER_SNAPSHOT.0 => self
                .handle_get_current_language_server_snapshot(ctx, assert_params(&parsed))
                .map(to_any),
            // ts#64299
            m if m == Method::CREATE_MODULE_RESOLVER.0 => self
                .handle_create_module_resolver(assert_params(&parsed))
                .map(to_any),
            m if m == Method::RELEASE_MODULE_RESOLVER.0 => {
                self.handle_release_module_resolver(assert_params(&parsed))
            }
            m if m == Method::RESOLVE_MODULE_NAME.0 => self
                .handle_resolve_module_name(ctx, assert_params(&parsed))
                .map(to_any),
            // ts#64158
            m if m == Method::CREATE_BUILD_ORCHESTRATOR.0 => self
                .handle_create_build_orchestrator(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::DISPOSE_BUILD_ORCHESTRATOR.0 => {
                self.handle_dispose_build_orchestrator(ctx, assert_params(&parsed))
            }
            m if m == Method::BUILD.0 => self.handle_build(ctx, assert_params(&parsed)).map(to_any),
            m if m == Method::BUILD_REFERENCES.0 => self
                .handle_build_references(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::CLEAN_BUILD.0 => self
                .handle_clean_build(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::CLEAN_REFERENCES.0 => self
                .handle_clean_references(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::PARSE_COMMAND_LINE.0 => self
                .handle_parse_command_line(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::READ_CONFIG_FILE.0 => self
                .handle_read_config_file(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::PARSE_JSON_CONFIG_FILE.0 => self
                .handle_parse_json_config_file_content(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::PARSE_CONFIG_FILE.0 => self
                .handle_parse_config_file(ctx, assert_params(&parsed))
                .map(to_any),
            // ts#64216
            m if m == Method::CREATE_SOURCE_FILE.0 => {
                self.handle_create_source_file(ctx, assert_params(&parsed))
            }
            m if m == Method::CREATE_SOURCE_FILE_FROM_FILE.0 => {
                self.handle_create_source_file_from_file(ctx, assert_params(&parsed))
            }
            // tsgo#4849
            m if m == Method::TRANSPILE_MODULE.0 => self
                .handle_transpile(ctx, assert_params(&parsed), false)
                .map(to_any),
            m if m == Method::TRANSPILE_MODULE_FROM_FILE.0 => self
                .handle_transpile_from_file(ctx, assert_params(&parsed), false)
                .map(to_any),
            m if m == Method::TRANSPILE_DECLARATION.0 => self
                .handle_transpile(ctx, assert_params(&parsed), true)
                .map(to_any),
            m if m == Method::TRANSPILE_DECLARATION_FROM_FILE.0 => self
                .handle_transpile_from_file(ctx, assert_params(&parsed), true)
                .map(to_any),
            m if m == Method::GET_DEFAULT_PROJECT_FOR_FILE.0 => self
                .handle_get_default_project_for_file(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_SOURCE_FILE.0 => {
                self.handle_get_source_file(ctx, assert_params(&parsed))
            }
            m if m == Method::GET_SOURCE_FILE_NAMES.0 => self
                .handle_get_source_file_names(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_SOURCE_FILE_METADATA.0 => self
                .handle_get_source_file_metadata(ctx, assert_params(&parsed))
                .map(to_any),
            // ts#64292
            m if m == Method::GET_MODE_FOR_USAGE_LOCATION.0 => self
                .handle_get_mode_for_usage_location(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_MODE_FOR_RESOLUTION_AT_INDEX.0 => self
                .handle_get_mode_for_resolution_at_index(ctx, assert_params(&parsed))
                .map(to_any),
            // ts#64247
            m if m == Method::GET_RESOLVED_MODULE.0 => self
                .handle_get_resolved_module(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_RESOLVED_MODULE_FROM_MODULE_SPECIFIER.0 => self
                .handle_get_resolved_module_from_module_specifier(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_RESOLVED_TYPE_REFERENCE_DIRECTIVE.0 => self
                .handle_get_resolved_type_reference_directive(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_RESOLVED_TYPE_REFERENCE_DIRECTIVE_FROM_REFERENCE.0 => self
                .handle_get_resolved_type_reference_directive_from_reference(
                    ctx,
                    assert_params(&parsed),
                )
                .map(to_any),
            m if m == Method::GET_CONFIG_FILE_NAMES.0 => self
                .handle_get_config_file_names(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_CONFIG_SOURCE_FILE.0 => {
                self.handle_get_config_source_file(ctx, assert_params(&parsed))
            }
            m if m == Method::GET_SYMBOL_AT_POSITION.0 => self
                .handle_get_symbol_at_position(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_SYMBOLS_AT_POSITIONS.0 => self
                .handle_get_symbols_at_positions(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_SYMBOL_AT_LOCATION.0 => self
                .handle_get_symbol_at_location(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_SYMBOLS_AT_LOCATIONS.0 => self
                .handle_get_symbols_at_locations(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_SYMBOL_OF_SOURCE_FILE.0 => self
                .handle_get_symbol_of_source_file(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_SYMBOLS_OF_SOURCE_FILES.0 => self
                .handle_get_symbols_of_source_files(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_TYPE_OF_SYMBOL.0 => self
                .handle_get_type_of_symbol(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_TYPES_OF_SYMBOLS.0 => self
                .handle_get_types_of_symbols(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_DECLARED_TYPE_OF_SYMBOL.0 => self
                .handle_get_declared_type_of_symbol(ctx, assert_params(&parsed))
                .map(to_any),
            // ts#63956
            m if m == Method::GET_NON_MISSING_TYPE_OF_SYMBOL.0 => self
                .handle_get_non_missing_type_of_symbol(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::RESOLVE_NAME.0 => self
                .handle_resolve_name(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_SYMBOLS_IN_SCOPE.0 => self
                .handle_get_symbols_in_scope(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_SIGNATURES_OF_TYPE.0 => self
                .handle_get_signatures_of_type(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_RESOLVED_SIGNATURE.0 => self
                .handle_get_resolved_signature(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_TYPE_AT_LOCATION.0 => self
                .handle_get_type_at_location(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_TYPE_AT_LOCATIONS.0 => self
                .handle_get_type_at_locations(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_TYPE_AT_POSITION.0 => self
                .handle_get_type_at_position(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_TYPES_AT_POSITIONS.0 => self
                .handle_get_types_at_positions(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_PARENT_OF_SYMBOL.0 => self
                .handle_get_parent_of_symbol(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_MEMBERS_OF_SYMBOL.0 => self
                .handle_get_members_of_symbol(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_EXPORTS_OF_SYMBOL.0 => self
                .handle_get_exports_of_symbol(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_EXPORT_SYMBOL_OF_SYMBOL.0 => self
                .handle_get_export_symbol_of_symbol(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_SYMBOL_OF_TYPE.0 => self
                .handle_get_symbol_of_type(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_TARGET_OF_TYPE.0 => self
                .handle_get_target_of_type(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_FRESH_TYPE_OF_TYPE.0 => self
                .handle_get_fresh_type_of_type(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_REGULAR_TYPE_OF_TYPE.0 => self
                .handle_get_regular_type_of_type(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_TYPES_OF_TYPE.0 => self
                .handle_get_types_of_type(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_TYPE_PARAMETERS_OF_TYPE.0 => self
                .handle_get_type_parameters_of_type(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_OUTER_TYPE_PARAMETERS_OF_TYPE.0 => self
                .handle_get_outer_type_parameters_of_type(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_LOCAL_TYPE_PARAMETERS_OF_TYPE.0 => self
                .handle_get_local_type_parameters_of_type(ctx, assert_params(&parsed))
                .map(to_any),
            // ts#64264
            m if m == Method::GET_THIS_TYPE_OF_TYPE.0 => self
                .handle_get_this_type_of_type(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_ALIAS_TYPE_ARGUMENTS_OF_TYPE.0 => self
                .handle_get_alias_type_arguments_of_type(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_ALIAS_SYMBOL_OF_TYPE.0 => self
                .handle_get_alias_symbol_of_type(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_OBJECT_TYPE_OF_TYPE.0 => self
                .handle_get_object_type_of_type(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_INDEX_TYPE_OF_TYPE.0 => self
                .handle_get_index_type_of_type(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_CHECK_TYPE_OF_TYPE.0 => self
                .handle_get_check_type_of_type(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_EXTENDS_TYPE_OF_TYPE.0 => self
                .handle_get_extends_type_of_type(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_BASE_TYPE_OF_TYPE.0 => self
                .handle_get_base_type_of_type(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_CONSTRAINT_OF_TYPE.0 => self
                .handle_get_constraint_of_type(ctx, assert_params(&parsed))
                .map(to_any),
            // ts#64397
            m if m == Method::GET_TYPE_PARAMETER_OF_MAPPED_TYPE.0 => self
                .handle_get_type_parameter_of_mapped_type(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_CONSTRAINT_TYPE_OF_MAPPED_TYPE.0 => self
                .handle_get_constraint_type_of_mapped_type(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_NAME_TYPE_OF_MAPPED_TYPE.0 => self
                .handle_get_name_type_of_mapped_type(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_TEMPLATE_TYPE_OF_MAPPED_TYPE.0 => self
                .handle_get_template_type_of_mapped_type(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_TRUE_TYPE_OF_CONDITIONAL_TYPE.0 => self
                .handle_get_true_type_of_conditional_type(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_FALSE_TYPE_OF_CONDITIONAL_TYPE.0 => self
                .handle_get_false_type_of_conditional_type(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_TYPE_PARAMETERS_OF_SIGNATURE.0 => self
                .handle_get_type_parameters_of_signature(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_PARAMETERS_OF_SIGNATURE.0 => self
                .handle_get_parameters_of_signature(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_THIS_PARAMETER_OF_SIGNATURE.0 => self
                .handle_get_this_parameter_of_signature(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_TARGET_OF_SIGNATURE.0 => self
                .handle_get_target_of_signature(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_CONTEXTUAL_TYPE.0 => self
                .handle_get_contextual_type(ctx, assert_params(&parsed))
                .map(to_any),
            // ts#64264
            m if m == Method::GET_CONTEXTUAL_TYPE_FOR_ARGUMENT.0 => self
                .handle_get_contextual_type_for_argument(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_AWAITED_TYPE.0 => self
                .handle_get_awaited_type(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_BASE_TYPE_OF_LITERAL_TYPE.0 => self
                .handle_get_base_type_of_literal_type(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_NON_NULLABLE_TYPE.0 => self
                .handle_get_non_nullable_type(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_TYPE_FROM_TYPE_NODE.0 => self
                .handle_get_type_from_type_node(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_WIDENED_TYPE.0 => self
                .handle_get_widened_type(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_PARAMETER_TYPE.0 => self
                .handle_get_parameter_type(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_TYPE_PARAMETER_AT_POSITION.0 => self
                .handle_get_type_parameter_at_position(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::IS_ARRAY_LIKE_TYPE.0 => self
                .handle_is_array_like_type(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::IS_TYPE_ASSIGNABLE_TO.0 => self
                .handle_is_type_assignable_to(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_SHORTHAND_ASSIGNMENT_VALUE_SYMBOL.0 => self
                .handle_get_shorthand_assignment_value_symbol(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_TYPE_OF_SYMBOL_AT_LOCATION.0 => self
                .handle_get_type_of_symbol_at_location(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::TYPE_TO_TYPE_NODE.0 => {
                self.handle_type_to_type_node(ctx, assert_params(&parsed))
            }
            m if m == Method::SIGNATURE_TO_SIGNATURE_DECLARATION.0 => {
                self.handle_signature_to_signature_declaration(ctx, assert_params(&parsed))
            }
            m if m == Method::TYPE_TO_STRING.0 => {
                self.handle_type_to_string(ctx, assert_params(&parsed))
            }
            m if m == Method::PRINT_NODE.0 => self
                .handle_print_node(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::FORMAT_NODE_FOR_INSERTION.0 => self
                .handle_format_node_for_insertion(ctx, assert_params(&parsed))
                .map(to_any),
            // tsgo#4699
            m if m == Method::EMIT.0 => self.handle_emit(ctx, assert_params(&parsed)).map(to_any),
            m if m == Method::EMIT_TO_STRING.0 => self
                .handle_emit_to_string(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_JAVA_SCRIPT_EMIT.0 => self
                .handle_selected_files_emit(ctx, assert_params(&parsed), EmitOnly::Js)
                .map(to_any),
            m if m == Method::GET_DECLARATION_EMIT.0 => self
                .handle_selected_files_emit(ctx, assert_params(&parsed), EmitOnly::Dts)
                .map(to_any),
            m if m == Method::IS_CONTEXT_SENSITIVE.0 => self
                .handle_is_context_sensitive(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_RETURN_TYPE_OF_SIGNATURE.0 => self
                .handle_get_return_type_of_signature(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_REST_TYPE_OF_SIGNATURE.0 => self
                .handle_get_rest_type_of_signature(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_TYPE_PREDICATE_OF_SIGNATURE.0 => self
                .handle_get_type_predicate_of_signature(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_BASE_TYPES.0 => self
                .handle_get_base_types(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_PROPERTIES_OF_TYPE.0 => self
                .handle_get_properties_of_type(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_APPARENT_PROPERTIES_OF_TYPE.0 => self
                .handle_get_apparent_properties_of_type(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_APPARENT_TYPE.0 => self
                .handle_get_apparent_type(ctx, assert_params(&parsed))
                .map(to_any),
            // ts#63899
            m if m == Method::GET_REDUCED_TYPE.0 => self
                .handle_get_reduced_type(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_PROPERTY_OF_TYPE.0 => self
                .handle_get_property_of_type(ctx, assert_params(&parsed))
                .map(to_any),
            // ts#64264
            m if m == Method::GET_TYPE_OF_PROPERTY_OF_TYPE.0 => self
                .handle_get_type_of_property_of_type(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_INDEX_INFO_OF_TYPE.0 => self
                .handle_get_index_info_of_type(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_INDEX_INFOS_OF_TYPE.0 => self
                .handle_get_index_infos_of_type(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_CONSTRAINT_OF_TYPE_PARAMETER.0 => self
                .handle_get_constraint_of_type_parameter(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_BASE_CONSTRAINT_OF_TYPE.0 => self
                .handle_get_base_constraint_of_type(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_DEFAULT_FROM_TYPE_PARAMETER.0 => self
                .handle_get_default_from_type_parameter(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_TYPE_ARGUMENTS.0 => self
                .handle_get_type_arguments(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_IMPORT_ADDER_EDITS.0 => self
                .handle_get_import_adder_edits(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_CONSTANT_VALUE.0 => self
                .handle_get_constant_value(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_SIGNATURE_FROM_DECLARATION.0 => self
                .handle_get_signature_from_declaration(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_EXPORT_SPECIFIER_LOCAL_TARGET.0 => self
                .handle_get_export_specifier_local_target_symbol(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_ALIASED_SYMBOL.0 => self
                .handle_get_aliased_symbol(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_IMMEDIATE_ALIASED_SYMBOL.0 => self
                .handle_get_immediate_aliased_symbol(ctx, assert_params(&parsed))
                .map(to_any),
            // ts#63945
            m if m == Method::GET_TARGET_SYMBOL.0 => self
                .handle_method_get_target_symbol(ctx, assert_params(&parsed))
                .map(to_any),
            // ts#64264
            m if m == Method::GET_EXPORT_SYMBOL_OF_SYMBOL_FOR_CHECKER.0 => self
                .handle_get_export_symbol_of_symbol_for_checker(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_FULLY_QUALIFIED_NAME.0 => self
                .handle_get_fully_qualified_name(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_EXPORTS_OF_MODULE.0 => self
                .handle_get_exports_of_module(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_MEMBER_IN_MODULE_EXPORTS.0 => self
                .handle_get_member_in_module_exports(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_JS_DOC_TAGS.0 => self
                .handle_get_js_doc_tags(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_DOCUMENTATION_COMMENT.0 => self
                .handle_get_documentation_comment(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::IS_ARRAY_TYPE.0 => self
                .handle_is_array_type(ctx, assert_params(&parsed))
                .map(to_any),
            // ts#63943
            m if m == Method::IS_READONLY_SYMBOL.0 => self
                .handle_is_readonly_symbol(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_ANY_TYPE.0 => self
                .handle_get_intrinsic_type(ctx, assert_params(&parsed), Checker::get_any_type)
                .map(to_any),
            m if m == Method::GET_STRING_TYPE.0 => self
                .handle_get_intrinsic_type(ctx, assert_params(&parsed), Checker::get_string_type)
                .map(to_any),
            m if m == Method::GET_NUMBER_TYPE.0 => self
                .handle_get_intrinsic_type(ctx, assert_params(&parsed), Checker::get_number_type)
                .map(to_any),
            m if m == Method::GET_BOOLEAN_TYPE.0 => self
                .handle_get_intrinsic_type(ctx, assert_params(&parsed), Checker::get_boolean_type)
                .map(to_any),
            m if m == Method::GET_VOID_TYPE.0 => self
                .handle_get_intrinsic_type(ctx, assert_params(&parsed), Checker::get_void_type)
                .map(to_any),
            m if m == Method::GET_UNDEFINED_TYPE.0 => self
                .handle_get_intrinsic_type(ctx, assert_params(&parsed), Checker::get_undefined_type)
                .map(to_any),
            m if m == Method::GET_NULL_TYPE.0 => self
                .handle_get_intrinsic_type(ctx, assert_params(&parsed), Checker::get_null_type)
                .map(to_any),
            m if m == Method::GET_NEVER_TYPE.0 => self
                .handle_get_intrinsic_type(ctx, assert_params(&parsed), Checker::get_never_type)
                .map(to_any),
            m if m == Method::GET_UNKNOWN_TYPE.0 => self
                .handle_get_intrinsic_type(ctx, assert_params(&parsed), Checker::get_unknown_type)
                .map(to_any),
            m if m == Method::GET_BIG_INT_TYPE.0 => self
                .handle_get_intrinsic_type(ctx, assert_params(&parsed), Checker::get_big_int_type)
                .map(to_any),
            m if m == Method::GET_ES_SYMBOL_TYPE.0 => self
                .handle_get_intrinsic_type(ctx, assert_params(&parsed), Checker::get_es_symbol_type)
                .map(to_any),
            m if m == Method::GET_NON_PRIMITIVE_TYPE.0 => self
                .handle_get_intrinsic_type(
                    ctx,
                    assert_params(&parsed),
                    Checker::get_non_primitive_type,
                )
                .map(to_any),
            m if m == Method::GET_WELL_KNOWN_SYMBOLS.0 => self
                .handle_get_well_known_symbols(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_WELL_KNOWN_SIGNATURES.0 => self
                .handle_get_well_known_signatures(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_SYNTACTIC_DIAGNOSTICS.0 => self
                .handle_get_syntactic_diagnostics(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_BIND_DIAGNOSTICS.0 => self
                .handle_get_bind_diagnostics(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_SEMANTIC_DIAGNOSTICS.0 => self
                .handle_get_semantic_diagnostics(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_SUGGESTION_DIAGNOSTICS.0 => self
                .handle_get_suggestion_diagnostics(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_DECLARATION_DIAGNOSTICS.0 => self
                .handle_get_declaration_diagnostics(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_PROGRAM_DIAGNOSTICS.0 => self
                .handle_get_program_diagnostics(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_GLOBAL_DIAGNOSTICS.0 => self
                .handle_get_global_diagnostics(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_CONFIG_FILE_PARSING_DIAGNOSTICS.0 => self
                .handle_get_config_file_parsing_diagnostics(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::START_CPU_PROFILE.0 => {
                self.handle_start_cpu_profile(ctx, Some(assert_params(&parsed)))
            }
            m if m == Method::STOP_CPU_PROFILE.0 => self.handle_stop_cpu_profile(ctx).map(to_any),
            m if m == Method::SAVE_HEAP_PROFILE.0 => self
                .handle_save_heap_profile(ctx, Some(assert_params(&parsed)))
                .map(to_any),
            m if m == Method::GET_REFERENCES_TO_SYMBOL_IN_FILE.0 => self
                .handle_get_references_to_symbol_in_file(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_REFERENCED_SYMBOLS_FOR_NODE.0 => self
                .handle_get_referenced_symbols_for_node(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_SIGNATURE_USAGES.0 => self
                .handle_get_signature_usages(ctx, assert_params(&parsed))
                .map(to_any),
            m if m == Method::GET_COMPLETIONS_AT_POSITION.0 => self
                .handle_get_completions_at_position(ctx, assert_params(&parsed))
                .map(to_any),
            _ => Err(errors::errorf(format!("unknown method: {method}"), vec![])),
        }
    }

    // Go: api/session.go:1213 HandleNotification
    // HandleNotification implements Handler.
    fn handle_notification(
        &self,
        _ctx: &Context,
        _method: &str,
        _params: JsonValue,
    ) -> Result<(), GoError> {
        // TODO: Implement notification handling
        Ok(())
    }
}

impl Session {
    // Go: api/session.go handleBatchRequests (ts#63937)
    // PORT: Go returns the unpaginated response for a nil session (a test
    // path); a Rust session is never nil.
    pub fn handle_batch_requests(
        &self,
        ctx: &Context,
        params: &BatchRequestsParams,
    ) -> Result<BatchRequestsResponse, GoError> {
        if !params.continuation_token.is_empty() {
            let page = self
                .batch_response_pages
                .borrow_mut()
                .remove(&params.continuation_token);
            let Some(page) = page else {
                return Err(errors::errorf(
                    format!("{}: invalid batch continuation token", *ERR_CLIENT_ERROR),
                    vec![ERR_CLIENT_ERROR.clone()],
                ));
            };
            return self.paginate_batch_responses(page, None, params.max_response_bytes_per_page);
        }

        let mut responses = Vec::with_capacity(params.requests.len());
        for request in &params.requests {
            responses.push(self.handle_batch_request(ctx, request));
        }
        let page = new_batch_response_page(&responses)?;
        self.paginate_batch_responses(page, Some(responses), params.max_response_bytes_per_page)
    }

    // Go: api/session.go paginateBatchResponses (ts#64061)
    // PORT: Go `responses` nil is `None`.
    pub fn paginate_batch_responses(
        &self,
        page: BatchResponsePage,
        responses: Option<Vec<BatchResponse>>,
        mut max_response_bytes_per_page: i32,
    ) -> Result<BatchRequestsResponse, GoError> {
        if max_response_bytes_per_page <= 0 {
            max_response_bytes_per_page = DEFAULT_MAX_RESPONSE_BYTES_PER_PAGE;
        }
        let max_response_bytes_per_page = max_response_bytes_per_page as usize;
        let mut encoded_length = r#"{"responses":[]}"#.len();
        let mut page_length = 0usize;
        for encoded in &page.encoded_responses {
            let mut additional_length = encoded.0.len();
            if page_length > 0 {
                additional_length += 1;
            }
            if page_length > 0 && encoded_length + additional_length > max_response_bytes_per_page {
                break;
            }
            encoded_length += additional_length;
            page_length += 1;
        }

        if page_length == page.encoded_responses.len() {
            return Ok(BatchRequestsResponse {
                responses: responses.unwrap_or_default(),
                continuation_token: String::new(),
                encoded_responses: Some(page.encoded_responses),
            });
        }
        self.next_batch_response_page_id
            .set(self.next_batch_response_page_id.get() + 1);
        let continuation_token = format!("{}-{}", self.id, self.next_batch_response_page_id.get());
        let continuation_length = r#","continuationToken":"""#.len() + continuation_token.len();
        while page_length > 1 && encoded_length + continuation_length > max_response_bytes_per_page
        {
            encoded_length -= page.encoded_responses[page_length - 1].0.len() + 1;
            page_length -= 1;
        }
        let mut current_responses = page.encoded_responses;
        let remaining_responses = current_responses.split_off(page_length);
        let mut response = BatchRequestsResponse {
            responses: Vec::new(),
            continuation_token: continuation_token.clone(),
            encoded_responses: Some(current_responses),
        };
        if let Some(mut responses) = responses {
            responses.truncate(page_length);
            response.responses = responses;
        }
        self.batch_response_pages.borrow_mut().insert(
            continuation_token,
            BatchResponsePage {
                encoded_responses: remaining_responses,
            },
        );
        Ok(response)
    }

    // Go: api/session.go handleBatchRequest (ts#63937)
    // PORT: Go recovers a panic in a deferred function; `catch_unwind` covers
    // the same call. Go `debug.Stack()` is the backtrace at the recover point.
    pub fn handle_batch_request(&self, ctx: &Context, request: &BatchRequest) -> BatchResponse {
        let mut response = BatchResponse {
            method: request.method.clone(),
            ..Default::default()
        };
        // ts#64061
        if request.method == Method::BATCH_REQUESTS {
            response.error = format!("{}: batchRequests cannot be nested", *ERR_INVALID_REQUEST);
            return response;
        }
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            ipc::Handler::handle_request(self, ctx, &request.method.0, request.params.clone())
        }));
        match outcome {
            Ok(Ok(result)) => response.result = result,
            Ok(Err(err)) => response.error = err.error(),
            Err(recovered) => {
                response.result = None;
                response.error = format!(
                    "panic: {}\n{}",
                    ipc::conn::recovered_value(recovered.as_ref()),
                    std::backtrace::Backtrace::force_capture()
                );
            }
        }
        // ts#64216
        // PORT: Go `RawBinary(nil)` is an empty `RawBinary` (an encoded file
        // is never empty).
        if is_source_file_response_method(&request.method)
            && let Some(data) = response
                .result
                .as_deref()
                .and_then(|result| result.downcast_ref::<RawBinary>())
        {
            if data.0.is_empty() {
                response.result = None;
            } else {
                response.result = to_any(SourceFileResponse {
                    data: base64_std_encoding_encode_to_string(&data.0),
                });
            }
        }
        response
    }
}

// Go: api/session.go isSourceFileResponseMethod (ts#64216)
pub fn is_source_file_response_method(method: &Method) -> bool {
    *method == Method::CREATE_SOURCE_FILE
        || *method == Method::CREATE_SOURCE_FILE_FROM_FILE
        || *method == Method::GET_SOURCE_FILE
        // ts#64518
        || *method == Method::GET_CACHED_SOURCE_FILE
        || *method == Method::GET_CONFIG_SOURCE_FILE
        || *method == Method::TYPE_TO_TYPE_NODE
        || *method == Method::SIGNATURE_TO_SIGNATURE_DECLARATION
}

// Go: api/session.go newBatchResponsePage (ts#64061)
pub fn new_batch_response_page(responses: &[BatchResponse]) -> Result<BatchResponsePage, GoError> {
    let mut encoded_responses = Vec::with_capacity(responses.len());
    for response in responses {
        let encoded = match crate::frontend::json::json_marshal(response, &[]) {
            Ok(encoded) => encoded,
            Err(err) => return Err(errors::from_value(err)),
        };
        // PORT: a `json.Value` holds Go bytes; the text is in the port form.
        encoded_responses.push(JsonValue(
            crate::scanner_util::go_string_bytes(&encoded).into_owned(),
        ));
    }
    Ok(BatchResponsePage { encoded_responses })
}

impl Session {
    // Go: api/session.go:1183 handleStartCPUProfile
    pub fn handle_start_cpu_profile(
        &self,
        _ctx: &Context,
        params: Option<&ProfileParams>,
    ) -> Result<Option<Box<dyn AnyValue>>, GoError> {
        let Some(params) = params.filter(|params| !params.dir.is_empty()) else {
            return Err(errors::errorf(
                format!("{}: dir is required", *ERR_CLIENT_ERROR),
                vec![ERR_CLIENT_ERROR.clone()],
            ));
        };
        if let Err(err) = self.cpu_profiler.start_cpu_profile(&params.dir) {
            return Err(errors::errorf(
                format!(
                    "{}: failed to start CPU profile: {}",
                    *ERR_CLIENT_ERROR, err
                ),
                vec![ERR_CLIENT_ERROR.clone(), err],
            ));
        }
        Ok(None)
    }

    // Go: api/session.go:1193 handleStopCPUProfile
    pub fn handle_stop_cpu_profile(&self, _ctx: &Context) -> Result<ProfileResult, GoError> {
        match self.cpu_profiler.stop_cpu_profile() {
            Ok(file_path) => Ok(ProfileResult { file: file_path }),
            Err(err) => Err(errors::errorf(
                format!("{}: failed to stop CPU profile: {}", *ERR_CLIENT_ERROR, err),
                vec![ERR_CLIENT_ERROR.clone(), err],
            )),
        }
    }

    // Go: api/session.go:1201 handleSaveHeapProfile
    pub fn handle_save_heap_profile(
        &self,
        _ctx: &Context,
        params: Option<&ProfileParams>,
    ) -> Result<ProfileResult, GoError> {
        let Some(params) = params.filter(|params| !params.dir.is_empty()) else {
            return Err(errors::errorf(
                format!("{}: dir is required", *ERR_CLIENT_ERROR),
                vec![ERR_CLIENT_ERROR.clone()],
            ));
        };
        match crate::pprof::save_heap_profile(&params.dir) {
            Ok(file_path) => Ok(ProfileResult { file: file_path }),
            Err(err) => Err(errors::errorf(
                format!(
                    "{}: failed to save heap profile: {}",
                    *ERR_CLIENT_ERROR, err
                ),
                vec![ERR_CLIENT_ERROR.clone(), err],
            )),
        }
    }

    // Go: api/session.go:1218 handleInitialize
    pub fn handle_initialize(&self, _ctx: &Context) -> Result<InitializeResponse, GoError> {
        Ok(InitializeResponse {
            use_case_sensitive_file_names: self.use_case_sensitive_file_names(),
            current_directory: self.get_current_directory(),
        })
    }

    // Go: api/session.go handleCreateSnapshot (ts#64204)
    // handleCreateSnapshot creates a new independent snapshot.
    pub fn handle_create_snapshot(
        &self,
        ctx: &Context,
        params: &CreateSnapshotParams,
    ) -> Result<CreateSnapshotResponse, GoError> {
        let mut api_request =
            self.to_api_snapshot_request(ctx, &params.snapshot_request_changes_params)?;
        // ts#64554 (Go N' api/session.go:1422). PORT: `APISnapshotRequest`
        // has no fields for these yet (`clone_api_snapshot`).
        let user_preferences = params.user_preferences.as_ref();
        let prepare_auto_imports = params
            .prepare_auto_imports
            .as_ref()
            .map(|file| file.to_uri(&self.get_current_directory()));

        let open_state =
            self.reconcile_snapshot_opens(&mut api_request, SnapshotOpenState::default());
        let mut file_changes = self.to_file_change_summary(params.file_notifications.as_ref());
        let mut snapshot_file_system: Option<Rc<dyn vfs::Fs>> = None;
        if let Some(request_file_system) = &params.file_system {
            let file_system = match requestfilesystem::new_for_update(
                Some(request_file_system),
                self.fs(),
                &self.get_current_directory(),
                &mut file_changes,
            ) {
                Ok(file_system) => file_system,
                Err(file_system_err) => {
                    return Err(errors::errorf(
                        format!("{}: {}", *ERR_CLIENT_ERROR, file_system_err),
                        vec![ERR_CLIENT_ERROR.clone(), file_system_err],
                    ));
                }
            };
            snapshot_file_system = Some(file_system.clone());
            api_request.file_system = Some(file_system);
            api_request.replace_file_system =
                request_file_system.kind == requestfilesystem::Kind::FULL;
        }
        let root = self.snapshot_host.new_root_snapshot_exported();
        let (snapshot, err) = self.clone_api_snapshot(
            ctx,
            &root,
            file_changes,
            &api_request,
            user_preferences,
            prepare_auto_imports.as_ref(),
        );
        project::Snapshot::deref(&root);
        if let Some(err) = err {
            project::Snapshot::deref(&snapshot);
            return Err(errors::errorf(
                format!("{}: failed to create snapshot: {}", *ERR_CLIENT_ERROR, err),
                vec![ERR_CLIENT_ERROR.clone(), err],
            ));
        }
        // ts#64554
        if let Err(err) = self.validate_prepared_auto_imports(
            ctx,
            &snapshot,
            params.prepare_auto_imports.as_ref(),
        ) {
            project::Snapshot::deref(&snapshot);
            return Err(err);
        }
        // ts#64299
        if let Some(err) = module_resolution_error(&snapshot) {
            project::Snapshot::deref(&snapshot);
            return Err(err);
        }

        let response = self.create_snapshot_response(
            &snapshot,
            None,
            Some(&params.snapshot_request_changes_params),
        );
        self.register_snapshot(snapshot, open_state, snapshot_file_system);
        Ok(response)
    }

    // Go: api/session.go handleUpdateSnapshot (ts#64204)
    // PORT: Go `defer func() { _ = s.releaseSnapshot(params.Snapshot) }()`: the
    // body runs in a closure, and the release runs after it on every path.
    pub fn handle_update_snapshot(
        &self,
        ctx: &Context,
        params: &UpdateSnapshotParams,
    ) -> Result<CreateSnapshotResponse, GoError> {
        let base_sd = self.retain_snapshot_data(params.snapshot)?;
        let result = (|| {
            let default_changes = CreateSnapshotParams::default();
            let changes = params.changes.as_ref().unwrap_or(&default_changes);
            let mut api_request =
                self.to_api_snapshot_request(ctx, &changes.snapshot_request_changes_params)?;
            // ts#64554 (Go N' api/session.go:1475)
            let user_preferences = changes.user_preferences.as_ref();
            let prepare_auto_imports = changes
                .prepare_auto_imports
                .as_ref()
                .map(|file| file.to_uri(&self.get_current_directory()));
            let open_state = self.reconcile_snapshot_opens(
                &mut api_request,
                SnapshotOpenState {
                    open_projects: base_sd.open_projects.clone(),
                    open_files: base_sd.open_files.clone(),
                },
            );
            let mut file_changes = self.to_file_change_summary(changes.file_notifications.as_ref());
            let mut snapshot_file_system = base_sd.file_system.clone();
            if let Some(request_file_system) = &changes.file_system {
                let base_file_system = snapshot_file_system.clone().unwrap_or_else(|| self.fs());
                let file_system = match requestfilesystem::new_for_update(
                    Some(request_file_system),
                    base_file_system,
                    &self.get_current_directory(),
                    &mut file_changes,
                ) {
                    Ok(file_system) => file_system,
                    Err(file_system_err) => {
                        return Err(errors::errorf(
                            format!("{}: {}", *ERR_CLIENT_ERROR, file_system_err),
                            vec![ERR_CLIENT_ERROR.clone(), file_system_err],
                        ));
                    }
                };
                snapshot_file_system = Some(file_system);
            }
            if let Some(snapshot_file_system) = &snapshot_file_system {
                api_request.file_system = Some(snapshot_file_system.clone());
                api_request.replace_file_system = changes
                    .file_system
                    .as_ref()
                    .is_some_and(|file_system| file_system.kind == requestfilesystem::Kind::FULL);
            }
            let (snapshot, err) = self.clone_api_snapshot(
                ctx,
                &base_sd.snapshot,
                file_changes,
                &api_request,
                user_preferences,
                prepare_auto_imports.as_ref(),
            );
            if let Some(err) = err {
                project::Snapshot::deref(&snapshot);
                return Err(errors::errorf(
                    format!("{}: failed to update snapshot: {}", *ERR_CLIENT_ERROR, err),
                    vec![ERR_CLIENT_ERROR.clone(), err],
                ));
            }
            // ts#64554
            if let Err(err) = self.validate_prepared_auto_imports(
                ctx,
                &snapshot,
                changes.prepare_auto_imports.as_ref(),
            ) {
                project::Snapshot::deref(&snapshot);
                return Err(err);
            }
            // ts#64299
            if let Some(err) = module_resolution_error(&snapshot) {
                project::Snapshot::deref(&snapshot);
                return Err(err);
            }

            let response = self.create_snapshot_response(
                &snapshot,
                Some(&base_sd.snapshot),
                Some(&changes.snapshot_request_changes_params),
            );
            self.register_snapshot(snapshot, open_state, snapshot_file_system);
            Ok(response)
        })();
        let _ = self.release_snapshot(params.snapshot);
        result
    }

    // Go: project/snapshothost.go:111 SnapshotHost.CloneSnapshot (N', with
    // the ts#64554 lines :125-:129)
    // PORT: Go sets `UserPreferences` and `PrepareAutoImports` on the
    // `APISnapshotRequest`, and `CloneSnapshot` moves them into the change.
    // The server lane owns `project/snapshothost.rs` and that request type
    // (bump D step 2), so until it ports ts#64554 the session builds the
    // change here. After that, this is `snapshot_host.clone_snapshot` with
    // the two fields set on `api_request`.
    fn clone_api_snapshot(
        &self,
        ctx: &Context,
        base_snapshot: &Rc<project::Snapshot>,
        file_changes: project::FileChangeSummary,
        api_request: &project::APISnapshotRequest,
        user_preferences: Option<&ls::lsutil::UserPreferences>,
        prepare_auto_imports: Option<&lsproto::DocumentUri>,
    ) -> (Rc<project::Snapshot>, Option<GoError>) {
        let mut change = project::SnapshotChange {
            api_request: Some(api_request.clone()),
            file_changes,
            fs: api_request.file_system.clone(),
            file_system_override: api_request.file_system.is_some(),
            replace_file_system: api_request.replace_file_system,
            new_config: user_preferences.cloned(),
            ..Default::default()
        };
        if let Some(uri) = prepare_auto_imports {
            change.resource_request = base_snapshot.resource_request_for_document(uri);
            change.resource_request.auto_imports = uri.clone();
        }
        let snapshot = self.snapshot_host.update(ctx, base_snapshot, change);
        let api_error = snapshot.api_error.clone();
        (snapshot, api_error)
    }

    // Go: api/session.go:1659 validatePreparedAutoImports (ts#64554)
    pub fn validate_prepared_auto_imports(
        &self,
        ctx: &Context,
        snapshot: &Rc<project::Snapshot>,
        file: Option<&DocumentIdentifier>,
    ) -> Result<(), GoError> {
        let Some(file) = file else {
            return Ok(());
        };
        if let Some(err) = ctx.err() {
            return Err(err);
        }
        let uri = file.to_uri(&self.get_current_directory());
        let prepared = snapshot.get_default_project(&uri).is_some_and(|proj| {
            let registry = snapshot.auto_import_registry();
            registry.is_some()
                && autoimport::Registry::is_prepared_for_importing_file(
                    registry.as_deref(),
                    &uri.file_name(),
                    &autoimport::ProjectID(proj.borrow().id().0.clone()),
                    &snapshot.user_preferences(),
                )
        });
        if !prepared {
            return Err(errors::errorf(
                format!(
                    "{}: could not prepare auto-imports for {}",
                    *ERR_CLIENT_ERROR, file
                ),
                vec![ERR_CLIENT_ERROR.clone()],
            ));
        }
        Ok(())
    }

    // Go: api/session.go toAPISnapshotRequest (ts#64204, ts#64319, ts#64324, ts#64391)
    pub fn to_api_snapshot_request(
        &self,
        ctx: &Context,
        changes: &SnapshotRequestChangesParams,
    ) -> Result<project::APISnapshotRequest, GoError> {
        let mut api_request = project::APISnapshotRequest::default();
        let cwd = self.get_current_directory();

        for p in &changes.open_projects {
            let config_file_name = p.to_absolute_file_name(&cwd);
            let (configured_project_id, ok) =
                project::parse_configured_project_id(&self.to_path(&config_file_name));
            if !ok {
                return Err(errors::errorf(
                    format!(
                        "{}: invalid configured project ID: {}",
                        *ERR_CLIENT_ERROR, config_file_name
                    ),
                    vec![ERR_CLIENT_ERROR.clone()],
                ));
            }
            api_request
                .ensure_programs
                .get_or_insert_with(|| {
                    FxHashSet::with_capacity_and_hasher(
                        changes.open_projects.len(),
                        Default::default(),
                    )
                })
                .insert(configured_project_id.as_id());
            api_request
                .open_projects
                .get_or_insert_with(|| {
                    FxHashSet::with_capacity_and_hasher(
                        changes.open_projects.len(),
                        Default::default(),
                    )
                })
                .insert(config_file_name);
        }

        for p in &changes.close_projects {
            let config_path = self.to_path(&p.to_absolute_file_name(&cwd));
            api_request
                .close_projects
                .get_or_insert_with(|| {
                    FxHashSet::with_capacity_and_hasher(
                        changes.close_projects.len(),
                        Default::default(),
                    )
                })
                .insert(config_path);
        }

        if let Some(open_files) = &changes.open_files {
            for f in open_files {
                let file_name = f.to_absolute_file_name(&cwd);
                let path = self.to_path(&file_name);
                if api_request.open_files.is_none() {
                    api_request.open_files = Some(IndexMap::with_capacity(open_files.len()));
                    api_request.ensure_files = Some(IndexMap::with_capacity(open_files.len()));
                }
                let request_open_files = api_request.open_files.as_mut().expect("set above");
                if !request_open_files.contains_key(&path) {
                    request_open_files.insert(path.clone(), file_name.clone());
                    api_request
                        .ensure_files
                        .as_mut()
                        .expect("set above")
                        .insert(path, file_name);
                }
            }
        }

        for f in &changes.close_files {
            let path = self.to_path(&f.to_uri(&cwd).file_name());
            api_request
                .close_files
                .get_or_insert_with(|| {
                    FxHashSet::with_capacity_and_hasher(
                        changes.close_files.len(),
                        Default::default(),
                    )
                })
                .insert(path);
        }

        let create_programs: &[Option<CreateSnapshotProgramParams>] =
            changes.create_programs.as_deref().unwrap_or(&[]);
        api_request.create_programs = Vec::with_capacity(create_programs.len());
        for (i, program_params) in create_programs.iter().enumerate() {
            let Some(program_params) = program_params else {
                return Err(errors::errorf(
                    format!(
                        "{}: createPrograms[{i}] must not be null",
                        *ERR_CLIENT_ERROR
                    ),
                    vec![ERR_CLIENT_ERROR.clone()],
                ));
            };
            let root_file_names: Vec<String> = program_params
                .root_files
                .iter()
                .map(|root_file| root_file.to_absolute_file_name(&cwd))
                .collect();
            let mut request = project::APICreateProgramRequest {
                root_file_names,
                compiler_options: Rc::new(program_params.compiler_options.clone()),
                ..Default::default()
            };
            if let Some(options) = &program_params.options {
                request.project_references = options.project_references.clone();
                request.config_file_parsing_diagnostics = options
                    .config_file_parsing_diagnostics
                    .iter()
                    .map(DiagnosticResponse::to_diagnostic)
                    .collect();
                // ts#64299
                request.module_resolver_factory = self.module_resolver_factory(ctx, options)?;
                request.module_resolver_id = options.module_resolver.0;
            }
            api_request.create_programs.push(request);
        }
        api_request.reconfigure_programs = Vec::with_capacity(changes.reconfigure_programs.len());
        let mut reconfigured_program_ids: FxHashSet<project::SyntheticProjectID> =
            FxHashSet::default();
        for (i, program_params) in changes.reconfigure_programs.iter().enumerate() {
            let Some(program_params) = program_params else {
                return Err(errors::errorf(
                    format!(
                        "{}: reconfigurePrograms[{i}] must not be null",
                        *ERR_CLIENT_ERROR
                    ),
                    vec![ERR_CLIENT_ERROR.clone()],
                ));
            };
            let (program_id, ok) = project::parse_synthetic_project_id(&program_params.id.0);
            if !ok {
                return Err(errors::errorf(
                    format!(
                        "{}: invalid synthetic project handle: {}",
                        *ERR_CLIENT_ERROR, program_params.id.0
                    ),
                    vec![ERR_CLIENT_ERROR.clone()],
                ));
            }
            if reconfigured_program_ids.contains(&program_id) {
                return Err(errors::errorf(
                    format!(
                        "{}: synthetic program reconfigured more than once: {}",
                        *ERR_CLIENT_ERROR, program_id.0
                    ),
                    vec![ERR_CLIENT_ERROR.clone()],
                ));
            }
            reconfigured_program_ids.insert(program_id.clone());
            let root_file_names: Vec<String> = program_params
                .root_files
                .iter()
                .map(|root_file| root_file.to_absolute_file_name(&cwd))
                .collect();
            let mut request = project::APIReconfigureProgramRequest {
                program_id,
                api_create_program_request: project::APICreateProgramRequest {
                    root_file_names,
                    compiler_options: Rc::new(program_params.compiler_options.clone()),
                    ..Default::default()
                },
            };
            if let Some(options) = &program_params.options {
                request.api_create_program_request.project_references =
                    options.project_references.clone();
                request
                    .api_create_program_request
                    .config_file_parsing_diagnostics = options
                    .config_file_parsing_diagnostics
                    .iter()
                    .map(DiagnosticResponse::to_diagnostic)
                    .collect();
                // ts#64299
                request.api_create_program_request.module_resolver_factory =
                    self.module_resolver_factory(ctx, options)?;
                request.api_create_program_request.module_resolver_id = options.module_resolver.0;
            }
            api_request.reconfigure_programs.push(request);
        }
        if !changes.remove_programs.is_empty() {
            api_request.remove_programs = Some(FxHashSet::with_capacity_and_hasher(
                changes.remove_programs.len(),
                Default::default(),
            ));
        }
        for program_id in &changes.remove_programs {
            if reconfigured_program_ids.contains(program_id) {
                return Err(errors::errorf(
                    format!(
                        "{}: synthetic program cannot be reconfigured and removed: {}",
                        *ERR_CLIENT_ERROR, program_id.0
                    ),
                    vec![ERR_CLIENT_ERROR.clone()],
                ));
            }
            api_request
                .remove_programs
                .as_mut()
                .expect("set above")
                .insert(program_id.clone());
        }
        if let Some(ensure_programs) = &changes.ensure_programs {
            api_request.ensure_all_programs = ensure_programs.all;
            if !ensure_programs.projects.is_empty() && api_request.ensure_programs.is_none() {
                api_request.ensure_programs = Some(FxHashSet::with_capacity_and_hasher(
                    ensure_programs.projects.len(),
                    Default::default(),
                ));
            }
            for program in &ensure_programs.projects {
                api_request
                    .ensure_programs
                    .as_mut()
                    .expect("set above")
                    .insert(program.clone());
            }
        }
        Ok(api_request)
    }

    // Go: api/session.go toLanguageServerSnapshotUpdate (ts#64204)
    pub fn to_language_server_snapshot_update(
        &self,
        ctx: &Context,
        changes: &SnapshotRequestChangesParams,
    ) -> Result<LanguageServerSnapshotUpdate, GoError> {
        let mut api_request = self.to_api_snapshot_request(ctx, changes)?;
        let open_state = self.reconcile_snapshot_opens(
            &mut api_request,
            SnapshotOpenState {
                open_projects: self.open_projects.borrow().clone(),
                open_files: self.open_files.borrow().clone(),
            },
        );

        if let Some(remove_programs) = &mut api_request.remove_programs {
            let created_programs = self.created_programs.borrow();
            remove_programs.retain(|program_id| created_programs.contains(program_id));
        }
        for reconfigure in &api_request.reconfigure_programs {
            if !self
                .created_programs
                .borrow()
                .contains(&reconfigure.program_id)
            {
                return Err(errors::errorf(
                    format!(
                        "{}: synthetic program is not owned by this API session: {}",
                        *ERR_CLIENT_ERROR, reconfigure.program_id.0
                    ),
                    vec![ERR_CLIENT_ERROR.clone()],
                ));
            }
        }
        Ok(LanguageServerSnapshotUpdate {
            request: api_request,
            open_state,
        })
    }

    // Go: api/session.go reconcileSnapshotOpens (ts#64204)
    pub fn reconcile_snapshot_opens(
        &self,
        api_request: &mut project::APISnapshotRequest,
        base: SnapshotOpenState,
    ) -> SnapshotOpenState {
        let mut state = SnapshotOpenState {
            open_projects: base.open_projects.clone(),
            open_files: base.open_files.clone(),
        };
        if let Some(close_projects) = &mut api_request.close_projects {
            close_projects.retain(|path| state.open_projects.remove(path));
        }
        if let Some(open_projects) = &mut api_request.open_projects {
            open_projects.retain(|config_file_name| {
                let path = self.to_path(config_file_name);
                if state.open_projects.contains(&path) {
                    false
                } else {
                    state.open_projects.insert(path);
                    true
                }
            });
        }
        if let Some(close_files) = &mut api_request.close_files {
            close_files.retain(|path| state.open_files.remove(path));
        }
        if let Some(open_files) = &mut api_request.open_files {
            open_files.retain(|path, _| {
                if state.open_files.contains(path) {
                    false
                } else {
                    state.open_files.insert(path.clone());
                    true
                }
            });
        }
        state
    }

    // Go: api/session.go registerSnapshot (ts#64204)
    pub fn register_snapshot(
        &self,
        snapshot: Rc<project::Snapshot>,
        open_state: SnapshotOpenState,
        file_system: Option<Rc<dyn vfs::Fs>>,
    ) {
        // If the same snapshot ID is returned (no changes), we increment the ref count
        // so each client-side Snapshot can be disposed independently.
        let handle = snapshot_handle(&snapshot);
        let existing = self.snapshots.borrow().get(&handle).cloned();
        if let Some(sd) = existing {
            // Same snapshot already stored — release the caller's ref since
            // the stored snapshot already has one, and bump the API refcount.
            project::Snapshot::deref(&snapshot);
            sd.ref_count.set(sd.ref_count.get() + 1);
        } else {
            let sd = Rc::new(SnapshotData {
                handle,
                snapshot,
                file_system,
                ref_count: Cell::new(1),
                open_projects: open_state.open_projects.clone(),
                open_files: open_state.open_files.clone(),
                symbol_registry: RefCell::new(FxHashMap::default()),
                symbol_canonical_projects: RefCell::new(FxHashMap::default()),
                project_registries: RefCell::new(FxHashMap::default()),
            });
            self.snapshots.borrow_mut().insert(handle, sd);
        }
    }

    // Go: api/session.go handleGetCurrentLanguageServerSnapshot (ts#64204)
    // PORT: Go `defer func() { _ = s.releaseSnapshot(params.BaseSnapshot) }()`:
    // the body runs in a closure, and the release runs after it.
    pub fn handle_get_current_language_server_snapshot(
        &self,
        ctx: &Context,
        params: &GetCurrentLanguageServerSnapshotParams,
    ) -> Result<CreateSnapshotResponse, GoError> {
        let Some(project_session) = self.project_session.clone() else {
            return Err(errors::errorf(
                format!(
                    "{}: getCurrentLanguageServerSnapshot requires an LSP-connected API session",
                    *ERR_CLIENT_ERROR
                ),
                vec![ERR_CLIENT_ERROR.clone()],
            ));
        };
        let mut base_snapshot: Option<Rc<project::Snapshot>> = None;
        if params.base_snapshot.0 != 0 {
            let base_sd = self.retain_snapshot_data(params.base_snapshot)?;
            base_snapshot = Some(base_sd.snapshot.clone());
        }
        let result = (|| {
            let default_changes = LanguageServerSnapshotChanges::default();
            let changes = params.changes.as_ref().unwrap_or(&default_changes);
            let update = self.to_language_server_snapshot_update(
                ctx,
                &changes.snapshot_request_changes_params,
            )?;

            let snapshot = match project_session.api_update(
                ctx,
                project::FileChangeSummary::default(),
                Some(&update.request),
            ) {
                Ok(snapshot) => snapshot,
                Err(err) => {
                    return Err(errors::errorf(
                        format!(
                            "{}: failed to update language server snapshot: {}",
                            *ERR_CLIENT_ERROR, err
                        ),
                        vec![ERR_CLIENT_ERROR.clone(), err],
                    ));
                }
            };

            update.commit(self, &snapshot);
            let response = self.create_snapshot_response(
                &snapshot,
                base_snapshot.as_ref(),
                Some(&changes.snapshot_request_changes_params),
            );
            self.register_snapshot(
                snapshot,
                SnapshotOpenState {
                    open_projects: self.open_projects.borrow().clone(),
                    open_files: self.open_files.borrow().clone(),
                },
                None,
            );
            Ok(response)
        })();
        if params.base_snapshot.0 != 0 {
            let _ = self.release_snapshot(params.base_snapshot);
        }
        result
    }

    // Go: api/session.go handleCreateBuildOrchestrator (ts#64158)
    pub fn handle_create_build_orchestrator(
        &self,
        _ctx: &Context,
        params: &CreateBuildOrchestratorParams,
    ) -> Result<CreateBuildOrchestratorResponse, GoError> {
        let build_sys = self.get_build_sys(params);
        let mut command =
            crate::execute::build::parse_build_command_line(&params.root_names, &*build_sys);
        let mut created_orchestrator_response = CreateBuildOrchestratorResponse::default();
        if let Some(compiler_options) = &params.compiler_options {
            command.compiler_options = Rc::new(compiler_options.clone());
        }
        if let Some(build_options) = &params.build_options {
            command.build_options = build_options.clone();
        }
        let orchestrator = crate::execute::build::orchestrator::new_orchestrator(
            crate::execute::build::orchestrator::Options {
                sys: build_sys,
                command: Rc::new(command),
                testing: None,
            },
        );
        created_orchestrator_response.build_orchestrator_id = new_build_orchestrator_id();
        self.build_orchestrators.borrow_mut().insert(
            created_orchestrator_response.build_orchestrator_id,
            Rc::new(RefCell::new(orchestrator)),
        );
        Ok(created_orchestrator_response)
    }

    // Go: api/session.go handleDisposeBuildOrchestrator (ts#64158)
    pub fn handle_dispose_build_orchestrator(
        &self,
        _ctx: &Context,
        params: &DisposeBuildOrchestratorParams,
    ) -> Result<Option<Box<dyn AnyValue>>, GoError> {
        let removed = self
            .build_orchestrators
            .borrow_mut()
            .remove(&params.build_orchestrator_id);
        if removed.is_none() {
            return Err(errors::new("build orchestrator not found while disposing"));
        }
        Ok(to_any(true))
    }

    /// The orchestrator for an ID. The map borrow ends before the build runs.
    fn build_orchestrator(
        &self,
        id: BuildOrchestratorID,
    ) -> Option<Rc<RefCell<crate::execute::build::orchestrator::Orchestrator>>> {
        self.build_orchestrators.borrow().get(&id).cloned()
    }

    // Go: api/session.go handleBuild (ts#64158)
    pub fn handle_build(
        &self,
        ctx: &Context,
        params: &BuildParams,
    ) -> Result<BuildResponse, GoError> {
        let Some(orchestrator) = self.build_orchestrator(params.build_orchestrator_id) else {
            return Err(errors::new(format!(
                "build orchestrator not found while building {}",
                params.project
            )));
        };
        let result = orchestrator.borrow_mut().build(ctx, &params.project);

        Ok(BuildResponse {
            status: result.result.status,
            diagnostics: new_diagnostic_responses(&result.errors),
            statistics: result.statistics.clone(),
        })
    }

    // Go: api/session.go handleBuildReferences (ts#64158)
    pub fn handle_build_references(
        &self,
        ctx: &Context,
        params: &BuildParams,
    ) -> Result<BuildResponse, GoError> {
        let Some(orchestrator) = self.build_orchestrator(params.build_orchestrator_id) else {
            return Err(errors::new(format!(
                "build orchestrator not found for building references for {}",
                params.project
            )));
        };
        let result = orchestrator
            .borrow_mut()
            .build_references(ctx, &params.project);

        Ok(BuildResponse {
            status: result.result.status,
            diagnostics: new_diagnostic_responses(&result.errors),
            statistics: result.statistics.clone(),
        })
    }

    // Go: api/session.go handleCleanBuild (ts#64158)
    pub fn handle_clean_build(
        &self,
        _ctx: &Context,
        params: &CleanBuildParams,
    ) -> Result<CleanBuildResponse, GoError> {
        let Some(orchestrator) = self.build_orchestrator(params.build_orchestrator_id) else {
            return Err(errors::new(format!(
                "build orchestrator not found while cleaning {}",
                params.project
            )));
        };
        // PORT: Go `Clean` is `clean_exported` (the build lane names it so;
        // Go also has `clean`).
        let result = orchestrator.borrow_mut().clean_exported(&params.project);
        Ok(CleanBuildResponse {
            status: result.result.status,
            diagnostics: new_diagnostic_responses(&result.errors),
            statistics: result.statistics.clone(),
            files_deleted: result.files_to_delete.clone(),
        })
    }

    // Go: api/session.go handleCleanReferences (ts#64158)
    pub fn handle_clean_references(
        &self,
        _ctx: &Context,
        params: &CleanBuildParams,
    ) -> Result<CleanBuildResponse, GoError> {
        let Some(orchestrator) = self.build_orchestrator(params.build_orchestrator_id) else {
            return Err(errors::new(format!(
                "build orchestrator not found while cleaning references for {}",
                params.project
            )));
        };
        let result = orchestrator.borrow_mut().clean_references(&params.project);
        Ok(CleanBuildResponse {
            status: result.result.status,
            diagnostics: new_diagnostic_responses(&result.errors),
            statistics: result.statistics.clone(),
            files_deleted: result.files_to_delete.clone(),
        })
    }

    // Go: api/session.go getBuildSys (ts#64158)
    pub fn get_build_sys(&self, params: &CreateBuildOrchestratorParams) -> Rc<ApiBuildSystem> {
        let mut current_directory = params.cwd.clone();
        if current_directory.is_empty() {
            current_directory = self.get_current_directory();
        }
        Rc::new(ApiBuildSystem {
            snapshot_host: self.snapshot_host.clone(),
            project_session: self.project_session.clone(),
            current_directory,
            start: std::time::Instant::now(),
        })
    }

    // Go: api/session.go:1592 handleRelease
    // handleRelease decrements the ref count for a snapshot.
    // The snapshot and its registries are only cleaned up when the ref count reaches zero.
    pub fn handle_release(
        &self,
        _ctx: &Context,
        params: Option<&ReleaseParams>,
    ) -> Result<Option<Box<dyn AnyValue>>, GoError> {
        let Some(params) = params.filter(|params| params.snapshot.0 != 0) else {
            return Err(errors::errorf(
                format!("{}: empty handle", *ERR_CLIENT_ERROR),
                vec![ERR_CLIENT_ERROR.clone()],
            ));
        };

        self.release_snapshot(params.snapshot)?;
        Ok(to_any(true))
    }

    // Go: api/session.go:1606 handleGetDefaultProjectForFile
    // handleGetDefaultProjectForFile returns the default project for a given file,
    // or nil if no project currently contains the file.
    pub fn handle_get_default_project_for_file(
        &self,
        _ctx: &Context,
        params: &GetDefaultProjectForFileParams,
    ) -> Result<Option<ProjectResponse>, GoError> {
        let sd = self.get_snapshot_data(params.snapshot)?;

        let uri = params.file.to_uri(&self.get_current_directory());
        let proj = sd.snapshot.get_default_project(&uri);
        let Some(proj) = proj else {
            return Ok(None);
        };

        Ok(Some(new_project_response(&proj.borrow())))
    }

    // Go: api/session.go:1749 handleParseCommandLine
    // handleParseCommandLine parses command-line arguments.
    pub fn handle_parse_command_line(
        &self,
        _ctx: &Context,
        params: &ParseCommandLineParams,
    ) -> Result<Option<ConfigFileResponse>, GoError> {
        Ok(new_config_file_response(Some(
            &tsoptions::parse_command_line(&params.command_line, &*self.snapshot_host),
        )))
    }

    // Go: api/session.go:1754 handleReadConfigFile
    // handleReadConfigFile reads and parses a JSON configuration file.
    pub fn handle_read_config_file(
        &self,
        _ctx: &Context,
        params: &ReadConfigFileParams,
    ) -> Result<ReadConfigFileResponse, GoError> {
        let config_file_name = params
            .file
            .to_absolute_file_name(&self.get_current_directory());
        let (config_file_content, ok) = self.snapshot_host.fs().read_file(&config_file_name);
        if !ok {
            return Ok(ReadConfigFileResponse {
                config: tsoptions::CompilerOptionsValue::Map(IndexMap::new()),
                error: Some(new_diagnostic_response(&new_compiler_diagnostic(
                    diag::Cannot_read_file_0,
                    args![config_file_name],
                ))),
            });
        }

        let (config, parse_errors) = tsoptions::parse_config_file_text_to_json(
            &config_file_name,
            self.to_path(&config_file_name),
            &config_file_content,
        );
        let mut response = ReadConfigFileResponse {
            config,
            error: None,
        };
        if !parse_errors.is_empty() {
            response.error = Some(new_diagnostic_response(&parse_errors[0]));
        }
        Ok(response)
    }

    // Go: api/session.go:1777 handleParseJsonConfigFileContent
    // handleParseJsonConfigFileContent parses an in-memory JSON configuration.
    pub fn handle_parse_json_config_file_content(
        &self,
        _ctx: &Context,
        params: &ParseJsonConfigFileContentParams,
    ) -> Result<Option<ConfigFileResponse>, GoError> {
        if params.config_directory.is_none() == params.config_file_name.is_none() {
            return Err(errors::errorf(
                format!(
                    "{}: exactly one of configDirectory or configFileName is required",
                    *ERR_CLIENT_ERROR
                ),
                vec![ERR_CLIENT_ERROR.clone()],
            ));
        }

        let base_path;
        let mut config_file_name = String::new();
        if let Some(config_directory) = &params.config_directory {
            base_path = tspath::get_normalized_absolute_path(
                config_directory,
                &self.get_current_directory(),
            );
        } else {
            config_file_name = params
                .config_file_name
                .as_ref()
                .expect("configFileName is set")
                .to_absolute_file_name(&self.get_current_directory());
            base_path = tspath::get_directory_path(&config_file_name);
        }

        let parsed_command_line = tsoptions::parse_json_config_file_content(
            &json_value_to_any(&params.json),
            &*self.snapshot_host,
            &base_path,
            None, /*existingOptions*/
            &config_file_name,
            &[],  /*resolutionStack*/
            None, /*extendedConfigCache*/
        );
        Ok(new_config_file_response(Some(&parsed_command_line)))
    }

    // Go: api/session.go:1804 handleParseConfigFile
    // handleParseConfigFile parses a tsconfig.json file and returns its contents.
    pub fn handle_parse_config_file(
        &self,
        _ctx: &Context,
        params: &ParseConfigFileParams,
    ) -> Result<ConfigFileResponse, GoError> {
        let config_file_name = params
            .file
            .to_absolute_file_name(&self.get_current_directory());
        let (config_file_content, ok) = self.snapshot_host.fs().read_file(&config_file_name);
        if !ok {
            return Err(errors::errorf(
                format!(
                    "{}: could not read file {}",
                    *ERR_CLIENT_ERROR,
                    gostd::strconv::quote(&config_file_name)
                ),
                vec![ERR_CLIENT_ERROR.clone()],
            ));
        }

        let config_dir = tspath::get_directory_path(&config_file_name);
        let ts_config_source_file = tsoptions::new_tsconfig_source_file_from_file_path(
            &config_file_name,
            self.to_path(&config_file_name),
            &config_file_content,
        );
        let parsed_command_line = tsoptions::parse_json_source_file_config_file_content(
            ts_config_source_file,
            &*self.snapshot_host,
            &config_dir,
            None, /*existingOptions*/
            None, /*existingOptionsRaw*/
            &config_file_name,
            &[],  /*resolutionStack*/
            None, /*extendedConfigCache*/
        );

        // PORT: Go returns a `*ConfigFileResponse` that is never nil here.
        Ok(new_config_file_response(Some(&parsed_command_line))
            .expect("NewConfigFileResponse of a parsed command line"))
    }

    // Go: api/session.go handleCreateSourceFile (ts#64216, ts#64434)
    pub fn handle_create_source_file(
        &self,
        _ctx: &Context,
        params: &CreateSourceFileParams,
    ) -> Result<Option<Box<dyn AnyValue>>, GoError> {
        let lease =
            self.create_source_file(&params.file_name, &params.source_text, &params.options)?;
        self.encode_leased_source_file(lease)
    }

    // Go: api/session.go handleCreateSourceFileFromFile (ts#64216, ts#64434)
    pub fn handle_create_source_file_from_file(
        &self,
        _ctx: &Context,
        params: &CreateSourceFileFromFileParams,
    ) -> Result<Option<Box<dyn AnyValue>>, GoError> {
        let file_name =
            tspath::get_normalized_absolute_path(&params.file_name, &self.get_current_directory());
        let (source_text, ok) = self.snapshot_host.fs().read_file(&file_name);
        if !ok {
            return Err(errors::errorf(
                format!(
                    "{}: could not read file {}",
                    *ERR_CLIENT_ERROR,
                    gostd::strconv::quote(&file_name)
                ),
                vec![ERR_CLIENT_ERROR.clone()],
            ));
        }
        let lease = self.create_source_file(&file_name, &source_text, &params.options)?;
        self.encode_leased_source_file(lease)
    }

    // Go: api/session.go createSourceFile (ts#64216, ts#64434)
    pub fn create_source_file(
        &self,
        file_name: &str,
        source_text: &str,
        options: &CreateSourceFileOptions,
    ) -> Result<Rc<project::SourceFileLease>, GoError> {
        let mut script_kind = options.script_kind;
        if script_kind == ScriptKind::UNKNOWN {
            script_kind = crate::frontend::core_ext::ensure_script_kind_from_file_name(file_name);
        }
        if !is_valid_create_source_file_script_kind(script_kind) {
            return Err(errors::errorf(
                format!(
                    "{}: invalid scriptKind {}",
                    *ERR_CLIENT_ERROR, script_kind.0
                ),
                vec![ERR_CLIENT_ERROR.clone()],
            ));
        }
        let file_name =
            tspath::get_normalized_absolute_path(file_name, &self.get_current_directory());
        let path = self.to_path(&file_name);
        Ok(self.acquire_source_file(
            crate::frontend::parser::SourceFileParseOptions {
                file_name,
                path,
                ..Default::default()
            },
            source_text,
            script_kind,
        ))
    }

    // Go: api/session.go acquireSourceFile (ts#64434)
    // The leased file is bound, so its encoded node flags have the binder
    // bits (`project::acquire_bound`, Go parsecache.go:80).
    pub fn acquire_source_file(
        &self,
        options: crate::frontend::parser::SourceFileParseOptions,
        source_text: &str,
        script_kind: ScriptKind,
    ) -> Rc<project::SourceFileLease> {
        self.snapshot_host
            .acquire_source_file(options, source_text, script_kind)
    }

    // Go: api/session.go encodeLeasedSourceFile (ts#64434)
    pub fn encode_leased_source_file(
        &self,
        lease: Rc<project::SourceFileLease>,
    ) -> Result<Option<Box<dyn AnyValue>>, GoError> {
        let mut data = match encoder::encode_parsed_source_file(lease.parsed_source_file()) {
            Ok((data, _)) => data,
            Err(err) => {
                lease.release();
                project::drop_released_lease(lease);
                return Err(errors::errorf(
                    format!("failed to encode source file: {err}"),
                    vec![err],
                ));
            }
        };
        // ts#64518
        encoder::set_source_file_id(&mut data, source_file_node_id(lease.source_file()));
        let id = self.register_source_file_lease(lease);
        encoder::set_source_file_lease(&mut data, id.0);
        if self.use_binary_responses {
            return Ok(to_any(RawBinary(data)));
        }
        Ok(to_any(SourceFileResponse {
            data: base64_std_encoding_encode_to_string(&data),
        }))
    }

    // Go: api/session.go:2069 handleRetainSourceFile (ts#64518)
    pub fn handle_retain_source_file(
        &self,
        params: &RetainSourceFileParams,
    ) -> Result<RetainSourceFileResponse, GoError> {
        let lease = self.acquire_cached_source_file(&params.file)?;
        Ok(RetainSourceFileResponse {
            lease: self.register_source_file_lease(lease),
        })
    }

    // Go: api/session.go:2080 handleGetCachedSourceFile (ts#64518)
    // @gen-proto-result: SourceFileResponse
    // PORT: Go encodes `lease.SourceFile()`. The lease's file may be in no
    // program, so the encoder reads the lease's parse record
    // (`encode_parsed_source_file`, as `encode_leased_source_file`).
    pub fn handle_get_cached_source_file(
        &self,
        params: &GetCachedSourceFileParams,
    ) -> Result<Option<Box<dyn AnyValue>>, GoError> {
        let lease = self.acquire_cached_source_file(&params.file)?;
        // Go: defer lease.Release()
        let result = match encoder::encode_parsed_source_file(lease.parsed_source_file()) {
            Ok((mut data, _)) => {
                encoder::set_source_file_id(&mut data, source_file_node_id(lease.source_file()));
                Ok(self.source_file_data_response(data))
            }
            Err(err) => Err(errors::errorf(
                format!("failed to encode source file: {err}"),
                vec![err],
            )),
        };
        lease.release();
        project::drop_released_lease(lease);
        result
    }

    // Go: api/session.go:2091 acquireCachedSourceFile (ts#64518)
    // acquireCachedSourceFile holds a reference to the exact ordinary cached AST identified by a
    // descriptor. It never parses; the caller must release the returned lease.
    // PORT: Go calls `SnapshotHost.AcquireExistingSourceFile(key)`
    // (project/snapshothost.go:65, ts#64518), which the server lane ports
    // in step 2. Until then this looks up the live entry of `key` in the
    // parse cache (Go `RefCountCache.AcquireExisting`,
    // project/refcountcache.go:64) and acquires it through
    // `acquire_source_file` with the entry's own text: that text has the
    // key's hash, so the acquire finds the same entry and does not parse.
    // PORT: the key hash is the descriptor's content hash, as in Go. For a
    // text with a Go string marker the port's content hash is the hash of
    // Go's bytes (`encoder::go_content_hash`), not the cache key's, so such
    // a file is "not available".
    pub fn acquire_cached_source_file(
        &self,
        descriptor: &SourceFileDescriptor,
    ) -> Result<Rc<project::SourceFileLease>, GoError> {
        let key = match descriptor.parse_cache_key() {
            Ok(key) => key,
            Err(err) => {
                return Err(errors::errorf(
                    format!(
                        "{}: invalid source file descriptor: {err}",
                        *ERR_CLIENT_ERROR
                    ),
                    vec![ERR_CLIENT_ERROR.clone(), err],
                ));
            }
        };
        let cache = &self.snapshot_host.parse_cache;
        let existing = cache.entries.borrow().get(&key).cloned().and_then(|entry| {
            let live = entry.ref_count.get() > 0 || cache.options.disable_deletion;
            live.then(|| entry.value.borrow().clone()).flatten()
        });
        let Some(existing) = existing else {
            return Err(errors::errorf(
                format!("{}: source file is not available", *ERR_CLIENT_ERROR),
                vec![ERR_CLIENT_ERROR.clone()],
            ));
        };
        let lease = self.snapshot_host.acquire_source_file(
            key.source_file_parse_options(),
            &existing.file.text,
            key.script_kind,
        );
        // The parse-cache key addresses a live ordinary file, but an equal key can identify a new
        // AST after the original entry is evicted. The node ID verifies that this is the exact AST
        // observed by the client; it is not used to address or retain the file.
        if new_parsed_source_file_descriptor(lease.parsed_source_file()) != *descriptor {
            lease.release();
            project::drop_released_lease(lease);
            return Err(errors::errorf(
                format!(
                    "{}: source file descriptor no longer identifies the cached source file",
                    *ERR_CLIENT_ERROR
                ),
                vec![ERR_CLIENT_ERROR.clone()],
            ));
        }
        Ok(lease)
    }

    // Go: api/session.go:2110 registerSourceFileLease (ts#64518)
    pub fn register_source_file_lease(
        &self,
        lease: Rc<project::SourceFileLease>,
    ) -> SourceFileLeaseID {
        self.next_source_file_lease_id
            .set(self.next_source_file_lease_id.get() + 1);
        let id = SourceFileLeaseID(self.next_source_file_lease_id.get());
        self.source_file_leases.borrow_mut().insert(id, lease);
        id
    }

    // Go: api/session.go handleReleaseSourceFile (ts#64434)
    pub fn handle_release_source_file(
        &self,
        params: Option<&ReleaseSourceFileParams>,
    ) -> Result<Option<Box<dyn AnyValue>>, GoError> {
        let Some(params) = params.filter(|params| params.lease.0 != 0) else {
            return Err(errors::errorf(
                format!("{}: empty source file lease", *ERR_CLIENT_ERROR),
                vec![ERR_CLIENT_ERROR.clone()],
            ));
        };
        let lease = self.source_file_leases.borrow_mut().remove(&params.lease);
        let Some(lease) = lease else {
            return Err(errors::errorf(
                format!(
                    "{}: source file lease {} not found",
                    *ERR_CLIENT_ERROR, params.lease.0
                ),
                vec![ERR_CLIENT_ERROR.clone()],
            ));
        };
        lease.release();
        project::drop_released_lease(lease);
        Ok(to_any(true))
    }

    // Go: api/session.go releaseSourceFileLeases (ts#64434)
    pub fn release_source_file_leases(&self) {
        let leases: Vec<Rc<project::SourceFileLease>> = self
            .source_file_leases
            .borrow_mut()
            .drain()
            .map(|(_, lease)| lease)
            .collect();
        for lease in leases {
            lease.release();
            project::drop_released_lease(lease);
        }
    }

    // Go: api/session.go:1830 handleTranspile (tsgo#4849)
    pub fn handle_transpile(
        &self,
        ctx: &Context,
        params: &TranspileParams,
        declaration: bool,
    ) -> Result<TranspileOutputResponse, GoError> {
        transpile_output(ctx, &params.input, &params.options, declaration)
    }

    // Go: api/session.go:1932 handleTranspileFromFile (tsgo#4849)
    pub fn handle_transpile_from_file(
        &self,
        ctx: &Context,
        params: &TranspileFromFileParams,
        declaration: bool,
    ) -> Result<TranspileOutputResponse, GoError> {
        let file_name =
            tspath::get_normalized_absolute_path(&params.file_name, &self.get_current_directory());
        let (input, ok) = self.snapshot_host.fs().read_file(&file_name);
        if !ok {
            return Err(errors::errorf(
                format!(
                    "{}: could not read file {}",
                    *ERR_CLIENT_ERROR,
                    gostd::strconv::quote(&file_name)
                ),
                vec![ERR_CLIENT_ERROR.clone()],
            ));
        }
        let mut options = params.options.clone();
        options.file_name = file_name;
        transpile_output(ctx, &input, &options, declaration)
    }
}

// Go: api/session.go isValidCreateSourceFileScriptKind (ts#64216)
pub fn is_valid_create_source_file_script_kind(script_kind: ScriptKind) -> bool {
    matches!(
        script_kind,
        ScriptKind::JS | ScriptKind::JSX | ScriptKind::TS | ScriptKind::TSX | ScriptKind::JSON
    )
}

// Go: api/session.go apiBuildSystem (ts#64158)
// Wrapper for the API session for build orchestrator
// PORT: Go keeps the session; the system needs its snapshot host and project
// session (Go `session.snapshotHost.FS()` and `session.DefaultLibraryPath()`).
pub struct ApiBuildSystem {
    snapshot_host: Rc<project::SnapshotHost>,
    project_session: Option<Rc<project::Session>>,
    current_directory: String,
    start: std::time::Instant,
}

impl crate::execute::tsc::System for ApiBuildSystem {
    fn writer(&self) -> crate::execute::tsc::Writer {
        Rc::new(RefCell::new(std::io::sink()))
    }
    fn error_writer(&self) -> crate::execute::tsc::ErrorWriter {
        std::sync::Arc::new(std::sync::Mutex::new(std::io::sink()))
    }
    fn fs(&self) -> Rc<dyn vfs::Fs> {
        self.snapshot_host.fs()
    }
    // PORT: Go `s.session.DefaultLibraryPath()`.
    fn default_library_path(&self) -> String {
        if let Some(project_session) = &self.project_session {
            return project_session.default_library_path();
        }
        self.snapshot_host.default_library_path()
    }
    fn get_current_directory(&self) -> String {
        self.current_directory.clone()
    }
    fn write_output_is_tty(&self) -> bool {
        false
    }
    fn get_width_of_terminal(&self) -> i32 {
        0
    }
    fn get_environment_variable(&self, _name: &str) -> (String, bool) {
        (String::new(), false)
    }
    fn spawn(
        &self,
        _command: &[String],
        _dir: &str,
        _stderr: Option<Box<dyn std::io::Write + Send>>,
    ) -> Result<std::sync::Arc<dyn crate::contentmapper::hostimpl::ProcessExitState>, GoError> {
        Err(errors::new(
            "spawning processes is not supported by the API build orchestrator",
        ))
    }
    fn now(&self) -> std::time::SystemTime {
        std::time::SystemTime::now()
    }
    fn since_start(&self) -> std::time::Duration {
        self.start.elapsed()
    }
    // PORT: the session file system (the client's, with `--callbacks`)
    // belongs to this thread, so the build writes through it here.
    fn emit_writes_through_osvfs(&self) -> bool {
        false
    }
}

// PORT: Go passes the `tsc.System` as the `tsoptions.ParseConfigHost` of
// `ParseBuildCommandLine`; Rust needs the impl.
impl tsoptions::ParseConfigHost for ApiBuildSystem {
    fn fs(&self) -> Rc<dyn vfs::Fs> {
        crate::execute::tsc::System::fs(self)
    }

    fn get_current_directory(&self) -> String {
        self.current_directory.clone()
    }
}

// Go: api/session.go:1943 transpileOutput (tsgo#4849)
fn transpile_output(
    ctx: &Context,
    input: &str,
    options: &TranspileOptions,
    declaration: bool,
) -> Result<TranspileOutputResponse, GoError> {
    let transpile_options = transpile::Options {
        compiler_options: options.compiler_options.clone(),
        file_name: options.file_name.clone(),
        report_diagnostics: options.report_diagnostics,
    };
    let output = if declaration {
        transpile::transpile_declaration(ctx, input, transpile_options)
    } else {
        transpile::transpile_module(ctx, input, transpile_options)
    };
    let Some(output) = output else {
        if let Some(err) = ctx.err() {
            return Err(err);
        }
        return Err(errors::new("transpilation produced no output"));
    };
    Ok(TranspileOutputResponse {
        output_text: output.output_text,
        diagnostics: new_diagnostic_responses(&output.diagnostics),
        source_map_text: output.source_map_text,
    })
}

impl Session {
    // Go: api/session.go:1971 handleGetSourceFile
    // handleGetSourceFile returns a source file from a project within a snapshot.
    pub fn handle_get_source_file(
        &self,
        _ctx: &Context,
        params: &GetSourceFileParams,
    ) -> Result<Option<Box<dyn AnyValue>>, GoError> {
        let sd = self.get_snapshot_data(params.snapshot)?;

        let program = &sd.get_program(&params.project)?;
        // The encoder reads lazy JSDoc (file header, "Current program").
        let _program = ls_program::enter(program);

        self.encode_source_file_response(
            program
                .get_source_file(&params.file.to_file_name())
                .map_or(Node::NIL, |f| f.root),
        )
    }

    // Go: api/session.go:1987 handleGetConfigFileNames
    // handleGetConfigFileNames returns tsconfig file names associated with the project's command line.
    // PORT: Go `program.CommandLine()` is never nil in the port.
    pub fn handle_get_config_file_names(
        &self,
        _ctx: &Context,
        params: &GetProjectDiagnosticsParams,
    ) -> Result<Vec<String>, GoError> {
        let sd = self.get_snapshot_data(params.snapshot)?;

        let program = sd.get_program(&params.project)?;

        let command_line = program.command_line();
        let Some(config_file) = command_line
            .config_file
            .as_ref()
            .filter(|f| f.source_file.is_some())
        else {
            return Ok(Vec::new());
        };

        let extended_files = command_line.extended_source_files();
        let mut config_files = Vec::with_capacity(extended_files.len() + 1);
        config_files.push(config_file.file_name.clone());
        config_files.extend(extended_files.iter().cloned());
        Ok(config_files)
    }

    // Go: api/session.go:2013 handleGetConfigSourceFile
    // handleGetConfigSourceFile returns a tsconfig source file associated with the project's command line.
    pub fn handle_get_config_source_file(
        &self,
        _ctx: &Context,
        params: &GetSourceFileParams,
    ) -> Result<Option<Box<dyn AnyValue>>, GoError> {
        let sd = self.get_snapshot_data(params.snapshot)?;

        let program = sd.get_program(&params.project)?;
        // The encoder reads lazy JSDoc (file header, "Current program").
        let _program = ls_program::enter(&program);

        let command_line = program.command_line();
        let Some(root_config_source_file) = command_line
            .config_file
            .as_ref()
            .filter(|f| f.source_file.is_some())
        else {
            return self.encode_source_file_response(Node::NIL);
        };

        let requested_path = tspath::to_path(
            &params.file.to_file_name(),
            &program.get_current_directory(),
            program.use_case_sensitive_file_names(),
        );
        if root_config_source_file.path == requested_path {
            return self.encode_source_file_response(root_config_source_file.source_file);
        }

        for config_file_name in command_line.extended_source_files() {
            if tspath::to_path(
                config_file_name,
                &program.get_current_directory(),
                program.use_case_sensitive_file_names(),
            ) != requested_path
            {
                continue;
            }

            let (config_file_content, ok) = sd.snapshot.read_file(config_file_name);
            if !ok {
                return self.encode_source_file_response(Node::NIL);
            }

            let config_source_file = tsoptions::new_tsconfig_source_file_from_file_path(
                config_file_name,
                requested_path,
                &config_file_content,
            );
            return self.encode_source_file_response(config_source_file.source_file);
        }

        self.encode_source_file_response(Node::NIL)
    }

    // Go: api/session.go:2052 encodeSourceFileResponse
    // PORT: Go `*ast.SourceFile` is the root node; `Node::NIL` is nil.
    pub fn encode_source_file_response(
        &self,
        source_file: Node,
    ) -> Result<Option<Box<dyn AnyValue>>, GoError> {
        if source_file.is_nil() {
            if self.use_binary_responses {
                return Ok(to_any(RawBinary(Vec::new())));
            }
            return Ok(None);
        }

        // Encode the full source file.
        let mut data = match encoder::encode_source_file(source_file) {
            Ok((data, _)) => data,
            Err(err) => {
                return Err(errors::errorf(
                    format!("failed to encode source file: {err}"),
                    vec![err],
                ));
            }
        };
        // ts#64518
        encoder::set_source_file_id(&mut data, source_file_node_id(source_file));

        Ok(self.source_file_data_response(data))
    }

    /// The tail of Go `encodeSourceFileResponse`: the encoded file as raw
    /// binary or base64 JSON.
    fn source_file_data_response(&self, data: Vec<u8>) -> Option<Box<dyn AnyValue>> {
        if self.use_binary_responses {
            return to_any(RawBinary(data));
        }
        to_any(SourceFileResponse {
            data: base64_std_encoding_encode_to_string(&data),
        })
    }

    // Go: api/session.go:2075 handleGetSourceFileNames
    // handleGetSourceFileNames returns file names of all source files in a project.
    pub fn handle_get_source_file_names(
        &self,
        _ctx: &Context,
        params: &GetSourceFileNamesParams,
    ) -> Result<Vec<String>, GoError> {
        let sd = self.get_snapshot_data(params.snapshot)?;

        let program = &sd.get_program(&params.project)?;

        let source_files = program.get_source_files();
        let mut result = Vec::with_capacity(source_files.len());
        for source_file in source_files {
            result.push(source_file.file_name().to_string());
        }
        Ok(result)
    }

    // Go: api/session.go:2097 handleGetSourceFileMetadata
    // handleGetSourceFileMetadata returns program-stored metadata for a single source file.
    // The client fetches this lazily per file and caches it.
    pub fn handle_get_source_file_metadata(
        &self,
        _ctx: &Context,
        params: &GetSourceFileParams,
    ) -> Result<Option<SourceFileMetadata>, GoError> {
        let sd = self.get_snapshot_data(params.snapshot)?;

        let program = &sd.get_program(&params.project)?;

        let Some(source_file) = program.get_source_file(&params.file.to_file_name()) else {
            return Ok(None);
        };

        let meta_data = program.get_source_file_meta_data(source_file.path());
        Ok(Some(SourceFileMetadata {
            is_default_library: program.is_source_file_default_library(source_file.path()),
            is_from_external_library: program.is_source_file_from_external_library(&source_file),
            package_json_type: meta_data.package_json_type,
            package_json_directory: meta_data.package_json_directory,
            implied_node_format: meta_data.implied_node_format.0,
        }))
    }
}

// Go: api/session.go newResolvedModuleResponse (ts#64247)
// PORT: Go `resolution.IsResolved()` is false for a nil resolution.
pub fn new_resolved_module_response(
    resolution: Option<&crate::program::ResolvedModule>,
) -> Option<proto::ResolvedModule> {
    let resolution = resolution.filter(|resolution| resolution.is_resolved())?;
    Some(proto::ResolvedModule {
        resolved_file_name: resolution.resolved_file_name.clone(),
        original_path: resolution.original_path.clone(),
        extension: resolution.extension.clone(),
        resolved_using_ts_extension: resolution.resolved_using_ts_extension,
        resolved_using_extra_extensions: resolution.resolved_using_extra_extensions,
        package_id: new_package_id(&resolution.package_id),
        is_external_library_import: resolution.is_external_library_import,
        alternate_result: resolution.alternate_result.clone(),
    })
}

// Go: api/session.go newResolvedTypeReferenceDirectiveResponse (ts#64247)
pub fn new_resolved_type_reference_directive_response(
    resolution: Option<&crate::frontend::module::ResolvedTypeReferenceDirective>,
) -> Option<proto::ResolvedTypeReferenceDirective> {
    let resolution = resolution.filter(|resolution| resolution.is_resolved())?;
    Some(proto::ResolvedTypeReferenceDirective {
        primary: resolution.primary,
        resolved_file_name: resolution.resolved_file_name.clone(),
        original_path: resolution.original_path.clone(),
        package_id: new_package_id(&resolution.package_id),
        is_external_library_import: resolution.is_external_library_import,
    })
}

/// Go passes an `*ast.SourceFile` where a program method takes an
/// `ast.HasFileName`; the Rust methods take `&dyn HasFileName`, which a file
/// root `Node` does not implement, so its file name and path are copied.
fn file_of(source_file: Node) -> HasFileNameImpl {
    new_has_file_name(
        source_file_file_name(source_file),
        &source_file_info(source_file).path,
    )
}

impl Session {
    // Go: api/session.go handleGetModeForUsageLocation (ts#64292)
    pub fn handle_get_mode_for_usage_location(
        &self,
        _ctx: &Context,
        params: &GetModeForUsageLocationParams,
    ) -> Result<ModuleKind, GoError> {
        let sd = self.get_snapshot_data(params.snapshot)?;
        let program = &sd.get_program(&params.project)?;
        // Current for the whole handler (file header).
        let _program = ls_program::enter(program);
        let source_file = self.resolve_optional_source_file(program, Some(&params.file))?;
        let usage = sd.resolve_node_handle(program, &params.usage)?;
        if !is_string_literal_like(usage) {
            return Err(errors::errorf(
                format!(
                    "{}: usage must be a StringLiteralLike node",
                    *ERR_CLIENT_ERROR
                ),
                vec![ERR_CLIENT_ERROR.clone()],
            ));
        }
        Ok(program.get_mode_for_usage_location(&file_of(source_file), usage))
    }

    // Go: api/session.go handleGetModeForResolutionAtIndex (ts#64292)
    pub fn handle_get_mode_for_resolution_at_index(
        &self,
        _ctx: &Context,
        params: &GetModeForResolutionAtIndexParams,
    ) -> Result<ModuleKind, GoError> {
        let sd = self.get_snapshot_data(params.snapshot)?;
        let program = &sd.get_program(&params.project)?;
        // Current for the whole handler (file header).
        let _program = ls_program::enter(program);
        let source_file = self.resolve_optional_source_file(program, Some(&params.file))?;
        let resolution_count = {
            let info = source_file_info(source_file);
            let mut resolution_count = info.imports.len() as i32;
            for &augmentation in &info.module_augmentations {
                if augmentation.kind() == SyntaxKind::StringLiteral {
                    resolution_count += 1;
                }
            }
            resolution_count
        };
        if params.index < 0 || params.index >= resolution_count {
            return Err(errors::errorf(
                format!("{}: invalid resolution index", *ERR_CLIENT_ERROR),
                vec![ERR_CLIENT_ERROR.clone()],
            ));
        }
        // The check above makes the index non-negative.
        Ok(program.get_mode_for_resolution_at_index(source_file, params.index as usize))
    }

    // Go: api/session.go handleGetResolvedModule (ts#64247)
    pub fn handle_get_resolved_module(
        &self,
        _ctx: &Context,
        params: &GetResolvedModuleParams,
    ) -> Result<Option<proto::ResolvedModule>, GoError> {
        let sd = self.get_snapshot_data(params.snapshot)?;
        let program = &sd.get_program(&params.project)?;
        // Current for the whole handler (file header).
        let _program = ls_program::enter(program);
        let source_file = self.resolve_optional_source_file(program, Some(&params.file))?;
        let resolution =
            program.get_resolved_module(&file_of(source_file), &params.module_name, params.mode);
        Ok(new_resolved_module_response(resolution.as_deref()))
    }

    // Go: api/session.go handleGetResolvedModuleFromModuleSpecifier (ts#64247)
    pub fn handle_get_resolved_module_from_module_specifier(
        &self,
        _ctx: &Context,
        params: &GetResolvedModuleFromModuleSpecifierParams,
    ) -> Result<Option<proto::ResolvedModule>, GoError> {
        let sd = self.get_snapshot_data(params.snapshot)?;
        let program = &sd.get_program(&params.project)?;
        // Current for the whole handler (file header).
        let _program = ls_program::enter(program);
        let node = sd.resolve_node_handle(program, &params.module_specifier)?;
        if !is_string_literal_like(node) {
            return Err(errors::errorf(
                format!(
                    "{}: moduleSpecifier must be a StringLiteralLike node",
                    *ERR_CLIENT_ERROR
                ),
                vec![ERR_CLIENT_ERROR.clone()],
            ));
        }
        let mut source_file = get_source_file_of_node(node);
        if params.source_file.is_some() {
            source_file =
                self.resolve_optional_source_file(program, params.source_file.as_ref())?;
        }
        if source_file.is_nil() {
            return Err(errors::errorf(
                format!(
                    "{}: moduleSpecifier must have a SourceFile ancestor or sourceFile must be provided",
                    *ERR_CLIENT_ERROR
                ),
                vec![ERR_CLIENT_ERROR.clone()],
            ));
        }
        let file = file_of(source_file);
        let mode = program.get_mode_for_usage_location(&file, node);
        let resolution = program.get_resolved_module(&file, node.text(), mode);
        Ok(new_resolved_module_response(resolution.as_deref()))
    }

    // Go: api/session.go handleGetResolvedTypeReferenceDirective (ts#64247)
    pub fn handle_get_resolved_type_reference_directive(
        &self,
        _ctx: &Context,
        params: &GetResolvedTypeReferenceDirectiveParams,
    ) -> Result<Option<proto::ResolvedTypeReferenceDirective>, GoError> {
        let sd = self.get_snapshot_data(params.snapshot)?;
        let program = &sd.get_program(&params.project)?;
        // Current for the whole handler (file header).
        let _program = ls_program::enter(program);
        let source_file = self.resolve_optional_source_file(program, Some(&params.file))?;
        let resolution = program.get_resolved_type_reference_directive(
            &file_of(source_file),
            &params.type_directive_name,
            params.mode,
        );
        Ok(new_resolved_type_reference_directive_response(
            resolution.as_deref(),
        ))
    }

    // Go: api/session.go handleGetResolvedTypeReferenceDirectiveFromReference (ts#64247)
    pub fn handle_get_resolved_type_reference_directive_from_reference(
        &self,
        _ctx: &Context,
        params: &GetResolvedTypeReferenceDirectiveFromReferenceParams,
    ) -> Result<Option<proto::ResolvedTypeReferenceDirective>, GoError> {
        let sd = self.get_snapshot_data(params.snapshot)?;
        let program = &sd.get_program(&params.project)?;
        // Current for the whole handler (file header).
        let _program = ls_program::enter(program);
        let source_file = self.resolve_optional_source_file(program, Some(&params.source_file))?;
        let file = file_of(source_file);
        let mut mode = params.resolution_mode;
        if mode == RESOLUTION_MODE_NONE {
            mode = program.get_default_resolution_mode_for_file(&file);
        }
        let resolution =
            program.get_resolved_type_reference_directive(&file, &params.type_directive_name, mode);
        Ok(new_resolved_type_reference_directive_response(
            resolution.as_deref(),
        ))
    }

    // Go: api/session.go:2288 handleGetSymbolAtPosition
    // handleGetSymbolAtPosition returns the symbol at a position in a file.
    pub fn handle_get_symbol_at_position(
        &self,
        ctx: &Context,
        params: &GetSymbolAtPositionParams,
    ) -> Result<Option<SymbolResponse>, GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        let source_file = setup
            .program
            .get_source_file(&params.file.to_file_name())
            .map_or(Node::NIL, |f| f.root);
        if source_file.is_nil() {
            return Err(errors::errorf(
                format!(
                    "{}: source file not found: {}",
                    *ERR_CLIENT_ERROR,
                    params.file.string()
                ),
                vec![ERR_CLIENT_ERROR.clone()],
            ));
        }

        let position_map = source_file_get_position_map(source_file);
        let node = astnav::get_touching_property_name(
            source_file,
            position_map.utf16_to_utf8(params.position as i32),
        );
        if node.is_nil() {
            return Ok(None);
        }

        let symbol = setup
            .checker
            .borrow_mut()
            .get_symbol_at_location_exported(node);
        if symbol.is_nil() {
            return Ok(None);
        }

        Ok(setup.new_symbol_response(symbol))
    }

    // Go: api/session.go:2317 handleGetSymbolOfSourceFile
    // handleGetSymbolOfSourceFile returns the module symbol for a source file, if any.
    // For non-module (script) files, returns nil.
    pub fn handle_get_symbol_of_source_file(
        &self,
        ctx: &Context,
        params: &GetSymbolOfSourceFileParams,
    ) -> Result<Option<SymbolResponse>, GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        let source_file = setup
            .program
            .get_source_file(&params.file.to_file_name())
            .map_or(Node::NIL, |f| f.root);
        if source_file.is_nil() {
            return Err(errors::errorf(
                format!(
                    "{}: source file not found: {}",
                    *ERR_CLIENT_ERROR,
                    params.file.string()
                ),
                vec![ERR_CLIENT_ERROR.clone()],
            ));
        }

        let symbol = setup
            .checker
            .borrow_mut()
            .get_symbol_at_location_exported(source_file);
        if symbol.is_nil() {
            return Ok(None);
        }
        Ok(setup.new_symbol_response(symbol))
    }

    // Go: api/session.go:2337 handleGetSymbolsOfSourceFiles
    // handleGetSymbolsOfSourceFiles returns the module symbols for multiple source files.
    pub fn handle_get_symbols_of_source_files(
        &self,
        ctx: &Context,
        params: &GetSymbolsOfSourceFilesParams,
    ) -> Result<Vec<Option<SymbolResponse>>, GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        let mut results: Vec<Option<SymbolResponse>> =
            (0..params.files.len()).map(|_| None).collect();
        for (i, file) in params.files.iter().enumerate() {
            let source_file = setup
                .program
                .get_source_file(&file.to_file_name())
                .map_or(Node::NIL, |f| f.root);
            if source_file.is_nil() {
                return Err(errors::errorf(
                    format!(
                        "{}: source file not found: {}",
                        *ERR_CLIENT_ERROR,
                        file.string()
                    ),
                    vec![ERR_CLIENT_ERROR.clone()],
                ));
            }
            let symbol = setup
                .checker
                .borrow_mut()
                .get_symbol_at_location_exported(source_file);
            if symbol.is_some() {
                results[i] = setup.new_symbol_response(symbol);
            }
        }
        Ok(results)
    }

    // Go: api/session.go:2359 handleGetSymbolsAtPositions
    // handleGetSymbolsAtPositions returns symbols at multiple positions in a file.
    pub fn handle_get_symbols_at_positions(
        &self,
        ctx: &Context,
        params: &GetSymbolsAtPositionsParams,
    ) -> Result<Vec<Option<SymbolResponse>>, GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        let source_file = setup
            .program
            .get_source_file(&params.file.to_file_name())
            .map_or(Node::NIL, |f| f.root);
        if source_file.is_nil() {
            return Err(errors::errorf(
                format!(
                    "{}: source file not found: {}",
                    *ERR_CLIENT_ERROR,
                    params.file.string()
                ),
                vec![ERR_CLIENT_ERROR.clone()],
            ));
        }

        let position_map = source_file_get_position_map(source_file);
        let mut results: Vec<Option<SymbolResponse>> =
            (0..params.positions.len()).map(|_| None).collect();
        for (i, &pos) in params.positions.iter().enumerate() {
            let node = astnav::get_touching_property_name(
                source_file,
                position_map.utf16_to_utf8(pos as i32),
            );
            if node.is_nil() {
                continue;
            }
            let symbol = setup
                .checker
                .borrow_mut()
                .get_symbol_at_location_exported(node);
            if symbol.is_some() {
                results[i] = setup.new_symbol_response(symbol);
            }
        }

        Ok(results)
    }

    // Go: api/session.go:2389 handleGetSymbolAtLocation
    // handleGetSymbolAtLocation returns the symbol at a node location.
    pub fn handle_get_symbol_at_location(
        &self,
        ctx: &Context,
        params: &GetSymbolAtLocationParams,
    ) -> Result<Option<SymbolResponse>, GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        let node = setup
            .sd
            .resolve_node_handle(&setup.program, &params.location)?;
        if node.is_nil() {
            return Ok(None);
        }

        let symbol = setup
            .checker
            .borrow_mut()
            .get_symbol_at_location_exported(node);
        if symbol.is_nil() {
            return Ok(None);
        }

        Ok(setup.new_symbol_response(symbol))
    }

    // Go: api/session.go:2413 handleGetSymbolsAtLocations
    // handleGetSymbolsAtLocations returns symbols at multiple node locations.
    pub fn handle_get_symbols_at_locations(
        &self,
        ctx: &Context,
        params: &GetSymbolsAtLocationsParams,
    ) -> Result<Vec<Option<SymbolResponse>>, GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        let mut results: Vec<Option<SymbolResponse>> =
            (0..params.locations.len()).map(|_| None).collect();
        for (i, loc) in params.locations.iter().enumerate() {
            let node = setup.sd.resolve_node_handle(&setup.program, loc)?;
            if node.is_nil() {
                continue;
            }
            let symbol = setup
                .checker
                .borrow_mut()
                .get_symbol_at_location_exported(node);
            if symbol.is_some() {
                results[i] = setup.new_symbol_response(symbol);
            }
        }

        Ok(results)
    }

    // Go: api/session.go:2439 handleGetTypeOfSymbol
    // handleGetTypeOfSymbol returns the type of a symbol.
    pub fn handle_get_type_of_symbol(
        &self,
        ctx: &Context,
        params: &GetTypeOfSymbolParams,
    ) -> Result<Option<TypeResponse>, GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        let (owner, symbol) = setup.resolve_symbol_handle(&params.symbol)?;
        let symbol = checker_symbol(&setup.checker, &owner, symbol);

        let t = setup
            .checker
            .borrow_mut()
            .get_type_of_symbol_exported(symbol);
        Ok(setup
            .sd
            .new_type_response(&setup.project_id, &setup.checker, t))
    }

    // Go: api/session.go:2455 handleGetTypesOfSymbols
    // handleGetTypesOfSymbols returns the types of multiple symbols.
    pub fn handle_get_types_of_symbols(
        &self,
        ctx: &Context,
        params: &GetTypesOfSymbolsParams,
    ) -> Result<Vec<Option<TypeResponse>>, GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        let mut results: Vec<Option<TypeResponse>> =
            (0..params.symbols.len()).map(|_| None).collect();
        for (i, symbol_reference) in params.symbols.iter().enumerate() {
            let (owner, symbol) = setup.resolve_symbol_handle(symbol_reference)?;
            let symbol = checker_symbol(&setup.checker, &owner, symbol);
            // resolveSymbolHandle errors on an unresolvable handle and GetTypeOfSymbol
            // never returns nil, so every element resolves to a type (error type at worst).
            let t = setup
                .checker
                .borrow_mut()
                .get_type_of_symbol_exported(symbol);
            results[i] = setup
                .sd
                .new_type_response(&setup.project_id, &setup.checker, t);
        }

        Ok(results)
    }

    // Go: api/session.go:2477 handleGetDeclaredTypeOfSymbol
    // handleGetDeclaredTypeOfSymbol returns the declared type of a symbol (e.g. the type alias body for type alias symbols).
    pub fn handle_get_declared_type_of_symbol(
        &self,
        ctx: &Context,
        params: &GetTypeOfSymbolParams,
    ) -> Result<Option<TypeResponse>, GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        let (owner, symbol) = setup.resolve_symbol_handle(&params.symbol)?;
        let symbol = checker_symbol(&setup.checker, &owner, symbol);

        let t = setup
            .checker
            .borrow_mut()
            .get_declared_type_of_symbol_exported(symbol);
        Ok(setup
            .sd
            .new_type_response(&setup.project_id, &setup.checker, t))
    }

    // Go: api/session.go handleGetNonMissingTypeOfSymbol (ts#63956)
    // handleGetNonMissingTypeOfSymbol returns the type of a symbol, excluding the missing type.
    pub fn handle_get_non_missing_type_of_symbol(
        &self,
        ctx: &Context,
        params: &GetTypeOfSymbolParams,
    ) -> Result<Option<TypeResponse>, GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        let (owner, symbol) = setup.resolve_symbol_handle(&params.symbol)?;
        let symbol = checker_symbol(&setup.checker, &owner, symbol);

        let t = setup
            .checker
            .borrow_mut()
            .get_non_missing_type_of_symbol_exported(symbol);
        Ok(setup
            .sd
            .new_type_response(&setup.project_id, &setup.checker, t))
    }

    // Go: api/session.go:2510 handleResolveName
    // handleResolveName resolves a name to a symbol at a given location.
    pub fn handle_resolve_name(
        &self,
        ctx: &Context,
        params: &ResolveNameParams,
    ) -> Result<Option<SymbolResponse>, GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        // Resolve location node - either from node handle or from fileName+position
        let location =
            setup.resolve_location(&params.location, params.file.as_ref(), params.position)?;

        let symbol = setup.checker.borrow_mut().resolve_name_exported(
            &params.name,
            location,
            SymbolFlags(params.meaning),
            params.exclude_globals,
        );
        if symbol.is_nil() {
            return Ok(None);
        }

        Ok(setup.new_symbol_response(symbol))
    }

    // Go: api/session.go:2532 handleGetSymbolsInScope
    // handleGetSymbolsInScope returns all symbols with the given meaning that are visible at a location.
    // PORT: Go builds the list from Go maps, so its order is random; the
    // port's order is stable (`Checker::get_symbols_in_scope`).
    pub fn handle_get_symbols_in_scope(
        &self,
        ctx: &Context,
        params: &GetSymbolsInScopeParams,
    ) -> Result<Vec<Option<SymbolResponse>>, GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        let location =
            setup.resolve_location(&params.location, params.file.as_ref(), params.position)?;
        if location.is_nil() {
            return Err(errors::errorf(
                format!(
                    "{}: getSymbolsInScope requires a location",
                    *ERR_CLIENT_ERROR
                ),
                vec![ERR_CLIENT_ERROR.clone()],
            ));
        }

        let symbols = setup
            .checker
            .borrow_mut()
            .get_symbols_in_scope_exported(location, SymbolFlags(params.meaning));
        let mut results = Vec::with_capacity(symbols.len());
        for symbol in symbols {
            results.push(setup.new_symbol_response(symbol));
        }

        Ok(results)
    }

    // Go: api/session.go:2557 handleGetSignaturesOfType
    // handleGetSignaturesOfType returns the call or construct signatures of a type.
    pub fn handle_get_signatures_of_type(
        &self,
        ctx: &Context,
        params: &GetSignaturesOfTypeParams,
    ) -> Result<Vec<Option<SignatureResponse>>, GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        let (owner, t) = setup.resolve_type_handle(params.type_)?;
        let t = checker_type(&setup.checker, &owner, t);

        let sigs = setup
            .checker
            .borrow_mut()
            .get_signatures_of_type_exported(t, SignatureKind(params.kind));
        let mut results = Vec::with_capacity(sigs.len());
        for sig in sigs {
            results.push(setup.new_signature_response(sig));
        }

        Ok(results)
    }

    // Go: api/session.go:2579 handleGetResolvedSignature
    // handleGetResolvedSignature returns the resolved signature of a call-like expression.
    pub fn handle_get_resolved_signature(
        &self,
        ctx: &Context,
        params: &GetResolvedSignatureParams,
    ) -> Result<Option<SignatureResponse>, GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        let node = setup
            .sd
            .resolve_node_handle(&setup.program, &params.location)?;

        let sig = setup
            .checker
            .borrow_mut()
            .get_resolved_signature_exported(node);
        Ok(setup.new_signature_response(sig))
    }

    // Go: api/session.go:2595 handleGetTypeAtLocation
    // handleGetTypeAtLocation returns the type at a node location.
    pub fn handle_get_type_at_location(
        &self,
        ctx: &Context,
        params: &GetTypeAtLocationParams,
    ) -> Result<Option<TypeResponse>, GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        let node = setup
            .sd
            .resolve_node_handle(&setup.program, &params.location)?;

        let t = setup.checker.borrow_mut().get_type_at_location(node);
        Ok(setup
            .sd
            .new_type_response(&setup.project_id, &setup.checker, t))
    }

    // Go: api/session.go:2611 handleGetTypeAtLocations
    // handleGetTypeAtLocations returns types at multiple node locations.
    pub fn handle_get_type_at_locations(
        &self,
        ctx: &Context,
        params: &GetTypeAtLocationsParams,
    ) -> Result<Vec<Option<TypeResponse>>, GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        let mut results: Vec<Option<TypeResponse>> =
            (0..params.locations.len()).map(|_| None).collect();
        for (i, loc) in params.locations.iter().enumerate() {
            let node = setup.sd.resolve_node_handle(&setup.program, loc)?;
            // resolveNodeHandle errors on an unresolvable handle and GetTypeAtLocation
            // never returns nil, so every element resolves to a type (error type at worst).
            let t = setup.checker.borrow_mut().get_type_at_location(node);
            results[i] = setup
                .sd
                .new_type_response(&setup.project_id, &setup.checker, t);
        }

        Ok(results)
    }

    // Go: api/session.go:2634 handleGetTypeAtPosition
    // handleGetTypeAtPosition returns the type at a position in a file.
    pub fn handle_get_type_at_position(
        &self,
        ctx: &Context,
        params: &GetTypeAtPositionParams,
    ) -> Result<Option<TypeResponse>, GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        let source_file = setup
            .program
            .get_source_file(&params.file.to_file_name())
            .map_or(Node::NIL, |f| f.root);
        if source_file.is_nil() {
            return Err(errors::errorf(
                format!(
                    "{}: source file not found: {}",
                    *ERR_CLIENT_ERROR,
                    params.file.string()
                ),
                vec![ERR_CLIENT_ERROR.clone()],
            ));
        }

        let position_map = source_file_get_position_map(source_file);
        let node = astnav::get_touching_property_name(
            source_file,
            position_map.utf16_to_utf8(params.position as i32),
        );
        if node.is_nil() {
            return Ok(None);
        }

        let t = setup.checker.borrow_mut().get_type_at_location(node);
        if t.is_nil() {
            return Ok(None);
        }

        Ok(setup
            .sd
            .new_type_response(&setup.project_id, &setup.checker, t))
    }

    // Go: api/session.go:2661 handleGetTypesAtPositions
    // handleGetTypesAtPositions returns types at multiple positions in a file.
    pub fn handle_get_types_at_positions(
        &self,
        ctx: &Context,
        params: &GetTypesAtPositionsParams,
    ) -> Result<Vec<Option<TypeResponse>>, GoError> {
        let setup = self.setup_checker(ctx, params.snapshot, &params.project)?;

        let source_file = setup
            .program
            .get_source_file(&params.file.to_file_name())
            .map_or(Node::NIL, |f| f.root);
        if source_file.is_nil() {
            return Err(errors::errorf(
                format!(
                    "{}: source file not found: {}",
                    *ERR_CLIENT_ERROR,
                    params.file.string()
                ),
                vec![ERR_CLIENT_ERROR.clone()],
            ));
        }

        let position_map = source_file_get_position_map(source_file);
        let mut results: Vec<Option<TypeResponse>> =
            (0..params.positions.len()).map(|_| None).collect();
        for (i, &pos) in params.positions.iter().enumerate() {
            let node = astnav::get_touching_property_name(
                source_file,
                position_map.utf16_to_utf8(pos as i32),
            );
            if node.is_nil() {
                continue;
            }
            let t = setup.checker.borrow_mut().get_type_at_location(node);
            if t.is_some() {
                results[i] = setup
                    .sd
                    .new_type_response(&setup.project_id, &setup.checker, t);
            }
        }

        Ok(results)
    }

    // Go: api/session.go:2690 handleGetParentOfSymbol
    pub fn handle_get_parent_of_symbol(
        &self,
        _ctx: &Context,
        params: &GetSymbolPropertyParams,
    ) -> Result<Option<SymbolResponse>, GoError> {
        self.resolve_symbol_property_of_symbol(params, &|symbols: &SymbolArena, sym: SymbolId| {
            symbols.sym(sym).parent
        })
    }

    // Go: api/session.go:2695 handleGetMembersOfSymbol
    pub fn handle_get_members_of_symbol(
        &self,
        ctx: &Context,
        params: &GetSymbolPropertyParams,
    ) -> Result<Vec<Option<SymbolResponse>>, GoError> {
        self.resolve_symbol_table_property_of_symbol(
            ctx,
            params,
            &|symbols: &SymbolArena, symbol: SymbolId| symbols.sym(symbol).members,
        )
    }

    // Go: api/session.go:2702 handleGetExportsOfSymbol
    pub fn handle_get_exports_of_symbol(
        &self,
        ctx: &Context,
        params: &GetSymbolPropertyParams,
    ) -> Result<Vec<Option<SymbolResponse>>, GoError> {
        self.resolve_symbol_table_property_of_symbol(
            ctx,
            params,
            &|symbols: &SymbolArena, symbol: SymbolId| symbols.sym(symbol).exports,
        )
    }

    // Go: api/session.go:2709 handleGetExportSymbolOfSymbol
    pub fn handle_get_export_symbol_of_symbol(
        &self,
        _ctx: &Context,
        params: &GetSymbolPropertyParams,
    ) -> Result<Option<SymbolResponse>, GoError> {
        self.resolve_symbol_property_of_symbol(params, &|symbols: &SymbolArena, sym: SymbolId| {
            symbols.sym(sym).export_symbol
        })
    }

    // Go: api/session.go:2714 handleGetSymbolOfType
    pub fn handle_get_symbol_of_type(
        &self,
        _ctx: &Context,
        params: &GetTypePropertyParams,
    ) -> Result<Option<SymbolResponse>, GoError> {
        self.resolve_symbol_property_of_type(params, &|c: &Checker, t: TypeId| c.ty(t).symbol())
    }

    // Go: api/session.go:2718 handleGetTargetOfType
    pub fn handle_get_target_of_type(
        &self,
        ctx: &Context,
        params: &GetTypePropertyParams,
    ) -> Result<Option<TypeResponse>, GoError> {
        self.resolve_type_property_of_type(ctx, params, &|c: &Checker, t: TypeId| c.ty(t).target())
    }

    // Go: api/session.go:2723 handleGetFreshTypeOfType
    pub fn handle_get_fresh_type_of_type(
        &self,
        ctx: &Context,
        params: &GetTypePropertyParams,
    ) -> Result<Option<TypeResponse>, GoError> {
        self.resolve_type_property_of_type(ctx, params, &|c: &Checker, t: TypeId| {
            c.ty(t).as_literal_type().fresh_type()
        })
    }

    // Go: api/session.go:2728 handleGetRegularTypeOfType
    pub fn handle_get_regular_type_of_type(
        &self,
        ctx: &Context,
        params: &GetTypePropertyParams,
    ) -> Result<Option<TypeResponse>, GoError> {
        self.resolve_type_property_of_type(ctx, params, &|c: &Checker, t: TypeId| {
            c.ty(t).as_literal_type().regular_type()
        })
    }

    // Go: api/session.go:2733 handleGetTypesOfType
    pub fn handle_get_types_of_type(
        &self,
        ctx: &Context,
        params: &GetTypePropertyParams,
    ) -> Result<Vec<Option<TypeResponse>>, GoError> {
        self.resolve_type_array_property_of_type(ctx, params, &|c: &Checker, t: TypeId| {
            c.ty(t).types().to_vec()
        })
    }

    // Go: api/session.go:2738 handleGetTypeParametersOfType
    pub fn handle_get_type_parameters_of_type(
        &self,
        ctx: &Context,
        params: &GetTypePropertyParams,
    ) -> Result<Vec<Option<TypeResponse>>, GoError> {
        self.resolve_type_array_property_of_type(ctx, params, &|c: &Checker, t: TypeId| {
            c.ty(t).as_interface_type().type_parameters().to_vec()
        })
    }

    // Go: api/session.go:2743 handleGetOuterTypeParametersOfType
    pub fn handle_get_outer_type_parameters_of_type(
        &self,
        ctx: &Context,
        params: &GetTypePropertyParams,
    ) -> Result<Vec<Option<TypeResponse>>, GoError> {
        self.resolve_type_array_property_of_type(ctx, params, &|c: &Checker, t: TypeId| {
            c.ty(t).as_interface_type().outer_type_parameters().to_vec()
        })
    }

    // Go: api/session.go:2748 handleGetLocalTypeParametersOfType
    pub fn handle_get_local_type_parameters_of_type(
        &self,
        ctx: &Context,
        params: &GetTypePropertyParams,
    ) -> Result<Vec<Option<TypeResponse>>, GoError> {
        self.resolve_type_array_property_of_type(ctx, params, &|c: &Checker, t: TypeId| {
            c.ty(t).as_interface_type().local_type_parameters().to_vec()
        })
    }

    // Go: api/session.go handleGetThisTypeOfType (ts#64264)
    // PORT: Go `t.AsInterfaceType().ThisType()` returns this field.
    pub fn handle_get_this_type_of_type(
        &self,
        ctx: &Context,
        params: &GetTypePropertyParams,
    ) -> Result<Option<TypeResponse>, GoError> {
        self.resolve_type_property_of_type(ctx, params, &|c: &Checker, t: TypeId| {
            c.ty(t).as_interface_type().this_type
        })
    }

    // Go: api/session.go:2758 handleGetAliasTypeArgumentsOfType
    pub fn handle_get_alias_type_arguments_of_type(
        &self,
        ctx: &Context,
        params: &GetTypePropertyParams,
    ) -> Result<Vec<Option<TypeResponse>>, GoError> {
        self.resolve_type_array_property_of_type(ctx, params, &|c: &Checker, t: TypeId| {
            let Some(alias) = c.ty(t).alias() else {
                return Vec::new();
            };
            alias.type_arguments().to_vec()
        })
    }

    // Go: api/session.go:2768 handleGetAliasSymbolOfType
    pub fn handle_get_alias_symbol_of_type(
        &self,
        _ctx: &Context,
        params: &GetTypePropertyParams,
    ) -> Result<Option<SymbolResponse>, GoError> {
        self.resolve_symbol_property_of_type(params, &|c: &Checker, t: TypeId| {
            let Some(alias) = c.ty(t).alias() else {
                return SymbolId::NIL;
            };
            alias.symbol()
        })
    }

    // Go: api/session.go:2777 handleGetObjectTypeOfType
    pub fn handle_get_object_type_of_type(
        &self,
        ctx: &Context,
        params: &GetTypePropertyParams,
    ) -> Result<Option<TypeResponse>, GoError> {
        self.resolve_type_property_of_type(ctx, params, &|c: &Checker, t: TypeId| {
            c.ty(t).as_indexed_access_type().object_type()
        })
    }

    // Go: api/session.go:2781 handleGetIndexTypeOfType
    pub fn handle_get_index_type_of_type(
        &self,
        ctx: &Context,
        params: &GetTypePropertyParams,
    ) -> Result<Option<TypeResponse>, GoError> {
        self.resolve_type_property_of_type(ctx, params, &|c: &Checker, t: TypeId| {
            c.ty(t).as_indexed_access_type().index_type()
        })
    }

    // Go: api/session.go:2785 handleGetCheckTypeOfType
    pub fn handle_get_check_type_of_type(
        &self,
        ctx: &Context,
        params: &GetTypePropertyParams,
    ) -> Result<Option<TypeResponse>, GoError> {
        self.resolve_type_property_of_type(ctx, params, &|c: &Checker, t: TypeId| {
            c.ty(t).as_conditional_type().check_type()
        })
    }

    // Go: api/session.go:2789 handleGetExtendsTypeOfType
    pub fn handle_get_extends_type_of_type(
        &self,
        ctx: &Context,
        params: &GetTypePropertyParams,
    ) -> Result<Option<TypeResponse>, GoError> {
        self.resolve_type_property_of_type(ctx, params, &|c: &Checker, t: TypeId| {
            c.ty(t).as_conditional_type().extends_type()
        })
    }

    // Go: api/session.go:2793 handleGetBaseTypeOfType
    pub fn handle_get_base_type_of_type(
        &self,
        ctx: &Context,
        params: &GetTypePropertyParams,
    ) -> Result<Option<TypeResponse>, GoError> {
        self.resolve_type_property_of_type(ctx, params, &|c: &Checker, t: TypeId| {
            c.ty(t).as_substitution_type().base_type()
        })
    }

    // Go: api/session.go:2799 handleGetConstraintOfType
    // handleGetConstraintOfType returns the constraint of a substitution type.
    // Type parameter constraints are handled by handleGetConstraintOfTypeParameter.
    pub fn handle_get_constraint_of_type(
        &self,
        ctx: &Context,
        params: &GetTypePropertyParams,
    ) -> Result<Option<TypeResponse>, GoError> {
        self.resolve_type_property_of_type(ctx, params, &|c: &Checker, t: TypeId| {
            c.ty(t).as_substitution_type().subst_constraint()
        })
    }

    // Go: api/session.go handleGetTypeParameterOfMappedType (ts#64397)
    // PORT: Go `t.AsMappedType().TypeParameter()` returns this field.
    pub fn handle_get_type_parameter_of_mapped_type(
        &self,
        ctx: &Context,
        params: &GetTypePropertyParams,
    ) -> Result<Option<TypeResponse>, GoError> {
        self.resolve_type_property_of_type(ctx, params, &|c: &Checker, t: TypeId| {
            c.ty(t).as_mapped_type().type_parameter
        })
    }

    // Go: api/session.go handleGetConstraintTypeOfMappedType (ts#64397)
    pub fn handle_get_constraint_type_of_mapped_type(
        &self,
        ctx: &Context,
        params: &GetTypePropertyParams,
    ) -> Result<Option<TypeResponse>, GoError> {
        self.resolve_type_property_of_type(ctx, params, &|c: &Checker, t: TypeId| {
            c.ty(t).as_mapped_type().constraint_type
        })
    }

    // Go: api/session.go handleGetNameTypeOfMappedType (ts#64397)
    pub fn handle_get_name_type_of_mapped_type(
        &self,
        ctx: &Context,
        params: &GetTypePropertyParams,
    ) -> Result<Option<TypeResponse>, GoError> {
        self.resolve_type_property_of_type(ctx, params, &|c: &Checker, t: TypeId| {
            c.ty(t).as_mapped_type().name_type
        })
    }

    // Go: api/session.go handleGetTemplateTypeOfMappedType (ts#64397)
    pub fn handle_get_template_type_of_mapped_type(
        &self,
        ctx: &Context,
        params: &GetTypePropertyParams,
    ) -> Result<Option<TypeResponse>, GoError> {
        self.resolve_type_property_of_type(ctx, params, &|c: &Checker, t: TypeId| {
            c.ty(t).as_mapped_type().template_type
        })
    }

    // Go: api/session.go:2821 handleGetTypeParametersOfSignature
    pub fn handle_get_type_parameters_of_signature(
        &self,
        ctx: &Context,
        params: &GetSignaturePropertyParams,
    ) -> Result<Vec<Option<TypeResponse>>, GoError> {
        self.resolve_type_array_property_of_signature(
            ctx,
            params,
            &|c: &Checker, sig: SignatureId| c.sig(sig).type_parameters().to_vec(),
        )
    }

    // Go: api/session.go:2826 handleGetParametersOfSignature
    pub fn handle_get_parameters_of_signature(
        &self,
        _ctx: &Context,
        params: &GetSignaturePropertyParams,
    ) -> Result<Vec<Option<SymbolResponse>>, GoError> {
        self.resolve_symbol_array_property_of_signature(params, &|c: &Checker, sig: SignatureId| {
            c.sig(sig).parameters().to_vec()
        })
    }

    // Go: api/session.go:2831 handleGetThisParameterOfSignature
    pub fn handle_get_this_parameter_of_signature(
        &self,
        _ctx: &Context,
        params: &GetSignaturePropertyParams,
    ) -> Result<Option<SymbolResponse>, GoError> {
        self.resolve_symbol_property_of_signature(params, &|c: &Checker, sig: SignatureId| {
            c.sig(sig).this_parameter()
        })
    }

    // Go: api/session.go:2836 handleGetTargetOfSignature
    pub fn handle_get_target_of_signature(
        &self,
        _ctx: &Context,
        params: &GetSignaturePropertyParams,
    ) -> Result<Option<SignatureResponse>, GoError> {
        self.resolve_signature_property_of_signature(params, &|c: &Checker, sig: SignatureId| {
            c.sig(sig).target()
        })
    }

    // Go: api/session.go:2840 handleGetImportAdderEdits (tsgo#3881, tsgo#4712)
    // PORT: Go returns `[]*TextEdit`, and `toAPITextEdits` can return nil
    // (JSON `null`); nil is `None`.
    // PORT: Go `defer preparedSnapshot.Deref(s.projectSession)` is the
    // `Release` guard `_deref_prepared`. It is declared before the checker
    // lease, so the lease (`defer done()`) ends first, as in Go. Go passes
    // `ch` to `NewImportAdder`; the port's adder takes the checker in
    // `AddImportFromExportedSymbol` (import_adder.rs header). A symbol handle
    // indexes the arena of the checker that made it, so `checker_symbol` maps
    // it into `ch` (file header).
    pub fn handle_get_import_adder_edits(
        &self,
        ctx: &Context,
        params: &GetImportAdderEditsParams,
    ) -> Result<Option<Vec<TextEdit>>, GoError> {
        let sd = self.get_snapshot_data(params.snapshot)?;

        // ts#64319: the project ID is used as is.
        let project_id = params.project.clone();
        let mut working_snapshot = sd.snapshot.clone();
        let mut program = sd.get_program(&params.project)?;
        let mut source_file = program
            .get_source_file(&params.file.to_file_name())
            .map_or(Node::NIL, |f| f.root);
        if source_file.is_nil() {
            return Err(errors::errorf(
                format!(
                    "{}: source file not found: {}",
                    *ERR_CLIENT_ERROR,
                    params.file.string()
                ),
                vec![ERR_CLIENT_ERROR.clone()],
            ));
        }

        let mut user_preferences = working_snapshot.user_preferences();
        let mut _deref_prepared = ls_program::Release::noop();
        let registry = working_snapshot.auto_import_registry();
        if registry.is_none()
            || !autoimport::Registry::is_prepared_for_importing_file(
                registry.as_deref(),
                source_file_file_name(source_file),
                &autoimport::ProjectID(project_id.0.clone()),
                &user_preferences,
            )
        {
            // ts#64163
            let prepared_snapshot = self.snapshot_host.clone_snapshot_with_auto_imports(
                ctx,
                &working_snapshot,
                &params.file.to_uri(&self.get_current_directory()),
                None,
            );
            if let Some(project_session) = &self.project_session {
                project_session
                    .try_adopt_snapshot_in_background(&working_snapshot, &prepared_snapshot);
            }
            _deref_prepared = {
                let snapshot = prepared_snapshot.clone();
                ls_program::Release::new(move || project::Snapshot::deref(&snapshot))
            };

            working_snapshot = prepared_snapshot;
            let proj = working_snapshot.project_collection.get_project(&project_id);
            let Some(proj) = proj else {
                return Err(errors::errorf(
                    format!("{}: project {} not found", *ERR_CLIENT_ERROR, project_id.0),
                    vec![ERR_CLIENT_ERROR.clone()],
                ));
            };
            let proj_program = proj.borrow().get_program();
            let Some(proj_program) = proj_program else {
                return Err(errors::errorf(
                    format!("{}: project has no program", *ERR_CLIENT_ERROR),
                    vec![ERR_CLIENT_ERROR.clone()],
                ));
            };
            program = proj_program;
            source_file = program
                .get_source_file(&params.file.to_file_name())
                .map_or(Node::NIL, |f| f.root);
            if source_file.is_nil() {
                return Err(errors::errorf(
                    format!(
                        "{}: source file not found: {}",
                        *ERR_CLIENT_ERROR,
                        params.file.string()
                    ),
                    vec![ERR_CLIENT_ERROR.clone()],
                ));
            }
            user_preferences = working_snapshot.user_preferences();
        }

        let registry = working_snapshot.auto_import_registry();
        let Some(registry) = registry else {
            return Ok(Some(Vec::new()));
        };

        let (ch, _done) = ls_program::get_type_checker(&program, ctx);

        // ts#64178: Go passes `ch` to NewView; the Rust view keeps no
        // checker (its methods take the checker from their callers).
        let view = autoimport::new_view(
            registry,
            source_file,
            autoimport::ProjectID(project_id.0.clone()),
            program.clone(),
            user_preferences.module_specifier_preferences(),
        );
        let mut import_adder = autoimport::new_import_adder(
            ctx,
            &program,
            source_file,
            Rc::new(view),
            working_snapshot
                .get_preferences(source_file_file_name(source_file))
                .format_code_settings,
            working_snapshot.converters(),
            user_preferences,
        );

        for (i, action) in params.actions.iter().enumerate() {
            match action.kind.0.as_str() {
                IMPORT_ADDER_ACTION_KIND_IMPORT_SYMBOL => {
                    // ts#64518
                    let Some(action_symbol) = &action.symbol else {
                        return Err(errors::errorf(
                            format!(
                                "{}: import adder action {} missing symbol",
                                *ERR_CLIENT_ERROR, i
                            ),
                            vec![ERR_CLIENT_ERROR.clone()],
                        ));
                    };
                    let (owner, symbol) = resolve_symbol_reference_for_checker(
                        &sd,
                        params.snapshot,
                        &program,
                        &ch,
                        action_symbol,
                    )?;
                    let symbol = checker_symbol(&ch, &owner, symbol);
                    let mut is_valid_type_only_use_site = true;
                    if let Some(value) = action.is_valid_type_only_use_site {
                        is_valid_type_only_use_site = value;
                    }
                    import_adder.add_import_from_exported_symbol(
                        &mut ch.borrow_mut(),
                        symbol,
                        is_valid_type_only_use_site,
                    );
                }
                _ => {
                    return Err(errors::errorf(
                        format!(
                            "{}: unknown import adder action kind {}",
                            *ERR_CLIENT_ERROR,
                            gostd::strconv::quote(&action.kind.0)
                        ),
                        vec![ERR_CLIENT_ERROR.clone()],
                    ));
                }
            }
        }

        if !import_adder.has_fixes() {
            return Ok(Some(Vec::new()));
        }
        Ok(to_api_text_edits(source_file, &import_adder.edits()))
    }
}

// Go: api/session.go:2935 toAPITextEdits (tsgo#3881, tsgo#4712)
// PORT: Go returns nil when an edit position is outside the original text;
// nil is `None`.
pub fn to_api_text_edits(source_file: Node, edits: &[lsproto::TextEdit]) -> Option<Vec<TextEdit>> {
    let original_text = source_file_original_text(source_file);
    let line_map = lsconv::compute_lsp_line_starts(&original_text);
    let position_map = compute_position_map(&original_text);
    let mut result = Vec::with_capacity(edits.len());
    for edit in edits {
        let (start, ok) = original_text_offset(&line_map, &edit.range.start, &original_text);
        if !ok {
            return None;
        }
        let (end, ok) = original_text_offset(&line_map, &edit.range.end, &original_text);
        if !ok {
            return None;
        }
        result.push(TextEdit {
            pos: position_map.utf8_to_utf16(start),
            end: position_map.utf8_to_utf16(end),
            new_text: edit.new_text.clone(),
        });
    }
    Some(result)
}

// Go: api/session.go:2958 originalTextOffset (tsgo#4712)
// PORT: Go takes `len(originalText)`; the port takes the text. The API
// session uses the UTF-8 encoding, so `position.character` counts Go bytes
// after the line start. Port offsets differ from Go offsets after a marker
// unit (see `scanner_util::GO_STRING_MARKER`), so the Go bytes after the
// line start give the port offset (`port_byte_offset`), as
// `Converters::line_and_character_to_position` does. Go does not stop at the
// line end, and `port_byte_offset` counts the bytes past the end, so the
// offset is past the port end exactly when Go's is past the Go end. A
// unit has at least as many port bytes as Go bytes, so a `character` past
// the port bytes after the line start is past the end without a scan (and
// fits no `i32`). Go `int` arithmetic is `i64` here; the offset is at most
// the text length, so it fits the `i32` that `PositionMap` takes.
pub fn original_text_offset(
    line_map: &lsconv::LSPLineMap,
    position: &lsproto::Position,
    text: &str,
) -> (i32, bool) {
    let line = i64::from(position.line);
    if line < 0 || line >= line_map.line_starts.len() as i64 {
        return (0, false);
    }
    let line_start = line_map.line_starts[line as usize] as usize;
    let rest = &text[line_start..];
    if i64::from(position.character) > rest.len() as i64 {
        return (0, false);
    }
    let offset = line_start as i64
        + i64::from(crate::scanner_util::port_byte_offset(
            rest,
            position.character as i32,
        ));
    if offset < line_start as i64 || offset > text.len() as i64 {
        return (0, false);
    }
    (offset as i32, true)
}

// Go: api/session_textedit_test.go (tsgo#4712)
#[cfg(test)]
mod textedit_tests {
    use super::*;
    use crate::frontend::parser::{self, SourceFileParseOptions};

    // Go: api/session_textedit_test.go:14 TestToAPITextEditsUsesOriginalCoordinates
    // PORT: Go sets the info on the parsed `*ast.SourceFile`. Here the parse
    // is recorded (`program::note_parsed_source_file`) and the info is set on
    // the `ParsedSourceFile`, as the content mapper transform does.
    #[test]
    fn test_to_api_text_edits_uses_original_coordinates() {
        let source_file = Rc::new(parser::parse_source_file(
            &SourceFileParseOptions {
                file_name: "/app.vue".to_string(),
                path: tspath::Path("/app.vue".to_string()),
                ..Default::default()
            },
            "const transformed = true;",
            ScriptKind::TS,
        ));
        crate::program::note_parsed_source_file(&source_file);
        source_file.set_content_mapper_info(ContentMapperSourceFileInfo {
            original_text: "😀\nabc".to_string(),
            content_mapper: "mapper".to_string(),
            ..Default::default()
        });

        let edits = to_api_text_edits(
            source_file.root,
            &[lsproto::TextEdit {
                range: lsproto::Range {
                    start: lsproto::Position {
                        line: 1,
                        character: 1,
                    },
                    end: lsproto::Position {
                        line: 1,
                        character: 2,
                    },
                },
                new_text: "x".to_string(),
            }],
        );

        assert_eq!(
            edits,
            Some(vec![TextEdit {
                pos: 4,
                end: 5,
                new_text: "x".to_string(),
            }])
        );
    }

    // PORT: no Go test. The characters count Go bytes; marker units (an
    // invalid byte, a WTF-8 lone surrogate, a real U+FDD0, see
    // `scanner_util::GO_STRING_MARKER`) have more port bytes than Go bytes.
    // Go reads `ab\xe9cd` on line 1 at Go offset 2: character 3 is `c` (Go
    // offset 5, UTF-16 offset 5).
    #[test]
    fn to_api_text_edits_counts_go_bytes() {
        let edit = |line: u32, start: u32, end: u32| lsproto::TextEdit {
            range: lsproto::Range {
                start: lsproto::Position {
                    line,
                    character: start,
                },
                end: lsproto::Position {
                    line,
                    character: end,
                },
            },
            new_text: "x".to_string(),
        };
        // (Go bytes, line, start, end, UTF-16 pos and end; None past the end)
        let cases: [(&[u8], u32, u32, u32, Option<(i32, i32)>); 6] = [
            (b"\xe9\nab\xe9cd", 1, 3, 4, Some((5, 6))),
            (b"\xe9\nab\xe9cd", 1, 5, 5, Some((7, 7))),
            (b"\xe9\nab\xe9cd", 1, 6, 6, None),
            // A WTF-8 lone surrogate is 3 Go bytes and one UTF-16 unit (Go
            // `DecodeJSStringRune` in `ComputePositionMap`).
            (b"a\xed\xa0\x80b\r\nc", 0, 4, 5, Some((2, 3))),
            // A real U+FDD0 is 3 Go bytes and one UTF-16 unit.
            ("\u{FDD0}\u{FDD0}x\ny".as_bytes(), 0, 6, 7, Some((2, 3))),
            ("\u{FDD0}\u{FDD0}x\ny".as_bytes(), 1, 0, 1, Some((4, 5))),
        ];
        for (bytes, line, start, end, want) in cases {
            let original_text = crate::scanner_util::go_string_from_bytes(bytes.to_vec());
            let source_file = Rc::new(parser::parse_source_file(
                &SourceFileParseOptions {
                    file_name: "/app.vue".to_string(),
                    path: tspath::Path("/app.vue".to_string()),
                    ..Default::default()
                },
                "const transformed = true;",
                ScriptKind::TS,
            ));
            crate::program::note_parsed_source_file(&source_file);
            source_file.set_content_mapper_info(ContentMapperSourceFileInfo {
                original_text,
                content_mapper: "mapper".to_string(),
                ..Default::default()
            });
            let edits = to_api_text_edits(source_file.root, &[edit(line, start, end)]);
            let got = edits.map(|edits| (edits[0].pos, edits[0].end));
            assert_eq!(got, want, "{bytes:?} line {line} {start}..{end}");
        }
    }
}
