//! `goport_typesyms -p <tsconfig> -o <outdir>`: writes the harness `.types`
//! and `.symbols` baselines for a project with the Go port, in the format of
//! the Go tool `tools/tsgo-src/cmd/typesymdump`.
//!
//! Flow (same as the Go dumper):
//! 1. Load the tsconfig as is (no `noEmit` override).
//! 2. Run the `tsgo --noEmit` diagnostics pass. Type ids and union order
//!    depend on the check order, so the walk must follow the same pass.
//! 3. Walk types for every non-default-lib file, then symbols for every file.
//!    Output names are the path relative to the tsconfig directory with `/`
//!    replaced by `!`. The header is that relative path.
//! 4. Write `files.txt`: unit names in walk order, then
//!    `hadErrorBaseline <bool>`.
//!
//! Each node's checker work runs under `catch_unwind`. A panicking node gets
//! `<<goport panic: MESSAGE>>` as its type or symbol text; the checker is
//! kept so later type ids do not shift. stderr gets `unported: <name>
//! <count>` lines. Exit 2 when anything panicked.
//!
//! A Go panic that the port keeps (`core::go_panic`) ends the run as the
//! Go dumper ends it: `panic: <message>` on stderr, exit 2 and no output
//! written after it. A config that cannot be read is one: the Go dumper
//! makes a program from the nil config and panics.

use std::any::Any;
use std::io::Write;
use std::panic::{AssertUnwindSafe, catch_unwind};

use ts_goport::baseline::type_symbol::{TestFile, generate_baseline, new_type_writer_walker};
use ts_goport::frontend::vfs::os_path;
use ts_goport::prelude::*;
use ts_goport::scanner_util::go_string_bytes;

const UNPORTED_PREFIX: &str = "unported Go code";

/// The opt-in `jemalloc` feature makes jemalloc the global allocator
/// (see `goport.rs` `set_malloc_tunables`).
#[cfg(all(feature = "jemalloc", not(windows)))]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

fn main() {
    let args: Vec<String> = ts_goport::frontend::vfs::os_args();
    let (mut project, mut out_dir) = (None, None);
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-p" => project = iter.next(),
            "-o" => out_dir = iter.next(),
            _ => {
                eprint_go(&format!("goport_typesyms: unknown argument {arg}\n"));
                std::process::exit(1);
            }
        }
    }
    let (Some(project), Some(out_dir)) = (project, out_dir) else {
        eprintln!("usage: goport_typesyms -p <tsconfig> -o <outdir>");
        std::process::exit(1);
    };
    install_panic_hook();
    // The loading thread keeps the frontend program and the checker pool, so
    // the whole run stays on it. The checkers run on their own threads.
    let worker = std::thread::Builder::new()
        .name("goport_typesyms".to_string())
        .stack_size(ts_goport::gostd::stack::max_stack_size())
        .spawn(move || run(&project, &out_dir));
    let code = match worker.map(std::thread::JoinHandle::join) {
        Ok(Ok(code)) => code,
        Ok(Err(payload)) if print_go_panic(payload.as_ref()) => {
            for (name, count) in unported_report() {
                eprintln!("unported: {name} {count}");
            }
            EXIT_GO_PANIC
        }
        _ => {
            eprintln!("goport_typesyms: worker thread failed");
            2
        }
    };
    std::process::exit(code);
}

/// Writes the Go bytes of the port form `text` to stderr (see
/// `scanner_util::GO_STRING_MARKER`).
fn eprint_go(text: &str) {
    let _ = std::io::stderr().write_all(&go_string_bytes(text));
}

/// Keeps unported panics quiet (they are counted) and prints other panics.
/// `main` prints a Go panic.
fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        if info.payload().is::<GoPanic>() {
            return;
        }
        let message = payload_message(info.payload());
        if message.starts_with(UNPORTED_PREFIX) {
            if std::env::var_os("GOPORT_TRACE").is_some() {
                eprintln!(
                    "trace: {message}\n{}",
                    ts_goport::backtrace::force_capture()
                );
            }
            return;
        }
        let location = info
            .location()
            .map(|l| format!(" at {}:{}", l.file(), l.line()))
            .unwrap_or_default();
        eprintln!("goport_typesyms: panic{location}: {message}");
    }));
}

fn payload_message(payload: &(dyn Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        String::new()
    }
}

fn note_panic(payload: &(dyn Any + Send)) {
    if !payload_message(payload).starts_with(UNPORTED_PREFIX) {
        record_unported("panic");
    }
}

/// Runs `f`, or returns the default value when it panics. A Go panic goes
/// on.
fn guard<T: Default>(f: impl FnOnce() -> T) -> T {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(value) => value,
        Err(payload) => {
            note_panic(resume_go_panic(payload).as_ref());
            T::default()
        }
    }
}

/// Go `compiler.GetDiagnosticsOfAnyProgram(ctx, program, nil, false,
/// program.GetBindDiagnostics, program.GetSemanticDiagnostics)`, with the
/// same per-file guards as `goport`. A file whose check panics gets a new
/// checker, as in `goport`, so the walk sees the `goport` checker state.
fn collect_all_diagnostics() -> Vec<Diagnostic> {
    get_diagnostics_of_any_program(
        None, // #4699: Go nil files
        false,
        &mut |file| guard(|| get_bind_diagnostics(file)),
        &mut |file| collect_checker_diagnostics_with(file, check_file_guarded),
        &mut || guard(get_global_diagnostics),
        &mut |file| guard(|| get_declaration_diagnostics(file)),
        true, // #64452: Go `program.(*Program)` ok
    )
}

fn check_file_guarded(checker: &mut Checker, file: Node) -> Vec<Diagnostic> {
    match catch_unwind(AssertUnwindSafe(|| {
        get_semantic_diagnostics_with_checker(
            &ts_goport::gostd::context::background(),
            checker,
            file,
        )
    })) {
        Ok(diagnostics) => diagnostics,
        Err(payload) => {
            note_panic(resume_go_panic(payload).as_ref());
            let index = (checker.id - 1) as usize;
            *checker = Checker::new(index);
            Vec::new()
        }
    }
}

/// Go `filepath.Clean` of an absolute `/` path: drops `.` and resolves `..`
/// lexically.
fn clean_components(path: &str) -> Vec<&str> {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            _ => parts.push(part),
        }
    }
    parts
}

/// Go `filepath.Rel(base, target)` for two absolute paths.
fn relative_path(base: &str, target: &str) -> String {
    let base = clean_components(base);
    let target = clean_components(target);
    let common = base.iter().zip(&target).take_while(|(a, b)| a == b).count();
    let mut parts: Vec<&str> = vec![".."; base.len() - common];
    parts.extend_from_slice(&target[common..]);
    if parts.is_empty() {
        return ".".to_string();
    }
    parts.join("/")
}

/// Go `filepath.Abs(filepath.Dir(project))`.
fn project_dir(project: &str) -> String {
    let dir = match project.rfind('/') {
        Some(0) => "/",
        Some(index) => &project[..index],
        None => ".",
    };
    if dir.starts_with('/') {
        return format!("/{}", clean_components(dir).join("/"));
    }
    let cwd = ts_goport::frontend::vfs::os_current_dir().expect("current directory");
    let joined = format!("{cwd}/{dir}");
    format!("/{}", clean_components(&joined).join("/"))
}

struct Unit {
    file: TestFile,
    header: String,
    name: String,
}

fn run(project: &str, out_dir: &str) -> i32 {
    match catch_unwind(AssertUnwindSafe(|| try_load(project))) {
        Ok(Ok(_)) => {}
        Ok(Err(message)) => {
            // Go: the dumper prints the count of config read errors, and
            // `compiler.NewProgram` with the nil config panics.
            // PORT: the error text takes the place of the count.
            eprint_go(&format!("goport_typesyms: {message}\n"));
            go_panic(
                "runtime error: invalid memory address or nil pointer dereference".to_string(),
            );
        }
        Err(payload) => {
            note_panic(resume_go_panic(payload).as_ref());
            eprintln!("goport_typesyms: load panicked");
            return 2;
        }
    }

    let diagnostics = guard(collect_all_diagnostics);
    let had_error_baseline = !diagnostics.is_empty();

    let dir = project_dir(project);
    let units: Vec<Unit> = source_files()
        .into_iter()
        .filter(|&f| !is_source_file_default_library(&source_file_info(f).path))
        .map(|f| {
            let file_name = source_file_file_name(f);
            let header = relative_path(&dir, file_name);
            Unit {
                name: header.replace('/', "!"),
                header,
                file: TestFile {
                    unit_name: file_name.to_string(),
                    content: source_file_text(f).to_string(),
                },
            }
        })
        .collect();

    std::fs::create_dir_all(os_path(out_dir)).expect("create output directory");
    let mut walker = new_type_writer_walker(had_error_baseline);
    walker.catch_panics = true;
    for is_symbol in [false, true] {
        let ext = if is_symbol { ".symbols" } else { ".types" };
        for unit in &units {
            let text = generate_baseline(
                std::slice::from_ref(&unit.file),
                &mut walker,
                &unit.header,
                is_symbol,
            );
            // Go writes the baseline string bytes unchanged. `text` and the
            // file name are the port form of Go strings, so write their Go
            // bytes.
            std::fs::write(
                os_path(&format!("{out_dir}/{}{ext}", unit.name)),
                go_string_bytes(&text),
            )
            .expect("write baseline");
        }
    }

    let mut list = String::new();
    for unit in &units {
        list.push_str(&unit.file.unit_name);
        list.push('\n');
    }
    list.push_str("hadErrorBaseline ");
    list.push_str(if had_error_baseline { "true" } else { "false" });
    list.push('\n');
    std::fs::write(
        os_path(&format!("{out_dir}/files.txt")),
        go_string_bytes(&list),
    )
    .expect("write files.txt");

    let unported = unported_report();
    let mut stderr = std::io::stderr().lock();
    let _ = writeln!(
        stderr,
        "files {} diagnostics {} node panics {}",
        units.len(),
        diagnostics.len(),
        walker.panic_count
    );
    for (name, count) in &unported {
        let _ = writeln!(stderr, "unported: {name} {count}");
    }
    if unported.is_empty() && walker.panic_count == 0 {
        0
    } else {
        2
    }
}
