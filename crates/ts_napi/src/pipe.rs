//! A one-way in-memory queue between threads, with a close reason: the
//! frame pipes between the JS thread and the session thread, and the
//! session thread's command queue.
//!
//! A reader that finds the queue empty spins before it sleeps on the
//! condition variable: a futex wake costs about 28 µs per round trip, where
//! a spinning reader sees the item in under 1 µs (measured on Linux x64).
//! It spins for `SPIN` only while most of its recent
//! waits on the pipe ended within `SPIN`, else for `SHORT_SPIN`: in
//! typescript-eslint's rule tests most waits are longer (the JS thread
//! works between requests, a request checks a file), and a long spin there
//! only burns a core that the other test workers need.

use std::collections::VecDeque;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

/// How long a reader spins on an empty pipe before it sleeps, while its
/// waits are short.
const SPIN: Duration = Duration::from_micros(50);

/// How long it spins while its waits are long.
const SHORT_SPIN: Duration = Duration::from_micros(2);

/// Spins between two reads of the clock.
const SPINS_PER_CLOCK: u32 = 64;

/// `Pipe::short_waits` in fixed point: 1 is `ALL`.
const ALL: u32 = 1 << 16;

/// The weight of the newest wait in `Pipe::short_waits`.
const WAIT_WEIGHT_SHIFT: u32 = 3;

/// Nanoseconds since the first call in the process.
fn clock_ns(at: Instant) -> u64 {
    static ORIGIN: OnceLock<Instant> = OnceLock::new();
    let origin = *ORIGIN.get_or_init(Instant::now);
    u64::try_from(at.saturating_duration_since(origin).as_nanos()).unwrap_or(u64::MAX)
}

pub struct Pipe<T> {
    state: Mutex<State<T>>,
    ready: Condvar,
    /// Counts pushes and the close: a spinning reader watches it without
    /// the lock.
    version: AtomicU64,
    /// When the last push came (`clock_ns`), so that a reader learns how
    /// long it waited even when it slept.
    last_push: AtomicU64,
    /// A moving share, out of `ALL`, of the reader's waits that ended
    /// within `SPIN`.
    short_waits: AtomicU32,
}

struct State<T> {
    items: VecDeque<T>,
    closed: Option<String>,
    /// Readers that sleep on `ready`.
    sleepers: usize,
}

impl<T> Pipe<T> {
    pub fn new() -> Self {
        Pipe {
            state: Mutex::new(State {
                items: VecDeque::new(),
                closed: None,
                sleepers: 0,
            }),
            ready: Condvar::new(),
            version: AtomicU64::new(0),
            last_push: AtomicU64::new(0),
            short_waits: AtomicU32::new(ALL),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State<T>> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Appends `items`. False when the pipe is closed: they are dropped.
    pub fn push(&self, items: impl IntoIterator<Item = T>) -> bool {
        let mut state = self.lock();
        if state.closed.is_some() {
            return false;
        }
        state.items.extend(items);
        self.last_push
            .store(clock_ns(Instant::now()), Ordering::Relaxed);
        self.version.fetch_add(1, Ordering::Release);
        let wake = state.sleepers > 0;
        drop(state);
        if wake {
            self.ready.notify_all();
        }
        true
    }

    /// Appends `item`, or gives it back when the pipe is closed.
    pub fn send(&self, item: T) -> Result<(), T> {
        let mut state = self.lock();
        if state.closed.is_some() {
            return Err(item);
        }
        state.items.push_back(item);
        self.last_push
            .store(clock_ns(Instant::now()), Ordering::Relaxed);
        self.version.fetch_add(1, Ordering::Release);
        let wake = state.sleepers > 0;
        drop(state);
        if wake {
            self.ready.notify_all();
        }
        Ok(())
    }

    /// Closes the pipe. The items in it can still be read. The first
    /// reason stays.
    pub fn close(&self, reason: &str) {
        let mut state = self.lock();
        if state.closed.is_none() {
            state.closed = Some(reason.to_string());
        }
        self.version.fetch_add(1, Ordering::Release);
        drop(state);
        self.ready.notify_all();
    }

    pub fn is_closed(&self) -> bool {
        self.lock().closed.is_some()
    }

    /// The close reason, or `None` while the pipe is open.
    pub fn reason(&self) -> Option<String> {
        self.lock().closed.clone()
    }

    /// Closed, with nothing left to read.
    pub fn is_finished(&self) -> bool {
        let state = self.lock();
        state.closed.is_some() && state.items.is_empty()
    }

    /// The next item without a wait: `None` when the pipe is empty and
    /// open, `Some(Err)` with the close reason when it is empty and closed.
    pub fn pop_now(&self) -> Option<Result<T, String>> {
        let mut state = self.lock();
        if let Some(item) = state.items.pop_front() {
            return Some(Ok(item));
        }
        state.closed.clone().map(Err)
    }

    /// The next item, after a wait for one. `Err` with the close reason
    /// once the pipe is closed and empty.
    pub fn pop(&self) -> Result<T, String> {
        if let Some(next) = self.pop_now() {
            return next;
        }
        let start = Instant::now();
        let spin = if self.short_waits.load(Ordering::Relaxed) >= ALL / 2 {
            SPIN
        } else {
            SHORT_SPIN
        };
        let next = self.wait(start, spin);
        if next.is_ok() {
            let waited = self
                .last_push
                .load(Ordering::Relaxed)
                .saturating_sub(clock_ns(start));
            self.learn(Duration::from_nanos(waited) <= SPIN);
        }
        next
    }

    fn wait(&self, start: Instant, spin: Duration) -> Result<T, String> {
        loop {
            let seen = self.version.load(Ordering::Acquire);
            let mut state = self.lock();
            if let Some(item) = state.items.pop_front() {
                return Ok(item);
            }
            if let Some(reason) = &state.closed {
                return Err(reason.clone());
            }
            if start.elapsed() >= spin {
                state.sleepers += 1;
                while state.items.is_empty() && state.closed.is_none() {
                    state = self
                        .ready
                        .wait(state)
                        .unwrap_or_else(PoisonError::into_inner);
                }
                state.sleepers -= 1;
                continue;
            }
            drop(state);
            self.spin_while_unchanged(seen, start, spin);
        }
    }

    /// Adds a wait to `short_waits`. A pipe has one reader, so a plain load
    /// and store suffice.
    fn learn(&self, short: bool) {
        let share = self.short_waits.load(Ordering::Relaxed);
        let newest = if short { ALL >> WAIT_WEIGHT_SHIFT } else { 0 };
        self.short_waits.store(
            share - (share >> WAIT_WEIGHT_SHIFT) + newest,
            Ordering::Relaxed,
        );
    }

    /// Spins until `version` moves past `seen` or `spin` has passed since
    /// `start`.
    fn spin_while_unchanged(&self, seen: u64, start: Instant, spin: Duration) {
        loop {
            for _ in 0..SPINS_PER_CLOCK {
                if self.version.load(Ordering::Acquire) != seen {
                    return;
                }
                std::hint::spin_loop();
            }
            if start.elapsed() >= spin {
                return;
            }
        }
    }
}
