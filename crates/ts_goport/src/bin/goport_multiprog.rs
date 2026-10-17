//! `goport_multiprog`: checks program versions in one process (the
//! multi-program plan, wave 3). A multi-program process loads a version with
//! `try_load_version`, makes a new one after each edit with
//! `update_program_version` and frees old ones with `release_program`.
//!
//! `pair <tsconfig> <changed-file> <new-text-file> <out-dir> [--first <other-tsconfig>]`
//! loads version A, writes the new text over the changed file, makes
//! version B and checks the file versions of A and B. It writes to
//! `<out-dir>`:
//! - `a.txt`, `b.txt`: the `goport -p` stdout of A and B;
//! - `a.status`, `b.status`: their exit codes (the number, no newline);
//! - `b.reused`: `true` when B shares the unchanged files of A, else `false`;
//! - `first.txt` (with `--first`): the report of the other project, which is
//!   loaded, reported and released first, so A and B get later file ids.
//!
//! `cycles <tsconfig> <changed-file> <count>` adds or removes a comment line
//! in the changed file `count` times. Each time it makes a new version,
//! reports it, releases the previous one and prints
//! `cycle <i> reused=<b> rss_kb=<VmRSS> hwm_kb=<VmHWM>`. The text of a
//! version alternates, so each report must equal the report of the last
//! version with the same text; a difference fails the run. After the last
//! release it prints `file_versions made=<n> dead=<m>`: the freeable file
//! versions (lsshells M3b) that the process made and freed. They are 0
//! unless `GOPORT_FREE_FILE_VERSIONS=1`, which gives each new parse of the
//! changed file a freeable version, as the language server does.
//!
//! Both modes put the original text back into the changed file at the end,
//! also after a failed check. A failed check or an unported hit panics.
//! Exit codes: 0 when every check passes, 1 when one fails, 2 for a usage
//! error.

use std::panic::catch_unwind;
use std::path::Path;

use ts_goport::execute::tsc::{
    CompileTimes, CompilerProgram, EmitInput, Writer, create_diagnostic_reporter,
    create_report_error_summary, emit_and_report_statistics, new_os_system,
};
use ts_goport::frontend::tspath::{normalize_path, resolve_path};
use ts_goport::prelude::*;

const USAGE: &str = "usage: goport_multiprog pair <tsconfig> <changed-file> <new-text-file> <out-dir> [--first <other-tsconfig>]
       goport_multiprog cycles <tsconfig> <changed-file> <count>";

/// The comment line that `cycles` adds and removes.
const CYCLE_LINE: &str = "\n// goport_multiprog cycle\n";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // The loading thread keeps the frontends and checker pools of every
    // version, so the whole run stays on it.
    let worker = std::thread::Builder::new()
        .name("goport_multiprog".to_string())
        .stack_size(ts_goport::gostd::stack::max_stack_size())
        .spawn(move || run(&args));
    let code = match worker.map(std::thread::JoinHandle::join) {
        Ok(Ok(Ok(()))) => 0,
        Ok(Ok(Err(message))) => {
            eprintln!("goport_multiprog: {message}\n{USAGE}");
            2
        }
        _ => {
            eprintln!("goport_multiprog: FAIL");
            1
        }
    };
    std::process::exit(code);
}

/// Reads the command line and runs one mode. `Err` is a usage error.
fn run(args: &[String]) -> Result<(), String> {
    let mut first = None;
    let mut rest = Vec::new();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == "--first" {
            first = Some(iter.next().ok_or("--first needs a tsconfig")?.as_str());
        } else {
            rest.push(arg.as_str());
        }
    }
    match rest.as_slice() {
        ["pair", config, changed, new_text_file, out_dir] => {
            pair(config, changed, new_text_file, Path::new(out_dir), first);
        }
        ["cycles", config, changed, count] if first.is_none() => {
            let count = count.parse().map_err(|_| format!("bad count {count}"))?;
            cycles(config, changed, count);
        }
        _ => return Err(format!("wrong arguments {args:?}")),
    }
    Ok(())
}

/// The `pair` mode. Each step number matches the plan.
fn pair(config: &str, changed: &str, new_text_file: &str, out: &Path, first: Option<&str>) {
    std::fs::create_dir_all(out).unwrap_or_else(|e| panic!("create {}: {e}", out.display()));
    let new_text = read(new_text_file);
    let changed_name = go_file_name(changed);

    // 1. An unrelated version first, so A and B are not in the first publish.
    if let Some(first) = first {
        let x = load(first);
        write(&out.join("first.txt"), &report(x).0);
        release_program(x);
    }

    // 2. Version A.
    let a = load(config);
    let a_report = report(a);
    write(&out.join("a.txt"), &a_report.0);
    write(&out.join("a.status"), a_report.1.to_string().as_bytes());

    // 3. The edit.
    let restore = Restore::new(changed);
    write(Path::new(changed), new_text.as_bytes());

    // 4. Version B.
    let (b, reused) = update_program_version(a, changed);
    write(&out.join("b.reused"), reused.to_string().as_bytes());

    // 5. File versions.
    check_versions(a, b, reused, &changed_name, &restore.text, &new_text);

    // 6. A works while B exists.
    expect_same(out, "a.again.txt", &a_report, &report(a));

    // 7. B.
    let b_report = report(b);
    write(&out.join("b.txt"), &b_report.0);
    write(&out.join("b.status"), b_report.1.to_string().as_bytes());

    // 8. B works after A is released (its workers joined).
    release_program(a);
    expect_same(out, "b.again.txt", &b_report, &report(b));

    // 9.
    release_program(b);
    drop(restore);
    println!("pair: pass reused={reused}");
}

/// Step 5 of `pair`: the changed file has a new version in B, both versions
/// are published and keep their text, published nodes are read-only, a
/// reused B shares every other file version of A in the same order, and the
/// new version is freeable only when the flag turns freeing on.
fn check_versions(
    a: &'static GoProgram,
    b: &'static GoProgram,
    reused: bool,
    changed: &str,
    old_text: &str,
    new_text: &str,
) {
    let a_changed = file_id(a, changed);
    let b_changed = file_id(b, changed);
    let a_max = a.source_file_order().iter().copied().max().unwrap_or(0);
    assert!(
        b_changed > a_max,
        "the changed file has id {b_changed} in B, not above every A id (max {a_max})"
    );
    for &id in a.source_file_order().iter().chain(&*b.source_file_order()) {
        assert!(is_published(id), "file {id} is not published");
    }
    assert!(
        file_store_text(a_changed) == old_text,
        "file {a_changed} (A) does not have the old text"
    );
    assert!(
        file_store_text(b_changed) == new_text,
        "file {b_changed} (B) does not have the new text"
    );
    if reused {
        // Go `UpdateProgram` replaces the changed file in place.
        let expected: Vec<usize> = a
            .source_file_order()
            .iter()
            .map(|&id| if id == a_changed { b_changed } else { id })
            .collect();
        assert!(
            *b.source_file_order() == *expected,
            "B does not keep the file ids and order of A"
        );
        let pairs = a.source_files().zip(b.source_files());
        for ((a_file, b_file), &id) in pairs.zip(b.source_file_order().iter()) {
            assert!(
                id == b_changed || std::ptr::eq(&*a_file, &*b_file),
                "B does not share the GoFile of {}",
                a_file.info.file_name
            );
        }
    }
    for id in [a_changed, b_changed] {
        assert!(
            published_write_panics(go_file(id).root),
            "set_store_node_parent on a node of published file {id} did not panic"
        );
    }
    // lsshells M3b: the first version of the changed file is static. Its
    // version in B is a freeable file version exactly when
    // `GOPORT_FREE_FILE_VERSIONS=1` (see `cycles`).
    assert!(
        file_version_probe(go_file(a_changed).root).is_none(),
        "file {a_changed} (A) is not static"
    );
    assert_eq!(
        file_version_probe(go_file(b_changed).root).is_some(),
        free_file_versions(),
        "file {b_changed} (B) is a freeable file version only when GOPORT_FREE_FILE_VERSIONS=1"
    );
}

/// The `cycles` mode: a leak record over `count` edits.
fn cycles(config: &str, changed: &str, count: usize) {
    let restore = Restore::new(changed);
    let edited = format!("{}{CYCLE_LINE}", restore.text);
    let mut current = load(config);
    // The reports of the original and the edited text, by `i % 2`.
    let mut reports = [Some(report(current)), None];
    for i in 1..=count {
        let text = if i % 2 == 1 { &edited } else { &restore.text };
        write(Path::new(changed), text.as_bytes());
        let (next, reused) = update_program_version(current, changed);
        let next_report = report(next);
        match &reports[i % 2] {
            Some(expected) => assert!(
                *expected == next_report,
                "cycle {i}: the report differs from the last version with the same text"
            ),
            None => reports[i % 2] = Some(next_report),
        }
        release_program(current);
        current = next;
        let (rss, hwm) = memory_kb();
        println!("cycle {i} reused={reused} rss_kb={rss} hwm_kb={hwm}");
    }
    release_program(current);
    println!(
        "file_versions made={} dead={}",
        file_versions_made(),
        dead_file_versions()
    );
    drop(restore);
}

/// The `goport` command line edits: `--noEmit --pretty false`.
fn goport_options(options: &mut CompilerOptions) {
    options.no_emit = Tristate::True;
    options.pretty = Tristate::False;
}

/// A new program version of `config`, with the `goport` options.
fn load(config: &str) -> &'static GoProgram {
    try_load_version(config, goport_options).unwrap_or_else(|e| panic!("load {config}: {e}"))
}

/// The `goport -p` report of `p`: the stdout bytes and the exit code.
/// Panics when any unported code was hit.
fn report(p: &'static GoProgram) -> (Vec<u8>, i32) {
    let _scope = enter_program(Some(p));
    let sys = new_os_system().unwrap_or_else(|status| panic!("no system: {status:?}"));
    let buffer: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
    let writer: Writer = buffer.clone();
    let sys = sys.with_writer(writer.clone());
    let options = options();
    let (result, _statistics) = emit_and_report_statistics(&EmitInput {
        sys: &sys,
        // #4407: under `noEmit`, Go's incremental `Program.Emit` gives the
        // result of the plain one (goport writes no build info), so the
        // report equals a fresh `goport` run.
        program_like: &CompilerProgram,
        config: None,
        report_diagnostic: create_diagnostic_reporter(
            &sys,
            writer.clone(),
            &ts_goport::locale::DEFAULT,
            options,
        ),
        report_error_summary: create_report_error_summary(
            &sys,
            &ts_goport::locale::DEFAULT,
            Some(options),
        ),
        writer,
        write_file: None,
        compile_times: Rc::new(RefCell::new(CompileTimes::default())),
        testing: None,
        testing_m_times_cache: None,
    });
    let hits = unported_report();
    assert!(
        hits.is_empty(),
        "program {} hit unported code: {hits:?}",
        p.id
    );
    (buffer.take(), result.status.code())
}

/// The id of the file of `p` named `file_name`.
fn file_id(p: &'static GoProgram, file_name: &str) -> usize {
    p.source_file_order()
        .iter()
        .copied()
        .find(|&id| go_file(id).info.file_name == file_name)
        .unwrap_or_else(|| panic!("program {} has no file {file_name}", p.id))
}

/// The Go file name of `path`: absolute and normalized, like the program's.
fn go_file_name(path: &str) -> String {
    let cwd = std::env::current_dir().expect("no current directory");
    resolve_path(&normalize_path(&cwd.to_string_lossy()), &[path])
}

/// True when `set_store_node_parent` on `node` panics because its file is
/// published. The expected panic is not printed.
fn published_write_panics(node: Node) -> bool {
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let result = catch_unwind(move || set_store_node_parent(node, Node::NIL));
    std::panic::set_hook(hook);
    let Err(payload) = result else {
        return false;
    };
    payload
        .downcast_ref::<String>()
        .is_some_and(|message| message.contains("published"))
}

/// Panics when a later report of a program differs from its first one. The
/// later report is kept in `<out>/<name>`.
fn expect_same(out: &Path, name: &str, first: &(Vec<u8>, i32), again: &(Vec<u8>, i32)) {
    if first != again {
        write(&out.join(name), &again.0);
        panic!(
            "{name}: the report changed (status {} then {})",
            first.1, again.1
        );
    }
}

/// `VmRSS` and `VmHWM` of this process in KiB, from `/proc/self/status`.
fn memory_kb() -> (u64, u64) {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let field = |name: &str| -> u64 {
        status
            .lines()
            .find_map(|line| line.strip_prefix(name))
            .and_then(|value| value.trim().trim_end_matches("kB").trim().parse().ok())
            .unwrap_or(0)
    };
    (field("VmRSS:"), field("VmHWM:"))
}

/// Puts the original text back into the changed file on drop, also when a
/// check panics.
struct Restore {
    path: String,
    text: String,
}

impl Restore {
    fn new(path: &str) -> Self {
        Self {
            path: path.to_string(),
            text: read(path),
        }
    }
}

impl Drop for Restore {
    fn drop(&mut self) {
        if let Err(e) = std::fs::write(&self.path, &self.text) {
            eprintln!("goport_multiprog: cannot restore {}: {e}", self.path);
        }
    }
}

fn read(path: &str) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {path}: {e}"))
}

fn write(path: &Path, bytes: &[u8]) {
    std::fs::write(path, bytes).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
}
