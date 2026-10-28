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
// PORT: Go `calls`, `reading` and `turn` (ts#64639) are not ported. Go's
// `Call` can run on several goroutines; they make the answers of nested
// requests go out in stack order: the answer of a request that came in
// during call `depth` waits until the calls above it have ended
// (`lockTurn`), and a new call waits while another call reads. Here every
// call runs on the dispatch thread (the connection is `Rc`, not `Send`).
// A call that starts while a nested request is handled is made by that
// handler, on the same stack, so it ends before the handler answers, and
// no call starts while another one reads. The answers go out in stack
// order with no wait, and `lockTurn` would never wait.
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

// Go: ipc/conn_sync.go:38 NewSyncConn
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
    // Go: ipc/conn_sync.go:51 SetCollectTiming
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

    // Go: ipc/conn_sync.go:61 Run
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
                // ts#64142. ts#64639: Go passes depth 0.
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

    // Go: ipc/conn_sync.go:100 handleRequest
    // handleRequest processes an incoming request.
    // PORT: Go recovers panics in a deferred function; `go_recover` covers
    // the same body (the handler call and the response write). Go
    // `debug.Stack()` is the backtrace at the recover point.
    // ts#64142: write failures are returned, not panics.
    // ts#64639: Go's `depth` parameter and each `lockTurn(depth)` before a
    // write are not ported: the turn is always this request's (see
    // `SyncConn`).
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
            let stack = crate::gostd::backtrace::text(&std::backtrace::Backtrace::force_capture());
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

    // Go: ipc/conn_sync.go:178 handleNotification
    // handleNotification processes an incoming notification.
    fn handle_notification(&self, ctx: &Context, msg: Message) {
        let _ = self
            .handler
            .handle_notification(ctx, &msg.method, msg.params);
    }

    // Go: ipc/conn_sync.go:184 Call
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
        // ts#64639: Go's turn state (`calls`, `reading`, the call's `depth`)
        // is not ported (see `SyncConn`).
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

    // Go: ipc/conn_sync.go:253 Notify
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

    // Go: ipc/conn_sync_test.go:18 syncFailingResponseProtocol
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

    // Go: ipc/conn_sync_test.go:48 panicHandler
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
    // `handleRequest` (ipc/conn_sync.go:134), so the Go runtime prints the
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

    // Go: ipc/conn_sync_test.go:58 TestSyncConnRunReturnsResponseWriteFailure
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

    // Go: ipc/conn_sync_test.go:71 TestSyncConnRunReturnsPanicResponseWriteFailure
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

    /// Go `json.Value(text)`.
    fn json_value(text: &str) -> JsonValue {
        JsonValue(text.as_bytes().to_vec())
    }

    /// Go `string(value)` of a `json.Value`.
    fn json_text(value: &JsonValue) -> String {
        String::from_utf8(value.0.clone()).expect("UTF-8 JSON")
    }

    // Go: ipc/conn_sync_test.go:85 gatedHandler
    // PORT: Go gates the handler of each request on a channel, and the
    // test starts the call of `"b"` on a second goroutine while the
    // handler of the nested request for `"a"` waits. The connection is
    // not `Send` (see `SyncConn`), so here the handler of `"a"` makes the
    // call of `"b"`: on one thread, that is how a second call starts before
    // `"a"` is answered. `b` keeps the result of that call.
    struct GatedHandler {
        conn: RefCell<Option<Rc<SyncConn>>>,
        b: RefCell<Option<JsonValue>>,
    }

    impl Handler for GatedHandler {
        fn handle_request(
            &self,
            ctx: &Context,
            _method: &str,
            params: JsonValue,
        ) -> Result<Option<Box<dyn AnyValue>>, GoError> {
            if json_text(&params) == r#""a""# {
                let conn = self.conn.borrow().clone().expect("the connection");
                let b = conn.call(ctx, "callback", Some(Box::new(json_value(r#""b""#))));
                *self.b.borrow_mut() = b.ok();
            }
            Ok(Some(Box::new(params)))
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

    // Go: ipc/conn_sync_test.go:96 TestSyncConnAnswersNestedRequestsInStackOrder
    // (ts#64639). The peer answers each call with a nested `resolve`
    // request and serves it before it answers the call, so an answer that
    // goes out of stack order lands in the wrong `serve` and changes
    // `answers`.
    #[test]
    fn test_sync_conn_answers_nested_requests_in_stack_order() {
        use std::collections::BTreeMap;
        use std::os::unix::net::UnixStream;

        let (server, client) = UnixStream::pair().expect("socket pair");
        let server: Arc<dyn ReadWriteCloser> =
            Arc::new(crate::ipc::conn_async::tests::PipeEnd::new(server));
        let client: Arc<dyn ReadWriteCloser> =
            Arc::new(crate::ipc::conn_async::tests::PipeEnd::new(client));
        let handler = Rc::new(GatedHandler {
            conn: RefCell::new(None),
            b: RefCell::new(None),
        });
        let conn = new_sync_conn(
            server.clone(),
            Box::new(crate::ipc::new_jsonrpc_protocol(server)),
            handler.clone(),
        );
        *handler.conn.borrow_mut() = Some(conn.clone());

        // Go `serve`: answers a request with a nested `resolve` request,
        // serves until that one is answered, then answers the request.
        fn serve(
            peer: &mut crate::ipc::JSONRPCProtocol,
            answers: &mut BTreeMap<String, String>,
        ) -> Option<JsonValue> {
            loop {
                let Ok(msg) = peer.read_message() else {
                    return None;
                };
                if msg.is_response() {
                    return Some(msg.result);
                }
                let _ = peer.write_request(
                    Some(&jsonrpc::new_id_string("resolve")),
                    "resolve",
                    Some(Box::new(msg.params.clone())),
                );
                let answer = serve(peer, answers).unwrap_or_default();
                answers.insert(json_text(&msg.params), json_text(&answer));
                let _ = peer.write_response(msg.id.as_ref(), Some(Box::new(msg.params)));
            }
        }
        let peer_end = client.clone();
        let peer = std::thread::spawn(move || {
            let mut peer = crate::ipc::new_jsonrpc_protocol(peer_end);
            let mut answers = BTreeMap::new();
            serve(&mut peer, &mut answers);
            answers
        });

        let a = conn.call(
            &context::background(),
            "callback",
            Some(Box::new(json_value(r#""a""#))),
        );
        let b = handler.b.borrow_mut().take();
        handler.conn.borrow_mut().take();
        client.close().expect("client close");
        let answers = peer.join().expect("the peer thread");

        assert_eq!(
            answers,
            BTreeMap::from([
                (r#""a""#.to_string(), r#""a""#.to_string()),
                (r#""b""#.to_string(), r#""b""#.to_string()),
            ])
        );
        assert_eq!(a.map(|a| json_text(&a)).ok().as_deref(), Some(r#""a""#));
        assert_eq!(b.map(|b| json_text(&b)).as_deref(), Some(r#""b""#));
    }

    // Go: ipc/conn_sync_test.go:139 callingNotificationHandler
    struct CallingNotificationHandler {
        conn: RefCell<Option<Rc<SyncConn>>>,
    }

    impl Handler for CallingNotificationHandler {
        // Go: the embedded noOpHandler.
        fn handle_request(
            &self,
            _ctx: &Context,
            _method: &str,
            _params: JsonValue,
        ) -> Result<Option<Box<dyn AnyValue>>, GoError> {
            Ok(None)
        }

        // Go: ipc/conn_sync_test.go:144
        fn handle_notification(
            &self,
            ctx: &Context,
            _method: &str,
            _params: JsonValue,
        ) -> Result<(), GoError> {
            let conn = self.conn.borrow().clone().expect("the connection");
            conn.call(ctx, "inner", None).map(|_| ())
        }
    }

    // Go: ipc/conn_sync_test.go:149 TestSyncConnNotificationHandlerCanCall
    // (ts#64639)
    #[test]
    fn test_sync_conn_notification_handler_can_call() {
        use std::os::unix::net::UnixStream;

        let (server, client) = UnixStream::pair().expect("socket pair");
        let server: Arc<dyn ReadWriteCloser> =
            Arc::new(crate::ipc::conn_async::tests::PipeEnd::new(server));
        let client: Arc<dyn ReadWriteCloser> =
            Arc::new(crate::ipc::conn_async::tests::PipeEnd::new(client));
        let handler = Rc::new(CallingNotificationHandler {
            conn: RefCell::new(None),
        });
        let conn = new_sync_conn(
            server.clone(),
            Box::new(crate::ipc::new_jsonrpc_protocol(server)),
            handler.clone(),
        );
        *handler.conn.borrow_mut() = Some(conn.clone());
        let peer_end = client.clone();
        let peer = std::thread::spawn(move || {
            let mut peer = crate::ipc::new_jsonrpc_protocol(peer_end);
            let outer = peer.read_message().unwrap_or_default();
            let _ = peer.write_notification("notify", None);
            let inner = peer.read_message().unwrap_or_default();
            let _ = peer.write_response(inner.id.as_ref(), Some(Box::new(inner.method)));
            let _ = peer.write_response(outer.id.as_ref(), Some(Box::new(outer.method)));
        });

        let result = conn.call(&context::background(), "outer", None);
        handler.conn.borrow_mut().take();
        peer.join().expect("the peer thread");
        client.close().expect("client close");

        let result = result.unwrap_or_else(|err| panic!("call failed: {}", err.error()));
        assert_eq!(json_text(&result), r#""outer""#);
    }
}
