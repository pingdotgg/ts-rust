//! Process-wide state of the addon: the panic hook and the program mode.

use std::any::Any;
use std::io::Write;
use std::sync::{Mutex, Once, PoisonError};

use ts_goport::core::{GoPanic, in_go_recover};

pub const UNPORTED_PREFIX: &str = "unported Go code";

/// How this process uses tsc-rs. A plain tsc run keeps one program per
/// process (`core::set_prog`), and an API session registers program
/// versions (`core::register_program_version`); `ts_goport` asserts that a
/// process does only one of the two.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Unused,
    Api,
    Tsc,
}

static MODE: Mutex<Mode> = Mutex::new(Mode::Unused);

/// Claims API sessions for this process.
pub fn claim_api() -> Result<(), String> {
    install_panic_hook();
    let mut mode = MODE.lock().unwrap_or_else(PoisonError::into_inner);
    if *mode == Mode::Tsc {
        return Err(
            "this process ran runTsc, and tsc-rs runs one program per process there: it cannot open an API session too"
                .to_string(),
        );
    }
    *mode = Mode::Api;
    Ok(())
}

/// Claims the one tsc run of this process.
pub fn claim_tsc() -> Result<(), String> {
    install_panic_hook();
    let mut mode = MODE.lock().unwrap_or_else(PoisonError::into_inner);
    match *mode {
        Mode::Api => Err(
            "this process opened a tsc-rs API session, so it cannot run tsc (runTsc) too: run it in a child process"
                .to_string(),
        ),
        Mode::Tsc => Err(
            "runTsc already ran in this process: tsc-rs runs one tsc program per process".to_string(),
        ),
        Mode::Unused => {
            *mode = Mode::Tsc;
            Ok(())
        }
    }
}

/// `bin/tsgo.rs` `install_panic_hook`: quiet for Go panics, unported code
/// and panics that a request recovers (the API answers those); others print
/// one line. It covers the code of this library only.
fn install_panic_hook() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        std::panic::set_hook(Box::new(|info| {
            if info.payload().is::<GoPanic>() {
                return;
            }
            let message = payload_message(info.payload());
            let trace = std::env::var_os("GOPORT_TRACE").is_some();
            if message.starts_with(UNPORTED_PREFIX) || in_go_recover() {
                if trace {
                    let _ = writeln!(
                        std::io::stderr(),
                        "trace: {message}\n{}",
                        std::backtrace::Backtrace::force_capture()
                    );
                }
                return;
            }
            let location = info
                .location()
                .map(|l| format!(" at {}:{}", l.file(), l.line()))
                .unwrap_or_default();
            let _ = writeln!(std::io::stderr(), "tsc-rs: panic{location}: {message}");
            if trace {
                let _ = writeln!(
                    std::io::stderr(),
                    "{}",
                    std::backtrace::Backtrace::force_capture()
                );
            }
        }));
    });
}

pub fn payload_message(payload: &(dyn Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else if let Some(panic) = payload.downcast_ref::<GoPanic>() {
        panic.message.clone()
    } else {
        String::new()
    }
}
