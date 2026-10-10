//! The wasm build of ts-rust: `tsc` (`ts_goport`) over a file system that the
//! JavaScript host provides (`host`). `npm/wasm` is the package that loads
//! it, and `scripts/wasm/build.sh` builds it for `wasm32-wasip1`.
//!
//! The host writes a request to the input buffer (`ts_input`), calls
//! `ts_run`, and reads the reply at `ts_output` (`ts_output_len` bytes).
//! One instance runs one request, because the port keeps one program per
//! process (`core::set_prog`). The host makes a new instance for each.
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

pub mod host;
#[cfg(target_family = "wasm")]
mod language_service;

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

    let _ = host::CASE_SENSITIVE.set(flags & FLAG_CASE_INSENSITIVE == 0);
    install_os_override(OsOverride {
        fs: Arc::new(|| Rc::new(host::HostFs::new()) as Rc<dyn Fs>),
        current_directory: cwd,
    });
    ts_goport::execute::tsc::stdio::init();
    let sys: Rc<dyn System> = match new_os_system() {
        Ok(sys) => Rc::new(sys),
        Err(status) => return (status.code(), String::new()),
    };

    let diagnostics = (flags & FLAG_DIAGNOSTICS_JSON != 0).then(Rc::default);
    let hooks = WasmTsc {
        diagnostics: diagnostics.clone(),
    };
    let status = command_line(&context::background(), sys.clone(), &args, &hooks)
        .status
        .code();
    let _ = sys.writer().borrow_mut().flush();
    let reply = diagnostics.map_or_else(String::new, |diagnostics| {
        format!("[{}]", diagnostics.take().join(","))
    });
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
        static SERVICE: RefCell<Option<super::language_service::Service>> = const { RefCell::new(None) };
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

    /// Handles one editor request in this instance's persistent language service.
    #[allow(unsafe_code)]
    #[unsafe(no_mangle)]
    pub extern "C" fn ts_service() {
        install_panic_hook();
        let request = INPUT.with(|input| std::mem::take(&mut *input.borrow_mut()));
        let reply = SERVICE
            .with(|service| super::language_service::receive(&mut service.borrow_mut(), &request));
        OUTPUT.with(|output| *output.borrow_mut() = reply.into_bytes());
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

// Native only: the test runs over `host::test_host`. A wasm build has no
// test host, because its file system is the JavaScript host's.
#[cfg(all(test, not(target_family = "wasm")))]
mod tests {
    use super::*;

    /// One run over the test host: the reply has the diagnostics as JSON,
    /// and the emit writes to the host. A process runs one request, so this
    /// is the crate's only test of `run`.
    #[test]
    fn run_checks_and_emits_through_the_host() {
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
}
