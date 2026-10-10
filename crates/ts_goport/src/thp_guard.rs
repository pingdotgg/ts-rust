//! The THP guard: turns transparent huge pages off for this process when
//! the kernel has little free memory in 2 MiB blocks. Not a Go port.
//!
//! jemalloc runs with `thp:always` (see `bin/goport.rs`
//! `set_malloc_tunables`), so on a kernel in THP `madvise` mode it marks
//! its data `MADV_HUGEPAGE`. In `always` mode it does not (jemalloc 5.3.1
//! `pages_set_thp_state`): the kernel gives huge pages without it. On clean
//! memory huge pages are 7% to 22% faster than 4 KiB pages (perf12 THP
//! grid). When the free memory is in small blocks (for example after a file
//! walk or a build fills the dentry and inode slab), each huge page fault
//! that may compact waits in direct compaction, and a run takes 50% to 85%
//! longer (perf12: 2,600 compaction stalls in one effect emit). With THP
//! off (`prctl(PR_SET_THP_DISABLE)`), the faults take 4 KiB pages and do
//! not wait, even where jemalloc asked for huge pages. Which faults may
//! compact depends on `enabled` and `defrag` (`start_step`).
//!
//! The guard has two parts:
//! - The start check turns THP off at once when less than the limit is free
//!   in 2 MiB blocks.
//! - Else a watcher thread reads the free memory again and again and turns
//!   THP off the first time it drops below the limit. The huge pages that
//!   the run faulted before stay. The flag changes only the later faults.
//!   The wait between two reads (`next_poll`) is 5 ms near the limit and
//!   up to 50 ms far above it. A process that lives on after its first
//!   build (`long_running`: watch mode, `--lsp`, `--api`) starts no
//!   watcher. Its polls cost CPU on every edit for up to `WATCH_FOR`
//!   (dropin1 diag: 9 to 22 ms per edit window), and an LSP server builds
//!   nothing until a request comes.
//!
//! The guard also tells `bin/tsgo.rs` `launch` whether the run ends on
//! 4 KiB pages (`huge_pages`), so that it runs the work in a worker process
//! and the caller does not wait for their unmap at exit: THP is off at the
//! start, or `never`, or kept below the limit where no fault waits. A run
//! whose watcher fires later also ends on 4 KiB pages, but it gets no
//! worker: the start check cannot tell a small run from a large one, and
//! for a small run a worker costs more than it saves (`huge_pages`).
//!
//! The limit is low (`DEFAULT_MIN_FREE_MIB`) because a run that finds
//! enough free 2 MiB blocks is faster with THP. perf13 on dbook, with only
//! a start check at 1 GiB, fragmented memory: query and hono started with
//! a median of 223 to 284 MiB free, and THP off made them 6% to 16%
//! slower. zod started with 272 MiB but faults about 900 MiB, and THP kept
//! made it 39% slower: the watcher is for that case. With 10 to 148 MiB
//! free at start, THP kept made effect 41% to 49% slower and query check 8
//! 3% to 12% slower.
//!
//! perf13b, a watcher at 128 MiB on fragmented memory: hono emit faulted
//! about 66 huge pages before it fired, where THP kept got all 144 with a
//! few compaction stalls, so hono was 2.2% slower than with THP kept. The
//! watcher fires later than the limit (see `POLL_MIN`); 64 MiB keeps enough
//! margin for that. In two more grids (more fragmented: most runs started
//! below 64 MiB) 64 MiB was within noise of 128 MiB or faster in every
//! cell, and tsgo -b query chain got about 90 to 130 huge pages, not 56 to
//! 99, with no compaction stall.
//!
//! `GOPORT_THP_GUARD` selects the mode: `0` does nothing, `force` always
//! turns THP off, `start` does the start check but starts no watcher, and
//! any other value (or no value) does both. `GOPORT_THP_GUARD_MIB` changes
//! the limit of both (a value that does not parse keeps the default).
//! `GOPORT_THP_GUARD_DEBUG=1` prints each decision and its inputs as one
//! line on stderr.

use std::time::Duration;

/// The guard turns THP off when less than this many MiB are free in
/// blocks of 2 MiB or more. See the module comment for the perf13 and
/// perf13b data.
const DEFAULT_MIN_FREE_MIB: u64 = 64;

/// The shortest wait between two reads of `/proc/buddyinfo`. A run faults
/// huge pages fastest at its start: in perf13b, with reads every 5 ms, the
/// free memory fell up to 36 MiB between two reads (effect emit and query
/// check), so the watcher must fire that far above zero to beat the first
/// compaction stall.
const POLL_MIN: Duration = Duration::from_millis(5);

/// The longest wait between two reads. One wake and read costs 55 to 190 us
/// of CPU (thpguard1, schedstat of the thread: cup2 about 65, alvin 130 to
/// 190; a longer sleep costs more per wake), so 50 ms keeps the guard under
/// 0.4% of one core. With reads every 5 ms it took 1.2% to 2.7% of the CPU
/// of a single-threaded check (cliperf1 on cup2, thpguard1 on alvin).
const POLL_MAX: Duration = Duration::from_millis(50);

/// The fastest fall of the free memory that `next_poll` plans for, in
/// bytes per ms. thpguard1 traced the fall with no guard (1 ms reads, zbook
/// with other agents building, and cup2): at most 122 MiB in 5 ms, 168 MiB
/// in 10 ms and 268 MiB in 50 ms (perf13b: 36 MiB in 5 ms). So a wait of
/// 10 ms covers 2x the largest 10 ms fall, and a wait of 50 ms 6x the
/// largest 50 ms fall. Less than 192 MiB above the limit the wait is 5 ms,
/// as it was before `next_poll`.
const MAX_FALL_PER_MS: u64 = 32 << 20;

/// The watcher stops after this time, so a long run does not read the
/// memory for its whole life. The longest run of the perf13 grid took 1.2 s.
const WATCH_FOR: Duration = Duration::from_secs(60);

/// The buffer for `/proc/buddyinfo`: about 110 bytes per zone, so room for
/// 70 nodes of 4 zones.
const BUDDYINFO_BYTES: usize = 32 << 10;

/// The smallest `/proc/buddyinfo` order that holds a whole 2 MiB huge page
/// (512 pages of 4 KiB). The guard runs only on x86-64, where both sizes
/// are fixed.
const HUGE_ORDER: usize = 9;

/// Bytes in one page of the buddy allocator.
const PAGE_BYTES: u64 = 4096;

/// Call first in `main`, before the first heap allocation: jemalloc maps
/// and touches its first memory at that allocation. The start check
/// allocates nothing (stack buffers only). Setting a `GOPORT_THP_GUARD*`
/// variable allocates its value first. The `long_running` check and the
/// watcher thread start allocate, after the start check.
///
/// Checks, in order, and stops at the first that says to keep THP:
/// 1. `GOPORT_THP_GUARD` (see the module comment).
/// 2. `PR_GET_THP_DISABLE`: THP is already off (the parent turned it off,
///    or this run is the exec of `set_malloc_tunables`: the flag stays set
///    across `execve`).
/// 3. `enabled` and `defrag` in `/sys/kernel/mm/transparent_hugepage`
///    (`start_step`).
/// 4. `/proc/buddyinfo`: the free memory in blocks of 2 MiB or more, in
///    all nodes and zones, against the limit. Below it, THP goes off. Else
///    the watcher starts (`watch`), unless the process is `long_running`.
///
/// A file that cannot be read keeps THP on and starts no watcher. The flag
/// stays set for the whole process and its children.
///
/// Returns false when the run will get 4 KiB pages (`huge_pages`): THP is
/// off (the start check turned it off, or it was off already), THP is
/// `never`, or the check kept THP on (its faults cannot wait for
/// compaction) with less than the limit free in 2 MiB blocks. `bin/tsgo.rs`
/// then runs the work in a worker process (`launch`). True otherwise, also
/// when a file cannot be read.
pub fn thp_guard() -> bool {
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        use nix::sys::prctl::{get_thp_disable, set_thp_disable};
        let debug = std::env::var_os("GOPORT_THP_GUARD_DEBUG").is_some_and(|v| v != "0");
        let watcher = match std::env::var_os("GOPORT_THP_GUARD") {
            Some(mode) if mode == "0" => {
                say(debug, format_args!("THP kept (GOPORT_THP_GUARD=0)"));
                return true;
            }
            Some(mode) if mode == "force" => {
                let ok = set_thp_disable(true).is_ok();
                say(
                    debug,
                    format_args!("THP off (GOPORT_THP_GUARD=force, prctl ok {ok})"),
                );
                return !ok;
            }
            Some(mode) => mode != "start",
            None => true,
        };
        let min_free_mib: u64 = std::env::var_os("GOPORT_THP_GUARD_MIB")
            .and_then(|mib| mib.to_str()?.parse().ok())
            .unwrap_or(DEFAULT_MIN_FREE_MIB);
        if get_thp_disable().unwrap_or(true) {
            say(debug, format_args!("THP already off at start"));
            return false;
        }
        let mut enabled = [0; 128];
        let mut defrag = [0; 128];
        let mut buddyinfo = [0; BUDDYINFO_BYTES];
        let (Some(enabled), Some(defrag), Some(buddyinfo)) = (
            read_small("/sys/kernel/mm/transparent_hugepage/enabled", &mut enabled),
            read_small("/sys/kernel/mm/transparent_hugepage/defrag", &mut defrag),
            read_small("/proc/buddyinfo", &mut buddyinfo),
        ) else {
            say(
                debug,
                format_args!("THP kept (a THP file or /proc/buddyinfo cannot be read)"),
            );
            return true;
        };
        let Some(free) = free_huge_bytes(buddyinfo) else {
            say(
                debug,
                format_args!("THP kept (/proc/buddyinfo does not parse)"),
            );
            return true;
        };
        let min_free = min_free_mib.saturating_mul(1 << 20);
        let step = start_step(enabled, defrag, free, min_free);
        let failed = step == Start::Off && set_thp_disable(true).is_err();
        let no_watcher = step == Start::Watch && watcher && long_running();
        let watching =
            step == Start::Watch && watcher && !no_watcher && start_watcher(min_free, free, debug);
        let huge = failed || huge_pages(step, enabled, free, min_free);
        say(
            debug,
            format_args!(
                "THP {} (enabled {}, defrag {}, {} MiB free in 2 MiB blocks, limit {min_free_mib} MiB{}{}{}{})",
                if step == Start::Off { "off" } else { "kept" },
                selected_mode(enabled).unwrap_or("?"),
                selected_mode(defrag).unwrap_or("?"),
                free >> 20,
                if failed { ", prctl failed" } else { "" },
                if watching { ", watcher started" } else { "" },
                if no_watcher {
                    ", no watcher: watch, LSP or API mode"
                } else {
                    ""
                },
                if huge { "" } else { ", 4 KiB pages" },
            ),
        );
        huge
    }
    #[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
    true
}

/// Whether this process lives on after its first build, so `thp_guard`
/// starts no watcher and `bin/tsgo.rs` `launch` starts no worker: `--lsp` or
/// `--api` as the first argument (Go cmd/tsc `runMain`), or a watch option
/// anywhere (`--watch` or `-w`; Go `getInputOptionName`: one or two leading
/// '-', any case). It reads the process arguments, so it allocates.
pub fn long_running() -> bool {
    let mut args = std::env::args_os().skip(1);
    let watch = |a: &std::ffi::OsString| {
        a.to_str()
            .and_then(|a| a.strip_prefix('-'))
            .map(|a| a.strip_prefix('-').unwrap_or(a))
            .is_some_and(|a| a.eq_ignore_ascii_case("watch") || a.eq_ignore_ascii_case("w"))
    };
    match args.next() {
        None => false,
        Some(first) => {
            first == "--lsp" || first == "--api" || watch(&first) || args.any(|a| watch(&a))
        }
    }
}

/// Prints `args` as one `goport thp_guard:` line on stderr when `debug`.
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn say(debug: bool, args: std::fmt::Arguments<'_>) {
    if debug {
        eprintln!("goport thp_guard: {args}");
    }
}

/// Starts the `watch` thread. `free` is the start check's read. False when
/// the thread cannot start. The buddyinfo buffer is made here, on the heap:
/// glibc puts the static TLS (79 to 99 KiB in perf13b) at the top of each
/// thread stack, so a small stack (a 128 KiB one, or `RUST_MIN_STACK`) had
/// too little room for it.
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn start_watcher(min_free: u64, free: u64, debug: bool) -> bool {
    let buddyinfo = vec![0; BUDDYINFO_BYTES];
    std::thread::Builder::new()
        .name("thp-guard".into())
        .spawn(move || watch(min_free, next_poll(free, min_free), debug, buddyinfo))
        .is_ok()
}

/// The watcher thread: waits `wait`, reads `/proc/buddyinfo` into
/// `buddyinfo` and follows `watch_step`, again and again. It turns THP off
/// at most once, then ends. It allocates nothing.
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn watch(min_free: u64, mut wait: Duration, debug: bool, mut buddyinfo: Vec<u8>) {
    let start = std::time::Instant::now();
    let mut reads = 0u32;
    loop {
        std::thread::sleep(wait);
        let free = read_small("/proc/buddyinfo", &mut buddyinfo).and_then(free_huge_bytes);
        let elapsed = start.elapsed();
        reads += 1;
        match (watch_step(elapsed, free, min_free), free) {
            (Watch::Wait(next), _) => wait = next,
            (Watch::Off, free) => {
                let ok = nix::sys::prctl::set_thp_disable(true).is_ok();
                return say(
                    debug,
                    format_args!(
                        "THP off by the watcher after {} ms and {reads} reads ({} MiB free in 2 MiB blocks, prctl ok {ok})",
                        elapsed.as_millis(),
                        free.unwrap_or(0) >> 20,
                    ),
                );
            }
            (Watch::Stop, None) => {
                return say(
                    debug,
                    format_args!(
                        "watcher stopped, THP kept (/proc/buddyinfo cannot be read or does not parse)"
                    ),
                );
            }
            (Watch::Stop, Some(free)) => {
                return say(
                    debug,
                    format_args!(
                        "watcher stopped after {} s and {reads} reads, THP kept ({} MiB free in 2 MiB blocks)",
                        elapsed.as_secs(),
                        free >> 20,
                    ),
                );
            }
        }
    }
}

/// What the start check does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Start {
    /// Keep THP and start no watcher: a fault never waits for compaction,
    /// or a THP text does not parse.
    Keep,
    /// Turn THP off now.
    Off,
    /// Keep THP and start the watcher.
    Watch,
}

/// The start decision. A huge page fault of `MADV_HUGEPAGE` memory can
/// wait in direct compaction when `enabled` is `always` or `madvise` and
/// `defrag` is `always`, `madvise` or `defer+madvise` (the texts of the
/// sysfs files, with the mode in brackets). With `defer` or `never`, it
/// takes a 4 KiB page when no 2 MiB block is free, so THP can stay on. When
/// a fault can wait: `Off` when `free` (the bytes free in blocks of 2 MiB
/// or more) is less than `min_free`, else `Watch`.
///
/// In `always` mode jemalloc marks nothing, so with `defrag` `madvise` or
/// `defer+madvise` no fault of ours waits either (thpfault1: 0 compaction
/// stalls in 250 runs on cup2). `Keep` there was slower all the same
/// (thpguard2, release builds, 2 builds a side, 2 runs): cup2 wall +2.8%
/// (geomean of 7 inputs as found and fragmented, query core +6%), and on
/// zbook query core, hono and zod took 5% to 25% more page faults. The
/// cause is the start of the watcher, not its reads: with the watcher's
/// first allocations (the argument list, its 32 KiB buffer, the thread) and
/// a thread that only sleeps, the faults were within 5% of the watcher's.
/// So `always` mode counts as `madvise`.
fn start_step(enabled: &str, defrag: &str, free: u64, min_free: u64) -> Start {
    let can_wait = matches!(selected_mode(enabled), Some("always" | "madvise"))
        && matches!(
            selected_mode(defrag),
            Some("always" | "madvise" | "defer+madvise")
        );
    match (can_wait, free < min_free) {
        (false, _) => Start::Keep,
        (true, true) => Start::Off,
        (true, false) => Start::Watch,
    }
}

/// Whether a run with the start decision `step` ends with its memory on
/// huge pages, so `bin/tsgo.rs` `launch` needs no worker: THP is on (not
/// `Off`, not `never`) and `free` (the bytes free in 2 MiB blocks at the
/// start) is at least `min_free`. Below it only `Keep` gets here, and its
/// faults soon fall back to 4 KiB pages.
///
/// A `Watch` run gets no worker, also when its watcher will fire soon.
/// thpguard2 on mini timed a worker against none on the same binary. A
/// worker is a second process start: 2.0 to 2.4 ms per run (tsgo --version
/// 3.3 against 5.3 ms). Query core check (32 ms, 58 MiB of huge pages) paid
/// for it in every memory state: +0.1 to +4.8 ms (up to +13%) with its
/// watcher set to fire after 8 to 32 MiB of huge pages, +5% to +12% at 0.4
/// to 1.8 GiB free. T3 Code server was 4% to 13% faster where its watcher
/// fired, hono and T3 Code shared 5% to 9% with the watcher set to fire
/// after 8 to 32 MiB, and zod 8% to 9% after 32 to 64 MiB. A worker from a
/// free-memory limit above `min_free` makes small runs pay, so there is
/// none.
fn huge_pages(step: Start, enabled: &str, free: u64, min_free: u64) -> bool {
    step != Start::Off && selected_mode(enabled) != Some("never") && free >= min_free
}

/// What the watcher does after one read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Watch {
    /// Read again after this wait (`next_poll`).
    Wait(Duration),
    /// Turn THP off and stop.
    Off,
    /// Stop and keep THP.
    Stop,
}

/// The watcher decision after `elapsed` since it started. `free` is the
/// bytes free in blocks of 2 MiB or more, None when `/proc/buddyinfo`
/// cannot be read or does not parse. Low memory wins over the time limit.
fn watch_step(elapsed: Duration, free: Option<u64>, min_free: u64) -> Watch {
    match free {
        None => Watch::Stop,
        Some(free) if free < min_free => Watch::Off,
        Some(_) if elapsed >= WATCH_FOR => Watch::Stop,
        Some(free) => Watch::Wait(next_poll(free, min_free)),
    }
}

/// The wait before the next read when `free` bytes are free in 2 MiB
/// blocks: the time the free memory takes to fall to `min_free` at
/// `MAX_FALL_PER_MS`, from `POLL_MIN` to `POLL_MAX`. With 160 MiB or more
/// above `min_free` (5 ms of that fall), the watcher reads again before
/// such a fall can reach the limit. Nearer the limit it reads every 5 ms,
/// as often as it did with a fixed 5 ms (cliperf1 rank 9: the fixed 5 ms
/// read cost 1.2% to 1.5% of the CPU of a single-threaded run on cup2),
/// and a fall at `MAX_FALL_PER_MS` can pass the limit before the next
/// read. Less than 192 MiB above `min_free`: 5 ms (at the default limit,
/// less than 256 MiB free; zbook in cliperf1 gapA had 166 MiB). 1,600 MiB
/// or more above it: 50 ms (at the default limit, 1,664 MiB free).
fn next_poll(free: u64, min_free: u64) -> Duration {
    let ms = free.saturating_sub(min_free) / MAX_FALL_PER_MS;
    Duration::from_millis(ms).clamp(POLL_MIN, POLL_MAX)
}

/// The text of the file at `path`, read into `buf` without a heap
/// allocation. None when the file cannot be read, is not UTF-8, or does not
/// fit in `buf`.
fn read_small<'a>(path: &str, buf: &'a mut [u8]) -> Option<&'a str> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).ok()?;
    let mut len = 0;
    loop {
        match file.read(&mut buf[len..]).ok()? {
            0 => break,
            n => len += n,
        }
        if len == buf.len() {
            return None;
        }
    }
    std::str::from_utf8(&buf[..len]).ok()
}

/// The mode in brackets in a THP sysfs file: `madvise` in
/// `always [madvise] never`.
fn selected_mode(text: &str) -> Option<&str> {
    let (_, rest) = text.split_once('[')?;
    rest.split_once(']').map(|(mode, _)| mode)
}

/// The free bytes in blocks of order `HUGE_ORDER` or more in
/// `/proc/buddyinfo`, all lines summed. Each line is
/// `Node 0, zone   Normal  <free blocks of order 0> <order 1> ...`. None
/// when no line parses.
fn free_huge_bytes(buddyinfo: &str) -> Option<u64> {
    let mut total = None;
    for line in buddyinfo.lines() {
        let mut words = line.split_whitespace();
        if words.next() != Some("Node") || words.nth(1) != Some("zone") || words.next().is_none() {
            continue;
        }
        let mut free = 0u64;
        for (order, count) in words.enumerate() {
            let count: u64 = count.parse().ok()?;
            if order >= HUGE_ORDER {
                free = free.saturating_add(count.saturating_mul(PAGE_BYTES << order));
            }
        }
        total = Some(total.unwrap_or(0u64).saturating_add(free));
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIB: u64 = 1 << 20;

    /// zbook: 1 + 29 + 477 blocks of 2 MiB and 2 + 1 + 10 of 4 MiB.
    const BUDDYINFO: &str = "\
Node 0, zone      DMA      0      1      0      0      1      1      1      1      0      1      2
Node 0, zone    DMA32    335   1658   2953   3171   2345   1679   1193    816    311     29      1
Node 0, zone   Normal 616681 607866 455781 337376 229283 129647  50350  16747   2527    477     10
";

    #[test]
    fn free_huge_bytes_sums_orders_9_and_up() {
        assert_eq!(
            free_huge_bytes(BUDDYINFO),
            Some((1 + 29 + 477) * 2 * MIB + 13 * 4 * MIB)
        );
        // Two nodes; order 11 counts 8 MiB.
        let two_nodes = "Node 0, zone Normal 5 0 0 0 0 0 0 0 0 1 0 0\n\
                         Node 1, zone Normal 0 0 0 0 0 0 0 0 0 0 0 1\n";
        assert_eq!(free_huge_bytes(two_nodes), Some(2 * MIB + 8 * MIB));
        assert_eq!(free_huge_bytes(""), None);
        assert_eq!(free_huge_bytes("Node 0, zone Normal 1 x 3\n"), None);
    }

    #[test]
    fn selected_mode_reads_the_bracketed_word() {
        assert_eq!(selected_mode("[always] madvise never\n"), Some("always"));
        assert_eq!(
            selected_mode("always defer [defer+madvise] madvise never\n"),
            Some("defer+madvise")
        );
        assert_eq!(selected_mode("always madvise never\n"), None);
    }

    #[test]
    fn start_step_turns_thp_off_only_below_the_limit_and_else_watches() {
        let enabled = "always [madvise] never\n";
        let defrag = "always defer defer+madvise [madvise] never\n";
        let limit = DEFAULT_MIN_FREE_MIB * MIB;
        assert_eq!(start_step(enabled, defrag, limit - 1, limit), Start::Off);
        assert_eq!(start_step(enabled, defrag, 0, limit), Start::Off);
        assert_eq!(start_step(enabled, defrag, limit, limit), Start::Watch);
        // perf13 fragd1: query started with 290 to 314 MiB free.
        assert_eq!(start_step(enabled, defrag, 300 * MIB, limit), Start::Watch);
        // A text that does not parse keeps THP on.
        assert_eq!(start_step("madvise", defrag, 0, limit), Start::Keep);
    }

    /// Every `enabled` and `defrag` mode: `Off` below the limit and else
    /// `Watch` with `always` or `madvise` and `always`, `madvise` or
    /// `defer+madvise`. Else no watcher and THP stays on. `always` with
    /// `madvise` (cup2) or `defer+madvise` (zbook) counts as `madvise`
    /// (thpguard2: no watcher there was slower).
    #[test]
    fn start_step_watches_where_a_fault_can_wait() {
        let limit = DEFAULT_MIN_FREE_MIB * MIB;
        let enabled_texts = [
            ("always", "[always] madvise never\n"),
            ("madvise", "always [madvise] never\n"),
            ("never", "always madvise [never]\n"),
        ];
        let defrag_texts = [
            ("always", "[always] defer defer+madvise madvise never\n"),
            ("defer", "always [defer] defer+madvise madvise never\n"),
            (
                "defer+madvise",
                "always defer [defer+madvise] madvise never\n",
            ),
            ("madvise", "always defer defer+madvise [madvise] never\n"),
            ("never", "always defer defer+madvise madvise [never]\n"),
        ];
        let can_wait = [
            ("madvise", "always"),
            ("madvise", "defer+madvise"),
            ("madvise", "madvise"),
            ("always", "always"),
            ("always", "defer+madvise"),
            ("always", "madvise"),
        ];
        for (e, enabled) in enabled_texts {
            for (d, defrag) in defrag_texts {
                let (low, high) = if can_wait.contains(&(e, d)) {
                    (Start::Off, Start::Watch)
                } else {
                    (Start::Keep, Start::Keep)
                };
                assert_eq!(
                    start_step(enabled, defrag, limit - 1, limit),
                    low,
                    "{e} {d}"
                );
                assert_eq!(start_step(enabled, defrag, limit, limit), high, "{e} {d}");
            }
        }
    }

    /// The worker rule: a worker (false) when THP is off or `never`, or
    /// kept below the limit (`Keep`, where no fault waits). No worker while
    /// the watcher runs, also just above the limit (thpguard2: small runs
    /// pay for it there).
    #[test]
    fn huge_pages_is_false_only_where_the_run_starts_on_4_kib_pages() {
        let limit = DEFAULT_MIN_FREE_MIB * MIB;
        let madvise = "always [madvise] never\n";
        let always = "[always] madvise never\n";
        let never = "always madvise [never]\n";
        for free in [0, limit, u64::MAX] {
            assert!(!huge_pages(Start::Off, madvise, free, limit));
            assert!(!huge_pages(Start::Keep, never, free, limit));
        }
        for free in [limit, limit + 10 * MIB, 1018 * MIB, u64::MAX] {
            assert!(huge_pages(Start::Watch, madvise, free, limit));
            assert!(huge_pages(Start::Keep, always, free, limit));
        }
        // Kept below the limit (`defrag` `defer` or `never`): a worker.
        assert!(!huge_pages(Start::Keep, always, 20 * MIB, limit));
        assert!(!huge_pages(Start::Keep, always, limit - 1, limit));
        // A text that does not parse keeps THP on.
        assert!(huge_pages(Start::Keep, "madvise", u64::MAX, limit));
    }

    #[test]
    fn watch_step_turns_thp_off_below_the_limit_until_the_time_limit() {
        let limit = DEFAULT_MIN_FREE_MIB * MIB;
        let early = POLL_MIN;
        assert_eq!(watch_step(early, Some(limit), limit), Watch::Wait(POLL_MIN));
        assert_eq!(
            watch_step(early, Some(10_000 * MIB), limit),
            Watch::Wait(POLL_MAX)
        );
        assert_eq!(watch_step(early, Some(limit - 1), limit), Watch::Off);
        // A file that cannot be read stops the watcher.
        assert_eq!(watch_step(early, None, limit), Watch::Stop);
        // After the time limit it stops, but low memory still turns THP off.
        assert_eq!(watch_step(WATCH_FOR, Some(limit), limit), Watch::Stop);
        assert_eq!(watch_step(WATCH_FOR, Some(0), limit), Watch::Off);
        assert_eq!(
            watch_step(WATCH_FOR - Duration::from_millis(1), Some(limit), limit),
            Watch::Wait(POLL_MIN)
        );
    }

    #[test]
    fn next_poll_waits_until_the_memory_can_reach_the_limit() {
        let limit = DEFAULT_MIN_FREE_MIB * MIB;
        let ms = Duration::from_millis;
        // At or below the limit (the start check and `watch_step` turn THP
        // off there first), and up to 5 ms of fall above it: 5 ms.
        assert_eq!(next_poll(0, limit), POLL_MIN);
        assert_eq!(next_poll(limit, limit), POLL_MIN);
        assert_eq!(next_poll(limit + 191 * MIB, limit), POLL_MIN);
        assert_eq!(next_poll(limit + 192 * MIB, limit), ms(6));
        // zbook (cliperf1 gapA): 166 MiB free.
        assert_eq!(next_poll(166 * MIB, limit), POLL_MIN);
        assert_eq!(next_poll(limit + 320 * MIB, limit), ms(10));
        assert_eq!(next_poll(limit + 960 * MIB, limit), ms(30));
        // 1,600 MiB above the limit and more: 50 ms.
        assert_eq!(next_poll(limit + 1599 * MIB, limit), ms(49));
        assert_eq!(next_poll(limit + 1600 * MIB, limit), POLL_MAX);
        assert_eq!(next_poll(u64::MAX, limit), POLL_MAX);
        // With as much free above the limit as the largest fall that
        // thpguard1 saw in 5, 10 and 50 ms, the next read comes within that
        // time.
        for (window, fall) in [(5, 122), (10, 168), (50, 268)] {
            assert!(next_poll(limit + fall * MIB, limit) <= ms(window));
        }
        // The planned fall of one wait stays above the limit exactly when
        // 160 MiB or more are above it. Nearer, the 5 ms wait can pass it.
        for free in (limit..limit + 4096 * MIB).step_by(MIB as usize) {
            let wait = next_poll(free, limit);
            let fall = MAX_FALL_PER_MS * wait.as_millis() as u64;
            let above = free - limit;
            assert_eq!(fall <= above, above >= 160 * MIB, "{free}");
            assert!(wait == POLL_MIN || fall <= above, "{free}");
        }
        // The bounds are above the limit, so another limit
        // (`GOPORT_THP_GUARD_MIB=128`) moves them: 1,664 MiB free is 48 ms.
        let other = 128 * MIB;
        assert_eq!(next_poll(other + 191 * MIB, other), POLL_MIN);
        assert_eq!(next_poll(256 * MIB, other), POLL_MIN);
        assert_eq!(next_poll(1664 * MIB, other), ms(48));
        assert_eq!(next_poll(other + 1600 * MIB, other), POLL_MAX);
    }
}
