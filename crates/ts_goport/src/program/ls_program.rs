//! Go `*compiler.Program` checker and diagnostics methods for the language
//! service, plus the Go compiler checker pool (`compiler/checkerpool.go`).
//!
//! Everything here runs on the LSP dispatch thread. Go `*compiler.Program`
//! is `Rc<NewProgram>` where it is stored and `&NewProgram` where code only
//! reads it. A holder of the `Rc` keeps the program alive, as a Go pointer
//! does. A Go program method `p.X(..)` that the language service needs is
//! `ls_program::x(p, ..)`.
//!
//! Programs: each `NewProgram` made by `new_program` or `update_program` is
//! also a program version of the process (`program::new_program_version`),
//! as Go makes a new `Program` for each snapshot change. Versions share the
//! file versions they have in common. The checkers of every version are
//! made here, on the dispatch thread.
//!
//! Current program: the checker and the `program.rs` functions that it
//! calls read `prog()`. `enter` makes a program current while a
//! `ProgramGuard` lives. A language service holds a guard for its program,
//! a checker from `get_type_checker*` holds one until its release, and the
//! functions here enter the program they are given. Guards can drop in any
//! order: the current program is the one of the last guard that is still
//! alive.
//!
//! Release: Go frees a program when no snapshot and no request uses it.
//! `release_program` is the snapshot part (Go `programCounter.Deref`); the
//! release waits until no guard and no hold (`hold_program`) of the program
//! is alive. It frees the version's program tables, and the registry drops
//! its `Rc`, so the `NewProgram` is freed with its last holder. On the
//! dispatch thread of the LSP server both frees wait until the answer is
//! sent (`gostd::local::drop_later`). The `GoProgram` shell and the
//! file versions stay leaked (see `program::release_program`). A compiler
//! host whose last live program is released drops its data
//! (`CompilerHost::release`), even when a stale holder keeps the program.
//!
//! PORT: Go keeps the checker pools in `Program` fields. `NewProgram` does
//! not have them, so they live in a thread-local registry keyed by the
//! program address (`ProgramCheckers`).
//!
//! The compile path (`program::with_type_checker_for_file`, the worker
//! pool) is separate and does not change.

use super::*;
use crate::emitter::emitter::{EmitOnly, Emitter};
use crate::emitter::program_emit::{EmitOptions, EmitResult, combine_emit_results};
use crate::frontend::compiler::{CompilerHost, NewProgram, ProgramOptions};
use crate::frontend::module;
use crate::frontend::outputpaths::ForceEmitPaths;
use crate::frontend::parser::ParsedSourceFile;
use crate::frontend::tspath;
use crate::gostd::Context;
use std::cell::{Cell, OnceCell};

// ---------------------------------------------------------------------------
// Release, CheckerPool
// ---------------------------------------------------------------------------

/// Go `func()` that releases a checker (the `done` of
/// `GetTypeCheckerForFile`). It runs once: on `call` or on drop, whichever
/// comes first.
// PORT: Go wraps release functions in `sync.OnceFunc`.
pub struct Release(Option<Box<dyn FnOnce()>>);

impl Release {
    pub fn new(f: impl FnOnce() + 'static) -> Release {
        Release(Some(Box::new(f)))
    }

    // Go: compiler/checkerpool.go:495 noop
    pub fn noop() -> Release {
        Release(None)
    }

    pub fn call(mut self) {
        if let Some(f) = self.0.take() {
            f();
        }
    }
}

impl Drop for Release {
    fn drop(&mut self) {
        if let Some(f) = self.0.take() {
            f();
        }
    }
}

// Go: compiler/checkerpool.go:24 CheckerPool
// CheckerPool is implemented by the project system to provide checkers with
// request-scoped lifetime and reclamation. It returns a checker and a release
// function that must be called when the caller is done with the checker.
// The returned checker must not be accessed concurrently; each acquisition is exclusive.
// Acquisitions are not reentrant, even when they share a request ID. Callers must
// pass an already acquired checker to nested operations instead of acquiring again.
// If file is non-nil, the pool may use it as an affinity hint to return the same
// checker for the same file across calls.
// PORT: `file` is `Node::NIL` for Go nil. The caller borrows the checker
// (`borrow_mut`) while it uses it.
pub trait CheckerPool {
    fn get_checker(&self, ctx: &Context, file: Node) -> (Rc<RefCell<Checker>>, Release);
}

/// Go `ProgramOptions.CreateCheckerPool`.
pub type CreateCheckerPool = Rc<dyn Fn(&Rc<NewProgram>) -> Rc<dyn CheckerPool>>;

// ---------------------------------------------------------------------------
// Program registry
// ---------------------------------------------------------------------------

/// The Go `Program` fields that `NewProgram` does not hold.
struct ProgramCheckers {
    /// The frontend program.
    program: Rc<NewProgram>,
    /// Its program version.
    version: &'static GoProgram,
    /// Go `Program.checkerPool`.
    checker_pool: Rc<dyn CheckerPool>,
    /// Go `Program.compilerCheckerPool`.
    compiler_checker_pool: Option<Rc<CompilerCheckerPool>>,
    /// Go `Program.declarationDiagnosticCache`.
    declaration_diagnostic_cache: RefCell<FxHashMap<Node, Vec<Diagnostic>>>,
    /// The host of the load that made the files of `program`: its own host
    /// after `new_program` or an update that loads again, and the old
    /// program's load host after an update that clones. The processed
    /// files keep the resolver of the load, which reads that host's file
    /// system after the load too (`GetPackageScopeForPath`).
    // PORT: since ts#64519 Go keeps no resolver: `Program.newResolver`
    // reads the program's own host. The module part of ts#64519
    // (`module.ResolutionData`) is not ported yet, so the port keeps the
    // load host until then.
    load_host: Rc<dyn CompilerHost>,
}

thread_local! {
    // Go: checker/checker.go:583 nextCheckerID
    // PORT: every language-service checker is made on the dispatch thread,
    // so the Go atomic is a thread-local counter.
    static NEXT_CHECKER_ID: Cell<u32> = const { Cell::new(0) };

    /// The checker pools of each program made by `new_program` or
    /// `update_program`, by program address. `release_program` removes an
    /// entry. An entry holds its program, so its address is not reused
    /// while the entry exists.
    static PROGRAM_CHECKERS: RefCell<FxHashMap<usize, Rc<ProgramCheckers>>> =
        RefCell::new(FxHashMap::default());

    /// The live guards of this thread, oldest first: the guard token and
    /// its program version.
    static GUARDS: RefCell<Vec<(u64, &'static GoProgram)>> = const { RefCell::new(Vec::new()) };

    /// The token of the next guard.
    static NEXT_GUARD: Cell<u64> = const { Cell::new(0) };

    /// The current program of this thread before its first live guard; it
    /// is current again when the last guard drops.
    static BEFORE_GUARDS: Cell<Option<&'static GoProgram>> = const { Cell::new(None) };

    /// Programs that `release_program` released while a guard or a hold of
    /// theirs was alive: the registry key, by program version id. The last
    /// such guard or hold releases them.
    static RELEASE_PENDING: RefCell<FxHashMap<u32, usize>> =
        RefCell::new(FxHashMap::default());

    /// The number of live `ProgramHold`s of each program version, by program
    /// version id.
    static HOLDS: RefCell<FxHashMap<u32, usize>> = RefCell::new(FxHashMap::default());

    /// The number of programs in `PROGRAM_CHECKERS` that use each compiler
    /// host, by host address (`host_key`). A program uses its own host and
    /// its load host (`ProgramCheckers::load_host`).
    // PORT: not in Go. Go's GC frees a host when no program references it.
    // Here a stale holder (a pool or a request) can keep a released program
    // and its host, so the live users are counted.
    static HOST_USERS: RefCell<FxHashMap<usize, u32>> = RefCell::new(FxHashMap::default());
}

/// Registry key of `p`.
fn program_key(p: &NewProgram) -> usize {
    std::ptr::from_ref(p).addr()
}

/// `HOST_USERS` key of `host`.
fn host_key(host: &Rc<dyn CompilerHost>) -> usize {
    Rc::as_ptr(host).cast::<()>().addr()
}

/// The hosts that a registered program uses: its own host, then its load
/// host when that is another host.
fn used_hosts(checkers: &ProgramCheckers) -> Vec<Rc<dyn CompilerHost>> {
    let own = checkers.program.host();
    let mut hosts = vec![own.clone()];
    if !Rc::ptr_eq(own, &checkers.load_host) {
        hosts.push(checkers.load_host.clone());
    }
    hosts
}

/// Counts the live programs that use each host of `checkers`.
fn hold_hosts(checkers: &ProgramCheckers) {
    HOST_USERS.with(|users| {
        let mut users = users.borrow_mut();
        for host in used_hosts(checkers) {
            *users.entry(host_key(&host)).or_insert(0) += 1;
        }
    });
}

/// Undoes `hold_hosts`, then releases each host that no live program uses
/// now (Go: the GC frees the host with its last program). When that is the
/// load host, no live program shares the processed files of the load
/// either, so their resolver caches go too.
fn release_hosts(checkers: &ProgramCheckers) {
    let unused: Vec<Rc<dyn CompilerHost>> = HOST_USERS.with(|users| {
        let mut users = users.borrow_mut();
        used_hosts(checkers)
            .into_iter()
            .filter(|host| {
                let key = host_key(host);
                let count = users
                    .get_mut(&key)
                    .expect("a registered program holds its hosts");
                *count -= 1;
                if *count == 0 {
                    users.remove(&key);
                    return true;
                }
                false
            })
            .collect()
    });
    for host in unused {
        if Rc::ptr_eq(&host, &checkers.load_host) {
            checkers.program.release_resolver_caches();
        }
        host.release();
    }
}

/// The registry entry of `p`. Panics for a program that `new_program` or
/// `update_program` did not make, or that is released.
fn program_checkers(p: &NewProgram) -> Rc<ProgramCheckers> {
    PROGRAM_CHECKERS
        .with(|programs| programs.borrow().get(&program_key(p)).cloned())
        .expect("program was not made by ls_program::new_program, or it is released")
}

/// The program version of `p`.
pub fn program_version(p: &NewProgram) -> &'static GoProgram {
    program_checkers(p).version
}

/// Not in Go: true when the tables of `version` started from those of the
/// version it was updated from, because its frontend program replaced files
/// of that one in place (Go `ReuseProgram`, editfast1); false when they were
/// built from its files alone. Tests pin the path with it.
pub fn version_tables_reused(version: &'static GoProgram) -> bool {
    held_tables(version)
        .go
        .as_ref()
        .is_some_and(|go| go.from_old_tables)
}

/// The parsed file whose root is `file`, from any program made here that
/// is not released, or None. Go `*ast.SourceFile` is one object in every
/// program that has it; here the programs hold the `ParsedSourceFile`.
/// A file that is not published yet (Go `parser.ParseSourceFile` outside a
/// program) has the parse that `program::note_parsed_source_file` recorded
/// on this thread, and so does such a file after a publish that gave it no
/// program.
pub fn parsed_source_file(file: Node) -> Option<Rc<ParsedSourceFile>> {
    if !crate::ast::is_published(file.file_index()) {
        return super::go_frontend::unpublished_parsed_source_file(file.file_index())
            .filter(|parsed| parsed.root == file);
    }
    program_parsed_source_file(file)
        // PORT: bump B 2c config hand-off (api ext battery,
        // `getConfigSourceFile` event 3): the root config that the project
        // system parsed (Go `tsoptions.NewTsconfigSourceFileFromFilePath`).
        .or_else(|| {
            super::go_frontend::published_outside_parsed_source_file(file.file_index())
                .filter(|parsed| parsed.root == file)
        })
}

/// `parsed_source_file` for a file of a program made here that is not
/// released: None for a file that no such program has, also when this
/// thread has its parse from outside a program.
// PORT: Go sets `SourceFile.Hash` only for the files of the language
// server programs (project/parsecache.go `NewParseCache`,
// project/compilerhost.go `GetContentMappedSourceFiles`). The api encoder
// (`source_file_content_hash`) uses this to write Hash 0 for other parses.
pub fn program_parsed_source_file(file: Node) -> Option<Rc<ParsedSourceFile>> {
    let go_file = crate::ast::try_go_file(file.file_index())?;
    let path = tspath::Path(go_file.info.path.clone());
    let programs: Vec<Rc<NewProgram>> = PROGRAM_CHECKERS.with(|programs| {
        programs
            .borrow()
            .values()
            .map(|checkers| Rc::clone(&checkers.program))
            .collect()
    });
    programs.into_iter().find_map(|p| {
        p.get_source_file_by_path(&path)
            .filter(|parsed| parsed.root == file)
    })
}

// ---------------------------------------------------------------------------
// Current program
// ---------------------------------------------------------------------------

/// From `enter`: `p` is current on this thread while the guard lives, unless
/// a later guard that is still alive made another program current. It is
/// `!Send`, so it drops on the thread that made it.
#[must_use = "the program is current only while the guard lives"]
pub struct ProgramGuard {
    token: u64,
    version: &'static GoProgram,
    _not_send: std::marker::PhantomData<*const ()>,
}

/// Makes `p` the current program of this thread while the guard lives.
pub fn enter(p: &NewProgram) -> ProgramGuard {
    enter_version(program_version(p))
}

/// `enter` for a program version. Use it for a version that no `NewProgram`
/// makes (`program::new_alias_resolver_program`).
pub fn enter_version(version: &'static GoProgram) -> ProgramGuard {
    let token = NEXT_GUARD.with(|next| {
        let token = next.get();
        next.set(token + 1);
        token
    });
    GUARDS.with(|guards| {
        let mut guards = guards.borrow_mut();
        if guards.is_empty() {
            BEFORE_GUARDS.with(|before| before.set(try_prog()));
        }
        guards.push((token, version));
    });
    crate::core::set_thread_program(Some(version));
    ProgramGuard {
        token,
        version,
        _not_send: std::marker::PhantomData,
    }
}

impl ProgramGuard {
    /// A `Release` that drops this guard after `release` runs: the checker
    /// that `release` gives back runs with the program current until then.
    pub fn with_release(self, release: Release) -> Release {
        Release::new(move || {
            release.call();
            drop(self);
        })
    }
}

impl Drop for ProgramGuard {
    fn drop(&mut self) {
        let (current, still_entered) = GUARDS.with(|guards| {
            let mut guards = guards.borrow_mut();
            if let Some(i) = guards.iter().position(|&(token, _)| token == self.token) {
                guards.remove(i);
            }
            let current = match guards.last() {
                Some(&(_, version)) => Some(version),
                None => BEFORE_GUARDS.with(Cell::get),
            };
            let still_entered = guards
                .iter()
                .any(|&(_, version)| std::ptr::eq(version, self.version));
            (current, still_entered)
        });
        crate::core::set_thread_program(current);
        if !still_entered && !is_held(self.version) {
            release_if_pending(self.version);
        }
    }
}

/// Not in Go: from `hold_program`. While it lives, `release_program` of its
/// program waits, as for a live `ProgramGuard`, but the program is not
/// current. It is `!Send`, so it drops on the thread that made it.
// PORT: a Go background task that has a pointer to a snapshot reads its
// programs also after the next snapshot change disposed it (the GC keeps
// them). The snapshot change task takes a hold of each program of its new
// snapshot instead (`project::Session::update_snapshot`).
#[must_use = "the program stays readable only while the hold lives"]
pub struct ProgramHold {
    version: &'static GoProgram,
    _not_send: std::marker::PhantomData<*const ()>,
}

/// Holds `p` registered, so its reads here work after `release_program` of
/// it, until the hold drops. None for a program that is not registered (not
/// made here, or released).
pub fn hold_program(p: &NewProgram) -> Option<ProgramHold> {
    let checkers =
        PROGRAM_CHECKERS.with(|programs| programs.borrow().get(&program_key(p)).cloned())?;
    let version = checkers.version;
    HOLDS.with(|holds| *holds.borrow_mut().entry(version.id).or_insert(0) += 1);
    Some(ProgramHold {
        version,
        _not_send: std::marker::PhantomData,
    })
}

impl Drop for ProgramHold {
    fn drop(&mut self) {
        // A hold of a queued task that never ran drops with the queues of
        // its thread when the thread ends; the registry may be gone then, and
        // the thread's programs go with it.
        if crate::gostd::local::is_ending() {
            return;
        }
        let last = HOLDS.with(|holds| {
            let mut holds = holds.borrow_mut();
            let count = holds
                .get_mut(&self.version.id)
                .expect("a live hold is counted");
            *count -= 1;
            let last = *count == 0;
            if last {
                holds.remove(&self.version.id);
            }
            last
        });
        if last && !is_entered(self.version) {
            release_if_pending(self.version);
        }
    }
}

/// True while a hold of `version` is alive on this thread.
fn is_held(version: &'static GoProgram) -> bool {
    HOLDS.with(|holds| holds.borrow().contains_key(&version.id))
}

/// True while a guard of `version` is alive on this thread.
fn is_entered(version: &'static GoProgram) -> bool {
    GUARDS.with(|guards| {
        guards
            .borrow()
            .iter()
            .any(|&(_, entered)| std::ptr::eq(entered, version))
    })
}

/// Releases `version` now when `release_program` released it while a
/// guard or a hold of it was alive. The caller dropped the last of them.
fn release_if_pending(version: &'static GoProgram) {
    let pending = RELEASE_PENDING.with(|pending| pending.borrow_mut().remove(&version.id));
    if let Some(key) = pending {
        release_now(key);
    }
}

// ---------------------------------------------------------------------------
// Program construction and release
// ---------------------------------------------------------------------------

// Go: compiler/program.go:313 NewProgram
// PORT: Go `opts.CreateCheckerPool` is the `create_checker_pool` argument.
// The program does not keep it (ts#64519).
// The frontend program parses with no current program, then becomes a
// program version of the process (`program::new_program_version`).
// PORT: Go runs `initCheckerPool` before `verifyCompilerOptions`. Here the
// pool is set up after the program is built. Neither step reads the other.
// #4712: Go `collectContentMapperOptionDiagnostics` runs when the program
// version is built (`go_frontend::content_mapper_option_diagnostics_of`).
pub fn new_program(
    opts: ProgramOptions,
    create_checker_pool: Option<CreateCheckerPool>,
) -> Rc<NewProgram> {
    let p = {
        let _scope = crate::core::enter_program(None);
        Rc::new(crate::frontend::compiler::new_program(opts))
    };
    let version = new_program_version(&p, None);
    init_checker_pool(&p, version, create_checker_pool, p.host().clone());
    p
}

// Go: compiler/program.go:335 UpdateProgram
// PORT: `NewProgram::update_program` builds the new program. It
// becomes a program version that shares the unchanged file versions of
// `p`, and gets its checker pool here. Since ts#64519 the new program uses
// `create_checker_pool` as given (`None` is the compiler pool), not the one
// of `p`. `create_module_resolver` goes to `NewProgram::update_program`; a
// Go nil `createModuleResolver` is `None` (ts#64299).
pub fn update_program(
    p: &NewProgram,
    changed_file_path: &tspath::Path,
    new_host: Rc<dyn CompilerHost>,
    create_checker_pool: Option<CreateCheckerPool>,
    create_module_resolver: Option<Rc<dyn Fn(module::ResolverOptions) -> Rc<dyn module::Resolver>>>,
) -> (Rc<NewProgram>, Option<Rc<ParsedSourceFile>>, bool) {
    let old = PROGRAM_CHECKERS.with(|programs| programs.borrow().get(&program_key(p)).cloned());
    let (result, new_file, reused) = {
        let _scope = crate::core::enter_program(None);
        p.update_program(changed_file_path, new_host, create_module_resolver)
    };
    let result = Rc::new(result);
    // A clone shares the old program's processed files (Go `UpdateProgram`),
    // so it uses the old load host too.
    let load_host = if reused {
        old.as_ref()
            .map_or_else(|| p.host().clone(), |old| old.load_host.clone())
    } else {
        result.host().clone()
    };
    let version = new_program_version(&result, old.map(|old| old.version));
    init_checker_pool(&result, version, create_checker_pool, load_host);
    (result, new_file, reused)
}

/// Not in Go: the number of programs that `new_program` or
/// `update_program` made on this thread and `release_program` has not
/// released. Tests check that no program is left behind with it.
pub fn registered_programs() -> usize {
    PROGRAM_CHECKERS.with(|programs| programs.borrow().len())
}

/// Go drops a program when no snapshot uses it (`programCounter.Deref`
/// returns true) and no request holds it. This is the snapshot part: the
/// checker pools of `p` and its program version are freed now, or when the
/// last guard or hold of `p` drops (a request that still runs on it, a
/// queued snapshot change task).
pub fn release_program(p: &NewProgram) {
    let Some(checkers) =
        PROGRAM_CHECKERS.with(|programs| programs.borrow().get(&program_key(p)).cloned())
    else {
        return;
    };
    if is_entered(checkers.version) || is_held(checkers.version) {
        RELEASE_PENDING.with(|pending| {
            pending
                .borrow_mut()
                .insert(checkers.version.id, program_key(p))
        });
    } else {
        release_now(program_key(p));
    }
}

/// Removes the program with registry key `key` from the registry: the
/// registry's `Rc` of the program goes, and its checker pools go once no
/// project holds them. It releases the program version and each host of
/// the program that no live program uses now, and frees the synthetic
/// nodes that this thread made while the version was current. The program
/// tables and the frontend program are freed after the answer
/// (`gostd::local::drop_later`), as Go's garbage collector frees them in
/// the background.
fn release_now(key: usize) {
    let Some(checkers) = PROGRAM_CHECKERS.with(|programs| programs.borrow_mut().remove(&key))
    else {
        return;
    };
    crate::gostd::local::drop_later(Box::new(crate::program::release_program_later(
        checkers.version,
    )));
    release_hosts(&checkers);
    let version = checkers.version;
    drop(checkers);
    // Not in Go: the GC frees the nodes that no checker or request reaches.
    // No snapshot and no guard of the version is alive here.
    crate::ast::free_synthetic_owner(version.id);
}

// Go: compiler/program.go:455 initCheckerPool
// PORT: `load_host` is not in Go (`ProgramCheckers::load_host`). Go ts#64519
// passes the factory as an argument and does not keep it.
fn init_checker_pool(
    p: &Rc<NewProgram>,
    version: &'static GoProgram,
    create_checker_pool: Option<CreateCheckerPool>,
    load_host: Rc<dyn CompilerHost>,
) {
    if !p.finished_processing {
        panic!("Program must finish processing files before initializing checker pool");
    }
    // PORT: the synthetic nodes that this thread makes while `version` is
    // current belong to it; `release_now` frees them.
    crate::ast::open_synthetic_owner(version.id);

    let (checker_pool, compiler_checker_pool): (
        Rc<dyn CheckerPool>,
        Option<Rc<CompilerCheckerPool>>,
    ) = if let Some(create) = &create_checker_pool {
        (create(p), None)
    } else {
        let pool = Rc::new(new_checker_pool_with_tracing(p));
        let checker_pool: Rc<dyn CheckerPool> = pool.clone();
        (checker_pool, Some(pool))
    };
    let checkers = Rc::new(ProgramCheckers {
        program: Rc::clone(p),
        version,
        checker_pool,
        compiler_checker_pool,
        declaration_diagnostic_cache: RefCell::new(FxHashMap::default()),
        load_host,
    });
    hold_hosts(&checkers);
    PROGRAM_CHECKERS.with(|programs| {
        programs.borrow_mut().insert(program_key(p), checkers);
    });
}

// Go: compiler/program.go:470 GetCheckerPool
// GetCheckerPool returns the checker pool associated with this program.
pub fn get_checker_pool(p: &NewProgram) -> Rc<dyn CheckerPool> {
    program_checkers(p).checker_pool.clone()
}

// Go: checker/checker.go:913 NewChecker (the checker id)
// PORT: Go `NewChecker(program, tracer)` also returns the checker mutex; a
// pool here holds `Rc<RefCell<Checker>>` and the borrow is the lock. The
// tracer is dropped. Go `program.BindSourceFiles()` runs inside
// `Checker::new` (`bind_all`), with the program of `p` current.
// PORT: Go `c.id = nextCheckerID.Add(1)`. `Checker::new(index)` sets
// `id = index + 1`, so the index is the counter value before the add.
pub fn new_checker(p: &NewProgram) -> Checker {
    new_checker_for_version(program_version(p))
}

/// `new_checker` for a program version. Go `checker.NewChecker(program)`
/// takes any `checker.Program`; the autoimport alias resolver's program
/// (`program::new_alias_resolver_program`) has no `NewProgram`.
pub fn new_checker_for_version(version: &'static GoProgram) -> Checker {
    let _program = enter_version(version);
    Checker::new(next_checker_index())
}

/// `new_checker` for the API's persistent checker
/// (`project::checkerpool` `get_persistent_checker`). Its symbol arena
/// copies the binder lineage as it is when the checker is made
/// (`program::lineage_for_checker`), not the program's older copy
/// (`program::bound_symbols`), which can still hold the chunks of versions
/// that died after the program bound.
// PORT: Go's API hands a symbol of any project to this checker
// (api/session.go:2439 handleGetTypeOfSymbol), and a Go checker reads
// `node.Symbol()` of every bound file. A port checker reads the lineage ids
// of its copy, and `api::checker_symbol` catches it up to the lineage
// (`program::catch_up_checker`) before a symbol of another checker enters
// it. Lineage ids are global, so its reads of its own files do not change.
pub fn new_api_checker(p: &NewProgram) -> Checker {
    let _program = enter_version(program_version(p));
    crate::program::bind_all();
    Checker::with_symbols(next_checker_index(), crate::program::lineage_for_checker())
}

/// The next checker index (`Checker::new`): Go `nextCheckerID.Add(1) - 1`.
fn next_checker_index() -> usize {
    NEXT_CHECKER_ID.with(|next| {
        let id = next.get() + 1;
        next.set(id);
        (id - 1) as usize
    })
}

// ---------------------------------------------------------------------------
// Compiler checker pool (Go compiler/checkerpool.go)
// ---------------------------------------------------------------------------

// Go: compiler/checkerpool.go:28 checkerPool
// PORT: the pool lives on the dispatch thread. Go `locks` are the
// `RefCell` of each checker: a caller holds `borrow_mut` where Go holds the
// lock, and a second borrow panics where Go would block. `tracing` is dropped.
// PORT: Go `checkers` starts as a list of nil pointers; each slot here is
// an empty `OnceCell` until `createCheckers`. Go `fileAssociations` maps a
// file to its checker; here it maps the file index to the checker index.
pub struct CompilerCheckerPool {
    program: Rc<NewProgram>,
    create_checkers_once: Cell<bool>,
    checkers: Vec<OnceCell<Rc<RefCell<Checker>>>>,
    file_associations: OnceCell<FxHashMap<usize, usize>>,
}

// ---------------------------------------------------------------------------
// Checker association (Go compiler/checkerpool.go, #4313)
// ---------------------------------------------------------------------------

// Go: compiler/checkerpool.go:38
//
// Checker association is a balanced graph-partitioning problem:
//
//   - A vertex is a source file.
//   - An undirected edge connects two files for each resolved, in-program import
//     entry between them. Multiple entries may connect the same pair and therefore
//     strengthen their affinity. Self-imports and unresolved or external targets do
//     not create edges.
//   - A partition is a checker with its own symbol, type, and instantiation caches.
//
// Putting related files on the same checker reduces duplicated cache construction,
// but concentrating too many roots on one checker increases the parallel critical
// path. We use weighted FENNEL to trade off those objectives:
//
//   affinity(partition) - alpha * incrementalLoadPenalty(partition)
//
// See Tsourakakis et al., "FENNEL: Streaming Graph Partitioning for Massive Scale
// Graphs", WSDM 2014: https://doi.org/10.1145/2556195.2556213.
//
// FENNEL is sensitive to stream order. Go's comment gives the calibration data;
// stream order is part of the policy below, rather than an incidental
// implementation detail.
//
// Go: compiler/checkerpool.go:74
// The constants below are empirical safety factors for a work proxy that cannot
// observe future semantic cache construction. They were swept across representative
// projects including VS Code, TypeScript, MUI docs, XState, and Bluesky, with 2, 4,
// and 8 checkers (Go's comment lists each sweep). These are project-independent
// operating points, not formulas derived by FENNEL.
// PORT: Go `int` is `i64` for weights and multipliers.

// Go: compiler/checkerpool.go:100 const(
pub const CHECKER_ASSOCIATION_TEXT_WEIGHT_DIVISOR: i64 = 100;
pub const CHECKER_ASSOCIATION_SOURCE_FILE_WEIGHT_MULTIPLIER: i64 = 4;
pub const CHECKER_ASSOCIATION_BALANCE_PENALTY_MULTIPLIER: i64 = 16;
pub const CHECKER_ASSOCIATION_PRIORITIZED_SOURCE_PENALTY: i64 = 12;
pub const CHECKER_ASSOCIATION_STRONG_BALANCE_MIN_CHECKER_COUNT: usize = 4;

// Go: compiler/checkerpool.go:110 checkerAssociationPolicy
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CheckerAssociationPolicy {
    pub prioritize_source_files: bool,
    pub source_file_weight_multiplier: i64,
    pub balance_penalty_multiplier: i64,
}

// Go: compiler/checkerpool.go:133 getCheckerAssociationPolicy
// getCheckerAssociationPolicy selects one of three calibrated regimes:
//
//  1. Source-dominated, any checker count:
//     source files first by descending weight; unmodified source-file weight;
//     checkerAssociationPrioritizedSourcePenalty.
//  2. Declaration-heavy, at least checkerAssociationStrongBalanceMinCheckerCount:
//     program order; checkerAssociationSourceFileWeightMultiplier;
//     checkerAssociationBalancePenaltyMultiplier.
//  3. Declaration-heavy, fewer checkers:
//     program order; unmodified source-file weight; unscaled adapted FENNEL
//     penalty.
//
// The source-dominated test is evaluated first intentionally: projects with very
// little declaration work benefit from balancing source-file roots directly even
// with a small checker pool.
#[must_use]
pub fn get_checker_association_policy(
    total_weight: i64,
    declaration_weight: i64,
    checker_count: usize,
) -> CheckerAssociationPolicy {
    if should_prioritize_source_files(total_weight, declaration_weight, checker_count) {
        return CheckerAssociationPolicy {
            prioritize_source_files: true,
            source_file_weight_multiplier: 1,
            balance_penalty_multiplier: CHECKER_ASSOCIATION_PRIORITIZED_SOURCE_PENALTY,
        };
    }
    if checker_count >= CHECKER_ASSOCIATION_STRONG_BALANCE_MIN_CHECKER_COUNT {
        return CheckerAssociationPolicy {
            prioritize_source_files: false,
            source_file_weight_multiplier: CHECKER_ASSOCIATION_SOURCE_FILE_WEIGHT_MULTIPLIER,
            balance_penalty_multiplier: CHECKER_ASSOCIATION_BALANCE_PENALTY_MULTIPLIER,
        };
    }
    CheckerAssociationPolicy {
        prioritize_source_files: false,
        source_file_weight_multiplier: 1,
        balance_penalty_multiplier: 1,
    }
}

// Go: compiler/checkerpool.go:166 getCheckerAssociationsInOrder
// getCheckerAssociationsInOrder partitions the import graph using a weighted adaptation
// of FENNEL's streaming graph-partitioning objective with gamma = 3/2. Each file
// is placed where it has the most already-placed neighbors, minus the incremental
// convex load penalty. The published alpha = m*sqrt(k)/n^(3/2) becomes
// m*sqrt(k)/W^(3/2), where W is total estimated checker work. penaltyMultiplier
// applies the empirical safety factor selected by getCheckerAssociationPolicy.
//
// A nil order means stable program order. The preferred maximum checker weight is
// the larger of the largest file and roughly 101% of average. If no checker can
// accept a file under that bound, the file is assigned to the least-loaded checker.
// The 1% slack permits discrete files to pack near the average while preventing
// affinity from deliberately creating meaningful estimated imbalance. Ties are
// deterministic.
// PORT: Go nil `fileOrder` is `None`. Go returns nil for no file; this
// returns an empty list. The float math is in Go's order, so the scores are
// the same bits.
#[must_use]
pub fn get_checker_associations_in_order(
    file_weights: &[i64],
    adjacent_files: &[Vec<usize>],
    file_order: Option<&[usize]>,
    checker_count: usize,
    penalty_multiplier: i64,
) -> Vec<usize> {
    if file_weights.is_empty() {
        return Vec::new();
    }

    let mut total_weight: i64 = 0;
    let mut max_file_weight: i64 = 0;
    let mut edge_count: usize = 0;
    for (i, &weight) in file_weights.iter().enumerate() {
        total_weight += weight;
        max_file_weight = max_file_weight.max(weight);
        edge_count += adjacent_files[i].len();
    }

    // Go -1 is `None`.
    let mut associations: Vec<Option<usize>> = vec![None; file_weights.len()];
    let mut checker_weights: Vec<i64> = vec![0; checker_count];
    let checker_count_int = checker_count as i64;
    let average_checker_weight = (total_weight + checker_count_int - 1) / checker_count_int;
    let max_checker_weight =
        max_file_weight.max(average_checker_weight + average_checker_weight / 100);
    let total_weight_float = total_weight as f64;
    let alpha = penalty_multiplier as f64 * (edge_count / 2) as f64 * (checker_count as f64).sqrt()
        / (total_weight_float * total_weight_float.sqrt());
    let mut neighbor_counts: Vec<i64> = vec![0; checker_count];

    for position in 0..file_weights.len() {
        let file_index = file_order.map_or(position, |order| order[position]);

        neighbor_counts.fill(0);
        for &adjacent_file in &adjacent_files[file_index] {
            if let Some(checker_index) = associations[adjacent_file] {
                neighbor_counts[checker_index] += 1;
            }
        }

        let mut best_checker: Option<usize> = None;
        let mut best_score = f64::NEG_INFINITY;
        for (checker_index, &checker_weight) in checker_weights.iter().enumerate() {
            if checker_weight + file_weights[file_index] > max_checker_weight {
                continue;
            }
            let old_weight = checker_weight as f64;
            let new_weight = (checker_weight + file_weights[file_index]) as f64;
            let new_penalty = new_weight * new_weight.sqrt();
            let old_penalty = old_weight * old_weight.sqrt();
            let penalty = alpha * (new_penalty - old_penalty);
            let score = neighbor_counts[checker_index] as f64 - penalty;
            if score > best_score
                || score == best_score
                    && best_checker.is_none_or(|best| checker_weight < checker_weights[best])
            {
                best_checker = Some(checker_index);
                best_score = score;
            }
        }
        // Go: the least-loaded checker, the first one on a tie.
        let best_checker = best_checker.unwrap_or_else(|| {
            let mut best_checker = 0;
            for (checker_index, &checker_weight) in checker_weights.iter().enumerate().skip(1) {
                if checker_weight < checker_weights[best_checker] {
                    best_checker = checker_index;
                }
            }
            best_checker
        });
        associations[file_index] = Some(best_checker);
        checker_weights[best_checker] += file_weights[file_index];
    }
    associations
        .into_iter()
        .map(|checker_index| checker_index.expect("every file gets a checker"))
        .collect()
}

// Go: compiler/checkerpool.go:241 getCheckerAssociationOrder
// getCheckerAssociationOrder places source files before declarations and
// orders each group by descending estimated work. This exposes expensive semantic
// roots early, when all checker loads are still available. Returning nil preserves
// program order without allocating an index array. Program order is itself a
// locality choice: it preserves deterministic groups produced during program
// construction and was consistently safer for declaration-heavy projects.
// PORT: Go nil is `None`.
#[must_use]
pub fn get_checker_association_order(
    file_weights: &[i64],
    is_declaration_file: &[bool],
    prioritize_source_files: bool,
) -> Option<Vec<usize>> {
    if !prioritize_source_files {
        return None;
    }
    let mut file_order: Vec<usize> = (0..file_weights.len()).collect();
    // Go: compiler/checkerpool.go:249 sort.Slice(fileOrder, ...)
    crate::gostd::slices::sort_slice(&mut file_order, |&left, &right| {
        if is_declaration_file[left] != is_declaration_file[right] {
            return !is_declaration_file[left];
        }
        if file_weights[left] != file_weights[right] {
            return file_weights[left] > file_weights[right];
        }
        left < right
    });
    Some(file_order)
}

// Go: compiler/checkerpool.go:263 getCheckerAssociationBaseWeight
#[must_use]
pub fn get_checker_association_base_weight(node_count: i64, text_length: i64) -> i64 {
    (node_count + text_length / CHECKER_ASSOCIATION_TEXT_WEIGHT_DIVISOR).max(1)
}

// Go: compiler/checkerpool.go:276 shouldPrioritizeSourceFiles
// shouldPrioritizeSourceFiles reports whether all declaration-file base work is at
// most half of one average checker load:
//
//	declarationWeight <= totalWeight / (2 * checkerCount)
//
// This threshold separated source-dominated projects such as VS Code from projects
// where declaration locality remained important, such as MUI docs, TypeScript, and
// XState. Delaying at most half a checker-load of declarations was the stable
// boundary in the cross-project sweeps.
#[must_use]
pub fn should_prioritize_source_files(
    total_weight: i64,
    declaration_weight: i64,
    checker_count: usize,
) -> bool {
    declaration_weight * checker_count as i64 * 2 <= total_weight
}

// Go: compiler/checkerpool.go:287 getCheckerAssociationWeights
// getCheckerAssociationWeights combines local syntax work with syntactic import
// fanout. One import unit is totalBaseWeight / totalImports, so imports collectively
// contribute approximately the same vertex weight as syntax. Syntactic imports are
// deliberately broader than getImportAdjacency's resolved, in-program edges: this
// term estimates the work of processing module references, while adjacency controls
// checker affinity. Normalizing the term avoids a project-specific vertex-weight
// constant.
#[must_use]
pub fn get_checker_association_weights(base_weights: &[i64], import_counts: &[i64]) -> Vec<i64> {
    let mut total_base_weight: i64 = 0;
    let mut total_imports: i64 = 0;
    for (i, &base_weight) in base_weights.iter().enumerate() {
        total_base_weight += base_weight;
        total_imports += import_counts[i];
    }
    let mut import_weight: i64 = 0;
    if total_imports > 0 {
        import_weight = (total_base_weight / total_imports).max(1);
    }
    base_weights
        .iter()
        .zip(import_counts)
        .map(|(&base_weight, &import_count)| base_weight + import_count * import_weight)
        .collect()
}

/// The checker index of each file of `program.files`, in file order: the
/// association part of Go `checkerPool.createCheckers` (#4313). The compile
/// path pool (`program::create_checkers`) uses it too.
// Go: compiler/checkerpool.go:367 (*checkerPool).createCheckers (the associations)
#[must_use]
pub fn get_checker_associations(program: &NewProgram, checker_count: usize) -> Vec<usize> {
    let files = &program.files;
    if checker_count <= 1 {
        return vec![0; files.len()];
    }
    let mut base_weights: Vec<i64> = Vec::with_capacity(files.len());
    let mut import_counts: Vec<i64> = Vec::with_capacity(files.len());
    let mut is_declaration_file: Vec<bool> = Vec::with_capacity(files.len());
    let mut total_base_weight: i64 = 0;
    let mut declaration_base_weight: i64 = 0;
    for file in files {
        let base_weight =
            get_checker_association_base_weight(file.node_count as i64, file.text.len() as i64);
        total_base_weight += base_weight;
        if file.is_declaration_file {
            declaration_base_weight += base_weight;
        }
        base_weights.push(base_weight);
        import_counts.push(file.imports.len() as i64);
        is_declaration_file.push(file.is_declaration_file);
    }
    let policy =
        get_checker_association_policy(total_base_weight, declaration_base_weight, checker_count);
    if policy.source_file_weight_multiplier != 1 {
        // Apply this before import normalization. The policy intentionally
        // increases both source-file work and the normalized import unit.
        for (base_weight, &declaration) in base_weights.iter_mut().zip(&is_declaration_file) {
            if !declaration {
                *base_weight *= policy.source_file_weight_multiplier;
            }
        }
    }
    let file_weights = get_checker_association_weights(&base_weights, &import_counts);
    let adjacent_files = get_import_adjacency(program);
    let file_order = get_checker_association_order(
        &file_weights,
        &is_declaration_file,
        policy.prioritize_source_files,
    );
    get_checker_associations_in_order(
        &file_weights,
        &adjacent_files,
        file_order.as_deref(),
        checker_count,
        policy.balance_penalty_multiplier,
    )
}

// Go: compiler/checkerpool.go:425 (*checkerPool).getImportAdjacency
// getImportAdjacency returns an undirected import graph represented by file
// index. A directed import from A to B makes both files adjacent because either
// file can benefit from sharing checker caches with the other.
// PORT: Go ranges over the resolution map in random order; the order of a
// list does not change the associations, only its length and contents do.
fn get_import_adjacency(program: &NewProgram) -> Vec<Vec<usize>> {
    let files = &program.files;
    let file_indices: FxHashMap<usize, usize> = files
        .iter()
        .enumerate()
        .map(|(i, file)| (file.store, i))
        .collect();
    let mut adjacent_files: Vec<Vec<usize>> = vec![Vec::new(); files.len()];
    for (file_index, file) in files.iter().enumerate() {
        let Some(resolved_modules) = program.processed_files.resolved_modules.get(file.path())
        else {
            continue;
        };
        for resolved in resolved_modules.values() {
            if !resolved.is_resolved() {
                continue;
            }
            let imported_index = program
                .get_source_file_for_resolved_module(&resolved.resolved_file_name)
                .and_then(|imported_file| file_indices.get(&imported_file.store).copied());
            let Some(imported_index) = imported_index else {
                continue;
            };
            if imported_index == file_index {
                continue;
            }
            adjacent_files[file_index].push(imported_index);
            adjacent_files[imported_index].push(file_index);
        }
    }
    adjacent_files
}

// Go: compiler/checkerpool.go:305 newCheckerPool
fn new_checker_pool(program: &Rc<NewProgram>) -> CompilerCheckerPool {
    new_checker_pool_with_tracing(program)
}

// Go: compiler/checkerpool.go:309 newCheckerPoolWithTracing
// PORT: tracing is dropped.
fn new_checker_pool_with_tracing(program: &Rc<NewProgram>) -> CompilerCheckerPool {
    let mut checker_count: i64 = 4;
    if program.single_threaded() {
        checker_count = 1;
    } else if let Some(c) = program.options().checkers {
        checker_count = i64::from(c);
    }

    checker_count = checker_count
        .min(program.files.len() as i64)
        .min(256)
        .max(1);

    CompilerCheckerPool {
        program: Rc::clone(program),
        create_checkers_once: Cell::new(false),
        checkers: (0..checker_count).map(|_| OnceCell::new()).collect(),
        file_associations: OnceCell::new(),
    }
}

impl CheckerPool for CompilerCheckerPool {
    // Go: compiler/checkerpool.go:331 (*checkerPool).GetChecker
    // GetChecker implements CheckerPool. When file is non-nil, returns the checker
    // associated with that file; otherwise returns the first checker.
    // PORT: Go locks checker 0 and returns the unlock. The lock is the
    // caller's borrow, so the release does nothing.
    fn get_checker(&self, ctx: &Context, file: Node) -> (Rc<RefCell<Checker>>, Release) {
        if file.is_some() {
            return self.get_checker_for_file_exclusive(ctx, file);
        }
        self.create_checkers();
        let c = Rc::clone(self.checker(0));
        (c, Release::noop())
    }
}

impl CompilerCheckerPool {
    /// Go `p.checkers[i]` after `createCheckers`.
    fn checker(&self, i: usize) -> &Rc<RefCell<Checker>> {
        self.checkers[i]
            .get()
            .expect("checkers are made by createCheckers")
    }

    /// The checker index of `file` (Go `fileAssociations[file]`).
    // PORT: Go returns a nil checker for a file outside the program, and
    // its callers then fail on it. This panics.
    fn checker_index_for_file(&self, file: Node) -> usize {
        *self
            .file_associations
            .get()
            .expect("checkers are made by createCheckers")
            .get(&file.file_index())
            .expect("file is not in the program")
    }

    // Go: compiler/checkerpool.go:346 (*checkerPool).getCheckerForFileNonExclusive
    // getCheckerForFileNonExclusive returns the checker for the given file without locking.
    // This is only safe when the caller guarantees no concurrent access to the same checker,
    // e.g. for read-only operations like obtaining an emit resolver.
    fn get_checker_for_file_non_exclusive(&self, file: Node) -> (Rc<RefCell<Checker>>, Release) {
        self.create_checkers();
        let c = Rc::clone(self.checker(self.checker_index_for_file(file)));
        (c, Release::noop())
    }

    // Go: compiler/checkerpool.go:351 (*checkerPool).getCheckerForFileExclusive
    // PORT: Go locks the checker and returns the unlock. The lock is the
    // caller's borrow, so the release does nothing.
    fn get_checker_for_file_exclusive(
        &self,
        ctx: &Context,
        file: Node,
    ) -> (Rc<RefCell<Checker>>, Release) {
        self.create_checkers();
        let idx = self.checker_index_for_file(file);
        let c = Rc::clone(self.checker(idx));
        (c, Release::noop())
    }

    // Go: compiler/checkerpool.go:362 (*checkerPool).getCheckerNonExclusive
    // getCheckerNonExclusive returns the first checker without locking.
    fn get_checker_non_exclusive(&self) -> (Rc<RefCell<Checker>>, Release) {
        self.create_checkers();
        (Rc::clone(self.checker(0)), Release::noop())
    }

    // Go: compiler/checkerpool.go:367 (*checkerPool).createCheckers
    // PORT: Go `createCheckersOnce` is a flag. Go makes the checkers on a
    // WorkGroup; here they are made in index order, so their ids follow the
    // index.
    fn create_checkers(&self) {
        if self.create_checkers_once.replace(true) {
            return;
        }
        let checker_count = self.checkers.len();
        for i in 0..checker_count {
            let checker = new_checker(&self.program);
            let _ = self.checkers[i].set(Rc::new(RefCell::new(checker)));
        }

        // #4313: balanced import affinity.
        let associations = get_checker_associations(&self.program, checker_count);
        let mut file_associations = FxHashMap::default();
        for (i, file) in self.program.files.iter().enumerate() {
            file_associations.insert(file.root.file_index(), associations[i]);
        }
        let _ = self.file_associations.set(file_associations);
    }

    // Go: compiler/checkerpool.go:451 (*checkerPool).forEachCheckerParallel
    // Runs `cb` for each checker in the pool concurrently, locking and unlocking checker mutexes as it goes,
    // making it safe to call `forEachCheckerParallel` from many threads simultaneously.
    // PORT: on the dispatch thread the checkers run one after another, in
    // index order.
    pub fn for_each_checker_parallel(&self, cb: &mut dyn FnMut(usize, &mut Checker)) {
        self.create_checkers();
        for (idx, checker) in self.checkers.iter().enumerate() {
            let checker = checker.get().expect("checkers are made by createCheckers");
            cb(idx, &mut checker.borrow_mut());
        }
    }

    // Go: compiler/checkerpool.go:464 (*checkerPool).GetGlobalDiagnostics
    pub fn get_global_diagnostics(&self) -> Vec<Diagnostic> {
        self.create_checkers();
        let mut global_diagnostics: Vec<Vec<Diagnostic>> = vec![Vec::new(); self.checkers.len()];
        self.for_each_checker_parallel(&mut |idx, checker| {
            global_diagnostics[idx] = checker.get_global_diagnostics();
        });
        sort_and_deduplicate_diagnostics(global_diagnostics.into_iter().flatten().collect())
    }

    // Go: compiler/checkerpool.go:476 (*checkerPool).forEachCheckerGroupDo
    // forEachCheckerGroupDo runs one task per checker in parallel. Each task iterates
    // the provided files, processing only those assigned to its checker. Within each
    // checker's set, files are visited in their original order.
    // PORT: on the dispatch thread the tasks run one after another, in
    // checker order.
    fn for_each_checker_group_do(
        &self,
        ctx: &Context,
        files: &[Node],
        single_threaded: bool,
        cb: &mut dyn FnMut(&mut Checker, usize, Node),
    ) {
        self.create_checkers();

        let checker_count = self.checkers.len();
        let file_associations = self
            .file_associations
            .get()
            .expect("checkers are made by createCheckers");
        for checker_idx in 0..checker_count {
            let mut checker = self.checker(checker_idx).borrow_mut();
            for (i, &file) in files.iter().enumerate() {
                if file_associations.get(&file.file_index()) == Some(&checker_idx) {
                    cb(&mut checker, i, file);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Program checker access (Go compiler/program.go:445-492)
// ---------------------------------------------------------------------------

// Go: compiler/program.go:595 BindSourceFiles
// PORT: the Rust binder binds every file of the program version into one
// arena (`program::bind_all`). A file version that an earlier version
// bound is not bound again.
pub fn bind_source_files(p: &NewProgram) {
    let _program = enter(p);
    bind_all();
}

// PORT: each `get_type_checker*` function makes the program of `p` current
// until the returned release runs (see the module comment), so the caller
// uses the checker with its program current.

// Go: compiler/program.go:611 GetTypeChecker
// Return the type checker associated with the program.
pub fn get_type_checker(p: &NewProgram, ctx: &Context) -> (Rc<RefCell<Checker>>, Release) {
    let program = enter(p);
    let checkers = program_checkers(p);
    let (checker, release) = match &checkers.compiler_checker_pool {
        Some(pool) => pool.get_checker_non_exclusive(),
        None => checkers.checker_pool.get_checker(ctx, Node::NIL),
    };
    (checker, program.with_release(release))
}

// Go: compiler/program.go:618 ForEachCheckerParallel
pub fn for_each_checker_parallel(p: &NewProgram, cb: &mut dyn FnMut(usize, &mut Checker)) {
    let _program = enter(p);
    let checkers = program_checkers(p);
    if let Some(pool) = &checkers.compiler_checker_pool {
        pool.for_each_checker_parallel(cb);
    }
}

// Go: compiler/program.go:628 GetTypeCheckerForFile
// Return a checker for the given file. We may have multiple checkers in concurrent scenarios and this
// method returns the checker that was tasked with checking the file. Note that it isn't possible to mix
// types obtained from different checkers, so only non-type data (such as diagnostics or string
// representations of types) should be obtained from checkers returned by this method.
pub fn get_type_checker_for_file(
    p: &NewProgram,
    ctx: &Context,
    file: Node,
) -> (Rc<RefCell<Checker>>, Release) {
    let program = enter(p);
    let checkers = program_checkers(p);
    let (checker, release) = match &checkers.compiler_checker_pool {
        Some(pool) => pool.get_checker_for_file_non_exclusive(file),
        None => checkers.checker_pool.get_checker(ctx, file),
    };
    (checker, program.with_release(release))
}

// Go: compiler/program.go:637 GetTypeCheckerForFileExclusive
// Return a checker for the given file, locked to the current thread to prevent data races from multiple threads
// accessing the same checker. The lock will be released when the `done` function is called.
pub fn get_type_checker_for_file_exclusive(
    p: &NewProgram,
    ctx: &Context,
    file: Node,
) -> (Rc<RefCell<Checker>>, Release) {
    let program = enter(p);
    let checkers = program_checkers(p);
    let (checker, release) = match &checkers.compiler_checker_pool {
        Some(pool) => pool.get_checker_for_file_exclusive(ctx, file),
        None => checkers.checker_pool.get_checker(ctx, file),
    };
    (checker, program.with_release(release))
}

// ---------------------------------------------------------------------------
// Diagnostics (Go compiler/program.go:534-712 and 1290-1420)
// ---------------------------------------------------------------------------
//
// PORT: the per-file bodies that already exist in program.rs for the
// current program are called from here: `get_semantic_diagnostics_with_checker`
// (Go program.go:1315 getSemanticDiagnosticsWithChecker),
// `get_bind_and_check_diagnostics_with_checker` (:1325),
// `get_diagnostics_with_preceding_directives` (:1359),
// `get_suggestion_diagnostics_with_checker` (:1410) and
// `get_additional_js_syntactic_diagnostics` (:618). They read the file data
// of the current program; each public function here makes `p` current.

/// Go `p.files` as file nodes.
fn source_file_nodes(p: &NewProgram) -> Vec<Node> {
    p.files.iter().map(|file| file.root).collect()
}

// Go: compiler/program.go:691 collectDiagnostics
// collectDiagnostics collects diagnostics from a single file or all files.
// If sourceFile is non-nil, returns diagnostics for just that file.
// If sourceFile is nil, returns diagnostics for all files in the program.
fn collect_diagnostics(
    p: &NewProgram,
    ctx: &Context,
    source_file: Node,
    concurrent: bool,
    collect: &mut dyn FnMut(&Context, Node) -> Vec<Diagnostic>,
) -> Vec<Diagnostic> {
    let result = if source_file.is_some() {
        collect(ctx, source_file)
    } else {
        let diagnostics =
            collect_diagnostics_from_files(p, ctx, &source_file_nodes(p), concurrent, collect);
        diagnostics.into_iter().flatten().collect()
    };
    // #4712
    filter_and_sort_diagnostics(result)
}

// Go: compiler/program.go:702 collectDiagnosticsFromFiles
// PORT: Go runs the files on a WorkGroup. On the dispatch thread they run
// one after another, in file order.
fn collect_diagnostics_from_files(
    p: &NewProgram,
    ctx: &Context,
    source_files: &[Node],
    concurrent: bool,
    collect: &mut dyn FnMut(&Context, Node) -> Vec<Diagnostic>,
) -> Vec<Vec<Diagnostic>> {
    let mut diagnostics: Vec<Vec<Diagnostic>> = vec![Vec::new(); source_files.len()];
    for (i, &file) in source_files.iter().enumerate() {
        diagnostics[i] = collect(ctx, file);
    }
    diagnostics
}

// Go: compiler/program.go:719 collectCheckerDiagnostics
// collectCheckerDiagnostics collects diagnostics from a single file or all files,
// using a callback that receives the checker for each file. When the checker pool
// supports grouped iteration (compiler pool), files are grouped by checker and
// processed in parallel with one task per checker, reducing contention and improving
// cache locality. Otherwise, falls back to per-file concurrent collection.
fn collect_checker_diagnostics(
    p: &NewProgram,
    ctx: &Context,
    source_file: Node,
    collect: &mut dyn FnMut(&Context, &mut Checker, Node) -> Vec<Diagnostic>,
) -> Vec<Diagnostic> {
    if source_file.is_some() {
        if skip_type_checking(p, source_file, false) {
            return Vec::new();
        }
        let (c, done) = get_type_checker_for_file_exclusive(p, ctx, source_file);
        let result = collect(ctx, &mut c.borrow_mut(), source_file);
        done.call();
        // #4712
        return filter_and_sort_diagnostics(result);
    }
    let diagnostics =
        collect_checker_diagnostics_from_files(p, ctx, &source_file_nodes(p), collect);
    filter_and_sort_diagnostics(diagnostics.into_iter().flatten().collect())
}

// Go: compiler/program.go:744 collectCheckerDiagnosticsFromFiles
// collectCheckerDiagnosticsFromFiles collects checker diagnostics for a list of files.
// PORT: Go runs the files of an external pool on a WorkGroup. On the
// dispatch thread they run one after another, in file order.
fn collect_checker_diagnostics_from_files(
    p: &NewProgram,
    ctx: &Context,
    source_files: &[Node],
    collect: &mut dyn FnMut(&Context, &mut Checker, Node) -> Vec<Diagnostic>,
) -> Vec<Vec<Diagnostic>> {
    let mut diagnostics: Vec<Vec<Diagnostic>> = vec![Vec::new(); source_files.len()];
    let checkers = program_checkers(p);
    if let Some(pool) = &checkers.compiler_checker_pool {
        pool.for_each_checker_group_do(
            ctx,
            source_files,
            p.single_threaded(),
            &mut |c, file_index, file| {
                diagnostics[file_index] = collect(ctx, c, file);
            },
        );
    } else {
        for (i, &file) in source_files.iter().enumerate() {
            if skip_type_checking(p, file, false) {
                continue;
            }
            let (c, done) = checkers.checker_pool.get_checker(ctx, file);
            diagnostics[i] = collect(ctx, &mut c.borrow_mut(), file);
            done.call();
        }
    }
    diagnostics
}

// Go: compiler/program.go:767 GetSyntacticDiagnostics
pub fn get_syntactic_diagnostics(
    p: &NewProgram,
    ctx: &Context,
    source_file: Node,
) -> Vec<Diagnostic> {
    let _program = enter(p);
    let options = p.options();
    collect_diagnostics(
        p,
        ctx,
        source_file,
        false, /*concurrent*/
        &mut |_ctx, file| {
            let info = source_file_info(file);
            let mut diags: Vec<Diagnostic> = info
                .diagnostics
                .iter()
                .chain(info.js_diagnostics.iter())
                .cloned()
                .collect();
            // For JS files that won't be checked by the checker (no checkJs/ts-check), we need
            // program-level syntactic checks that require compiler options. This mirrors Strada's
            // getJSSyntacticDiagnosticsForFile in program.ts.
            if is_source_file_js(file) && !is_check_js_enabled_for_file(file, options) {
                diags.extend(get_additional_js_syntactic_diagnostics(file, options));
            }
            diags
        },
    )
}

// Go: compiler/program.go:811 GetBindDiagnostics
// PORT: Go binds only `sourceFile` when it is set, else every file. The
// Rust binder binds every file of the program version into one arena, so
// both cases bind all files.
pub fn get_bind_diagnostics(p: &NewProgram, ctx: &Context, source_file: Node) -> Vec<Diagnostic> {
    let _program = enter(p);
    bind_source_files(p);
    collect_diagnostics(
        p,
        ctx,
        source_file,
        false, /*concurrent*/
        &mut |_ctx, file| file_bind_data(file).bind_diagnostics.clone(),
    )
}

// Go: compiler/program.go:822 GetSemanticDiagnostics
pub fn get_semantic_diagnostics(
    p: &NewProgram,
    ctx: &Context,
    source_file: Node,
) -> Vec<Diagnostic> {
    let _program = enter(p);
    collect_checker_diagnostics(p, ctx, source_file, &mut |ctx, c, file| {
        get_semantic_diagnostics_with_checker(ctx, c, file)
    })
}

// Go: compiler/program.go:828 GetSemanticDiagnosticsForIncremental
// GetSemanticDiagnosticsForIncremental includes newly discovered globals in each
// file's cached diagnostics and leaves noEmit filtering to the builder.
pub fn get_semantic_diagnostics_for_incremental(
    p: &NewProgram,
    ctx: &Context,
    source_files: &[Node],
) -> FxHashMap<Node, Vec<Diagnostic>> {
    let _program = enter(p);
    let all_diags =
        collect_checker_diagnostics_from_files(p, ctx, source_files, &mut |ctx, c, file| {
            get_bind_and_check_diagnostics_with_checker(
                ctx, c, file, true, /*includeDeferredGlobals*/
            )
        });
    let mut result = FxHashMap::default();
    for (i, diags) in all_diags.into_iter().enumerate() {
        // #4712
        result.insert(source_files[i], filter_and_sort_diagnostics(diags));
    }
    result
}

// Go: compiler/program.go:839 GetSuggestionDiagnostics
pub fn get_suggestion_diagnostics(
    p: &NewProgram,
    ctx: &Context,
    source_file: Node,
) -> Vec<Diagnostic> {
    let _program = enter(p);
    collect_checker_diagnostics(p, ctx, source_file, &mut |ctx, c, file| {
        get_suggestion_diagnostics_with_checker(ctx, c, file)
    })
}

// Go: compiler/program.go:843 GetProgramDiagnostics
pub fn get_program_diagnostics(p: &NewProgram) -> Vec<Diagnostic> {
    let _program = enter(p);
    let mut diagnostics = p.program_diagnostics.clone();
    // #4712
    diagnostics.extend(p.content_mapper_diagnostics.iter().cloned());
    diagnostics.extend(content_mapper_option_diagnostics());
    diagnostics.extend(
        p.include_processor
            .get_diagnostics(p)
            .borrow_mut()
            .get_global_diagnostics(),
    );
    sort_and_deduplicate_diagnostics(diagnostics)
}

// Go: compiler/program.go:864 GetIncludeProcessorDiagnostics
pub fn get_include_processor_diagnostics(p: &NewProgram, source_file: Node) -> Vec<Diagnostic> {
    let _program = enter(p);
    if skip_type_checking(p, source_file, false) {
        return Vec::new();
    }
    // #4825: keyed by the source file, not its name.
    let diagnostics = p
        .include_processor
        .get_diagnostics(p)
        .borrow_mut()
        .get_diagnostics_for_file(source_file);
    let (filtered, _) = get_diagnostics_with_preceding_directives(source_file, diagnostics);
    filtered
}

// Go: compiler/program.go:872 SkipTypeChecking
pub fn skip_type_checking(p: &NewProgram, source_file: Node, ignore_no_check: bool) -> bool {
    let _program = enter(p);
    let options = p.options();
    let info = source_file_info(source_file);
    let path = tspath::Path(info.path.clone());
    (!ignore_no_check && options.no_check.is_true())
        || options.skip_lib_check.is_true() && info.is_declaration_file
        || options.skip_default_lib_check.is_true() && p.is_source_file_default_library(&path)
        || p.is_source_from_project_reference(&path)
        || !can_include_bind_and_check_diagnostics(p, source_file)
}

// Go: compiler/program.go:880 canIncludeBindAndCheckDiagnostics
fn can_include_bind_and_check_diagnostics(p: &NewProgram, source_file: Node) -> bool {
    let info = source_file_info(source_file);
    if info.check_js_directive.is_some_and(|d| !d.enabled) {
        return false;
    }

    // #4712: no ScriptKindExternal.
    if info.script_kind == ScriptKind::TS || info.script_kind == ScriptKind::TSX {
        return true;
    }

    let is_js = info.script_kind == ScriptKind::JS || info.script_kind == ScriptKind::JSX;
    let is_check_js = is_js && is_check_js_enabled_for_file(source_file, p.options());
    let is_plain_js = is_plain_js_file(source_file, p.options().check_js);

    // By default, only type-check .ts, .tsx, plain JS, and checked JS
    // - plain JS: .js files with no // ts-check and checkJs: undefined
    // - check JS: .js files with either // ts-check or checkJs: true
    // #4712: no ScriptKindDeferred.
    is_plain_js || is_check_js
}

// Go: compiler/program.go:1479 GetGlobalDiagnostics
pub fn get_global_diagnostics(p: &NewProgram, ctx: &Context) -> Vec<Diagnostic> {
    let _program = enter(p);
    if p.files.is_empty() {
        return Vec::new();
    }
    let checkers = program_checkers(p);
    if let Some(pool) = &checkers.compiler_checker_pool {
        return pool.get_global_diagnostics();
    }
    // For external pools (project system), global diagnostics are collected
    // incrementally as checkers are used, not via a bulk query.
    Vec::new()
}

// Go: compiler/program.go:1491 GetDeclarationDiagnostics
pub fn get_declaration_diagnostics(
    p: &NewProgram,
    ctx: &Context,
    source_file: Node,
) -> Vec<Diagnostic> {
    let _program = enter(p);
    collect_diagnostics(
        p,
        ctx,
        source_file,
        true, /*concurrent*/
        &mut |ctx, file| get_declaration_diagnostics_for_file(p, ctx, file),
    )
}

// Go: compiler/program.go:1642 getDeclarationDiagnosticsForFile
fn get_declaration_diagnostics_for_file(
    p: &NewProgram,
    ctx: &Context,
    source_file: Node,
) -> Vec<Diagnostic> {
    if source_file_info(source_file).is_declaration_file {
        return Vec::new();
    }

    let checkers = program_checkers(p);
    if let Some(cached) = checkers
        .declaration_diagnostic_cache
        .borrow()
        .get(&source_file)
    {
        return cached.clone();
    }

    let (host, done) = new_emit_host(p, ctx, source_file);
    let diagnostics = get_declaration_diagnostics_worker(host, source_file);
    // Go `LoadOrStore`: keep the first stored value.
    let diagnostics = checkers
        .declaration_diagnostic_cache
        .borrow_mut()
        .entry(source_file)
        .or_insert(diagnostics)
        .clone();
    // Go `defer done()`.
    done.call();
    diagnostics
}

// Go: compiler/emitHost.go:39 newEmitHost
// PORT: the language-service form of `program::new_emit_host`. The checker
// comes from `GetTypeCheckerForFile` of `p`, which a dispatch-thread pool
// shares as `Rc<RefCell<Checker>>`, so each resolver links to that `Rc`
// (`new_emit_resolver_of_shared_checker`, ts#64649) instead of a compile
// worker checker. The host keeps a weak link, as the resolvers do. The host
// methods read the current program, which is `p`.
fn new_emit_host(p: &NewProgram, ctx: &Context, file: Node) -> (Rc<EmitHost>, Release) {
    let (checker, done) = get_type_checker_for_file(p, ctx, file);
    let checker = Rc::downgrade(&checker);
    let host = crate::program::new_emit_host_with(Rc::new(move |emit_context| {
        let checker = checker
            .upgrade()
            .expect("the checker of an emit host was dropped");
        let resolver: Rc<dyn crate::printer::EmitResolver> =
            crate::checker::emit_resolver_p1::new_emit_resolver_of_shared_checker(
                &checker,
                emit_context,
            );
        resolver
    }));
    (host, done)
}

// Go: compiler/program.go:1890 Program.Emit
// PORT: the language service form of `program_emit::emit` (#4710: the
// language server flake check; the API emit methods call Go `Program.Emit`
// too). Each file's emit host gets its checker from the checker pool of `p`
// (`new_emit_host`), as in Go, so the emit uses the checkers that the
// diagnostics use, not a compile worker pool. Go runs the files on a
// WorkGroup. On the dispatch thread they run one after another, in file
// order. A language service program has no Go `opts.Tracing`, so there is
// no trace event. Go returns nil when `ctx` is done after
// `HandleNoEmitOptions` gave nil; here that is `EmitResult::default()`.
pub fn emit(p: &NewProgram, ctx: &Context, options: EmitOptions) -> EmitResult {
    let _program = enter(p);
    if !options.force_emit && options.emit_only != EmitOnly::BuilderSignature {
        let result = handle_no_emit_options(p, ctx, options.target_source_files.as_deref());
        if result.is_some() || ctx.err().is_some() {
            return result.unwrap_or_default();
        }
    }

    let new_line = p.options().new_line.get_new_line_character();
    let force_dts_emit = options.emit_only == EmitOnly::BuilderSignature
        || options.force_emit && options.emit_only == EmitOnly::Dts;
    let force_js_emit = options.force_emit && options.emit_only == EmitOnly::Js;
    let source_files = get_source_files_to_emit(
        options.target_source_files.as_deref(),
        force_dts_emit,
        force_js_emit,
    );

    let results: Vec<EmitResult> = source_files
        .into_iter()
        .map(|source_file| {
            let (host, done) = new_emit_host(p, ctx, source_file);

            let writer: Rc<RefCell<dyn EmitTextWriter>> =
                Rc::new(RefCell::new(new_text_writer(new_line, 0)));
            writer.borrow_mut().clear();

            // attach writer and perform emit
            let paths = get_output_paths_for_source_file(
                source_file,
                host.as_ref(),
                ForceEmitPaths {
                    dts: force_dts_emit,
                    js: force_js_emit,
                    declaration_map: options.force_emit && options.emit_only == EmitOnly::Dts,
                },
            );
            let mut emitter = Emitter {
                host,
                emit_only: options.emit_only,
                emitter_diagnostics: DiagnosticsCollection::default(),
                writer: Some(writer),
                paths,
                source_file,
                emit_result: EmitResult::default(),
                force_emit: options.force_emit,
                write_file: options.write_file.clone(),
                js_part: None,
            };
            emitter.emit();
            emitter.writer = None;
            // Go `defer done()`.
            done.call();
            emitter.emit_result
        })
        .collect();

    // collect results from emit, preserving input order
    combine_emit_results(results)
}

// Go: compiler/program.go:1999 HandleNoEmitOptions
// PORT: the language service form of `program_emit::handle_no_emit_options`
// (`Program.Emit` passes a nil `emitBuildInfo`). The bind, semantic, global
// and declaration diagnostics come from `p` and its checker pool. The
// config, syntactic and program diagnostics of `get_diagnostics_of_any_program`
// read the current program, which is `p`.
fn handle_no_emit_options(
    p: &NewProgram,
    ctx: &Context,
    files: Option<&[Node]>,
) -> Option<EmitResult> {
    let options = p.options();
    if !options.no_emit.is_true() {
        if !options.no_emit_on_error.is_true() {
            return None; // NoEmit is false and NoEmitOnError is also false, so we can proceed with normal emit
        }

        let diagnostics = get_diagnostics_of_any_program(
            files,
            true,
            &mut |file: Node| get_bind_diagnostics(p, ctx, file),
            &mut |file: Node| get_semantic_diagnostics(p, ctx, file),
            &mut || get_global_diagnostics(p, ctx),
            &mut |file: Node| get_declaration_diagnostics(p, ctx, file),
            true, // a `*compiler.Program`
        );
        if diagnostics.is_empty() {
            return None; // NoEmitOnError is enabled, but no diagnostics were found, so we can proceed with emitting
        }
        return Some(EmitResult {
            diagnostics,
            emit_skipped: true,
            ..EmitResult::default()
        });
    }
    if files.is_some() {
        return Some(EmitResult {
            emit_skipped: true,
            ..EmitResult::default()
        });
    }
    Some(EmitResult::default())
}

// Go: compiler/program.go:1809 IsGlobalTypingsFile
pub fn is_global_typings_file(p: &NewProgram, file_name: &str) -> bool {
    if !tspath::is_declaration_file_name(file_name) {
        return false;
    }
    // ts#64159 (program.go:1813): the file system's case sensitivity (N: the
    // zero-value `comparePathsOptions`).
    tspath::contains_path(
        &p.get_global_typings_cache_location(),
        file_name,
        &p.case_sensitivity(),
    )
}
