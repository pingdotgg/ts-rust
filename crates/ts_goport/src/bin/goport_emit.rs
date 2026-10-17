//! `goport_emit -p <tsconfig> --outDir <dir> [tsc options]`: compiles a
//! project with the Go port and writes the `.js`, `.d.ts` and `.map`
//! outputs like `tsgo`.
//!
//! Go: execute/tsc.go `CommandLine` and `tscCompilation`
//! (`execute::execute_tsc`): the Go command line parser, the Go
//! branches for errors, `--init`, `--version`, `--help`, `-p`, the config
//! search and `--showConfig`, then `performCompilation`, which reports
//! through execute/tsc/emit.go `EmitAndReportStatistics`. All output goes
//! to stdout, as in Go. The report is the shared `execute::tsc` one, as for
//! `goport` and `goport_build`.
//!
//! Output paths are the Go paths. A config `declarationDir`, or a `.js`
//! file of a source outside the common source directory, can put an output
//! outside `--outDir`.
//!
//! Safety (not in Go; see `EmitBin`): every write must be under the write
//! root, and no write may replace a program source file. By default the
//! write root is `--outDir`, and the compile step stops before it writes
//! anything when `--outDir` is inside the project directory or a source
//! directory. A write outside the root is refused and reported as TS5033,
//! so the project inputs are never written. The emit guard does not cover
//! `--init` (a `tsconfig.json`), `--pprofDir` (two empty profile files,
//! see `ts_goport::pprof`) and `--generateTrace` (the trace directory, see
//! `ts_goport::tracing`), which write as in Go.
//!
//! Options:
//! - `--outDir <dir>` (required for a compile) replaces the config
//!   `outDir`, as in tsgo.
//! - `--writeRoot <dir>` (`goport_emit` only) sets the write root. Use it
//!   when Go writes outside `--outDir` (for example a config
//!   `declarationDir`) and every such path is in a scratch copy under
//!   `<dir>`. The `--outDir` location checks are then skipped. It is taken
//!   out of the command line before the Go parser reads it.
//! - Every other argument goes to the Go command line parser, as in tsgo
//!   (`--declaration`, `--sourceMap`, `--noEmit false`, `--outFile`,
//!   `--listEmittedFiles`, `--noEmitOnError`, `--pretty`, ...). An unknown
//!   option is TS5023 on stdout, exit 1.
//!
//! `.tsbuildinfo` is not written (see `perform_incremental_compilation` in
//! `execute::execute_tsc`).
//!
//! Exit codes are the tsc ones: 0, 1 when there are diagnostics and the
//! emit was skipped (also command line and project errors, and a refused
//! `--outDir`), 2 when there are diagnostics and outputs were written (also
//! config file read errors). A run that hit unported code (or another
//! panic) exits `execute::tsc::EXIT_UNPORTED` (70), a code tsgo never
//! returns (Go uses 0 to 5), and says so on stderr. A Go panic that the
//! port keeps (`core::go_panic`) ends the run as in Go: the output so far,
//! `panic: <message>` on stderr and exit 2.

use std::any::Any;
use std::cell::{Cell, OnceCell};
use std::collections::HashSet;
use std::io::Write;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use ts_goport::emitter::program_emit::{EmitOptions, EmitResult, WriteFile, WriteFileData, emit};
use ts_goport::execute::execute_tsc::{TscCompilationHooks, command_line};
use ts_goport::execute::tsc::{
    EXIT_UNPORTED, ExitStatus, ProgramLike, System, Writer, new_os_system, write_go_output,
};
use ts_goport::frontend::tsoptions::ParsedCommandLine;
use ts_goport::frontend::tspath;
use ts_goport::gostd::context;
use ts_goport::prelude::*;
use ts_goport::scanner_util::go_string_bytes;

const UNPORTED_PREFIX: &str = "unported Go code";

const USAGE: &str = "usage: goport_emit -p <tsconfig.json | project dir> --outDir <dir> [--writeRoot <dir>] [tsc options]";

/// The opt-in `jemalloc` feature makes jemalloc the global allocator
/// (see `goport.rs` `set_malloc_tunables`).
#[cfg(all(feature = "jemalloc", not(windows)))]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

fn main() {
    // Go: `System.SinceStart` counts from the process start.
    let start = Instant::now();
    let (args, write_root) = match take_write_root(ts_goport::frontend::vfs::os_args()) {
        Ok(parsed) => parsed,
        Err(message) => {
            eprintln!("goport_emit: {message}");
            eprintln!("{USAGE}");
            std::process::exit(1);
        }
    };
    install_panic_hook();
    let worker = std::thread::Builder::new()
        .name("goport_emit".to_string())
        .stack_size(ts_goport::gostd::stack::max_stack_size())
        .spawn(move || run(&args, write_root, start));
    let code = if let Ok(Ok(code)) = worker.map(std::thread::JoinHandle::join) {
        code
    } else {
        eprintln!("goport_emit: worker thread failed");
        EXIT_UNPORTED
    };
    std::process::exit(code);
}

/// Takes `--writeRoot <dir>` (or `--writeRoot=<dir>`) out of `args` and
/// returns the other arguments, for the Go parser, and the absolute write
/// root.
fn take_write_root(args: Vec<String>) -> Result<(Vec<String>, Option<String>), String> {
    let mut rest = Vec::with_capacity(args.len());
    let mut write_root = None;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        if arg == "--writeRoot" {
            let dir = iter
                .next()
                .ok_or_else(|| "--writeRoot needs a value".to_string())?;
            write_root = Some(absolute(&dir));
        } else if let Some(dir) = arg.strip_prefix("--writeRoot=") {
            write_root = Some(absolute(dir));
        } else {
            rest.push(arg);
        }
    }
    Ok((rest, write_root))
}

/// `path` made absolute against the current directory and normalized.
fn absolute(path: &str) -> String {
    let cwd = ts_goport::frontend::vfs::os_current_dir()
        .map(|d| d.replace('\\', "/"))
        .unwrap_or_default();
    tspath::resolve_path(&cwd, &[path])
}

/// Whether `path` is `dir` or inside it (both absolute and normalized).
fn is_inside(path: &str, dir: &str) -> bool {
    let dir = dir.trim_end_matches('/');
    path == dir || path.starts_with(&format!("{dir}/"))
}

/// Keeps unported panics quiet (they are counted) and prints other panics.
/// `run` prints a Go panic.
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
            .map(|l| format!(" at {}:{}", go_site_file(l.file()), l.line()))
            .unwrap_or_default();
        eprintln!("goport_emit: panic{location}: {message}");
        if std::env::var_os("GOPORT_TRACE").is_some() {
            eprintln!("{}", ts_goport::backtrace::force_capture());
        }
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

// Go: cmd/tsc/main.go runMain: `execute.CommandLine` with the process
// system and arguments, then `os.Exit` with the status.
// PORT: the report goes to a buffer that is written to stdout at the end,
// also when a step panics. A panic ends the run; the steps inside
// `GuardedProgram` are guarded on their own. A Go panic (`go_panic`) ends
// the run with the Go runtime exit code.
fn run(args: &[String], write_root: Option<String>, start: Instant) -> i32 {
    let sys = match new_os_system() {
        Ok(sys) => sys,
        Err(status) => return status.code(),
    };
    let buffer: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
    let writer: Writer = buffer.clone();
    let sys: Rc<dyn System> = Rc::new(sys.with_start(start).with_writer(writer));

    let bin = EmitBin {
        write_root,
        check_out_dir: Cell::new(true),
        out_dir: OnceCell::new(),
        root: OnceCell::new(),
        write_file: OnceCell::new(),
        refused: Arc::new(Mutex::new(Vec::new())),
    };
    let result = catch_unwind(AssertUnwindSafe(|| {
        command_line(&context::background(), sys.clone(), args, &bin)
    }));

    let refused = bin.refused.lock().map(|r| r.clone()).unwrap_or_default();
    // A refused write is also a TS5033 diagnostic, so the status is the tsc one.
    finish(
        &buffer,
        &refused,
        bin.root.get().map_or("", String::as_str),
        result.map(|result| result.status),
    )
}

/// The `goport_emit` part of the compile step (see `TscCompilationHooks`):
/// the `--outDir` rules and the guarded `WriteFile`.
// PORT: not in Go. tsgo writes any output path; goport_emit requires
// `--outDir`, refuses an out dir in the project or a source directory,
// and refuses writes outside the write root (emit-modes D2 and D3, equal
// with `--writeRoot`). The out dir location checks are skipped under
// `--writeRoot` and under `listFilesOnly`: Go does not call `Emit` then
// (execute/tsc/emit.go:106), so nothing is written.
struct EmitBin {
    /// `--writeRoot`, or `None` for the default root, the `--outDir`.
    write_root: Option<String>,
    /// Whether the out dir location checks run (see above).
    check_out_dir: Cell<bool>,
    /// The command line `--outDir`, absolute (Go makes it absolute in
    /// `ParseCommandLine`).
    out_dir: OnceCell<String>,
    /// The write root in use.
    root: OnceCell<String>,
    write_file: OnceCell<WriteFile>,
    refused: Arc<Mutex<Vec<String>>>,
}

impl TscCompilationHooks for EmitBin {
    // PORT: `-b` is unported here: the build writes outputs without the
    // write guard. `goport_build` runs build mode.
    fn build_mode(&self) -> bool {
        false
    }

    fn prepare_compilation(
        &self,
        sys: &dyn System,
        command_line_options: &CompilerOptions,
        config_file_name: &str,
        config: &mut ParsedCommandLine,
    ) -> Result<(), ExitStatus> {
        let out_dir = &command_line_options.out_dir;
        if out_dir.is_empty() {
            eprintln!("goport_emit: --outDir is required");
            eprintln!("{USAGE}");
            return Err(ExitStatus::DiagnosticsPresentOutputsSkipped);
        }
        self.check_out_dir
            .set(self.write_root.is_none() && !config.compiler_options().list_files_only.is_true());
        // The project directory: the config directory, or the current
        // directory for source files on the command line.
        let project_dir = if config_file_name.is_empty() {
            sys.get_current_directory()
        } else {
            tspath::get_directory_path(config_file_name)
        };
        if self.check_out_dir.get() && is_inside(out_dir, &project_dir) {
            eprintln!(
                "goport_emit: refusing --outDir {out_dir} inside the project directory {project_dir}"
            );
            return Err(ExitStatus::DiagnosticsPresentOutputsSkipped);
        }
        let _ = self.out_dir.set(out_dir.clone());
        let _ = self
            .root
            .set(self.write_root.clone().unwrap_or_else(|| out_dir.clone()));
        Ok(())
    }

    /// Refuses before any write when the out dir is inside a source
    /// directory, and makes the guarded `WriteFile`.
    fn program_created(&self) -> Result<(), ExitStatus> {
        let out_dir = self.out_dir.get().expect("prepare_compilation ran");
        let inputs: HashSet<String> = source_files()
            .into_iter()
            .map(|file| tspath::normalize_path(source_file_file_name(file)))
            .collect();
        for file in source_files() {
            let dir = tspath::get_directory_path(source_file_file_name(file));
            if self.check_out_dir.get()
                && !source_file_info(file).is_declaration_file
                && is_inside(out_dir, &dir)
            {
                eprintln!(
                    "goport_emit: refusing --outDir {out_dir} inside the source directory {dir}"
                );
                return Err(ExitStatus::DiagnosticsPresentOutputsSkipped);
            }
        }
        let root = self.root.get().expect("prepare_compilation ran").clone();
        let _ = self
            .write_file
            .set(new_write_file(root, inputs, self.refused.clone()));
        Ok(())
    }

    fn program_like(&self) -> Option<&dyn ProgramLike> {
        Some(&GuardedProgram)
    }

    fn write_file(&self) -> Option<WriteFile> {
        self.write_file.get().cloned()
    }
}

/// The Go `WriteFile` callback: writes only under `root`, and never over a
/// program source file (`inputs`).
fn new_write_file(
    root: String,
    inputs: HashSet<String>,
    refused: Arc<Mutex<Vec<String>>>,
) -> WriteFile {
    Arc::new(
        move |file_name: &str, text: &str, _data: &mut WriteFileData| -> Result<(), String> {
            let path = tspath::normalize_path(file_name);
            if !is_inside(&path, &root)
                || path == root.trim_end_matches('/')
                || inputs.contains(&path)
            {
                if let Ok(mut refused) = refused.lock() {
                    refused.push(path.clone());
                }
                return Err(format!("{path} is outside the output directory"));
            }
            // `path` is the port form of the Go path; the OS gets its Go bytes.
            let target = ts_goport::frontend::vfs::os_path(&path);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            // Go writes the string bytes unchanged. `text` is the port form
            // of the Go string, so write its Go bytes.
            std::fs::write(&target, go_string_bytes(text)).map_err(|e| e.to_string())
        },
    )
}

/// Writes the report to stdout, then a Go panic, the refused writes and the
/// unported counts to stderr, and returns the exit code: the tsc status,
/// `EXIT_GO_PANIC` after a Go panic, or `EXIT_UNPORTED` when something was
/// unported or another panic ended the run.
fn finish(
    buffer: &RefCell<Vec<u8>>,
    refused: &[String],
    write_root: &str,
    result: std::thread::Result<ExitStatus>,
) -> i32 {
    let mut stdout = std::io::stdout().lock();
    let _ = write_go_output(&mut stdout, &buffer.borrow());
    let _ = stdout.flush();

    let code = match result {
        Ok(status) => status.code(),
        Err(payload) if print_go_panic(payload.as_ref()) => EXIT_GO_PANIC,
        Err(payload) => {
            note_panic(payload.as_ref());
            ExitStatus::Success.code()
        }
    };
    for path in refused {
        eprintln!("goport_emit: refused to write {path} (outside {write_root} or an input file)");
    }
    let unported = unported_report();
    let mut stderr = std::io::stderr().lock();
    for (name, count) in &unported {
        let _ = writeln!(stderr, "unported: {name} {count}");
    }

    if !unported.is_empty() {
        return EXIT_UNPORTED;
    }
    code
}

/// The installed program as a Go `ProgramLike`, with each step guarded on
/// its own so one unported path does not hide the other diagnostics.
struct GuardedProgram;

impl ProgramLike for GuardedProgram {
    fn options(&self) -> &'static CompilerOptions {
        options()
    }
    fn get_bind_diagnostics(&self, file: Node) -> Vec<Diagnostic> {
        guard(|| get_bind_diagnostics(file))
    }
    fn get_global_diagnostics(&self) -> Vec<Diagnostic> {
        guard(get_global_diagnostics)
    }
    fn get_semantic_diagnostics(&self, file: Node) -> Vec<Diagnostic> {
        collect_checker_diagnostics_with(file, check_file_guarded)
    }
    fn get_declaration_diagnostics(&self, file: Node) -> Vec<Diagnostic> {
        guard(|| get_declaration_diagnostics(file))
    }
    // PORT: `.tsbuildinfo` is not written (see the top).
    fn emit(&self, emit_options: EmitOptions) -> EmitResult {
        guard(|| emit(emit_options))
    }
}

/// Semantic diagnostics for one file. A panic drops that file's results and
/// replaces the checker, whose caches may be half written. A Go panic goes
/// on.
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
