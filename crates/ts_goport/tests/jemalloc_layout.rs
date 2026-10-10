//! The jemalloc layout (`src/jemalloc_layout.rs`).
//!
//! `layout_from_inside` runs again in a process with the jemalloc settings
//! of the bins and jemalloc as its allocator, calls `jemalloc_layout` and
//! checks what that process can see: 4 arenas, the holder thread, and
//! arena grows at 2 MiB boundaries in every arena.
//!
//! `tsgo_exits_with_the_holder`: tsgo still exits, with its exit code, in
//! each mode that keeps the holder thread to the end (plain, worker,
//! `--singleThreaded`, `-b`, `-w` and `--lsp`).
#![cfg(all(target_os = "linux", feature = "jemalloc"))]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use tikv_jemalloc_ctl::{Access, AsName};
use ts_goport::jemalloc_layout::{HOLDER_NAME, jemalloc_layout};

#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

/// Set in the process that `layout_from_inside` starts.
const INNER: &str = "JEMALLOC_LAYOUT_TEST_INNER";

const MIB: usize = 1 << 20;

/// The size and number of the blocks that `layout_from_inside` makes in
/// each arena.
const BLOCK: usize = 5 * MIB / 2;
const BLOCKS: usize = 8;

/// The longest time a tsgo run of the small project may take.
const RUN_LIMIT: Duration = Duration::from_secs(60);

#[test]
fn layout_from_inside() {
    if std::env::var_os(INNER).is_none() {
        let status = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "layout_from_inside", "--nocapture"])
            .args(["--test-threads", "1"])
            .env(INNER, "1")
            .env("_RJEM_MALLOC_CONF", bins_jemalloc_conf())
            .env_remove("GOPORT_JEMALLOC_LAYOUT")
            .status()
            .unwrap();
        assert!(status.success(), "{status}");
        return;
    }
    if ts_goport::gostd::stack::memory_limit().is_some() {
        eprintln!("skipped: the layout does nothing under a memory limit");
        return;
    }
    let arenas = Access::<u32>::read(b"opt.narenas\0".name()).unwrap();
    assert_eq!(arenas, 4, "narenas of the bins' jemalloc settings");
    jemalloc_layout();
    // The holder ends at once when it cannot bind the last arena.
    std::thread::sleep(Duration::from_millis(50));
    assert!(
        has_thread(Path::new("/proc/self"), HOLDER_NAME),
        "no {HOLDER_NAME} thread"
    );
    // A block of 8 MiB or more goes to jemalloc's huge arena, which maps
    // its first 16 MiB as one piece. When that is not at a 2 MiB boundary,
    // this kernel does not align anonymous mappings (before Linux 6.7, THP
    // not built in, or THP off for this process).
    let probe = Vec::<u8>::with_capacity(16 * MIB);
    if !on_2mib(probe.as_ptr()) {
        eprintln!("skipped the grow check: this kernel does not align a 16 MiB mapping");
        return;
    }
    // In each arena, blocks of 2.5 MiB fill its 8 MiB grow and then make it
    // grow by 10 and 12 MiB. A block that does not follow the one before it
    // starts a new grow, and must be at a 2 MiB boundary. Without the
    // layout, the arena grows by 2.5, 3 and 3.5 MiB here, at unaligned
    // addresses. Block 0 can follow small blocks of another thread.
    let own = Access::<u32>::read(b"thread.arena\0".name()).unwrap();
    // Made here, so that no other allocation goes into the arena under test.
    let mut blocks: Vec<Vec<u8>> = Vec::with_capacity(BLOCKS * arenas as usize);
    for arena in 0..arenas {
        b"thread.arena\0".name().write(arena).unwrap();
        let mut grows = 0;
        for n in 0..BLOCKS {
            let block = Vec::<u8>::with_capacity(BLOCK);
            let follows = blocks
                .last()
                .is_some_and(|last| last.as_ptr().wrapping_add(BLOCK) == block.as_ptr());
            if n > 0 && !follows {
                grows += 1;
                assert!(on_2mib(block.as_ptr()), "arena {arena}, block {n}");
            }
            blocks.push(block);
        }
        assert!(grows >= 2, "arena {arena}: {grows} grows");
    }
    b"thread.arena\0".name().write(own).unwrap();
}

#[test]
fn tsgo_exits_with_the_holder() {
    let dir =
        TempDir::new(std::env::temp_dir().join(format!("jemalloc_layout-{}", std::process::id())));
    let project = small_project(&dir.0);
    let p = project.to_str().unwrap();
    // Plain, in a worker (`GOPORT_LAUNCH=1`), the one checker of
    // `--singleThreaded` and build mode. Each run ends with exit 0.
    let runs: [(&[&str], &str); 4] = [
        (&["-p", p], "0"),
        (&["-p", p], "1"),
        (&["-p", p, "--singleThreaded"], "0"),
        (&["-b", p], "0"),
    ];
    for (args, launch) in runs {
        let child = tsgo(args).env("GOPORT_LAUNCH", launch).spawn().unwrap();
        let status = wait_limited(child, args);
        assert_eq!(status.code(), Some(0), "{args:?} GOPORT_LAUNCH={launch}");
    }
    // Watch mode ends with exit 0 on SIGINT after its first build (its own
    // outDir shows the end of that build). The LSP server ends with exit 0
    // when its input closes. Both have the holder until then, except under
    // a memory limit (`run-cargo-capped.sh` sets one), where the layout does
    // nothing.
    let holder = ts_goport::gostd::stack::memory_limit().is_none();
    let out = dir.0.join("watch-out");
    let mut watch = tsgo(&["-w", "-p", p, "--outDir", out.to_str().unwrap()])
        .spawn()
        .unwrap();
    let watch_dir = proc_dir(&watch);
    wait_for(&mut watch, || {
        (!holder || has_thread(&watch_dir, HOLDER_NAME))
            && has_thread(&watch_dir, "signal.Notify")
            && out.join("b.js").exists()
    });
    rustix::process::kill_process(
        rustix::process::Pid::from_child(&watch),
        rustix::process::Signal::INT,
    )
    .unwrap();
    assert_eq!(wait_limited(watch, &["-w"]).code(), Some(0), "-w");
    let mut lsp = tsgo(&["--lsp", "--stdio"]).spawn().unwrap();
    let lsp_dir = proc_dir(&lsp);
    wait_for(&mut lsp, || !holder || has_thread(&lsp_dir, HOLDER_NAME));
    drop(lsp.stdin.take());
    assert_eq!(wait_limited(lsp, &["--lsp"]).code(), Some(0), "--lsp");
}

/// `JEMALLOC_CONF` of bin/tsgo.rs, so this test follows a change there.
fn bins_jemalloc_conf() -> &'static str {
    let source = include_str!("../src/bin/tsgo.rs");
    let line = source
        .lines()
        .find_map(|line| line.strip_prefix("const JEMALLOC_CONF: &str = \""))
        .unwrap();
    line.strip_suffix("\";").unwrap()
}

/// True when `ptr` is at a 2 MiB boundary.
fn on_2mib(ptr: *const u8) -> bool {
    (ptr as usize).is_multiple_of(2 * MIB)
}

/// True when the process at `proc_dir` (/proc/<pid>) has a thread whose
/// name starts with `name`.
fn has_thread(proc_dir: &Path, name: &str) -> bool {
    let Ok(tasks) = std::fs::read_dir(proc_dir.join("task")) else {
        return false;
    };
    tasks
        .filter_map(|task| std::fs::read_to_string(task.ok()?.path().join("comm")).ok())
        .any(|comm| comm.starts_with(name))
}

/// A tsgo command that does the work in its own process
/// (`GOPORT_LAUNCH=0`), with no output and its stdin a pipe.
fn tsgo(args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_tsgo"));
    command
        .args(args)
        .env("GOPORT_LAUNCH", "0")
        .env_remove("GOPORT_JEMALLOC_LAYOUT")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

/// The /proc dir of `child`.
fn proc_dir(child: &Child) -> PathBuf {
    PathBuf::from(format!("/proc/{}", child.id()))
}

/// Waits until `ready` is true, at most `RUN_LIMIT`, while `child` runs.
/// On a timeout it kills `child` before the panic, so that no `-w` or
/// `--lsp` tsgo stays after the test.
fn wait_for(child: &mut Child, ready: impl Fn() -> bool) {
    let start = Instant::now();
    while !ready() {
        if start.elapsed() >= RUN_LIMIT {
            let _ = child.kill();
            let _ = child.wait();
            panic!("tsgo {} not ready in {RUN_LIMIT:?}", child.id());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The exit status of `child`, which must end within `RUN_LIMIT`.
fn wait_limited(mut child: Child, args: &[&str]) -> ExitStatus {
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        if start.elapsed() > RUN_LIMIT {
            let _ = child.kill();
            let _ = child.wait();
            panic!("tsgo {args:?} did not end in {RUN_LIMIT:?}");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// A project of two files with no errors, in `dir`. Returns its tsconfig.
fn small_project(dir: &Path) -> PathBuf {
    std::fs::create_dir(dir.join("src")).unwrap();
    std::fs::write(dir.join("src/a.ts"), "export const a: number = 1;\n").unwrap();
    std::fs::write(
        dir.join("src/b.ts"),
        "import { a } from \"./a\";\nexport const b = a + 1;\n",
    )
    .unwrap();
    let tsconfig = dir.join("tsconfig.json");
    std::fs::write(
        &tsconfig,
        r#"{"compilerOptions":{"strict":true,"outDir":"out","rootDir":"src","module":"nodenext"},"include":["src"]}"#,
    )
    .unwrap();
    tsconfig
}

struct TempDir(PathBuf);

impl TempDir {
    fn new(path: PathBuf) -> Self {
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir(&path).unwrap();
        TempDir(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
