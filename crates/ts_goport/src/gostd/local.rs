//! The dispatch-thread work queue. No Go source: this is the port's model
//! for Go goroutines and `time.AfterFunc` callbacks that touch
//! language-service state (PORTING.md "Threads" and "Go runtime").
//!
//! One thread (the LSP dispatch thread) owns all `Rc`/`RefCell` state. A Go
//! `go f()` over that state becomes `local::go(Box::new(f))`, and a Go
//! `time.AfterFunc(d, f)` over that state becomes `local::after_func`. Both
//! run on the dispatch thread when it calls `run_pending`, in the order they
//! became ready: a job when it is queued, a timer when it is due.
//!
//! Contract with the dispatch loop: the server calls `set_waker(f)` once. A
//! timer thread calls the waker when a `LocalTimer` becomes due. Each
//! thread that arms a `LocalTimer` gets one timer thread for all its timers,
//! made at its first arm. It ends when that thread ends. The
//! dispatch loop calls `run_pending()` after each message and after each
//! wake-up. Go `WaitForBackgroundTasks` (`background::Queue::wait`) calls
//! `run_pending()`, and `wait_pending()` while a queued task sleeps on a
//! timer, until the queue's tasks have finished.
//!
//! A job that waits for another thread is `post_later(f)`: the other
//! thread posts the `Send` handle it returns once, when its work ends, and
//! `f` then runs in `run_pending` like a `go` job (Go: a goroutine that a
//! channel send makes runnable). Nothing runs or polls before the post.
//!
//! Idle work (`go_idle`) is a second, separate queue for long work that
//! sends nothing to the client (the auto-import warm). The dispatch loop
//! runs it with `run_idle()` only when no message waits: at once or after a
//! quiet period with no message, as the job asks (`IdleStart`), so it does
//! not delay a request that has arrived. `run_pending` does not run it.
//!
//! Garbage (`drop_later`) is a third queue, for large frees that Go's
//! garbage collector does in the background (a released program, the
//! parse tasks of a load). On a thread whose dispatch loop called
//! `keep_garbage`, the values wait until the loop calls `drop_garbage`
//! after a message, so the free is not in the answer time. On other
//! threads `drop_later` drops at once. `drop_after_pause` uses this queue
//! only for the first release after a client pause (`note_message_gap`),
//! and only when the client has not sent its next edit yet
//! (`set_stream_check`). It drops at once otherwise, so only the releases
//! of one message wait (of one pipelined burst on an API connection: see
//! `drop_after_pause`).
//!
//! The queues are per thread: `go`, `post_later`, `go_idle`, `after_func`,
//! `run_pending`, `run_idle`, `drop_later` and `drop_garbage` act on the
//! calling thread's queues.

use crate::prelude::*;

use std::any::Any;
use std::cell::Cell;
use std::collections::{BTreeSet, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

// PORT: Go mutexes do not poison.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The waker the dispatch loop installs with `set_waker`.
pub type Waker = Arc<dyn Fn() + Send + Sync>;

/// The part of a thread's queue that its timer thread can reach.
struct LocalShared {
    /// Ready work in the order it became ready.
    queue: Mutex<VecDeque<Entry>>,
    /// Signalled when the timer thread or a `Post` adds to `queue` (for
    /// `wait_pending`).
    ready: Condvar,
    waker: Mutex<Option<Waker>>,
    /// `Post` handles that have not posted yet. A post changes it while it
    /// holds the `queue` lock, after it queues its job.
    posts: AtomicUsize,
    /// The timers of the thread. A thread that holds this lock can take
    /// `queue`, not the other way round.
    timers: Mutex<Timers>,
    /// Signalled when a timer becomes the first due, and when the thread
    /// ends (`Timers::closed`).
    timers_changed: Condvar,
}

/// The `LocalTimer`s of one thread, for its timer thread
/// (`run_local_timers`).
// PERF (perfplan4 R6): one timer thread per thread, not one per timer. The
// language server stops and makes two timers on each edit
// (`schedule_idle_cache_clean`, `schedule_cleanup_locked`), which started
// two threads per edit. Go `time.AfterFunc` starts no thread.
#[derive(Default)]
struct Timers {
    /// Go `t.when` of each armed timer, by id.
    when: FxHashMap<u64, Instant>,
    /// The armed timers by `when`, then id.
    due: BTreeSet<(Instant, u64)>,
    /// Due entries of each timer in the queue that have not run yet.
    queued: FxHashMap<u64, u32>,
    /// Whether the timer thread runs.
    thread_running: bool,
    /// Set when the thread ends: its timer thread then ends too.
    closed: bool,
}

impl Timers {
    /// Arms timer `id` for `when`. Returns whether it was armed before.
    fn arm(&mut self, id: u64, when: Instant) -> bool {
        let old = self.when.insert(id, when);
        if let Some(old) = old {
            self.due.remove(&(old, id));
        }
        self.due.insert((when, id));
        old.is_some()
    }

    /// Disarms timer `id`. Returns whether it was armed.
    fn disarm(&mut self, id: u64) -> bool {
        let old = self.when.remove(&id);
        if let Some(old) = old {
            self.due.remove(&(old, id));
        }
        old.is_some()
    }

    /// Whether timer `id` is neither armed nor queued.
    fn idle(&self, id: u64) -> bool {
        !self.when.contains_key(&id) && !self.queued.contains_key(&id)
    }

    /// Counts one run of a due entry of timer `id`.
    fn unqueue(&mut self, id: u64) {
        if let Some(count) = self.queued.get_mut(&id) {
            *count -= 1;
            if *count == 0 {
                self.queued.remove(&id);
            }
        }
    }
}

#[derive(Clone, Copy)]
enum Entry {
    /// A job from `go`, by id.
    Job(u64),
    /// A due `LocalTimer`, by id.
    Timer(u64),
}

/// The thread-local part: closures that are not `Send`.
struct LocalState {
    shared: Arc<LocalShared>,
    next_id: Cell<u64>,
    jobs: RefCell<FxHashMap<u64, Box<dyn FnOnce()>>>,
    /// Timers that are armed or have a due entry in the queue.
    timers: RefCell<FxHashMap<u64, Rc<LocalTimerInner>>>,
    /// Jobs from `go_idle`, oldest first.
    idle: RefCell<VecDeque<(IdleStart, Box<dyn FnOnce()>)>>,
    /// Values from `drop_later`, oldest first. None until `keep_garbage`.
    garbage: RefCell<Option<VecDeque<Box<dyn Any>>>>,
    /// What `drop_after_pause` does (`note_message_gap`).
    pause: Cell<Pause>,
    /// Whether the client sent more than its burst (`set_stream_check`).
    stream: RefCell<Option<Box<dyn Fn() -> bool>>>,
}

/// The state of `drop_after_pause` on one thread.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Pause {
    /// It drops at once.
    None,
    /// The client paused before the current burst of messages, and nothing
    /// was released since. The next release decides.
    Paused,
    /// The release after the pause waits for `drop_garbage`, and so do the
    /// other values of its message, until `drop_garbage` starts.
    Kept,
}

impl Drop for LocalState {
    // The timer thread of this thread ends with it.
    fn drop(&mut self) {
        lock(&self.shared.timers).closed = true;
        self.shared.timers_changed.notify_all();
    }
}

thread_local! {
    static LOCAL: LocalState = LocalState {
        shared: Arc::new(LocalShared {
            queue: Mutex::new(VecDeque::new()),
            ready: Condvar::new(),
            waker: Mutex::new(None),
            posts: AtomicUsize::new(0),
            timers: Mutex::new(Timers::default()),
            timers_changed: Condvar::new(),
        }),
        next_id: Cell::new(1),
        jobs: RefCell::new(FxHashMap::default()),
        timers: RefCell::new(FxHashMap::default()),
        idle: RefCell::new(VecDeque::new()),
        garbage: RefCell::new(None),
        pause: Cell::new(Pause::None),
        stream: RefCell::new(None),
    };
}

/// True while the queues of this thread are being dropped or are gone: the
/// thread ends. A value that a queued job holds (a job that never ran)
/// drops then, and must not queue more work here.
pub fn is_ending() -> bool {
    LOCAL.try_with(|_| ()).is_err()
}

fn next_id() -> u64 {
    LOCAL.with(|l| {
        let id = l.next_id.get();
        l.next_id.set(id + 1);
        id
    })
}

/// Go `go f()` for a function that touches dispatch-thread state: queue `f`
/// to run on this thread in FIFO order.
pub fn go(f: Box<dyn FnOnce()>) {
    let id = next_id();
    LOCAL.with(|l| {
        l.jobs.borrow_mut().insert(id, f);
        lock(&l.shared.queue).push_back(Entry::Job(id));
    });
}

/// The `Send` handle of `post_later`.
pub struct Post {
    id: u64,
    shared: Arc<LocalShared>,
}

/// Go `go f()` where `f` first waits for work on another thread (a
/// goroutine that blocks on a channel or a child process, then touches
/// dispatch-thread state). `f` waits on this thread until the other thread
/// calls `post` on the handle; then it is queued like a `go` job and the
/// waker is called. Until then `wait_pending` waits for it.
pub fn post_later(f: Box<dyn FnOnce()>) -> Post {
    let id = next_id();
    let shared = LOCAL.with(|l| {
        l.jobs.borrow_mut().insert(id, f);
        l.shared.posts.fetch_add(1, Ordering::SeqCst);
        l.shared.clone()
    });
    Post { id, shared }
}

impl Post {
    /// Queues the job on its thread, once. Dropping the handle posts it
    /// too, so the job also runs when the other thread panics.
    pub fn post(self) {}
}

impl Drop for Post {
    fn drop(&mut self) {
        {
            let mut queue = lock(&self.shared.queue);
            queue.push_back(Entry::Job(self.id));
            self.shared.posts.fetch_sub(1, Ordering::SeqCst);
            self.shared.ready.notify_all();
        }
        let waker = lock(&self.shared.waker).clone();
        if let Some(waker) = waker {
            waker();
        }
    }
}

/// When the dispatch loop may start an idle job (`go_idle`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdleStart {
    /// As soon as no message waits.
    AtOnce,
    /// After a quiet period with no message (the LSP server's
    /// `IDLE_QUIET_PERIOD`).
    AfterQuiet,
}

/// Queues `f` as idle work on this thread. The dispatch loop runs it with
/// `run_idle` when no message waits, at the time that `start` gives. No Go
/// counterpart: Go runs this work on a goroutine, at the same time as
/// requests.
pub fn go_idle(start: IdleStart, f: Box<dyn FnOnce()>) {
    LOCAL.with(|l| l.idle.borrow_mut().push_back((start, f)));
}

/// When the oldest idle job of this thread may start, or `None` when no
/// idle work waits for `run_idle`.
pub fn next_idle() -> Option<IdleStart> {
    LOCAL.with(|l| l.idle.borrow().front().map(|(start, _)| *start))
}

/// Runs the oldest idle job of this thread. Returns false if there was none.
/// Work that the job queues with `go` waits for the next `run_pending`.
pub fn run_idle() -> bool {
    let job = LOCAL.with(|l| l.idle.borrow_mut().pop_front());
    match job {
        Some((_, job)) => {
            job();
            true
        }
        None => false,
    }
}

/// The most values that `drop_garbage` keeps while a message waits. More
/// are dropped at once, so a stream of messages with no gap between them
/// does not keep old programs.
const GARBAGE_LIMIT: usize = 16;

/// Makes `drop_later` on this thread keep its values until `drop_garbage`.
/// The dispatch loop calls it once, before its first message.
pub fn keep_garbage() {
    LOCAL.with(|l| {
        l.garbage.borrow_mut().get_or_insert_with(VecDeque::new);
    });
}

/// Drops `value` in a later `drop_garbage` on this thread, or now when this
/// thread did not call `keep_garbage`. No Go counterpart: Go's garbage
/// collector frees old data in the background, never in a request.
pub fn drop_later(value: Box<dyn Any>) {
    let value = LOCAL.with(|l| match l.garbage.borrow_mut().as_mut() {
        Some(garbage) => {
            garbage.push_back(value);
            None
        }
        None => Some(value),
    });
    drop(value);
}

/// A gap between two messages that is this long or longer starts a new
/// burst of messages. Shorter gaps are one burst (a client that sends a
/// didChange and its requests together).
const BURST_GAP: Duration = Duration::from_millis(1);

/// A gap before a burst that is this long or longer is a client pause
/// (`drop_after_pause`). It is longer than every gap of a client that sends
/// its messages with no pause (0.6 to 9 ms in ls_edit_bench) and shorter
/// than the gaps of a person who types.
const CLIENT_PAUSE: Duration = Duration::from_millis(20);

/// The dispatch loop calls it when it takes a message: `gap` is the time
/// since it finished the last message. A gap of `BURST_GAP` or more starts
/// a new burst. The burst is paused when the gap is `CLIENT_PAUSE` or more
/// and nothing waits for `drop_garbage` (no release that `drop_after_pause`
/// kept still waits).
pub fn note_message_gap(gap: Duration) {
    if gap >= BURST_GAP {
        LOCAL.with(|l| {
            let idle = l.garbage.borrow().as_ref().is_none_or(VecDeque::is_empty);
            l.pause.set(if gap >= CLIENT_PAUSE && idle {
                Pause::Paused
            } else {
                Pause::None
            });
        });
    }
}

/// Installs the function that tells whether the client already sent more
/// than the requests of the current burst: a notification (its next edit)
/// waits for the dispatch loop. `drop_after_pause` then drops at once. The
/// dispatch loop calls it once.
pub fn set_stream_check(f: Box<dyn Fn() -> bool>) {
    LOCAL.with(|l| *l.stream.borrow_mut() = Some(f));
}

/// Drops `value` now, except for the first release after a client pause
/// (`note_message_gap`) when the client has not sent its next edit yet
/// (`set_stream_check`): then `value` and the other values of that message
/// wait for `drop_garbage` (`drop_later`). So only the releases of one
/// message wait. An API connection of the LSP server reads its messages
/// from its inbox and runs the dispatch loop, and with it `drop_garbage`,
/// only when the inbox is empty (lsp/server.rs `ApiConnProtocol`). Its
/// messages do not call `note_message_gap`, and the stream check sees only
/// LSP notifications. So when the pause state lets an API message keep its
/// release, the releases of the rest of a pipelined API burst wait too,
/// until the inbox is empty. No Go counterpart: Go's garbage collector
/// frees old data in the background.
// PERF (freecheck1): use it for a large free on the answer path that the
// next check would otherwise reuse (the checkers of a released program).
// In a stream of messages, a free now is cheap (hot memory) and the next
// check reuses its memory from the thread cache; a free after the answer
// made those runs slower (+1% to +6%) and kept more memory. After a pause,
// the free is cold (hono 6.5 ms, 12.7% of the edit) and the client waits
// for it, so it goes after the answers of the burst (also after a
// codeAction that comes with the diagnostics). When the client already
// sent its next edit (pipelined, or typing faster than the server), the
// values would wait until the stream ends (freecheck2: hono HWM x2.3 when
// every release of a stream after a pause waited), so they drop now. A
// check for any waiting request, not only a notification, also dropped
// them in the errfix rounds (hono errfix-paced 0.946 instead of 0.902).
pub fn drop_after_pause(value: Box<dyn Any>) {
    // The thread may be ending, after its queues (a checker pool that
    // another thread-local holds): then `value` drops now.
    let keep = LOCAL
        .try_with(|l| match l.pause.get() {
            Pause::None => false,
            Pause::Kept => true,
            Pause::Paused => {
                let keep = !l.stream.borrow().as_ref().is_some_and(|stream| stream());
                l.pause.set(if keep { Pause::Kept } else { Pause::None });
                keep
            }
        })
        .unwrap_or(false);
    if keep {
        drop_later(value);
    } else {
        drop(value);
    }
}

/// The number of values that wait for `drop_garbage` on this thread (for
/// tests).
pub fn garbage_len() -> usize {
    LOCAL.with(|l| l.garbage.borrow().as_ref().map_or(0, VecDeque::len))
}

/// Drops the values of `drop_later`, oldest first, until none is left or
/// `busy()` is true (a message waits). While more than `GARBAGE_LIMIT`
/// values wait, it drops them even when busy. It ends the message of a
/// release that `drop_after_pause` kept: later releases drop at once.
pub fn drop_garbage(busy: impl Fn() -> bool) {
    LOCAL.with(|l| {
        if l.pause.get() == Pause::Kept {
            l.pause.set(Pause::None);
        }
    });
    loop {
        // The borrow ends before the drop: a drop can call `drop_later`.
        let value = LOCAL.with(|l| {
            let mut garbage = l.garbage.borrow_mut();
            let garbage = garbage.as_mut()?;
            if garbage.is_empty() || (garbage.len() <= GARBAGE_LIMIT && busy()) {
                return None;
            }
            garbage.pop_front()
        });
        let Some(value) = value else {
            return;
        };
        drop(value);
    }
}

/// Installs the function that timer threads call when a `LocalTimer` of this
/// thread becomes due. The dispatch loop wakes and calls `run_pending`.
pub fn set_waker(f: Waker) {
    LOCAL.with(|l| {
        *lock(&l.shared.waker) = Some(f);
    });
}

/// Whether ready work (queued jobs or due timers) is waiting for
/// `run_pending`. Armed timers that are not due yet do not count.
pub fn has_pending() -> bool {
    LOCAL.with(|l| !lock(&l.shared.queue).is_empty())
}

/// Blocks until ready work waits for `run_pending`. Returns false at once
/// when nothing is ready, no timer of this thread is armed and no `Post`
/// of this thread waits, so nothing can become ready (only this thread arms
/// timers and makes posts).
pub fn wait_pending() -> bool {
    let shared = LOCAL.with(|l| l.shared.clone());
    loop {
        if has_pending() {
            return true;
        }
        // The timers lock is not taken under the queue lock: the timer
        // thread takes them in the other order.
        let armed =
            shared.posts.load(Ordering::SeqCst) > 0 || !lock(&shared.timers).when.is_empty();
        if !armed {
            // A timer that fired after the first check cleared `when` and
            // queued its entry under one hold of the timers lock, and a post
            // queued its job before it counted down, so the entry is there
            // now.
            return has_pending();
        }
        let queue = lock(&shared.queue);
        if queue.is_empty() {
            // The timeout only bounds a missed signal; a due timer and a
            // post signal.
            drop(shared.ready.wait_timeout(queue, Duration::from_millis(50)));
        }
    }
}

/// Runs the ready work of this thread in the order it became ready, until
/// the queue is empty. Work that a job queues (or a timer that becomes due
/// meanwhile) also runs before this returns.
pub fn run_pending() {
    let shared = LOCAL.with(|l| l.shared.clone());
    loop {
        let entry = lock(&shared.queue).pop_front();
        let Some(entry) = entry else {
            return;
        };
        match entry {
            Entry::Job(id) => {
                let job = LOCAL.with(|l| l.jobs.borrow_mut().remove(&id));
                if let Some(job) = job {
                    job();
                }
            }
            Entry::Timer(id) => {
                lock(&shared.timers).unqueue(id);
                let timer = LOCAL.with(|l| l.timers.borrow().get(&id).cloned());
                let Some(timer) = timer else {
                    continue;
                };
                // Go: the goroutine that the timer started runs f.
                {
                    let mut f = timer.f.borrow_mut();
                    (*f)();
                }
                timer.forget_if_idle();
            }
        }
    }
}

/// Go `*time.Timer` from `time.AfterFunc` whose function runs on the
/// dispatch thread.
pub struct LocalTimer {
    inner: Rc<LocalTimerInner>,
}

struct LocalTimerInner {
    id: u64,
    f: RefCell<Box<dyn FnMut()>>,
    /// The queues of the thread that made the timer.
    shared: Arc<LocalShared>,
}

/// Go `time.AfterFunc(d, f)` when `f` touches dispatch-thread state. After
/// `d`, `f` is queued on this thread (the waker is called) and runs in
/// `run_pending`.
///
/// PORT: Go `f` runs each time the timer fires, and it fires again after a
/// `reset`, so `f` is `FnMut`.
pub fn after_func(d: Duration, f: Box<dyn FnMut()>) -> LocalTimer {
    let id = next_id();
    let shared = LOCAL.with(|l| l.shared.clone());
    let inner = Rc::new(LocalTimerInner {
        id,
        f: RefCell::new(f),
        shared,
    });
    inner.arm(when(d));
    LOCAL.with(|l| {
        l.timers.borrow_mut().insert(id, inner.clone());
    });
    LocalTimer { inner }
}

impl LocalTimer {
    /// Go `t.Stop()` of an AfterFunc timer: true if the call stops the
    /// timer, false if the timer has already expired (its function is queued
    /// or has run) or been stopped. Stop does not remove a queued run.
    pub fn stop(&self) -> bool {
        // The timer thread is not woken: at the old `when` it finds this
        // timer gone and waits for the next one.
        let pending = lock(&self.inner.shared.timers).disarm(self.inner.id);
        self.inner.forget_if_idle();
        pending
    }

    /// Go `t.Reset(d)` of an AfterFunc timer: true if the timer had been
    /// active; false if it had expired or been stopped, in which case the
    /// function runs again after `d`.
    pub fn reset(&self, d: Duration) -> bool {
        let pending = self.inner.arm(when(d));
        LOCAL.with(|l| {
            l.timers
                .borrow_mut()
                .entry(self.inner.id)
                .or_insert_with(|| self.inner.clone());
        });
        pending
    }
}

impl std::fmt::Debug for LocalTimer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "LocalTimer(when: {:?})",
            lock(&self.inner.shared.timers).when.get(&self.inner.id)
        )
    }
}

impl LocalTimerInner {
    /// Sets `when` and makes sure the timer thread waits for it: starts the
    /// thread at the first arm, or wakes it when this timer is now the first
    /// due. Returns whether the timer was armed before.
    fn arm(&self, when: Instant) -> bool {
        let mut timers = lock(&self.shared.timers);
        let pending = timers.arm(self.id, when);
        if !timers.thread_running {
            drop(timers);
            let shared = self.shared.clone();
            std::thread::Builder::new()
                .name("local-timer".to_string())
                .spawn(move || run_local_timers(shared))
                .expect("failed to spawn local-timer thread");
            lock(&self.shared.timers).thread_running = true;
        } else if timers.due.first() == Some(&(when, self.id)) {
            self.shared.timers_changed.notify_all();
        }
        pending
    }

    /// Drops this thread's reference when the timer is neither armed nor
    /// queued, so an unreferenced timer is freed.
    fn forget_if_idle(&self) {
        let idle = lock(&self.shared.timers).idle(self.id);
        if idle {
            LOCAL.with(|l| {
                l.timers.borrow_mut().remove(&self.id);
            });
        }
    }
}

/// The timer thread of one thread (`shared`): when the first armed timer
/// is due, it queues the timer on that thread and calls the waker. It waits
/// while no timer is armed, and ends when that thread ends.
fn run_local_timers(shared: Arc<LocalShared>) {
    let mut timers = lock(&shared.timers);
    loop {
        if timers.closed {
            timers.thread_running = false;
            return;
        }
        let Some(&(w, id)) = timers.due.first() else {
            timers = shared
                .timers_changed
                .wait(timers)
                .unwrap_or_else(|e| e.into_inner());
            continue;
        };
        let now = Instant::now();
        if now < w {
            timers = shared
                .timers_changed
                .wait_timeout(timers, w - now)
                .unwrap_or_else(|e| e.into_inner())
                .0;
            continue;
        }
        timers.disarm(id);
        *timers.queued.entry(id).or_default() += 1;
        lock(&shared.queue).push_back(Entry::Timer(id));
        shared.ready.notify_all();
        drop(timers);
        let waker = lock(&shared.waker).clone();
        if let Some(waker) = waker {
            waker();
        }
        timers = lock(&shared.timers);
    }
}

// Go: time/sleep.go:24 when
fn when(d: Duration) -> Instant {
    let now = Instant::now();
    match now.checked_add(d) {
        Some(t) => t,
        // PORT: Go clamps to MaxInt64 nanoseconds.
        None => {
            let mut d = Duration::from_nanos(i64::MAX as u64);
            loop {
                if let Some(t) = now.checked_add(d) {
                    return t;
                }
                d /= 2;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs the ready work of this thread until no timer is armed.
    fn run_until_idle() {
        while wait_pending() {
            run_pending();
        }
    }

    #[test]
    fn timers_of_one_thread_fire_in_deadline_order_after_stop_and_reset() {
        // A thread of its own: the timer thread ends with it.
        std::thread::spawn(|| {
            let log = Rc::new(RefCell::new(Vec::new()));
            let push = |name: &'static str| -> Box<dyn FnMut()> {
                let log = log.clone();
                Box::new(move || log.borrow_mut().push(name))
            };
            let late = after_func(Duration::from_millis(40), push("late"));
            let early = after_func(Duration::from_millis(10), push("early"));
            let stopped = after_func(Duration::from_millis(5), push("stopped"));
            assert!(stopped.stop());
            assert!(!stopped.stop());
            run_until_idle();
            assert_eq!(*log.borrow(), ["early", "late"]);
            // Reset after it fired: false, and it runs again.
            assert!(!early.reset(Duration::from_millis(1)));
            // Reset while armed moves the deadline: true.
            assert!(!late.reset(Duration::from_millis(30)));
            assert!(late.reset(Duration::from_millis(2)));
            run_until_idle();
            assert_eq!(*log.borrow(), ["early", "late", "early", "late"]);
        })
        .join()
        .expect("timer test thread");
    }

    // `next_idle` gives the start of the oldest idle job, and `run_idle`
    // runs the jobs in queue order.
    #[test]
    fn idle_jobs_run_in_order_and_give_their_start() {
        std::thread::spawn(|| {
            let log = Rc::new(RefCell::new(Vec::new()));
            assert_eq!(next_idle(), None);
            for (start, name) in [
                (IdleStart::AtOnce, "warm"),
                (IdleStart::AfterQuiet, "retry"),
            ] {
                let log = log.clone();
                go_idle(start, Box::new(move || log.borrow_mut().push(name)));
            }
            assert_eq!(next_idle(), Some(IdleStart::AtOnce));
            run_pending();
            assert!(
                log.borrow().is_empty(),
                "run_pending does not run idle work"
            );
            assert!(run_idle());
            assert_eq!(next_idle(), Some(IdleStart::AfterQuiet));
            assert!(run_idle());
            assert_eq!(next_idle(), None);
            assert!(!run_idle());
            assert_eq!(*log.borrow(), ["warm", "retry"]);
        })
        .join()
        .expect("idle test thread");
    }

    // Only the first release after a client pause waits for
    // `drop_garbage`, with the other values of its message, and only when the
    // client has not sent its next edit and no kept release waits.
    #[test]
    fn drop_after_pause_keeps_one_release_after_a_client_pause() {
        std::thread::spawn(|| {
            keep_garbage();
            let stream = Rc::new(Cell::new(false));
            {
                let stream = stream.clone();
                set_stream_check(Box::new(move || stream.get()));
            }
            // A release of two values. Returns how many wait.
            let release = || {
                let values = [Rc::new(()), Rc::new(())];
                for value in &values {
                    drop_after_pause(Box::new(value.clone()));
                }
                values.iter().filter(|v| Rc::strong_count(v) == 2).count()
            };
            // One message after `gap_ms` with a release, as the dispatch
            // loop runs it.
            let message = |gap_ms: u64| {
                drop_garbage(|| stream.get());
                note_message_gap(Duration::from_millis(gap_ms));
                release()
            };
            assert_eq!(message(0), 0, "no pause before the first message");
            assert_eq!(message(20), 2, "the first release after a pause of 20 ms");
            assert_eq!(message(0), 0, "the next release of the burst");
            assert_eq!(garbage_len(), 0, "drop_garbage after the answer");
            assert_eq!(message(5), 0, "a new burst with no pause");

            stream.set(true);
            assert_eq!(message(500), 0, "the next edit waits at the release");
            stream.set(false);

            // A message came while the kept release was freed.
            assert_eq!(message(500), 2);
            drop_garbage(|| true);
            note_message_gap(Duration::from_millis(500));
            assert_eq!(release(), 0, "a kept release waits");

            // A message with no release after the pause.
            drop_garbage(|| false);
            note_message_gap(Duration::from_millis(500));
            assert_eq!(message(0), 2, "the first release of the burst");
            drop_garbage(|| false);
            assert_eq!(garbage_len(), 0);
        })
        .join()
        .expect("drop_after_pause test thread");
    }
}
