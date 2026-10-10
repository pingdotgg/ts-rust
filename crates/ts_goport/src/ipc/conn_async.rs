//! Port of internal/ipc/conn_async.go (internal/api/conn_async.go before
//! tsgo#4712).
//!
//! PORT: Go handles each incoming request in its own goroutine, and `Call`
//! waits on a response channel that the `Run` goroutine fills. The API
//! session and project state live on the dispatch thread, so here `Run`
//! handles each request and notification inline. A request or notification
//! that arrives while a `Call` waits is handled inline at once, as the sync
//! connection does: a client callback may make a nested API request and
//! wait for its answer before it answers the call (ts#64299), which Go's
//! goroutines allow. So the responses can come in a different order than
//! in Go.
//!
//! PORT: the reads have two forms.
//! - With `read_on_thread` (`--api --async`), `run` reads on a thread of
//!   its own, as Go's `Run` goroutine reads: it checks `ctx` before each
//!   read (Go :83), so it reads exactly 1 message after `ctx` is done. The
//!   dispatch thread takes the messages from an inbox. A `Call` waits on
//!   the inbox and on `ctx`, as Go's `select` does, so it returns
//!   `ctx.Err()` at once when `ctx` is done (a signal), and a later `Call`
//!   writes its request and returns `ctx.Err()`. When the thread ends (Go's
//!   `Run` returns), the dispatch thread runs the first part of Go's
//!   deferred function before it handles anything more: `closePendingCalls`
//!   sets `terminal`, and a later `Call` returns it and writes nothing.
//! - Without it, `run` and `call` read on the dispatch thread, and `Call`
//!   reads messages itself until its response arrives. A blocked read does
//!   not wake when `ctx` is done. When a read in `call` fails, the read
//!   loop ends there, as Go's `Run` would: `closePendingCalls` sets
//!   `terminal` (tsgo#4712), the call returns it, and a later `run` returns
//!   what Go's `Run` returned. A panic in one of these reads is a panic in
//!   Go's `Run`, which the handler that made the call does not recover: the
//!   deferred function of `Run` closes the pending calls, the call returns
//!   `terminal`, and the panic leaves `run` once that handler returns (Go's
//!   deferred function waits for the handlers).

use crate::ipc::prelude::*;

use crate::core::{GoThread, go_recover};
use crate::frontend::json_ext::{AnyValue, JsonValue};
use crate::gostd::context::Done;
use crate::gostd::{Context, GoError, context, errors};
use crate::ipc::conn::{Conn, ERR_CONN_CLOSED, Handler, recovered_value};
use crate::ipc::protocol::{Message, Protocol};
use crate::ipc::protocol_jsonrpc::new_jsonrpc_protocol;
use crate::ipc::transport::ReadWriteCloser;
use crate::jsonrpc;
use std::any::Any;
use std::cell::Cell;
use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::Instant;

/// Go `chan *Message` with capacity 1 for one pending server-to-client call.
type ResponseChan = Rc<RefCell<Option<Message>>>;

// Go: ipc/conn_async.go:20 AsyncConn
// AsyncConn manages bidirectional JSON-RPC communication with async request handling.
// Each incoming request is handled in its own goroutine, allowing concurrent processing.
// This is the standard implementation for LSP-style JSON-RPC protocols.
pub struct AsyncConn {
    rwc: Arc<dyn ReadWriteCloser>,
    // PORT: Go `writeMu` guards writes across goroutines; on the dispatch
    // thread the `RefCell` borrow stands in for it (reads use it too).
    protocol: RefCell<Box<dyn Protocol>>,
    handler: Rc<dyn Handler>,

    // timing, when non-nil, accumulates the wall-clock time spent handling each
    // request. Clients retrieve the collected data via a getServerTiming request.
    timing: RefCell<Option<TimingCollector>>,

    // For server→client requests
    // PORT: Go `atomic.Int64` and the `pendingMu` lock are plain cells.
    seq: Cell<i64>,
    pending: RefCell<FxHashMap<jsonrpc::ID, ResponseChan>>,
    terminal: RefCell<Option<GoError>>,
    // ts#64142
    has_cause: Cell<bool>,

    // ts#64142: Go `requestErrors := make(chan error, 1)` of `Run`. PORT: a
    // field, because `call` also handles requests (file header).
    request_errors: RefCell<Option<GoError>>,
    // PORT: how the read loop ended when a read in `call` ended it: the
    // value Go's `Run` returned. `run` returns it and reads no more.
    read_loop_end: RefCell<Option<Result<(), GoError>>>,
    // PORT: the payload of a panic in a read in `call`, where Go's `Run`
    // panicked. `run` resumes the panic.
    read_panic: RefCell<Option<Box<dyn Any + Send>>>,
    // PORT: true once `run` reads. A `call` then runs in a handler of `run`.
    running: Cell<bool>,
    // PORT: the protocol that `run` gives to its reader thread
    // (`read_on_thread`).
    read_protocol: RefCell<Option<Box<dyn Protocol + Send>>>,
    // PORT: set while `run` reads on its thread, and after.
    reader: RefCell<Option<Rc<RunReader>>>,
}

// Go: ipc/conn_async.go:41 NewAsyncConn
// NewAsyncConn creates a new async connection with the given transport and handler.
// It uses JSONRPCProtocol (LSP-style Content-Length framing) by default.
pub fn new_async_conn(rwc: Arc<dyn ReadWriteCloser>, handler: Rc<dyn Handler>) -> Rc<AsyncConn> {
    let protocol = new_jsonrpc_protocol(rwc.clone());
    new_async_conn_with_protocol(rwc, Box::new(protocol), handler)
}

// Go: ipc/conn_async.go:46 NewAsyncConnWithProtocol
// NewAsyncConnWithProtocol creates a new async connection with a custom protocol.
pub fn new_async_conn_with_protocol(
    rwc: Arc<dyn ReadWriteCloser>,
    protocol: Box<dyn Protocol>,
    handler: Rc<dyn Handler>,
) -> Rc<AsyncConn> {
    Rc::new(AsyncConn {
        rwc,
        protocol: RefCell::new(protocol),
        handler,
        timing: RefCell::new(None),
        seq: Cell::new(0),
        pending: RefCell::new(FxHashMap::default()),
        terminal: RefCell::new(None),
        has_cause: Cell::new(false),
        request_errors: RefCell::new(None),
        read_loop_end: RefCell::new(None),
        read_panic: RefCell::new(None),
        running: Cell::new(false),
        read_protocol: RefCell::new(None),
        reader: RefCell::new(None),
    })
}

// PORT: the Go methods are inherent methods; `impl Conn` below forwards to
// them, so callers need not import `Conn`.
impl AsyncConn {
    // Go: ipc/conn_async.go:58 SetCollectTiming
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

    /// PORT: not in Go. Makes `run` read on a thread of its own through
    /// `protocol`, as Go's `Run` goroutine reads (file header). `protocol`
    /// reads the connection's transport; only its `read_message` is used,
    /// and the connection's protocol then only writes. Call it before `run`:
    /// a `call` made before `run` reads through the connection's protocol.
    /// wasm has no threads, so there the reads stay on the dispatch thread.
    pub fn read_on_thread(&self, protocol: Box<dyn Protocol + Send>) {
        if cfg!(target_family = "wasm") {
            return;
        }
        *self.read_protocol.borrow_mut() = Some(protocol);
    }

    // Go: ipc/conn_async.go:68 Run
    // Run starts processing messages on the connection.
    // It blocks until the context is cancelled or an error occurs.
    pub fn run(&self, ctx: &Context) -> Result<(), GoError> {
        // Go: ipc/conn_async.go:69
        let (handler_ctx, cancel_handlers) = context::with_cancel(ctx);
        // Go: defer func() { c.closePendingCalls(err); cancelHandlers(); c.handlers.Wait(); ... }()
        // PORT: `RunDefer` runs the first two calls on every exit, also when
        // the loop panics. The handlers run inline (file header), so when
        // the loop ends no handler is active and `handlers.Wait()`
        // (ts#64163) has nothing to wait for. The cancel still reaches a
        // context that a handler kept. The join of the request error comes
        // after the guard, because a panic discards Go's result too.
        let mut deferred = RunDefer {
            conn: self,
            cancel_handlers: cancel_handlers.clone(),
            err: None,
        };
        let read_protocol = self.read_protocol.borrow_mut().take();
        let mut result = match read_protocol {
            Some(protocol) => {
                let reader = Rc::new(RunReader {
                    inbox: start_reader(ctx, cancel_handlers, protocol),
                    handler_ctx,
                });
                *self.reader.borrow_mut() = Some(reader.clone());
                self.run_loop_on_reader(&reader)
            }
            None => self.run_loop(ctx, &handler_ctx),
        };
        deferred.err = result.as_ref().err().cloned();
        drop(deferred);
        let request_err = self.request_errors.borrow_mut().take();
        if let Some(request_err) = request_err {
            result = Err(errors::join([result.err(), Some(request_err)])
                .expect("the request error is non-nil"));
        }
        result
    }

    /// The loop of Go `Run`, without its deferred function. Requests and
    /// notifications get `handler_ctx` (Go `handlerCtx`).
    fn run_loop(&self, ctx: &Context, handler_ctx: &Context) -> Result<(), GoError> {
        self.running.set(true);
        loop {
            // PORT: a read in `call` panicked. Go's `Run` panicked at that
            // read, before it checked `ctx` again.
            if let Some(payload) = self.read_panic.take() {
                resume_unwind(payload);
            }
            if let Some(err) = ctx.err() {
                return Err(err);
            }

            // PORT: when a read in `call` ended the read loop, there is
            // nothing more to read.
            if let Some(end) = self.read_loop_end.borrow().clone() {
                return end;
            }
            let result = self.protocol.borrow_mut().read_message();
            let msg = match result {
                Ok(msg) => msg,
                Err(err) => return read_loop_result(err),
            };

            self.dispatch(handler_ctx, msg);
        }
    }

    /// The loop of Go `Run` when `run` reads on its thread: the dispatch
    /// of each message that the thread read, in order. It ends when the
    /// thread ended and no message is left, and returns what Go's `Run`
    /// returned (or resumes the panic of its read).
    fn run_loop_on_reader(&self, reader: &RunReader) -> Result<(), GoError> {
        while let Some(msg) = reader.inbox.wait(None) {
            self.dispatch(&reader.handler_ctx, msg);
        }
        self.end_read_loop(reader);
        if let Some(payload) = self.read_panic.take() {
            resume_unwind(payload);
        }
        self.read_loop_end
            .borrow()
            .clone()
            .expect("the reader thread ended")
    }

    /// PORT: when the reader thread ended (Go's `Run` returned), runs
    /// Go's deferred `closePendingCalls` with the result of the loop on the
    /// dispatch thread, once (the thread runs `cancelHandlers`). Each
    /// `call` and `notify` runs this first, so they return `terminal` once
    /// the thread ended. This holds also for the calls of a request read
    /// with the end, which Go starts before its deferred function runs: Go
    /// races them, and the deferred function wins in practice (race 1).
    fn end_read_loop(&self, reader: &RunReader) {
        let Some(end) = lock(&reader.inbox.state).end.take() else {
            return;
        };
        match end {
            ReadEnd::Returned(result) => {
                self.close_pending_calls(result.as_ref().err());
                *self.read_loop_end.borrow_mut() = Some(result);
            }
            ReadEnd::Panicked(payload) => {
                // Go runs the deferred function while the panic unwinds;
                // its `err` is nil.
                self.close_pending_calls(None);
                *self.read_panic.borrow_mut() = Some(payload);
            }
        }
    }

    /// The message branches of the Go `Run` loop. A request and a
    /// notification run inline (Go `c.handlers.Go(...)`, ts#64163), also
    /// when `call` read them.
    fn dispatch(&self, ctx: &Context, msg: Message) {
        if msg.is_response() {
            self.handle_response(msg);
        } else if msg.is_request() {
            if let Err(request_err) = self.handle_request(ctx, msg)
                && self.record_request_error(request_err)
            {
                // PORT: Go checks `c.rwc != nil`; the Rust transport is
                // always set.
                let _ = self.rwc.close();
            }
        } else if msg.is_notification() {
            self.handle_notification(ctx, msg);
        }
    }

    // Go: ipc/conn_async.go:116 closePendingCalls
    // closePendingCalls records that the read loop has exited and unblocks requests waiting for a response.
    // PORT: Go closes each pending response channel, and the `Call` that
    // waits on it returns `terminal`. Here a call whose entry is gone
    // without a response returns `terminal` when it looks again.
    fn close_pending_calls(&self, run_err: Option<&GoError>) {
        self.record_terminal_error_locked(run_err);
        self.close_pending_calls_locked();
    }

    // Go: ipc/conn_async.go recordRequestError (ts#64142)
    // PORT: Go sends to the `requestErrors` channel (capacity 1); here the
    // slot is an `Option`. Only the first cause returns true, so one error is
    // stored at most.
    fn record_request_error(&self, request_err: GoError) -> bool {
        if !self.record_terminal_error_locked(Some(&request_err)) {
            return false;
        }
        *self.request_errors.borrow_mut() = Some(request_err);
        self.close_pending_calls_locked();
        true
    }

    // Go: ipc/conn_async.go recordTerminalErrorLocked (ts#64142)
    fn record_terminal_error_locked(&self, terminal_err: Option<&GoError>) -> bool {
        let mut terminal = self.terminal.borrow_mut();
        if terminal.is_none() {
            let err = ERR_CONN_CLOSED.clone();
            if let Some(terminal_err) = terminal_err {
                *terminal = Some(
                    errors::join([err, terminal_err.clone()]).expect("both errors are non-nil"),
                );
                self.has_cause.set(true);
                return true;
            }
            *terminal = Some(err);
        } else if !self.has_cause.get()
            && let Some(terminal_err) = terminal_err
        {
            let current = terminal.take().expect("terminal is set");
            *terminal = Some(
                errors::join([current, terminal_err.clone()]).expect("both errors are non-nil"),
            );
            self.has_cause.set(true);
            return true;
        }
        false
    }

    // Go: ipc/conn_async.go closePendingCallsLocked (ts#64142)
    fn close_pending_calls_locked(&self) {
        self.pending.borrow_mut().clear();
    }

    // Go: ipc/conn_async.go:158 handleResponse
    // handleResponse matches a response to a pending request.
    fn handle_response(&self, msg: Message) {
        let Some(id) = msg.id.clone() else {
            // Go dereferences the nil ID and panics; responses always have one.
            panic!("runtime error: invalid memory address or nil pointer dereference");
        };
        let ch = self.pending.borrow_mut().remove(&id);

        if let Some(ch) = ch {
            *ch.borrow_mut() = Some(msg);
        }
    }

    // Go: ipc/conn_async.go:173 handleRequest
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

        let r = match outcome {
            Ok(result) => return result,
            Err(r) => r,
        };
        {
            let r = recovered_value(r.as_ref());
            let stack = std::backtrace::Backtrace::force_capture().to_string();
            let err = errors::new(format!("panic: {r}\n{stack}"));

            let write_err = self.protocol.borrow_mut().write_error(
                id.as_ref(),
                &jsonrpc::ResponseError {
                    code: jsonrpc::CODE_INTERNAL_ERROR,
                    message: err.error(),
                    data: None,
                },
            );

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

    // Go: ipc/conn_async.go:251 handleNotification
    // handleNotification processes an incoming notification.
    fn handle_notification(&self, ctx: &Context, msg: Message) {
        let _ = self
            .handler
            .handle_notification(ctx, &msg.method, msg.params);
    }

    // Go: ipc/conn_async.go:256 Call
    // Call sends a request to the client and waits for a response.
    pub fn call(
        &self,
        ctx: &Context,
        method: &str,
        params: Option<Box<dyn AnyValue>>,
    ) -> Result<JsonValue, GoError> {
        // Create unique request ID
        self.seq.set(self.seq.get() + 1);
        let id = jsonrpc::new_id_string(&format!("api{}", self.seq.get()));

        // Register response channel BEFORE sending request to avoid race
        let response_chan: ResponseChan = Rc::new(RefCell::new(None));
        let reader = self.reader.borrow().clone();
        if let Some(reader) = &reader {
            self.end_read_loop(reader);
        }
        let terminal = self.terminal.borrow().clone();
        if let Some(err) = terminal {
            return Err(err);
        }
        self.pending
            .borrow_mut()
            .insert(id.clone(), response_chan.clone());

        // Go: ipc/conn_async.go:271 defer func() { ... delete(c.pending, *id) }()
        let _deferred = CallDefer {
            conn: self,
            id: &id,
        };

        // Send the request
        let err = self
            .protocol
            .borrow_mut()
            .write_request(Some(&id), method, params);

        if let Err(err) = err {
            return Err(err);
        }

        if let Some(reader) = &reader {
            return self.wait_on_reader(ctx, &id, &response_chan, reader);
        }

        // PORT: Go selects on `ctx.Done()` and the response channel while the
        // Run goroutine reads. Here the loop reads messages until the
        // response is in the channel; `ctx` is checked before each read (a
        // blocked read is not interrupted). A read error ends the read loop,
        // as it ends Go's `Run`, and the call returns `terminal`, as Go's
        // `Call` does when `closePendingCalls` closes its channel.
        loop {
            if let Some(err) = ctx.err() {
                return Err(err);
            }

            let resp = response_chan.borrow_mut().take();
            if let Some(resp) = resp {
                return response_result(resp);
            }

            // PORT: a panic in this read is a panic in Go's `Run` (file
            // header), so it does not unwind through the handler that made
            // the call.
            let read = catch_unwind(AssertUnwindSafe(|| {
                self.protocol.borrow_mut().read_message()
            }));
            let msg = match read {
                Ok(Ok(msg)) => msg,
                Ok(Err(err)) => {
                    let end = read_loop_result(err);
                    self.close_pending_calls(end.as_ref().err());
                    *self.read_loop_end.borrow_mut() = Some(end);
                    let terminal = self.terminal.borrow().clone();
                    return Err(terminal.expect("closePendingCalls sets terminal"));
                }
                Err(payload) => {
                    // Go: the deferred function of `Run` closes the pending
                    // calls while the panic unwinds; its `err` is nil.
                    self.close_pending_calls(None);
                    if !self.running.get() {
                        // No `run` can resume the panic, so it leaves here.
                        // In Go the panic ends the process. The read loop
                        // ends as on the run path: a later `run` reads no
                        // more and returns Go's deferred `err` (nil).
                        *self.read_loop_end.borrow_mut() = Some(Ok(()));
                        resume_unwind(payload);
                    }
                    *self.read_panic.borrow_mut() = Some(payload);
                    let terminal = self.terminal.borrow().clone();
                    return Err(terminal.expect("closePendingCalls sets terminal"));
                }
            };
            self.dispatch(ctx, msg);
            // Go: a closed response channel (`closePendingCalls` after a
            // request error) makes the call return `terminal`.
            if !self.pending.borrow().contains_key(&id) && response_chan.borrow().is_none() {
                let terminal = self.terminal.borrow().clone();
                return Err(terminal.expect("closePendingCalls sets terminal"));
            }
        }
    }

    /// The `select` of Go `Call` (:289-303) when `run` reads on its thread:
    /// the call waits until its response comes, its channel closes or
    /// `ctx` is done, and dispatches the messages that come meanwhile (file
    /// header). The order of the checks is the case that wakes Go's
    /// `select`: a response or a close (the dispatch thread runs them
    /// before it looks at `ctx` again), then `ctx`. A call made when `ctx`
    /// is already done returns `ctx.Err()` right after its write.
    fn wait_on_reader(
        &self,
        ctx: &Context,
        id: &jsonrpc::ID,
        response_chan: &ResponseChan,
        reader: &RunReader,
    ) -> Result<JsonValue, GoError> {
        let _waker = reader.inbox.wake_on_done(ctx);
        loop {
            let resp = response_chan.borrow_mut().take();
            if let Some(resp) = resp {
                return response_result(resp);
            }
            if !self.pending.borrow().contains_key(id) {
                let terminal = self.terminal.borrow().clone();
                return Err(terminal.expect("closePendingCalls sets terminal"));
            }
            if let Some(err) = ctx.err() {
                return Err(err);
            }
            match reader.inbox.wait(Some(ctx)) {
                Some(msg) => self.dispatch(&reader.handler_ctx, msg),
                None => self.end_read_loop(reader),
            }
        }
    }

    // Go: ipc/conn_async.go:307 Notify
    // Notify sends a notification to the client (no response expected).
    pub fn notify(
        &self,
        ctx: &Context,
        method: &str,
        params: Option<Box<dyn AnyValue>>,
    ) -> Result<(), GoError> {
        let _ = ctx;
        let reader = self.reader.borrow().clone();
        if let Some(reader) = &reader {
            self.end_read_loop(reader);
        }
        let terminal = self.terminal.borrow().clone();
        if let Some(err) = terminal {
            return Err(err);
        }
        self.protocol
            .borrow_mut()
            .write_notification(method, params)
    }
}

/// The first part of the deferred function of Go `Run`: close the pending
/// calls with Go's named result `err`, then cancel the handler context. Its
/// `Drop` runs on a normal return and when the loop panics (Go runs a
/// deferred function while a panic unwinds; `err` is then nil).
struct RunDefer<'a> {
    conn: &'a AsyncConn,
    cancel_handlers: context::CancelFunc,
    err: Option<GoError>,
}

impl Drop for RunDefer<'_> {
    fn drop(&mut self) {
        self.conn.close_pending_calls(self.err.as_ref());
        (self.cancel_handlers)();
    }
}

/// What Go `Run` returns when its read fails: nil for `io.EOF`, else the
/// error.
fn read_loop_result(err: GoError) -> Result<(), GoError> {
    if errors::is(&err, &errors::EOF) {
        return Ok(());
    }
    Err(err)
}

/// What Go `Call` returns for the response it received (:299-302).
fn response_result(resp: Message) -> Result<JsonValue, GoError> {
    if let Some(error) = &resp.error {
        return Err(errors::new(format!(
            "ipc: remote error [{}]: {}",
            error.code, error.message
        )));
    }
    Ok(resp.result)
}

/// PORT: what `run` keeps while its reader thread reads
/// (`read_on_thread`).
struct RunReader {
    inbox: Arc<Inbox>,
    /// Go `handlerCtx`. A request that a `call` dispatches gets it too.
    handler_ctx: Context,
}

/// PORT: the messages that the reader thread read, and how its loop ended.
#[derive(Default)]
struct Inbox {
    state: Mutex<InboxState>,
    ready: Condvar,
}

#[derive(Default)]
struct InboxState {
    messages: VecDeque<Message>,
    /// True once the thread ended (Go's `Run` returned).
    ended: bool,
    /// How the loop ended, until `end_read_loop` takes it.
    end: Option<ReadEnd>,
}

/// How Go's `Run` loop ended: it returned this value, or its read panicked.
enum ReadEnd {
    Returned(Result<(), GoError>),
    Panicked(Box<dyn Any + Send>),
}

impl Inbox {
    /// The next message, waiting as needed. It returns `None` once the
    /// thread ended and no message is left, and, with `call_ctx` (a
    /// `call`), once `call_ctx` is done: then the messages left are for
    /// `run` (Go's `Run` already started their goroutines).
    fn wait(&self, call_ctx: Option<&Context>) -> Option<Message> {
        let mut state = lock(&self.state);
        loop {
            if call_ctx.is_some_and(|ctx| ctx.err().is_some()) {
                return None;
            }
            if let Some(msg) = state.messages.pop_front() {
                return Some(msg);
            }
            if state.ended {
                return None;
            }
            state = self
                .ready
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    /// Wakes `wait` when `ctx` is done, until the result drops.
    fn wake_on_done(self: &Arc<Self>, ctx: &Context) -> DoneWaker {
        let Some(done) = ctx.done() else {
            return DoneWaker(None);
        };
        let inbox = self.clone();
        let id = done.register_waker(move || {
            let _state = lock(&inbox.state);
            inbox.ready.notify_all();
        });
        DoneWaker(id.map(|id| (done, id)))
    }

    fn push(&self, msg: Option<Message>, end: Option<ReadEnd>) {
        let mut state = lock(&self.state);
        state.messages.extend(msg);
        if end.is_some() {
            state.ended = true;
            state.end = end;
        }
        self.ready.notify_all();
    }
}

/// A waker of `Inbox::wake_on_done`; drop removes it.
struct DoneWaker(Option<(Done, u64)>);

impl Drop for DoneWaker {
    fn drop(&mut self) {
        if let Some((done, id)) = &self.0 {
            done.unregister_waker(*id);
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Starts the reader thread: the loop of Go `Run` (:82-112) up to the
/// dispatch, which the dispatch thread does (`run_loop_on_reader`). The
/// check of `ctx` before each read is Go's :83, so after `ctx` is done the
/// thread reads 1 more message at most.
fn start_reader(
    ctx: &Context,
    cancel_handlers: context::CancelFunc,
    mut protocol: Box<dyn Protocol + Send>,
) -> Arc<Inbox> {
    let inbox = Arc::new(Inbox::default());
    let thread_inbox = inbox.clone();
    let ctx = ctx.clone();
    // The Go stack size, as the LSP reader threads have.
    GoThread::new()
        .name("ipc-reader".to_string())
        .stack_size(crate::gostd::stack::max_stack_size())
        .spawn(move || {
            let (last, end) = loop {
                // Go: ipc/conn_async.go:83
                if let Some(err) = ctx.err() {
                    break (None, ReadEnd::Returned(Err(err)));
                }
                // Go: ipc/conn_async.go:87. A panic here is a panic in Go's
                // `Run`: the dispatch thread resumes it (`run_loop_on_reader`).
                let msg = match catch_unwind(AssertUnwindSafe(|| protocol.read_message())) {
                    Ok(Ok(msg)) => msg,
                    Ok(Err(err)) => break (None, ReadEnd::Returned(read_loop_result(err))),
                    Err(payload) => break (None, ReadEnd::Panicked(payload)),
                };
                // Go dispatches the message, then checks `ctx` (:83). The
                // check is here, so the dispatch thread finds the end of the
                // loop with the message read after a signal (`end_read_loop`).
                if let Some(err) = ctx.err() {
                    break (Some(msg), ReadEnd::Returned(Err(err)));
                }
                thread_inbox.push(Some(msg), None);
            };
            thread_inbox.push(last, Some(end));
            // Go: the deferred `cancelHandlers` (:73), at once, also while a
            // handler runs. Go runs `closePendingCalls` first; here the
            // dispatch thread runs it when it finds the end (`end_read_loop`),
            // and a call that this cancel wakes finds the end first.
            cancel_handlers();
        });
    inbox
}

/// The deferred function of Go `Call`: close and delete the call's pending
/// entry. Its `Drop` runs on every return and when the call panics (Go
/// runs a deferred function while a panic unwinds).
struct CallDefer<'a> {
    conn: &'a AsyncConn,
    id: &'a jsonrpc::ID,
}

impl Drop for CallDefer<'_> {
    fn drop(&mut self) {
        self.conn.pending.borrow_mut().remove(self.id);
    }
}

impl Conn for AsyncConn {
    fn run(&self, ctx: &Context) -> Result<(), GoError> {
        AsyncConn::run(self, ctx)
    }

    fn call(
        &self,
        ctx: &Context,
        method: &str,
        params: Option<Box<dyn AnyValue>>,
    ) -> Result<JsonValue, GoError> {
        AsyncConn::call(self, ctx, method, params)
    }

    fn notify(
        &self,
        ctx: &Context,
        method: &str,
        params: Option<Box<dyn AnyValue>>,
    ) -> Result<(), GoError> {
        AsyncConn::notify(self, ctx, method, params)
    }
}

// Go: ipc/conn_async_test.go (tsgo#4712)
// PORT: Go `net.Pipe` is a `UnixStream` pair. Go runs `Run` and `Call` in
// goroutines, and each request and notification handler in a goroutine of
// its own. The port handles them inline (see the file header), so:
// - `Call` reads its own response. The tests run `run` and `call` in turn
//   on one thread, and the peer runs on a thread when it must act while
//   `call` blocks.
// - A test that waits for a handler while `Run` runs makes the connection
//   on a thread of its own (the connection is not `Send`), and sends back
//   the texts of the errors that it asserts on.
// - A second handler starts only after the first one returns.
#[cfg(all(test, unix))]
pub(crate) mod tests {
    use super::*;
    use crate::gostd::context;
    use std::io::{Read as _, Write as _};
    use std::os::unix::net::UnixStream;
    use std::sync::Mutex;
    use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender};
    use std::time::Duration;

    // Go: ipc/conn_async_test.go:19 noOpHandler
    pub(crate) struct NoOpHandler;

    impl Handler for NoOpHandler {
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

    /// Go `nil` for the `io.ReadWriteCloser` of a connection.
    // PORT: the Rust transport is always set (see `dispatch`). The protocol
    // of these tests never uses it, and Go skips `Close` on nil, so `close`
    // does nothing.
    pub(crate) struct NilTransport;

    impl ReadWriteCloser for NilTransport {
        fn read(&self, _buf: &mut [u8]) -> std::io::Result<usize> {
            panic!("runtime error: invalid memory address or nil pointer dereference")
        }

        fn write(&self, _buf: &[u8]) -> std::io::Result<usize> {
            panic!("runtime error: invalid memory address or nil pointer dereference")
        }

        fn flush(&self) -> std::io::Result<()> {
            panic!("runtime error: invalid memory address or nil pointer dereference")
        }

        fn close(&self) -> Result<(), GoError> {
            Ok(())
        }
    }

    /// Go `errors.New("response write failed")`.
    pub(crate) fn response_write_failed() -> GoError {
        errors::new("response write failed")
    }

    /// Go `&ipc.Message{ID: id, Method: method}`.
    pub(crate) fn message(id: Option<jsonrpc::ID>, method: &str) -> Message {
        Message {
            id,
            method: method.to_string(),
            ..Default::default()
        }
    }

    // Go: ipc/conn_async_test.go:29 queuedProtocol
    struct QueuedProtocol {
        messages: Vec<Message>,
        response_err: Option<GoError>,
    }

    impl Protocol for QueuedProtocol {
        fn read_message(&mut self) -> Result<Message, GoError> {
            if self.messages.is_empty() {
                return Err(errors::EOF.clone());
            }
            Ok(self.messages.remove(0))
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
            self.response_err.clone().map_or(Ok(()), Err)
        }

        fn write_error(
            &mut self,
            _id: Option<&jsonrpc::ID>,
            _err: &jsonrpc::ResponseError,
        ) -> Result<(), GoError> {
            self.response_err.clone().map_or(Ok(()), Err)
        }
    }

    // Go: ipc/conn_async_test.go:59 blockingHandler
    // PORT: Go `started` is a buffered channel. Go `<-h.release` returns when
    // the test closes `release`; here the test drops the sender, and `recv`
    // returns.
    struct BlockingHandler {
        started: SyncSender<()>,
        release: Receiver<()>,
    }

    impl Handler for BlockingHandler {
        fn handle_request(
            &self,
            _ctx: &Context,
            _method: &str,
            _params: JsonValue,
        ) -> Result<Option<Box<dyn AnyValue>>, GoError> {
            self.started.send(()).expect("started");
            let _ = self.release.recv();
            Ok(None)
        }

        fn handle_notification(
            &self,
            _ctx: &Context,
            _method: &str,
            _params: JsonValue,
        ) -> Result<(), GoError> {
            self.started.send(()).expect("started");
            let _ = self.release.recv();
            Ok(())
        }
    }

    // Go: ipc/conn_async_test.go:76 contextHandler
    // PORT: Go waits for the handler context to be done and returns its
    // error. The Rust handler runs inline, before `run` reads the EOF that
    // cancels the context, so a wait would never end. The handler keeps its
    // context instead, and the test checks it after `run` returns.
    struct ContextHandler {
        contexts: Sender<Context>,
    }

    impl Handler for ContextHandler {
        fn handle_request(
            &self,
            ctx: &Context,
            _method: &str,
            _params: JsonValue,
        ) -> Result<Option<Box<dyn AnyValue>>, GoError> {
            self.contexts.send(ctx.clone()).expect("contexts");
            ctx.err().map_or(Ok(None), Err)
        }

        fn handle_notification(
            &self,
            ctx: &Context,
            _method: &str,
            _params: JsonValue,
        ) -> Result<(), GoError> {
            self.contexts.send(ctx.clone()).expect("contexts");
            ctx.err().map_or(Ok(()), Err)
        }
    }

    // Go: ipc/conn_async_test.go:88 TestAsyncConnRunWaitsForHandlers
    // PORT: Go receives both `started` signals before it checks `Run`. Here
    // the notification handler starts only after the request handler
    // returns, so the test checks `Run` after the first signal and receives
    // the second one after it closes `release`.
    #[test]
    fn test_async_conn_run_waits_for_handlers() {
        let (started, started_rx) = mpsc::sync_channel(2);
        let (release, release_rx) = mpsc::channel::<()>();
        let (run_done, run_done_rx) = mpsc::sync_channel(1);
        let runner = std::thread::spawn(move || {
            let protocol = QueuedProtocol {
                messages: vec![
                    message(Some(jsonrpc::new_id_string("1")), "request"),
                    message(None, "notification"),
                ],
                response_err: None,
            };
            let handler = BlockingHandler {
                started,
                release: release_rx,
            };
            let conn = new_async_conn_with_protocol(
                Arc::new(NilTransport),
                Box::new(protocol),
                Rc::new(handler),
            );
            let result = conn.run(&context::background());
            run_done
                .send(result.map_err(|err| err.error()))
                .expect("runDone");
        });

        started_rx.recv().expect("<-handler.started");
        let run_returned = run_done_rx.try_recv().is_ok();
        assert!(!run_returned, "Run returned while handlers were active");

        drop(release);
        started_rx.recv().expect("<-handler.started");
        let result = run_done_rx.recv().expect("<-runDone");
        assert!(result.is_ok(), "{result:?}");
        runner.join().expect("run thread");
    }

    // Go: ipc/conn_async_test.go:120 TestAsyncConnRunCancelsHandlersOnEOF
    // PORT: see `ContextHandler`. Go's handler returns once `Run` cancels its
    // context at EOF; here the test checks that `run` returned nil within the
    // same limit and cancelled the context that the handler got.
    #[test]
    fn test_async_conn_run_cancels_handlers_on_eof() {
        let (contexts, contexts_rx) = mpsc::channel();
        let (run_done, run_done_rx) = mpsc::sync_channel(1);
        let runner = std::thread::spawn(move || {
            let protocol = QueuedProtocol {
                messages: vec![message(Some(jsonrpc::new_id_string("1")), "request")],
                response_err: None,
            };
            let conn = new_async_conn_with_protocol(
                Arc::new(NilTransport),
                Box::new(protocol),
                Rc::new(ContextHandler { contexts }),
            );
            let result = conn.run(&context::background());
            run_done
                .send(result.map_err(|err| err.error()))
                .expect("runDone");
        });

        match run_done_rx.recv_timeout(Duration::from_secs(1)) {
            Ok(result) => assert!(result.is_ok(), "{result:?}"),
            Err(_) => panic!("Run did not cancel active handlers after EOF"),
        }
        runner.join().expect("run thread");
        let handler_ctx = contexts_rx.recv().expect("the handler ran");
        let err = handler_ctx
            .err()
            .expect("Run did not cancel active handlers after EOF");
        assert!(
            errors::is(&err, &context::CANCELED),
            "expected context.Canceled, got {}",
            err.error()
        );
    }

    /// A `QueuedProtocol` whose read panics when the queue is empty.
    struct PanicAtEndProtocol(QueuedProtocol);

    impl Protocol for PanicAtEndProtocol {
        fn read_message(&mut self) -> Result<Message, GoError> {
            if self.0.messages.is_empty() {
                panic!("read panicked");
            }
            self.0.read_message()
        }

        fn write_request(
            &mut self,
            id: Option<&jsonrpc::ID>,
            method: &str,
            params: Option<Box<dyn AnyValue>>,
        ) -> Result<(), GoError> {
            self.0.write_request(id, method, params)
        }

        fn write_notification(
            &mut self,
            method: &str,
            params: Option<Box<dyn AnyValue>>,
        ) -> Result<(), GoError> {
            self.0.write_notification(method, params)
        }

        fn write_response(
            &mut self,
            id: Option<&jsonrpc::ID>,
            result: Option<Box<dyn AnyValue>>,
        ) -> Result<(), GoError> {
            self.0.write_response(id, result)
        }

        fn write_error(
            &mut self,
            id: Option<&jsonrpc::ID>,
            err: &jsonrpc::ResponseError,
        ) -> Result<(), GoError> {
            self.0.write_error(id, err)
        }
    }

    // PORT: no Go test. Go's deferred function in `Run` also runs while a
    // panic unwinds. When the loop panics, `run` must still close the
    // pending calls and cancel the context that a handler kept.
    #[test]
    fn test_async_conn_run_cancels_handlers_on_panic() {
        let (contexts, contexts_rx) = mpsc::channel();
        let protocol = PanicAtEndProtocol(QueuedProtocol {
            messages: vec![message(Some(jsonrpc::new_id_string("1")), "request")],
            response_err: None,
        });
        let conn = new_async_conn_with_protocol(
            Arc::new(NilTransport),
            Box::new(protocol),
            Rc::new(ContextHandler { contexts }),
        );

        let outcome = catch_unwind(AssertUnwindSafe(|| conn.run(&context::background())));
        assert!(outcome.is_err(), "run did not panic");

        let handler_ctx = contexts_rx.recv().expect("the handler ran");
        let err = handler_ctx
            .err()
            .expect("Run did not cancel active handlers after a panic");
        assert!(
            errors::is(&err, &context::CANCELED),
            "expected context.Canceled, got {}",
            err.error()
        );
        let err = conn
            .notify(&context::background(), "after", None)
            .expect_err("Run did not close the pending calls after a panic");
        assert!(
            errors::is(&err, &ERR_CONN_CLOSED),
            "expected ErrConnClosed, got {}",
            err.error()
        );
    }

    // Go: ipc/conn_async_test.go:138 TestAsyncConnResponseWriteFailureWithNilTransport
    #[test]
    fn test_async_conn_response_write_failure_with_nil_transport() {
        let response_err = response_write_failed();
        let protocol = QueuedProtocol {
            messages: vec![message(Some(jsonrpc::new_id_string("1")), "request")],
            response_err: Some(response_err.clone()),
        };
        let conn = new_async_conn_with_protocol(
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

    // Go: ipc/conn_async_test.go:153 closeSignal
    // PORT: Go closes the `closed` channel once; here `close` drops the
    // sender once, and the receiver's `recv` returns.
    struct CloseSignal {
        closed: Mutex<Option<Sender<()>>>,
    }

    impl ReadWriteCloser for CloseSignal {
        fn read(&self, _buf: &mut [u8]) -> std::io::Result<usize> {
            // Go returns io.EOF.
            Ok(0)
        }

        fn write(&self, buf: &[u8]) -> std::io::Result<usize> {
            Ok(buf.len())
        }

        fn flush(&self) -> std::io::Result<()> {
            Ok(())
        }

        fn close(&self) -> Result<(), GoError> {
            self.closed.lock().expect("closed").take();
            Ok(())
        }
    }

    // Go: ipc/conn_async_test.go:171 failingResponseProtocol
    struct FailingResponseProtocol {
        closed: Receiver<()>,
        request_read: bool,
        response_err: GoError,
    }

    impl Protocol for FailingResponseProtocol {
        fn read_message(&mut self) -> Result<Message, GoError> {
            if !self.request_read {
                self.request_read = true;
                return Ok(message(Some(jsonrpc::new_id_int(1)), "transform"));
            }
            let _ = self.closed.recv();
            // Go: io.ErrClosedPipe
            Err(errors::new("io: read/write on closed pipe"))
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

    /// One end of Go `net.Pipe()`.
    ///
    /// PORT: Go's Close ends both directions of both ends: the peer reads
    /// EOF and its writes fail. `close` shuts the socket down, which wakes a
    /// blocked read, and then closes it. A shutdown alone is not enough on
    /// macOS, where the peer can still write to a socket that only shut
    /// down.
    struct PipeEnd(std::sync::RwLock<Option<UnixStream>>);

    impl PipeEnd {
        fn new(stream: UnixStream) -> Self {
            PipeEnd(std::sync::RwLock::new(Some(stream)))
        }

        fn with_stream<R>(
            &self,
            f: impl FnOnce(&UnixStream) -> std::io::Result<R>,
        ) -> std::io::Result<R> {
            match &*self.0.read().unwrap() {
                Some(stream) => f(stream),
                None => Err(std::io::ErrorKind::NotConnected.into()),
            }
        }
    }

    impl ReadWriteCloser for PipeEnd {
        fn read(&self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.with_stream(|mut stream| stream.read(buf))
        }

        fn write(&self, buf: &[u8]) -> std::io::Result<usize> {
            self.with_stream(|mut stream| stream.write(buf))
        }

        fn flush(&self) -> std::io::Result<()> {
            self.with_stream(|mut stream| stream.flush())
        }

        fn close(&self) -> Result<(), GoError> {
            let _ = self.with_stream(|stream| stream.shutdown(std::net::Shutdown::Both));
            self.0.write().unwrap().take();
            Ok(())
        }
    }

    // Go: ipc/conn_async_test.go:202 TestAsyncConnCallReturnsWhenPeerCloses
    #[test]
    fn test_async_conn_call_returns_when_peer_closes() {
        let (client, server) = UnixStream::pair().expect("socket pair");
        let client: Arc<dyn ReadWriteCloser> = Arc::new(PipeEnd::new(client));
        let conn = new_async_conn(client.clone(), Rc::new(NoOpHandler));
        let ctx = context::background();

        // The peer reads the request and closes its end.
        let peer = std::thread::spawn(move || {
            let mut buffer = [0u8; 1024];
            let read = (&server).read(&mut buffer);
            drop(server);
            read.map(|_| ())
        });

        let call_err = conn
            .call(&ctx, "transform", None)
            .expect_err("call fails when the peer closes");
        peer.join().expect("peer thread").expect("server read");
        assert!(conn.run(&ctx).is_ok());
        assert!(
            errors::is(&call_err, &ERR_CONN_CLOSED),
            "expected ErrConnClosed, got {}",
            call_err.error()
        );
        client.close().expect("client close");
    }

    /// Answers every request with `true`.
    struct TrueHandler;

    impl Handler for TrueHandler {
        fn handle_request(
            &self,
            _ctx: &Context,
            _method: &str,
            _params: JsonValue,
        ) -> Result<Option<Box<dyn AnyValue>>, GoError> {
            Ok(Some(Box::new(true)))
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

    /// Reads one message with its `Content-Length` header.
    fn read_framed(stream: &UnixStream) -> String {
        let mut header = Vec::new();
        let mut byte = [0u8; 1];
        while !header.ends_with(b"\r\n\r\n") {
            (&*stream).read_exact(&mut byte).expect("header byte");
            header.push(byte[0]);
        }
        let header = String::from_utf8(header).expect("utf-8 header");
        let length: usize = header
            .trim()
            .strip_prefix("Content-Length: ")
            .and_then(|length| length.parse().ok())
            .expect("Content-Length header");
        let mut body = vec![0u8; length];
        (&*stream).read_exact(&mut body).expect("body");
        String::from_utf8(body).expect("utf-8 body")
    }

    fn write_framed(stream: &UnixStream, body: &str) {
        write!(&*stream, "Content-Length: {}\r\n\r\n{body}", body.len()).expect("write");
    }

    // PORT: not in Go, whose goroutines handle the request below while
    // `Call` waits. A client callback may answer a call only after its own
    // request returns (ts#64299), so the call handles that request inline.
    #[test]
    fn test_async_conn_call_handles_a_nested_request() {
        let (client, server) = UnixStream::pair().expect("socket pair");
        let conn = new_async_conn(Arc::new(PipeEnd::new(client)), Rc::new(TrueHandler));
        let peer = std::thread::spawn(move || {
            let call = read_framed(&server);
            write_framed(&server, r#"{"jsonrpc":"2.0","id":"n1","method":"nested"}"#);
            let nested = read_framed(&server);
            write_framed(&server, r#"{"jsonrpc":"2.0","id":"api1","result":1}"#);
            (call, nested)
        });
        let result = conn
            .call(&context::background(), "resolve", None)
            .expect("the call returns");
        let (call, nested) = peer.join().expect("peer thread");
        assert!(call.contains(r#""method":"resolve""#), "{call}");
        assert!(
            nested.contains(r#""id":"n1""#) && nested.contains(r#""result":true"#),
            "{nested}"
        );
        assert_eq!(result.0, b"1");
    }

    // Go: ipc/conn_async_test.go:224 TestAsyncConnCallAfterReadLoopFailureReturnsImmediately
    #[test]
    fn test_async_conn_call_after_read_loop_failure_returns_immediately() {
        let (client, server) = UnixStream::pair().expect("socket pair");
        let conn = new_async_conn(Arc::new(PipeEnd::new(client)), Rc::new(NoOpHandler));
        let background = context::background();

        (&server).write_all(b"oops\n").expect("server write");
        let run_err = conn.run(&background).expect_err("run fails");
        assert!(
            run_err.error().contains("invalid header"),
            "{}",
            run_err.error()
        );
        // PORT: Go drains the server end (`io.Copy(io.Discard, server)`) so a
        // write cannot block on `net.Pipe`; the socket buffer covers that here.

        let (ctx, cancel) = context::with_timeout(&background, Duration::from_secs(1));
        let err = conn
            .call(&ctx, "transform", None)
            .expect_err("call fails after the read loop ended");
        assert!(
            errors::is(&err, &ERR_CONN_CLOSED),
            "expected ErrConnClosed, got {}",
            err.error()
        );
        assert!(
            !errors::is(&err, &context::DEADLINE_EXCEEDED),
            "call waited for its context deadline: {}",
            err.error()
        );
        let err = conn
            .notify(&ctx, "changed", None)
            .expect_err("notify fails after the read loop ended");
        assert!(
            errors::is(&err, &ERR_CONN_CLOSED),
            "expected ErrConnClosed, got {}",
            err.error()
        );
        cancel();
        drop(server);
    }

    // Go: ipc/conn_async_test.go:247 TestAsyncConnTerminalErrorIncludesResponseWriteFailure
    #[test]
    fn test_async_conn_terminal_error_includes_response_write_failure() {
        let response_err = response_write_failed();
        let (closed, closed_rx) = mpsc::channel::<()>();
        let rwc = Arc::new(CloseSignal {
            closed: Mutex::new(Some(closed)),
        });
        let protocol = FailingResponseProtocol {
            closed: closed_rx,
            request_read: false,
            response_err: response_err.clone(),
        };
        let conn = new_async_conn_with_protocol(rwc, Box::new(protocol), Rc::new(NoOpHandler));
        let ctx = context::background();

        let err = conn
            .run(&ctx)
            .expect_err("run returns the response write error");
        assert!(
            errors::is(&err, &response_err),
            "expected response write error, got {}",
            err.error()
        );
        let err = conn
            .call(&ctx, "transform", None)
            .expect_err("call returns the terminal error");
        assert!(
            errors::is(&err, &response_err),
            "expected terminal response write error, got {}",
            err.error()
        );
        assert_eq!(err.error().matches(&response_err.error()).count(), 1);
    }

    // Go: ipc/conn_async_test.go:264 TestAsyncConnRunWaitsForRequestAfterPeerCloses
    #[test]
    fn test_async_conn_run_waits_for_request_after_peer_closes() {
        let (client, server) = UnixStream::pair().expect("socket pair");
        let (started, started_rx) = mpsc::sync_channel(1);
        let (release, release_rx) = mpsc::channel::<()>();
        let (run_done, run_done_rx) = mpsc::sync_channel(1);
        let runner = std::thread::spawn(move || {
            let handler = BlockingHandler {
                started,
                release: release_rx,
            };
            let conn = new_async_conn(Arc::new(PipeEnd::new(server)), Rc::new(handler));
            let ctx = context::background();
            let run = conn.run(&ctx).err().map(|err| err.error());
            // Go calls these after `Run` returns.
            let call = conn
                .call(&ctx, "transform", None)
                .err()
                .map(|err| err.error());
            let notify = conn
                .notify(&ctx, "changed", None)
                .err()
                .map(|err| err.error());
            run_done.send((run, call, notify)).expect("runDone");
        });

        let client: Arc<dyn ReadWriteCloser> = Arc::new(PipeEnd::new(client));
        let mut client_protocol = new_jsonrpc_protocol(client.clone());
        client_protocol
            .write_request(Some(&jsonrpc::new_id_int(1)), "transform", None)
            .expect("assert.NilError");
        if started_rx.recv_timeout(Duration::from_secs(1)).is_err() {
            panic!("request handler did not start");
        }
        client.close().expect("assert.NilError");

        let handler_blocked = match run_done_rx.recv_timeout(Duration::from_millis(100)) {
            Ok((run, _, _)) => {
                panic!("connection stopped while request handler was blocked: {run:?}")
            }
            Err(RecvTimeoutError::Timeout) => true,
            Err(RecvTimeoutError::Disconnected) => panic!("the run thread ended"),
        };
        assert!(handler_blocked);

        drop(release);
        let Ok((run, call, notify)) = run_done_rx.recv_timeout(Duration::from_secs(1)) else {
            panic!("connection did not stop after request handler completed");
        };
        for err in [run, call, notify] {
            let err = err.expect("assert.ErrorContains: nil error");
            assert!(err.contains("ipc: failed to write response"), "{err}");
        }
        runner.join().expect("run thread");
    }

    /// A `QueuedProtocol` whose read panics when its queue is empty and
    /// whose `write_request` panics when `write_request_panics` is set. It
    /// keeps the message of each error response it writes.
    struct PanickingProtocol {
        queued: QueuedProtocol,
        write_request_panics: bool,
        error_messages: Rc<RefCell<Vec<String>>>,
    }

    impl Protocol for PanickingProtocol {
        fn read_message(&mut self) -> Result<Message, GoError> {
            if self.queued.messages.is_empty() {
                panic!("read panicked");
            }
            self.queued.read_message()
        }

        fn write_request(
            &mut self,
            id: Option<&jsonrpc::ID>,
            method: &str,
            params: Option<Box<dyn AnyValue>>,
        ) -> Result<(), GoError> {
            if self.write_request_panics {
                panic!("write request panicked");
            }
            self.queued.write_request(id, method, params)
        }

        fn write_notification(
            &mut self,
            method: &str,
            params: Option<Box<dyn AnyValue>>,
        ) -> Result<(), GoError> {
            self.queued.write_notification(method, params)
        }

        fn write_response(
            &mut self,
            id: Option<&jsonrpc::ID>,
            result: Option<Box<dyn AnyValue>>,
        ) -> Result<(), GoError> {
            self.queued.write_response(id, result)
        }

        fn write_error(
            &mut self,
            id: Option<&jsonrpc::ID>,
            err: &jsonrpc::ResponseError,
        ) -> Result<(), GoError> {
            self.error_messages.borrow_mut().push(err.message.clone());
            self.queued.write_error(id, err)
        }
    }

    fn panicking_conn(
        messages: Vec<Message>,
        write_request_panics: bool,
        handler: Rc<dyn Handler>,
    ) -> (Rc<AsyncConn>, Rc<RefCell<Vec<String>>>) {
        let error_messages = Rc::new(RefCell::new(Vec::new()));
        let protocol = PanickingProtocol {
            queued: QueuedProtocol {
                messages,
                response_err: None,
            },
            write_request_panics,
            error_messages: error_messages.clone(),
        };
        let conn =
            new_async_conn_with_protocol(Arc::new(NilTransport), Box::new(protocol), handler);
        (conn, error_messages)
    }

    // PORT: no Go test. Go's deferred function in `Call` deletes the pending
    // entry also when `WriteRequest` panics. The read loop does not end.
    // PORT: Go locks `writeMu` around `WriteRequest` with no defer
    // (conn_async.go:281-283), so after this panic Go keeps `writeMu`
    // locked and a later `Notify` (:315) blocks forever. The port does not
    // port that deadlock: its write lock is the `protocol` RefCell borrow,
    // which the unwind releases, so the `notify` below succeeds.
    #[test]
    fn test_async_conn_call_deletes_pending_entry_when_write_request_panics() {
        let (conn, _) = panicking_conn(Vec::new(), true, Rc::new(NoOpHandler));
        let ctx = context::background();

        let outcome = catch_unwind(AssertUnwindSafe(|| conn.call(&ctx, "callback", None)));
        assert!(outcome.is_err(), "call did not panic");
        assert!(conn.pending.borrow().is_empty(), "the pending entry stays");
        assert!(conn.notify(&ctx, "after", None).is_ok());
    }

    /// Makes a call while it handles a request, and keeps the call's error.
    struct CallingHandler {
        conn: std::cell::OnceCell<std::rc::Weak<AsyncConn>>,
        call_err: RefCell<Option<GoError>>,
    }

    impl Handler for CallingHandler {
        fn handle_request(
            &self,
            ctx: &Context,
            _method: &str,
            _params: JsonValue,
        ) -> Result<Option<Box<dyn AnyValue>>, GoError> {
            let conn = self.conn.get().and_then(std::rc::Weak::upgrade);
            let conn = conn.expect("the connection is set");
            let err = conn
                .call(ctx, "callback", None)
                .expect_err("the call fails");
            *self.call_err.borrow_mut() = Some(err.clone());
            Err(err)
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

    // PORT: no Go test. A read in `call` is a read of Go's `Run`. When it
    // panics, Go's deferred function in `Run` closes the pending call, the
    // call returns ErrConnClosed, the handler's response is written, and
    // then the panic leaves `Run`.
    #[test]
    fn test_async_conn_call_read_panic_leaves_run() {
        let handler = Rc::new(CallingHandler {
            conn: std::cell::OnceCell::new(),
            call_err: RefCell::new(None),
        });
        let (conn, error_messages) = panicking_conn(
            vec![message(Some(jsonrpc::new_id_string("1")), "request")],
            false,
            handler.clone(),
        );
        let _ = handler.conn.set(Rc::downgrade(&conn));

        let outcome = catch_unwind(AssertUnwindSafe(|| conn.run(&context::background())));
        let payload = outcome.expect_err("run did not panic");
        assert_eq!(recovered_value(payload.as_ref()), "read panicked");
        let call_err = handler.call_err.borrow().clone().expect("the handler ran");
        assert!(
            errors::is(&call_err, &ERR_CONN_CLOSED),
            "expected ErrConnClosed, got {}",
            call_err.error()
        );
        assert!(conn.pending.borrow().is_empty(), "the pending entry stays");
        assert_eq!(*error_messages.borrow(), vec![call_err.error()]);
    }

    // PORT: no Go test. With no `run`, the panic of a read in `call` leaves
    // the call after Go's deferred function in `Run` closed the pending
    // calls. The read loop ended: a later `run` does not read again.
    #[test]
    fn test_async_conn_call_read_panic_without_run() {
        let (conn, _) = panicking_conn(Vec::new(), false, Rc::new(NoOpHandler));
        let ctx = context::background();

        let outcome = catch_unwind(AssertUnwindSafe(|| conn.call(&ctx, "callback", None)));
        let payload = outcome.expect_err("call did not panic");
        assert_eq!(recovered_value(payload.as_ref()), "read panicked");
        assert!(conn.pending.borrow().is_empty(), "the pending entry stays");
        let err = conn
            .notify(&ctx, "after", None)
            .expect_err("the read loop ended");
        assert!(
            errors::is(&err, &ERR_CONN_CLOSED),
            "expected ErrConnClosed, got {}",
            err.error()
        );
        // The protocol's queue is empty, so a second read would panic again.
        let run = catch_unwind(AssertUnwindSafe(|| conn.run(&ctx)));
        assert!(
            matches!(run, Ok(Ok(()))),
            "a later run read again or failed"
        );
    }

    /// Calls the client back with `call_ctx`, as the API's callback FS
    /// calls with the run context. `outer` makes a call. When it fails, the
    /// handler notes the error and whether its own context and the contexts
    /// that `inner` kept are done (it waits up to 1 s for each), makes a
    /// second call and returns that call's error. `inner` keeps its
    /// context. `wait` notes whether its context is done within 1 s. Other
    /// requests answer `true`.
    struct CallbackHandler {
        conn: std::cell::OnceCell<std::rc::Weak<AsyncConn>>,
        call_ctx: Context,
        inner: RefCell<Vec<Context>>,
        notes: Sender<String>,
    }

    impl Handler for CallbackHandler {
        fn handle_request(
            &self,
            ctx: &Context,
            method: &str,
            _params: JsonValue,
        ) -> Result<Option<Box<dyn AnyValue>>, GoError> {
            let done = |ctx: &Context| {
                ctx.done()
                    .is_some_and(|done| done.wait_timeout(Duration::from_secs(1)))
            };
            match method {
                "outer" => {}
                "inner" => {
                    self.inner.borrow_mut().push(ctx.clone());
                    return Ok(None);
                }
                "wait" => {
                    let note = format!("wait | done {}", done(ctx));
                    self.notes.send(note).expect("notes");
                    return Ok(None);
                }
                _ => return Ok(Some(Box::new(true))),
            }
            let conn = self.conn.get().and_then(std::rc::Weak::upgrade);
            let conn = conn.expect("the connection is set");
            let Err(err) = conn.call(&self.call_ctx, "callback", None) else {
                return Ok(None);
            };
            let inner: Vec<bool> = self.inner.borrow().iter().map(done).collect();
            let note = format!("{} | done {} | inner {inner:?}", err.error(), done(ctx));
            self.notes.send(note).expect("notes");
            conn.call(&self.call_ctx, "callback", None).map(|_| None)
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

    /// Runs a connection that reads on its thread (`read_on_thread`) over
    /// one end of a socket pair, with `CallbackHandler`, on a thread of its
    /// own. Returns the other end, the handler's notes and the result of
    /// `run`, which the thread sends when `run` returns.
    fn run_on_reader(
        ctx: &Context,
    ) -> (
        UnixStream,
        Receiver<String>,
        Receiver<Result<(), String>>,
        std::thread::JoinHandle<()>,
    ) {
        let (client, server) = UnixStream::pair().expect("socket pair");
        let (notes, notes_rx) = mpsc::channel();
        let (run_done, run_done_rx) = mpsc::sync_channel(1);
        let ctx = ctx.clone();
        let runner = std::thread::spawn(move || {
            let server: Arc<dyn ReadWriteCloser> = Arc::new(PipeEnd::new(server));
            let handler = Rc::new(CallbackHandler {
                conn: std::cell::OnceCell::new(),
                call_ctx: ctx.clone(),
                inner: RefCell::new(Vec::new()),
                notes,
            });
            let conn = new_async_conn(server.clone(), handler.clone());
            let _ = handler.conn.set(Rc::downgrade(&conn));
            conn.read_on_thread(Box::new(new_jsonrpc_protocol(server)));
            let result = conn.run(&ctx).map_err(|err| err.error());
            run_done.send(result).expect("runDone");
        });
        (client, notes_rx, run_done_rx, runner)
    }

    // PORT: no Go test; Go `Run` and `Call` (ipc/conn_async.go:83-87,
    // :262-303). After the context is cancelled, a call that waits returns
    // `context canceled` at once, and a new call writes its request and
    // returns it too (Phase A). `run` reads 1 more message, and Go's
    // deferred function sets `terminal` before that request makes its
    // calls, so they return it and write nothing (Phase B). The message
    // after it is not read.
    #[test]
    fn test_async_conn_reader_cancel_phases() {
        let (ctx, cancel) = context::with_cancel(&context::background());
        let (client, notes, run_done, runner) = run_on_reader(&ctx);
        write_framed(&client, r#"{"jsonrpc":"2.0","id":1,"method":"outer"}"#);
        let call = read_framed(&client);
        assert!(
            call.contains(r#""id":"api1","method":"callback""#),
            "{call}"
        );

        cancel();
        let call = read_framed(&client);
        assert!(
            call.contains(r#""id":"api2","method":"callback""#),
            "{call}"
        );
        let answer = read_framed(&client);
        assert!(
            answer.contains(r#""id":1,"#) && answer.contains(r#""message":"context canceled""#),
            "{answer}"
        );
        let note = notes.recv().expect("note");
        assert!(note.starts_with("context canceled |"), "{note}");
        assert!(
            run_done.recv_timeout(Duration::from_millis(100)).is_err(),
            "run returned before the next message"
        );

        let two = r#"{"jsonrpc":"2.0","id":2,"method":"outer"}"#;
        let three = r#"{"jsonrpc":"2.0","id":3,"method":"ping"}"#;
        let both = format!(
            "Content-Length: {}\r\n\r\n{two}Content-Length: {}\r\n\r\n{three}",
            two.len(),
            three.len()
        );
        (&client).write_all(both.as_bytes()).expect("write");
        let result = run_done
            .recv_timeout(Duration::from_secs(5))
            .expect("run returned");
        assert_eq!(result, Err("context canceled".to_string()));
        runner.join().expect("run thread");
        let note = notes.recv().expect("note");
        assert!(
            note.starts_with("ipc: connection closed\ncontext canceled |"),
            "{note}"
        );
        let answer = read_framed(&client);
        assert!(
            answer.contains(r#""id":2,"#)
                && answer.contains(r#""message":"ipc: connection closed\ncontext canceled""#),
            "{answer}"
        );
        let mut rest = Vec::new();
        (&client).read_to_end(&mut rest).expect("read");
        assert_eq!(String::from_utf8_lossy(&rest), "");
    }

    // PORT: no Go test; Go `Run` (ipc/conn_async.go:71-74, :89-90). EOF
    // while a call waits ends the read loop: the call returns
    // `ipc: connection closed`, and the deferred function has cancelled
    // `handlerCtx`, which every request gets, also one that came while the
    // call waited.
    #[test]
    fn test_async_conn_reader_eof_cancels_handlers() {
        let ctx = context::background();
        let (client, notes, run_done, runner) = run_on_reader(&ctx);
        write_framed(&client, r#"{"jsonrpc":"2.0","id":1,"method":"outer"}"#);
        let call = read_framed(&client);
        assert!(call.contains(r#""id":"api1""#), "{call}");
        write_framed(&client, r#"{"jsonrpc":"2.0","id":2,"method":"inner"}"#);
        let answer = read_framed(&client);
        assert!(answer.contains(r#""id":2,"result":null"#), "{answer}");
        client
            .shutdown(std::net::Shutdown::Write)
            .expect("shutdown");

        let note = notes
            .recv_timeout(Duration::from_secs(5))
            .expect("the call returned");
        assert_eq!(note, "ipc: connection closed | done true | inner [true]");
        let result = run_done
            .recv_timeout(Duration::from_secs(5))
            .expect("run returned");
        assert_eq!(result, Ok(()));
        runner.join().expect("run thread");
        let answer = read_framed(&client);
        assert!(
            answer.contains(r#""id":1,"#)
                && answer.contains(r#""message":"ipc: connection closed""#),
            "{answer}"
        );
    }

    // PORT: no Go test; Go `Run` (ipc/conn_async.go:89-96). Go's `Run`
    // hands a reply to its call before it reads EOF, so a call takes the
    // messages that the reader thread read before its end, then the end.
    #[test]
    fn test_async_conn_reader_call_takes_messages_before_the_end() {
        let inbox = Inbox::default();
        let reply = message(Some(jsonrpc::new_id_string("api1")), "");
        inbox.push(Some(reply), Some(ReadEnd::Returned(Ok(()))));
        let ctx = context::background();
        assert!(inbox.wait(Some(&ctx)).is_some(), "the call got no reply");
        assert!(inbox.wait(Some(&ctx)).is_none(), "the end is lost");
    }

    // PORT: no Go test; Go `Run` (ipc/conn_async.go:73, :89-90). EOF while
    // a handler runs with no call cancels its context at once, so a long
    // request can stop early.
    #[test]
    fn test_async_conn_reader_eof_cancels_a_running_handler() {
        let ctx = context::background();
        let (client, notes, run_done, runner) = run_on_reader(&ctx);
        write_framed(&client, r#"{"jsonrpc":"2.0","id":1,"method":"wait"}"#);
        client
            .shutdown(std::net::Shutdown::Write)
            .expect("shutdown");
        let note = notes
            .recv_timeout(Duration::from_secs(5))
            .expect("the handler ran");
        assert_eq!(note, "wait | done true");
        let result = run_done
            .recv_timeout(Duration::from_secs(5))
            .expect("run returned");
        assert_eq!(result, Ok(()));
        runner.join().expect("run thread");
    }
}
