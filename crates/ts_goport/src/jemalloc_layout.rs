//! The jemalloc layout: puts the jemalloc memory of a run where the kernel
//! can give it 2 MiB pages. Not a Go port (jemlayout1, jemlayout2).
//!
//! Two behaviors of jemalloc 5.3.1 (tikv-jemalloc-sys 0.7.1) put memory on
//! 4 KiB pages:
//! - A. Purge when an arena loses its last thread. When the last thread
//!   bound to an arena exits (tcache.c `tcache_destroy`) or moves to another
//!   arena (jemalloc.c `arena_migrate`), and there is no background thread,
//!   jemalloc purges every free page of that arena at once ("Force purging
//!   when no threads assigned to the arena anymore"). `dirty_decay_ms:-1`
//!   does not stop it. The parse workers end that way. The purge is one
//!   `MADV_DONTNEED` per free extent (4 KiB to 200 KiB). Each one splits
//!   the huge page under it, and the next use faults 4 KiB pages.
//! - B. Unaligned grows. An arena maps more memory in steps of 2, 2.5, 3,
//!   3.5, 4, 5, 6, 7, 8, 10, 12 MiB and on (exp_grow.h
//!   `exp_grow_size_prepare`, extent.c `extent_grow_retained`). Linux (6.7
//!   and later) puts an anonymous mapping at a 2 MiB boundary only when its
//!   length is a multiple of 2 MiB. So the 2.5 to 7 MiB steps land at
//!   unaligned addresses, and the blocks at their ends take 4 KiB pages.
//!
//! `jemalloc_layout` runs once in `main` (bin/tsgo.rs, bin/goport.rs):
//! - For B: in each arena, one allocation of 4 MiB with 4 MiB alignment,
//!   freed at once. It needs 8 MiB - 4 KiB in one piece, so the arena skips
//!   to its 8 MiB step. Its later steps (10, 12, 14 MiB and on) are all
//!   multiples of 2 MiB.
//! - For A: a parked thread bound to the last arena (`HOLDER_NAME`). With
//!   the THP watcher (the default CLI path), main, the watcher, the work
//!   thread and this thread each keep one of the 4 arenas (`narenas:4`)
//!   while the watcher runs. The watcher stops after 60 s
//!   (`thp_guard::WATCH_FOR`), or sooner when it turns THP off or cannot
//!   read `/proc/buddyinfo`; then its arena can lose its last thread, as
//!   without the watcher. Without the watcher (LSP, API, watch mode,
//!   `GOPORT_THP_GUARD=0` or `start`), one arena can still lose its last
//!   parse worker; jemlayout1 measured the same faults there as with the
//!   watcher. Before this, only the watcher kept the fourth arena, by
//!   chance.
//!
//! jemlayout1, query check: 5.0k to 6.2k minor faults became 1.2k to 1.6k on
//! alvin, cup2 (THP `always`) and mini-743d (THP `madvise`). Max RSS within
//! 1%. It costs up to 0.3 ms at start (three arenas made on this thread).
//! `GOPORT_JEMALLOC_LAYOUT=0` turns it off (for A/B timing). Under an address
//! space or data limit it does nothing: such a run counts its address space.
//!
//! After a jemalloc update, check both behaviors again:
//! - A: `tcache_destroy` and `arena_migrate` still purge all when
//!   `arena_nthreads_get` is 0. `strace -f -e trace=madvise` of a query
//!   check with `GOPORT_THP_GUARD=start GOPORT_LAUNCH=0` (no watcher):
//!   jemlayout1 saw 80 `MADV_DONTNEED` calls below 4 MiB without the
//!   layout and 11 with it.
//! - B: exp_grow.h still grows in these steps. `strace -f -e trace=mmap` of
//!   the same run: with the layout, every jemalloc grow has a length that is
//!   a multiple of 2 MiB and a 2 MiB aligned address (jemlayout1: 11 to 13
//!   grows were not, without it).
//! - `cargo test --test jemalloc_layout` checks the layout from inside.
//! - Minor faults (`GOPORT_LAUNCH=0 /usr/bin/time -f %R`) of a query check
//!   with `GOPORT_JEMALLOC_LAYOUT=0` and without it.
//!
//! When jemalloc no longer does A or B, remove that part. In the worst case
//! the code costs one 4 MiB allocation and free per arena and one parked
//! thread.

use tikv_jemalloc_ctl::{Access, AsName};

/// The name of the thread that keeps the last arena (A).
/// tests/jemalloc_layout.rs looks for it.
pub const HOLDER_NAME: &str = "arena-hold";

/// The free allocation that makes an arena skip to its 8 MiB step (B).
#[repr(C, align(4194304))]
struct Grow([u8; 4 << 20]);

/// Lays out the jemalloc arenas (see the module comment). Call once in
/// `main`, after `set_malloc_tunables` (it can exec the binary again) and
/// before the work threads start. It does not wait for the holder thread.
pub fn jemalloc_layout() {
    if std::env::var_os("GOPORT_JEMALLOC_LAYOUT").is_some_and(|v| v == "0")
        || crate::gostd::stack::memory_limit().is_some()
    {
        return;
    }
    let Ok(arenas) = Access::<u32>::read(b"opt.narenas\0".name()) else {
        return;
    };
    let Ok(own) = Access::<u32>::read(b"thread.arena\0".name()) else {
        return;
    };
    for arena in 0..arenas {
        if b"thread.arena\0".name().write(arena).is_ok() {
            // Fallible, so a run that is out of address space goes on.
            // `black_box` keeps the compiler from removing the pair.
            let mut grow = Vec::<Grow>::new();
            let _ = grow.try_reserve_exact(1);
            std::hint::black_box(&mut grow);
        }
    }
    let _ = b"thread.arena\0".name().write(own);
    let Some(last) = arenas.checked_sub(1) else {
        return;
    };
    // The stack holds glibc's static TLS and a park loop. A thread that
    // cannot start, or a write that fails, leaves A as it was.
    let _ = std::thread::Builder::new()
        .name(HOLDER_NAME.into())
        .stack_size(256 << 10)
        .spawn(move || {
            if b"thread.arena\0".name().write(last).is_ok() {
                loop {
                    std::thread::park();
                }
            }
        });
}
