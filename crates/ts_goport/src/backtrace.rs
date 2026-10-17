//! The stack text of panic logs, panic replies and the bins' GOPORT_TRACE
//! output (Go `debug.Stack()`). Every caller gets it here.
//!
//! With feature `backtrace` (on by default), `force_capture` and `captured`
//! are `std::backtrace::Backtrace` as text. Off (lust dev builds,
//! `.lust.toml`), `force_capture` gives `NOT_CAPTURED` and `captured` gives
//! `None`: a backtrace's text depends on the compiler's
//! symbols, so a lust build cannot give the same text as a Cargo build.

/// The text that `force_capture` gives without the feature.
pub const NOT_CAPTURED: &str = "backtrace not captured (build with --features backtrace)";

/// `Backtrace::force_capture()` as text: the frames, whatever the
/// environment says.
#[cfg(feature = "backtrace")]
pub fn force_capture() -> String {
    std::backtrace::Backtrace::force_capture().to_string()
}

/// `Backtrace::capture()` as text when RUST_BACKTRACE or RUST_LIB_BACKTRACE
/// asks for frames (status `Captured`), else `None`.
#[cfg(feature = "backtrace")]
pub fn captured() -> Option<String> {
    let backtrace = std::backtrace::Backtrace::capture();
    (backtrace.status() == std::backtrace::BacktraceStatus::Captured).then(|| backtrace.to_string())
}

#[cfg(not(feature = "backtrace"))]
pub fn force_capture() -> String {
    NOT_CAPTURED.to_owned()
}

#[cfg(not(feature = "backtrace"))]
pub fn captured() -> Option<String> {
    None
}
