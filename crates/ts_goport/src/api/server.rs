//! Port of internal/api/server.go.

use crate::api::prelude::*;

use crate::api::callbackfs::{CallbackFS, new_callback_fs};
use crate::api::protocol_msgpack::new_message_pack_protocol;
use crate::contentmapper;
use crate::frontend::bundled;
use crate::frontend::json_ext::{AnyValue, JsonValue};
use crate::frontend::vfs::{self, Fs};
use crate::gostd::{Context, GoError, errors};
use crate::ipc::{
    Conn, Handler, Transport, new_async_conn_with_protocol, new_jsonrpc_protocol,
    new_pipe_transport, new_stdio_transport, new_sync_conn,
};
use crate::lsp::lsproto;
use crate::project;
use std::cell::Cell;
use std::io::{Read, Write};
use std::time::Duration;

// Go: server.go:18 StdioServerOptions
// StdioServerOptions configures the STDIO-based API server.
// PORT: Go `io.ReadCloser` / `io.WriteCloser` / `io.Writer` are boxed
// `Read` / `Write` values; a nil one is `None`. `In` and `Async` are Rust
// keywords, so the fields are `in_` and `async_`. The Go
// `contentmapper.Spawner` interface is `Rc<dyn contentmapper::Spawner>`,
// and a nil one is `None` (tsgo#4712).
#[derive(Default)]
pub struct StdioServerOptions {
    pub in_: Option<Box<dyn Read + Send>>,
    pub out: Option<Box<dyn Write + Send>>,
    pub err: Option<Box<dyn Write + Send>>,
    pub cwd: String,
    pub default_library_path: String,
    // PipePath, if set, listens on a named pipe (Windows) or Unix domain
    // socket instead of using In/Out for communication.
    pub pipe_path: String,
    // Callbacks specifies which filesystem operations should be delegated
    // to the client (e.g., "readFile", "fileExists"). Empty means no callbacks.
    pub callbacks: Vec<String>,
    // UseCaseSensitiveFileNames overrides the base filesystem's case sensitivity.
    // PORT: the Go `*bool` is `Option<bool>` (ts#64447).
    pub use_case_sensitive_file_names: Option<bool>,
    // Async enables JSON-RPC protocol with async connection handling.
    // When false (default), uses MessagePack protocol with sync connection.
    pub async_: bool,
    // CollectTiming enables per-request server processing-time measurement.
    // When enabled, the server accumulates each request's processing time into
    // running totals and a recent-request ring buffer. Response messages are
    // left unchanged; the client folds this data into its own timing snapshot
    // on demand via getServerTiming / resetServerTiming requests.
    pub collect_timing: bool,
    // RunExternalCode allows configured content mappers to execute.
    pub run_external_code: bool,
    pub content_mapper_spawner: Option<Rc<dyn contentmapper::Spawner>>,
}

// Go: server.go:49 StdioServer
// StdioServer runs an API session over STDIO using MessagePack protocol.
// This is the entry point for the synchronous STDIO-based API used by
// native TypeScript tooling integration.
pub struct StdioServer {
    options: StdioServerOptions,
}

// Go: server.go:54 NewStdioServer
// NewStdioServer creates a new STDIO-based API server.
// PORT: Go keeps the `*StdioServerOptions` pointer; the server owns the
// options here (they hold the stdin and stdout handles).
pub fn new_stdio_server(options: StdioServerOptions) -> StdioServer {
    if options.cwd.is_empty() {
        crate::core::go_panic("StdioServerOptions.Cwd is required".to_string());
    }

    StdioServer { options }
}

/// Not in Go: the handler of the stdio connection. It runs the frees that
/// its messages leave (`gostd::local::drop_later`: released programs, the
/// pin release of a released source file lease) when a message that the
/// connection read at the top level ends. A request that a client makes
/// while it answers a callback (ts#64299) runs inside the request that made
/// the callback, so its frees wait for the end of that request. Go's GC
/// frees in the background.
struct FreeAfterMessage {
    /// The session.
    inner: Rc<dyn Handler>,
    /// The messages that run now, the outer one first.
    depth: Cell<u32>,
}

impl FreeAfterMessage {
    /// Counts one more running message until the guard drops, also on a
    /// panic. The guard of the top-level message runs the frees.
    fn enter(&self) -> Defer<impl FnMut() + '_> {
        self.depth.set(self.depth.get() + 1);
        Defer(move || {
            self.depth.set(self.depth.get() - 1);
            if self.depth.get() == 0 {
                crate::gostd::local::drop_garbage(|| false);
            }
        })
    }
}

impl Handler for FreeAfterMessage {
    fn handle_request(
        &self,
        ctx: &Context,
        method: &str,
        params: JsonValue,
    ) -> Result<Option<Box<dyn AnyValue>>, GoError> {
        let _message = self.enter();
        self.inner.handle_request(ctx, method, params)
    }

    fn handle_notification(
        &self,
        ctx: &Context,
        method: &str,
        params: JsonValue,
    ) -> Result<(), GoError> {
        let _message = self.enter();
        self.inner.handle_notification(ctx, method, params)
    }
}

// Go `defer f()`: runs `f` when the guard leaves scope, on every return
// path and during a panic.
struct Defer<F: FnMut()>(F);

impl<F: FnMut()> Drop for Defer<F> {
    fn drop(&mut self) {
        (self.0)();
    }
}

impl StdioServer {
    // Go: server.go:65 Run
    // Run starts the server and blocks until the connection closes.
    // PORT: `&mut self` because Accept moves the stdin and stdout handles
    // out of the options.
    pub fn run(&mut self, ctx: &Context) -> Result<(), GoError> {
        let transport: Box<dyn Transport> = if !self.options.pipe_path.is_empty() {
            let t = match new_pipe_transport(&self.options.pipe_path) {
                Ok(t) => t,
                Err(err) => {
                    return Err(errors::errorf(
                        format!("failed to create pipe transport: {}", err.error()),
                        vec![err],
                    ));
                }
            };
            Box::new(t)
        } else {
            let t = new_stdio_transport(self.options.in_.take(), self.options.out.take());
            Box::new(t)
        };
        // defer t.Close()
        let transport = RefCell::new(transport);
        let _close_transport = Defer(|| {
            let _ = transport.borrow_mut().close();
        });

        let mut fs: Rc<dyn Fs> = bundled::wrap_fs_exported(vfs::osvfs_fs());

        // Wrap the base FS when callbacks or an explicit case-sensitivity setting are requested.
        // ts#64447
        let mut callback_fs: Option<Rc<CallbackFS>> = None;
        if !self.options.callbacks.is_empty()
            || self.options.use_case_sensitive_file_names.is_some()
        {
            let cfs = new_callback_fs(
                fs.clone(),
                &self.options.callbacks,
                self.options.use_case_sensitive_file_names,
            );
            fs = cfs.clone();
            callback_fs = Some(cfs);
        }

        // ts#64163
        let session_init = project::SessionInit {
            background_ctx: ctx.clone(),
            logger: None, // TODO: Add logging support
            fs,
            options: Rc::new(project::SessionOptions {
                current_directory: self.options.cwd.clone(),
                default_library_path: self.options.default_library_path.clone(),
                position_encoding: lsproto::PositionEncodingKind::UTF8,
                logging_enabled: false,
                run_external_code: self.options.run_external_code,
                // PORT: Go leaves the other fields at their zero values.
                typings_location: String::new(),
                watch_enabled: false,
                telemetry_enabled: false,
                push_diagnostics_enabled: false,
                debounce_delay: Duration::ZERO,
                checker_pool_options: project::CheckerPoolOptions::default(),
            }),
            spawner: self.options.content_mapper_spawner.clone(),
            // PORT: Go leaves the other fields at their zero values.
            client: None,
            npm_executor: None,
            content_mapper_logger: None,
            parse_cache: None,
            content_mapped_parse_cache: None,
        };

        let session = new_standalone_session(
            &session_init,
            Some(&SessionOptions {
                use_binary_responses: !self.options.async_, // Only msgpack uses binary responses
            }),
        );
        // defer session.Close()
        let _close_session = Defer(|| {
            session.close();
        });

        // Accept connection from transport
        let accepted = transport.borrow_mut().accept();
        let rwc = match accepted {
            Ok(rwc) => rwc,
            Err(err) => {
                return Err(errors::errorf(
                    format!("failed to accept connection: {}", err.error()),
                    vec![err],
                ));
            }
        };

        // Create protocol and connection based on async mode
        // PORT: the frees of a message wait until it ends (`FreeAfterMessage`).
        // A source file version that a lease release frees dies in the
        // release. Only the data of a version of 10,000 or more nodes goes
        // to the free thread (`free_released_versions_in_background`; perf,
        // apiperf1 and followups30).
        crate::gostd::local::keep_garbage();
        crate::ast::free_released_versions_in_background();
        let handler: Rc<dyn Handler> = Rc::new(FreeAfterMessage {
            inner: session.clone(),
            depth: Cell::new(0),
        });
        let conn: Rc<dyn Conn>;
        if self.options.async_ {
            let protocol = new_jsonrpc_protocol(rwc.clone());
            let async_conn = new_async_conn_with_protocol(rwc, Box::new(protocol), handler);
            async_conn.set_collect_timing(self.options.collect_timing);
            conn = async_conn;
        } else {
            let protocol = new_message_pack_protocol(rwc.clone());
            let sync_conn = new_sync_conn(rwc, Box::new(protocol), handler);
            sync_conn.set_collect_timing(self.options.collect_timing);
            conn = sync_conn;
        }

        // If callbacks are enabled, set the connection on the FS
        if let Some(callback_fs) = &callback_fs {
            callback_fs.set_connection(ctx, conn.clone());
        }
        // ts#64299
        session.set_connection(conn.clone());

        // ts#64276
        server_run_error(ctx, conn.run(ctx))
    }
}

// Go: server.go serverRunError (ts#64276)
fn server_run_error(ctx: &Context, err: Result<(), GoError>) -> Result<(), GoError> {
    if ctx.err().is_some() {
        return Ok(());
    }
    err
}

// Go: api/server_test.go (ts#64276)
#[cfg(test)]
mod server_tests {
    use super::*;
    use crate::gostd::context;

    // PORT: no Go counterpart. A free that a nested message leaves (a
    // request that a client makes while it answers a callback, ts#64299)
    // waits for the end of the top-level message.
    #[test]
    fn frees_wait_for_the_end_of_the_top_level_message() {
        struct Answers;
        impl Handler for Answers {
            fn handle_request(
                &self,
                _ctx: &Context,
                _method: &str,
                _params: JsonValue,
            ) -> Result<Option<Box<dyn AnyValue>>, GoError> {
                Ok(None)
            }
            fn handle_notification(
                &self,
                _ctx: &Context,
                _method: &str,
                _params: JsonValue,
            ) -> Result<(), GoError> {
                Ok(())
            }
        }
        struct Freed(Rc<Cell<bool>>);
        impl Drop for Freed {
            fn drop(&mut self) {
                self.0.set(true);
            }
        }
        crate::gostd::local::keep_garbage();
        let handler = FreeAfterMessage {
            inner: Rc::new(Answers),
            depth: Cell::new(0),
        };
        let freed = Rc::new(Cell::new(false));
        {
            let _outer = handler.enter();
            {
                let _nested = handler.enter();
                crate::gostd::local::drop_later(Box::new(Freed(freed.clone())));
            }
            assert!(!freed.get(), "a nested message keeps its frees");
        }
        assert!(freed.get(), "the top-level message runs them at its end");
        let freed = Rc::new(Cell::new(false));
        crate::gostd::local::drop_later(Box::new(Freed(freed.clone())));
        assert!(
            handler
                .handle_notification(&context::background(), "x", JsonValue::default())
                .is_ok()
        );
        assert!(freed.get(), "a message runs the frees that wait");
    }

    // Go: api/server_test.go:14 TestServerRunError/EOF
    #[test]
    fn test_server_run_error_eof() {
        assert!(server_run_error(&context::background(), Ok(())).is_ok());
    }

    // Go: api/server_test.go:19 TestServerRunError/context cancellation
    #[test]
    fn test_server_run_error_context_cancellation() {
        let (ctx, cancel) = context::with_cancel(&context::background());
        cancel();
        assert!(server_run_error(&ctx, Err(context::CANCELED.clone())).is_ok());
    }

    // Go: api/server_test.go:26 TestServerRunError/unrelated cancellation
    #[test]
    fn test_server_run_error_unrelated_cancellation() {
        let err = context::CANCELED.clone();
        let result = server_run_error(&context::background(), Err(err.clone()));
        assert!(result.is_err_and(|result| errors::is(&result, &err)));
    }

    // Go: api/server_test.go:32 TestServerRunError/context cancellation supersedes server error
    #[test]
    fn test_server_run_error_context_cancellation_supersedes_server_error() {
        let (ctx, cancel) = context::with_cancel(&context::background());
        cancel();
        assert!(server_run_error(&ctx, Err(errors::new("server failed"))).is_ok());
    }
}
