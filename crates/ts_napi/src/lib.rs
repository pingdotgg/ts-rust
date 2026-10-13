//! The Node-API addon of `npm/typescript-api`.
//!
//! - `NativeChannel` is one session of tsc-rs's API server (the server
//!   behind `tsgo --api`). The sessions of the process run on one thread
//!   with the Go stack size (`session`), and each speaks the msgpack sync
//!   protocol over two in-memory pipes (`pipe`, `frame`). The JS thread
//!   blocks in a request and answers the session's callbacks inline
//!   (`channel`), as TS7's `SyncRpcChannel` does with a child process.
//! - `runTsc` runs the tsc command line in the process (`tsc`).
//!
//! The two are exclusive per process (`hooks`): a plain tsc run keeps one
//! program per process, and API sessions are multi-program.

mod channel;
mod exports;
mod frame;
mod hooks;
mod pipe;
mod session;
mod tsc;
