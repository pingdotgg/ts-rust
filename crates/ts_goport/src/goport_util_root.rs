//! Crate root of `goport_util`: the bottom part of the goport port (Go std
//! pieces, `tspath`, `vfs`, `json`, `fswatch`, `jsonrpc`, `diag`, `locale`,
//! and the syntax model `astdata`, the message catalog `diagnostics` and
//! `jsnum`, which were the crates `ts_ast`, `ts_core`, `ts_diagnostics` and
//! `ts_jsnum`).
//! Its files stay in `crates/ts_goport/src` and keep their module paths.
//! `ts_goport` re-exports each module at its old path, so `crate::` paths in
//! these files and `ts_goport::` paths outside do not change.
//!
//! Two shim modules keep the old paths of the items that moved here:
//! `core` (`gopanic.rs`: `GoPanic`, `go_panic`, `unported!`) and
//! `scanner_util` (`gostring.rs`: the Go string helpers). The upper
//! `core.rs` and `scanner_util.rs` re-export them.

#![allow(
    dead_code,
    unused_imports,
    unused_variables,
    unused_mut,
    clippy::pedantic,
    clippy::all,
    non_snake_case
)]

pub mod astdata;
pub mod backtrace;
#[path = "gopanic.rs"]
pub mod core;
pub mod diag;
pub mod diagnostics;
mod flags_macros;
pub mod fswatch;
pub mod gostd;
pub mod jsnum;
pub mod jsonrpc;
pub mod locale;
#[path = "util_prelude.rs"]
pub mod prelude;
#[path = "gostring.rs"]
pub mod scanner_util;

/// The util part of `crate::frontend`. `ts_goport`'s `frontend/mod.rs`
/// re-exports each module.
pub mod frontend {
    pub mod bundled;
    pub mod core_bfs;
    pub mod core_binarysearch;
    pub mod core_context;
    pub mod core_nodemodules;
    pub mod core_workgroup;
    pub mod json;
    pub mod json_ext;
    pub mod json_indexmap;
    pub mod nativepath;
    pub mod osutil;
    #[path = "util_prelude.rs"]
    pub mod prelude;
    pub mod semver;
    pub mod stringutil_ls;
    pub mod tspath;
    pub mod vfs;
}
