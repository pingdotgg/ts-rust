//! Go package tree `internal/lsp`.

pub mod dynamic_queue;
pub mod logger;
pub use goport_lsproto::lsp::lsproto;
pub mod lspwatcher;
pub mod progress;
pub mod run_end;
pub mod server;
pub mod stack_sanitizer;

pub use dynamic_queue::*;
pub use logger::*;
pub use progress::*;
pub use server::*;
pub use stack_sanitizer::*;

/// Glob import for lsp files: `use crate::lsp::prelude::*;`.
pub mod prelude {
    pub use super::{dynamic_queue::*, logger::*, progress::*, server::*, stack_sanitizer::*};
    pub use crate::api;
    pub use crate::frontend::bundled;
    pub use crate::frontend::json_ext::{self, LspAny};
    pub use crate::frontend::vfs::osvfs;
    pub use crate::frontend::{tspath, vfs};
    pub use crate::fswatch;
    pub use crate::gostd::{self, Context, GoError};
    pub use crate::jsonrpc;
    pub use crate::locale;
    pub use crate::ls::{self, lsconv, lsutil};
    pub use crate::lsp::{lsproto, lspwatcher};
    pub use crate::prelude::*;
    pub use crate::project::{self, ata, logging};
}
