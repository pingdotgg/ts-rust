//! Crate root of `goport_core`: the port below the language service (binder,
//! checker, parser, emitter, `execute`, `effect`, ...). It is everything of
//! `ts_goport` but `api`, `cmd`, `format`, `ls`, `lsp` and `project`, which
//! stay in `ts_goport` (crate split step 2). An edit on either side rebuilds
//! only that side and the crates above it.
//! Its files stay in `crates/ts_goport/src` and keep their module paths.
//! `ts_goport` re-exports each module at its old path, so `crate::` paths in
//! these files and `ts_goport::` paths outside do not change.

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
compile_error!("build goport_core for wasm without its jemalloc feature, as crates/ts_wasm does");

pub mod ast;
pub use goport_util::astdata;
pub mod baseline;
pub mod binder;
pub mod checker;
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
/// The opt-in fast mode (`--fast`, `GOPORT_FAST=1`). Not in Go.
pub mod fast_mode;
pub mod flags;
/// The leaked bump arena of a thread (AST and checker arenas).
pub mod leak_arena;
/// loadstamp1 probe: load stamps (`GOPORT_LOADSTAMP`).
pub mod loadstamp;
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
/// Test builds only: one test thread at a time builds node stores.
#[cfg(test)]
mod test_lock;
pub mod thp_guard;
pub mod tracing;
pub mod transformers;
pub mod transpile;

// Macros that goport_util exports. `crate::go_assert` and `crate::unported`
// keep working.
pub use goport_util::{go_assert, unported};

pub mod astnav;
pub use goport_util::{fswatch, gostd, jsonrpc};
