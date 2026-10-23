//! loadstamp1 probe (branch `goport-loadstamp1`, never merged): timing
//! stamps of the serial loader tail and of the window up to the bind.
//!
//! With `GOPORT_LOADSTAMP` set (any value), the loading thread of a load
//! with parse workers keeps one event per span (begin, end, kind) from the
//! first `start` to the end of `run_queue`, the parse workers note the end
//! of each job (`worker_done`), and one-shot marks note the phases after
//! the load (`mark`). The marks and the span summary are printed on stderr
//! after the first `bind_all` (`print_marks`), so the summary adds no time
//! to the window. Lines start with `loadstamp `. Without the variable a
//! stamp is one relaxed load. No stamp changes what the program does.

use std::cell::{Cell, RefCell};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::time::Instant;

// Loader spans.
pub const LOAD: u8 = 0;
pub const START: u8 = 1;
pub const RUNQ: u8 = 2;
pub const META: u8 = 3;
pub const C4_META_JOB: u8 = 4;
pub const PSF: u8 = 5;
pub const TAKE: u8 = 6;
pub const C4_TAKE_JOB: u8 = 7;
pub const ADOPT: u8 = 8;
pub const WAIT: u8 = 9;
pub const C2_PATH_REFS: u8 = 10;
pub const C1_TYPE_REFS: u8 = 11;
pub const C1_RESOLVER: u8 = 12;
pub const LIB_REFS: u8 = 13;
pub const RES: u8 = 14;
pub const RES_COMMIT: u8 = 15;
pub const RES_AHEAD: u8 = 16;
pub const RES_LOOP: u8 = 17;
pub const C3_AUG: u8 = 18;
pub const C5_FREE: u8 = 19;
pub const C6_GET: u8 = 20;
pub const C6_INSERT: u8 = 21;
pub const QUEUE_BATCH: u8 = 22;
pub const META_OWN: u8 = 23;
pub const RES_NOTE: u8 = 24;
pub const RES_ADDS: u8 = 25;
pub const C6_HASH: u8 = 26;
const KINDS: usize = 27;
const KIND_NAMES: [&str; KINDS] = [
    "load",
    "start",
    "run_queued",
    "meta",
    "c4_meta_job",
    "parse_source_file",
    "take",
    "c4_take_job",
    "adopt",
    "wait",
    "c2_path_refs",
    "c1_type_refs",
    "c1_resolver",
    "lib_refs",
    "res",
    "res_commit",
    "res_ahead",
    "res_loop",
    "c3_aug",
    "c5_free",
    "c6_get",
    "c6_insert",
    "queue_batch",
    "meta_own",
    "res_note_logs",
    "res_adds",
    "c6_hash",
];

// Counts.
pub const N_TYPE_REFS: usize = 0;
pub const N_PATH_REFS: usize = 1;
pub const N_AUG: usize = 2;
pub const N_PREP: usize = 3;
pub const N_COMMIT: usize = 4;
const COUNTS: usize = 5;
const COUNT_NAMES: [&str; COUNTS] = [
    "type_ref_directives",
    "path_refs",
    "aug_names",
    "preps_taken",
    "commits_taken",
];

// One-shot marks (the first program of the process).
pub const M_PAPF_B: usize = 0;
pub const M_PARSE_B: usize = 1;
pub const M_RUNQ_E: usize = 2;
pub const M_POOL_E: usize = 3;
pub const M_COLLECT_B: usize = 4;
pub const M_COLLECT_E: usize = 5;
pub const M_PAPF_E: usize = 6;
pub const M_POOL_INIT_E: usize = 7;
pub const M_VERIFY_E: usize = 8;
pub const M_BUILD_B: usize = 9;
pub const M_PUB_B: usize = 10;
pub const M_GOFILES_E: usize = 11;
pub const M_PUB_E: usize = 12;
pub const M_PUB_CLOSURE_E: usize = 13;
pub const M_PARTS_E: usize = 14;
pub const M_SET_PROG: usize = 15;
pub const M_TABLES_E: usize = 16;
pub const M_BUILD_E: usize = 17;
pub const M_GDAP_B: usize = 18;
pub const M_SYN_E: usize = 19;
pub const M_PROGD_E: usize = 20;
pub const M_BIND_B: usize = 21;
pub const M_BIND_PAR_E: usize = 22;
pub const M_BIND_E: usize = 23;
pub const M_LAST_WORKER: usize = 24;
pub const M_STAMP_B: usize = 25;
pub const M_STAMP_E: usize = 26;
const MARKS: usize = 27;
const MARK_NAMES: [&str; MARKS] = [
    "papf_b",
    "parse_b",
    "runq_e",
    "pool_e",
    "collect_b",
    "collect_e",
    "papf_e",
    "checker_pool_e",
    "verify_e",
    "build_b",
    "publish_b",
    "go_files_e",
    "publish_e",
    "publish_closure_e",
    "table_parts_e",
    "set_prog",
    "tables_e",
    "build_e",
    "gdap_b",
    "syntactic_e",
    "program_diags_e",
    "bind_b",
    "bind_parallel_e",
    "bind_e",
    "last_worker_job_e",
    "stamp_b",
    "stamp_e",
];

static ON: AtomicU8 = AtomicU8::new(2);
static BASE: OnceLock<Instant> = OnceLock::new();
static LAST_WORKER: AtomicU64 = AtomicU64::new(0);
static MARK_AT: [AtomicU64; MARKS] = [const { AtomicU64::new(0) }; MARKS];
static PRINTED: AtomicU8 = AtomicU8::new(0);

thread_local! {
    static ACTIVE: Cell<bool> = const { Cell::new(false) };
    static EVENTS: RefCell<Vec<(u64, u64, u8)>> = const { RefCell::new(Vec::new()) };
    static COUNT: RefCell<[u64; COUNTS]> = const { RefCell::new([0; COUNTS]) };
}

/// True when `GOPORT_LOADSTAMP` is set (read once).
#[inline]
pub fn on() -> bool {
    match ON.load(Ordering::Relaxed) {
        0 => false,
        1 => true,
        _ => init(),
    }
}

#[cold]
fn init() -> bool {
    let value = std::env::var_os("GOPORT_LOADSTAMP").is_some();
    if value {
        BASE.get_or_init(Instant::now);
    }
    ON.store(u8::from(value), Ordering::Relaxed);
    value
}

/// Nanoseconds since the first stamp of the process.
#[inline]
fn now() -> u64 {
    BASE.get_or_init(Instant::now).elapsed().as_nanos() as u64
}

/// A loader span: records (begin, end, kind) when it drops.
pub struct Span {
    kind: u8,
    begin: u64,
}

impl Drop for Span {
    #[inline]
    fn drop(&mut self) {
        let end = now();
        EVENTS.with(|events| events.borrow_mut().push((self.begin, end, self.kind)));
    }
}

/// Starts span `kind` on the loading thread of a stamped load.
#[inline]
pub fn span(kind: u8) -> Option<Span> {
    if on() && ACTIVE.with(Cell::get) {
        Some(Span { kind, begin: now() })
    } else {
        None
    }
}

/// Adds `n` to count `index` on the loading thread of a stamped load.
#[inline]
pub fn count(index: usize, n: u64) {
    if on() && ACTIVE.with(Cell::get) {
        COUNT.with(|count| count.borrow_mut()[index] += n);
    }
}

/// Notes mark `index` once per process.
#[inline]
pub fn mark(index: usize) {
    if on() {
        let _ = MARK_AT[index].compare_exchange(0, now(), Ordering::Relaxed, Ordering::Relaxed);
    }
}

/// A parse worker ends a job (or looks for its first one).
#[inline]
pub fn worker_done() {
    if on() {
        LAST_WORKER.fetch_max(now(), Ordering::Relaxed);
    }
}

/// Starts the stamped part of a load with parse workers on this thread.
pub fn load_begin() {
    if !on() || PRINTED.load(Ordering::Relaxed) != 0 {
        return;
    }
    mark(M_PARSE_B);
    LAST_WORKER.store(0, Ordering::Relaxed);
    EVENTS.with(|events| {
        let mut events = events.borrow_mut();
        events.clear();
        events.reserve(1 << 20);
    });
    ACTIVE.with(|active| active.set(true));
}

/// The cost of one empty span: (begin to end inside it, whole cost) in ns.
fn calibrate() -> (f64, f64) {
    let rounds = 20_000;
    let mut scratch: Vec<(u64, u64, u8)> = Vec::with_capacity(rounds);
    let start = now();
    for _ in 0..rounds {
        let begin = now();
        let end = now();
        scratch.push((begin, end, 0));
    }
    let whole = (now() - start) as f64 / rounds as f64;
    let mut inner: Vec<u64> = scratch.iter().map(|(b, e, _)| e - b).collect();
    inner.sort_unstable();
    (inner[rounds / 2] as f64, whole)
}

/// What `load_end` keeps for the summary that `print_marks` prints.
struct Stash {
    events: Vec<(u64, u64, u8)>,
    counts: [u64; COUNTS],
    load_begin: u64,
    runq_end: u64,
    last_worker: u64,
    hash_ns: [f64; 2],
    keys: usize,
}

static STASH: std::sync::Mutex<Option<Stash>> = std::sync::Mutex::new(None);

/// Ends the stamped part of the load (after `run_queue`). The summary is
/// made after the bind (`print_marks`), so it adds no time to the window.
pub fn load_end() {
    if !on() || !ACTIVE.with(Cell::get) {
        return;
    }
    let runq_end = now();
    let _ = MARK_AT[M_RUNQ_E].compare_exchange(0, runq_end, Ordering::Relaxed, Ordering::Relaxed);
    ACTIVE.with(|active| active.set(false));
    let last_worker = LAST_WORKER.load(Ordering::Relaxed);
    let _ = MARK_AT[M_LAST_WORKER].compare_exchange(
        0,
        last_worker,
        Ordering::Relaxed,
        Ordering::Relaxed,
    );
    let events = EVENTS.with(|events| std::mem::take(&mut *events.borrow_mut()));
    let counts = COUNT.with(|count| std::mem::take(&mut *count.borrow_mut()));
    *STASH.lock().unwrap() = Some(Stash {
        events,
        counts,
        load_begin: MARK_AT[M_PARSE_B].load(Ordering::Relaxed),
        runq_end,
        last_worker,
        hash_ns: [0.0; 2],
        keys: 0,
    });
}

/// Keeps the hash replay of the `task_data_by_path` keys (the mean ns of
/// one hash: first pass, second pass).
pub fn set_hash(hash_ns: [f64; 2], keys: usize) {
    if let Some(stash) = STASH.lock().unwrap().as_mut() {
        stash.hash_ns = hash_ns;
        stash.keys = keys;
    }
}

/// Prints the span summary of the stamped load.
fn print_summary(stash: Stash) {
    let Stash {
        mut events,
        counts,
        load_begin,
        runq_end,
        last_worker,
        hash_ns,
        keys,
    } = stash;
    let tail_start = last_worker.clamp(load_begin, runq_end);
    // After the last wait of the loader for a worker, the loader does not
    // wait again: all its work from there to the end of the queue is serial
    // (the `crit` window).
    let crit_start = events
        .iter()
        .filter(|&&(_, _, kind)| kind == WAIT)
        .map(|&(_, e, _)| e)
        .max()
        .unwrap_or(load_begin)
        .clamp(load_begin, runq_end);
    let (inner, whole) = calibrate();
    // Parents end after their children; sort by begin, longer first.
    events.sort_unstable_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));
    // Per window (0: the tail, 1: crit): the overlap of a span with it.
    let starts = [tail_start, crit_start];
    let overlap = |w: usize, b: u64, e: u64| e.min(runq_end).saturating_sub(b.max(starts[w]));
    let mut n = [0u64; KINDS];
    let mut gross = [0u64; KINDS];
    let mut child_gross = [0u64; KINDS];
    let mut children = [0u64; KINDS];
    let mut gross_in = [[0u64; KINDS]; 2];
    let mut child_in = [[0u64; KINDS]; 2];
    let mut children_in = [[0u64; KINDS]; 2];
    let mut n_in = [[0u64; KINDS]; 2];
    // The open spans: (end, kind).
    let mut stack: Vec<(u64, u8)> = Vec::new();
    for &(b, e, kind) in &events {
        while stack.last().is_some_and(|&(end, _)| end <= b) {
            stack.pop();
        }
        let k = kind as usize;
        n[k] += 1;
        gross[k] += e - b;
        let parent = stack.last().map(|&(_, parent)| parent as usize);
        if let Some(p) = parent {
            child_gross[p] += e - b;
            children[p] += 1;
        }
        for w in 0..2 {
            let part = overlap(w, b, e);
            gross_in[w][k] += part;
            let inside = b >= starts[w];
            n_in[w][k] += u64::from(inside);
            if let Some(p) = parent {
                child_in[w][p] += part;
                children_in[w][p] += u64::from(inside);
            }
        }
        stack.push((e, kind));
    }
    let ms = |ns: f64| ns / 1e6;
    let events_tail: u64 = n_in[0].iter().sum();
    let events_crit: u64 = n_in[1].iter().sum();
    eprintln!(
        "loadstamp load begin_ms={:.3} runq_end_ms={:.3} last_worker_ms={:.3} last_wait_ms={:.3} load_ms={:.3} tail_ms={:.3} crit_ms={:.3} events={} events_tail={} events_crit={} overhead_ms={:.3} overhead_tail_ms={:.3} overhead_crit_ms={:.3} inner_ns={:.1} whole_ns={:.1} hash_cold_ns={:.2} hash_warm_ns={:.2} keys={}",
        ms(load_begin as f64),
        ms(runq_end as f64),
        ms(last_worker as f64),
        ms(crit_start as f64),
        ms((runq_end - load_begin) as f64),
        ms((runq_end - tail_start) as f64),
        ms((runq_end - crit_start) as f64),
        events.len(),
        events_tail,
        events_crit,
        ms(events.len() as f64 * whole),
        ms(events_tail as f64 * whole),
        ms(events_crit as f64 * whole),
        inner,
        whole,
        hash_ns[0],
        hash_ns[1],
        keys
    );
    let extra = whole - inner;
    for k in 0..KINDS {
        if n[k] == 0 {
            continue;
        }
        let net = gross[k] as f64 - n[k] as f64 * inner;
        let self_net = net - child_gross[k] as f64 - children[k] as f64 * extra;
        let mut line = format!(
            "loadstamp span {} n={} n_tail={} n_crit={} gross_ms={:.3} net_ms={:.3} self_ms={:.3}",
            KIND_NAMES[k],
            n[k],
            n_in[0][k],
            n_in[1][k],
            ms(gross[k] as f64),
            ms(net),
            ms(self_net)
        );
        for (w, name) in ["tail", "crit"].iter().enumerate() {
            let net_in = gross_in[w][k] as f64 - n_in[w][k] as f64 * inner;
            let self_in = net_in - child_in[w][k] as f64 - children_in[w][k] as f64 * extra;
            line.push_str(&format!(
                " {name}_net_ms={:.3} self_{name}_ms={:.3}",
                ms(net_in),
                ms(self_in)
            ));
        }
        eprintln!("{line}");
    }
    for (index, value) in counts.iter().enumerate() {
        eprintln!("loadstamp count {} {}", COUNT_NAMES[index], value);
    }
}

/// Prints the marks once, after the first `bind_all` of the process.
pub fn print_marks() {
    if !on() || PRINTED.swap(1, Ordering::Relaxed) != 0 {
        return;
    }
    mark(M_BIND_E);
    let base = MARK_AT[M_PAPF_B].load(Ordering::Relaxed);
    let stash = STASH.lock().unwrap().take();
    let mut line = String::from("loadstamp marks");
    for (index, name) in MARK_NAMES.iter().enumerate() {
        let at = MARK_AT[index].load(Ordering::Relaxed);
        if at == 0 {
            line.push_str(&format!(" {name}=-"));
        } else {
            line.push_str(&format!(" {name}={:.3}", (at as f64 - base as f64) / 1e6));
        }
    }
    eprintln!("{line}");
    if let Some(stash) = stash {
        print_summary(stash);
    }
}
