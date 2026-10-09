//! Go `internal/project/checkerpool.go`.
//!
//! PORT: the pool lives on the LSP dispatch thread (PORTING.md "Threads").
//! `p.mu` is dropped: every Go `p.mu.Lock()` is `self.mu_lock()`, which
//! only runs the request cleanups whose context is done (see
//! `register_request_cleanup`). Fields that Go changes under the lock are
//! `Cell`/`RefCell`. Go `*checker.Checker` is `Rc<RefCell<Checker>>` from
//! `ls_program::new_checker`; the caller's `borrow_mut` is the exclusive
//! use. The semaphores (`chan struct{}`) are counters: on one thread no
//! other goroutine can release a slot, so a send on a full semaphore is
//! `unreachable!`. The idle-cleanup timer is `gostd::local::after_func`.
//! Go `func()` release results are `ls_program::Release` (once).

use crate::project::prelude::*;

use crate::frontend::core_context::{self, CheckerLifetime};
use crate::program::ls_program::{self, Release};
use std::cell::Cell;
use std::rc::Weak;
use std::time::{Duration, Instant};

// Go: project/checkerpool.go:20 checkerHeldAnonymous
// checkerHeldAnonymous is a sentinel stored in heldBy when a checker is held
// by a caller that has no request ID (e.g., context.Background()). This
// distinguishes "held without ID" from "not held" (empty string).
pub const CHECKER_HELD_ANONYMOUS: &str = "<anonymous>";

// Go: project/checkerpool.go:22 CheckerPoolOptions
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CheckerPoolOptions {
    // MaxCheckers controls the total number of checker slots per project
    // (1 dedicated diagnostics checker + N-1 query checkers). Minimum 2.
    // Zero uses the default (4).
    pub max_checkers: i32,
    // IdleTimeout controls how long an idle checker is kept
    // before being disposed. Zero uses the default (30s).
    pub idle_timeout: Duration,
}

/// Go `chan struct{}` with a buffer, used as a counting semaphore
/// (`sem <- struct{}{}` claims a slot, `<-sem` releases one).
// PORT: on the dispatch thread nobody else can release a slot while a send
// waits, so Go would block forever there; that is `unreachable!`.
struct Semaphore {
    len: Cell<i32>,
    cap: i32,
}

impl Semaphore {
    fn new(cap: i32) -> Semaphore {
        Semaphore {
            len: Cell::new(0),
            cap,
        }
    }

    /// Go `sem <- struct{}{}`.
    fn send(&self) {
        if self.len.get() >= self.cap {
            unreachable!(
                "checkerpool: semaphore full on the dispatch thread (Go would block forever)"
            );
        }
        self.len.set(self.len.get() + 1);
    }

    /// Go `<-sem`.
    fn recv(&self) {
        if self.len.get() <= 0 {
            unreachable!(
                "checkerpool: semaphore empty on the dispatch thread (Go would block forever)"
            );
        }
        self.len.set(self.len.get() - 1);
    }
}

// Go: project/checkerpool.go:41 checkerPool
// checkerPool manages three categories of type checkers for a project:
//
//   - Diagnostics (index 0): A single checker for LSP diagnostics, providing
//     consistent walk order. Idle-cleaned.
//   - Temporary (indices 1+): Ephemeral query checkers for LSP operations.
//     Idle-cleaned after a configurable timeout.
//   - API: A single checker for API operations, providing stable
//     instance identity for reference equality on type/symbol handles.
//     Never idle-cleaned.
// PORT: Go `time.Time` zero values are `None`. `fileAssociations` is keyed
// by the file root `Node` (Go `*ast.SourceFile`).
pub struct CheckerPool {
    pub opts: CheckerPoolOptions,
    pub program: Rc<compiler::NewProgram>,

    // discarded is set when the pool's program has been replaced. The pool
    // remains fully functional but stops its idle-cleanup timer so that
    // query checkers are not disposed until the pool is GC'd.
    pub discarded: Cell<bool>,

    // checkers[0] is the diagnostics checker.
    // checkers[1:] are ephemeral query checkers.
    // All are idle-cleaned.
    pub checkers: RefCell<Vec<Option<Rc<RefCell<Checker>>>>>,
    // heldBy[i] is the requestID holding checker i, checkerHeldAnonymous, or "" if not held
    pub held_by: RefCell<Vec<String>>,
    // file → query checker index (1+)
    pub file_associations: RefCell<FxHashMap<Node, i32>>,
    // requestID → checker index
    pub request_associations: RefCell<FxHashMap<String, i32>>,

    // lastReleased tracks when each checker was last released.
    pub last_released: RefCell<Vec<Option<Instant>>>,

    // cleanupTimer is reset each time a checker is released.
    // When it fires, idle checkers are disposed.
    pub cleanup_timer: RefCell<Option<gostd::local::LocalTimer>>,

    // persistentChecker is the API checker. It is never idle-cleaned,
    // providing stable instance identity for API clients.
    pub persistent_checker: RefCell<Option<Rc<RefCell<Checker>>>>,
    pub persistent_held: Cell<bool>,

    diag_sem: Semaphore,
    query_sem: Semaphore,
    persistent_sem: Semaphore,

    pub log: Rc<dyn Fn(&str)>,
    pub global_diag_accumulated: RefCell<Vec<Diagnostic>>,
    pub global_diag_changed: Cell<bool>,
    // per-checker count of globals last seen
    pub global_diag_checker_count: RefCell<Vec<i32>>,

    /// The `context.AfterFunc` callbacks of `register_request_cleanup` that
    /// have not run: each request id with the done channel of its request
    /// context. `mu_lock` runs the callbacks whose channel is closed.
    request_cleanups: RefCell<Vec<(String, gostd::context::Done)>>,
    /// The Go pointer `p`, for the release and timer closures.
    this: Weak<CheckerPool>,
}

// Go: project/checkerpool.go:84 newCheckerPool
// PORT: Go `log func(msg string)` (nil-able) is `Option<Rc<dyn Fn(&str)>>`.
pub fn new_checker_pool(
    opts: CheckerPoolOptions,
    program: Rc<compiler::NewProgram>,
    log: Option<Rc<dyn Fn(&str)>>,
) -> Rc<CheckerPool> {
    let mut opts = opts;
    if opts.max_checkers <= 0 {
        opts.max_checkers = 4;
    } else if opts.max_checkers < 2 {
        opts.max_checkers = 2; // at least 1 diagnostics + 1 query checker
    }
    // PORT: a Go `time.Duration` can be negative; `Duration` can not.
    if opts.idle_timeout.is_zero() {
        opts.idle_timeout = Duration::from_secs(30);
    }
    let query_slots = opts.max_checkers - 1;
    let max_checkers = opts.max_checkers as usize;
    // Go: if pool.log == nil { pool.log = func(msg string) {} }
    let log = match log {
        Some(log) => log,
        None => Rc::new(|_msg: &str| {}),
    };
    Rc::new_cyclic(|this| CheckerPool {
        program,
        opts,
        discarded: Cell::new(false),
        checkers: RefCell::new(vec![None; max_checkers]),
        held_by: RefCell::new(vec![String::new(); max_checkers]),
        file_associations: RefCell::new(FxHashMap::default()),
        request_associations: RefCell::new(FxHashMap::default()),
        last_released: RefCell::new(vec![None; max_checkers]),
        cleanup_timer: RefCell::new(None),
        persistent_checker: RefCell::new(None),
        persistent_held: Cell::new(false),
        diag_sem: Semaphore::new(1),
        query_sem: Semaphore::new(query_slots),
        persistent_sem: Semaphore::new(1),
        log,
        global_diag_accumulated: RefCell::new(Vec::new()),
        global_diag_changed: Cell::new(false),
        global_diag_checker_count: RefCell::new(vec![0; max_checkers]),
        request_cleanups: RefCell::new(Vec::new()),
        this: this.clone(),
    })
}

// Go: project/checkerpool.go:116 holdTag
// holdTag returns the value to store in heldBy for the given request ID.
pub fn hold_tag(request_id: &str) -> String {
    if request_id.is_empty() {
        return CHECKER_HELD_ANONYMOUS.to_string();
    }
    request_id.to_string()
}

// Go: project/checkerpool.go:82 `var _ compiler.CheckerPool = (*checkerPool)(nil)`
impl ls_program::CheckerPool for CheckerPool {
    // Go: project/checkerpool.go:123 checkerPool.GetChecker
    fn get_checker(&self, ctx: &Context, file: Node) -> (Rc<RefCell<Checker>>, Release) {
        let lifetime = core_context::get_checker_lifetime(ctx);
        let mut request_id = core_context::get_request_id(ctx);

        // Request affinity is cleaned up via context.AfterFunc when the request
        // context is done. If the context can never be canceled (ctx.Done() == nil,
        // e.g. context.Background()), that cleanup would never run and
        // requestAssociations would grow unboundedly, so disable affinity entirely.
        if ctx.done().is_none() {
            request_id = String::new();
        }

        match lifetime {
            CheckerLifetime::DIAGNOSTICS => self.get_diagnostics_checker(ctx, &request_id),
            CheckerLifetime::API => self.get_persistent_checker(),
            _ => self.get_query_checker(ctx, &request_id, file),
        }
    }
}

impl CheckerPool {
    /// Go `p` as a shared pointer.
    fn this_rc(&self) -> Rc<CheckerPool> {
        self.this.upgrade().expect("checkerPool: pool dropped")
    }

    /// Go `p.mu.Lock()`. Tests call it where Go tests lock `pool.mu`.
    // PORT: the mutex is dropped (one thread). In Go, a request cleanup
    // (`register_request_cleanup`) whose context is done waits for `p.mu`
    // and then deletes its request association. Here each lock point first
    // runs the deletes of all requests whose context is done: the Go
    // schedule in which the waiting cleanups get the lock next. Only lock
    // holders read `requestAssociations`, so no reader can tell.
    pub fn mu_lock(&self) {
        self.request_cleanups
            .borrow_mut()
            .retain(|(request_id, done)| {
                if !done.is_closed() {
                    return true;
                }
                // Go: the registerRequestCleanup callback body.
                self.request_associations.borrow_mut().remove(request_id);
                false
            });
    }

    // Go: project/checkerpool.go:158 checkerPool.tryReacquireForRequest (ts#64543, at fed0bf24149f)
    // tryReacquireForRequest claims a semaphore slot, then checks whether the given
    // request has an idle associated checker. The caller must provide the
    // appropriate semaphore channel and indicate whether this is a diagnostics
    // request (isDiag). If the associated checker is in the wrong category
    // (e.g. a diagnostics index for a query request), the association is deleted
    // and normal acquisition proceeds.
    //
    // Request affinity is only a preference for an idle checker, not permission to
    // reuse a held checker: concurrent acquisitions can share the same request ID.
    // Returns (checker, release, true) if the checker was reclaimed.
    // Returns (nil, nil, false) if the caller must proceed with
    // normal acquisition — in this case, a semaphore slot has already been claimed.
    // Must NOT be called with p.mu held.
    // PORT: a held checker's request no longer skips the slot, so a nested
    // acquisition on a full semaphore is `Semaphore::send`'s `unreachable!`
    // (Go blocks; compiler/checkerpool.go:20 says acquisitions are not
    // reentrant).
    fn try_reacquire_for_request(
        &self,
        request_id: &str,
        sem: &Semaphore,
        is_diag: bool,
    ) -> (Option<Rc<RefCell<Checker>>>, Option<Release>, bool) {
        sem.send();
        if request_id.is_empty() {
            return (None, None, false);
        }

        self.mu_lock();
        let index = self.request_associations.borrow().get(request_id).copied();
        let Some(index) = index else {
            return (None, None, false);
        };

        // Validate that the associated index matches the expected category.
        // Index 0 is for diagnostics; indices 1+ are for queries.
        if (is_diag && index != 0) || (!is_diag && index == 0) {
            self.request_associations.borrow_mut().remove(request_id);
            return (None, None, false);
        }

        let c = self.checkers.borrow()[index as usize].clone();
        let Some(c) = c else {
            self.request_associations.borrow_mut().remove(request_id);
            return (None, None, false);
        };

        if self.held_by.borrow()[index as usize].is_empty() {
            self.held_by.borrow_mut()[index as usize] = request_id.to_string();
            let release = self.create_release(request_id, index, c.clone());
            return (Some(c), Some(release), true);
        }

        (None, None, false)
    }

    // Go: project/checkerpool.go:220 checkerPool.getDiagnosticsChecker
    // getDiagnosticsChecker returns the dedicated diagnostics checker (index 0).
    // Creates it on first use. Blocks on diagSem if it's currently in use.
    pub fn get_diagnostics_checker(
        &self,
        ctx: &Context,
        request_id: &str,
    ) -> (Rc<RefCell<Checker>>, Release) {
        const DIAG_INDEX: i32 = 0;

        if let (Some(c), Some(release), true) =
            self.try_reacquire_for_request(request_id, &self.diag_sem, true)
        {
            return (c, release);
        }

        // Token consumed — proceed with normal acquisition.
        self.mu_lock();

        let missing = self.checkers.borrow()[DIAG_INDEX as usize].is_none();
        if missing {
            (self.log)("checkerpool: Creating diagnostics checker");
            let c = Rc::new(RefCell::new(ls_program::new_checker(&self.program)));
            self.checkers.borrow_mut()[DIAG_INDEX as usize] = Some(c);
        }

        let c = self.checkers.borrow()[DIAG_INDEX as usize]
            .clone()
            .expect("the diagnostics checker was created above");
        self.held_by.borrow_mut()[DIAG_INDEX as usize] = hold_tag(request_id);
        (self.log)(&format!(
            "checkerpool: Acquired diagnostics checker for request {}",
            hold_tag(request_id)
        ));
        if !request_id.is_empty() {
            let already_registered = self.request_associations.borrow().contains_key(request_id);
            if !already_registered {
                self.request_associations
                    .borrow_mut()
                    .insert(request_id.to_string(), DIAG_INDEX);
                self.register_request_cleanup(ctx, request_id);
            }
        }
        let release = self.create_release(request_id, DIAG_INDEX, c.clone());
        (c, release)
    }

    // Go: project/checkerpool.go:252 checkerPool.getQueryChecker
    // getQueryChecker returns an ephemeral query checker from indices 1+.
    // Uses request affinity, then file affinity, then finds/creates.
    // Blocks on querySem if all query slots are in use.
    pub fn get_query_checker(
        &self,
        ctx: &Context,
        request_id: &str,
        file: Node,
    ) -> (Rc<RefCell<Checker>>, Release) {
        if let (Some(c), Some(release), true) =
            self.try_reacquire_for_request(request_id, &self.query_sem, false)
        {
            return (c, release);
        }

        // Token consumed — proceed with normal acquisition.
        self.mu_lock();

        // Try file affinity.
        if file.is_some() {
            let index = self.file_associations.borrow().get(&file).copied();
            if let Some(index) = index
                && index > 0
            {
                let c = self.checkers.borrow()[index as usize].clone();
                if let Some(c) = c
                    && self.held_by.borrow()[index as usize].is_empty()
                {
                    self.held_by.borrow_mut()[index as usize] = hold_tag(request_id);
                    if !request_id.is_empty() {
                        let already_registered =
                            self.request_associations.borrow().contains_key(request_id);
                        if !already_registered {
                            self.request_associations
                                .borrow_mut()
                                .insert(request_id.to_string(), index);
                            self.register_request_cleanup(ctx, request_id);
                        }
                    }
                    let release = self.create_release(request_id, index, c.clone());
                    return (c, release);
                }
            }
        }

        // Find any available query checker or create one.
        let (c, index) = self.find_or_create_query_checker_locked();
        self.held_by.borrow_mut()[index as usize] = hold_tag(request_id);
        (self.log)(&format!(
            "checkerpool: Acquired query checker {} for request {}",
            index,
            hold_tag(request_id)
        ));
        if !request_id.is_empty() {
            let already_registered = self.request_associations.borrow().contains_key(request_id);
            if !already_registered {
                self.request_associations
                    .borrow_mut()
                    .insert(request_id.to_string(), index);
                self.register_request_cleanup(ctx, request_id);
            }
        }
        if file.is_some() {
            self.file_associations.borrow_mut().insert(file, index);
        }
        let release = self.create_release(request_id, index, c.clone());
        (c, release)
    }

    // Go: project/checkerpool.go:296 checkerPool.findOrCreateQueryCheckerLocked
    // findOrCreateQueryCheckerLocked returns an idle query checker or creates one
    // in the first empty slot. The semaphore guarantees at least one slot is
    // available. Must be called with p.mu held.
    pub fn find_or_create_query_checker_locked(&self) -> (Rc<RefCell<Checker>>, i32) {
        let len = self.checkers.borrow().len();
        // Prefer an existing idle checker.
        for i in 1..len {
            let c = self.checkers.borrow()[i].clone();
            if let Some(c) = c
                && self.held_by.borrow()[i].is_empty()
            {
                return (c, i as i32);
            }
        }
        // Create in the first empty slot.
        for i in 1..len {
            let empty = self.checkers.borrow()[i].is_none();
            if empty {
                (self.log)(&format!("checkerpool: Creating query checker {i}"));
                let c = Rc::new(RefCell::new(ls_program::new_checker(&self.program)));
                self.checkers.borrow_mut()[i] = Some(c.clone());
                return (c, i as i32);
            }
        }
        crate::core::go_panic(
            "checkerpool: no available query slot despite holding semaphore token".to_string(),
        );
    }

    // Go: project/checkerpool.go:315 checkerPool.getPersistentChecker
    pub fn get_persistent_checker(&self) -> (Rc<RefCell<Checker>>, Release) {
        self.persistent_sem.send();
        self.mu_lock();

        let missing = self.persistent_checker.borrow().is_none();
        if missing {
            (self.log)("checkerpool: Creating persistent checker");
            // PORT: the API hands it symbols of other projects
            // (`ls_program::new_api_checker`).
            let c = Rc::new(RefCell::new(ls_program::new_api_checker(&self.program)));
            *self.persistent_checker.borrow_mut() = Some(c);
        }

        let c = self
            .persistent_checker
            .borrow()
            .clone()
            .expect("the persistent checker was created above");
        self.persistent_held.set(true);

        let p = self.this_rc();
        let released = c.clone();
        // Go: sync.OnceFunc(..) (PORT: `Release` runs once)
        let release = Release::new(move || {
            let c = released;
            p.mu_lock();
            p.persistent_held.set(false);
            let was_canceled = c.borrow().was_canceled();
            if was_canceled {
                // A canceled checker panics on reuse, so drop it; the next API
                // acquisition will create a fresh persistent checker.
                (p.log)("checkerpool: Persistent checker was canceled, disposing");
                let is_persistent = p
                    .persistent_checker
                    .borrow()
                    .as_ref()
                    .is_some_and(|persistent| Rc::ptr_eq(persistent, &c));
                if is_persistent {
                    *p.persistent_checker.borrow_mut() = None;
                }
            }
            p.persistent_sem.recv();
        });
        (c, release)
    }

    // Go: project/checkerpool.go:345 checkerPool.createRelease
    pub fn create_release(&self, request_id: &str, index: i32, c: Rc<RefCell<Checker>>) -> Release {
        let p = self.this_rc();
        let request_id = request_id.to_string();
        // Go: sync.OnceFunc(..) (PORT: `Release` runs once)
        Release::new(move || {
            p.mu_lock();

            let was_canceled = c.borrow().was_canceled();
            if was_canceled {
                // Canceled checkers must be disposed.
                (p.log)(&format!(
                    "checkerpool: Checker {} for request {} was canceled, disposing",
                    index,
                    hold_tag(&request_id)
                ));
                p.dispose_checker_locked(index, &c);
            } else {
                // Query checkers can produce incidental errors while serializing types.
                // ts#64452
                if index == 0 {
                    p.merge_global_diagnostics_from_checker_locked(index, &c);
                }
                p.held_by.borrow_mut()[index as usize] = String::new();
                p.last_released.borrow_mut()[index as usize] = Some(Instant::now());
                if !p.discarded.get() {
                    p.schedule_cleanup_locked();
                }
                // If discarded, skip scheduling cleanup — checkers stay alive
                // until the pool is garbage collected so that API clients can
                // continue resolving type/symbol handles.
            }

            // Unlock before releasing the semaphore slot. If we received from
            // the channel while holding p.mu, a woken goroutine could immediately
            // try to acquire p.mu, risking priority inversion or unnecessary
            // contention.

            // Release the semaphore slot.
            if index == 0 {
                p.diag_sem.recv();
            } else {
                p.query_sem.recv();
            }
        })
    }

    // Go: project/checkerpool.go:387 checkerPool.registerRequestCleanup
    // registerRequestCleanup uses context.AfterFunc to delete the request
    // association when the request context is done. This prevents the map
    // from growing unboundedly with completed request IDs.
    // Must be called with p.mu held; the cleanup runs asynchronously.
    // PORT: `gostd::context::after_func` starts an OS thread when the
    // context is done, which is one thread per request. The pool keeps the
    // request's done channel instead, and the next `mu_lock` after the
    // channel closes runs the delete (see there). A context with no done
    // channel is never canceled, so Go never runs the callback; the port
    // keeps nothing.
    pub fn register_request_cleanup(&self, ctx: &Context, request_id: &str) {
        if let Some(done) = ctx.done() {
            self.request_cleanups
                .borrow_mut()
                .push((request_id.to_string(), done));
        }
    }

    // Go: project/checkerpool.go:399 checkerPool.scheduleCleanupLocked
    // scheduleCleanupLocked resets (or starts) the cleanup timer so it fires at
    // the earliest pending checker-expiration deadline among all currently idle,
    // unheld checkers.
    // Must be called with p.mu held. Must NOT be called on discarded pools.
    pub fn schedule_cleanup_locked(&self) {
        let mut earliest_deadline: Option<Instant> = None;
        {
            let checkers = self.checkers.borrow();
            let held_by = self.held_by.borrow();
            let last_released = self.last_released.borrow();
            for i in 0..checkers.len() {
                if checkers[i].is_none() || !held_by[i].is_empty() || last_released[i].is_none() {
                    continue;
                }
                let released = last_released[i].expect("checked above");
                let deadline = released + self.opts.idle_timeout;
                if earliest_deadline.is_none_or(|earliest| deadline < earliest) {
                    earliest_deadline = Some(deadline);
                }
            }
        }
        let Some(earliest_deadline) = earliest_deadline else {
            // No idle checkers remain — stop the timer if it exists.
            let timer = self.cleanup_timer.borrow_mut().take();
            if let Some(timer) = timer {
                timer.stop();
            }
            return;
        };
        // PORT: Go `time.Until` can be negative; `Duration` saturates at zero.
        let mut delay = earliest_deadline.saturating_duration_since(Instant::now());
        if delay.is_zero() {
            delay = Duration::from_millis(1);
        }
        let has_timer = self.cleanup_timer.borrow().is_some();
        if has_timer {
            if let Some(timer) = self.cleanup_timer.borrow().as_ref() {
                timer.reset(delay);
            }
        } else {
            // PORT: the timer holds a `Weak` to the pool, so an armed timer
            // does not keep the pool alive through a reference cycle.
            let p = self.this.clone();
            let timer = gostd::local::after_func(
                delay,
                Box::new(move || {
                    if let Some(p) = p.upgrade() {
                        p.cleanup_idle_checkers();
                    }
                }),
            );
            *self.cleanup_timer.borrow_mut() = Some(timer);
        }
    }

    // Go: project/checkerpool.go:431 checkerPool.cleanupIdleCheckers
    // cleanupIdleCheckers disposes checkers that have been idle for longer than
    // the idle timeout. The API checker is separate and never idle-cleaned.
    pub fn cleanup_idle_checkers(&self) {
        self.mu_lock();
        // The timer callback may already have been in flight when Discard() called
        // Stop() (which does not guarantee the callback won't run). Bail out without
        // rescheduling so a discarded pool doesn't keep itself alive via a new timer.
        if self.discarded.get() {
            return;
        }
        let now = Instant::now();
        let len = self.checkers.borrow().len();
        for i in 0..len {
            let c = self.checkers.borrow()[i].clone();
            let Some(c) = c else {
                continue;
            };
            if !self.held_by.borrow()[i].is_empty() {
                continue;
            }
            let Some(released) = self.last_released.borrow()[i] else {
                continue;
            };
            let idle = now.saturating_duration_since(released);
            if idle >= self.opts.idle_timeout {
                // PORT: Go `%v` of a Duration; the log text is not compared.
                (self.log)(&format!(
                    "checkerpool: Disposing idle checker {i} (idle {idle:?})"
                ));
                self.dispose_checker_locked(i as i32, &c);
            }
        }
        // Reschedule for any remaining idle-but-not-yet-expired checkers.
        // scheduleCleanupLocked will Reset the existing timer rather than
        // creating a new one, avoiding goroutine leaks.
        self.schedule_cleanup_locked();
    }

    // Go: project/checkerpool.go:463 checkerPool.disposeCheckerLocked
    // disposeCheckerLocked removes a checker from the pool and clears all associations
    // (file and request) that reference it. Must be called with p.mu held.
    pub fn dispose_checker_locked(&self, index: i32, c: &Rc<RefCell<Checker>>) {
        let i = index as usize;
        debug_assert!(
            self.checkers.borrow()[i]
                .as_ref()
                .is_some_and(|checker| Rc::ptr_eq(checker, c))
        );
        self.checkers.borrow_mut()[i] = None;
        self.held_by.borrow_mut()[i] = String::new();
        self.global_diag_checker_count.borrow_mut()[i] = 0;
        self.last_released.borrow_mut()[i] = None;
        // Go: delete matching entries while ranging over the map.
        self.file_associations
            .borrow_mut()
            .retain(|_file, idx| *idx != index);
        self.request_associations
            .borrow_mut()
            .retain(|_req, idx| *idx != index);
    }

    // Go: project/checkerpool.go:484 checkerPool.mergeGlobalDiagnosticsFromCheckerLocked
    // mergeGlobalDiagnosticsFromCheckerLocked checks if the given checker has produced new global
    // diagnostics since the last time we looked, and if so merges them into the accumulated set.
    // Must be called with p.mu held.
    pub fn merge_global_diagnostics_from_checker_locked(
        &self,
        index: i32,
        c: &Rc<RefCell<Checker>>,
    ) {
        let i = index as usize;
        let globals = c.borrow_mut().get_global_diagnostics();
        if globals.len() as i32 == self.global_diag_checker_count.borrow()[i] {
            return;
        }
        self.global_diag_checker_count.borrow_mut()[i] = globals.len() as i32;
        let before = self.global_diag_accumulated.borrow().len();
        let mut accumulated = self.global_diag_accumulated.borrow().clone();
        accumulated.extend(globals);
        *self.global_diag_accumulated.borrow_mut() = sort_and_deduplicate_diagnostics(accumulated);
        if self.global_diag_accumulated.borrow().len() != before {
            self.global_diag_changed.set(true);
        }
    }

    // Go: project/checkerpool.go:499 checkerPool.GetGlobalDiagnostics
    // GetGlobalDiagnostics returns the global diagnostics accumulated from the dedicated
    // diagnostics checker across its instances during this pool's lifetime.
    pub fn get_global_diagnostics(&self) -> Vec<Diagnostic> {
        self.mu_lock();
        self.global_diag_accumulated.borrow().clone()
    }

    // Go: project/checkerpool.go:507 checkerPool.TakeNewGlobalDiagnostics
    // TakeNewGlobalDiagnostics reports whether new global diagnostics have been
    // accumulated since the last call, and resets the flag.
    pub fn take_new_global_diagnostics(&self) -> bool {
        self.mu_lock();
        let changed = self.global_diag_changed.get();
        self.global_diag_changed.set(false);
        changed
    }

    // Go: project/checkerpool.go:519 checkerPool.Discard
    // Discard signals that this pool's program has been replaced. The pool
    // remains functional but stops its idle-cleanup timer so that checkers
    // are not disposed until the pool is GC'd. The API checker is unaffected
    // since it is never idle-cleaned.
    pub fn discard(&self) {
        self.mu_lock();
        if self.discarded.get() {
            return; // already discarded
        }
        (self.log)("checkerpool: Discarding pool, stopping idle cleanup");
        self.discarded.set(true);
        let timer = self.cleanup_timer.borrow_mut().take();
        if let Some(timer) = timer {
            timer.stop();
        }
    }
}

// Go: project/checkerpool.go:533 noop (at 673a5f17d713; removed by ts#64543)

// Not in Go: the GC frees the checkers of a released pool in the background.
// PERF (freecheck1): a language server edit frees the old program's pool on
// the first request after the edit, before its answer: 1 or 2 checkers (hono
// 36 MiB each; per edit hono 2.7 ms when the memory is hot and 6.5 ms after a
// client pause, query-core 0.3 ms). In the first release after a client pause
// each checker waits for `drop_garbage` (`gostd::local::drop_after_pause`).
// One value per checker, so a message that comes during the frees waits for
// one checker at most. A checker that is still held elsewhere is only
// decremented later.
impl Drop for CheckerPool {
    fn drop(&mut self) {
        let checkers = std::mem::take(self.checkers.get_mut());
        let persistent = self.persistent_checker.get_mut().take();
        for checker in checkers.into_iter().flatten().chain(persistent) {
            gostd::local::drop_after_pause(Box::new(checker));
        }
    }
}
