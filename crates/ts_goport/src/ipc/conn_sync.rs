//! Port of internal/ipc/conn_sync.go (internal/api/conn_sync.go before tsgo#4712).

use crate::ipc::prelude::*;

use crate::core::{go_after_recover, go_recover};
use crate::frontend::json_ext::{AnyValue, JsonValue};
use crate::gostd::{Context, GoError, errors, strconv};
use crate::ipc::conn::{Conn, Handler, recovered_value};
use crate::ipc::protocol::{Message, Protocol};
use crate::ipc::transport::ReadWriteCloser;
use crate::jsonrpc;
use std::sync::Arc;
use std::time::Instant;

// Go: ipc/conn_sync.go:18 SyncConn
// SyncConn manages bidirectional communication with synchronous request handling.
// Requests are handled one at a time inline, and outgoing calls are serialized.
pub struct SyncConn {
    rwc: Arc<dyn ReadWriteCloser>,
    // PORT: Go `mu` serializes all protocol operations across goroutines.
    // Every use here runs on the dispatch thread, so the `RefCell` borrow
    // stands in for the lock: it is held for the same spans as `mu`.
    protocol: RefCell<Box<dyn Protocol>>,
    handler: Rc<dyn Handler>,

    // timing, when non-nil, accumulates the wall-clock time spent handling each
    // request. Clients retrieve the collected data via a getServerTiming request.
    timing: RefCell<Option<TimingCollector>>,
}

// Go: ipc/conn_sync.go:34 NewSyncConn
// NewSyncConn creates a new sync connection with the given transport and handler.
pub fn new_sync_conn(
    rwc: Arc<dyn ReadWriteCloser>,
    protocol: Box<dyn Protocol>,
    handler: Rc<dyn Handler>,
) -> Rc<SyncConn> {
    Rc::new(SyncConn {
        rwc,
        protocol: RefCell::new(protocol),
        handler,
        timing: RefCell::new(None),
    })
}

// PORT: the Go methods are inherent methods; `impl Conn` below forwards to
// them, so callers need not import `Conn`.
impl SyncConn {
    // Go: ipc/conn_sync.go:45 SetCollectTiming
    // SetCollectTiming enables or disables per-request server processing-time
    // measurement. When enabled, the connection accumulates timing that clients can
    // retrieve via a getServerTiming request.
    pub fn set_collect_timing(&self, enabled: bool) {
        if enabled {
            *self.timing.borrow_mut() = Some(new_timing_collector());
        } else {
            *self.timing.borrow_mut() = None;
        }
    }

    // Go: ipc/conn_sync.go:55 Run
    // Run starts processing messages on the connection.
    // It blocks until the context is cancelled or an error occurs.
    pub fn run(&self, ctx: &Context) -> Result<(), GoError> {
        loop {
            if let Some(err) = ctx.err() {
                return Err(err);
            }

            let result = self.protocol.borrow_mut().read_message();

            let msg = match result {
                Ok(msg) => msg,
                Err(err) => {
                    if errors::is(&err, &errors::EOF) {
                        return Ok(());
                    }
                    return Err(err);
                }
            };

            if msg.is_request() {
                // ts#64142
                self.handle_request(ctx, msg)?;
            } else if msg.is_notification() {
                self.handle_notification(ctx, msg);
            } else {
                // Responses are not expected in the main loop - they are read inline by Call().
                return Err(errors::new(
                    "ipc: unexpected response message in sync connection",
                ));
            }
        }
    }

    // Go: ipc/conn_sync.go:86 handleRequest
    // handleRequest processes an incoming request.
    // PORT: Go recovers panics in a deferred function; `go_recover` covers
    // the same body (the handler call and the response write). Go
    // `debug.Stack()` is the backtrace at the recover point.
    // ts#64142: write failures are returned, not panics.
    fn handle_request(&self, ctx: &Context, msg: Message) -> Result<(), GoError> {
        // Intercept the meta-requests for collected server timing before dispatching
        // to the handler, so they are answered directly and not themselves recorded.
        if msg.method == METHOD_GET_SERVER_TIMING {
            let snapshot = server_timing_snapshot(self.timing.borrow().as_ref());
            let write_err = self
                .protocol
                .borrow_mut()
                .write_response(msg.id.as_ref(), Some(Box::new(snapshot)));
            if let Err(write_err) = write_err {
                return Err(errors::errorf(
                    format!(
                        "ipc: failed to write server timing response: {}",
                        write_err.error()
                    ),
                    vec![write_err],
                ));
            }
            return Ok(());
        }
        if msg.method == METHOD_RESET_SERVER_TIMING {
            if let Some(timing) = self.timing.borrow_mut().as_mut() {
                timing.reset();
            }
            let write_err = self
                .protocol
                .borrow_mut()
                .write_response(msg.id.as_ref(), None);
            if let Err(write_err) = write_err {
                return Err(errors::errorf(
                    format!(
                        "ipc: failed to write reset server timing response: {}",
                        write_err.error()
                    ),
                    vec![write_err],
                ));
            }
            return Ok(());
        }

        let id = msg.id.clone();

        let start = Instant::now();

        // Recover from panics and convert to error response with stack trace
        let outcome = go_recover(|| -> Result<(), GoError> {
            let (result, err) = match self.handler.handle_request(ctx, &msg.method, msg.params) {
                Ok(result) => (result, None),
                Err(err) => (None, Some(err)),
            };

            if let Some(timing) = self.timing.borrow_mut().as_mut() {
                timing.record(&msg.method, start.elapsed());
            }

            let mut protocol = self.protocol.borrow_mut();

            let write_err = if let Some(err) = err {
                protocol.write_error(
                    id.as_ref(),
                    &jsonrpc::ResponseError {
                        code: jsonrpc::CODE_INTERNAL_ERROR,
                        message: err.error(),
                        data: None,
                    },
                )
            } else {
                protocol.write_response(id.as_ref(), result)
            };

            if let Err(write_err) = write_err {
                return Err(errors::errorf(
                    format!("ipc: failed to write response: {}", write_err.error()),
                    vec![write_err],
                ));
            }
            Ok(())
        });

        let payload = match outcome {
            Ok(result) => return result,
            Err(payload) => payload,
        };
        {
            let r = recovered_value(payload.as_ref());
            let stack = std::backtrace::Backtrace::force_capture().to_string();
            let err = errors::new(format!("panic: {r}\n{stack}"));

            // The panic answer runs in Go's deferred recover: a panic in it
            // prints the recovered panic first.
            let write_err = go_after_recover(payload.as_ref(), || {
                self.protocol.borrow_mut().write_error(
                    id.as_ref(),
                    &jsonrpc::ResponseError {
                        code: jsonrpc::CODE_INTERNAL_ERROR,
                        message: err.error(),
                        data: None,
                    },
                )
            });

            if let Err(write_err) = write_err {
                return Err(errors::errorf(
                    format!(
                        "ipc: failed to write panic error response: {} (original panic: {r})",
                        write_err.error()
                    ),
                    vec![write_err],
                ));
            }
        }
        Ok(())
    }

    // Go: ipc/conn_sync.go:164 handleNotification
    // handleNotification processes an incoming notification.
    fn handle_notification(&self, ctx: &Context, msg: Message) {
        let _ = self
            .handler
            .handle_notification(ctx, &msg.method, msg.params);
    }

    // Go: ipc/conn_sync.go:170 Call
    // Call sends a request to the client and waits for a response.
    // This method is safe to call from multiple goroutines - calls are serialized.
    pub fn call(
        &self,
        ctx: &Context,
        method: &str,
        params: Option<Box<dyn AnyValue>>,
    ) -> Result<JsonValue, GoError> {
        // Serialize all Call operations. This is critical because:
        // 1. The msgpack protocol uses method names as response IDs
        // 2. The handler code (project internals) may spawn goroutines that call
        //    filesystem callbacks concurrently
        // 3. We need to ensure write/read pairs are atomic
        // PORT: the Go mutex is the `protocol` borrow, taken for each read and
        // write, so that a nested request (ts#64299) can use the protocol.
        let id = jsonrpc::new_id_string(method);

        self.protocol
            .borrow_mut()
            .write_request(Some(&id), method, params)?;

        if let Some(err) = ctx.err() {
            return Err(err);
        }

        loop {
            // Read the response inline.
            let msg = self.protocol.borrow_mut().read_message()?;

            if msg.is_response() && msg.id.as_ref().is_some_and(|id| id.string() == method) {
                if let Some(error) = &msg.error {
                    return Err(errors::new(format!(
                        "ipc: remote error [{}]: {}",
                        error.code, error.message
                    )));
                }
                return Ok(msg.result);
            }
            if msg.is_request() {
                // A synchronous client callback may make a nested API request. Release
                // the protocol lock while handling it so nested callbacks can proceed.
                self.handle_request(ctx, msg)?;
                continue;
            }
            if msg.is_notification() {
                self.handle_notification(ctx, msg);
                continue;
            }
            return Err(errors::new(format!(
                "ipc: unexpected message while waiting for {} response",
                strconv::quote(method)
            )));
        }
    }

    // Go: ipc/conn_sync.go:224 Notify
    // Notify sends a notification to the client (no response expected).
    pub fn notify(
        &self,
        ctx: &Context,
        method: &str,
        params: Option<Box<dyn AnyValue>>,
    ) -> Result<(), GoError> {
        let _ = ctx;
        self.protocol
            .borrow_mut()
            .write_notification(method, params)
    }
}

impl Conn for SyncConn {
    fn run(&self, ctx: &Context) -> Result<(), GoError> {
        SyncConn::run(self, ctx)
    }

    fn call(
        &self,
        ctx: &Context,
        method: &str,
        params: Option<Box<dyn AnyValue>>,
    ) -> Result<JsonValue, GoError> {
        SyncConn::call(self, ctx, method, params)
    }

    fn notify(
        &self,
        ctx: &Context,
        method: &str,
        params: Option<Box<dyn AnyValue>>,
    ) -> Result<(), GoError> {
        SyncConn::notify(self, ctx, method, params)
    }
}

// Go: ipc/conn_sync_test.go
// PORT: Go shares `noOpHandler` with conn_async_test.go (one test package);
// here the conn_async tests export it. Go `nil` for the transport is
// `NilTransport`.
#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::gostd::context;
    use crate::ipc::conn_async::tests::{
        NilTransport, NoOpHandler, message, response_write_failed,
    };

    // Go: ipc/conn_sync_test.go:15 syncFailingResponseProtocol
    struct SyncFailingResponseProtocol {
        message: Option<Message>,
        response_err: GoError,
    }

    impl Protocol for SyncFailingResponseProtocol {
        fn read_message(&mut self) -> Result<Message, GoError> {
            self.message.take().ok_or_else(|| errors::EOF.clone())
        }

        fn write_request(
            &mut self,
            _id: Option<&jsonrpc::ID>,
            _method: &str,
            _params: Option<Box<dyn AnyValue>>,
        ) -> Result<(), GoError> {
            Ok(())
        }

        fn write_notification(
            &mut self,
            _method: &str,
            _params: Option<Box<dyn AnyValue>>,
        ) -> Result<(), GoError> {
            Ok(())
        }

        fn write_response(
            &mut self,
            _id: Option<&jsonrpc::ID>,
            _result: Option<Box<dyn AnyValue>>,
        ) -> Result<(), GoError> {
            Err(self.response_err.clone())
        }

        fn write_error(
            &mut self,
            _id: Option<&jsonrpc::ID>,
            _err: &jsonrpc::ResponseError,
        ) -> Result<(), GoError> {
            Err(self.response_err.clone())
        }
    }

    // Go: ipc/conn_sync_test.go:45 panicHandler
    struct PanicHandler;

    impl Handler for PanicHandler {
        fn handle_request(
            &self,
            _ctx: &Context,
            _method: &str,
            _params: JsonValue,
        ) -> Result<Option<Box<dyn AnyValue>>, GoError> {
            panic!("handler panic")
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

    // PORT: not in Go. A protocol that reads `message` once and panics
    // with a Go panic in each error answer.
    struct PanickingErrorAnswers {
        message: Option<Message>,
    }

    impl Protocol for PanickingErrorAnswers {
        fn read_message(&mut self) -> Result<Message, GoError> {
            self.message.take().ok_or_else(|| errors::EOF.clone())
        }

        fn write_request(
            &mut self,
            _id: Option<&jsonrpc::ID>,
            _method: &str,
            _params: Option<Box<dyn AnyValue>>,
        ) -> Result<(), GoError> {
            Ok(())
        }

        fn write_notification(
            &mut self,
            _method: &str,
            _params: Option<Box<dyn AnyValue>>,
        ) -> Result<(), GoError> {
            Ok(())
        }

        fn write_response(
            &mut self,
            _id: Option<&jsonrpc::ID>,
            _result: Option<Box<dyn AnyValue>>,
        ) -> Result<(), GoError> {
            Ok(())
        }

        fn write_error(
            &mut self,
            _id: Option<&jsonrpc::ID>,
            _err: &jsonrpc::ResponseError,
        ) -> Result<(), GoError> {
            crate::core::go_panic("write panic".to_string())
        }
    }

    // PORT: not in Go. The panic answer runs in the deferred recover of
    // `handleRequest` (ipc/conn_sync.go:119), so the Go runtime prints the
    // recovered panic before a panic in it. Go's stderr for `Run` on a
    // plain goroutine, as tsgo --api runs it (followups41
    // `go test -overlay`): `panic: handler panic [recovered]`, then
    // `\tpanic: write panic`.
    #[test]
    fn a_panic_in_the_panic_answer_keeps_the_recovered_panic_first() {
        let protocol = PanickingErrorAnswers {
            message: Some(message(Some(jsonrpc::new_id_int(1)), "transform")),
        };
        let conn = new_sync_conn(
            Arc::new(NilTransport),
            Box::new(protocol),
            Rc::new(PanicHandler),
        );

        let payload = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            conn.run(&context::background())
        }))
        .expect_err("the panic answer panics");
        let panic = payload
            .downcast_ref::<crate::core::GoPanic>()
            .expect("a Go panic");
        assert_eq!(panic.recovered_before, ["handler panic"]);
        assert_eq!(panic.value_text(), "write panic");
        assert!(!panic.repanicked);
    }

    // Go: ipc/conn_sync_test.go:55 TestSyncConnRunReturnsResponseWriteFailure
    #[test]
    fn test_sync_conn_run_returns_response_write_failure() {
        let response_err = response_write_failed();
        let protocol = SyncFailingResponseProtocol {
            message: Some(message(Some(jsonrpc::new_id_int(1)), "transform")),
            response_err: response_err.clone(),
        };
        let conn = new_sync_conn(
            Arc::new(NilTransport),
            Box::new(protocol),
            Rc::new(NoOpHandler),
        );

        let err = conn
            .run(&context::background())
            .expect_err("run returns the response write error");
        assert!(
            errors::is(&err, &response_err),
            "expected response write error, got {}",
            err.error()
        );
    }

    // Go: ipc/conn_sync_test.go:68 TestSyncConnRunReturnsPanicResponseWriteFailure
    #[test]
    fn test_sync_conn_run_returns_panic_response_write_failure() {
        let response_err = response_write_failed();
        let protocol = SyncFailingResponseProtocol {
            message: Some(message(Some(jsonrpc::new_id_int(1)), "transform")),
            response_err: response_err.clone(),
        };
        let conn = new_sync_conn(
            Arc::new(NilTransport),
            Box::new(protocol),
            Rc::new(PanicHandler),
        );

        let err = conn
            .run(&context::background())
            .expect_err("run returns the panic response write error");
        assert!(
            errors::is(&err, &response_err),
            "expected panic response write error, got {}",
            err.error()
        );
        assert!(
            err.error().contains("original panic: handler panic"),
            "{}",
            err.error()
        );
    }
}
