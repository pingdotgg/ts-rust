//! Go `internal/project/background/queue.go`.
//!
//! PORT: background tasks touch dispatch-thread state, so `wg.Go` posts the
//! task to `gostd::local::go` (PORTING "Go runtime"). The dispatch loop runs
//! it later, in enqueue order. A Go task that sleeps (`time.After`) goes on
//! in a `gostd::local::after_func` timer; it takes a `TaskHold` so that it
//! still counts as running until then. `Wait` runs
//! `gostd::local::run_pending`, and waits for due timers, until this queue's
//! tasks have finished. `mu` is dropped (one thread).

use crate::project::background::prelude::*;
use std::cell::Cell;

// Go: project/background/queue.go:9 Queue
// Queue manages background tasks execution
pub struct Queue {
    // PORT: Go `wg sync.WaitGroup`: the number of enqueued tasks that have
    // not finished. Shared with the posted tasks.
    wg: Rc<Cell<i32>>,
    closed: Cell<bool>,
}

// Go: project/background/queue.go:16 NewQueue
// NewQueue creates a new background queue for managing background tasks execution.
pub fn new_queue() -> Rc<Queue> {
    Rc::new(Queue {
        wg: Rc::new(Cell::new(0)),
        closed: Cell::new(false),
    })
}

// PORT: Go `defer wg.Done()` inside `wg.Go`.
struct WaitGroupDone(Rc<Cell<i32>>);

impl Drop for WaitGroupDone {
    fn drop(&mut self) {
        self.0.set(self.0.get() - 1);
    }
}

/// PORT: the rest of a running task that goes on later on the dispatch
/// thread (Go: the same goroutine after a sleep). A task takes it with
/// `Queue::hold` and moves it into the timer function; the task counts as
/// running until the hold is dropped. The timer function runs the rest
/// under `core::go_wait_group_task`, as `Enqueue` runs the task.
pub struct TaskHold {
    _done: WaitGroupDone,
}

impl Queue {
    // Go: project/background/queue.go:20 Enqueue
    // ts#64544: Go now adds the task to `wg` before it releases `mu`, so a
    // Close that comes in between waits for it. One thread here: no change.
    pub fn enqueue(&self, ctx: &Context, fn_: impl FnOnce(&Context) + 'static) {
        if self.closed.get() {
            return;
        }

        // Don't start new tasks if context is already cancelled
        if ctx.err().is_some() {
            return;
        }

        // Go: q.wg.Go(func() { ... })
        self.wg.set(self.wg.get() + 1);
        let wg = self.wg.clone();
        let ctx = ctx.clone();
        gostd::local::go(Box::new(move || {
            let _done = WaitGroupDone(wg);
            // Check context again before executing
            if ctx.err().is_some() {
                return;
            }
            // Go: the wg.Go goroutine recovers a panic and raises it again.
            crate::core::go_wait_group_task(|| fn_(&ctx));
        }));
    }

    /// PORT: called by a running task whose rest runs later (see
    /// `TaskHold`). No Go counterpart: the Go task is one goroutine.
    pub fn hold(&self) -> TaskHold {
        self.wg.set(self.wg.get() + 1);
        TaskHold {
            _done: WaitGroupDone(self.wg.clone()),
        }
    }

    // Go: project/background/queue.go:47 Wait
    // Wait waits for all active tasks to complete.
    // It does not prevent new tasks from being enqueued while waiting.
    pub fn wait(&self) {
        while self.wg.get() > 0 {
            // PORT: a task that sleeps waits for its timer. Go blocks forever
            // when the tasks can never finish (for example, Wait called from
            // inside one of them). The port panics instead.
            if !gostd::local::wait_pending() {
                panic!(
                    "background.Queue.Wait: {} task(s) can not finish on the dispatch thread",
                    self.wg.get()
                );
            }
            gostd::local::run_pending();
        }
    }

    // Go: project/background/queue.go:51 Close
    // ts#64544: Close waits for the tasks that are running.
    pub fn close(&self) {
        self.closed.set(true);
        self.wait();
    }
}
