//! The wasm build of ts-rust: `tsc` (`ts_goport`) over a file system that the
//! JavaScript host provides (`host`). `npm/wasm` is the package that loads
//! it, and `scripts/wasm/build.sh` builds it for `wasm32-wasip1`.
//!
//! The host writes a request to the input buffer (`ts_input`), calls
//! `ts_run`, and reads the reply at `ts_output` (`ts_output_len` bytes).
//! One instance serves many requests: each `ts_run` loads a new program
//! version (`program::multirun`), whose unchanged files keep their parses
//! of the runs before, so a warm run parses only what changed. The parse
//! cache is keyed by the content of each file (bundled lib files skip the
//! hash: their text is embedded in the module), so the host needs no
//! mtime contract. A panic still ends the instance (wasm cannot unwind):
//! the host makes a new one.
//!
//! The request is NUL-separated UTF-8 fields: the current directory, the
//! flags (a decimal number of `FLAG_*` bits), then the tsc arguments. tsc
//! writes its usual output to stdout (WASI fd 1). With
//! `FLAG_DIAGNOSTICS_JSON`, the diagnostics are not printed: the reply is
//! a JSON array of them (the API's `DiagnosticResponse`, with UTF-16
//! positions). Otherwise the reply is empty. `ts_run` returns the exit
//! status. A panic prints its message to stderr and exits through WASI
//! `proc_exit`: 2 for a Go panic, 70 for any other.

use std::cell::RefCell;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::rc::Rc;
use std::sync::Arc;

use ts_goport::api::proto::new_diagnostic_response;
use ts_goport::execute::execute_tsc::{GoTsc, TscCompilationHooks, command_line};
use ts_goport::execute::tsc::{DiagnosticReporter, System, new_os_system};
use ts_goport::frontend::json::json_marshal;
use ts_goport::frontend::tspath::normalize_path;
use ts_goport::frontend::vfs::{Fs, OsOverride, install_os_override};
use ts_goport::gostd::context;
use ts_goport::scanner_util::go_string_from_utf8;

/// The current directory of the run that installed the OS override (see
/// `run`). Every run of the instance must ask for it.
static OVERRIDE_CWD: std::sync::OnceLock<String> = std::sync::OnceLock::new();

// E1: the request hash, the host world hash and the reply of the last run
// that used the whole pipeline. An identical request (the same bytes: cwd,
// flags and args) replayed while every file the parse cache knows serves
// the content it had (`multirun::file_world_fingerprint` — the module's
// only view of the files is what its loads read and hashed, and the held
// flow's config text is a function of its file set) returns the stored
// status and reply with no version, no check and no marshal. A changed
// guest reads differently, so its rerun re-checks. Only a
// `FLAG_DIAGNOSTICS_JSON` run is stored: its reply carries the whole
// answer, while a text run would also have to replay stdout.
thread_local! {
    static FINGERPRINT: RefCell<Option<(u64, u64, i32, String)>> = const { RefCell::new(None) };
}

pub mod host;

/// Report the diagnostics as JSON in the reply, not as text on stdout.
pub const FLAG_DIAGNOSTICS_JSON: u32 = 1;
/// The host file system ignores case (as on macOS and Windows).
pub const FLAG_CASE_INSENSITIVE: u32 = 2;

/// The tsc hooks of the wasm build: Go's (`GoTsc`), with the diagnostics
/// kept as JSON when the request asks for it.
struct WasmTsc {
    /// The JSON of each diagnostic, when the request asks for JSON.
    diagnostics: Option<Rc<RefCell<Vec<String>>>>,
}

impl TscCompilationHooks for WasmTsc {
    fn write_file(&self) -> Option<ts_goport::emitter::program_emit::WriteFile> {
        GoTsc.write_file()
    }

    fn diagnostic_reporter(&self, reporter: DiagnosticReporter) -> DiagnosticReporter {
        let Some(diagnostics) = &self.diagnostics else {
            return reporter;
        };
        let diagnostics = diagnostics.clone();
        Rc::new(move |diagnostic| {
            if let Ok(json) = json_marshal(&new_diagnostic_response(diagnostic), &[]) {
                diagnostics.borrow_mut().push(json);
            }
        })
    }
}

/// True when `args` is a `tsc -b` command line, by the test of
/// `execute_tsc::command_line`. Build mode reports through its own
/// reporters, which `diagnostic_reporter` does not reach.
fn is_build(args: &[String]) -> bool {
    args.first().is_some_and(|arg| {
        matches!(
            arg.to_lowercase().as_str(),
            "-b" | "--b" | "-build" | "--build"
        )
    })
}

/// Runs one request (see the crate comment) and returns the exit status and
/// the reply.
#[must_use]
pub fn run(request: &str) -> (i32, String) {
    let mut fields = request.split('\0');
    let cwd = normalize_path(fields.next().filter(|cwd| !cwd.is_empty()).unwrap_or("/"));
    let flags: u32 = fields
        .next()
        .and_then(|flags| flags.parse().ok())
        .unwrap_or(0);
    let args: Vec<String> = fields
        .map(|arg| go_string_from_utf8(arg.to_string()))
        .collect();
    // ts_goport ends a run that asks for watch mode or `--pprofDir` on
    // wasm (`execute_tsc` `wasm_unsupported`).
    if flags & FLAG_DIAGNOSTICS_JSON != 0 && is_build(&args) {
        eprintln!("error: diagnostics as JSON are not supported with --build");
        return (1, String::new());
    }
    // E1: an identical request over an unchanged host file system replays
    // the last reply (see `FINGERPRINT`). The hash covers the whole request;
    // the world hash is every path the parse cache knows, read and hashed
    // now (bundled lib files hash their path only), so a changed, deleted
    // or newly written guest never hits.
    let mut fingerprint = DefaultHasher::new();
    request.hash(&mut fingerprint);
    let request_hash = fingerprint.finish();
    let world_hash = if flags & FLAG_DIAGNOSTICS_JSON != 0 {
        ts_goport::program::multirun::file_world_fingerprint(&host::HostFs::new())
    } else {
        0
    };
    if flags & FLAG_DIAGNOSTICS_JSON != 0
        && let Some((seen, world, status, reply)) = FINGERPRINT.with(|cell| {
            cell.borrow()
                .as_ref()
                .map(|(h, w, s, r)| (*h, *w, *s, r.clone()))
        })
        && seen == request_hash
        && world == world_hash
    {
        return (status, reply);
    }

    // The case-insensitivity of the host file system is read once per
    // instance (`host::CASE_SENSITIVE`): a later run with the other flag
    // would run in the wrong mode with no error. Reject it explicitly, as
    // a cwd change below.
    let case_sensitive = flags & FLAG_CASE_INSENSITIVE == 0;
    match host::CASE_SENSITIVE.get() {
        Some(installed) if *installed != case_sensitive => {
            eprintln!(
                "error: this instance serves a {} file system, the request asks for {} (multi-run: one case mode per instance)",
                if *installed {
                    "case-sensitive"
                } else {
                    "case-insensitive"
                },
                if case_sensitive {
                    "case-sensitive"
                } else {
                    "case-insensitive"
                },
            );
            return (1, String::new());
        }
        Some(_) => {}
        None => {
            let _ = host::CASE_SENSITIVE.set(case_sensitive);
        }
    }
    // The override replaces the OS for the process, so it installs once
    // (`osvfs::install_os_override` panics on the second call): every run
    // of the instance resolves paths against the directory of the first
    // one. A request with another cwd is rejected explicitly instead of
    // being served with the wrong directory. The file system closure makes
    // a new `HostFs` per thread, so the file system of a run is the host's
    // now.
    match OVERRIDE_CWD.get() {
        Some(installed) if *installed != cwd => {
            eprintln!(
                "error: cwd is {cwd}, but this instance serves {installed} (multi-run: one cwd per instance)"
            );
            return (1, String::new());
        }
        Some(_) => {}
        None => {
            install_os_override(OsOverride {
                fs: Arc::new(|| Rc::new(host::HostFs::new()) as Rc<dyn Fs>),
                current_directory: cwd.clone(),
            });
            let _ = OVERRIDE_CWD.set(cwd);
        }
    }
    ts_goport::execute::tsc::stdio::init();
    let sys: Rc<dyn System> = match new_os_system() {
        Ok(sys) => Rc::new(sys),
        Err(status) => return (status.code(), String::new()),
    };

    // Multi-run (see the `program::multirun` module): the run's program is
    // a program version, built from the version of the run before, whose
    // unchanged files keep their parses. `end_run` releases the version of
    // the run before the last, after the diagnostics of this run are made.
    ts_goport::program::multirun::enable();
    let diagnostics = (flags & FLAG_DIAGNOSTICS_JSON != 0).then(Rc::default);
    let hooks = WasmTsc {
        diagnostics: diagnostics.clone(),
    };
    let status = command_line(&context::background(), sys.clone(), &args, &hooks)
        .status
        .code();
    ts_goport::program::multirun::end_run();
    let _ = sys.writer().borrow_mut().flush();
    let reply = diagnostics.map_or_else(String::new, |diagnostics| {
        format!("[{}]", diagnostics.take().join(","))
    });
    // E1: remember the answer of this exact request over this exact host
    // state (see `FINGERPRINT`). The world is hashed again: the run may
    // have written files (an emit), and the replay must not serve a reply
    // from before them.
    if flags & FLAG_DIAGNOSTICS_JSON != 0 {
        let world = ts_goport::program::multirun::file_world_fingerprint(&host::HostFs::new());
        FINGERPRINT.with(|cell| {
            *cell.borrow_mut() = Some((request_hash, world, status, reply.clone()));
        });
    }
    (status, reply)
}

#[cfg(target_family = "wasm")]
mod exports {
    //! The wasm ABI. These functions are the module's exports.

    use std::cell::RefCell;
    use std::io::Write;

    use ts_goport::execute::tsc::EXIT_UNPORTED;
    use ts_goport::prelude::{EXIT_GO_PANIC, print_go_panic, record_unported, unported_report};

    thread_local! {
        static INPUT: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
        static OUTPUT: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    }

    /// Makes the input buffer `len` bytes long and returns its address. The
    /// host writes the request there.
    #[allow(unsafe_code)]
    #[unsafe(no_mangle)]
    pub extern "C" fn ts_input(len: usize) -> *mut u8 {
        INPUT.with(|input| {
            let mut input = input.borrow_mut();
            input.clear();
            input.resize(len, 0);
            input.as_mut_ptr()
        })
    }

    /// Runs the request in the input buffer and returns the exit status.
    #[allow(unsafe_code)]
    #[unsafe(no_mangle)]
    pub extern "C" fn ts_run() -> i32 {
        install_panic_hook();
        let request = INPUT.with(|input| std::mem::take(&mut *input.borrow_mut()));
        let (status, reply) = super::run(&String::from_utf8_lossy(&request));
        OUTPUT.with(|output| *output.borrow_mut() = reply.into_bytes());
        status
    }

    /// The address of the reply of the last `ts_run`.
    #[allow(unsafe_code)]
    #[unsafe(no_mangle)]
    pub extern "C" fn ts_output() -> *const u8 {
        OUTPUT.with(|output| output.borrow().as_ptr())
    }

    /// The length of the reply of the last `ts_run`.
    #[allow(unsafe_code)]
    #[unsafe(no_mangle)]
    pub extern "C" fn ts_output_len() -> usize {
        OUTPUT.with(|output| output.borrow().len())
    }

    /// wasm cannot unwind, so a panic cannot be caught as `tsgo` catches
    /// it (`bin/tsgo.rs` `install_panic_hook` and `finish`). This prints
    /// what `tsgo` prints for it and ends the run with `tsgo`'s exit
    /// status: 2 for a Go panic, 70 for any other or for unported code.
    fn install_panic_hook() {
        std::panic::set_hook(Box::new(|info| {
            ts_goport::execute::tsc::stdio::flush_cli_stdout_at_exit();
            let _ = std::io::stdout().flush();
            let go_panic = print_go_panic(info.payload());
            if !go_panic {
                let message = info
                    .payload()
                    .downcast_ref::<&str>()
                    .map(|s| (*s).to_string())
                    .or_else(|| info.payload().downcast_ref::<String>().cloned())
                    .unwrap_or_default();
                // `tsgo` counts unported code quietly.
                if !message.starts_with("unported Go code") {
                    let location = info
                        .location()
                        .map(|l| format!(" at {}:{}", l.file(), l.line()))
                        .unwrap_or_default();
                    eprintln!("tsgo: panic{location}: {message}");
                    record_unported("panic");
                }
            }
            let unported = unported_report();
            for (name, count) in &unported {
                eprintln!("unported: {name} {count}");
            }
            let code = if go_panic && unported.is_empty() {
                EXIT_GO_PANIC
            } else {
                EXIT_UNPORTED
            };
            std::process::exit(code);
        }));
    }
}

// Native only: the tests run over `host::test_host`. A wasm build has no
// test host, because its file system is the JavaScript host's.
#[cfg(all(test, not(target_family = "wasm")))]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::PoisonError;

    /// The test host of the process is one state (the files, the OS
    /// override, the parse cache), so the tests take turns.
    static HOST: Mutex<()> = Mutex::new(());

    /// One run over the test host: the reply has the diagnostics as JSON,
    /// and the emit writes to the host.
    #[test]
    fn run_checks_and_emits_through_the_host() {
        let _host = HOST.lock().unwrap_or_else(PoisonError::into_inner);
        host::test_host::set_files([
            (
                "/p/tsconfig.json".to_string(),
                r#"{"compilerOptions":{"outDir":"out","types":[]},"files":["a.ts"]}"#.to_string(),
            ),
            (
                "/p/a.ts".to_string(),
                "export const n: number = 'one';\n".to_string(),
            ),
        ]);
        let (status, reply) = run(&format!("/p\0{FLAG_DIAGNOSTICS_JSON}\0-p\0."));
        assert_eq!(status, 2);
        assert!(
            reply.starts_with(r#"[{"fileName":"/p/a.ts","pos":13,"end":14,"#),
            "{reply}"
        );
        assert!(reply.contains(r#""code":2322"#), "{reply}");
        assert_eq!(
            host::test_host::file("/p/out/a.js").as_deref(),
            Some("export const n = 'one';\n")
        );
    }

    /// PORT: not in Go. The multi-run flow (`program::multirun`): many
    /// runs in this one process (one `WebAssembly.Instance` in the shipped
    /// module), across guests that change between runs. Every run reports
    /// the diagnostics of the files it was given, including a file whose
    /// text changed: the parse cache is keyed by content, so the changed
    /// text is a cache miss and the parse is redone.
    #[test]
    fn many_runs_in_one_process_report_each_guest() {
        const CLEAN_A: &str = "export const a: number = 1;\n";
        const B: &str = "import { a } from './a';\nexport const b: number = a + 1;\n";
        let _host = HOST.lock().unwrap_or_else(PoisonError::into_inner);
        let tsconfig = || {
            (
                "/p/tsconfig.json".to_string(),
                r#"{"compilerOptions":{"outDir":"out","types":[]},"files":["a.ts","b.ts"]}"#
                    .to_string(),
            )
        };
        let a = |text: &str| ("/p/a.ts".to_string(), text.to_string());
        let b = |text: &str| ("/p/b.ts".to_string(), text.to_string());
        let request = || format!("/p\0{FLAG_DIAGNOSTICS_JSON}\0-p\0.");

        // Run 1: a clean guest (cold: no parse cache, no earlier version).
        host::test_host::set_files([tsconfig(), a(CLEAN_A), b(B)]);
        let (status, reply) = run(&request());
        assert_eq!(status, 0, "{reply}");
        assert_eq!(reply, "[]", "{reply}");
        assert_eq!(
            host::test_host::file("/p/out/a.js").as_deref(),
            Some("export const a = 1;\n"),
            "run 1 emit"
        );

        // Run 2: a.ts now has a type error, b.ts unchanged. The changed
        // file must be re-parsed (stale-parse test); the unchanged one may
        // reuse its parse.
        host::test_host::set_files([tsconfig(), a("export const a: number = 'one';\n"), b(B)]);
        let (status, reply) = run(&request());
        assert_eq!(status, 2, "{reply}");
        assert!(reply.contains(r#""code":2322"#), "{reply}");
        assert!(reply.contains(r#""fileName":"/p/a.ts""#), "{reply}");
        assert_eq!(
            host::test_host::file("/p/out/a.js").as_deref(),
            Some("export const a = 'one';\n"),
            "run 2 emit"
        );

        // Run 3: the error is fixed, and the reply is clean again.
        host::test_host::set_files([tsconfig(), a("export const a: number = 2;\n"), b(B)]);
        let (status, reply) = run(&request());
        assert_eq!(status, 0, "{reply}");
        assert_eq!(reply, "[]", "{reply}");

        // Run 4: exactly the files of run 3 again (the warm, unchanged
        // case: every parse comes from the cache).
        let (status, reply) = run(&request());
        assert_eq!(status, 0, "{reply}");
        assert_eq!(reply, "[]", "{reply}");
        assert_eq!(
            host::test_host::file("/p/out/b.js").as_deref(),
            Some("import { a } from './a';\nexport const b = a + 1;\n"),
            "run 4 emit"
        );
    }

    /// An early-return run (`--version`) installs no program version: its
    /// `end_run` must rotate nothing, and a compile after it must still
    /// work on the version chain (the released-as-`previous` bug).
    #[test]
    fn early_return_run_does_not_rotate_versions() {
        let _host = HOST.lock().unwrap_or_else(PoisonError::into_inner);
        let files = || {
            [
                (
                    "/p/tsconfig.json".to_string(),
                    r#"{"compilerOptions":{"outDir":"out","types":[]},"files":["a.ts"]}"#
                        .to_string(),
                ),
                (
                    "/p/a.ts".to_string(),
                    "export const n: number = 1;\n".to_string(),
                ),
            ]
        };
        host::test_host::set_files(files());
        let request = || format!("/p\0{FLAG_DIAGNOSTICS_JSON}\0-p\0.");

        // Run 1: installs version 1; `end_run` retires it.
        let (status, reply) = run(&request());
        assert_eq!(status, 0, "{reply}");

        // Run 2: an early return installs nothing.
        let (status, reply) = run(&["/p", "0", "--version"].join("\0"));
        assert_eq!(status, 0, "{reply}");

        // Run 3: must build on the live version, not a released one.
        host::test_host::set_files(files());
        let (status, reply) = run(&request());
        assert_eq!(status, 0, "{reply}");
        assert_eq!(reply, "[]", "{reply}");
    }

    /// A run whose cwd differs from the one of the instance is rejected
    /// explicitly (`run` documents one cwd per multi-run instance).
    #[test]
    fn cwd_change_is_rejected() {
        let _host = HOST.lock().unwrap_or_else(PoisonError::into_inner);
        host::test_host::set_files([
            (
                "/p/tsconfig.json".to_string(),
                r#"{"compilerOptions":{"types":[]},"files":["a.ts"]}"#.to_string(),
            ),
            (
                "/p/a.ts".to_string(),
                "export const n: number = 1;\n".to_string(),
            ),
        ]);
        let (status, reply) = run(&format!("/p\0{FLAG_DIAGNOSTICS_JSON}\0-p\0."));
        assert_eq!(status, 0, "{reply}");
        let (status, reply) = run(&format!("/q\0{FLAG_DIAGNOSTICS_JSON}\0-p\0."));
        assert_eq!(status, 1, "{reply}");
        assert!(reply.is_empty(), "{reply}");
    }

    /// A run whose case-insensitivity flag differs from the one the
    /// instance installed is rejected explicitly (`run` documents one case
    /// mode per multi-run instance), and the instance keeps serving the
    /// installed mode after the rejected run.
    #[test]
    fn case_flag_change_is_rejected() {
        let _host = HOST.lock().unwrap_or_else(PoisonError::into_inner);
        host::test_host::set_files([
            (
                "/p/tsconfig.json".to_string(),
                r#"{"compilerOptions":{"types":[]},"files":["A.ts"]}"#.to_string(),
            ),
            (
                "/p/A.ts".to_string(),
                "export const n: number = 1;\n".to_string(),
            ),
        ]);
        let request = || format!("/p\0{FLAG_DIAGNOSTICS_JSON}\0-p\0.");
        let (status, reply) = run(&request());
        assert_eq!(status, 0, "{reply}");
        // The other flag: rejected with no reply.
        let (status, reply) = run(&format!(
            "/p\0{}\0-p\0.",
            FLAG_DIAGNOSTICS_JSON | FLAG_CASE_INSENSITIVE
        ));
        assert_eq!(status, 1, "{reply}");
        assert!(reply.is_empty(), "{reply}");
        // The instance still serves the installed mode.
        let (status, reply) = run(&request());
        assert_eq!(status, 0, "{reply}");
        assert_eq!(reply, "[]", "{reply}");
    }

    /// A file that leaves the program frees its parse (`end_run` evicts
    /// the parse cache entries of paths that left): a later run that
    /// brings the path back with other content reports THIS content — a
    /// stale kept parse of the old text must not be served.
    #[test]
    fn a_path_that_returns_with_other_content_rereports() {
        let _host = HOST.lock().unwrap_or_else(PoisonError::into_inner);
        let tsconfig = |files: &str| {
            (
                "/p/tsconfig.json".to_string(),
                format!(r#"{{"compilerOptions":{{"outDir":"out","types":[]}},"files":[{files}]}}"#),
            )
        };
        let clean = "export const b: number = 1;\n";
        let broken = "export const b: number = 'one';\n";
        let a = (
            "/p/a.ts".to_string(),
            "export const a: number = 1;\n".to_string(),
        );
        let b = |text: &str| ("/p/b.ts".to_string(), text.to_string());
        let request = || format!("/p\0{FLAG_DIAGNOSTICS_JSON}\0-p\0.");

        // Run 1: a.ts and b.ts, clean.
        host::test_host::set_files([tsconfig(r#""a.ts","b.ts""#), a.clone(), b(clean)]);
        let (status, reply) = run(&request());
        assert_eq!(status, 0, "{reply}");

        // Run 2: b.ts left the program (its parse is evicted at end_run).
        host::test_host::set_files([tsconfig(r#""a.ts""#), a.clone()]);
        let (status, reply) = run(&request());
        assert_eq!(status, 0, "{reply}");

        // Run 3: b.ts is back with other content: the fresh parse must be
        // checked, and the error must be reported.
        host::test_host::set_files([tsconfig(r#""a.ts","b.ts""#), a.clone(), b(broken)]);
        let (status, reply) = run(&request());
        assert_eq!(status, 2, "{reply}");
        assert!(reply.contains(r#""fileName":"/p/b.ts""#), "{reply}");
        assert!(reply.contains(r#""code":2322"#), "{reply}");
    }

    // E1 + the semantic-diagnostic reuse: the reply of a run whose request
    // bytes are the same as the run before must still be THIS run's — the
    // host file system changed under the identical request, so neither the
    // replay nor the per-file cache may serve the old answer.
    #[test]
    fn identical_request_over_a_changed_guest_rereports() {
        let _host = HOST.lock().unwrap_or_else(PoisonError::into_inner);
        let tsconfig = || {
            (
                "/p/tsconfig.json".to_string(),
                r#"{"compilerOptions":{"outDir":"out","types":[]},"files":["a.ts"]}"#.to_string(),
            )
        };
        let red = "const n: number = 'not a number';\nconsole.log(n);\n";
        host::test_host::set_files([tsconfig(), ("/p/a.ts".to_string(), red.to_string())]);
        let request = || format!("/p\0{FLAG_DIAGNOSTICS_JSON}\0-p\0.");
        let (status, reply) = run(&request());
        assert_eq!(status, 2, "{reply}");
        assert!(reply.contains(r#""code":2322"#), "{reply}");
        // Same request bytes, new text: the world hash changes with the
        // content, so the replay must miss and the fresh check must report
        // clean.
        host::test_host::set_files([
            tsconfig(),
            (
                "/p/a.ts".to_string(),
                "export const a: number = 1;\n".to_string(),
            ),
        ]);
        let (status, reply) = run(&request());
        assert_eq!(status, 0, "{reply}");
        assert_eq!(reply, "[]", "{reply}");
        // And back to red (the parse and the cached per-file lists are for
        // the old parse): the fresh check must report the error again.
        host::test_host::set_files([tsconfig(), ("/p/a.ts".to_string(), red.to_string())]);
        let (status, reply) = run(&request());
        assert_eq!(status, 2, "{reply}");
        assert!(reply.contains(r#""code":2322"#), "{reply}");
    }

    // A script guest whose top-level shape is the same (the same annotated
    // function, a new body) re-checks only itself; a script whose top-level
    // shape changed re-checks everything. Both replies must be the fresh
    // check's.
    #[test]
    fn script_body_change_rereports_and_shape_change_rechecks() {
        const HEAD: &str = "function slugify(input: string): string {\n";
        const FOOT: &str = "}\ntest('a', () => { expect(slugify('A B')).toEqual('a-b'); });\n";
        const GLOBALS: &str = "declare function test(name: string, fn: () => void): void;\ndeclare function expect(actual: unknown): { toEqual(expected: unknown): void; };\n";
        let _host = HOST.lock().unwrap_or_else(PoisonError::into_inner);
        let tsconfig = || {
            (
                "/p/tsconfig.json".to_string(),
                r#"{"compilerOptions":{"outDir":"out","types":[]},"files":["a.ts","globals.d.ts"]}"#.to_string(),
            )
        };
        let broken =
            format!("{HEAD}  const slug: number = input.toLowerCase();\n  return slug;\n{FOOT}");
        let fixed =
            format!("{HEAD}  return input.trim().toLowerCase().replace(/\\s+/g, \"-\");\n{FOOT}");
        host::test_host::set_files([
            tsconfig(),
            ("/p/globals.d.ts".to_string(), GLOBALS.to_string()),
            ("/p/a.ts".to_string(), broken),
        ]);
        let request = || format!("/p\0{FLAG_DIAGNOSTICS_JSON}\0-p\0.");
        let (status, reply) = run(&request());
        assert_eq!(status, 2, "{reply}");
        assert!(reply.contains(r#""fileName":"/p/a.ts""#), "{reply}");
        // The body edit keeps the signature: the guest is re-checked and
        // the reply is the fixed one.
        host::test_host::set_files([
            tsconfig(),
            ("/p/globals.d.ts".to_string(), GLOBALS.to_string()),
            ("/p/a.ts".to_string(), fixed),
        ]);
        let (status, reply) = run(&request());
        assert_eq!(status, 0, "{reply}");
        assert_eq!(reply, "[]", "{reply}");
        // A new top-level declaration changes the global scope: the whole
        // program re-checks, and the duplicate it makes is reported on both
        // sides that the full check reports (here: the globals side keeps
        // its own diagnostic).
        host::test_host::set_files([
            tsconfig(),
            ("/p/globals.d.ts".to_string(), GLOBALS.to_string()),
            (
                "/p/a.ts".to_string(),
                "const test = 5;\nconsole.log(test);\n".to_string(),
            ),
        ]);
        let (status, _reply) = run(&request());
        assert_eq!(status, 2);
    }
}
