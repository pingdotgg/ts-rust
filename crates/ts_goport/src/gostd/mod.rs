//! Go runtime and standard library pieces used by the language-service port
//! (`context`, `errors`, `slices`, `net/url`, `net/netip`,
//! `strconv`, `unicode`, `regexp` with its `unicode` tables,
//! `golang.org/x/text/collate` and `unicode/norm`, the dispatch-thread `go`
//! queue, the goroutine stack size, and the open-file limit),
//! and the `internal/debug` checks. See PORTING.md "Go runtime".

pub mod collate;
pub mod context;
pub mod debug;
pub mod errors;
pub mod local;
pub mod netip;
pub mod norm;
pub mod regexp;
pub mod rlimit;
pub mod slices;
pub mod stack;
pub mod strconv;
pub mod unicode;
pub mod unicode_tables;
pub mod url;

pub use context::Context;
pub use errors::GoError;
