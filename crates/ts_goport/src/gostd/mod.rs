//! Go runtime and standard library pieces used by the language-service port
//! (`context`, `errors`, `errgroup`, `slices`, `net/url`, `net/netip`,
//! `strconv`, `time` timers, `unicode`, `regexp` with its `unicode` tables,
//! `golang.org/x/text/collate` and `unicode/norm`, the dispatch-thread `go`
//! queue, the goroutine stack size, `GOMAXPROCS` and the open-file limit),
//! the `internal/debug` checks and the backtrace text that takes the place
//! of a goroutine stack (`backtrace`). See PORTING.md "Go runtime".

pub mod backtrace;
pub mod collate;
pub mod context;
pub mod debug;
pub mod errgroup;
pub mod errors;
pub mod local;
pub mod netip;
pub mod norm;
pub mod regexp;
pub mod rlimit;
pub mod runtime;
pub mod slices;
pub mod stack;
pub mod strconv;
pub mod timer;
pub mod unicode;
pub mod unicode_tables;
pub mod url;

pub use context::Context;
pub use errors::GoError;
