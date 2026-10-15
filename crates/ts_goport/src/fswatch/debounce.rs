//! Go: internal/fswatch/debounce.go (the per-backend event debouncer).
//!
//! PORT: the loop goroutine is a `std::thread`. Go `d.mu` and `d.latchMu`
//! are `std::sync::Mutex`es over the fields they guard (`DebounceLocked`,
//! `DebounceLatch`). The latch channels are `SignalChan`s.

use crate::fswatch::prelude::*;

use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

// Go: debounce.go:9 defaultMinWaitTime, defaultMaxWaitTime
pub const DEFAULT_MIN_WAIT_TIME: Duration = Duration::from_millis(50);
pub const DEFAULT_MAX_WAIT_TIME: Duration = Duration::from_millis(500);

// Go: debounce.go:14 minWaitTime, maxWaitTime
// PORT: Go package vars that only tests change; statics here.
pub static MIN_WAIT_TIME: Duration = DEFAULT_MIN_WAIT_TIME;
pub static MAX_WAIT_TIME: Duration = DEFAULT_MAX_WAIT_TIME;

/// PORT: not in Go. How many debouncers deliver a fire now
/// (`fire_callbacks`). In Go a callback that sends on a channel
/// (watchmanager `signalDoCycle`) makes the receiver the next goroutine of
/// the sender's P, so the receiver nearly always runs after the debounce
/// goroutine has delivered the whole fire. A port thread wakes at once on
/// another CPU, so the receiver waits for the fire (`wait_for_fires`).
static FIRING: Mutex<usize> = Mutex::new(0);
static FIRES_DONE: Condvar = Condvar::new();

/// PORT: not in Go. Waits until no debouncer delivers a fire (see `FIRING`).
pub fn wait_for_fires() {
    let mut firing = FIRING.lock().unwrap();
    while *firing > 0 {
        firing = FIRES_DONE.wait(firing).unwrap();
    }
}

/// Counts one fire in `FIRING` while it lives, also when a callback panics.
struct Firing;

impl Firing {
    fn start() -> Firing {
        *FIRING.lock().unwrap() += 1;
        Firing
    }
}

impl Drop for Firing {
    fn drop(&mut self) {
        let mut firing = FIRING
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *firing -= 1;
        if *firing == 0 {
            FIRES_DONE.notify_all();
        }
    }
}

// Go: debounce.go:28 debounce
/// debounce batches filesystem events for one backend. Each *watcher
/// owns one debounce instance, created lazily on first subscribe and
/// living for the process lifetime. The background goroutine costs
/// nothing when idle.
///
/// Per-backend (rather than process-wide) isolation means a slow user
/// callback on one backend cannot starve event delivery on the others.
///
/// Internally uses a resettable latch: the loop blocks until trigger()
/// is called, then coalesces for minWaitTime before firing callbacks.
pub struct Debounce {
    pub mu: Mutex<DebounceLocked>,

    // Latch state: waitCh is the persistent gate (closed = signalled),
    // triggerCh is replaced on each trigger for timed waits.
    pub latch_mu: Mutex<DebounceLatch>,
}

/// PORT: the `debounce` fields that Go `d.mu` guards.
#[derive(Default)]
pub struct DebounceLocked {
    /// Go `map[any]func()`. PORT: the key is the dirWatch pointer
    /// (`Arc::as_ptr(..) as usize`).
    pub callbacks: FxHashMap<usize, Arc<dyn Fn() + Send + Sync>>,
    /// PORT: `None` is Go's zero `time.Time`.
    pub last_time: Option<Instant>,
}

/// PORT: the `debounce` fields that Go `d.latchMu` guards. `None` is a nil
/// channel.
#[derive(Default)]
pub struct DebounceLatch {
    pub wait_ch: Option<Arc<SignalChan>>,
    pub trigger_ch: Option<Arc<SignalChan>>,
    pub notified: bool,
}

// Go: debounce.go:41 newDebounce
pub fn new_debounce() -> Arc<Debounce> {
    let d = Arc::new(Debounce {
        mu: Mutex::new(DebounceLocked {
            callbacks: FxHashMap::default(),
            last_time: None,
        }),
        latch_mu: Mutex::new(DebounceLatch::default()),
    });
    let loop_d = d.clone();
    std::thread::spawn(move || loop_d.loop_());
    d
}

impl Debounce {
    // Go: debounce.go:50 debounce.add
    /// add registers a callback under key.
    pub fn add(&self, key: usize, cb: Arc<dyn Fn() + Send + Sync>) {
        let mut d = self.mu.lock().unwrap();
        d.callbacks.insert(key, cb);
    }

    // Go: debounce.go:57 debounce.remove
    /// remove deregisters the callback for key.
    pub fn remove(&self, key: usize) {
        let mut d = self.mu.lock().unwrap();
        d.callbacks.remove(&key);
    }

    // Go: debounce.go:64 debounce.trigger
    /// trigger wakes the debounce loop.
    pub fn trigger(&self) {
        let mut latch = self.latch_mu.lock().unwrap();
        if !latch.notified {
            latch.notified = true;
            Debounce::wait_ch_locked(&mut latch).close();
        }
        Debounce::trigger_ch_locked(&mut latch).close();
        latch.trigger_ch = Some(Arc::new(SignalChan::new()));
    }

    // Go: debounce.go:75 debounce.loop
    pub fn loop_(&self) {
        loop {
            self.latch_wait();
            self.notify_if_ready();
        }
    }

    // Go: debounce.go:82 debounce.notifyIfReady
    pub fn notify_if_ready(&self) {
        let mut d = self.mu.lock().unwrap();
        let now = Instant::now();
        // PORT: Go `now.Sub(d.lastTime)` with a zero lastTime is a huge gap.
        let gap_exceeds_max = match d.last_time {
            None => true,
            Some(last_time) => now.duration_since(last_time) > MAX_WAIT_TIME,
        };
        if gap_exceeds_max {
            d.last_time = Some(now);
            drop(d);
            self.fire_callbacks();
            return;
        }
        drop(d);
        self.coalesce_wait();
    }

    // Go: debounce.go:96 debounce.coalesceWait
    pub fn coalesce_wait(&self) {
        let ch = {
            let mut latch = self.latch_mu.lock().unwrap();
            Debounce::trigger_ch_locked(&mut latch)
        };
        if ch.wait_timeout(MIN_WAIT_TIME) {
            // Do nothing; new event triggered, fire on the next tick.
        } else {
            self.fire_callbacks();
        }
    }

    // Go: debounce.go:109 debounce.fireCallbacks
    /// fireCallbacks snapshots and invokes all registered callbacks.
    pub fn fire_callbacks(&self) {
        let _firing = Firing::start();
        let cbs: Vec<Arc<dyn Fn() + Send + Sync>> = {
            let mut d = self.mu.lock().unwrap();
            d.last_time = Some(Instant::now());
            let mut cbs: Vec<Arc<dyn Fn() + Send + Sync>> = Vec::with_capacity(d.callbacks.len());
            for cb in d.callbacks.values() {
                cbs.push(cb.clone());
            }
            cbs
        };

        self.latch_reset();

        for cb in &cbs {
            cb();
        }
    }

    // ----- latch helpers (replace signal_) ------------------------------------

    // Go: debounce.go:127 debounce.waitChLocked
    // PORT: Go calls it with d.latchMu held; it takes the guarded data.
    pub fn wait_ch_locked(latch: &mut DebounceLatch) -> Arc<SignalChan> {
        latch
            .wait_ch
            .get_or_insert_with(|| Arc::new(SignalChan::new()))
            .clone()
    }

    // Go: debounce.go:134 debounce.triggerChLocked
    // PORT: Go calls it with d.latchMu held; it takes the guarded data.
    pub fn trigger_ch_locked(latch: &mut DebounceLatch) -> Arc<SignalChan> {
        latch
            .trigger_ch
            .get_or_insert_with(|| Arc::new(SignalChan::new()))
            .clone()
    }

    // Go: debounce.go:141 debounce.latchWait
    pub fn latch_wait(&self) {
        let ch = {
            let mut latch = self.latch_mu.lock().unwrap();
            Debounce::wait_ch_locked(&mut latch)
        };
        ch.wait();
    }

    // Go: debounce.go:148 debounce.latchReset
    pub fn latch_reset(&self) {
        let mut latch = self.latch_mu.lock().unwrap();
        if latch.notified {
            latch.notified = false;
            latch.wait_ch = Some(Arc::new(SignalChan::new()));
        }
    }
}
