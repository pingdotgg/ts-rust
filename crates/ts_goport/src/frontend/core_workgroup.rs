//! Go `internal/core/workgroup.go`.
//!
//! PORT: work groups in the language service run over dispatch-thread state
//! (`Rc`, `RefCell`), so they run serially (PORTING "Go runtime";
//! `project/dirty/interfaces.rs`, decision 1). `new_work_group` always makes
//! the single-threaded group. The trait has a lifetime so queued functions
//! can borrow the caller's locals, as Go closures do.

use crate::frontend::prelude::*;
use crate::gostd::{Context, GoError};
use std::cell::Cell;
use std::collections::VecDeque;
use std::sync::mpsc::{Receiver, SyncSender, TrySendError};

// Go: core/workgroup.go:11 WorkGroup
pub trait WorkGroup<'a> {
    // Queue queues a function to run. It may be invoked immediately, or deferred until RunAndWait.
    // It is not safe to call Queue after RunAndWait has returned.
    fn queue(&self, fn_: Box<dyn FnOnce() + 'a>);

    // RunAndWait runs all queued functions, blocking until they have all completed.
    fn run_and_wait(&self);
}

// Go: core/workgroup.go:20 NewWorkGroup
// PORT: Go makes the parallel group when `singleThreaded` is false. Every
// language-service work group runs on the dispatch thread, so the port
// always makes the single-threaded group. Its functions run last-in
// first-out, as in Go's single-threaded group.
// PORT: returns the concrete group, not `Rc<dyn WorkGroup<'a>>` (lust has no
// lifetime arguments of a trait in a `dyn` type).
pub fn new_work_group<'a>(single_threaded: bool) -> Rc<SingleThreadedWorkGroup<'a>> {
    let _ = single_threaded;
    Rc::new(SingleThreadedWorkGroup {
        done: Cell::new(false),
        fns: RefCell::new(Vec::new()),
    })
}

// PORT: Go `defer w.done.Store(true)` in RunAndWait.
struct DoneOnDrop<'g>(&'g Cell<bool>);

impl Drop for DoneOnDrop<'_> {
    fn drop(&mut self) {
        self.0.set(true);
    }
}

// Go: core/workgroup.go:27 parallelWorkGroup
// PORT: never made (see `new_work_group`). Go `wg.Go(fn)` starts a
// goroutine; here the function waits in a first-in first-out list, and
// RunAndWait (Go `wg.Wait()`) runs the list in start order.
pub struct ParallelWorkGroup<'a> {
    pub done: Cell<bool>,
    pub wg: RefCell<VecDeque<Box<dyn FnOnce() + 'a>>>,
}

impl<'a> WorkGroup<'a> for ParallelWorkGroup<'a> {
    // Go: core/workgroup.go:34 Queue
    fn queue(&self, fn_: Box<dyn FnOnce() + 'a>) {
        if self.done.get() {
            panic!("Queue called after RunAndWait returned");
        }

        // Go: w.wg.Go(func() { fn() })
        self.wg.borrow_mut().push_back(fn_);
    }

    // Go: core/workgroup.go:44 RunAndWait
    fn run_and_wait(&self) {
        let _done = DoneOnDrop(&self.done);
        // Go: w.wg.Wait()
        loop {
            let fn_ = self.wg.borrow_mut().pop_front();
            let Some(fn_) = fn_ else {
                return;
            };
            fn_();
        }
    }
}

// Go: core/workgroup.go:49 singleThreadedWorkGroup
// PORT: `fnsMu` is dropped (one thread).
pub struct SingleThreadedWorkGroup<'a> {
    pub done: Cell<bool>,
    pub fns: RefCell<Vec<Box<dyn FnOnce() + 'a>>>,
}

impl<'a> WorkGroup<'a> for SingleThreadedWorkGroup<'a> {
    // Go: core/workgroup.go:57 Queue
    fn queue(&self, fn_: Box<dyn FnOnce() + 'a>) {
        if self.done.get() {
            panic!("Queue called after RunAndWait returned");
        }

        self.fns.borrow_mut().push(fn_);
    }

    // Go: core/workgroup.go:67 RunAndWait
    fn run_and_wait(&self) {
        let _done = DoneOnDrop(&self.done);
        loop {
            let Some(fn_) = self.pop() else {
                return;
            };
            fn_();
        }
    }
}

impl<'a> SingleThreadedWorkGroup<'a> {
    // Go: core/workgroup.go:78 pop
    pub fn pop(&self) -> Option<Box<dyn FnOnce() + 'a>> {
        // Go: take the last function and clear its slot (Allow GC).
        self.fns.borrow_mut().pop()
    }
}

// Go: core/workgroup.go:92 ThrottleGroup
// ThrottleGroup is like errgroup.Group but with global concurrency limiting via a semaphore.
// PORT: serial. Go `semaphore chan struct{}` is the `sync_channel` pair
// (PORTING "Go runtime"); Go `group *errgroup.Group` is its first error,
// because the functions run on the dispatch thread and `gostd::errgroup`
// runs threads.
pub struct ThrottleGroup<'s> {
    pub semaphore: &'s (SyncSender<()>, Receiver<()>),
    pub err: RefCell<Option<GoError>>,
}

// Go: core/workgroup.go:98 NewThrottleGroup
// NewThrottleGroup creates a new ThrottleGroup with the given context and semaphore for concurrency limiting.
// PORT: Go `errgroup.WithContext(ctx)` makes a child context that the
// ThrottleGroup drops unread (`g, _ :=`) and only cancels in Wait. Nothing
// observes it, so the port does not make it.
pub fn new_throttle_group<'s>(
    ctx: &Context,
    semaphore: &'s (SyncSender<()>, Receiver<()>),
) -> ThrottleGroup<'s> {
    let _ = ctx;
    ThrottleGroup {
        semaphore,
        err: RefCell::new(None),
    }
}

impl ThrottleGroup<'_> {
    // Go: core/workgroup.go:108 Go
    // Go runs the given function in a new goroutine, but first acquires a slot from the semaphore.
    // The semaphore slot is released when the function completes.
    // PORT: the goroutine body runs now, in call order.
    pub fn go(&self, fn_: impl FnOnce() -> Result<(), GoError>) {
        // Go: tg.group.Go(func() error { ... })
        let result = {
            // Acquire semaphore slot - this will block until a slot is available
            match self.semaphore.0.try_send(()) {
                Ok(()) => {}
                // PORT: on one thread nothing can free a slot while this
                // waits; Go would block here forever.
                Err(TrySendError::Full(())) => {
                    panic!("ThrottleGroup: semaphore full on the dispatch thread")
                }
                Err(TrySendError::Disconnected(())) => panic!("ThrottleGroup: semaphore closed"),
            }
            let result = fn_();
            // Release semaphore slot when done
            let _ = self.semaphore.1.try_recv();
            result
        };
        // Go: errgroup keeps the first error (errOnce).
        if let Err(err) = result {
            let mut first = self.err.borrow_mut();
            if first.is_none() {
                *first = Some(err);
            }
        }
    }

    // Go: core/workgroup.go:121 Wait
    // Wait waits for all goroutines to complete and returns the first error encountered, if any.
    pub fn wait(&self) -> Result<(), GoError> {
        match self.err.borrow().clone() {
            Some(err) => Err(err),
            None => Ok(()),
        }
    }
}
