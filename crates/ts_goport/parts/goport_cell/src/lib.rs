//! `ZeroOnceLock`: std's `OnceLock` (`get`, `set`, `get_or_init`) with an
//! empty state of all zero bytes. A static array of empty cells is then
//! `.bss`, which takes no bytes in the binary file.
//!
//! bssblocks1: std `Once` starts at INCOMPLETE = 3 (Rust 1.95,
//! `sys/sync/once/futex.rs`), so the AST registry entry arrays (`ast/store.rs`
//! `FILE_BLOCKS`, 64-byte entries, and `HIGH_BLOCKS`) as `OnceLock` arrays
//! were 4,456,384 bytes of `.data`. The value comes first and the state after
//! it, as in `OnceLock<T>`: the hot entry read of `file_block` is then the
//! code it was with `OnceLock` (the `once_cell` cell puts its state first, which
//! made each inlined read 6 bytes longer, +245 KB of `.text` in the release
//! tsc).

use std::cell::UnsafeCell;
use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicU8, Ordering};

/// No value. Zero, so that an empty cell is all zero bytes.
const EMPTY: u8 = 0;
/// One `set` owns the value slot and is writing it.
const WRITING: u8 = 1;
/// The value is written and never changes again.
const READY: u8 = 2;

/// A cell that is set at most once and then read by any thread.
///
/// Publication as `OnceLock`: `set` writes the value, then stores `READY`
/// with Release; `get` gives the value only after an Acquire load of
/// `READY`, so a reader that gets a value sees all of it. Two differences,
/// which no caller needs: a `set` while another `set` writes returns `Err`
/// at once (`OnceLock::set` waits for the other one, then returns `Err`),
/// and `get_or_init` can run `init` on more than one thread at once (one
/// value is kept, the others are dropped).
#[repr(C)]
pub struct ZeroOnceLock<T> {
    /// Written once, by the `set` that moves the state from `EMPTY`.
    value: UnsafeCell<MaybeUninit<T>>,
    /// `EMPTY`, `WRITING` or `READY`. It only goes up.
    state: AtomicU8,
}

// SAFETY: as std `OnceLock`: `get` shares `&T` between threads, so `T: Sync`;
// a value that one thread sets can be dropped by another, so `T: Send`.
// `Send` is the auto trait: `UnsafeCell<MaybeUninit<T>>` is `Send` when `T`
// is.
#[allow(unsafe_code)]
unsafe impl<T: Sync + Send> Sync for ZeroOnceLock<T> {}

impl<T> Default for ZeroOnceLock<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> ZeroOnceLock<T> {
    /// An empty cell: all zero bytes but the bytes of the value slot, which
    /// stay uninitialized (zero in a static).
    #[must_use]
    pub const fn new() -> Self {
        Self {
            value: UnsafeCell::new(MaybeUninit::uninit()),
            state: AtomicU8::new(EMPTY),
        }
    }

    /// The value, or `None` before a `set` has finished.
    #[inline]
    pub fn get(&self) -> Option<&T> {
        if self.state.load(Ordering::Acquire) == READY {
            // SAFETY: `set` stores READY (Release) only after it wrote the
            // value, and this Acquire load sees that store, so the value is
            // written and its write is visible here. Nothing writes the value
            // after READY: the state never goes back to EMPTY, and `drop`
            // needs `&mut self`, so no `&T` from here is alive then.
            #[allow(unsafe_code)]
            Some(unsafe { (*self.value.get()).assume_init_ref() })
        } else {
            None
        }
    }

    /// Sets the value. `Err(value)` when the cell has a value, or when
    /// another thread is setting one.
    ///
    /// # Errors
    ///
    /// Gives back `value` when the cell is not empty.
    pub fn set(&self, value: T) -> Result<(), T> {
        // Relaxed is enough: the claim orders no data. Only the thread whose
        // compare_exchange moves the state from EMPTY touches the value
        // slot, and readers read it only after the Release store of READY.
        if self
            .state
            .compare_exchange(EMPTY, WRITING, Ordering::Relaxed, Ordering::Relaxed)
            .is_err()
        {
            return Err(value);
        }
        // SAFETY: only one compare_exchange moves the state from EMPTY, and
        // the state never goes back, so this thread has the only access to
        // the slot: no other `set` writes it, and `get` reads it only after
        // READY, which is stored below.
        #[allow(unsafe_code)]
        unsafe {
            (*self.value.get()).write(value);
        }
        self.state.store(READY, Ordering::Release);
        Ok(())
    }

    /// The value, set from `init()` when the cell is empty. When threads
    /// call it at once, more than one can run `init`; the first `set` wins,
    /// and the other values are dropped.
    pub fn get_or_init(&self, init: impl FnOnce() -> T) -> &T {
        if let Some(value) = self.get() {
            return value;
        }
        // A lost `set` drops its value here.
        let _ = self.set(init());
        loop {
            if let Some(value) = self.get() {
                return value;
            }
            // Another thread is between its claim and READY: a move of a
            // value that is already made.
            std::thread::yield_now();
        }
    }
}

impl<T> Drop for ZeroOnceLock<T> {
    fn drop(&mut self) {
        if *self.state.get_mut() == READY {
            // SAFETY: READY: the value is written. `&mut self`: no `&T` from
            // `get` is alive, and nothing uses the slot after this.
            #[allow(unsafe_code)]
            unsafe {
                self.value.get_mut().assume_init_drop();
            }
        }
    }
}

// The empty state must stay zero: a nonzero one puts the static arrays back
// in `.data` (check with `readelf -S` on a release tsc).
const _: () = assert!(EMPTY == 0);

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Barrier;
    use std::sync::atomic::AtomicUsize;

    /// `f()` once it gives a value; panics after 60 s.
    fn wait_for<T>(mut f: impl FnMut() -> Option<T>) -> T {
        let start = std::time::Instant::now();
        loop {
            if let Some(v) = f() {
                return v;
            }
            assert!(start.elapsed().as_secs() < 60, "no value after 60 s");
            std::hint::spin_loop();
        }
    }

    #[test]
    fn set_once_then_get() {
        let cell = ZeroOnceLock::new();
        assert_eq!(cell.get(), None);
        assert_eq!(cell.set(7), Ok(()));
        assert_eq!(cell.set(8), Err(8));
        assert_eq!(cell.get(), Some(&7));
        assert_eq!(*cell.get_or_init(|| 9), 7);
    }

    // First-use race: 8 threads set one empty cell at once, each with its
    // own value of 7 equal words, and 8 more read it from before the sets.
    // One set wins, and every read sees one whole value. 500 rounds.
    #[test]
    fn first_use_race_gives_one_whole_value() {
        const SETTERS: usize = 8;
        const READERS: usize = 8;
        let check = |v: &[u64; 7]| {
            assert!((1..=SETTERS as u64).contains(&v[0]), "{v:?}");
            assert!(v.iter().all(|&w| w == v[0]), "a whole value: {v:?}");
        };
        for _ in 0..500 {
            let cell = ZeroOnceLock::<[u64; 7]>::new();
            let barrier = Barrier::new(SETTERS + READERS);
            let wins = AtomicUsize::new(0);
            std::thread::scope(|s| {
                for t in 1..=SETTERS as u64 {
                    let (cell, barrier, wins) = (&cell, &barrier, &wins);
                    s.spawn(move || {
                        barrier.wait();
                        if cell.set([t; 7]).is_ok() {
                            wins.fetch_add(1, Ordering::Relaxed);
                        }
                        check(wait_for(|| cell.get()));
                    });
                }
                for _ in 0..READERS {
                    let (cell, barrier) = (&cell, &barrier);
                    s.spawn(move || {
                        barrier.wait();
                        check(wait_for(|| cell.get()));
                    });
                }
            });
            assert_eq!(wins.load(Ordering::Relaxed), 1, "one set wins");
        }
    }

    /// Counts its drops.
    struct Counted<'a>(&'a AtomicUsize, usize);

    impl Drop for Counted<'_> {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    // 16 threads ask for the same value at once (`get_or_init`, as a
    // `HIGH_BLOCKS` chunk): all get the one kept value, and every value
    // made is dropped once: the lost ones at once, the kept one with the
    // cell. 500 rounds.
    #[test]
    fn get_or_init_race_keeps_one_value_and_drops_the_rest() {
        const THREADS: usize = 16;
        for _ in 0..500 {
            let (made, dropped) = (AtomicUsize::new(0), AtomicUsize::new(0));
            let cell = ZeroOnceLock::new();
            let barrier = Barrier::new(THREADS);
            let kept: Vec<usize> = std::thread::scope(|s| {
                let handles: Vec<_> = (0..THREADS)
                    .map(|t| {
                        let (cell, barrier, made, dropped) = (&cell, &barrier, &made, &dropped);
                        s.spawn(move || {
                            barrier.wait();
                            let v = cell.get_or_init(|| {
                                made.fetch_add(1, Ordering::Relaxed);
                                Counted(dropped, t)
                            });
                            v.1
                        })
                    })
                    .collect();
                handles.into_iter().map(|h| h.join().unwrap()).collect()
            });
            assert!(kept.iter().all(|&t| t == kept[0]), "one value: {kept:?}");
            let made = made.load(Ordering::Relaxed);
            assert!(made >= 1);
            assert_eq!(dropped.load(Ordering::Relaxed), made - 1, "lost values");
            drop(cell);
            assert_eq!(dropped.load(Ordering::Relaxed), made, "the kept value");
        }
    }

    #[test]
    fn an_empty_cell_drops_nothing() {
        let dropped = AtomicUsize::new(0);
        drop(ZeroOnceLock::<Counted<'_>>::new());
        let cell = ZeroOnceLock::new();
        assert!(cell.set(Counted(&dropped, 0)).is_ok());
        let Err(lost) = cell.set(Counted(&dropped, 1)) else {
            panic!("a second set")
        };
        drop(lost);
        assert_eq!(dropped.load(Ordering::Relaxed), 1);
        drop(cell);
        assert_eq!(dropped.load(Ordering::Relaxed), 2);
    }
}
