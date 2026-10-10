//! Faithful Rust port of the pinned typescript-go binder and checker.
//! Read `crates/ts_goport/PORTING.md` before editing.
//!
//! Two parts of `src` build as their own crates (`parts/`): `goport_util`
//! (`src/goport_util_root.rs`) and `goport_lsproto`
//! (`src/goport_lsproto_root.rs`). Their root files declare their modules;
//! this crate re-exports them at the same paths.

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

pub mod ast;
pub use goport_util::astdata;
pub mod baseline;
pub mod binder;
pub mod checker;
pub mod cmd;
// Go `internal/contentmapper` and `internal/spanmap` (tsgo#4712).
pub mod contentmapper;
pub mod core;
pub mod declarations;
// Native Effect diagnostics (Effect-TS/tsgo port).
pub mod effect;
pub use goport_util::diag;
pub use goport_util::diagnostics;
pub mod diagnostics_loc;
pub mod emitter;
pub mod evaluator;
pub mod execute;
pub mod flags;
/// The leaked bump arena of a thread (AST and checker arenas).
pub mod leak_arena;
/// `go_enum!` and `go_flags!` (`#[macro_export]` in goport_util).
mod flags_macros {
    pub(crate) use goport_util::{go_enum, go_flags};
}
pub mod frontend;
// Go `internal/ipc` (tsgo#4712).
pub mod ipc;
// The jemalloc arena layout at start: Linux only (THP).
#[cfg(all(feature = "jemalloc", target_os = "linux"))]
pub mod jemalloc_layout;
pub use goport_util::jsnum;
pub use goport_util::locale;
pub mod modulespecifiers;
pub mod options;
// The JSON form of `core.CompilerOptions` (moved out of `api` in bump B wave 3).
pub mod options_json;
pub mod pprof;
pub mod prelude;
pub mod printer;
pub mod program;
pub mod pseudochecker;
pub mod scanner_util;
pub mod sourcemap;
pub mod spanmap;
pub mod thp_guard;
pub mod tracing;
pub mod transformers;
pub mod transpile;

// Macros that goport_util exports. `crate::go_assert` and `crate::unported`
// keep working.
pub use goport_util::{go_assert, unported};

// Language-service port (Go `internal/{ls,lsp,project,format,astnav,fswatch,jsonrpc,api}`).
pub mod api;
pub mod astnav;
pub mod format;
pub use goport_util::{fswatch, gostd, jsonrpc};
pub mod ls;
pub mod lsp;
pub mod project;
