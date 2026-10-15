//! Go `context` (go1.27.1 `src/context/context.go`; it differs from go1.26.8
//! only in the package comment).
//!
//! PORT: Go `Context` is an interface. The port has only the standard
//! implementations (background, TODO, cancel, timer, value, withoutCancel),
//! so `Context` is a closed enum of shared handles. Code paths that exist in
//! Go only for custom `Context` implementations (the `afterFuncer` parent,
//! `stopCtx`, the watcher goroutine in `propagateCancel`, the `default`
//! branch of `value`) cannot run and are marked where they would be.
//!
//! A Go channel `<-chan struct{}` from `Done()` is `Done`. Go `time.Time` is
//! `std::time::Instant`.

use crate::prelude::*;

use crate::gostd::errors::{self, GoError};
use std::any::{Any, TypeId};
use std::collections::BTreeMap;
use std::fmt;
use std::marker::PhantomData;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, LazyLock, Mutex, MutexGuard, Once, OnceLock};
use std::time::{Duration, Instant};

// PORT: Go mutexes do not poison. A panic while a lock is held leaves the
// data as it is, as in Go.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

// PORT: Go `go f()`.
fn go(f: Box<dyn FnOnce() + Send>) {
    std::thread::Builder::new()
        .name("goroutine".to_string())
        .stack_size(crate::gostd::stack::max_stack_size())
        .spawn(f)
        .expect("failed to spawn context after_func thread");
}

struct DeadlineScheduler {
    state: Mutex<DeadlineSchedulerState>,
    cond: Condvar,
}

#[derive(Default)]
struct DeadlineSchedulerState {
    next_id: u64,
    due: BTreeMap<(Instant, u64), Box<dyn FnOnce() + Send>>,
    thread_running: bool,
}

static DEADLINE_SCHEDULER: LazyLock<DeadlineScheduler> = LazyLock::new(|| DeadlineScheduler {
    state: Mutex::new(DeadlineSchedulerState::default()),
    cond: Condvar::new(),
});

struct DeadlineTimer {
    key: (Instant, u64),
}

impl DeadlineTimer {
    fn schedule(when: Instant, f: impl FnOnce() + Send + 'static) -> Self {
        let scheduler = &*DEADLINE_SCHEDULER;
        let mut state = lock(&scheduler.state);
        let id = state.next_id;
        state.next_id += 1;
        let key = (when, id);
        let is_first = state.due.first_key_value().is_none_or(|(&first, _)| key < first);
        state.due.insert(key, Box::new(f));
        if !state.thread_running {
            state.thread_running = true;
            drop(state);
            std::thread::Builder::new()
                .name("context-timer".to_string())
                .spawn(run_deadline_scheduler)
                .expect("failed to spawn context-timer thread");
        } else if is_first {
            scheduler.cond.notify_all();
        }
        Self { key }
    }

    fn stop(self) {
        let mut state = lock(&DEADLINE_SCHEDULER.state);
        state.due.remove(&self.key);
    }
}

fn run_deadline_scheduler() {
    let scheduler = &*DEADLINE_SCHEDULER;
    let mut state = lock(&scheduler.state);
    loop {
        let Some((&(when, _), _)) = state.due.first_key_value() else {
            state = scheduler
                .cond
                .wait(state)
                .unwrap_or_else(|e| e.into_inner());
            continue;
        };
        let now = Instant::now();
        if now < when {
            state = scheduler
                .cond
                .wait_timeout(state, when - now)
                .unwrap_or_else(|e| e.into_inner())
                .0;
            continue;
        }
        if let Some((_, f)) = state.due.pop_first() {
            drop(state);
            f();
            state = lock(&scheduler.state);
        }
    }
}

/// A Context carries a deadline, a cancellation signal, and other values
/// across API boundaries.
///
/// Context's methods may be called by multiple goroutines simultaneously.
#[derive(Clone)]
pub struct Context(Ctx);

#[derive(Clone)]
enum Ctx {
    /// Go `backgroundCtx`.
    Background,
    /// Go `todoCtx`.
    Todo,
    /// Go `*cancelCtx`.
    Cancel(Arc<CancelCtx>),
    /// Go `*timerCtx`.
    Timer(Arc<TimerCtx>),
    /// Go `*valueCtx`.
    Value(Arc<ValueCtx>),
    /// Go `withoutCancelCtx`.
    WithoutCancel(Arc<WithoutCancelCtx>),
}

impl Context {
    /// Go `ctx.Deadline()`: the time when work done on behalf of this
    /// context should be canceled. `None` when no deadline is set.
    pub fn deadline(&self) -> Option<Instant> {
        match &self.0 {
            // Go: context/context.go:184 emptyCtx.Deadline
            Ctx::Background | Ctx::Todo => None,
            // Go: the embedded parent Context.
            Ctx::Cancel(c) => c.context.deadline(),
            // Go: context/context.go:670 timerCtx.Deadline
            Ctx::Timer(c) => Some(c.deadline),
            // Go: the embedded parent Context.
            Ctx::Value(c) => c.context.deadline(),
            // Go: context/context.go:597 withoutCancelCtx.Deadline
            Ctx::WithoutCancel(_) => None,
        }
    }

    /// Go `ctx.Done()`: a channel that's closed when work done on behalf of
    /// this context should be canceled. `None` is Go's nil channel: this
    /// context can never be canceled. Successive calls return the same
    /// channel.
    pub fn done(&self) -> Option<Done> {
        match &self.0 {
            // Go: context/context.go:188 emptyCtx.Done
            Ctx::Background | Ctx::Todo => None,
            Ctx::Cancel(c) => Some(c.done()),
            Ctx::Timer(c) => Some(c.cancel_ctx.done()),
            // Go: the embedded parent Context.
            Ctx::Value(c) => c.context.done(),
            // Go: context/context.go:601 withoutCancelCtx.Done
            Ctx::WithoutCancel(_) => None,
        }
    }

    /// Go `ctx.Err()`. If Done is not yet closed, Err returns nil. If Done
    /// is closed, Err returns a non-nil error explaining why:
    /// DeadlineExceeded if the context's deadline passed, or Canceled if the
    /// context was canceled for some other reason.
    pub fn err(&self) -> Option<GoError> {
        match &self.0 {
            // Go: context/context.go:192 emptyCtx.Err
            Ctx::Background | Ctx::Todo => None,
            Ctx::Cancel(c) => c.err(),
            Ctx::Timer(c) => c.cancel_ctx.err(),
            // Go: the embedded parent Context.
            Ctx::Value(c) => c.context.err(),
            // Go: context/context.go:605 withoutCancelCtx.Err
            Ctx::WithoutCancel(_) => None,
        }
    }

    /// Go `ctx.Value(key)`: the value associated with this context for
    /// key, or nil if no value is associated with key.
    ///
    /// PORT: Go keys are values of unexported types; the port uses one
    /// `static ContextKey<T>` per Go key. The value type is fixed by the key,
    /// so Go's type assertion on the result always succeeds.
    pub fn value<T: Send + Sync + 'static>(&self, key: &'static ContextKey<T>) -> Option<Arc<T>> {
        // PORT: every standard `Value` method ends in the package function
        // `value` (the valueCtx and cancelCtx key checks are its first loop
        // step), so the method calls it directly.
        match value(self, &KeyRef::User(key.id())) {
            Some(ValueRef::User(v)) => v.downcast::<T>().ok(),
            _ => None,
        }
    }

    /// Go `String()` of the standard contexts (for debugging).
    pub fn string(&self) -> String {
        match &self.0 {
            // Go: context/context.go:202 backgroundCtx.String
            Ctx::Background => "context.Background".to_string(),
            // Go: context/context.go:208 todoCtx.String
            Ctx::Todo => "context.TODO".to_string(),
            Ctx::Cancel(c) => c.string(),
            Ctx::Timer(c) => c.string(),
            Ctx::Value(c) => c.string(),
            Ctx::WithoutCancel(c) => c.string(),
        }
    }
}

impl fmt::Debug for Context {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.string())
    }
}

/// A typed key for `with_value` and `Context::value`. Declare one static per
/// Go key: `pub static KEY: ContextKey<T> = ContextKey::new("goName");`.
/// Two keys are equal only when they are the same static.
pub struct ContextKey<T> {
    name: &'static str,
    _value: PhantomData<fn() -> T>,
}

impl<T: 'static> ContextKey<T> {
    pub const fn new(name: &'static str) -> ContextKey<T> {
        ContextKey {
            name,
            _value: PhantomData,
        }
    }

    /// The Go name of the key (used by `String()`).
    pub fn name(&self) -> &'static str {
        self.name
    }

    fn id(&'static self) -> KeyId {
        KeyId {
            addr: self as *const ContextKey<T> as usize,
            type_id: TypeId::of::<T>(),
        }
    }
}

impl<T> fmt::Debug for ContextKey<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name)
    }
}

/// Go `key == ctx.key` for a user key: the same static.
#[derive(Clone, Copy, PartialEq, Eq)]
struct KeyId {
    addr: usize,
    type_id: TypeId,
}

/// A key passed to Go `value`: a user key or `&cancelCtxKey`.
enum KeyRef {
    /// Go `&cancelCtxKey`.
    CancelCtx,
    User(KeyId),
}

/// A result of Go `value`.
enum ValueRef {
    /// The `*cancelCtx` found for `&cancelCtxKey`.
    CancelCtx(CancelCtxRef),
    User(Arc<dyn Any + Send + Sync>),
}

/// A Go `*cancelCtx`: a plain cancelCtx or the one embedded in a timerCtx.
#[derive(Clone)]
enum CancelCtxRef {
    Cancel(Arc<CancelCtx>),
    Timer(Arc<TimerCtx>),
}

impl CancelCtxRef {
    fn get(&self) -> &CancelCtx {
        match self {
            CancelCtxRef::Cancel(c) => c,
            CancelCtxRef::Timer(c) => &c.cancel_ctx,
        }
    }
}

/// Go `<-chan struct{}` returned by `Context::done`.
///
/// PORT: a Go channel is waited on with `select`. `Done` offers the waits
/// the port needs: `is_closed` (a `select` with `default`), `wait`,
/// `wait_timeout`, and `register_waker` so that code blocked on its own
/// condition variable wakes when the channel closes.
#[derive(Clone)]
pub struct Done(Arc<DoneInner>);

impl fmt::Debug for Done {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Done(closed: {})", self.is_closed())
    }
}

struct DoneInner {
    state: Mutex<DoneState>,
    cond: Condvar,
}

struct DoneState {
    closed: bool,
    next_waker: u64,
    wakers: Vec<(u64, Box<dyn FnOnce() + Send>)>,
}

impl Done {
    /// Go `make(chan struct{})`.
    fn new() -> Done {
        Done(Arc::new(DoneInner {
            state: Mutex::new(DoneState {
                closed: false,
                next_waker: 0,
                wakers: Vec::new(),
            }),
            cond: Condvar::new(),
        }))
    }

    /// Whether the channel is closed (Go `select { case <-done: ...; default: }`).
    pub fn is_closed(&self) -> bool {
        lock(&self.0.state).closed
    }

    /// Go `<-done`: blocks until the channel is closed.
    pub fn wait(&self) {
        let mut state = lock(&self.0.state);
        while !state.closed {
            state = self.0.cond.wait(state).unwrap_or_else(|e| e.into_inner());
        }
    }

    /// Waits at most `timeout` for the channel to close. Returns whether it
    /// is closed.
    pub fn wait_timeout(&self, timeout: Duration) -> bool {
        match Instant::now().checked_add(timeout) {
            Some(deadline) => self.wait_deadline(deadline),
            None { .. } => {
                self.wait();
                true
            }
        }
    }

    /// Waits until the channel closes or `deadline` passes. Returns whether
    /// it is closed.
    pub fn wait_deadline(&self, deadline: Instant) -> bool {
        let mut state = lock(&self.0.state);
        while !state.closed {
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            state = self
                .0
                .cond
                .wait_timeout(state, deadline - now)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        true
    }

    /// Registers `f` to run once when the channel closes. Returns an id for
    /// `unregister_waker`, or `None` when the channel is already closed (then
    /// `f` is dropped without running; the caller checks `ctx.err()`).
    ///
    /// `f` runs on the thread that cancels the context, after the channel is
    /// marked closed and the context locks are released. It should only
    /// signal (for example lock a mutex and notify a condition variable).
    pub fn register_waker(&self, f: impl FnOnce() + Send + 'static) -> Option<u64> {
        let mut state = lock(&self.0.state);
        if state.closed {
            return None;
        }
        let id = state.next_waker;
        state.next_waker += 1;
        state.wakers.push((id, Box::new(f)));
        Some(id)
    }

    /// Removes a waker that has not run yet. Returns whether it was removed.
    pub fn unregister_waker(&self, id: u64) -> bool {
        let mut state = lock(&self.0.state);
        let before = state.wakers.len();
        state.wakers.retain(|(waker_id, _)| *waker_id != id);
        state.wakers.len() != before
    }

    /// Go channel equality (`done == closedchan`).
    pub fn ptr_eq(&self, other: &Done) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    /// Go `close(d)`.
    fn close(&self) {
        let wakers = {
            let mut state = lock(&self.0.state);
            if state.closed {
                panic!("close of closed channel");
            }
            state.closed = true;
            std::mem::take(&mut state.wakers)
        };
        self.0.cond.notify_all();
        let wakers: Vec<Box<dyn FnOnce() + Send>> =
            wakers.into_iter().map(|(_, waker)| waker).collect();
        let wakers = DEFERRED_WAKERS.with(|deferred| match &mut *deferred.borrow_mut() {
            Some(deferred) => {
                deferred.extend(wakers);
                Vec::new()
            }
            None => wakers,
        });
        for waker in wakers {
            waker();
        }
    }
}

thread_local! {
    /// PORT: wakers of channels that the cancel running on this thread
    /// closed. They run when the outermost `cancelCtx.cancel` has released
    /// its lock, so a waker never runs while this thread holds a context
    /// lock.
    static DEFERRED_WAKERS: RefCell<Option<Vec<Box<dyn FnOnce() + Send>>>> = const { RefCell::new(None) };
}

/// Starts collecting wakers on this thread. Returns whether this call is the
/// outermost one (it must then call `run_deferred_wakers`).
fn defer_wakers() -> bool {
    DEFERRED_WAKERS.with(|deferred| {
        let mut deferred = deferred.borrow_mut();
        if deferred.is_some() {
            return false;
        }
        *deferred = Some(Vec::new());
        true
    })
}

fn run_deferred_wakers() {
    let wakers = DEFERRED_WAKERS
        .with(|deferred| deferred.borrow_mut().take())
        .unwrap_or_default();
    for waker in wakers {
        waker();
    }
}

// Go: context/context.go:168 Canceled
/// Canceled is the error returned by [Context.Err] when the context is canceled
/// for some reason other than its deadline passing.
pub static CANCELED: LazyLock<GoError> = LazyLock::new(|| errors::new("context canceled"));

// Go: context/context.go:172 DeadlineExceeded
/// DeadlineExceeded is the error returned by [Context.Err] when the context is canceled
/// due to its deadline passing.
pub static DEADLINE_EXCEEDED: LazyLock<GoError> =
    LazyLock::new(|| errors::from_value(DeadlineExceededError));

// Go: context/context.go:174 deadlineExceededError
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeadlineExceededError;

impl fmt::Display for DeadlineExceededError {
    // Go: context/context.go:176 Error
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("context deadline exceeded")
    }
}

impl DeadlineExceededError {
    // Go: context/context.go:177 Timeout
    pub fn timeout(&self) -> bool {
        true
    }

    // Go: context/context.go:178 Temporary
    pub fn temporary(&self) -> bool {
        true
    }
}

// Go: context/context.go:216 Background
/// Background returns a non-nil, empty [Context]. It is never canceled, has no
/// values, and has no deadline. It is typically used by the main function,
/// initialization, and tests, and as the top-level Context for incoming
/// requests.
pub fn background() -> Context {
    Context(Ctx::Background)
}

// Go: context/context.go:224 TODO
/// TODO returns a non-nil, empty [Context]. Code should use context.TODO when
/// it's unclear which Context to use or it is not yet available.
pub fn todo() -> Context {
    Context(Ctx::Todo)
}

// Go: context/context.go:232 CancelFunc
/// A CancelFunc tells an operation to abandon its work.
/// A CancelFunc does not wait for the work to stop.
/// A CancelFunc may be called by multiple goroutines simultaneously.
/// After the first call, subsequent calls to a CancelFunc do nothing.
pub type CancelFunc = Arc<dyn Fn() + Send + Sync>;

// Go: context/context.go:241 WithCancel
/// WithCancel returns a derived context that points to the parent context
/// but has a new Done channel. The returned context's Done channel is closed
/// when the returned cancel function is called or when the parent context's
/// Done channel is closed, whichever happens first.
pub fn with_cancel(parent: &Context) -> (Context, CancelFunc) {
    let c = with_cancel_unexported(parent);
    let cc = c.clone();
    (
        Context(Ctx::Cancel(c)),
        Arc::new(move || cc.cancel(true, CANCELED.clone(), None)),
    )
}

// Go: context/context.go:256 CancelCauseFunc
/// A CancelCauseFunc behaves like a [CancelFunc] but additionally sets the
/// cancellation cause. `None` is a nil cause.
pub type CancelCauseFunc = Arc<dyn Fn(Option<GoError>) + Send + Sync>;

// Go: context/context.go:269 WithCancelCause
/// WithCancelCause behaves like [WithCancel] but returns a [CancelCauseFunc]
/// instead of a [CancelFunc]. Calling cancel with a non-nil error (the
/// "cause") records that error in ctx; it can then be retrieved using
/// Cause(ctx). Calling cancel with nil sets the cause to Canceled.
pub fn with_cancel_cause(parent: &Context) -> (Context, CancelCauseFunc) {
    let c = with_cancel_unexported(parent);
    let cc = c.clone();
    (
        Context(Ctx::Cancel(c)),
        Arc::new(move |cause: Option<GoError>| cc.cancel(true, CANCELED.clone(), cause)),
    )
}

// Go: context/context.go:274 withCancel
// PORT: named `with_cancel_unexported` because Go `WithCancel` is
// `with_cancel`. Go panics on a nil parent, which cannot occur here.
fn with_cancel_unexported(parent: &Context) -> Arc<CancelCtx> {
    let c = Arc::new(CancelCtx::new(parent.clone()));
    c.propagate_cancel(parent, c.clone());
    c
}

// Go: context/context.go:289 Cause
/// Cause returns a non-nil error explaining why c was canceled.
/// The first cancellation of c or one of its parents sets the cause.
/// If that cancellation happened via a call to CancelCauseFunc(err),
/// then [Cause] returns err.
/// Otherwise Cause(c) returns the same value as c.Err().
/// Cause returns nil if c has not been canceled yet.
pub fn cause(c: &Context) -> Option<GoError> {
    let err = c.err()?;
    if let Some(ValueRef::CancelCtx(cc)) = value(c, &KeyRef::CancelCtx) {
        let cause = lock(&cc.get().mu).cause.clone();
        if cause.is_some() {
            return cause;
        }
        // Either this context is not canceled,
        // or it is canceled and the cancellation happened in a
        // custom context implementation rather than a *cancelCtx.
    }
    // There is no cancelCtxKey value with a cause, so we know that c is
    // not a descendant of some canceled Context created by WithCancelCause.
    // Therefore, there is no specific cause to return.
    // If this is not one of the standard Context types,
    // it might still have an error even though it won't have a cause.
    Some(err)
}

/// The `stop` function returned by `after_func`.
pub type AfterFuncStop = Arc<dyn Fn() -> bool + Send + Sync>;

// Go: context/context.go:326 AfterFunc
/// AfterFunc arranges to call f in its own goroutine after ctx is canceled.
/// If ctx is already canceled, AfterFunc calls f immediately in its own goroutine.
///
/// Multiple calls to AfterFunc on a context operate independently;
/// one does not replace another.
///
/// Calling the returned stop function stops the association of ctx with f.
/// It returns true if the call stopped f from being run.
/// If stop returns false,
/// either the context is canceled and f has been started in its own goroutine;
/// or f was already stopped.
/// The stop function does not wait for f to complete before returning.
///
/// PORT: `f` runs on a new OS thread, so it may only touch `Send` data.
/// A thread is much more costly than a goroutine: do not call this once per
/// request (see `CheckerPool::register_request_cleanup`).
pub fn after_func<F: FnOnce() + Send + 'static>(ctx: &Context, f: F) -> AfterFuncStop {
    let a = Arc::new(AfterFuncCtx {
        cancel_ctx: CancelCtx::new(ctx.clone()),
        once: Once::new(),
        f: Mutex::new(Some(Box::new(f))),
    });
    a.cancel_ctx.propagate_cancel(ctx, a.clone());
    Arc::new(move || {
        let mut stopped = false;
        a.once.call_once(|| {
            stopped = true;
        });
        if stopped {
            a.cancel(true, CANCELED.clone(), None);
        }
        stopped
    })
}

/// The number of threads that `after_func` callbacks have started in this
/// process. Tests read it to check that a code path starts no thread.
pub static AFTER_FUNC_GOROUTINES: AtomicUsize = AtomicUsize::new(0);

// Go: context/context.go:343 afterFuncer
// PORT: only custom Context implementations have an AfterFunc method; the
// port has none, so the interface is not needed.

// Go: context/context.go:347 afterFuncCtx
struct AfterFuncCtx {
    cancel_ctx: CancelCtx,
    once: Once, // either starts running f or stops f from running
    f: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

impl Canceler for AfterFuncCtx {
    // Go: context/context.go:353 cancel
    fn cancel(&self, remove_from_parent: bool, err: GoError, cause: Option<GoError>) {
        self.cancel_ctx.cancel(false, err, cause);
        if remove_from_parent {
            remove_child(&self.cancel_ctx.context, canceler_key(self));
        }
        self.once.call_once(|| {
            if let Some(f) = lock(&self.f).take() {
                AFTER_FUNC_GOROUTINES.fetch_add(1, Ordering::Relaxed);
                go(f);
            }
        });
    }

    fn done(&self) -> Done {
        self.cancel_ctx.done()
    }
}

// Go: context/context.go:366 stopCtx
// PORT: a stopCtx replaces the parent of a cancelCtx whose parent has an
// AfterFunc method (a custom Context). The port has none.

// Go: context/context.go:372 goroutines
// PORT: a test counter for the watcher goroutine of propagateCancel, which
// the port never starts.

// Go: context/context.go:383 parentCancelCtx
/// parentCancelCtx returns the underlying *cancelCtx for parent.
/// It does this by looking up parent.Value(&cancelCtxKey) to find
/// the innermost enclosing *cancelCtx and then checking whether
/// parent.Done() matches that *cancelCtx. (If not, the *cancelCtx
/// has been wrapped in a custom implementation providing a
/// different done channel, in which case we should not bypass it.)
fn parent_cancel_ctx(parent: &Context) -> Option<CancelCtxRef> {
    let done = parent.done()?;
    if done.ptr_eq(&CLOSEDCHAN) {
        return None;
    }
    let p = match value(parent, &KeyRef::CancelCtx) {
        Some(ValueRef::CancelCtx(p)) => p,
        _ => return None,
    };
    let pdone = p.get().done.get()?;
    if !pdone.ptr_eq(&done) {
        return None;
    }
    Some(p)
}

// Go: context/context.go:400 removeChild
/// removeChild removes a context from its parent.
/// PORT: Go first checks for a `stopCtx` parent (see `stopCtx`); the port
/// has none. `child` is the canceler's identity (`canceler_key`).
fn remove_child(parent: &Context, child: usize) {
    let Some(p) = parent_cancel_ctx(parent) else {
        return;
    };
    let mut fields = lock(&p.get().mu);
    if let Some(children) = &mut fields.children {
        children.shift_remove(&child);
    }
}

// Go: context/context.go:418 canceler
/// A canceler is a context type that can be canceled directly. The
/// implementations are *cancelCtx and *timerCtx.
trait Canceler: Send + Sync {
    fn cancel(&self, remove_from_parent: bool, err: GoError, cause: Option<GoError>);
    fn done(&self) -> Done;
}

/// Go `map[canceler]struct{}` key: the canceler's address.
fn canceler_key<T: ?Sized>(c: &T) -> usize {
    c as *const T as *const () as usize
}

// Go: context/context.go:424 closedchan
/// closedchan is a reusable closed channel.
static CLOSEDCHAN: LazyLock<Done> = LazyLock::new(|| {
    // Go: context/context.go:426 init
    let d = Done::new();
    d.close();
    d
});

// Go: context/context.go:432 cancelCtx
/// A cancelCtx can be canceled. When canceled, it also cancels any children
/// that implement canceler.
struct CancelCtx {
    /// Go embedded `Context` (the parent).
    context: Context,

    mu: Mutex<CancelCtxFields>, // protects following fields
    /// Go `done atomic.Value`: of chan struct{}, created lazily, closed by first cancel call.
    done: OnceLock<Done>,
    /// Go `err atomic.Value`: set to non-nil by the first cancel call.
    err: OnceLock<GoError>,
}

struct CancelCtxFields {
    /// set to nil by the first cancel call.
    /// PORT: Go map order is random; the port cancels children in insertion
    /// order.
    children: Option<IndexMap<usize, Arc<dyn Canceler>>>,
    /// set to non-nil by the first cancel call.
    cause: Option<GoError>,
    /// Go `timerCtx.timer` ("Under cancelCtx.mu."); always `None` for a
    /// plain cancelCtx or an afterFuncCtx.
    timer: Option<DeadlineTimer>,
}

impl CancelCtx {
    // PORT: Go makes `&cancelCtx{}` and sets the embedded parent in
    // propagateCancel; the port sets it here.
    fn new(parent: Context) -> CancelCtx {
        CancelCtx {
            context: parent,
            mu: Mutex::new(CancelCtxFields {
                children: None,
                cause: None,
                timer: None,
            }),
            done: OnceLock::new(),
            err: OnceLock::new(),
        }
    }

    // Go: context/context.go:449 Done
    fn done(&self) -> Done {
        if let Some(d) = self.done.get() {
            return d.clone();
        }
        let _fields = lock(&self.mu);
        self.done.get_or_init(Done::new).clone()
    }

    // Go: context/context.go:464 Err
    fn err(&self) -> Option<GoError> {
        // An atomic load is ~5x faster than a mutex, which can matter in tight loops.
        if let Some(err) = self.err.get() {
            // Ensure the done channel has been closed before returning a non-nil error.
            self.done().wait();
            return Some(err.clone());
        }
        None
    }

    // Go: context/context.go:476 propagateCancel
    /// propagateCancel arranges for child to be canceled when parent is.
    /// It sets the parent context of cancelCtx.
    fn propagate_cancel(&self, parent: &Context, child: Arc<dyn Canceler>) {
        // PORT: `c.Context = parent` happened in `CancelCtx::new`.

        let Some(done) = parent.done() else {
            return; // parent is never canceled
        };

        if done.is_closed() {
            // parent is already canceled
            child.cancel(
                false,
                parent.err().expect("closed Done without Err"),
                cause(parent),
            );
            return;
        }

        if let Some(p) = parent_cancel_ctx(parent) {
            // parent is a *cancelCtx, or derives from one.
            let p = p.get();
            let mut fields = lock(&p.mu);
            if let Some(err) = p.err.get() {
                // parent has already been canceled
                let cause = fields.cause.clone();
                child.cancel(false, err.clone(), cause);
            } else {
                fields
                    .children
                    .get_or_insert_with(IndexMap::new)
                    .insert(canceler_key(&*child), child);
            }
            return;
        }

        // PORT: Go now handles a parent with an AfterFunc method and then
        // starts a goroutine that waits for parent.Done() or child.Done().
        // Both are for custom Context implementations: for the standard
        // contexts parentCancelCtx always succeeds once parent.Done() is a
        // channel that was not closed above.
        unreachable!("context: parent is not a standard Context");
    }

    // Go: context/context.go:543 String
    fn string(&self) -> String {
        context_name(&self.context) + ".WithCancel"
    }
}

impl Canceler for CancelCtx {
    // Go: context/context.go:550 cancel
    /// cancel closes c.done, cancels each of c's children, and, if
    /// removeFromParent is true, removes c from its parent's children.
    /// cancel sets c.cause to cause if this is the first time c is canceled.
    fn cancel(&self, remove_from_parent: bool, err: GoError, cause: Option<GoError>) {
        // PORT: Go panics "context: internal error: missing cancel error" on
        // a nil err; err is never nil here.
        let cause = match cause {
            Some(cause) => cause,
            None => err.clone(),
        };
        let outermost = defer_wakers();
        let mut fields = lock(&self.mu);
        if self.err.get().is_some() {
            drop(fields);
            if outermost {
                run_deferred_wakers();
            }
            return; // already canceled
        }
        let _ = self.err.set(err.clone());
        fields.cause = Some(cause.clone());
        match self.done.get() {
            None => {
                let _ = self.done.set(CLOSEDCHAN.clone());
            }
            Some(d) => d.close(),
        }
        if let Some(children) = &fields.children {
            for child in children.values() {
                // NOTE: acquiring the child's lock while holding parent's lock.
                child.cancel(false, err.clone(), Some(cause.clone()));
            }
        }
        fields.children = None;
        drop(fields);
        if outermost {
            run_deferred_wakers();
        }

        if remove_from_parent {
            remove_child(&self.context, canceler_key(self));
        }
    }

    fn done(&self) -> Done {
        CancelCtx::done(self)
    }
}

// Go: context/context.go:536 contextName
fn context_name(c: &Context) -> String {
    c.string()
}

// Go: context/context.go:586 WithoutCancel
/// WithoutCancel returns a derived context that points to the parent context
/// and is not canceled when parent is canceled.
/// The returned context returns no Deadline or Err, and its Done channel is nil.
/// Calling [Cause] on the returned context returns nil.
pub fn without_cancel(parent: &Context) -> Context {
    Context(Ctx::WithoutCancel(Arc::new(WithoutCancelCtx {
        c: parent.clone(),
    })))
}

// Go: context/context.go:593 withoutCancelCtx
struct WithoutCancelCtx {
    c: Context,
}

impl WithoutCancelCtx {
    // Go: context/context.go:613 String
    fn string(&self) -> String {
        context_name(&self.c) + ".WithoutCancel"
    }
}

// Go: context/context.go:626 WithDeadline
/// WithDeadline returns a derived context that points to the parent context
/// but has the deadline adjusted to be no later than d. If the parent's
/// deadline is already earlier than d, WithDeadline(parent, d) is semantically
/// equivalent to parent. The returned [Context.Done] channel is closed when
/// the deadline expires, when the returned cancel function is called,
/// or when the parent context's Done channel is closed, whichever happens first.
pub fn with_deadline(parent: &Context, d: Instant) -> (Context, CancelFunc) {
    with_deadline_cause(parent, d, None)
}

// Go: context/context.go:633 WithDeadlineCause
/// WithDeadlineCause behaves like [WithDeadline] but also sets the cause of the
/// returned Context when the deadline is exceeded. The returned [CancelFunc] does
/// not set the cause.
pub fn with_deadline_cause(
    parent: &Context,
    d: Instant,
    cause: Option<GoError>,
) -> (Context, CancelFunc) {
    if let Some(cur) = parent.deadline() {
        if cur < d {
            // The current deadline is already sooner than the new one.
            return with_cancel(parent);
        }
    }
    let c = Arc::new(TimerCtx {
        cancel_ctx: CancelCtx::new(parent.clone()),
        deadline: d,
    });
    c.cancel_ctx.propagate_cancel(parent, c.clone());
    let dur = d.saturating_duration_since(Instant::now());
    if dur.is_zero() {
        c.cancel(true, DEADLINE_EXCEEDED.clone(), cause); // deadline has already passed
        let cc = c.clone();
        return (
            Context(Ctx::Timer(c)),
            Arc::new(move || cc.cancel(false, CANCELED.clone(), None)),
        );
    }
    {
        let mut fields = lock(&c.cancel_ctx.mu);
        if c.cancel_ctx.err.get().is_none() {
            let cc = c.clone();
            fields.timer = Some(DeadlineTimer::schedule(d, move || {
                cc.cancel(true, DEADLINE_EXCEEDED.clone(), cause);
            }));
        }
    }
    let cc = c.clone();
    (
        Context(Ctx::Timer(c)),
        Arc::new(move || cc.cancel(true, CANCELED.clone(), None)),
    )
}

// Go: context/context.go:663 timerCtx
/// A timerCtx carries a timer and a deadline. It embeds a cancelCtx to
/// implement Done and Err. It implements cancel by stopping its timer then
/// delegating to cancelCtx.cancel.
struct TimerCtx {
    cancel_ctx: CancelCtx,
    // Go `timer *time.Timer` is `cancel_ctx.mu`'s `timer` field.
    deadline: Instant,
}

impl TimerCtx {
    // Go: context/context.go:674 String
    // PORT: Go prints `deadline.String()` (wall clock) and the Go duration
    // text; an Instant has no wall clock, so this uses Rust debug text.
    fn string(&self) -> String {
        format!(
            "{}.WithDeadline({:?} [{:?}])",
            context_name(&self.cancel_ctx.context),
            self.deadline,
            self.deadline.saturating_duration_since(Instant::now())
        )
    }
}

impl Canceler for TimerCtx {
    // Go: context/context.go:680 cancel
    fn cancel(&self, remove_from_parent: bool, err: GoError, cause: Option<GoError>) {
        self.cancel_ctx.cancel(false, err, cause);
        if remove_from_parent {
            // Remove this timerCtx from its parent cancelCtx's children.
            remove_child(&self.cancel_ctx.context, canceler_key(self));
        }
        let mut fields = lock(&self.cancel_ctx.mu);
        if let Some(t) = fields.timer.take() {
            t.stop();
        }
    }

    fn done(&self) -> Done {
        self.cancel_ctx.done()
    }
}

// Go: context/context.go:704 WithTimeout
/// WithTimeout returns WithDeadline(parent, time.Now().Add(timeout)).
///
/// Canceling this context releases resources associated with it, so code should
/// call cancel as soon as the operations running in this [Context] complete.
pub fn with_timeout(parent: &Context, timeout: Duration) -> (Context, CancelFunc) {
    with_deadline(parent, Instant::now() + timeout)
}

// Go: context/context.go:711 WithTimeoutCause
/// WithTimeoutCause behaves like [WithTimeout] but also sets the cause of the
/// returned Context when the timeout expires. The returned [CancelFunc] does
/// not set the cause.
pub fn with_timeout_cause(
    parent: &Context,
    timeout: Duration,
    cause: Option<GoError>,
) -> (Context, CancelFunc) {
    with_deadline_cause(parent, Instant::now() + timeout, cause)
}

// Go: context/context.go:728 WithValue
/// WithValue returns a derived context that points to the parent Context.
/// In the derived context, the value associated with key is val.
///
/// Use context Values only for request-scoped data that transits processes and
/// APIs, not for passing optional parameters to functions.
///
/// PORT: Go panics on a nil parent, a nil key or a key that is not
/// comparable; none can occur here.
pub fn with_value<T: Send + Sync + 'static>(
    parent: &Context,
    key: &'static ContextKey<T>,
    val: T,
) -> Context {
    Context(Ctx::Value(Arc::new(ValueCtx {
        context: parent.clone(),
        key: key.id(),
        key_name: key.name,
        val: Arc::new(val),
        val_type_name: std::any::type_name::<T>(),
    })))
}

// Go: context/context.go:743 valueCtx
/// A valueCtx carries a key-value pair. It implements Value for that key and
/// delegates all other calls to the embedded Context.
struct ValueCtx {
    context: Context,
    key: KeyId,
    key_name: &'static str,
    val: Arc<dyn Any + Send + Sync>,
    val_type_name: &'static str,
}

impl ValueCtx {
    // Go: context/context.go:763 String
    fn string(&self) -> String {
        context_name(&self.context)
            + ".WithValue("
            + self.key_name
            + ", "
            + &self.stringify_val()
            + ")"
    }

    // Go: context/context.go:751 stringify
    // PORT: Go prints a key with its type name (for example `core.key`); the
    // port prints the key's Go name. A value that is not a string prints its
    // Rust type name where Go prints its Go type name.
    fn stringify_val(&self) -> String {
        if let Some(s) = self.val.downcast_ref::<String>() {
            return s.clone();
        }
        if let Some(s) = self.val.downcast_ref::<&'static str>() {
            return (*s).to_string();
        }
        self.val_type_name.to_string()
    }
}

// Go: context/context.go:776 value
// PORT: Go's `default` case (`c.Value(key)` of a custom Context) cannot
// occur.
fn value(c: &Context, key: &KeyRef) -> Option<ValueRef> {
    let mut c = c.clone();
    loop {
        let next = match &c.0 {
            Ctx::Value(ctx) => {
                if let KeyRef::User(k) = key {
                    if *k == ctx.key {
                        return Some(ValueRef::User(ctx.val.clone()));
                    }
                }
                ctx.context.clone()
            }
            Ctx::Cancel(ctx) => {
                if let KeyRef::CancelCtx = key {
                    return Some(ValueRef::CancelCtx(CancelCtxRef::Cancel(ctx.clone())));
                }
                ctx.context.clone()
            }
            Ctx::WithoutCancel(ctx) => {
                if let KeyRef::CancelCtx = key {
                    // This implements Cause(ctx) == nil
                    // when ctx is created using WithoutCancel.
                    return None;
                }
                ctx.c.clone()
            }
            Ctx::Timer(ctx) => {
                if let KeyRef::CancelCtx = key {
                    return Some(ValueRef::CancelCtx(CancelCtxRef::Timer(ctx.clone())));
                }
                ctx.cancel_ctx.context.clone()
            }
            Ctx::Background | Ctx::Todo => return None,
        };
        c = next;
    }
}
