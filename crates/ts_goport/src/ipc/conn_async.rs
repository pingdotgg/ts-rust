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
//!   `Run` returns), it cancels `handlerCtx` (Go's deferred
//!   `cancelHandlers`), and the dispatch thread runs Go's deferred
//!   `closePendingCalls` before the next `Call` or `Notify`: `terminal` is
//!   set, and the call returns it and writes nothing.
//!
//!   The Go clock: Go's `terminal` check (:263) depends only on when a
//!   goroutine makes its call, before or after `Run` returned. Here a
//!   request that waits in a call while a nested request runs is held
//!   until that request returns, and a request read while another one
//!   runs starts when it returns. So each request and notification that
//!   the dispatch thread runs is a frame with a lag: the time that the port
//!   held it while Go ran it. Its Go time is the port time less the lag. A
//!   `Call` or `Notify` whose Go time is before the end of the read loop
//!   finds Go's `terminal` nil: it writes, and a call returns what Go's
//!   `select` gave (`call_before_the_end`). Without a hold the lag is 0.
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
use std::time::{Duration, Instant};

/// Go `chan *Message` with capacity 1 for one pending server-to-client call.
/// PORT: with the instant at which the reader thread read the response
/// (`read_on_thread`; `None` without it), for the Go clock.
type ResponseChan = Rc<RefCell<Option<(Message, Option<Instant>)>>>;

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
    // PORT: the Go clock (file header): the lag of each request and
    // notification that runs on the dispatch stack, the innermost last.
    frames: RefCell<Vec<Duration>>,
    // PORT: when the reader thread ended the read loop, if that end set
    // `terminal` (`end_read_loop`).
    end_terminal: Cell<Option<Instant>>,
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
        frames: RefCell::new(Vec::new()),
        end_terminal: Cell::new(None),
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
                let inbox = start_reader(ctx, cancel_handlers, protocol);
                let reader = Rc::new(RunReader {
                    _cancel_waker: inbox.record_cancel(ctx),
                    inbox,
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

            self.dispatch(handler_ctx, msg, None);
        }
    }

    /// The loop of Go `Run` when `run` reads on its thread: the dispatch
    /// of each message that the thread read, in order. It ends when the
    /// thread ended and no message is left, and returns what Go's `Run`
    /// returned (or resumes the panic of its read).
    fn run_loop_on_reader(&self, reader: &RunReader) -> Result<(), GoError> {
        loop {
            let entered = Instant::now();
            let Some((msg, read_at)) = reader.inbox.next_for_run() else {
                break;
            };
            self.dispatch_read(reader, msg, read_at, entered);
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
        let Some((end, ended_at)) = lock(&reader.inbox.state).end.take() else {
            return;
        };
        match end {
            ReadEnd::Returned(result) => {
                // The Go clock: this end sets `terminal` at `ended_at`,
                // unless a request error set it before.
                if self.terminal.borrow().is_none() {
                    self.end_terminal.set(Some(ended_at));
                }
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
    /// when `call` read them. `read_at`: when the reader thread read the
    /// message (`read_on_thread`).
    fn dispatch(&self, ctx: &Context, msg: Message, read_at: Option<Instant>) {
        if msg.is_response() {
            self.handle_response(msg, read_at);
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

    /// PORT: the dispatch of a message that the reader thread read at
    /// `read_at`, when the dispatch thread looked for it at `entered`. A
    /// request or a notification runs as a frame of the Go clock (file
    /// header). Its lag starts with the time that it waited for the
    /// dispatch thread, which was busy from before `read_at` until
    /// `entered`: Go started its goroutine at once. Returns whether a frame
    /// ran.
    fn dispatch_read(
        &self,
        reader: &RunReader,
        msg: Message,
        read_at: Instant,
        entered: Instant,
    ) -> bool {
        if msg.is_response() {
            self.handle_response(msg, Some(read_at));
            return false;
        }
        let _frame = Frame::push(self, entered.saturating_duration_since(read_at));
        self.dispatch(&reader.handler_ctx, msg, Some(read_at));
        true
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
    fn handle_response(&self, msg: Message, read_at: Option<Instant>) {
        let Some(id) = msg.id.clone() else {
            // Go dereferences the nil ID and panics; responses always have one.
            panic!("runtime error: invalid memory address or nil pointer dereference");
        };
        let ch = self.pending.borrow_mut().remove(&id);

        if let Some(ch) = ch {
            *ch.borrow_mut() = Some((msg, read_at));
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
            if let Some(reader) = &reader
                && self.frame_before_the_end().is_some()
            {
                return self.call_before_the_end(ctx, &id, method, params, reader, err);
            }
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
            let wrote = Instant::now();
            return self.wait_on_reader(ctx, &id, &response_chan, reader, wrote);
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
            if let Some((resp, _)) = resp {
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
            self.dispatch(ctx, msg, None);
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
    /// header). It returns what wakes Go's `select` first
    /// (`Inbox::next_for_call`). A call made when `ctx` is already done
    /// returns `ctx.Err()` right after its write. `wrote`: when the call
    /// wrote its request.
    fn wait_on_reader(
        &self,
        ctx: &Context,
        id: &jsonrpc::ID,
        response_chan: &ResponseChan,
        reader: &RunReader,
        wrote: Instant,
    ) -> Result<JsonValue, GoError> {
        let _waker = reader.inbox.wake_on_done(ctx);
        // When the last request or notification that this wait ran nested
        // returned: it held the call.
        let mut held_until = None;
        let (result, woke) = loop {
            let resp = response_chan.borrow_mut().take();
            if let Some((resp, read_at)) = resp {
                break (response_result(resp), read_at.map(Woke::Reply));
            }
            if !self.pending.borrow().contains_key(id) {
                // Go: a channel that closes after `ctx` is done loses to
                // `ctx.Done()`. This happens when the read loop ends after
                // a signal (at the 1 message read after it, at EOF or at a
                // read error) while a nested request runs, or after a
                // request error.
                if let Some(err) = ctx.err()
                    && !reader.inbox.ended_before_cancel()
                {
                    break (Err(err), Some(Woke::Canceled(reader.inbox.canceled_at())));
                }
                let terminal = self.terminal.borrow().clone();
                let woke = self.end_terminal.get().map(Woke::Ended);
                break (
                    Err(terminal.expect("closePendingCalls sets terminal")),
                    woke,
                );
            }
            let entered = Instant::now();
            match reader.inbox.next_for_call(ctx) {
                CallStep::Message(msg, read_at) => {
                    if self.dispatch_read(reader, msg, read_at, entered) {
                        held_until = Some(Instant::now());
                    }
                }
                CallStep::Canceled(err) => {
                    break (Err(err), Some(Woke::Canceled(reader.inbox.canceled_at())));
                }
                CallStep::Ended => self.end_read_loop(reader),
            }
        };
        if let Some(woke) = woke {
            self.advance_clock(wrote, woke, held_until);
        }
        result
    }

    /// PORT: the end of the read loop (`end_terminal`) when the running
    /// frame's Go time (the port time less its lag) is before it: Go's
    /// `terminal` was still nil when that frame made this call or notify
    /// (the Go clock, file header).
    fn frame_before_the_end(&self) -> Option<Instant> {
        let end = self.end_terminal.get()?;
        let lag = self.frames.borrow().last().copied()?;
        (Instant::now().saturating_duration_since(end) < lag).then_some(end)
    }

    /// PORT: a `Call` that Go made before `Run` returned
    /// (`frame_before_the_end`). Go's `terminal` was nil (:263), so
    /// it writes its request (:281), and its `select` (:289) returns
    /// `ctx.Err()` when `ctx` was done before the end (a signal), or else
    /// `terminal` when the end closes its channel. No response can come:
    /// the reader thread has ended.
    fn call_before_the_end(
        &self,
        ctx: &Context,
        id: &jsonrpc::ID,
        method: &str,
        params: Option<Box<dyn AnyValue>>,
        reader: &RunReader,
        terminal: GoError,
    ) -> Result<JsonValue, GoError> {
        let end = self.end_terminal.get().expect("the end set terminal");
        self.protocol
            .borrow_mut()
            .write_request(Some(id), method, params)?;
        let wrote = Instant::now();
        let (err, woke) = match ctx.err() {
            Some(err) if !reader.inbox.ended_before_cancel() => {
                (err, Woke::Canceled(reader.inbox.canceled_at()))
            }
            _ => (terminal, Woke::Ended(end)),
        };
        self.advance_clock(wrote, woke, None);
        Err(err)
    }

    /// PORT: the Go clock (file header) when a call of the running frame
    /// returns. It wrote its request at `wrote`, and `woke` woke Go's
    /// `select`, so Go's `Call` returned at max(`wrote` - lag, the Go time
    /// of `woke`). The port returns at max(`wrote`, `woke`, `held_until`):
    /// it does not count the dispatch thread's own wake when it waited
    /// idle, only a nested request that held it. The new lag is the
    /// difference.
    fn advance_clock(&self, wrote: Instant, woke: Woke, held_until: Option<Instant>) {
        let mut frames = self.frames.borrow_mut();
        let Some(lag) = frames.last_mut() else {
            return;
        };
        let (Woke::Reply(at) | Woke::Canceled(at) | Woke::Ended(at)) = woke;
        let returned = wrote.max(at).max(held_until.unwrap_or(wrote));
        let since = |t: Instant| returned.saturating_duration_since(t);
        *lag = match woke {
            // The client answered the port's write, which Go made `lag`
            // earlier, so Go got the reply `lag` earlier too.
            Woke::Reply(_) => since(wrote.max(at)) + *lag,
            // A signal and the end of the read loop come at the same time
            // in Go and here.
            Woke::Canceled(_) | Woke::Ended(_) => (since(wrote) + *lag).min(since(at)),
        };
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
        // PORT: Go's `terminal` was nil when the frame made a notify before
        // the end of the read loop (`frame_before_the_end`).
        if let Some(err) = terminal
            && self.frame_before_the_end().is_none()
        {
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

/// PORT: what woke the `select` of a `Call` (:289) and when, for the Go
/// clock (`AsyncConn::advance_clock`).
enum Woke {
    /// The response, which the reader thread read then.
    Reply(Instant),
    /// `ctx.Done()`, done then (a signal).
    Canceled(Instant),
    /// The channel, which the end of the read loop closed then.
    Ended(Instant),
}

/// PORT: a frame of the Go clock on the dispatch stack
/// (`AsyncConn::dispatch_read`). Drop pops it, also when the dispatch
/// unwinds.
struct Frame<'a> {
    conn: &'a AsyncConn,
    depth: usize,
}

impl<'a> Frame<'a> {
    fn push(conn: &'a AsyncConn, lag: Duration) -> Frame<'a> {
        let mut frames = conn.frames.borrow_mut();
        frames.push(lag);
        Frame {
            conn,
            depth: frames.len(),
        }
    }
}

impl Drop for Frame<'_> {
    fn drop(&mut self) {
        let mut frames = self.conn.frames.borrow_mut();
        debug_assert_eq!(frames.len(), self.depth, "the frames nest");
        frames.pop();
    }
}

/// PORT: what `run` keeps while its reader thread reads
/// (`read_on_thread`).
struct RunReader {
    inbox: Arc<Inbox>,
    /// Go `handlerCtx`. A request that a `call` dispatches gets it too.
    handler_ctx: Context,
    /// Records when the `ctx` of `run` is done (`Inbox::record_cancel`).
    _cancel_waker: DoneWaker,
}

/// PORT: the messages that the reader thread read, and how its loop ended.
#[derive(Default)]
struct Inbox {
    state: Mutex<InboxState>,
    ready: Condvar,
}

/// The messages carry the instant at which the thread read them, for the
/// Go clock.
#[derive(Default)]
struct InboxState {
    /// The messages read before `ctx` was done, in order.
    messages: VecDeque<(Message, Instant)>,
    /// The message read after `ctx` was done (the 1 read after a signal).
    /// `run` dispatches it after `messages`. It was read when the loop
    /// ended.
    after_cancel: Option<(Message, Instant)>,
    /// True once the thread ended (Go's `Run` returned).
    ended: bool,
    /// True when `ctx` was done when the loop ended: at its check of `ctx`
    /// (after a signal), or at a read that ended after the signal (EOF, a
    /// read error or a panic). In Go's `select`, a waiting call's
    /// `ctx.Done()` then came before its channel closed.
    ended_after_cancel: bool,
    /// How and when the loop ended, until `end_read_loop` takes it.
    end: Option<(ReadEnd, Instant)>,
    /// When the `ctx` of `run` was done (`Inbox::record_cancel`).
    canceled_at: Option<Instant>,
}

/// How Go's `Run` loop ended: it returned this value, or its read panicked.
enum ReadEnd {
    Returned(Result<(), GoError>),
    Panicked(Box<dyn Any + Send>),
}

/// What a waiting `call` does next (`Inbox::next_for_call`).
enum CallStep {
    /// Dispatch this message, read then.
    Message(Message, Instant),
    /// Return this error of the call's `ctx`.
    Canceled(GoError),
    /// The read loop ended: run `end_read_loop`, which closes the call.
    Ended,
}

impl Inbox {
    /// The next message for `run`, waiting as needed: `messages` in order,
    /// then `after_cancel`. `None` once the thread ended and no message is
    /// left.
    fn next_for_run(&self) -> Option<(Message, Instant)> {
        let mut state = lock(&self.state);
        loop {
            if let Some(msg) = state
                .messages
                .pop_front()
                .or_else(|| state.after_cancel.take())
            {
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

    /// The next step of a `call` that waits, in the order in which the
    /// cases wake Go's `select`, waiting as needed:
    /// - A message read before `ctx` was done. Go's `Run` handed a response
    ///   to its call before `ctx.Done()`. When `ctx` is done, only the
    ///   responses are taken: Go's goroutines run the requests, here `run`
    ///   does.
    /// - The end of a loop that ended before `ctx` was done (EOF, a read
    ///   error): Go's `closePendingCalls` runs before `cancelHandlers`.
    /// - `ctx` done. After a signal the loop ends only at its next read (1
    ///   more message, EOF or a read error), so `ctx.Done()` comes first.
    fn next_for_call(&self, ctx: &Context) -> CallStep {
        let mut state = lock(&self.state);
        loop {
            let canceled = ctx.err();
            let next = state
                .messages
                .iter()
                .position(|(msg, _)| canceled.is_none() || msg.is_response());
            if let Some((msg, read_at)) = next.and_then(|i| state.messages.remove(i)) {
                return CallStep::Message(msg, read_at);
            }
            if state.ended && !state.ended_after_cancel {
                return CallStep::Ended;
            }
            if let Some(err) = canceled {
                return CallStep::Canceled(err);
            }
            if state.ended {
                return CallStep::Ended;
            }
            state = self
                .ready
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    /// Whether the loop ended before `ctx` was done (EOF, a read error or
    /// a panic before a signal).
    fn ended_before_cancel(&self) -> bool {
        let state = lock(&self.state);
        state.ended && !state.ended_after_cancel
    }

    /// When the `ctx` of `run` was done. Now when its waker has not run
    /// yet (it runs right after `ctx` is done).
    fn canceled_at(&self) -> Instant {
        lock(&self.state).canceled_at.unwrap_or_else(Instant::now)
    }

    /// Records in `canceled_at` when `ctx` (of `run`) is done, until the
    /// result drops.
    fn record_cancel(self: &Arc<Self>, ctx: &Context) -> DoneWaker {
        let Some(done) = ctx.done() else {
            return DoneWaker(None);
        };
        let inbox = self.clone();
        let id = done.register_waker(move || {
            lock(&inbox.state)
                .canceled_at
                .get_or_insert_with(Instant::now);
        });
        if id.is_none() {
            // Done before `run`.
            lock(&self.state)
                .canceled_at
                .get_or_insert_with(Instant::now);
        }
        DoneWaker(id.map(|id| (done, id)))
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

    /// A message, read just now.
    fn push(&self, msg: Message) {
        let read_at = Instant::now();
        lock(&self.state).messages.push_back((msg, read_at));
        self.ready.notify_all();
    }

    /// The end of the loop just now, with the message read after `ctx` was
    /// done when there is one. `ctx_done`: `ctx` was done when the loop
    /// ended.
    fn end(&self, after_cancel: Option<Message>, end: ReadEnd, ctx_done: bool) {
        let ended_at = Instant::now();
        let mut state = lock(&self.state);
        state.after_cancel = after_cancel.map(|msg| (msg, ended_at));
        state.ended = true;
        state.ended_after_cancel = ctx_done;
        state.end = Some((end, ended_at));
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
            let (after_cancel, end) = loop {
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
                // loop with the message read after a signal.
                if let Some(err) = ctx.err() {
                    break (Some(msg), ReadEnd::Returned(Err(err)));
                }
                thread_inbox.push(msg);
            };
            // `ctx` is done also when a read ended after a signal (EOF
            // while a nested request runs): Go's waiting call returned at
            // the signal, before this end.
            thread_inbox.end(after_cancel, end, ctx.err().is_some());
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
    /// context. `wait` notes whether its context is done within 1 s. `late`
    /// makes a call, waits until the read loop ended, makes a second call,
    /// notes both errors and returns the second. `calls` makes 3 calls and
    /// notes their results; `slowcalls` too, but it waits `HOLD` after the
    /// first. `hcall` makes a call with its own context (the handler
    /// context, as the API's module resolver) and notes the result. `lags`
    /// makes 2 calls and notes its frame's lag after each. `hold` waits
    /// `HOLD` and does not look at its context. Other requests answer
    /// `true`.
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
            let conn = || {
                let conn = self.conn.get().and_then(std::rc::Weak::upgrade);
                conn.expect("the connection is set")
            };
            let text = |result: Result<JsonValue, GoError>| match result {
                Ok(_) => "ok".to_string(),
                Err(err) => err.error(),
            };
            match method {
                "outer" | "late" => {}
                "hold" => {
                    std::thread::sleep(HOLD);
                    return Ok(None);
                }
                "calls" | "slowcalls" => {
                    let conn = conn();
                    let mut results = Vec::new();
                    for i in 0..3 {
                        if i == 1 && method == "slowcalls" {
                            std::thread::sleep(HOLD);
                        }
                        results.push(text(conn.call(&self.call_ctx, "callback", None)));
                    }
                    let note = format!("{method} | {results:?}");
                    self.notes.send(note).expect("notes");
                    return Ok(None);
                }
                "lags" => {
                    let conn = conn();
                    let mut lags = Vec::new();
                    for _ in 0..2 {
                        let _ = conn.call(&self.call_ctx, "callback", None);
                        lags.push(conn.frames.borrow().last().copied());
                    }
                    self.notes.send(format!("lags | {lags:?}")).expect("notes");
                    return Ok(None);
                }
                "hcall" => {
                    let result = text(conn().call(ctx, "callback", None));
                    self.notes.send(format!("hcall | {result}")).expect("notes");
                    return Ok(None);
                }
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
            let conn = conn();
            if method == "late" {
                let first = conn.call(&self.call_ctx, "callback", None).err();
                let reader = conn
                    .reader
                    .borrow()
                    .clone()
                    .expect("run reads on its thread");
                let end = Instant::now() + Duration::from_secs(5);
                while !lock(&reader.inbox.state).ended && Instant::now() < end {
                    std::thread::sleep(Duration::from_millis(1));
                }
                let second = conn.call(&self.call_ctx, "callback", None).err();
                let texts = [&first, &second].map(|err| err.as_ref().map(GoError::error));
                self.notes.send(format!("late | {texts:?}")).expect("notes");
                return second.map_or(Ok(None), Err);
            }
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
        inbox.push(message(Some(jsonrpc::new_id_string("api1")), ""));
        inbox.end(None, ReadEnd::Returned(Ok(())), false);
        let ctx = context::background();
        let step = inbox.next_for_call(&ctx);
        assert!(
            matches!(step, CallStep::Message(..)),
            "the call got no reply"
        );
        let step = inbox.next_for_call(&ctx);
        assert!(matches!(step, CallStep::Ended), "the end is lost");
    }

    // PORT: no Go test; Go `Run` and `Call` (ipc/conn_async.go:83-110,
    // :289-303). After a signal, a waiting call still gets a response that
    // `Run` read before the signal, and then returns `context canceled`.
    // A request read before the signal stays for `run`, and a response read
    // after it (the 1 read after a signal) is too late.
    #[test]
    fn test_async_conn_reader_call_after_cancel_takes_only_earlier_responses() {
        let inbox = Inbox::default();
        inbox.push(message(Some(jsonrpc::new_id_string("n1")), "nested"));
        inbox.push(message(Some(jsonrpc::new_id_string("api1")), ""));
        let (ctx, cancel) = context::with_cancel(&context::background());
        cancel();
        let late = message(Some(jsonrpc::new_id_string("api2")), "");
        let err = ctx.err().expect("ctx is done");
        inbox.end(Some(late), ReadEnd::Returned(Err(err)), true);
        let step = inbox.next_for_call(&ctx);
        assert!(
            matches!(&step, CallStep::Message(msg, _) if msg.is_response()),
            "the earlier response is lost"
        );
        let step = inbox.next_for_call(&ctx);
        assert!(matches!(step, CallStep::Canceled(_)), "no context error");
        assert!(!inbox.ended_before_cancel());
        // `run` gets the request, then the late response.
        let responses: Vec<bool> = std::iter::from_fn(|| inbox.next_for_run())
            .map(|(msg, _)| msg.is_response())
            .collect();
        assert_eq!(responses, [false, true]);
    }

    // PORT: no Go test; Go `Call` (ipc/conn_async.go:289-303). A call that
    // waits at a signal returns `context canceled`, also when a request that
    // it dispatched (nested, as the port runs them) ends the read loop with
    // a later call: Go's `Call` returned at the signal, before its channel
    // closed. The apisig3 repair answered `ipc: connection closed` here.
    #[test]
    fn test_async_conn_reader_cancel_beats_a_later_close() {
        let (ctx, cancel) = context::with_cancel(&context::background());
        let (client, notes, run_done, runner) = run_on_reader(&ctx);
        write_framed(&client, r#"{"jsonrpc":"2.0","id":1,"method":"outer"}"#);
        let call = read_framed(&client);
        assert!(call.contains(r#""id":"api1""#), "{call}");
        write_framed(&client, r#"{"jsonrpc":"2.0","id":2,"method":"late"}"#);
        let call = read_framed(&client);
        assert!(call.contains(r#""id":"api2""#), "{call}");
        cancel();
        // The 1 message read after the cancel ends the read loop.
        write_framed(&client, r#"{"jsonrpc":"2.0","id":3,"method":"ping"}"#);
        let late = notes
            .recv_timeout(Duration::from_secs(5))
            .expect("late ran");
        assert_eq!(
            late,
            r#"late | [Some("context canceled"), Some("ipc: connection closed\ncontext canceled")]"#
        );
        let outer = notes
            .recv_timeout(Duration::from_secs(5))
            .expect("outer ran");
        assert!(outer.starts_with("context canceled |"), "{outer}");
        let result = run_done
            .recv_timeout(Duration::from_secs(5))
            .expect("run returned");
        assert_eq!(result, Err("context canceled".to_string()));
        runner.join().expect("run thread");
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

    /// How long a `hold` request holds the dispatch thread. The tests act
    /// at 50 ms and 150 ms into it.
    const HOLD: Duration = Duration::from_millis(300);

    /// Starts a request of `method` (id 1) that calls the client back, then
    /// a `hold` request (id 2) that runs nested in that call's wait, and
    /// returns 50 ms into the hold.
    fn hold_a_call(client: &UnixStream, method: &str) {
        let body = format!(r#"{{"jsonrpc":"2.0","id":1,"method":"{method}"}}"#);
        write_framed(client, &body);
        let call = read_framed(client);
        assert!(call.contains(r#""id":"api1""#), "{call}");
        write_framed(client, r#"{"jsonrpc":"2.0","id":2,"method":"hold"}"#);
        std::thread::sleep(Duration::from_millis(50));
    }

    /// The rest of the stream after `run` returned: what the connection
    /// wrote after its first call.
    fn rest(client: &UnixStream) -> String {
        let mut rest = Vec::new();
        let mut client = client;
        client.read_to_end(&mut rest).expect("read");
        String::from_utf8_lossy(&rest).into_owned()
    }

    // PORT: no Go test; the Go clock (file header). A request whose call
    // waits at a signal is held by a nested request (`hold`), and the read
    // loop ends (EOF) during the hold. Go's call returned at the signal and
    // the request made its next calls at once, before the end: they write
    // their requests and return `context canceled` (Phase A).
    #[test]
    fn test_async_conn_reader_held_call_keeps_phase_a() {
        let (ctx, cancel) = context::with_cancel(&context::background());
        let (client, notes, run_done, runner) = run_on_reader(&ctx);
        hold_a_call(&client, "calls");
        cancel();
        std::thread::sleep(Duration::from_millis(100));
        client
            .shutdown(std::net::Shutdown::Write)
            .expect("shutdown");
        let note = notes.recv_timeout(Duration::from_secs(5)).expect("note");
        assert_eq!(
            note,
            r#"calls | ["context canceled", "context canceled", "context canceled"]"#
        );
        let result = run_done
            .recv_timeout(Duration::from_secs(5))
            .expect("run returned");
        assert_eq!(result, Ok(()));
        runner.join().expect("run thread");
        let rest = rest(&client);
        for id in ["api2", "api3"] {
            assert!(rest.contains(&format!(r#""id":"{id}""#)), "{rest}");
        }
    }

    // PORT: no Go test; the Go clock (file header). As
    // `..._held_call_keeps_phase_a`, but the request works `HOLD` after its
    // call returns, which in Go ends after the end of the read loop: its
    // next calls return `terminal` and write nothing (Phase B).
    #[test]
    fn test_async_conn_reader_held_call_passes_the_end() {
        let (ctx, cancel) = context::with_cancel(&context::background());
        let (client, notes, run_done, runner) = run_on_reader(&ctx);
        hold_a_call(&client, "slowcalls");
        cancel();
        std::thread::sleep(Duration::from_millis(100));
        client
            .shutdown(std::net::Shutdown::Write)
            .expect("shutdown");
        let note = notes.recv_timeout(Duration::from_secs(5)).expect("note");
        assert_eq!(
            note,
            r#"slowcalls | ["context canceled", "ipc: connection closed", "ipc: connection closed"]"#
        );
        let result = run_done
            .recv_timeout(Duration::from_secs(5))
            .expect("run returned");
        assert_eq!(result, Ok(()));
        runner.join().expect("run thread");
        let rest = rest(&client);
        assert!(!rest.contains(r#""id":"api2""#), "{rest}");
    }

    // PORT: no Go test; the Go clock (file header). No signal: the reply
    // to a held call comes during the hold, then EOF. Go's call returned at
    // the reply, so its next call came before the end: it writes its
    // request, and the end closes its channel (`terminal`). The call after
    // it comes after the end (Phase B).
    #[test]
    fn test_async_conn_reader_held_reply_without_a_signal() {
        let ctx = context::background();
        let (client, notes, run_done, runner) = run_on_reader(&ctx);
        hold_a_call(&client, "calls");
        write_framed(&client, r#"{"jsonrpc":"2.0","id":"api1","result":null}"#);
        std::thread::sleep(Duration::from_millis(100));
        client
            .shutdown(std::net::Shutdown::Write)
            .expect("shutdown");
        let note = notes.recv_timeout(Duration::from_secs(5)).expect("note");
        assert_eq!(
            note,
            r#"calls | ["ok", "ipc: connection closed", "ipc: connection closed"]"#
        );
        let result = run_done
            .recv_timeout(Duration::from_secs(5))
            .expect("run returned");
        assert_eq!(result, Ok(()));
        runner.join().expect("run thread");
        let rest = rest(&client);
        assert!(rest.contains(r#""id":"api2""#), "{rest}");
        assert!(!rest.contains(r#""id":"api3""#), "{rest}");
    }

    // PORT: no Go test; Go `Run` and `Call` (ipc/conn_async.go:71-74,
    // :289-303). EOF with no signal while a call with the handler context
    // (the module resolver's) is held: Go's `closePendingCalls` closed its
    // channel before `cancelHandlers` cancelled that context, so the call
    // returns `ipc: connection closed`. The hold makes the handler context
    // done before the call looks.
    #[test]
    fn test_async_conn_reader_held_call_end_before_a_signal() {
        let ctx = context::background();
        let (client, notes, run_done, runner) = run_on_reader(&ctx);
        hold_a_call(&client, "hcall");
        client
            .shutdown(std::net::Shutdown::Write)
            .expect("shutdown");
        let note = notes.recv_timeout(Duration::from_secs(5)).expect("note");
        assert_eq!(note, "hcall | ipc: connection closed");
        let result = run_done
            .recv_timeout(Duration::from_secs(5))
            .expect("run returned");
        assert_eq!(result, Ok(()));
        runner.join().expect("run thread");
    }

    // PORT: no Go test; Go `Call` (ipc/conn_async.go:289-303). A read
    // error after a signal while a call is held: Go's call returned
    // `context canceled` at the signal, before the error ended `Run`, and
    // the request's next calls came before the end too.
    #[test]
    fn test_async_conn_reader_held_call_read_error_after_a_signal() {
        let (ctx, cancel) = context::with_cancel(&context::background());
        let (client, notes, run_done, runner) = run_on_reader(&ctx);
        hold_a_call(&client, "calls");
        cancel();
        std::thread::sleep(Duration::from_millis(100));
        (&client)
            .write_all(b"Content-Length: 22\r\n\r\n{\"jsonrpc\":\"2.0\",\"id\":")
            .expect("write");
        let note = notes.recv_timeout(Duration::from_secs(5)).expect("note");
        assert_eq!(
            note,
            r#"calls | ["context canceled", "context canceled", "context canceled"]"#
        );
        let result = run_done
            .recv_timeout(Duration::from_secs(5))
            .expect("run returned");
        assert!(result.is_err(), "{result:?}");
        runner.join().expect("run thread");
    }

    // PORT: no Go test; the Go clock (file header). A call that waits on an
    // idle dispatch thread keeps its frame's lag at 0, after a reply and
    // after a signal: the thread's own wake is not a hold. So without a
    // hold the port decides as before the clock.
    #[test]
    fn test_async_conn_reader_idle_call_keeps_lag_zero() {
        let (ctx, cancel) = context::with_cancel(&context::background());
        let (client, notes, run_done, runner) = run_on_reader(&ctx);
        write_framed(&client, r#"{"jsonrpc":"2.0","id":1,"method":"lags"}"#);
        let call = read_framed(&client);
        assert!(call.contains(r#""id":"api1""#), "{call}");
        std::thread::sleep(Duration::from_millis(50));
        write_framed(&client, r#"{"jsonrpc":"2.0","id":"api1","result":null}"#);
        let call = read_framed(&client);
        assert!(call.contains(r#""id":"api2""#), "{call}");
        std::thread::sleep(Duration::from_millis(50));
        cancel();
        let note = notes.recv_timeout(Duration::from_secs(5)).expect("note");
        assert_eq!(note, "lags | [Some(0ns), Some(0ns)]");
        // The 1 read after the signal is EOF: Go's `Run` returns nil.
        client
            .shutdown(std::net::Shutdown::Write)
            .expect("shutdown");
        let result = run_done
            .recv_timeout(Duration::from_secs(5))
            .expect("run returned");
        assert_eq!(result, Ok(()));
        runner.join().expect("run thread");
    }
}
