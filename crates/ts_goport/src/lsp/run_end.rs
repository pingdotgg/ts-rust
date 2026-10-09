//! PORT: not a Go file. The end of `Server::run` while the dispatch thread
//! runs the work of a Go goroutine.
//!
//! Go's `Run` (server.go:859) returns when its group of three goroutines
//! returns: the dispatch loop, the write loop and the wait on `ctx.Done()`
//! and the read loop. After a SIGINT, a SIGTERM, the parent watchdog or the
//! end of stdin, the context is done: the write loop and the wait return at
//! once, and the dispatch loop returns at its next `requestQueue.Get`
//! (server.go:972). The async part of a request (server.go:1017) and the
//! other background work run on goroutines of their own, so `Run` does not
//! wait for them: `runLSP` (cmd/tsc/lsp.go:69) prints the error and `main`
//! calls `os.Exit` while they still run.
//!
//! The port runs that goroutine work inline on the dispatch thread (file
//! header of server.rs), so the dispatch loop cannot return until it ends.
//! `RunEnd` keeps where the dispatch thread is in Go's terms
//! (`DispatchPhase`). When the context is done while the thread runs such
//! work, a watcher thread ends the run in its place: it waits for the write
//! loop and the wait, takes the result that Go's `Run` returns, and gives
//! it to the caller's `EarlyEnd`, which ends the process as `runLSP` and
//! `main` do. Without an `EarlyEnd` (tests, other callers) the run ends as
//! before, when the work ends.

use crate::gostd::{Context, GoError, errors};
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};

/// The end of a run that the dispatch thread cannot reach yet: it gets the
/// result of Go's `Run` and ends the process (it does not return).
pub type EarlyEnd = Box<dyn FnOnce(Result<(), GoError>) + Send>;

/// Where the dispatch thread is, in the terms of Go's dispatch goroutine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DispatchPhase {
    /// In `requestQueue.Get` or on the way to it: a done context ends the
    /// dispatch loop at once, here as in Go.
    Get,
    /// The sync part of a handler: Go's dispatch goroutine runs it, so
    /// Go's `Run` waits for it too.
    Sync,
    /// Work that Go runs on other goroutines: the async part of a request,
    /// `gostd::local` work and idle work. Go's dispatch loop is in `Get`.
    Work,
}

struct State {
    phase: DispatchPhase,
    /// The run has ended: `Server::run` returns, or the watcher ended it.
    ended: bool,
}

/// The results of the write loop and of the wait (Go's other two group
/// members), in the order they return.
struct Others {
    returned: u8,
    first_err: Option<GoError>,
}

pub struct RunEnd {
    state: Mutex<State>,
    cond: Condvar,
    others: Mutex<Others>,
    others_cond: Condvar,
}

// PORT: Go mutexes do not poison.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Default for RunEnd {
    fn default() -> RunEnd {
        RunEnd {
            state: Mutex::new(State {
                phase: DispatchPhase::Get,
                ended: false,
            }),
            cond: Condvar::new(),
            others: Mutex::new(Others {
                returned: 0,
                first_err: None,
            }),
            others_cond: Condvar::new(),
        }
    }
}

impl RunEnd {
    /// The dispatch thread moves to `phase`.
    pub fn set_phase(&self, phase: DispatchPhase) {
        let mut state = lock(&self.state);
        if state.phase != phase {
            state.phase = phase;
            self.cond.notify_all();
        }
    }

    /// The write loop or the wait has returned `result`. Go's group keeps
    /// the first error.
    pub fn other_returned(&self, result: &Result<(), GoError>) {
        let mut others = lock(&self.others);
        others.returned += 1;
        if let Err(err) = result
            && others.first_err.is_none()
        {
            others.first_err = Some(err.clone());
        }
        self.others_cond.notify_all();
    }

    /// `Server::run` is about to return on the dispatch thread. False when
    /// the watcher has ended the run: the process is ending, so the caller
    /// must not go on.
    pub fn end_on_dispatch_thread(&self) -> bool {
        let mut state = lock(&self.state);
        if state.ended {
            return false;
        }
        state.ended = true;
        self.cond.notify_all();
        true
    }

    /// The watcher thread of a run with an `EarlyEnd`. `ctx` is the
    /// context of Go's group.
    pub fn watch(&self, ctx: &Context, early_end: EarlyEnd) {
        let Some(done) = ctx.done() else {
            return;
        };
        done.wait();
        // Go's `g.Wait()`: the write loop and the wait return on the cancel.
        let first_err = {
            let mut others = lock(&self.others);
            while others.returned < 2 {
                others = self
                    .others_cond
                    .wait(others)
                    .unwrap_or_else(PoisonError::into_inner);
            }
            others.first_err.clone()
        };
        // The dispatch loop: Go's returns at its next `Get`, after the sync
        // part of a handler. When the port's thread is at `Get`, it returns
        // by itself (`end_on_dispatch_thread`).
        {
            let mut state = lock(&self.state);
            loop {
                if state.ended {
                    return;
                }
                if state.phase == DispatchPhase::Work {
                    state.ended = true;
                    break;
                }
                state = self
                    .cond
                    .wait(state)
                    .unwrap_or_else(PoisonError::into_inner);
            }
        }
        // Go: server.go:877, the result of `Run`.
        let result = match first_err {
            Some(err) if !errors::is(&err, &errors::EOF) && ctx.err().is_some() => Err(err),
            _ => Ok(()),
        };
        early_end(result);
    }
}
