//! Faithful Rust port of the pinned typescript-go binder and checker.
//! Read `crates/ts_goport/PORTING.md` before editing.
//!
//! Three parts of `src` build as their own crates (`parts/`): `goport_util`
//! (`src/goport_util_root.rs`), `goport_lsproto`
//! (`src/goport_lsproto_root.rs`) and `goport_core`
//! (`src/goport_core_root.rs`: everything below the language service). Their
//! root files declare their modules; this crate re-exports them at the same
//! paths. This crate keeps the language service and the command line: `api`,
//! `cmd`, `format`, `ls`, `lsp` and `project`.

#![allow(
    dead_code,
    unused_imports,
    unused_variables,
    unused_mut,
    clippy::pedantic,
    clippy::all,
    non_snake_case
)]

// jemalloc does not build for wasm (see Cargo.toml).
#[cfg(all(feature = "jemalloc", target_family = "wasm"))]
compile_error!("build ts_goport for wasm with --no-default-features, as crates/ts_wasm does");

// The port below the language service (crate split step 2): each module keeps its path.
pub use goport_core::{
    ast, astdata, astnav, backtrace, baseline, binder, checker, contentmapper, core, declarations,
    diag, diagnostics, diagnostics_loc, emitter, evaluator, execute, fast_mode, flags, frontend,
    fswatch, gostd, ipc, jsnum, jsonrpc, leak_arena, locale, modulespecifiers, options,
    options_json, pprof, prelude, printer, program, pseudochecker, scanner_util, sourcemap,
    spanmap, thp_guard, tracing, transformers, transpile,
};
// The jemalloc arena layout at start: Linux only (THP).
#[cfg(all(feature = "jemalloc", target_os = "linux"))]
pub use goport_core::jemalloc_layout;
/// `go_enum!` and `go_flags!` (`#[macro_export]` in goport_util).
mod flags_macros {
    pub(crate) use goport_util::{go_enum, go_flags};
}
// Macros that goport_util exports. `crate::go_assert` and `crate::unported`
// keep working.
pub use goport_util::{go_assert, unported};
// Macros that goport_core exports (`#[macro_export]`), at their
// `ts_goport::` paths from before crate split step 2.
pub use goport_core::{args, effect_cached, effect_export_member};

// Language-service port (Go `internal/{ls,lsp,project,format,api}`) and the command line.
pub mod api;
pub mod cmd;
pub mod format;
pub mod ls;
pub mod lsp;
pub mod project;

/// Native Effect diagnostics (`goport_core::effect`) and Effect's language
/// service hooks. `effect::ls` (src/effect/ls) uses `ls`, so it builds in
/// this crate, at its old path.
pub mod effect {
    pub use goport_core::effect::*;
    pub mod ls;
}
