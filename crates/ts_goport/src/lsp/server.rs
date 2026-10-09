//! Go `internal/lsp/server.go`.
//!
//! PORT: threads (PORTING.md "Threads", map-lsp-server.md section 3). Go
//! runs the read loop, the dispatch loop, the write loop, the async part of
//! each handler, the progress loop and the parent watchdog on goroutines.
//! Language-service state (`Rc`/`RefCell`) must stay on one thread, so:
//!
//! - The dispatch thread (the thread that calls `Server::run`, with the
//!   stack size of `gostd::stack::max_stack_size`) owns `Server`: the
//!   session, the file system, the handler table and the API sessions. It
//!   runs the sync part of each handler and then its async part inline (Go
//!   starts the async part on a goroutine, so Go can answer requests out of
//!   order; the port answers them in order). Between the two parts, after
//!   each message and after each wake-up it runs
//!   `gostd::local::run_pending()`. When no message waits, it runs
//!   `gostd::local::run_idle()`.
//! - The reader thread owns the `Reader`. It routes responses to
//!   `pending_server_requests`, handles `$/cancelRequest` and the first
//!   `initialize`, and queues all other messages (Go does the same on the
//!   read goroutine). It also cancels the auto-import warm when a file
//!   event arrives, or makes it yield (below).
//! - The writer thread owns the `Writer` and drains the outgoing queue.
//! - The progress thread (`progress.rs`) and the parent watchdog
//!   (`cmd/tsgo/lsp.rs`) touch only `Send` data.
//! - An API session (`custom/initializeAPISession`) has an accept thread
//!   and, once connected, a reader thread. They queue the connection and
//!   wake-ups in the request queue; the dispatch thread serves the
//!   connection (`ApiConnProtocol`).
//!
//! So Go `*Server` is split. `ServerShared` (`Arc`, `Send + Sync`) holds
//! the queues, the pending maps, the atomics and the state that
//! `handleInitialize` writes once on the reader thread (`OnceLock`).
//! `Server` (`Rc`, dispatch thread) holds the rest. Go methods that the
//! reader thread calls (`readLoop`, `sendError`, `handleInitialize`, the
//! send functions) are on `ServerShared`.
//!
//! Effects of the one dispatch thread (lsp-concurrency.md D1, D5, D7). The
//! LS oracle keeps one request in flight, so it sees none of them:
//!
//! - Answers come in arrival order (above). A fast request waits for a slow
//!   one; Go answers the fast one first.
//! - `gostd::local` timers (diagnostics refresh, snapshot update, idle cache
//!   clean) fire at the next `run_pending`: before the async part of the
//!   running request or after its answer. Go fires them on time.
//! - A background client request (`update_watches` registerCapability, 1 s
//!   timeout) blocks the dispatch thread until the client answers. Go waits
//!   on a goroutine.
//! - A request that arrives while the auto-import warm runs waits for it
//!   (below). Go runs the request at the same time.
//! - LSP messages wait while an API request runs, and API requests wait
//!   while an LSP message runs. Go runs them at the same time. An API
//!   connection that opens while another is connected holds the other's
//!   requests until it closes (`ApiConnProtocol`).
//!
//! Background tasks and timers stay on the dispatch thread. A task that the
//! sync part of a request queues (the snapshot update's logging, watch
//! updates and publishDiagnostics) runs before the async part: Go starts it
//! on a goroutine before the async part, and it usually ends before the
//! answer (the publishDiagnostics of a changed tsconfig.json comes before
//! the textDocument/diagnostic answer that picks up the change). Tasks
//! queued later run at the message boundary, after the answer. Do not move
//! them to another thread: the oracle compares where background
//! publishDiagnostics land. Do not hold them back while messages wait
//! either: a task queued by didOpen (the `update_watches`
//! registerCapability) must go out before the answer of the next request,
//! as Go's does.
//!
//! The one exception is idle work (`gostd::local::go_idle`): the clone of
//! the auto-import warm, which sends nothing to the client. It starts only
//! with an empty request queue (`queued_requests`). Go runs it on a
//! goroutine, at the same time, from before the answer of the request that
//! made the snapshot. So the first attempt of a warm starts at once, and a
//! message that comes during it waits only up to Go's head start (a few
//! ms). Then a didOpen, didChange, didClose or didChangeWatchedFiles
//! cancels the warm, as Go's dispatch goroutine does when it handles them,
//! and any other message makes the attempt yield. An attempt after a
//! yield, and each attempt while the session is marked slow (an eager
//! attempt did not end), starts after `IDLE_QUIET_PERIOD`, and only a file
//! event cancels it (`project::WarmAutoImportPreempt`,
//! `project::Session::run_pending_warm`).
//! The warm stops at its next context check.
//!
//! Large frees (`gostd::local::drop_later`: a released program, the parse
//! tasks of a load) wait until the message is done, and then run only
//! while no message waits. Go's garbage collector does them in the
//! background. The checkers and synthetic nodes of a released program wait
//! in the same way only for the first release after a client pause, and
//! only when the client has not sent its next edit yet
//! (`gostd::local::drop_after_pause`), so only the releases of one message
//! wait (on an API connection, of one pipelined burst). Other releases free
//! them at once, so that the next check reuses their memory.
//!
//! Cancellation is Go's: `$/cancelRequest` reaches only a request that the
//! dispatch loop took (`pending_client_requests`); a cancel for a queued
//! request is dropped. The LS loops check the request context. The checker
//! checks it at each top-level statement and deferred node, like Go.
//!
//! When the `run` context ends (Go `signal.NotifyContext` in
//! `cmd/tsc/lsp.go`), every loop returns `context canceled`, as in Go. A
//! request on the dispatch thread first runs to its next cancel check. Its
//! answer is not written, because `send` uses the ended group context.

use crate::lsp::prelude::*;

use crate::contentmapper;
use crate::emitter::program_emit;
use crate::frontend::compiler;
use crate::frontend::json_ext::{self, AnyValue};
use crate::frontend::tsoptions;
use crate::gostd::context::{self, CancelCauseFunc, CancelFunc};
use crate::gostd::errors;
use crate::ipc;
use crate::ipc::Protocol as _;
use crate::lsp::lsproto::{ErrorCode, HasTextDocumentPosition, HasTextDocumentURI};
use crate::program::ls_program;
use crate::project::logging::{self, Logger as _};
use crate::project::{Snapshot, ata};
use std::any::Any;
use std::cell::Cell;
use std::collections::VecDeque;
use std::io::{BufRead, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Weak;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::sync::{Arc, Condvar, LazyLock, Mutex, MutexGuard, OnceLock, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

// PORT: Go mutexes do not poison.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

// PORT: Go `sync.RWMutex` read lock (no poison).
fn read_lock<T: Clone>(m: &RwLock<T>) -> T {
    m.read().unwrap_or_else(|e| e.into_inner()).clone()
}

// PORT: Go `sync.RWMutex` write lock (no poison).
fn write_lock<T>(m: &RwLock<T>, value: T) {
    *m.write().unwrap_or_else(|e| e.into_inner()) = value;
}

// Go: server.go:42 ServerOptions
// PORT: Go `In`, `Out` and `Err` move to the reader, writer and logging
// threads, so they are `Send`. `NpmInstall` and `SetParentProcessID` are
// nil-able Go funcs (`None`). `NpmInstall` returns Go's `([]byte, error)`
// pair, as `ata::NpmExecutor` does, and is `Send`: ATA runs it on a helper
// thread (`ata::NpmExecutor::npm_install_func`).
pub struct ServerOptions {
    pub in_: Box<dyn Reader + Send>,
    pub out: Box<dyn Writer + Send>,
    pub err: Box<dyn Write + Send>,

    pub cwd: String,
    pub fs: Rc<dyn vfs::Fs>,
    pub default_library_path: String,
    pub typings_location: String,
    pub parse_cache: Option<Rc<project::ParseCache>>,
    pub npm_install:
        Option<Box<dyn Fn(&str, &[String]) -> (Vec<u8>, Option<GoError>) + Send + Sync>>,
    // Spawn launches a child process, returning its stdio as an io.ReadWriteCloser (Read is its stdout,
    // Write is its stdin). It is nil when the host cannot spawn processes. Currently used for content mappers.
    // PORT: tsgo#4712. The Go func returns an `io.ReadWriteCloser`; the
    // content mapper spawner function returns a `ProcessExitState` (see
    // `contentmapper::Spawner`).
    pub spawn: Option<Rc<contentmapper::SpawnFn>>,
    pub progress_delay: Duration, // delay before showing progress UI; 0 means no delay
    pub set_parent_process_id: Option<Box<dyn Fn(i32) + Send + Sync>>,
}

// Go: server.go:60 NewServer
pub fn new_server(opts: ServerOptions) -> Rc<Server> {
    if opts.cwd.is_empty() {
        crate::core::go_panic("Cwd is required".to_string());
    }

    let ServerOptions {
        in_,
        out,
        err,
        cwd,
        fs,
        default_library_path,
        typings_location,
        parse_cache,
        npm_install,
        spawn,
        progress_delay,
        set_parent_process_id,
    } = opts;

    // Go: s.logger = newLogger(s)
    let shared = Arc::new_cyclic(|weak| ServerShared {
        background_ctx: OnceLock::new(),
        stderr: Mutex::new(err),
        logger: Arc::new(new_logger(weak.clone())),
        init_started: AtomicBool::new(false),
        client_seq: AtomicI32::new(0),
        request_queue: new_dynamic_queue(),
        queued_requests: AtomicUsize::new(0),
        queued_mu: Mutex::new(()),
        queued_cond: Condvar::new(),
        outgoing_queue: new_dynamic_queue(),
        pending_client_requests: Mutex::new(FxHashMap::default()),
        pending_server_requests: Mutex::new(FxHashMap::default()),
        cwd,
        initialize_params: OnceLock::new(),
        initialization_options: OnceLock::new(),
        client_capabilities: OnceLock::new(),
        position_encoding: OnceLock::new(),
        locale: RwLock::new(locale::Locale::default()),
        init_locale: OnceLock::new(),
        last_request_time_ms: AtomicI64::new(0),
        progress_delay,
        project_progress: OnceLock::new(),
        start_watchdog: set_parent_process_id,
        flake_logging: OnceLock::new(),
        warm_auto_import_preempt: OnceLock::new(),
    });

    Rc::new(Server {
        logger: shared.logger.clone(),
        shared,
        r: RefCell::new(Some(in_)),
        w: RefCell::new(Some(out)),
        fs,
        default_library_path,
        typings_location,
        watch_enabled: Cell::new(false),
        telemetry_enabled: Cell::new(false),
        watcher_id: Cell::new(0),
        watchers: RefCell::new(FxHashSet::default()),
        builtin_watcher: RefCell::new(None),
        session: RefCell::new(None),
        api_sessions: RefCell::new(None),
        stopping_api_sessions: RefCell::new(Vec::new()),
        close_session_after_api_sessions: Cell::new(false),
        client: None,
        init_complete: Cell::new(false),
        compiler_options_for_inferred_projects: RefCell::new(None),
        parse_cache,
        // PORT: ts#64544 gives Go `NpmInstall` a ctx, which cmd/tsc/lsp.go
        // passes to `exec.CommandContext` (npm is killed when the session
        // closes). `ServerOptions.npm_install` (set by cmd/tsgo/lsp.rs, a
        // build lane file) does not take it yet, so it is dropped here.
        npm_install: npm_install.map(|npm_install| -> ata::NpmInstallFunc {
            Arc::new(move |_ctx: &Context, cwd: &str, args: &[String]| npm_install(cwd, args))
        }),
        spawn,
        content_mapper_extensions_registered: Cell::new(false),
        cpu_profiler: crate::pprof::CpuProfiler::default(),
        dispatch_ctx: RefCell::new(None),
        free_since: Cell::new(Instant::now()),
    })
}

// Go: server.go:90 fileRenameFilters
pub static FILE_RENAME_FILTERS: LazyLock<Vec<lsproto::FileOperationFilter>> = LazyLock::new(|| {
    vec![lsproto::FileOperationFilter {
        scheme: Some("file".to_string()),
        pattern: Some(lsproto::FileOperationPattern {
            glob: "**/*.{ts,tsx,js,jsx,cts,cjs,mts,mjs,json}".to_string(),
            ..Default::default()
        }),
    }]
});

// Go: server.go:98 `_ ata.NpmExecutor = (*Server)(nil)` and
// `_ project.Client = (*Server)(nil)`: the impls below.

// Go: server.go:102 pendingClientRequest
// PORT: Go keeps the `*lsproto.RequestMessage`, which no code reads. The
// request stays on the dispatch thread (its params are not `Sync`), so the
// entry keeps its method.
pub struct PendingClientRequest {
    pub method: lsproto::Method,
    pub cancel: CancelFunc,
}

// Go: server.go:107 Reader
// PORT: Go `Read() (*lsproto.Message, error)` can return a message and an
// error together (invalid params), so the result is a pair.
pub trait Reader {
    fn read(&mut self) -> (Option<lsproto::Message>, Option<GoError>);
}

// Go: server.go:111 Writer
pub trait Writer {
    fn write(&mut self, msg: &lsproto::Message) -> Result<(), GoError>;
}

// Go: server.go:115 lspReader
pub struct LspReader {
    pub r: lsproto::BaseReader,
}

// Go: server.go:119 lspWriter
pub struct LspWriter {
    pub w: lsproto::BaseWriter,
}

// Go: server.go:123 messageMarshalError
// PORT: a value type made with `errors::from_value_with_unwrap`, so
// `errors::as_type` finds it. Go `Unwrap() []error` returns
// `{lsproto.ErrorCodeInternalError, e.err}`; the port's value types have
// only `Unwrap() error`, so the one unwrap result is an error that wraps
// both, in the same order (see `new_message_marshal_error`).
#[derive(Clone, Debug, PartialEq)]
pub struct MessageMarshalError {
    pub err: GoError,
}

// Go: server.go:127 messageMarshalError.Error
impl std::fmt::Display for MessageMarshalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "failed to marshal message: {}", self.err.error())
    }
}

/// Go `&messageMarshalError{err: err}` as an `error`.
fn new_message_marshal_error(err: GoError) -> GoError {
    let value = MessageMarshalError { err: err.clone() };
    // Go: server.go:129 messageMarshalError.Unwrap
    let unwrap = errors::errorf(
        value.to_string(),
        vec![errors::from_value(ErrorCode::INTERNAL_ERROR), err],
    );
    errors::from_value_with_unwrap(value, unwrap)
}

// Go: `fmt.Errorf("%w: %w", code, err)`.
fn wrap_error_code(code: ErrorCode, err: GoError) -> GoError {
    let code = errors::from_value(code);
    errors::errorf(
        format!("{}: {}", code.error(), err.error()),
        vec![code, err],
    )
}

impl Reader for LspReader {
    // Go: server.go:133 lspReader.Read
    fn read(&mut self) -> (Option<lsproto::Message>, Option<GoError>) {
        let data = match self.r.read() {
            Ok(data) => data,
            Err(err) => return (None, Some(err)),
        };

        let mut req = lsproto::Message::default();
        // Go `json.Unmarshal(data, req)` reads the raw value, then calls the
        // v1 method `(*Message).UnmarshalJSON`. A method error gets the outer
        // "json: cannot unmarshal JSON object into Go lsproto.Message: "
        // text, and stays in the chain for `errors.Is`.
        if let Err(err) =
            json_ext::unmarshal_json_method::<lsproto::Message>(&data, |d| req.unmarshal_json(d))
        {
            if errors::is(&err, &errors::from_value(ErrorCode::INVALID_PARAMS)) {
                return (
                    Some(req),
                    Some(wrap_error_code(ErrorCode::INVALID_PARAMS, err)),
                );
            }
            return (None, Some(wrap_error_code(ErrorCode::INVALID_REQUEST, err)));
        }

        (Some(req), None)
    }
}

// Go: server.go:150 ToReader
// PORT: `lsproto.NewBaseReader` takes a `BufRead` (the caller supplies the
// buffer that Go's bufio adds).
pub fn to_reader(r: Box<dyn BufRead + Send>) -> Box<dyn Reader + Send> {
    Box::new(LspReader {
        r: lsproto::new_base_reader(r),
    })
}

impl Writer for LspWriter {
    // Go: server.go:154 lspWriter.Write
    fn write(&mut self, msg: &lsproto::Message) -> Result<(), GoError> {
        let data = match crate::frontend::json::json_marshal(msg, &[]) {
            Ok(data) => data,
            Err(err) => {
                return Err(new_message_marshal_error(errors::from_value(err)));
            }
        };
        // PORT: the text is in the port form (see
        // `scanner_util::GO_STRING_MARKER`); the client gets its Go bytes.
        self.w.write(&crate::scanner_util::go_string_bytes(&data))
    }
}

// Go: server.go:162 ToWriter
pub fn to_writer(w: Box<dyn Write + Send>) -> Box<dyn Writer + Send> {
    Box::new(LspWriter {
        w: lsproto::new_base_writer(w),
    })
}

// Go: server.go:167 `_ Reader = (*lspReader)(nil)`, `_ Writer = (*lspWriter)(nil)`:
// the impls above.

/// PORT: an item of the request queue. Go queues `*lsproto.RequestMessage`.
/// The port also queues `Wake` from `gostd::local` timers (the dispatch
/// loop contract in PORTING.md "Go runtime") and from API reader threads,
/// and `ApiAccepted` from API accept threads (`handle_initialize_api_session`).
pub enum QueuedRequest {
    Request(lsproto::RequestMessage),
    Wake,
    ApiAccepted(ApiAccepted),
}

/// PORT: the end of Go's `transport.Accept()` in the API session goroutine
/// (`handle_initialize_api_session`). `rwc` is `None` when the accept
/// failed (the accept thread logged it).
pub struct ApiAccepted {
    session_id: String,
    rwc: Option<Arc<dyn ipc::ReadWriteCloser>>,
}

// Go: server.go:255 apiSessionState (ts#64544)
// PORT: `mu` is dropped (dispatch thread). Go's `transport` stays with the
// accept thread, which closes it after `Accept` (see `stop`). The rest of
// the accept goroutine runs on the dispatch thread (`serve_api_connection`),
// so `done` is `ended`, set when that rest has finished.
pub struct ApiSessionState {
    session: Rc<api::Session>,
    cancel: CancelFunc,
    // PORT: Go makes `apiCtx` in `handleInitializeAPISession` and the
    // goroutine reads it; the state keeps it for `serve_api_connection`.
    api_ctx: Context,
    connection: RefCell<Option<Arc<dyn ipc::ReadWriteCloser>>>,
    stopped: Cell<bool>,
    ended: Cell<bool>,
    // PORT: the state of the running connection (`run_api_connection`),
    // for `stop`.
    conn_state: RefCell<Option<Rc<ApiConnState>>>,
}

impl ApiSessionState {
    // Go: server.go:266 apiSessionState.attachConnection
    fn attach_connection(&self, connection: Arc<dyn ipc::ReadWriteCloser>) -> bool {
        if self.stopped.get() {
            let _ = connection.close();
            return false;
        }
        *self.connection.borrow_mut() = Some(connection);
        true
    }

    // Go: server.go:277 apiSessionState.stop
    // PORT: Go also closes the transport, which ends a pending `Accept`. The
    // port's listener holds its lock while it waits in accept, so a close
    // from the dispatch thread would wait for a client. When no client has
    // connected yet, the accept thread waits until the process exits; the
    // connection it may still accept finds no session
    // (`serve_api_connection`) and is closed. Go then waits for `done`:
    // - Without a connection, the port runs the end of the goroutine here
    //   (`apiSession.Close()`; the caller removed the session). Go also logs
    //   the accept error of the closed transport; the port does not.
    // - With a connection, the connection waits for its next message below
    //   on this thread's stack (an LSP message that it serves called this),
    //   so the port cannot wait. Closing the connection ends its wait, and
    //   `serve_api_connection` runs the end of the goroutine afterwards;
    //   Shutdown closes the project session then
    //   (`Server::close_session_after_api_sessions`).
    // - When an API request below waits for the answer of a call to the
    //   client, its wait keeps the LSP requests that need the session
    //   (`ApiConnProtocol`), and Go's dispatch goroutine would still be
    //   blocked on them. Ending the wait would serve them on the session
    //   that Shutdown closes, so the port leaves this connection and ends
    //   it with the dispatch loop (exit), as before ts#64544.
    fn stop(&self) {
        let waits_for_client = self
            .conn_state
            .borrow()
            .as_ref()
            .is_some_and(|state| state.calls.get() > 0);
        if !self.stopped.replace(true) && !waits_for_client {
            (self.cancel)();
            if let Some(connection) = self.connection.borrow().as_ref() {
                let _ = connection.close();
            }
        }
        if !self.ended.get() && self.connection.borrow().is_none() {
            self.session.close();
            self.ended.set(true);
        }
    }
}

// Go: server.go:171 Server (the fields that other threads use)
pub struct ServerShared {
    pub background_ctx: OnceLock<Context>,

    pub stderr: Mutex<Box<dyn Write + Send>>,

    pub logger: Arc<Logger>,
    pub init_started: AtomicBool,
    pub client_seq: AtomicI32,
    pub request_queue: DynamicQueue<QueuedRequest>,
    // PORT: the number of items in `request_queue` (see `queue_request`).
    // The dispatch loop runs idle work only when it is 0.
    pub queued_requests: AtomicUsize,
    // PORT: `queue_request` signals `queued_cond` (under `queued_mu`) after
    // each put, to end a `wait_quiet`.
    pub queued_mu: Mutex<()>,
    pub queued_cond: Condvar,
    pub outgoing_queue: DynamicQueue<lsproto::Message>,
    // PORT: Go `pendingClientRequestsMu` and `pendingServerRequestsMu` are
    // the mutexes. A pending server request holds the sending end of its
    // response channel; `None` on that channel is the context wake-up of
    // `send_client_request`.
    pub pending_client_requests: Mutex<FxHashMap<crate::jsonrpc::ID, PendingClientRequest>>,
    pub pending_server_requests:
        Mutex<FxHashMap<crate::jsonrpc::ID, SyncSender<Option<lsproto::ResponseMessage>>>>,

    pub cwd: String,

    // PORT: written once by `handle_initialize` on the reader thread.
    pub initialize_params: OnceLock<lsproto::InitializeParams>,
    pub initialization_options: OnceLock<lsproto::InitializationOptions>,
    pub client_capabilities: OnceLock<Arc<lsproto::ResolvedClientCapabilities>>,
    pub position_encoding: OnceLock<lsproto::PositionEncodingKind>,
    // PORT: Go `localeMu` is the `RwLock`. `locale` changes after
    // `initialize` (#4660 `SetLocale`), so it is not a `OnceLock`.
    pub locale: RwLock<locale::Locale>,
    // initLocale is the locale resolved from the initialize request; it is
    // used as the fallback when the user's locale preference is "auto".
    // PORT: written once by `handle_initialize` on the reader thread.
    pub init_locale: OnceLock<locale::Locale>,

    pub last_request_time_ms: AtomicI64,

    pub progress_delay: Duration,
    pub project_progress: OnceLock<Arc<ProjectLoadingProgress>>,

    pub start_watchdog: Option<Box<dyn Fn(i32) + Send + Sync>>,

    // PORT: written once by `handle_initialize` on the reader thread.
    pub flake_logging: OnceLock<lsproto::DiagnosticFlakeLogLevel>,

    // PORT: the session's `warm_auto_import_preempt`, set by
    // `handle_initialized`. The reader thread queues each message through
    // it, and cancels the warm or makes it yield with it.
    pub warm_auto_import_preempt: OnceLock<project::WarmAutoImportPreempt>,
}

// Go: server.go:171 Server (the dispatch-thread fields)
pub struct Server {
    pub shared: Arc<ServerShared>,

    // PORT: taken by `run` for the reader and writer threads.
    pub r: RefCell<Option<Box<dyn Reader + Send>>>,
    pub w: RefCell<Option<Box<dyn Writer + Send>>>,

    // PORT: the same logger as `shared.logger`.
    pub logger: Arc<Logger>,

    pub fs: Rc<dyn vfs::Fs>,
    pub default_library_path: String,
    pub typings_location: String,

    pub watch_enabled: Cell<bool>,
    pub telemetry_enabled: Cell<bool>,
    pub watcher_id: Cell<u32>,
    pub watchers: RefCell<FxHashSet<project::WatcherID>>,
    // builtinWatcher is non-nil when the server is running its own
    // in-process file watcher instead of using LSP-based watching. It
    // is enabled when the client lacks DynamicRegistration for
    // workspace/didChangeWatchedFiles and the builtin watcher backend
    // supports efficient recursive watching (Windows or FSEvents).
    pub builtin_watcher: RefCell<Option<Rc<lspwatcher::Watcher>>>,

    pub session: RefCell<Option<Rc<project::Session>>>,

    // apiSessions holds active API sessions keyed by their ID
    // PORT: `apiSessionsMu` is dropped (dispatch thread). `None` is Go's nil map.
    pub api_sessions: RefCell<Option<FxHashMap<String, Rc<ApiSessionState>>>>,
    // PORT: the API sessions that `close_api_sessions` stopped while their
    // connection still ran below on the dispatch thread's stack. Go's
    // `stop` waits for them before Shutdown closes the project session; the
    // port closes the project session when the last of them has ended
    // (`close_session_after_api_sessions`).
    pub stopping_api_sessions: RefCell<Vec<Rc<ApiSessionState>>>,
    pub close_session_after_api_sessions: Cell<bool>,

    // Test options for initializing session
    pub client: Option<Rc<dyn project::Client>>,

    // initComplete is closed when handleInitialized completes.
    // Used by tests to wait for full initialization.
    // PORT: Go `chan struct{}`; `true` is closed.
    pub init_complete: Cell<bool>,

    // !!! temporary; remove when we have `handleDidChangeConfiguration`/implicit project config support
    pub compiler_options_for_inferred_projects: RefCell<Option<Rc<CompilerOptions>>>,
    // parseCache can be passed in so separate tests can share ASTs
    pub parse_cache: Option<Rc<project::ParseCache>>,

    pub npm_install: Option<ata::NpmInstallFunc>,
    // tsgo#4712
    pub spawn: Option<Rc<contentmapper::SpawnFn>>,

    // contentMapperExtensionsRegistered records whether a content mapper text document sync
    // registration is currently active with the client, so it can be replaced or removed.
    // PORT: `contentMapperRegistrationMu` is dropped (dispatch thread).
    pub content_mapper_extensions_registered: Cell<bool>,

    pub cpu_profiler: crate::pprof::CpuProfiler,
    // PORT: the dispatch loop's context and Go `lspExit`, set by
    // `dispatch_loop`. An API connection that waits for its next message
    // runs the dispatch loop with them (`ApiConnProtocol`).
    pub dispatch_ctx: RefCell<Option<(Context, CancelCauseFunc)>>,
    // PORT: when the dispatch loop last finished a message (see
    // `IDLE_QUIET_PERIOD` and `gostd::local::note_message_gap`).
    pub free_since: Cell<Instant>,
    // PORT: Go `progressDelay` and `projectProgress` are in `ServerShared`,
    // `startWatchdog` is `ServerShared::start_watchdog`.
}

impl ServerShared {
    /// Go `s.backgroundCtx`, set by `Run`.
    pub fn background_ctx(&self) -> Context {
        self.background_ctx
            .get()
            .cloned()
            .unwrap_or_else(|| crate::core::go_nil_dereference())
    }

    /// Go `s.initializeParams` (nil before `initialize`).
    pub fn initialize_params(&self) -> &lsproto::InitializeParams {
        self.initialize_params
            .get()
            .unwrap_or_else(|| crate::core::go_nil_dereference())
    }

    /// Go `s.initializationOptions` (nil before `initialize`).
    pub fn initialization_options(&self) -> &lsproto::InitializationOptions {
        self.initialization_options
            .get()
            .unwrap_or_else(|| crate::core::go_nil_dereference())
    }

    /// Go `&s.clientCapabilities` (the zero value before `initialize`).
    pub fn client_capabilities(&self) -> Arc<lsproto::ResolvedClientCapabilities> {
        match self.client_capabilities.get() {
            Some(caps) => caps.clone(),
            None => Arc::new(lsproto::ResolvedClientCapabilities::default()),
        }
    }

    /// Go `s.positionEncoding` (the zero value before `initialize`).
    pub fn position_encoding(&self) -> lsproto::PositionEncodingKind {
        self.position_encoding.get().cloned().unwrap_or_default()
    }

    /// Go `s.locale` (the zero value until `initialize` parses one).
    // PORT: read under `localeMu`, as Go `GetLocale` does.
    pub fn locale(&self) -> locale::Locale {
        read_lock(&self.locale)
    }

    /// Go `s.flakeLogging` (the zero value, `Off`, before `initialize`).
    pub fn flake_logging(&self) -> lsproto::DiagnosticFlakeLogLevel {
        self.flake_logging.get().copied().unwrap_or_default()
    }

    /// Go `s.initLocale` (the zero value before `initialize`).
    pub fn init_locale(&self) -> locale::Locale {
        self.init_locale.get().cloned().unwrap_or_default()
    }
}

impl Server {
    // Go: server.go:291 Session
    pub fn session(&self) -> Option<Rc<project::Session>> {
        self.session.borrow().clone()
    }

    /// Go `s.session` where Go dereferences it: a nil session panics like a
    /// Go nil pointer dereference.
    pub fn session_ref(&self) -> Rc<project::Session> {
        self.session
            .borrow()
            .clone()
            .unwrap_or_else(|| crate::core::go_nil_dereference())
    }

    // Go: server.go:296 InitComplete
    // InitComplete returns a channel that is closed when the server has finished
    // processing the initialized notification, including the initial configuration
    // exchange with the client.
    // PORT: whether the channel is closed.
    pub fn init_complete(&self) -> bool {
        self.init_complete.get()
    }
}

// Go: server.go:323 content mapper registration IDs (tsgo#4712)
const CONTENT_MAPPER_DID_OPEN_REGISTRATION_ID: &str = "content-mapper-did-open";
const CONTENT_MAPPER_DID_CHANGE_REGISTRATION_ID: &str = "content-mapper-did-change";
const CONTENT_MAPPER_DID_CLOSE_REGISTRATION_ID: &str = "content-mapper-did-close";
const CONTENT_MAPPER_DIAGNOSTIC_REGISTRATION_ID: &str = "content-mapper-diagnostic";
const CONTENT_MAPPER_HOVER_REGISTRATION_ID: &str = "content-mapper-hover";
const CONTENT_MAPPER_SIGNATURE_HELP_REGISTRATION_ID: &str = "content-mapper-signature-help";
const CONTENT_MAPPER_DEFINITION_REGISTRATION_ID: &str = "content-mapper-definition";
const CONTENT_MAPPER_TYPE_DEFINITION_REGISTRATION_ID: &str = "content-mapper-type-definition";
const CONTENT_MAPPER_IMPLEMENTATION_REGISTRATION_ID: &str = "content-mapper-implementation";
const CONTENT_MAPPER_REFERENCES_REGISTRATION_ID: &str = "content-mapper-references";
const CONTENT_MAPPER_DOCUMENT_HIGHLIGHT_REGISTRATION_ID: &str = "content-mapper-document-highlight";
const CONTENT_MAPPER_COMPLETION_REGISTRATION_ID: &str = "content-mapper-completion";
const CONTENT_MAPPER_RENAME_REGISTRATION_ID: &str = "content-mapper-rename";
const CONTENT_MAPPER_SEMANTIC_TOKENS_REGISTRATION_ID: &str = "content-mapper-semantic-tokens";
const CONTENT_MAPPER_DOCUMENT_SYMBOL_REGISTRATION_ID: &str = "content-mapper-document-symbol";
const CONTENT_MAPPER_FOLDING_RANGE_REGISTRATION_ID: &str = "content-mapper-folding-range";
const CONTENT_MAPPER_SELECTION_RANGE_REGISTRATION_ID: &str = "content-mapper-selection-range";
const CONTENT_MAPPER_INLAY_HINT_REGISTRATION_ID: &str = "content-mapper-inlay-hint";
const CONTENT_MAPPER_CODE_LENS_REGISTRATION_ID: &str = "content-mapper-code-lens";
const CONTENT_MAPPER_CODE_ACTION_REGISTRATION_ID: &str = "content-mapper-code-action";
const CONTENT_MAPPER_FORMATTING_REGISTRATION_ID: &str = "content-mapper-formatting";
const CONTENT_MAPPER_RANGE_FORMATTING_REGISTRATION_ID: &str = "content-mapper-range-formatting";
const CONTENT_MAPPER_ON_TYPE_FORMATTING_REGISTRATION_ID: &str = "content-mapper-on-type-formatting";
const CONTENT_MAPPER_LINKED_EDITING_REGISTRATION_ID: &str = "content-mapper-linked-editing";
const CONTENT_MAPPER_CALL_HIERARCHY_REGISTRATION_ID: &str = "content-mapper-call-hierarchy";
const CONTENT_MAPPER_WILL_RENAME_FILES_REGISTRATION_ID: &str = "content-mapper-will-rename-files";

// Go: server.go:388 supportedCodeActionKinds (ts#63951)
pub fn supported_code_action_kinds() -> Vec<lsproto::CodeActionKind> {
    vec![
        lsproto::CodeActionKind::QUICK_FIX,
        lsproto::CodeActionKind::SOURCE_ORGANIZE_IMPORTS_TS,
        lsproto::CodeActionKind::SOURCE_REMOVE_UNUSED_IMPORTS_TS,
        lsproto::CodeActionKind::SOURCE_SORT_IMPORTS_TS,
        lsproto::CodeActionKind::SOURCE_FIX_ALL_TS,
    ]
}

impl Server {
    // Go: server.go:398 supportsContentMapperRegistration (tsgo#4712)
    pub fn supports_content_mapper_registration(&self, id: &str) -> bool {
        let caps = self.shared.client_capabilities();
        let text_document = &caps.text_document;
        match id {
            CONTENT_MAPPER_DID_OPEN_REGISTRATION_ID
            | CONTENT_MAPPER_DID_CHANGE_REGISTRATION_ID
            | CONTENT_MAPPER_DID_CLOSE_REGISTRATION_ID => {
                text_document.synchronization.dynamic_registration
            }
            CONTENT_MAPPER_DIAGNOSTIC_REGISTRATION_ID => {
                text_document.diagnostic.dynamic_registration
            }
            CONTENT_MAPPER_HOVER_REGISTRATION_ID => text_document.hover.dynamic_registration,
            CONTENT_MAPPER_SIGNATURE_HELP_REGISTRATION_ID => {
                text_document.signature_help.dynamic_registration
            }
            CONTENT_MAPPER_DEFINITION_REGISTRATION_ID => {
                text_document.definition.dynamic_registration
            }
            CONTENT_MAPPER_TYPE_DEFINITION_REGISTRATION_ID => {
                text_document.type_definition.dynamic_registration
            }
            CONTENT_MAPPER_IMPLEMENTATION_REGISTRATION_ID => {
                text_document.implementation.dynamic_registration
            }
            CONTENT_MAPPER_REFERENCES_REGISTRATION_ID => {
                text_document.references.dynamic_registration
            }
            CONTENT_MAPPER_DOCUMENT_HIGHLIGHT_REGISTRATION_ID => {
                text_document.document_highlight.dynamic_registration
            }
            CONTENT_MAPPER_COMPLETION_REGISTRATION_ID => {
                text_document.completion.dynamic_registration
            }
            CONTENT_MAPPER_RENAME_REGISTRATION_ID => text_document.rename.dynamic_registration,
            CONTENT_MAPPER_SEMANTIC_TOKENS_REGISTRATION_ID => {
                text_document.semantic_tokens.dynamic_registration
            }
            CONTENT_MAPPER_DOCUMENT_SYMBOL_REGISTRATION_ID => {
                text_document.document_symbol.dynamic_registration
            }
            CONTENT_MAPPER_FOLDING_RANGE_REGISTRATION_ID => {
                text_document.folding_range.dynamic_registration
            }
            CONTENT_MAPPER_SELECTION_RANGE_REGISTRATION_ID => {
                text_document.selection_range.dynamic_registration
            }
            CONTENT_MAPPER_INLAY_HINT_REGISTRATION_ID => {
                text_document.inlay_hint.dynamic_registration
            }
            CONTENT_MAPPER_CODE_LENS_REGISTRATION_ID => {
                text_document.code_lens.dynamic_registration
            }
            CONTENT_MAPPER_CODE_ACTION_REGISTRATION_ID => {
                text_document.code_action.dynamic_registration
            }
            CONTENT_MAPPER_FORMATTING_REGISTRATION_ID => {
                text_document.formatting.dynamic_registration
            }
            CONTENT_MAPPER_RANGE_FORMATTING_REGISTRATION_ID => {
                text_document.range_formatting.dynamic_registration
            }
            CONTENT_MAPPER_ON_TYPE_FORMATTING_REGISTRATION_ID => {
                text_document.on_type_formatting.dynamic_registration
            }
            CONTENT_MAPPER_LINKED_EDITING_REGISTRATION_ID => {
                text_document.linked_editing_range.dynamic_registration
            }
            CONTENT_MAPPER_CALL_HIERARCHY_REGISTRATION_ID => {
                text_document.call_hierarchy.dynamic_registration
            }
            CONTENT_MAPPER_WILL_RENAME_FILES_REGISTRATION_ID => {
                caps.workspace.file_operations.dynamic_registration
                    && caps.workspace.file_operations.will_rename
            }
            _ => false,
        }
    }
}

impl project::Client for Server {
    // Go: server.go:299 WatchFiles
    // WatchFiles implements project.Client.
    fn watch_files(
        &self,
        ctx: &Context,
        id: project::WatcherID,
        watchers: &[lsproto::FileSystemWatcher],
    ) -> Result<(), GoError> {
        let builtin_watcher = self.builtin_watcher.borrow().clone();
        if let Some(builtin_watcher) = builtin_watcher {
            if let Err(err) = builtin_watcher.watch_files(&id.0, watchers) {
                return Err(errors::errorf(
                    format!("failed to register file watcher: {}", err.error()),
                    vec![err],
                ));
            }
            self.watchers.borrow_mut().insert(id);
            return Ok(());
        }
        let result = send_client_request(
            ctx,
            &self.shared,
            &lsproto::CLIENT_REGISTER_CAPABILITY_INFO,
            lsproto::RegistrationParams {
                registrations: vec![lsproto::Registration {
                    id: id.0.clone(),
                    register_options: Some(lsproto::RegisterOptions {
                        workspace_did_change_watched_files: Some(
                            lsproto::DidChangeWatchedFilesRegistrationOptions {
                                watchers: watchers.to_vec(),
                            },
                        ),
                        ..Default::default()
                    }),
                }],
            },
        );
        if let Err(err) = result {
            return Err(errors::errorf(
                format!("failed to register file watcher: {}", err.error()),
                vec![err],
            ));
        }

        self.watchers.borrow_mut().insert(id);
        Ok(())
    }

    // Go: server.go:328 UnwatchFiles
    // UnwatchFiles implements project.Client.
    fn unwatch_files(&self, ctx: &Context, id: project::WatcherID) -> Result<(), GoError> {
        let builtin_watcher = self.builtin_watcher.borrow().clone();
        if let Some(builtin_watcher) = builtin_watcher {
            if !self.watchers.borrow().contains(&id) {
                return Err(errors::new(format!(
                    "no file watcher exists with ID {}",
                    id.0
                )));
            }
            if let Err(err) = builtin_watcher.unwatch_files(&id.0) {
                return Err(errors::errorf(
                    format!("failed to unregister file watcher: {}", err.error()),
                    vec![err],
                ));
            }
            self.watchers.borrow_mut().remove(&id);
            return Ok(());
        }
        if self.watchers.borrow().contains(&id) {
            let result = send_client_request(
                ctx,
                &self.shared,
                &lsproto::CLIENT_UNREGISTER_CAPABILITY_INFO,
                lsproto::UnregistrationParams {
                    unregisterations: vec![lsproto::Unregistration {
                        id: id.0.clone(),
                        method: lsproto::Method::WORKSPACE_DID_CHANGE_WATCHED_FILES
                            .0
                            .to_string(),
                    }],
                },
            );
            if let Err(err) = result {
                return Err(errors::errorf(
                    format!("failed to unregister file watcher: {}", err.error()),
                    vec![err],
                ));
            }

            self.watchers.borrow_mut().remove(&id);
            return Ok(());
        }

        Err(errors::new(format!(
            "no file watcher exists with ID {}",
            id.0
        )))
    }

    // Go: server.go:458 RegisterContentMapperExtensions (tsgo#4712)
    // RegisterContentMapperExtensions implements project.Client. It dynamically registers text document
    // synchronization and pull diagnostics for the given otherwise unsupported file extensions so the editor forwards their
    // open/change/close notifications to the server and requests diagnostics for them. It is called with the
    // full desired set each time it changes; an empty slice removes any prior registration.
    fn register_content_mapper_extensions(
        &self,
        ctx: &Context,
        extensions: &[String],
    ) -> Result<(), GoError> {
        let client_capabilities = self.shared.client_capabilities();
        if !client_capabilities
            .text_document
            .synchronization
            .dynamic_registration
        {
            return Ok(());
        }

        if self.content_mapper_extensions_registered.get() {
            let mut unregistrations: Vec<lsproto::Unregistration> = [
                (
                    CONTENT_MAPPER_DID_OPEN_REGISTRATION_ID,
                    lsproto::Method::TEXT_DOCUMENT_DID_OPEN,
                ),
                (
                    CONTENT_MAPPER_DID_CHANGE_REGISTRATION_ID,
                    lsproto::Method::TEXT_DOCUMENT_DID_CHANGE,
                ),
                (
                    CONTENT_MAPPER_DID_CLOSE_REGISTRATION_ID,
                    lsproto::Method::TEXT_DOCUMENT_DID_CLOSE,
                ),
                (
                    CONTENT_MAPPER_DIAGNOSTIC_REGISTRATION_ID,
                    lsproto::Method::TEXT_DOCUMENT_DIAGNOSTIC,
                ),
                (
                    CONTENT_MAPPER_HOVER_REGISTRATION_ID,
                    lsproto::Method::TEXT_DOCUMENT_HOVER,
                ),
                (
                    CONTENT_MAPPER_SIGNATURE_HELP_REGISTRATION_ID,
                    lsproto::Method::TEXT_DOCUMENT_SIGNATURE_HELP,
                ),
                (
                    CONTENT_MAPPER_DEFINITION_REGISTRATION_ID,
                    lsproto::Method::TEXT_DOCUMENT_DEFINITION,
                ),
                (
                    CONTENT_MAPPER_TYPE_DEFINITION_REGISTRATION_ID,
                    lsproto::Method::TEXT_DOCUMENT_TYPE_DEFINITION,
                ),
                (
                    CONTENT_MAPPER_IMPLEMENTATION_REGISTRATION_ID,
                    lsproto::Method::TEXT_DOCUMENT_IMPLEMENTATION,
                ),
                (
                    CONTENT_MAPPER_REFERENCES_REGISTRATION_ID,
                    lsproto::Method::TEXT_DOCUMENT_REFERENCES,
                ),
                (
                    CONTENT_MAPPER_DOCUMENT_HIGHLIGHT_REGISTRATION_ID,
                    lsproto::Method::TEXT_DOCUMENT_DOCUMENT_HIGHLIGHT,
                ),
                (
                    CONTENT_MAPPER_COMPLETION_REGISTRATION_ID,
                    lsproto::Method::TEXT_DOCUMENT_COMPLETION,
                ),
                (
                    CONTENT_MAPPER_RENAME_REGISTRATION_ID,
                    lsproto::Method::TEXT_DOCUMENT_RENAME,
                ),
                (
                    CONTENT_MAPPER_SEMANTIC_TOKENS_REGISTRATION_ID,
                    lsproto::Method::TEXT_DOCUMENT_SEMANTIC_TOKENS,
                ),
                (
                    CONTENT_MAPPER_DOCUMENT_SYMBOL_REGISTRATION_ID,
                    lsproto::Method::TEXT_DOCUMENT_DOCUMENT_SYMBOL,
                ),
                (
                    CONTENT_MAPPER_FOLDING_RANGE_REGISTRATION_ID,
                    lsproto::Method::TEXT_DOCUMENT_FOLDING_RANGE,
                ),
                (
                    CONTENT_MAPPER_SELECTION_RANGE_REGISTRATION_ID,
                    lsproto::Method::TEXT_DOCUMENT_SELECTION_RANGE,
                ),
                (
                    CONTENT_MAPPER_INLAY_HINT_REGISTRATION_ID,
                    lsproto::Method::TEXT_DOCUMENT_INLAY_HINT,
                ),
                (
                    CONTENT_MAPPER_CODE_LENS_REGISTRATION_ID,
                    lsproto::Method::TEXT_DOCUMENT_CODE_LENS,
                ),
                (
                    CONTENT_MAPPER_CODE_ACTION_REGISTRATION_ID,
                    lsproto::Method::TEXT_DOCUMENT_CODE_ACTION,
                ),
                (
                    CONTENT_MAPPER_FORMATTING_REGISTRATION_ID,
                    lsproto::Method::TEXT_DOCUMENT_FORMATTING,
                ),
                (
                    CONTENT_MAPPER_RANGE_FORMATTING_REGISTRATION_ID,
                    lsproto::Method::TEXT_DOCUMENT_RANGE_FORMATTING,
                ),
                (
                    CONTENT_MAPPER_ON_TYPE_FORMATTING_REGISTRATION_ID,
                    lsproto::Method::TEXT_DOCUMENT_ON_TYPE_FORMATTING,
                ),
                (
                    CONTENT_MAPPER_LINKED_EDITING_REGISTRATION_ID,
                    lsproto::Method::TEXT_DOCUMENT_LINKED_EDITING_RANGE,
                ),
                (
                    CONTENT_MAPPER_CALL_HIERARCHY_REGISTRATION_ID,
                    lsproto::Method::TEXT_DOCUMENT_PREPARE_CALL_HIERARCHY,
                ),
                (
                    CONTENT_MAPPER_WILL_RENAME_FILES_REGISTRATION_ID,
                    lsproto::Method::WORKSPACE_WILL_RENAME_FILES,
                ),
            ]
            .into_iter()
            .map(|(id, method)| lsproto::Unregistration {
                id: id.to_string(),
                method: method.0.to_string(),
            })
            .collect();
            unregistrations
                .retain(|registration| self.supports_content_mapper_registration(&registration.id));
            if let Err(err) = send_client_request(
                ctx,
                &self.shared,
                &lsproto::CLIENT_UNREGISTER_CAPABILITY_INFO,
                lsproto::UnregistrationParams {
                    unregisterations: unregistrations,
                },
            ) {
                return Err(errors::errorf(
                    format!(
                        "failed to unregister content mapper text document sync: {}",
                        err.error()
                    ),
                    vec![err],
                ));
            }
            self.content_mapper_extensions_registered.set(false);
        }

        if extensions.is_empty() {
            return Ok(());
        }

        let mut filters: Vec<lsproto::TextDocumentFilterLanguageOrSchemeOrPattern> =
            Vec::with_capacity(extensions.len());
        for ext in extensions {
            filters.push(lsproto::TextDocumentFilterLanguageOrSchemeOrPattern {
                pattern: Some(lsproto::TextDocumentFilterPattern {
                    pattern: lsproto::PatternOrRelativePattern {
                        pattern: Some(format!("**/*{ext}")),
                        ..Default::default()
                    },
                    ..Default::default()
                }),
                ..Default::default()
            });
        }
        let selector = lsproto::DocumentSelectorOrNull {
            document_selector: Some(filters),
        };
        let mut content_mapper_file_rename_filters: Vec<lsproto::FileOperationFilter> =
            Vec::with_capacity(extensions.len());
        for extension in extensions {
            content_mapper_file_rename_filters.push(lsproto::FileOperationFilter {
                scheme: Some("file".to_string()),
                pattern: Some(lsproto::FileOperationPattern {
                    glob: format!("**/*{extension}"),
                    ..Default::default()
                }),
            });
        }
        let to_strings =
            |chars: &[&str]| -> Vec<String> { chars.iter().map(|c| c.to_string()).collect() };

        let mut registrations: Vec<lsproto::Registration> = vec![
            lsproto::Registration {
                id: CONTENT_MAPPER_DID_OPEN_REGISTRATION_ID.to_string(),
                register_options: Some(lsproto::RegisterOptions {
                    text_document_did_open: Some(lsproto::TextDocumentRegistrationOptions {
                        document_selector: selector.clone(),
                    }),
                    ..Default::default()
                }),
            },
            lsproto::Registration {
                id: CONTENT_MAPPER_DID_CHANGE_REGISTRATION_ID.to_string(),
                register_options: Some(lsproto::RegisterOptions {
                    text_document_did_change: Some(
                        lsproto::TextDocumentChangeRegistrationOptions {
                            document_selector: selector.clone(),
                            sync_kind: lsproto::TextDocumentSyncKind::INCREMENTAL,
                        },
                    ),
                    ..Default::default()
                }),
            },
            lsproto::Registration {
                id: CONTENT_MAPPER_DID_CLOSE_REGISTRATION_ID.to_string(),
                register_options: Some(lsproto::RegisterOptions {
                    text_document_did_close: Some(lsproto::TextDocumentRegistrationOptions {
                        document_selector: selector.clone(),
                    }),
                    ..Default::default()
                }),
            },
            lsproto::Registration {
                id: CONTENT_MAPPER_DIAGNOSTIC_REGISTRATION_ID.to_string(),
                register_options: Some(lsproto::RegisterOptions {
                    text_document_diagnostic: Some(lsproto::DiagnosticRegistrationOptions {
                        document_selector: selector.clone(),
                        identifier: Some("typescript".to_string()),
                        inter_file_dependencies: true,
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
            },
            lsproto::Registration {
                id: CONTENT_MAPPER_HOVER_REGISTRATION_ID.to_string(),
                register_options: Some(lsproto::RegisterOptions {
                    text_document_hover: Some(lsproto::HoverRegistrationOptions {
                        document_selector: selector.clone(),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
            },
            lsproto::Registration {
                id: CONTENT_MAPPER_SIGNATURE_HELP_REGISTRATION_ID.to_string(),
                register_options: Some(lsproto::RegisterOptions {
                    text_document_signature_help: Some(lsproto::SignatureHelpRegistrationOptions {
                        document_selector: selector.clone(),
                        trigger_characters: Some(to_strings(ls::SIGNATURE_HELP_TRIGGER_CHARACTERS)),
                        retrigger_characters: Some(to_strings(
                            ls::SIGNATURE_HELP_RETRIGGER_CHARACTERS,
                        )),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
            },
            lsproto::Registration {
                id: CONTENT_MAPPER_DEFINITION_REGISTRATION_ID.to_string(),
                register_options: Some(lsproto::RegisterOptions {
                    text_document_definition: Some(lsproto::DefinitionRegistrationOptions {
                        document_selector: selector.clone(),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
            },
            lsproto::Registration {
                id: CONTENT_MAPPER_TYPE_DEFINITION_REGISTRATION_ID.to_string(),
                register_options: Some(lsproto::RegisterOptions {
                    text_document_type_definition: Some(
                        lsproto::TypeDefinitionRegistrationOptions {
                            document_selector: selector.clone(),
                            ..Default::default()
                        },
                    ),
                    ..Default::default()
                }),
            },
            lsproto::Registration {
                id: CONTENT_MAPPER_IMPLEMENTATION_REGISTRATION_ID.to_string(),
                register_options: Some(lsproto::RegisterOptions {
                    text_document_implementation: Some(
                        lsproto::ImplementationRegistrationOptions {
                            document_selector: selector.clone(),
                            ..Default::default()
                        },
                    ),
                    ..Default::default()
                }),
            },
            lsproto::Registration {
                id: CONTENT_MAPPER_REFERENCES_REGISTRATION_ID.to_string(),
                register_options: Some(lsproto::RegisterOptions {
                    text_document_references: Some(lsproto::ReferenceRegistrationOptions {
                        document_selector: selector.clone(),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
            },
            lsproto::Registration {
                id: CONTENT_MAPPER_DOCUMENT_HIGHLIGHT_REGISTRATION_ID.to_string(),
                register_options: Some(lsproto::RegisterOptions {
                    text_document_document_highlight: Some(
                        lsproto::DocumentHighlightRegistrationOptions {
                            document_selector: selector.clone(),
                            ..Default::default()
                        },
                    ),
                    ..Default::default()
                }),
            },
            lsproto::Registration {
                id: CONTENT_MAPPER_COMPLETION_REGISTRATION_ID.to_string(),
                register_options: Some(lsproto::RegisterOptions {
                    text_document_completion: Some(lsproto::CompletionRegistrationOptions {
                        document_selector: selector.clone(),
                        trigger_characters: Some(to_strings(&ls::COMPLETION_TRIGGER_CHARACTERS)),
                        resolve_provider: Some(true),
                        completion_item: Some(lsproto::ServerCompletionItemOptions {
                            label_details_support: Some(true),
                        }),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
            },
            lsproto::Registration {
                id: CONTENT_MAPPER_RENAME_REGISTRATION_ID.to_string(),
                register_options: Some(lsproto::RegisterOptions {
                    text_document_rename: Some(lsproto::RenameRegistrationOptions {
                        document_selector: selector.clone(),
                        prepare_provider: Some(true),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
            },
            lsproto::Registration {
                id: CONTENT_MAPPER_SEMANTIC_TOKENS_REGISTRATION_ID.to_string(),
                register_options: Some(lsproto::RegisterOptions {
                    text_document_semantic_tokens: Some(
                        lsproto::SemanticTokensRegistrationOptions {
                            document_selector: selector.clone(),
                            legend: ls::semantic_tokens_legend(
                                &client_capabilities.text_document.semantic_tokens,
                            ),
                            full: Some(lsproto::BooleanOrSemanticTokensFullDelta {
                                boolean: Some(true),
                                ..Default::default()
                            }),
                            range: Some(lsproto::BooleanOrEmptyObject {
                                boolean: Some(true),
                                ..Default::default()
                            }),
                            ..Default::default()
                        },
                    ),
                    ..Default::default()
                }),
            },
            lsproto::Registration {
                id: CONTENT_MAPPER_DOCUMENT_SYMBOL_REGISTRATION_ID.to_string(),
                register_options: Some(lsproto::RegisterOptions {
                    text_document_document_symbol: Some(
                        lsproto::DocumentSymbolRegistrationOptions {
                            document_selector: selector.clone(),
                            ..Default::default()
                        },
                    ),
                    ..Default::default()
                }),
            },
            lsproto::Registration {
                id: CONTENT_MAPPER_FOLDING_RANGE_REGISTRATION_ID.to_string(),
                register_options: Some(lsproto::RegisterOptions {
                    text_document_folding_range: Some(lsproto::FoldingRangeRegistrationOptions {
                        document_selector: selector.clone(),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
            },
            lsproto::Registration {
                id: CONTENT_MAPPER_SELECTION_RANGE_REGISTRATION_ID.to_string(),
                register_options: Some(lsproto::RegisterOptions {
                    text_document_selection_range: Some(
                        lsproto::SelectionRangeRegistrationOptions {
                            document_selector: selector.clone(),
                            ..Default::default()
                        },
                    ),
                    ..Default::default()
                }),
            },
            lsproto::Registration {
                id: CONTENT_MAPPER_INLAY_HINT_REGISTRATION_ID.to_string(),
                register_options: Some(lsproto::RegisterOptions {
                    text_document_inlay_hint: Some(lsproto::InlayHintRegistrationOptions {
                        document_selector: selector.clone(),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
            },
            lsproto::Registration {
                id: CONTENT_MAPPER_CODE_LENS_REGISTRATION_ID.to_string(),
                register_options: Some(lsproto::RegisterOptions {
                    text_document_code_lens: Some(lsproto::CodeLensRegistrationOptions {
                        document_selector: selector.clone(),
                        resolve_provider: Some(true),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
            },
            lsproto::Registration {
                id: CONTENT_MAPPER_CODE_ACTION_REGISTRATION_ID.to_string(),
                register_options: Some(lsproto::RegisterOptions {
                    text_document_code_action: Some(lsproto::CodeActionRegistrationOptions {
                        document_selector: selector.clone(),
                        code_action_kinds: Some(supported_code_action_kinds()),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
            },
            lsproto::Registration {
                id: CONTENT_MAPPER_FORMATTING_REGISTRATION_ID.to_string(),
                register_options: Some(lsproto::RegisterOptions {
                    text_document_formatting: Some(
                        lsproto::DocumentFormattingRegistrationOptions {
                            document_selector: selector.clone(),
                            ..Default::default()
                        },
                    ),
                    ..Default::default()
                }),
            },
            lsproto::Registration {
                id: CONTENT_MAPPER_RANGE_FORMATTING_REGISTRATION_ID.to_string(),
                register_options: Some(lsproto::RegisterOptions {
                    text_document_range_formatting: Some(
                        lsproto::DocumentRangeFormattingRegistrationOptions {
                            document_selector: selector.clone(),
                            ..Default::default()
                        },
                    ),
                    ..Default::default()
                }),
            },
            lsproto::Registration {
                id: CONTENT_MAPPER_ON_TYPE_FORMATTING_REGISTRATION_ID.to_string(),
                register_options: Some(lsproto::RegisterOptions {
                    text_document_on_type_formatting: Some(
                        lsproto::DocumentOnTypeFormattingRegistrationOptions {
                            document_selector: selector.clone(),
                            first_trigger_character: "{".to_string(),
                            more_trigger_character: Some(vec![
                                "}".to_string(),
                                ";".to_string(),
                                "\n".to_string(),
                            ]),
                        },
                    ),
                    ..Default::default()
                }),
            },
            lsproto::Registration {
                id: CONTENT_MAPPER_LINKED_EDITING_REGISTRATION_ID.to_string(),
                register_options: Some(lsproto::RegisterOptions {
                    text_document_linked_editing_range: Some(
                        lsproto::LinkedEditingRangeRegistrationOptions {
                            document_selector: selector.clone(),
                            ..Default::default()
                        },
                    ),
                    ..Default::default()
                }),
            },
            lsproto::Registration {
                id: CONTENT_MAPPER_CALL_HIERARCHY_REGISTRATION_ID.to_string(),
                register_options: Some(lsproto::RegisterOptions {
                    text_document_prepare_call_hierarchy: Some(
                        lsproto::CallHierarchyRegistrationOptions {
                            document_selector: selector.clone(),
                            ..Default::default()
                        },
                    ),
                    ..Default::default()
                }),
            },
            lsproto::Registration {
                id: CONTENT_MAPPER_WILL_RENAME_FILES_REGISTRATION_ID.to_string(),
                register_options: Some(lsproto::RegisterOptions {
                    workspace_will_rename_files: Some(lsproto::FileOperationRegistrationOptions {
                        filters: content_mapper_file_rename_filters,
                    }),
                    ..Default::default()
                }),
            },
        ];
        registrations
            .retain(|registration| self.supports_content_mapper_registration(&registration.id));
        if let Err(err) = send_client_request(
            ctx,
            &self.shared,
            &lsproto::CLIENT_REGISTER_CAPABILITY_INFO,
            lsproto::RegistrationParams { registrations },
        ) {
            return Err(errors::errorf(
                format!(
                    "failed to register content mapper text document sync: {}",
                    err.error()
                ),
                vec![err],
            ));
        }

        self.content_mapper_extensions_registered.set(true);
        Ok(())
    }

    // Go: server.go:741 RefreshDiagnostics
    // RefreshDiagnostics implements project.Client.
    fn refresh_diagnostics(&self, ctx: &Context) -> Result<(), GoError> {
        if !self
            .shared
            .client_capabilities()
            .workspace
            .diagnostics
            .refresh_support
        {
            return Ok(());
        }

        if let Some(err) = ctx.err() {
            return Err(err);
        }

        // Fire-and-forget: the client always returns null, and waiting for the response
        // can cause the server to hang if the client is slow or unresponsive.
        // Any response from the client will be silently ignored by the read loop.
        if let Err(err) = send_client_request_fire_and_forget(
            &self.shared,
            &lsproto::WORKSPACE_DIAGNOSTIC_REFRESH_INFO,
            lsproto::NoParams,
        ) {
            return Err(errors::errorf(
                format!("failed to refresh diagnostics: {}", err.error()),
                vec![err],
            ));
        }

        Ok(())
    }

    // Go: server.go:761 PublishDiagnostics
    // PublishDiagnostics implements project.Client.
    fn publish_diagnostics(
        &self,
        _ctx: &Context,
        params: lsproto::PublishDiagnosticsParams,
    ) -> Result<(), GoError> {
        send_notification(
            &self.shared,
            &lsproto::TEXT_DOCUMENT_PUBLISH_DIAGNOSTICS_INFO,
            params,
        )
    }

    // Go: server.go:766 SendTelemetry
    // SendTelemetry implements project.Client.
    fn send_telemetry(
        &self,
        _ctx: &Context,
        telemetry: lsproto::TelemetryEvent,
    ) -> Result<(), GoError> {
        if !self.telemetry_enabled.get() {
            crate::core::go_panic("SendTelemetry called with telemetry disabled".to_string());
        }
        send_notification(&self.shared, &lsproto::TELEMETRY_EVENT_INFO, telemetry)
    }

    // Go: server.go:774 IsActive
    // IsActive implements project.Client.
    fn is_active(&self) -> bool {
        let last = self.shared.last_request_time_ms.load(Ordering::SeqCst);
        last == 0 || unix_milli_now() - last <= Duration::from_secs(60).as_millis() as i64
    }

    // Go: server.go:779 RefreshInlayHints
    fn refresh_inlay_hints(&self, _ctx: &Context) -> Result<(), GoError> {
        if !self
            .shared
            .client_capabilities()
            .workspace
            .inlay_hint
            .refresh_support
        {
            return Ok(());
        }

        if let Err(err) = send_client_request_fire_and_forget(
            &self.shared,
            &lsproto::WORKSPACE_INLAY_HINT_REFRESH_INFO,
            lsproto::NoParams,
        ) {
            return Err(errors::errorf(
                format!("failed to refresh inlay hints: {}", err.error()),
                vec![err],
            ));
        }
        Ok(())
    }

    // Go: server.go:790 RefreshCodeLens
    fn refresh_code_lens(&self, _ctx: &Context) -> Result<(), GoError> {
        if !self
            .shared
            .client_capabilities()
            .workspace
            .code_lens
            .refresh_support
        {
            return Ok(());
        }

        if let Err(err) = send_client_request_fire_and_forget(
            &self.shared,
            &lsproto::WORKSPACE_CODE_LENS_REFRESH_INFO,
            lsproto::NoParams,
        ) {
            return Err(errors::errorf(
                format!("failed to refresh code lens: {}", err.error()),
                vec![err],
            ));
        }
        Ok(())
    }

    // Go: server.go:802 ProgressStart
    // ProgressStart implements project.Client.
    fn progress_start(&self, message: &'static crate::diagnostics::Message, args: Vec<String>) {
        if let Some(project_progress) = self.shared.project_progress.get() {
            project_progress.start(message, args);
        }
    }

    // Go: server.go:809 ProgressFinish
    // ProgressFinish implements project.Client.
    fn progress_finish(&self, message: &'static crate::diagnostics::Message, args: Vec<String>) {
        if let Some(project_progress) = self.shared.project_progress.get() {
            project_progress.finish(message, args);
        }
    }

    // Go: server.go:816 GetLocale
    // GetLocale implements project.Client.
    fn get_locale(&self) -> locale::Locale {
        self.shared.locale()
    }

    // Go: server.go:823 SetLocale
    // SetLocale implements project.Client.
    fn set_locale(&self, locale_string: &str) {
        let mut new_locale = self.shared.init_locale();
        if locale_string != "auto" {
            let (parsed, ok) = locale::parse(locale_string);
            if !ok {
                return;
            }
            new_locale = parsed;
        }
        write_lock(&self.shared.locale, new_locale);
    }
}

// Go: server.go:1936 generateDiagnosticDiffString
// PORT: Go `[]*lsproto.Diagnostic` are the borrowed results of
// `lsproto::compare_diagnostics`.
fn generate_diagnostic_diff_string(
    missing_from_pre: &[&lsproto::Diagnostic],
    missing_from_post: &[&lsproto::Diagnostic],
    stringifier: fn(&lsproto::Diagnostic) -> String,
) -> String {
    let mut b = String::new();
    for elem in missing_from_pre {
        b.push_str(&format!(
            "Diagnostic {} was present after emit but not before emit\n",
            stringifier(elem)
        ));
    }
    for elem in missing_from_post {
        b.push_str(&format!(
            "Diagnostic {} was present before emit but not after emit\n",
            stringifier(elem)
        ));
    }
    b
}

/// Go `time.Now().UnixMilli()`.
fn unix_milli_now() -> i64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_millis() as i64,
        Err(e) => -(e.duration().as_millis() as i64),
    }
}

impl Server {
    // Go: server.go:837 RequestConfiguration
    pub fn request_configuration(&self, ctx: &Context) -> Result<lsutil::UserPreferences, GoError> {
        let caps = lsproto::get_client_capabilities(ctx);
        if !caps.workspace.configuration {
            let opts = self.shared.initialization_options();
            if let Some(user_prefs) = &opts.user_preferences {
                // PORT: Go `%T` and `%+v`; log text is not compared.
                self.logger.logf(&format!(
                    "received formatting options from initialization: {}\n{:?}",
                    lsp_any_type_name(user_prefs),
                    user_prefs
                ));
                if let LspAny::Object(config) = user_prefs {
                    let mut items: IndexMap<String, LspAny> = IndexMap::default();
                    items.insert("js/ts".to_string(), LspAny::Object(config.clone()));
                    return Ok(lsutil::parse_user_preferences(&items));
                }
            }
            return Ok(lsutil::new_default_user_preferences());
        }
        let configs = send_client_request(
            ctx,
            &self.shared,
            &lsproto::WORKSPACE_CONFIGURATION_INFO,
            lsproto::ConfigurationParams {
                items: vec![
                    lsproto::ConfigurationItem {
                        section: Some("js/ts".to_string()),
                        ..Default::default()
                    },
                    lsproto::ConfigurationItem {
                        section: Some("typescript".to_string()),
                        ..Default::default()
                    },
                    lsproto::ConfigurationItem {
                        section: Some("javascript".to_string()),
                        ..Default::default()
                    },
                    lsproto::ConfigurationItem {
                        section: Some("editor".to_string()),
                        ..Default::default()
                    },
                ],
            },
        );
        let configs = match configs {
            Ok(configs) => configs,
            Err(err) => {
                return Err(errors::errorf(
                    format!("configure request failed: {}", err.error()),
                    vec![err],
                ));
            }
        };
        // PORT: Go `map[string]any`; `ParseUserPreferences` looks keys up by name.
        let mut config_map: IndexMap<String, LspAny> = IndexMap::default();
        for (i, config) in configs.into_iter().enumerate() {
            match i {
                0 => {
                    config_map.insert("js/ts".to_string(), config);
                }
                1 => {
                    config_map.insert("typescript".to_string(), config);
                }
                2 => {
                    config_map.insert("javascript".to_string(), config);
                }
                3 => {
                    config_map.insert("editor".to_string(), config);
                }
                _ => {}
            }
        }
        // PORT: Go `%+v` of each value (`<nil>` for a missing key); log
        // text is not compared.
        let show = |key: &str| match config_map.get(key) {
            Some(v) => format!("{v:?}"),
            None => "<nil>".to_string(),
        };
        self.logger.logf(&format!(
            "received options from workspace/configuration request:\njs/ts: {}\n\ntypescript: {}\n\njavascript: {}\n\neditor: {}\n",
            show("js/ts"),
            show("typescript"),
            show("javascript"),
            show("editor"),
        ));
        Ok(lsutil::parse_user_preferences(&config_map))
    }
}

/// Go `%T` of the dynamic value of an `any` (log text only).
fn lsp_any_type_name(v: &LspAny) -> &'static str {
    match v {
        LspAny::Null => "<nil>",
        LspAny::Bool(_) => "bool",
        LspAny::Number(_) => "float64",
        LspAny::String(_) => "string",
        LspAny::Array(_) => "[]interface {}",
        LspAny::Object(_) => "map[string]interface {}",
    }
}

/// PORT: a panic on a Go goroutine without `recover` ends the Go process.
/// A Go panic that the port keeps (`core::go_panic`) ends it as the Go
/// runtime does: `panic: <message>` on stderr and `EXIT_GO_PANIC` (2).
/// Any other panic is a port gap and ends it the way `bin/goport.rs` ends a
/// failed run: the unported report on stderr and `EXIT_UNPORTED` (70). The
/// panic hook already printed such a panic.
pub fn go_crash(payload: Box<dyn Any + Send>) -> ! {
    if crate::core::print_go_panic(payload.as_ref()) {
        std::process::exit(crate::core::EXIT_GO_PANIC);
    }
    let mut stderr = std::io::stderr().lock();
    for (name, count) in crate::core::unported_report() {
        let _ = writeln!(stderr, "unported: {name} {count}");
    }
    let _ = stderr.flush();
    std::process::exit(crate::execute::tsc::EXIT_UNPORTED);
}

/// The text of a panic value (Go `%v` of the recovered value).
pub fn panic_value_string(r: &(dyn Any + Send)) -> String {
    if let Some(panic) = r.downcast_ref::<crate::core::GoPanic>() {
        panic.message.clone()
    } else if let Some(message) = r.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = r.downcast_ref::<String>() {
        message.clone()
    } else {
        String::new()
    }
}

impl Server {
    // Go: server.go:895 Run
    // PORT: the dispatch loop runs on the calling thread, because it owns
    // the `!Send` state. Its result joins the group after it returns, so
    // the group keeps the first error in the same order as Go.
    pub fn run(self: &Rc<Self>, ctx: &Context) -> Result<(), GoError> {
        let (g, ctx) = gostd::errgroup::with_context(ctx);
        let _ = self.shared.background_ctx.set(ctx.clone());

        let mut w = self.w.borrow_mut().take().expect("lsp: Run called twice");
        {
            let shared = self.shared.clone();
            let ctx = ctx.clone();
            g.go(move || shared.write_loop(&ctx, &mut *w));
        }

        // Don't run readLoop in the group, as it blocks on stdin read and cannot be cancelled.
        // PORT: `None` on the channel is the wake-up of the waiter's
        // `ctx.Done()` case.
        let (read_loop_err_tx, read_loop_err) = sync_channel::<Option<Result<(), GoError>>>(2);
        {
            let ctx = ctx.clone();
            let wake = read_loop_err_tx.clone();
            g.go(move || {
                // Go:
                //	select {
                //	case <-ctx.Done():
                //		return ctx.Err()
                //	case err := <-readLoopErr:
                //		return err
                //	}
                match recv_or_done(&ctx, &wake, &read_loop_err) {
                    Ok(err) => err,
                    Err(err) => Err(err),
                }
            });
        }
        let mut r = self.r.borrow_mut().take().expect("lsp: Run called twice");
        {
            let shared = self.shared.clone();
            let ctx = ctx.clone();
            // The Go stack size: the read loop decodes the initialize
            // params, whose `LSPAny` values decode one call per level.
            crate::core::GoThread::new()
                .name("lsp-reader".to_string())
                .stack_size(crate::gostd::stack::max_stack_size())
                .spawn(move || {
                    match catch_unwind(AssertUnwindSafe(|| shared.read_loop(&ctx, &mut *r))) {
                        Ok(err) => {
                            let _ = read_loop_err_tx.try_send(Some(err));
                        }
                        Err(payload) => go_crash(payload),
                    }
                });
        }

        let dispatch_result = self.dispatch_loop(&ctx);
        g.go(move || dispatch_result);

        if let Err(err) = g.wait() {
            if !errors::is(&err, &errors::EOF) && ctx.err().is_some() {
                return Err(err);
            }
        }
        Ok(())
    }
}

/// PORT: Go `select { case <-ctx.Done(): return ctx.Err(); case v := <-ch: }`.
/// The channel carries `Option<T>`; a waker on `ctx.Done()` sends `None`
/// through `wake` (a sender of the same channel). When both cases are
/// ready Go picks one at random; the port picks the context.
fn recv_or_done<T: Send + 'static>(
    ctx: &Context,
    wake: &SyncSender<Option<T>>,
    ch: &Receiver<Option<T>>,
) -> Result<T, GoError> {
    let done = ctx.done();
    let waker_id = match &done {
        Some(done) => {
            let wake = wake.clone();
            done.register_waker(move || {
                let _ = wake.try_send(None);
            })
        }
        None => None,
    };
    let unregister = || {
        if let (Some(done), Some(id)) = (&done, waker_id) {
            done.unregister_waker(id);
        }
    };
    loop {
        if let Some(err) = ctx.err() {
            unregister();
            return Err(err);
        }
        match ch.recv() {
            Ok(Some(v)) => {
                unregister();
                return Ok(v);
            }
            // The context wake-up: the next iteration returns ctx.Err().
            Ok(None) => {}
            // Go: a receive from a closed channel gives the zero value; no
            // caller closes its channel before it sends.
            Err(_) => crate::core::go_nil_dereference(),
        }
    }
}

/// PORT: the notifications whose Go handlers call
/// `cancelWarmAutoImportCache` (through `Session.DidOpenFile`,
/// `DidChangeFile`, `DidCloseFile` and `DidChangeWatchedFiles`).
fn cancels_warm_auto_import(method: &lsproto::Method) -> bool {
    *method == lsproto::Method::TEXT_DOCUMENT_DID_OPEN
        || *method == lsproto::Method::TEXT_DOCUMENT_DID_CHANGE
        || *method == lsproto::Method::TEXT_DOCUMENT_DID_CLOSE
        || *method == lsproto::Method::WORKSPACE_DID_CHANGE_WATCHED_FILES
}

/// PORT: the LSP messages that the wait of a call to the client from an
/// API request serves (`wait_during_api_call`). Their Go handlers queue
/// file changes and timers and do not build a snapshot (session.go:363
/// DidCloseFile, :375 DidChangeFile, :410 DidSaveFile; server.go:1834
/// handleSetTrace does nothing). Go's DidChangeFile reads the session's
/// snapshot (`isContentMapperFile`), so it waits while an API request
/// builds the session's next snapshot (project/api.go:18 APIUpdate holds
/// `snapshotMu`). Here it reads the snapshot before that one.
fn served_during_api_call(method: &lsproto::Method) -> bool {
    *method == lsproto::Method::TEXT_DOCUMENT_DID_CHANGE
        || *method == lsproto::Method::TEXT_DOCUMENT_DID_CLOSE
        || *method == lsproto::Method::TEXT_DOCUMENT_DID_SAVE
        || *method == lsproto::Method::SET_TRACE
}

impl ServerShared {
    // Go: server.go:919 readLoop
    // PORT: `r` is the reader, which this thread owns.
    pub fn read_loop(self: &Arc<Self>, ctx: &Context, r: &mut dyn Reader) -> Result<(), GoError> {
        loop {
            if let Some(err) = ctx.err() {
                return Err(err);
            }
            let (msg, err) = r.read();
            if let Some(err) = err {
                if errors::is(&err, &errors::from_value(ErrorCode::INVALID_REQUEST))
                    || errors::is(&err, &errors::from_value(ErrorCode::INVALID_PARAMS))
                {
                    let mut id: Option<crate::jsonrpc::ID> = None;
                    if errors::is(&err, &errors::from_value(ErrorCode::INVALID_PARAMS)) {
                        if let Some(msg) = &msg {
                            if msg.kind == crate::jsonrpc::MessageKind::REQUEST {
                                id = msg.as_request().id.clone();
                            }
                        }
                    }
                    self.send_error(id, err)?;
                    continue;
                }
                return Err(err);
            }
            let msg = msg.unwrap_or_else(|| crate::core::go_nil_dereference());

            if self.initialize_params.get().is_none()
                && msg.kind == crate::jsonrpc::MessageKind::REQUEST
            {
                let req = msg.as_request();
                if req.method == lsproto::Method::INITIALIZE {
                    let params = match lsproto::unmarshal_params::<lsproto::InitializeParams>(req) {
                        Ok(params) => params,
                        Err(err) => {
                            self.send_error(req.id.clone(), err)?;
                            continue;
                        }
                    };
                    let resp = self.handle_initialize(ctx, Some(&params), req)?;
                    self.send_result(req.id.clone(), Box::new(resp))?;
                } else {
                    self.send_error(
                        req.id.clone(),
                        errors::from_value(ErrorCode::SERVER_NOT_INITIALIZED),
                    )?;
                }
                continue;
            }

            if msg.kind == crate::jsonrpc::MessageKind::RESPONSE {
                let resp = msg.into_response();
                let mut pending_server_requests = lock(&self.pending_server_requests);
                let id = resp
                    .id
                    .clone()
                    .unwrap_or_else(|| crate::core::go_nil_dereference());
                // Go: respChan <- resp; close(respChan); delete(...)
                if let Some(resp_chan) = pending_server_requests.remove(&id) {
                    let _ = resp_chan.try_send(Some(resp));
                }
            } else {
                let req = msg.into_request();
                if req.method == lsproto::Method::CANCEL_REQUEST {
                    if let Ok(params) = lsproto::unmarshal_params::<lsproto::CancelParams>(&req) {
                        self.cancel_request(&params.id);
                    }
                } else if let Some(preempt) = self.warm_auto_import_preempt.get() {
                    // PORT: Go's handler cancels the warm when the dispatch
                    // goroutine reaches this message, and other messages run
                    // at the same time as the warm. Here the warm holds the
                    // dispatch thread, so cancel it now, or make it yield.
                    preempt.on_message(
                        cancels_warm_auto_import(&req.method),
                        &*self.logger,
                        || self.queue_request(ctx, QueuedRequest::Request(req)),
                    )?;
                } else {
                    self.queue_request(ctx, QueuedRequest::Request(req))?;
                }
            }
        }
    }

    /// PORT: `request_queue.put` that also counts the item in
    /// `queued_requests`. The count goes up before the put, so the dispatch
    /// loop never sees 0 while an item is in the queue.
    pub fn queue_request(&self, ctx: &Context, item: QueuedRequest) -> Result<(), GoError> {
        self.queued_requests.fetch_add(1, Ordering::SeqCst);
        let result = self.request_queue.put(ctx, item);
        if result.is_err() {
            self.queued_requests.fetch_sub(1, Ordering::SeqCst);
        }
        let _guard = lock(&self.queued_mu);
        self.queued_cond.notify_all();
        result
    }

    /// PORT: waits until `until` while the request queue stays empty.
    /// Returns true if it was empty the whole time, false as soon as an
    /// item is queued. No Go counterpart (see `IDLE_QUIET_PERIOD`).
    pub fn wait_quiet(&self, until: Instant) -> bool {
        let mut guard = lock(&self.queued_mu);
        loop {
            if self.queued_requests.load(Ordering::SeqCst) != 0 {
                return false;
            }
            let now = Instant::now();
            if now >= until {
                return true;
            }
            guard = self
                .queued_cond
                .wait_timeout(guard, until - now)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }

    /// PORT: the wait of a call to the client from an API request
    /// (`ApiConnProtocol`). Returns when a message is in `inbox` or `ctx`
    /// is done, or with an LSP message to serve now, which it takes out of
    /// the request queue. Go serves LSP messages on its dispatch goroutine
    /// while the API goroutine waits (server.go:968 dispatchLoop), and its
    /// process exits when the dispatch loop ends. Here the API request is
    /// in the middle of its work on the session, so the wait serves, in
    /// arrival order, only the messages that do not need the session
    /// (`served_during_api_call`). The first message that needs it stays
    /// in the queue with all messages after it, except `shutdown` and
    /// `exit`, which never wait behind another message. The API reader
    /// queues a `Wake` after each message it puts in `inbox`, so each
    /// message signals `queued_cond`.
    fn wait_during_api_call(
        self: &Arc<Self>,
        ctx: &Context,
        inbox: &ApiInbox,
    ) -> Option<lsproto::RequestMessage> {
        let done = ctx.done();
        let waker = done.as_ref().and_then(|done| {
            let shared = self.clone();
            done.register_waker(move || {
                let _guard = lock(&shared.queued_mu);
                shared.queued_cond.notify_all();
            })
        });
        let mut guard = lock(&self.queued_mu);
        let exit_request = loop {
            if ctx.err().is_some() || !lock(&inbox.messages).is_empty() {
                break None;
            }
            let taken = self.request_queue.with_items(|items| {
                // Whether a message that needs the session is ahead.
                let mut kept = false;
                let index = items.iter().position(|item| {
                    let QueuedRequest::Request(req) = item else {
                        return false;
                    };
                    if req.method == lsproto::Method::SHUTDOWN
                        || req.method == lsproto::Method::EXIT
                    {
                        return true;
                    }
                    kept |= !served_during_api_call(&req.method);
                    !kept
                })?;
                match items.remove(index) {
                    Some(QueuedRequest::Request(req)) => Some(req),
                    _ => None,
                }
            });
            if let Some(req) = taken {
                self.queued_requests.fetch_sub(1, Ordering::SeqCst);
                break Some(req);
            }
            guard = self
                .queued_cond
                .wait(guard)
                .unwrap_or_else(|e| e.into_inner());
        };
        drop(guard);
        if let (Some(done), Some(id)) = (&done, waker) {
            done.unregister_waker(id);
        }
        exit_request
    }

    // Go: server.go:990 cancelRequest
    pub fn cancel_request(&self, raw_id: &lsproto::IntegerOrString) {
        let id = lsproto::new_id(raw_id);
        let mut pending_client_requests = lock(&self.pending_client_requests);
        if let Some(pending_req) = pending_client_requests.get(&id) {
            (pending_req.cancel)();
            pending_client_requests.remove(&id);
        }
    }

    // Go: server.go:1000 read
    // PORT: the reader is owned by the reader thread; `read_loop` calls
    // `r.read()` directly.
}

impl Server {
    // Go: server.go:1004 dispatchLoop
    pub fn dispatch_loop(self: &Rc<Self>, ctx: &Context) -> Result<(), GoError> {
        let (ctx, lsp_exit) = context::with_cancel_cause(ctx);
        // Go: defer lspExit(nil)
        struct LspExitGuard(CancelCauseFunc);
        impl Drop for LspExitGuard {
            fn drop(&mut self) {
                (self.0)(None);
            }
        }
        let _lsp_exit_guard = LspExitGuard(lsp_exit.clone());

        // PORT: the dispatch loop contract. A due `gostd::local` timer wakes
        // this loop through the request queue.
        {
            let shared = self.shared.clone();
            let wake_ctx = ctx.clone();
            gostd::local::set_waker(Arc::new(move || {
                let _ = shared.queue_request(&wake_ctx, QueuedRequest::Wake);
            }));
        }

        *self.dispatch_ctx.borrow_mut() = Some((ctx.clone(), lsp_exit.clone()));
        self.free_since.set(Instant::now());
        gostd::local::keep_garbage();
        {
            // Weak: the thread's queues do not keep the server alive.
            let shared = Arc::downgrade(&self.shared);
            gostd::local::set_stream_check(Box::new(move || {
                shared
                    .upgrade()
                    .is_some_and(|s| next_edit_waits(&s.request_queue))
            }));
        }
        loop {
            self.dispatch_next(&ctx, &lsp_exit)?;
        }
    }

    /// PORT: one turn of the Go dispatch loop: waits for the next item of
    /// the request queue and handles it. `dispatch_loop` calls it, and so
    /// does an API connection while it waits for its next message
    /// (`ApiConnProtocol`). An error is the end of the loop.
    fn dispatch_next(
        self: &Rc<Self>,
        ctx: &Context,
        lsp_exit: &CancelCauseFunc,
    ) -> Result<(), GoError> {
        run_idle_work(
            ctx,
            || self.shared.queued_requests.load(Ordering::SeqCst) != 0,
            |quiet| self.shared.wait_quiet(self.free_since.get() + quiet),
        );

        let item = self.shared.request_queue.get(ctx)?;
        if matches!(item, QueuedRequest::Request(_)) {
            gostd::local::note_message_gap(self.free_since.get().elapsed());
        }
        self.shared.queued_requests.fetch_sub(1, Ordering::SeqCst);
        let req = match item {
            QueuedRequest::Request(req) => Rc::new(req),
            QueuedRequest::Wake => {
                gostd::local::run_pending();
                return Ok(());
            }
            QueuedRequest::ApiAccepted(accepted) => {
                self.serve_api_connection(accepted);
                gostd::local::run_pending();
                return Ok(());
            }
        };

        self.dispatch_request(ctx, lsp_exit, &req);

        gostd::local::run_pending();
        self.free_since.set(Instant::now());
        Ok(())
    }

    /// PORT: the part of one turn of the Go dispatch loop that handles the
    /// request or notification `req`. `dispatch_next` calls it, and so does
    /// the wait of a call to the client from an API request for the LSP
    /// messages that it serves (`ApiConnProtocol`).
    fn dispatch_request(
        self: &Rc<Self>,
        ctx: &Context,
        lsp_exit: &CancelCauseFunc,
        req: &Rc<lsproto::RequestMessage>,
    ) {
        self.shared
            .last_request_time_ms
            .store(unix_milli_now(), Ordering::SeqCst);
        // Go: locale.WithLocale(ctx, s.GetLocale())
        let mut request_ctx = locale::with_locale(ctx, self.shared.locale());
        let mut cancel: Option<CancelFunc> = None;
        if let Some(id) = &req.id {
            let (c, f) = context::with_cancel(&crate::frontend::core_context::with_request_id(
                &request_ctx,
                &id.string(),
            ));
            request_ctx = c;
            cancel = Some(f.clone());
            lock(&self.shared.pending_client_requests).insert(
                id.clone(),
                PendingClientRequest {
                    method: req.method.clone(),
                    cancel: f,
                },
            );
        }

        let handle_error = |err: GoError| {
            if errors::is(&err, &context::CANCELED) {
                if let Err(err) = self.shared.send_error(
                    req.id.clone(),
                    errors::from_value(ErrorCode::REQUEST_CANCELLED),
                ) {
                    lsp_exit(Some(err));
                }
            } else if errors::is(&err, &errors::EOF) {
                lsp_exit(None);
            } else if let Err(err) = self.shared.send_error(req.id.clone(), err) {
                lsp_exit(Some(err));
            }
        };

        let remove_request = || {
            if let Some(id) = &req.id {
                lock(&self.shared.pending_client_requests).remove(id);
                // Go: defer cancel()
                if let Some(cancel) = &cancel {
                    cancel();
                }
            }
        };

        match self.handle_request_or_notification(&request_ctx, req) {
            Err(err) => {
                handle_error(err);
                remove_request();
            }
            Ok(Some(do_async_work)) => {
                // PORT: Go starts the background tasks that the sync
                // part queued (a snapshot update's logging, watch
                // updates and publishDiagnostics) before this goroutine,
                // and they usually end before its answer. Run them
                // first.
                gostd::local::run_pending();
                // PORT: Go runs the async work on a goroutine
                // (`go func() {...}()`); it runs here, on the dispatch
                // thread, before the next message.
                if let Err(ls_error) = do_async_work() {
                    handle_error(ls_error);
                }
                remove_request();
            }
            Ok(None) => remove_request(),
        }
    }
}

/// PORT: the idle part of a dispatch turn, before it takes the next
/// message. Idle work (the auto-import warm) runs only when no message
/// waits: at once or after a quiet period with no message, as the job asks
/// (`wait_quiet(period)` is false when a message comes first), so it does
/// not delay a request that has arrived. Work it queues runs right after
/// it, as it did when the warm ran inside `run_pending`. The frees that the
/// last message or wake-up left (`gostd::local::drop_later`) run after its
/// answer, while no message waits (`busy`), but after a job that starts at
/// once: Go's warm runs on a goroutine from before the answer, and Go's
/// garbage collector frees in the background, so the frees do not delay
/// Go's warm.
fn run_idle_work(
    ctx: &Context,
    busy: impl Fn() -> bool,
    mut wait_quiet: impl FnMut(Duration) -> bool,
) {
    loop {
        let quiet = match gostd::local::next_idle() {
            Some(gostd::local::IdleStart::AtOnce) => Duration::ZERO,
            Some(gostd::local::IdleStart::AfterQuiet) => {
                gostd::local::drop_garbage(&busy);
                IDLE_QUIET_PERIOD
            }
            None => break,
        };
        if ctx.err().is_some() || !wait_quiet(quiet) || !gostd::local::run_idle() {
            break;
        }
        gostd::local::run_pending();
    }
    gostd::local::drop_garbage(busy);
}

/// PORT: how long the dispatch loop waits with an empty request queue
/// before it starts idle work that asks for it (`IdleStart::AfterQuiet`: an
/// attempt of the auto-import warm that is not eager). No Go counterpart: Go runs the
/// warm on a goroutine at once, and a request never waits for it. Here a
/// request that arrives while such an attempt runs waits for it (a file
/// event only until the warm's next context check). Clients send the next
/// message within about a millisecond of an answer (fast typing: didChange
/// and a diagnostic pull), so the attempt starts only when they pause.
pub const IDLE_QUIET_PERIOD: Duration = Duration::from_millis(50);

impl ServerShared {
    // Go: server.go:1065 writeLoop
    // PORT: `w` is the writer, which this thread owns.
    pub fn write_loop(&self, ctx: &Context, w: &mut dyn Writer) -> Result<(), GoError> {
        loop {
            let msg = self.outgoing_queue.get(ctx)?;
            if let Err(err) = w.write(&msg) {
                if let Some(marshal_err) = errors::as_type::<MessageMarshalError>(&err)
                    && msg.kind == crate::jsonrpc::MessageKind::RESPONSE
                {
                    let resp = msg.as_response();
                    if let Some(id) = &resp.id
                        && resp.error.is_none()
                    {
                        self.logger.errorf(&format!(
                            "failed to marshal response for request {}: {}",
                            id.string(),
                            marshal_err
                        ));
                        let marshal_err = new_message_marshal_error(marshal_err.err);
                        self.send_error(Some(id.clone()), marshal_err)?;
                        continue;
                    }
                }
                return Err(errors::errorf(
                    format!("failed to write message: {}", err.error()),
                    vec![err],
                ));
            }
        }
    }
}

// Go: server.go:1089 sendClientRequest
// WARNING: this should only be called in the async portion of a request handler,
// otherwise a deadlock can occur.
// PORT: the reader thread delivers the response, so the dispatch thread can
// wait here in the sync portion too (Go's handleInitialized does).
pub fn send_client_request<
    Req: AnyValue,
    Resp: crate::frontend::json::UnmarshalerFrom + Default + 'static,
>(
    ctx: &Context,
    s: &ServerShared,
    info: &lsproto::RequestInfo<Req, Resp>,
    params: Req,
) -> Result<Resp, GoError> {
    let id = crate::jsonrpc::new_id_string(&format!(
        "ts{}",
        s.client_seq.fetch_add(1, Ordering::SeqCst) + 1
    ));
    let req = info.new_request_message(Some(id.clone()), params);

    let (response_tx, response_chan) = sync_channel::<Option<lsproto::ResponseMessage>>(2);
    lock(&s.pending_server_requests).insert(id.clone(), response_tx.clone());

    let result = (|| {
        s.send(req.message())?;

        // Go:
        //	select {
        //	case <-ctx.Done():
        //		return *new(Resp), ctx.Err()
        //	case resp := <-responseChan:
        let resp = recv_or_done(ctx, &response_tx, &response_chan)?;
        if resp.error.is_some() {
            return Err(errors::new(format!(
                "request failed: {}",
                crate::jsonrpc::ResponseError::string(resp.error.as_ref())
            )));
        }
        info.unmarshal_result(resp.result)
    })();

    // Go: defer: close(respChan); delete(s.pendingServerRequests, *id)
    lock(&s.pending_server_requests).remove(&id);

    result
}

// Go: server.go:1126 sendClientRequestFireAndForget
// sendClientRequestFireAndForget sends a request to the client without waiting for a response.
// The response, if any, will be silently ignored by the read loop since no pending channel is registered.
// This means any error returned by the client will not be observed. Use only for requests where the
// response value is not needed (e.g., the client always returns null).
pub fn send_client_request_fire_and_forget<Req: AnyValue, Resp>(
    s: &ServerShared,
    info: &lsproto::RequestInfo<Req, Resp>,
    params: Req,
) -> Result<(), GoError> {
    let id = crate::jsonrpc::new_id_string(&format!(
        "ts{}",
        s.client_seq.fetch_add(1, Ordering::SeqCst) + 1
    ));
    let req = info.new_request_message(Some(id), params);
    s.send(req.message())
}

impl ServerShared {
    // Go: server.go:1132 sendResult
    pub fn send_result(
        &self,
        id: Option<crate::jsonrpc::ID>,
        result: Box<dyn AnyValue>,
    ) -> Result<(), GoError> {
        self.send_response(lsproto::ResponseMessage {
            id,
            result: Some(result),
            ..Default::default()
        })
    }
}

// Go: server.go:1139 userFacingRequestFailedError
#[derive(Clone, Debug, PartialEq)]
pub struct UserFacingRequestFailedError(pub String);

impl std::fmt::Display for UserFacingRequestFailedError {
    // Go: server.go:1105 Error
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Go `userFacingRequestFailedError(msg)` as an `error`.
/// Go: server.go:1106 Unwrap returns `lsproto.ErrorCodeRequestFailed`.
pub fn user_facing_request_failed_error(msg: String) -> GoError {
    errors::from_value_with_unwrap(
        UserFacingRequestFailedError(msg),
        errors::from_value(ErrorCode::REQUEST_FAILED),
    )
}

impl ServerShared {
    // Go: server.go:1144 sendError
    pub fn send_error(&self, id: Option<crate::jsonrpc::ID>, err: GoError) -> Result<(), GoError> {
        // Do not send error response for notifications,
        // except for parse errors which may occur before determining if the message is a request or notification.
        if id.is_none() && !errors::is(&err, &errors::from_value(ErrorCode::INVALID_REQUEST)) {
            self.logger
                .errorf(&format!("error handling notification: {}", err.error()));
            return Ok(());
        }
        let mut code = ErrorCode::INTERNAL_ERROR;
        if let Some(err_code) = errors::as_type::<ErrorCode>(&err) {
            code = err_code;
        }
        // TODO(jakebailey): error data
        self.send_response(lsproto::ResponseMessage {
            id,
            error: Some(crate::jsonrpc::ResponseError {
                code: code.0,
                message: err.error(),
                ..Default::default()
            }),
            ..Default::default()
        })
    }
}

// Go: server.go:1165 sendNotification
pub fn send_notification<Params: AnyValue>(
    s: &ServerShared,
    info: &lsproto::NotificationInfo<Params>,
    params: Params,
) -> Result<(), GoError> {
    s.send(info.new_notification_message(params).message())
}

impl ServerShared {
    // Go: server.go:1169 sendResponse
    pub fn send_response(&self, resp: lsproto::ResponseMessage) -> Result<(), GoError> {
        self.send(resp.message())
    }

    // Go: server.go:1174 send
    // send writes a message to the outgoing queue, respecting context cancellation.
    pub fn send(&self, msg: lsproto::Message) -> Result<(), GoError> {
        self.outgoing_queue.put(&self.background_ctx(), msg)
    }
}

/// PORT: the async part of a handler. Go `func() error`.
pub type AsyncWork = Box<dyn FnOnce() -> Result<(), GoError>>;

impl Server {
    // Go: server.go:1180 handleRequestOrNotification
    // handleRequestOrNotification looks up the handler for the given request or notification, executes its synchronous work
    // and returns any asynchronous work as a function to be executed by the caller.
    pub fn handle_request_or_notification(
        self: &Rc<Self>,
        ctx: &Context,
        req: &Rc<lsproto::RequestMessage>,
    ) -> Result<Option<AsyncWork>, GoError> {
        let ctx = lsproto::with_client_capabilities(ctx, self.shared.client_capabilities());

        if let Some(handler) = handlers().get(&req.method) {
            let start = Instant::now();
            let result = handler(self, &ctx, req);
            let mut id_str = String::new();
            if let Some(id) = &req.id {
                id_str = format!(" ({})", id.string());
            }
            // PORT: Go prints `time.Duration`; log text is not compared.
            let do_async_work = match result {
                Err(err) => {
                    if let Some(resp) = content_mapper_fallback_response(&req.method, &err) {
                        if !self.logger.is_tracing() {
                            self.logger.info(&format!(
                                "handled method '{}'{} in {:?}",
                                req.method,
                                id_str,
                                start.elapsed()
                            ));
                        }
                        self.shared.send_result(req.id.clone(), resp)?;
                        return Ok(None);
                    }
                    if errors::as_type::<UserFacingRequestFailedError>(&err).is_none() {
                        self.logger.error(&format!(
                            "error handling method '{}'{}: {}",
                            req.method,
                            id_str,
                            err.error()
                        ));
                    } else if !self.logger.is_tracing() {
                        self.logger.info(&format!(
                            "handled method '{}'{} in {:?}",
                            req.method,
                            id_str,
                            start.elapsed()
                        ));
                    }
                    return Err(err);
                }
                Ok(do_async_work) => do_async_work,
            };
            if let Some(do_async_work) = do_async_work {
                let s = self.clone();
                let req = req.clone();
                return Ok(Some(Box::new(move || -> Result<(), GoError> {
                    // note: ctx.Err() has to be checked in the async work to allow async handlers to cleanup resources correctly
                    let async_work_err = do_async_work();
                    let is_user_facing = match &async_work_err {
                        Err(err) => errors::as_type::<UserFacingRequestFailedError>(err).is_some(),
                        Ok(()) => false,
                    };
                    let is_real_error = async_work_err.is_err() && !is_user_facing;
                    if is_real_error {
                        s.logger.info(&format!(
                            "error handling method '{}'{} in {:?}",
                            req.method,
                            id_str,
                            start.elapsed()
                        ));
                    } else if !s.logger.is_tracing() {
                        s.logger.info(&format!(
                            "handled method '{}'{} in {:?}",
                            req.method,
                            id_str,
                            start.elapsed()
                        ));
                    }
                    async_work_err
                }) as AsyncWork));
            }
            if !self.logger.is_tracing() {
                self.logger.info(&format!(
                    "handled method '{}'{} in {:?}",
                    req.method,
                    id_str,
                    start.elapsed()
                ));
            }
            return Ok(None);
        }
        self.logger
            .warn(&format!("unknown method '{}'", req.method));
        if req.id.is_some() {
            self.shared.send_error(
                req.id.clone(),
                errors::from_value(ErrorCode::INVALID_REQUEST),
            )?;
            return Ok(None);
        }
        Ok(None)
    }
}

// Go: server.go:1234 contentMapperFallbackResponse (tsgo#4712)
// contentMapperFallbackResponse returns an empty response for requests made for
// unknown file types not handled by any content mapper. This typically serves a
// short window in time between when the server has unregistered content mapper
// extensions and when the client has stopped sending requests for those file types.
// PORT: Go returns `(any, bool)`; `None` is Go `false`.
pub fn content_mapper_fallback_response(
    method: &lsproto::Method,
    err: &GoError,
) -> Option<Box<dyn AnyValue>> {
    if !errors::is(err, &project::ERR_NO_PROJECT_FOR_UNKNOWN_SCRIPT_KIND) {
        return None;
    }
    if *method == lsproto::Method::TEXT_DOCUMENT_DIAGNOSTIC {
        let resp: lsproto::DocumentDiagnosticResponse =
            lsproto::RelatedFullDocumentDiagnosticReportOrUnchangedDocumentDiagnosticReport {
                full_document_diagnostic_report: Some(
                    lsproto::RelatedFullDocumentDiagnosticReport {
                        items: Vec::new(),
                        ..Default::default()
                    },
                ),
                ..Default::default()
            };
        return Some(Box::new(resp));
    }
    if *method == lsproto::Method::TEXT_DOCUMENT_HOVER
        || *method == lsproto::Method::TEXT_DOCUMENT_SIGNATURE_HELP
        || *method == lsproto::Method::TEXT_DOCUMENT_DEFINITION
        || *method == lsproto::Method::TEXT_DOCUMENT_TYPE_DEFINITION
        || *method == lsproto::Method::TEXT_DOCUMENT_IMPLEMENTATION
        || *method == lsproto::Method::TEXT_DOCUMENT_REFERENCES
        || *method == lsproto::Method::TEXT_DOCUMENT_DOCUMENT_HIGHLIGHT
        || *method == lsproto::Method::TEXT_DOCUMENT_COMPLETION
        || *method == lsproto::Method::TEXT_DOCUMENT_RENAME
    {
        return Some(Box::new(lsproto::Null));
    }
    None
}

// Go: server.go:1263 handlerMap
// handlerMap maps LSP method to a handler function. The handler function executes any work that must be done synchronously
// before other requests/notifications can be processed, and returns any additional work as a function to be executed
// asynchronously after the synchronous work is complete.
// PORT: Go `func(*Server, context.Context, *lsproto.RequestMessage) (func() error, error)`;
// a nil `func() error` is `None`.
pub type Handler = Box<
    dyn Fn(
            &Rc<Server>,
            &Context,
            &Rc<lsproto::RequestMessage>,
        ) -> Result<Option<AsyncWork>, GoError>
        + Send
        + Sync,
>;
pub type HandlerMap = FxHashMap<lsproto::Method, Handler>;

// Go: server.go:1265 handlers
static HANDLERS: LazyLock<HandlerMap> = LazyLock::new(|| {
    let mut handlers = HandlerMap::default();

    register_request_handler(
        &mut handlers,
        lsproto::INITIALIZE_INFO,
        |s: &Rc<Server>,
         ctx: &Context,
         params: Option<&lsproto::InitializeParams>,
         req: &Rc<lsproto::RequestMessage>| s.shared.handle_initialize(ctx, params, req),
    );
    register_notification_handler(
        &mut handlers,
        lsproto::INITIALIZED_INFO,
        Server::handle_initialized,
    );
    register_request_handler(
        &mut handlers,
        lsproto::SHUTDOWN_INFO,
        Server::handle_shutdown,
    );
    register_notification_handler(&mut handlers, lsproto::EXIT_INFO, Server::handle_exit);

    register_notification_handler(
        &mut handlers,
        lsproto::WORKSPACE_DID_CHANGE_CONFIGURATION_INFO,
        Server::handle_did_change_workspace_configuration,
    );
    register_notification_handler(
        &mut handlers,
        lsproto::TEXT_DOCUMENT_DID_OPEN_INFO,
        Server::handle_did_open,
    );
    register_notification_handler(
        &mut handlers,
        lsproto::TEXT_DOCUMENT_DID_CHANGE_INFO,
        Server::handle_did_change,
    );
    register_notification_handler(
        &mut handlers,
        lsproto::TEXT_DOCUMENT_DID_SAVE_INFO,
        Server::handle_did_save,
    );
    register_notification_handler(
        &mut handlers,
        lsproto::TEXT_DOCUMENT_DID_CLOSE_INFO,
        Server::handle_did_close,
    );
    register_notification_handler(
        &mut handlers,
        lsproto::WORKSPACE_DID_CHANGE_WATCHED_FILES_INFO,
        Server::handle_did_change_watched_files,
    );
    register_notification_handler(
        &mut handlers,
        lsproto::SET_TRACE_INFO,
        Server::handle_set_trace,
    );
    register_notification_handler(
        &mut handlers,
        lsproto::CUSTOM_SET_LOG_VERBOSITY_INFO,
        Server::handle_set_log_verbosity,
    );
    register_request_handler(
        &mut handlers,
        lsproto::WORKSPACE_WILL_RENAME_FILES_INFO,
        Server::handle_will_rename_files,
    );

    register_language_service_document_request_handler(
        &mut handlers,
        lsproto::TEXT_DOCUMENT_DIAGNOSTIC_INFO,
        Server::handle_document_diagnostic,
    );
    register_language_service_document_request_handler(
        &mut handlers,
        lsproto::TEXT_DOCUMENT_HOVER_INFO,
        Server::handle_hover,
    );
    register_language_service_document_request_handler(
        &mut handlers,
        lsproto::TEXT_DOCUMENT_DEFINITION_INFO,
        Server::handle_definition,
    );
    register_language_service_document_request_handler(
        &mut handlers,
        lsproto::CUSTOM_TEXT_DOCUMENT_SOURCE_DEFINITION_INFO,
        Server::handle_source_definition,
    );
    register_language_service_document_request_handler(
        &mut handlers,
        lsproto::TEXT_DOCUMENT_TYPE_DEFINITION_INFO,
        Server::handle_type_definition,
    );
    register_language_service_document_request_handler(
        &mut handlers,
        lsproto::TEXT_DOCUMENT_SIGNATURE_HELP_INFO,
        Server::handle_signature_help,
    );
    register_language_service_document_request_handler(
        &mut handlers,
        lsproto::TEXT_DOCUMENT_FORMATTING_INFO,
        Server::handle_document_format,
    );
    register_language_service_document_request_handler(
        &mut handlers,
        lsproto::TEXT_DOCUMENT_RANGE_FORMATTING_INFO,
        Server::handle_document_range_format,
    );
    register_language_service_document_request_handler(
        &mut handlers,
        lsproto::TEXT_DOCUMENT_ON_TYPE_FORMATTING_INFO,
        Server::handle_document_on_type_format,
    );
    register_language_service_document_request_handler(
        &mut handlers,
        lsproto::TEXT_DOCUMENT_DOCUMENT_SYMBOL_INFO,
        Server::handle_document_symbol,
    );
    register_language_service_document_request_handler(
        &mut handlers,
        lsproto::TEXT_DOCUMENT_DOCUMENT_HIGHLIGHT_INFO,
        Server::handle_document_highlight,
    );
    register_language_service_document_request_handler(
        &mut handlers,
        lsproto::CUSTOM_TEXT_DOCUMENT_MULTI_DOCUMENT_HIGHLIGHT_INFO,
        Server::handle_multi_document_highlight,
    );
    register_language_service_document_request_handler(
        &mut handlers,
        lsproto::TEXT_DOCUMENT_SELECTION_RANGE_INFO,
        Server::handle_selection_range,
    );
    register_language_service_document_request_handler(
        &mut handlers,
        lsproto::TEXT_DOCUMENT_INLAY_HINT_INFO,
        Server::handle_inlay_hint,
    );
    register_language_service_document_request_handler(
        &mut handlers,
        lsproto::TEXT_DOCUMENT_CODE_LENS_INFO,
        Server::handle_code_lens,
    );
    register_language_service_document_request_handler(
        &mut handlers,
        lsproto::TEXT_DOCUMENT_CODE_ACTION_INFO,
        Server::handle_code_action,
    );
    register_language_service_document_request_handler(
        &mut handlers,
        lsproto::TEXT_DOCUMENT_PREPARE_CALL_HIERARCHY_INFO,
        Server::handle_prepare_call_hierarchy,
    );
    register_language_service_document_request_handler(
        &mut handlers,
        lsproto::TEXT_DOCUMENT_FOLDING_RANGE_INFO,
        Server::handle_folding_range,
    );
    register_language_service_document_request_handler(
        &mut handlers,
        lsproto::TEXT_DOCUMENT_PREPARE_RENAME_INFO,
        Server::handle_prepare_rename,
    );
    register_language_service_document_request_handler(
        &mut handlers,
        lsproto::TEXT_DOCUMENT_LINKED_EDITING_RANGE_INFO,
        Server::handle_linked_editing_range,
    );

    register_language_service_with_auto_imports_request_handler(
        &mut handlers,
        lsproto::TEXT_DOCUMENT_COMPLETION_INFO,
        Server::handle_completion,
    );
    // This replaces the textDocument/codeAction handler registered above
    // (same as Go: the map keeps the last registration).
    register_language_service_with_auto_imports_request_handler(
        &mut handlers,
        lsproto::TEXT_DOCUMENT_CODE_ACTION_INFO,
        Server::handle_code_action,
    );

    register_language_service_document_request_handler(
        &mut handlers,
        lsproto::TEXT_DOCUMENT_VS_ON_AUTO_INSERT_INFO,
        Server::handle_vs_on_auto_insert,
    );

    register_multi_project_reference_request_handler(
        &mut handlers,
        lsproto::TEXT_DOCUMENT_REFERENCES_INFO,
        ls::LanguageService::provide_references,
    );
    register_multi_project_reference_request_handler(
        &mut handlers,
        lsproto::TEXT_DOCUMENT_VS_REFERENCES_INFO,
        ls::LanguageService::provide_vs_references,
    );
    register_request_handler(
        &mut handlers,
        lsproto::TEXT_DOCUMENT_RENAME_INFO,
        Server::handle_rename,
    );
    register_multi_project_reference_request_handler(
        &mut handlers,
        lsproto::TEXT_DOCUMENT_IMPLEMENTATION_INFO,
        ls::LanguageService::provide_implementations,
    );

    register_request_handler(
        &mut handlers,
        lsproto::CALL_HIERARCHY_INCOMING_CALLS_INFO,
        Server::handle_call_hierarchy_incoming_calls,
    );
    register_request_handler(
        &mut handlers,
        lsproto::CALL_HIERARCHY_OUTGOING_CALLS_INFO,
        Server::handle_call_hierarchy_outgoing_calls,
    );

    register_request_handler(
        &mut handlers,
        lsproto::WORKSPACE_SYMBOL_INFO,
        Server::handle_workspace_symbol,
    );
    register_request_handler(
        &mut handlers,
        lsproto::COMPLETION_ITEM_RESOLVE_INFO,
        Server::handle_completion_item_resolve,
    );
    register_request_handler(
        &mut handlers,
        lsproto::CODE_LENS_RESOLVE_INFO,
        Server::handle_code_lens_resolve,
    );
    register_language_service_document_request_handler(
        &mut handlers,
        lsproto::TEXT_DOCUMENT_SEMANTIC_TOKENS_FULL_INFO,
        Server::handle_semantic_tokens_full,
    );
    register_language_service_document_request_handler(
        &mut handlers,
        lsproto::TEXT_DOCUMENT_SEMANTIC_TOKENS_RANGE_INFO,
        Server::handle_semantic_tokens_range,
    );

    // Developer/debugging commands
    register_request_handler(
        &mut handlers,
        lsproto::CUSTOM_RUN_GC_INFO,
        Server::handle_run_gc,
    );
    register_request_handler(
        &mut handlers,
        lsproto::CUSTOM_SAVE_HEAP_PROFILE_INFO,
        Server::handle_save_heap_profile,
    );
    register_request_handler(
        &mut handlers,
        lsproto::CUSTOM_SAVE_ALLOC_PROFILE_INFO,
        Server::handle_save_alloc_profile,
    );
    register_request_handler(
        &mut handlers,
        lsproto::CUSTOM_START_CPU_PROFILE_INFO,
        Server::handle_start_cpu_profile,
    );
    register_request_handler(
        &mut handlers,
        lsproto::CUSTOM_STOP_CPU_PROFILE_INFO,
        Server::handle_stop_cpu_profile,
    );

    register_request_handler(
        &mut handlers,
        lsproto::CUSTOM_INITIALIZE_API_SESSION_INFO,
        Server::handle_initialize_api_session,
    );
    register_request_handler(
        &mut handlers,
        lsproto::CUSTOM_PROJECT_INFO_INFO,
        Server::handle_project_info,
    );
    register_request_handler(
        &mut handlers,
        lsproto::CUSTOM_SET_CONTENT_MAPPER_CONTRIBUTIONS_INFO,
        Server::handle_set_content_mapper_contributions,
    );
    handlers
});

/// Go `handlers()`, the `sync.OnceValue` of the handler map.
pub fn handlers() -> &'static HandlerMap {
    &HANDLERS
}

// Go: server.go:1336 registerNotificationHandler
// PORT: Go `fn func(*Server, context.Context, Req) error`. `Req` is a
// pointer type (or `NoParams`), so the handler gets `Option<&Req>` (`None`
// is a nil pointer). `lsproto.UnmarshalParams` never gives a nil pointer
// without an error, so the register helpers pass `Some`.
pub fn register_notification_handler<
    Req: crate::frontend::json::UnmarshalerFrom + Default + 'static,
>(
    handlers: &mut HandlerMap,
    info: lsproto::NotificationInfo<Req>,
    fn_: fn(&Rc<Server>, &Context, Option<&Req>) -> Result<(), GoError>,
) {
    handlers.insert(
        info.method.clone(),
        Box::new(
            move |s: &Rc<Server>,
                  ctx: &Context,
                  req: &Rc<lsproto::RequestMessage>|
                  -> Result<Option<AsyncWork>, GoError> {
                if s.session.borrow().is_none() && req.method != lsproto::Method::INITIALIZED {
                    return Err(errors::from_value(ErrorCode::SERVER_NOT_INITIALIZED));
                }

                let params = lsproto::unmarshal_params::<Req>(req)?;
                fn_(s, ctx, Some(&params))?;
                match ctx.err() {
                    Some(err) => Err(err),
                    None => Ok(None),
                }
            },
        ),
    );
}

// Go: server.go:1353 registerRequestHandler
// PORT: `params` as in `register_notification_handler`.
pub fn register_request_handler<
    Req: crate::frontend::json::UnmarshalerFrom + Default + 'static,
    Resp: AnyValue,
>(
    handlers: &mut HandlerMap,
    info: lsproto::RequestInfo<Req, Resp>,
    fn_: fn(
        &Rc<Server>,
        &Context,
        Option<&Req>,
        &Rc<lsproto::RequestMessage>,
    ) -> Result<Resp, GoError>,
) {
    handlers.insert(
        info.method.clone(),
        Box::new(
            move |s: &Rc<Server>,
                  ctx: &Context,
                  req: &Rc<lsproto::RequestMessage>|
                  -> Result<Option<AsyncWork>, GoError> {
                if s.session.borrow().is_none() && req.method != lsproto::Method::INITIALIZE {
                    return Err(errors::from_value(ErrorCode::SERVER_NOT_INITIALIZED));
                }

                let params = lsproto::unmarshal_params::<Req>(req)?;
                let resp = fn_(s, ctx, Some(&params), req)?;
                if let Some(err) = ctx.err() {
                    return Err(err);
                }
                s.shared.send_result(req.id.clone(), Box::new(resp))?;
                Ok(None)
            },
        ),
    );
}

// Go: server.go:1377 registerLanguageServiceDocumentRequestHandler
// PORT: Go calls `params.TextDocumentURI()` in the sync part, which
// dereferences the params pointer, so `fn` gets `&Req`. The async part
// owns the decoded params (Go captures the pointer).
pub fn register_language_service_document_request_handler<
    Req: HasTextDocumentURI + crate::frontend::json::UnmarshalerFrom + Default + 'static,
    Resp: AnyValue,
>(
    handlers: &mut HandlerMap,
    info: lsproto::RequestInfo<Req, Resp>,
    fn_: fn(&Rc<Server>, &Context, &ls::LanguageService, &Req) -> Result<Resp, GoError>,
) {
    handlers.insert(
        info.method.clone(),
        Box::new(
            move |s: &Rc<Server>,
                  ctx: &Context,
                  req: &Rc<lsproto::RequestMessage>|
                  -> Result<Option<AsyncWork>, GoError> {
                let params = lsproto::unmarshal_params::<Req>(req)?;
                let ls = s
                    .session_ref()
                    .get_language_service(ctx, &params.text_document_uri())?;
                let s = s.clone();
                let ctx = ctx.clone();
                let req = req.clone();
                Ok(Some(Box::new(move || {
                    s.recover_guard(
                        &req,
                        || -> Result<(), GoError> { Ok(()) },
                        || {
                            let result = fn_(&s, &ctx, &ls, &params);
                            // After any language service request, check if new global diagnostics were
                            // discovered during checking and push updated tsconfig diagnostics if so.
                            s.session_ref().enqueue_publish_global_diagnostics();
                            let resp = result?;
                            if let Some(err) = ctx.err() {
                                return Err(err);
                            }
                            s.shared.send_result(req.id.clone(), Box::new(resp))
                        },
                    )
                }) as AsyncWork))
            },
        ),
    );
}

// Go: server.go:1404 registerLanguageServiceWithAutoImportsRequestHandler
// PORT: the async part owns the decoded params (Go captures the pointer).
pub fn register_language_service_with_auto_imports_request_handler<
    Req: HasTextDocumentURI + crate::frontend::json::UnmarshalerFrom + Default + 'static,
    Resp: AnyValue,
>(
    handlers: &mut HandlerMap,
    info: lsproto::RequestInfo<Req, Resp>,
    fn_: fn(&Rc<Server>, &Context, &ls::LanguageService, &Req) -> Result<Resp, GoError>,
) {
    let method = info.method.clone();
    handlers.insert(
        info.method.clone(),
        Box::new(
            move |s: &Rc<Server>,
                  ctx: &Context,
                  req: &Rc<lsproto::RequestMessage>|
                  -> Result<Option<AsyncWork>, GoError> {
            let params = lsproto::unmarshal_params::<Req>(req)?;
            let uri = params.text_document_uri();
            let s = s.clone();
            let ctx = ctx.clone();
            let req = req.clone();
            let method = method.clone();
            let session = s.session_ref();
            session.with_language_service_and_snapshot(
                &ctx.clone(),
                &uri.clone(),
                move |language_service, snapshot| {
                    Ok(Some(Box::new(move || {
                        s.recover_guard(
                            &req,
                            || -> Result<(), GoError> { Ok(()) },
                            || {
                                let mut language_service = language_service;
                                let mut result = fn_(&s, &ctx, &language_service, &params);
                                if let Err(ls_err) = &result {
                                    if errors::is(ls_err, &ls::ERR_NEEDS_AUTO_IMPORTS) {
                                        language_service = s
                                            .session_ref()
                                            .get_language_service_with_auto_imports(
                                                &ctx, &snapshot, &uri,
                                            )?;
                                        if let Some(err) = ctx.err() {
                                            return Err(err);
                                        }
                                        result = fn_(&s, &ctx, &language_service, &params);
                                        if let Err(ls_err) = &result {
                                            if errors::is(ls_err, &ls::ERR_NEEDS_AUTO_IMPORTS) {
                                                crate::core::go_panic(format!(
                                                    "{} returned ErrNeedsAutoImports even after enabling auto imports",
                                                    method
                                                ));
                                            }
                                        }
                                    }
                                }
                                let resp = result?;
                                if let Some(err) = ctx.err() {
                                    return Err(err);
                                }
                                s.shared.send_result(req.id.clone(), Box::new(resp))
                            },
                        )
                    }) as Box<dyn FnOnce() -> Result<(), GoError>>))
                },
            )
        }),
    );
}

// Go: server.go:1439 registerMultiProjectReferenceRequestHandler
// PORT: the async part owns the decoded params (Go captures the pointer).
pub fn register_multi_project_reference_request_handler<
    Req: HasTextDocumentPosition + crate::frontend::json::UnmarshalerFrom + Default + 'static,
    Resp: AnyValue,
>(
    handlers: &mut HandlerMap,
    info: lsproto::RequestInfo<Req, Resp>,
    fn_: fn(
        &ls::LanguageService,
        &Context,
        &Req,
        Option<&dyn ls::CrossProjectOrchestrator>,
    ) -> Result<Resp, GoError>,
) {
    handlers.insert(
        info.method.clone(),
        Box::new(
            move |s: &Rc<Server>,
                  ctx: &Context,
                  req: &Rc<lsproto::RequestMessage>|
                  -> Result<Option<AsyncWork>, GoError> {
                let params = lsproto::unmarshal_params::<Req>(req)?;
                // !!! sheetal: multiple projects that contain the file through symlinks
                let (default_ls, orchestrator) = s
                    .get_language_service_and_cross_project_orchestrator(
                        ctx,
                        &params.text_document_uri(),
                        req,
                    )?;
                let s = s.clone();
                let ctx = ctx.clone();
                let req = req.clone();
                Ok(Some(Box::new(move || {
                    s.recover_guard(
                        &req,
                        || -> Result<(), GoError> { Ok(()) },
                        || {
                            let resp = fn_(&default_ls, &ctx, &params, Some(&orchestrator))?;
                            if let Some(err) = ctx.err() {
                                return Err(err);
                            }
                            s.shared.send_result(req.id.clone(), Box::new(resp))
                        },
                    )
                }) as AsyncWork))
            },
        ),
    );
}

// Go: server.go:1467 crossProjectOrchestrator
// PORT: Go `defaultProject *project.Project` is the session's
// `Rc<RefCell<Project>>`. Go stores `req`, which no code reads.
pub struct CrossProjectOrchestrator {
    pub server: Rc<Server>,
    pub req: Rc<lsproto::RequestMessage>,
    pub default_project: Rc<RefCell<project::Project>>,
    pub all_projects: Vec<Rc<dyn ls::Project>>,
}

// Go: server.go:1438 `var _ ls.CrossProjectOrchestrator = (*crossProjectOrchestrator)(nil)`: the impl below.

impl ls::CrossProjectOrchestrator for CrossProjectOrchestrator {
    // Go: server.go:1476 GetDefaultProject
    fn get_default_project(&self) -> Rc<dyn ls::Project> {
        self.default_project.clone()
    }

    // Go: server.go:1480 GetAllProjectsForInitialRequest
    fn get_all_projects_for_initial_request(&self) -> Vec<Rc<dyn ls::Project>> {
        self.all_projects.clone()
    }

    // Go: server.go:1484 GetLanguageServiceForProjectWithFile
    // PORT: Go asserts `p.(*project.Project)`; the session method takes the
    // `ls.Project` itself (see project/session.rs).
    fn get_language_service_for_project_with_file(
        &self,
        ctx: &Context,
        p: &Rc<dyn ls::Project>,
        uri: &lsproto::DocumentUri,
    ) -> Option<ls::LanguageService> {
        self.server
            .session_ref()
            .get_language_service_for_project_with_file(ctx, &**p, uri)
    }

    // Go: server.go:1488 GetProjectsForFile
    fn get_projects_for_file(
        &self,
        ctx: &Context,
        uri: &lsproto::DocumentUri,
    ) -> Result<Vec<Rc<dyn ls::Project>>, GoError> {
        self.server.session_ref().get_projects_for_file(ctx, uri)
    }

    // Go: server.go:1492 GetProjectsLoadingProjectTree
    fn get_projects_loading_project_tree(
        &self,
        ctx: &Context,
        requested_project_trees: &FxHashSet<tspath::Path>,
        yield_: &mut dyn FnMut(Rc<dyn ls::Project>) -> bool,
    ) {
        self.server
            .session_ref()
            .with_snapshot_loading_project_tree(
                ctx,
                Some(requested_project_trees),
                &mut |snapshot: &Rc<Snapshot>| {
                    // ts#64204
                    for p in snapshot.project_collection.language_service_projects() {
                        if !yield_(p) {
                            return;
                        }
                    }
                },
            );
    }
}

impl Server {
    // Go: server.go:1504 getLanguageServiceAndCrossProjectOrchestrator
    // PORT: Go returns the orchestrator only when err is nil.
    pub fn get_language_service_and_cross_project_orchestrator(
        self: &Rc<Self>,
        ctx: &Context,
        uri: &lsproto::DocumentUri,
        req: &Rc<lsproto::RequestMessage>,
    ) -> Result<(ls::LanguageService, CrossProjectOrchestrator), GoError> {
        let (default_project, default_ls, all_projects) = self
            .session_ref()
            .get_language_service_and_projects_for_file(ctx, uri)?;
        let orchestrator = CrossProjectOrchestrator {
            server: self.clone(),
            req: req.clone(),
            default_project,
            all_projects,
        };
        Ok((default_ls, orchestrator))
    }

    // Go: server.go:1513 recover
    // PORT: Go `defer s.recover(req)`; `recover_guard` runs the guarded code
    // in `catch_unwind` and calls this with the panic value.
    // PORT: Go `debug.Stack()` is the stack of the panicking goroutine.
    // Rust unwinding has left that stack here, so this is the current
    // backtrace (empty unless RUST_BACKTRACE is set); `sanitize_stack_trace`
    // finds no Go frames in it and gives "" (log and telemetry text only).
    pub fn recover(&self, req: &lsproto::RequestMessage, r: Box<dyn Any + Send>) {
        let r = panic_value_string(r.as_ref());
        let stack = std::backtrace::Backtrace::capture().to_string();
        self.logger.errorf(&format!(
            "panic handling request {}: {}\n{}",
            req.method, r, stack
        ));
        if req.id.is_some() {
            let code = errors::from_value(ErrorCode::INTERNAL_ERROR);
            let _ = self.shared.send_error(
                req.id.clone(),
                errors::errorf(
                    format!(
                        "{}: panic handling request {}: {}",
                        code.error(),
                        req.method,
                        r
                    ),
                    vec![code],
                ),
            );
        } else {
            // Go: fmt.Sprint adds no spaces next to string operands.
            self.logger.error(&format!(
                "unhandled panic in notification{}{}",
                req.method, r
            ));
        }

        if self.telemetry_enabled.get() {
            let _ = send_notification(
                &self.shared,
                &lsproto::TELEMETRY_EVENT_INFO,
                lsproto::TelemetryEvent {
                    request_failure_telemetry_event: Some(lsproto::RequestFailureTelemetryEvent {
                        properties: Some(lsproto::RequestFailureTelemetryProperties {
                            error_code: ErrorCode::INTERNAL_ERROR.string(),
                            request_method: req.method.0.replace('/', "."),
                            stack: sanitize_stack_trace(&stack),
                        }),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            );
        }
    }

    /// Go `defer s.recover(req)` at the top of a function: runs `f`; when
    /// it panics, recovers the panic with `recover` and returns `zero()`
    /// (Go returns the zero values of the unnamed results).
    pub fn recover_guard<T>(
        &self,
        req: &lsproto::RequestMessage,
        zero: impl FnOnce() -> T,
        f: impl FnOnce() -> T,
    ) -> T {
        match catch_unwind(AssertUnwindSafe(f)) {
            Ok(v) => v,
            Err(r) => {
                self.recover(req, r);
                zero()
            }
        }
    }
}

impl ServerShared {
    // Go: server.go:1537 handleInitialize
    pub fn handle_initialize(
        self: &Arc<Self>,
        _ctx: &Context,
        params: Option<&lsproto::InitializeParams>,
        _req: &lsproto::RequestMessage,
    ) -> Result<lsproto::InitializeResponse, GoError> {
        if self.initialize_params.get().is_some() {
            return Err(errors::from_value(ErrorCode::INVALID_REQUEST));
        }

        self.init_started.store(true, Ordering::SeqCst);

        let params = params.unwrap_or_else(|| crate::core::go_nil_dereference());
        let _ = self.initialize_params.set(params.clone());
        // The spec types initializationOptions as nullable; treat both null and an
        // absent value as empty options so the rest of the server can read fields
        // off s.initializationOptions without nil-checking the container.
        let initialization_options = match &params.initialization_options {
            Some(options) if options.initialization_options.is_some() => options
                .initialization_options
                .clone()
                .expect("checked above"),
            _ => lsproto::InitializationOptions::default(),
        };
        let _ = self.initialization_options.set(initialization_options);
        if let Some(v) = self.initialization_options().log_verbosity {
            if is_valid_log_verbosity(v) {
                self.logger.set_verbosity(v);
            }
        }
        if let Some(level) = self.initialization_options().track_flaky_diagnostics {
            let _ = self.flake_logging.set(level);
        }
        let _ = self
            .client_capabilities
            .set(Arc::new(lsproto::ClientCapabilities::resolve(
                params.capabilities.as_ref(),
            )));
        let client_capabilities = self.client_capabilities();
        if client_capabilities.window.work_done_progress {
            let _ = self.project_progress.set(new_project_loading_progress(
                self.clone(),
                self.progress_delay,
            ));
        }

        let capabilities_json = match json_ext::marshal_indent(&*client_capabilities, "", "\t") {
            Ok(json) => json,
            Err(err) => return Err(errors::from_value(err)),
        };
        self.logger.info(&format!(
            "Resolved client capabilities: {capabilities_json}"
        ));

        let mut position_encoding = lsproto::PositionEncodingKind::UTF16;
        if client_capabilities
            .general
            .position_encodings
            .contains(&lsproto::PositionEncodingKind::UTF8)
        {
            position_encoding = lsproto::PositionEncodingKind::UTF8;
        }
        let _ = self.position_encoding.set(position_encoding.clone());

        if let Some(l) = &params.locale {
            let (parsed, _) = locale::parse(l);
            write_lock(&self.locale, parsed);
        }
        let _ = self.init_locale.set(self.locale());

        if let Some(start_watchdog) = &self.start_watchdog {
            if let Some(process_id) = params.process_id.integer {
                start_watchdog(process_id);
            }
        }

        let response = lsproto::InitializeResult {
            server_info: Some(lsproto::ServerInfo {
                // microsoft/TypeScript 5f647a841a (the TS 7 migration)
                name: "typescript".to_string(),
                version: Some(crate::core::version().to_string()),
            }),
            capabilities: Some(lsproto::ServerCapabilities {
                position_encoding: Some(position_encoding),
                text_document_sync: Some(lsproto::TextDocumentSyncOptionsOrKind {
                    options: Some(lsproto::TextDocumentSyncOptions {
                        open_close: Some(true),
                        change: Some(lsproto::TextDocumentSyncKind::INCREMENTAL),
                        save: Some(lsproto::BooleanOrSaveOptions {
                            boolean: Some(true),
                            ..Default::default()
                        }),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                hover_provider: Some(lsproto::BooleanOrHoverOptions {
                    boolean: Some(true),
                    ..Default::default()
                }),
                definition_provider: Some(lsproto::BooleanOrDefinitionOptions {
                    boolean: Some(true),
                    ..Default::default()
                }),
                type_definition_provider: Some(
                    lsproto::BooleanOrTypeDefinitionOptionsOrTypeDefinitionRegistrationOptions {
                        boolean: Some(true),
                        ..Default::default()
                    },
                ),
                references_provider: Some(lsproto::BooleanOrReferenceOptions {
                    boolean: Some(true),
                    ..Default::default()
                }),
                implementation_provider: Some(
                    lsproto::BooleanOrImplementationOptionsOrImplementationRegistrationOptions {
                        boolean: Some(true),
                        ..Default::default()
                    },
                ),
                diagnostic_provider: Some(lsproto::DiagnosticOptionsOrRegistrationOptions {
                    options: Some(lsproto::DiagnosticOptions {
                        identifier: Some("typescript".to_string()),
                        inter_file_dependencies: true,
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                completion_provider: Some(lsproto::CompletionOptions {
                    trigger_characters: Some(
                        ls::COMPLETION_TRIGGER_CHARACTERS
                            .iter()
                            .map(|c| c.to_string())
                            .collect(),
                    ),
                    resolve_provider: Some(true),
                    completion_item: Some(lsproto::ServerCompletionItemOptions {
                        label_details_support: Some(true),
                    }),
                    ..Default::default()
                }),
                signature_help_provider: Some(lsproto::SignatureHelpOptions {
                    trigger_characters: Some(
                        ls::SIGNATURE_HELP_TRIGGER_CHARACTERS
                            .iter()
                            .map(|c| c.to_string())
                            .collect(),
                    ),
                    retrigger_characters: Some(
                        ls::SIGNATURE_HELP_RETRIGGER_CHARACTERS
                            .iter()
                            .map(|c| c.to_string())
                            .collect(),
                    ),
                    ..Default::default()
                }),
                document_formatting_provider: Some(lsproto::BooleanOrDocumentFormattingOptions {
                    boolean: Some(true),
                    ..Default::default()
                }),
                document_range_formatting_provider: Some(
                    lsproto::BooleanOrDocumentRangeFormattingOptions {
                        boolean: Some(true),
                        ..Default::default()
                    },
                ),
                document_on_type_formatting_provider: Some(lsproto::DocumentOnTypeFormattingOptions {
                    first_trigger_character: "{".to_string(),
                    more_trigger_character: Some(vec![
                        "}".to_string(),
                        ";".to_string(),
                        "\n".to_string(),
                    ]),
                }),
                workspace_symbol_provider: Some(lsproto::BooleanOrWorkspaceSymbolOptions {
                    boolean: Some(true),
                    ..Default::default()
                }),
                document_symbol_provider: Some(lsproto::BooleanOrDocumentSymbolOptions {
                    boolean: Some(true),
                    ..Default::default()
                }),
                folding_range_provider: Some(
                    lsproto::BooleanOrFoldingRangeOptionsOrFoldingRangeRegistrationOptions {
                        boolean: Some(true),
                        ..Default::default()
                    },
                ),
                rename_provider: Some(lsproto::BooleanOrRenameOptions {
                    rename_options: Some(lsproto::RenameOptions {
                        prepare_provider: Some(true),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                document_highlight_provider: Some(lsproto::BooleanOrDocumentHighlightOptions {
                    boolean: Some(true),
                    ..Default::default()
                }),
                selection_range_provider: Some(
                    lsproto::BooleanOrSelectionRangeOptionsOrSelectionRangeRegistrationOptions {
                        boolean: Some(true),
                        ..Default::default()
                    },
                ),
                linked_editing_range_provider: Some(
                    lsproto::BooleanOrLinkedEditingRangeOptionsOrLinkedEditingRangeRegistrationOptions {
                        boolean: Some(true),
                        ..Default::default()
                    },
                ),
                inlay_hint_provider: Some(
                    lsproto::BooleanOrInlayHintOptionsOrInlayHintRegistrationOptions {
                        boolean: Some(true),
                        ..Default::default()
                    },
                ),
                code_lens_provider: Some(lsproto::CodeLensOptions {
                    resolve_provider: Some(true),
                    ..Default::default()
                }),
                code_action_provider: Some(lsproto::BooleanOrCodeActionOptions {
                    code_action_options: Some(lsproto::CodeActionOptions {
                        code_action_kinds: Some(supported_code_action_kinds()),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                call_hierarchy_provider: Some(
                    lsproto::BooleanOrCallHierarchyOptionsOrCallHierarchyRegistrationOptions {
                        boolean: Some(true),
                        ..Default::default()
                    },
                ),
                experimental: Some(lsproto::ExperimentalServerCapabilities {
                    custom_source_definition_provider: Some(true),
                    custom_multi_document_highlight_provider: Some(true),
                }),
                vs_references_provider: Some(true),
                vs_on_auto_insert_provider: Some(lsproto::VSOnAutoInsertOptions {
                    vs_trigger_characters: vec![">".to_string()],
                }),
                workspace: Some(lsproto::WorkspaceOptions {
                    file_operations: Some(lsproto::FileOperationOptions {
                        will_rename: Some(lsproto::FileOperationRegistrationOptions {
                            filters: FILE_RENAME_FILTERS.clone(),
                        }),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                semantic_tokens_provider: Some(lsproto::SemanticTokensOptionsOrRegistrationOptions {
                    options: Some(lsproto::SemanticTokensOptions {
                        legend: ls::semantic_tokens_legend(
                            &client_capabilities.text_document.semantic_tokens,
                        ),
                        full: Some(lsproto::BooleanOrSemanticTokensFullDelta {
                            boolean: Some(true),
                            ..Default::default()
                        }),
                        range: Some(lsproto::BooleanOrEmptyObject {
                            boolean: Some(true),
                            ..Default::default()
                        }),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            }),
        };

        Ok(Some(response))
    }
}

impl Server {
    // Go: server.go:1713 handleInitialized
    pub fn handle_initialized(
        self: &Rc<Self>,
        ctx: &Context,
        _params: Option<&lsproto::InitializedParams>,
    ) -> Result<(), GoError> {
        let mut disable_push_diagnostics = false;
        let mut enable_telemetry = false;
        let initialization_options = self.shared.initialization_options();
        if let Some(v) = initialization_options.disable_push_diagnostics {
            disable_push_diagnostics = v;
        }
        if let Some(v) = initialization_options.enable_telemetry {
            enable_telemetry = v;
        }
        let mut run_external_code = false;
        if let Some(v) = initialization_options.run_external_code {
            run_external_code = v;
        }
        let client_capabilities = self.shared.client_capabilities();
        let has_dynamic_watch_registration = client_capabilities
            .workspace
            .did_change_watched_files
            .dynamic_registration;
        if has_dynamic_watch_registration {
            self.logger.logf(
                "file watching: using LSP client-side watching (client supports dynamic registration)",
            );
            self.watch_enabled.set(true);
        } else if fswatch::default().has_fast_recursive_backend() {
            // The client cannot watch files itself, but the builtin watcher has a
            // backend with efficient recursive watching (Windows or FSEvents), so
            // fall back to watching files in-process.
            self.logger.logf(
                "file watching: using builtin in-process watcher (client lacks dynamic watch registration)",
            );
            self.watch_enabled.set(true);
            let s = self.clone();
            *self.builtin_watcher.borrow_mut() = Some(lspwatcher::new(
                self.fs.clone(),
                Box::new(move |changes: Vec<lsproto::FileEvent>| {
                    let session = s.session.borrow().clone();
                    if let Some(session) = session {
                        // PORT: the watcher gives values; Go's are `*lsproto.FileEvent`.
                        let changes: Vec<Option<lsproto::FileEvent>> =
                            changes.into_iter().map(Some).collect();
                        session.did_change_watched_files(&s.shared.background_ctx(), &changes);
                    }
                }),
                Some(Rc::new(self.logger.clone()) as Rc<dyn logging::Logger>),
            ));
        } else {
            // The client cannot watch files and the builtin watcher backend lacks
            // efficient recursive watching, so file watching is disabled.
            self.logger.logf(
                "file watching: disabled (client lacks dynamic watch registration and builtin watcher backend is not fast-recursive)",
            );
        }

        let mut cwd = self.shared.cwd.clone();
        let initialize_params = self.shared.initialize_params();
        let single_workspace_folder = match &initialize_params.workspace_folders {
            Some(folders) => match &folders.workspace_folders {
                Some(folders) if folders.len() == 1 => Some(&folders[0]),
                _ => None,
            },
            None => None,
        };
        // ts#64159: the workspace folder and root URI give rooted file
        // names, and a root path counts only when it is absolute; each is
        // normalized (server.go:1752-1762).
        if client_capabilities.workspace.workspace_folders && single_workspace_folder.is_some() {
            let folder = single_workspace_folder
                .expect("checked above")
                .as_ref()
                .unwrap_or_else(|| crate::core::go_nil_dereference());
            let file_name = lsproto::DocumentUri(folder.uri.0.clone()).file_name();
            if !file_name.is_empty() {
                cwd = file_name;
            }
        } else if let Some(root_uri) = &initialize_params.root_uri.document_uri {
            let file_name = root_uri.file_name();
            if !file_name.is_empty() {
                cwd = file_name;
            }
        } else if let Some(root_path) = initialize_params
            .root_path
            .as_ref()
            .and_then(|root_path| root_path.string.as_ref())
        {
            if tspath::path_is_absolute(root_path) {
                cwd = lsproto::rooted_path_from_absolute(root_path);
            }
        }

        self.telemetry_enabled.set(enable_telemetry);

        let session = project::new_session(&project::SessionInit {
            background_ctx: lsproto::with_client_capabilities(
                &self.shared.background_ctx(),
                self.shared.client_capabilities(),
            ),
            options: Rc::new(project::SessionOptions {
                current_directory: cwd,
                default_library_path: self.default_library_path.clone(),
                typings_location: self.typings_location.clone(),
                position_encoding: self.shared.position_encoding(),
                watch_enabled: self.watch_enabled.get(),
                logging_enabled: true,
                telemetry_enabled: enable_telemetry,
                debounce_delay: Duration::from_millis(500),
                push_diagnostics_enabled: !disable_push_diagnostics,
                run_external_code,
                checker_pool_options: project::CheckerPoolOptions::default(),
            }),
            fs: self.fs.clone(),
            logger: Some(Rc::new(self.logger.clone())),
            client: Some(self.clone()),
            npm_executor: Some(self.clone()),
            spawner: self.content_mapper_spawner(),
            content_mapper_logger: Some(self.content_mapper_logger()),
            parse_cache: self.parse_cache.clone(),
            content_mapped_parse_cache: None,
        });
        *self.session.borrow_mut() = Some(session.clone());
        {
            // Weak: the session does not keep the server alive.
            let shared = Arc::downgrade(&self.shared);
            session.warm_auto_import_preempt.set_busy(Box::new(move || {
                shared
                    .upgrade()
                    .is_some_and(|s| s.queued_requests.load(Ordering::SeqCst) != 0)
            }));
        }
        let _ = self
            .shared
            .warm_auto_import_preempt
            .set(session.warm_auto_import_preempt.clone());

        let user_preferences = self.request_configuration(ctx)?;
        session.initialize_with_user_config(user_preferences);

        let result = send_client_request(
            ctx,
            &self.shared,
            &lsproto::CLIENT_REGISTER_CAPABILITY_INFO,
            lsproto::RegistrationParams {
                registrations: vec![lsproto::Registration {
                    id: "typescript-config-watch-id".to_string(),
                    register_options: Some(lsproto::RegisterOptions {
                        workspace_did_change_configuration: Some(
                            lsproto::DidChangeConfigurationRegistrationOptions {
                                section: Some(lsproto::StringOrStrings {
                                    strings: Some(vec![
                                        "js/ts".to_string(),
                                        "typescript".to_string(),
                                        "javascript".to_string(),
                                        "editor".to_string(),
                                    ]),
                                    ..Default::default()
                                }),
                            },
                        ),
                        ..Default::default()
                    }),
                }],
            },
        );
        if let Err(err) = result {
            return Err(errors::errorf(
                format!(
                    "failed to register configuration change watcher: {}",
                    err.error()
                ),
                vec![err],
            ));
        }

        // !!! temporary.
        // Remove when we have `handleDidChangeConfiguration`/implicit project config support
        // derived from 'js/ts.implicitProjectConfig.*'.
        let compiler_options_for_inferred_projects =
            self.compiler_options_for_inferred_projects.borrow().clone();
        if compiler_options_for_inferred_projects.is_some() {
            session.did_change_compiler_options_for_inferred_projects(
                ctx,
                compiler_options_for_inferred_projects,
            );
        }

        session.start_performance_telemetry();

        // Go: close(s.initComplete)
        self.init_complete.set(true);
        Ok(())
    }

    // Go: server.go:1827 handleShutdown
    pub fn handle_shutdown(
        self: &Rc<Self>,
        _ctx: &Context,
        _params: Option<&lsproto::NoParams>,
        _req: &Rc<lsproto::RequestMessage>,
    ) -> Result<lsproto::ShutdownResponse, GoError> {
        let builtin_watcher = self.builtin_watcher.borrow().clone();
        if let Some(builtin_watcher) = builtin_watcher {
            builtin_watcher.close();
        }
        // ts#64544
        self.close_api_sessions();
        if self.stopping_api_sessions.borrow().is_empty() {
            self.session_ref().close();
        } else {
            // PORT: an API connection still runs below (see
            // `stopping_api_sessions`); the session closes when it ends.
            self.close_session_after_api_sessions.set(true);
        }
        Ok(lsproto::ShutdownResponse::default())
    }

    // Go: server.go:1836 handleExit
    pub fn handle_exit(
        self: &Rc<Self>,
        _ctx: &Context,
        _params: Option<&lsproto::NoParams>,
    ) -> Result<(), GoError> {
        Err(errors::EOF.clone())
    }

    // Go: server.go:1840 handleDidChangeWorkspaceConfiguration
    pub fn handle_did_change_workspace_configuration(
        self: &Rc<Self>,
        _ctx: &Context,
        params: Option<&lsproto::DidChangeConfigurationParams>,
    ) -> Result<(), GoError> {
        let params = params.unwrap_or_else(|| crate::core::go_nil_dereference());
        if params.settings == LspAny::Null {
            return Ok(());
        } else if let LspAny::Object(settings) = &params.settings {
            self.session_ref()
                .configure(lsutil::parse_user_preferences(settings));
        }
        Ok(())
    }

    // Go: server.go:1849 handleDidOpen
    pub fn handle_did_open(
        self: &Rc<Self>,
        ctx: &Context,
        params: Option<&lsproto::DidOpenTextDocumentParams>,
    ) -> Result<(), GoError> {
        let text_document = params
            .unwrap_or_else(|| crate::core::go_nil_dereference())
            .text_document
            .as_ref()
            .unwrap_or_else(|| crate::core::go_nil_dereference());
        self.session_ref().did_open_file(
            ctx,
            &text_document.uri,
            text_document.version,
            &text_document.text,
            &text_document.language_id,
        );
        Ok(())
    }

    // Go: server.go:1854 handleDidChange
    pub fn handle_did_change(
        self: &Rc<Self>,
        ctx: &Context,
        params: Option<&lsproto::DidChangeTextDocumentParams>,
    ) -> Result<(), GoError> {
        let params = params.unwrap_or_else(|| crate::core::go_nil_dereference());
        self.session_ref().did_change_file(
            ctx,
            &params.text_document.uri,
            params.text_document.version,
            &params.content_changes,
        );
        Ok(())
    }

    // Go: server.go:1859 handleDidSave
    pub fn handle_did_save(
        self: &Rc<Self>,
        ctx: &Context,
        params: Option<&lsproto::DidSaveTextDocumentParams>,
    ) -> Result<(), GoError> {
        let params = params.unwrap_or_else(|| crate::core::go_nil_dereference());
        self.session_ref()
            .did_save_file(ctx, &params.text_document.uri);
        Ok(())
    }

    // Go: server.go:1864 handleDidClose
    pub fn handle_did_close(
        self: &Rc<Self>,
        ctx: &Context,
        params: Option<&lsproto::DidCloseTextDocumentParams>,
    ) -> Result<(), GoError> {
        let params = params.unwrap_or_else(|| crate::core::go_nil_dereference());
        self.session_ref()
            .did_close_file(ctx, &params.text_document.uri);
        Ok(())
    }

    // Go: server.go:1869 handleDidChangeWatchedFiles
    pub fn handle_did_change_watched_files(
        self: &Rc<Self>,
        ctx: &Context,
        params: Option<&lsproto::DidChangeWatchedFilesParams>,
    ) -> Result<(), GoError> {
        let params = params.unwrap_or_else(|| crate::core::go_nil_dereference());
        self.session_ref()
            .did_change_watched_files(ctx, &params.changes);
        Ok(())
    }

    // Go: server.go:1874 handleSetTrace
    pub fn handle_set_trace(
        self: &Rc<Self>,
        _ctx: &Context,
        _params: Option<&lsproto::SetTraceParams>,
    ) -> Result<(), GoError> {
        // $/setTrace is sent by vscode-languageclient when trace settings change.
        // Server log verbosity is controlled separately by custom/setLogVerbosity,
        // so this handler is intentionally a no-op.
        Ok(())
    }

    // Go: server.go:1881 handleSetLogVerbosity
    pub fn handle_set_log_verbosity(
        self: &Rc<Self>,
        _ctx: &Context,
        params: Option<&lsproto::SetLogVerbosityParams>,
    ) -> Result<(), GoError> {
        let params = params.unwrap_or_else(|| crate::core::go_nil_dereference());
        if !is_valid_log_verbosity(params.verbosity) {
            let code = errors::from_value(ErrorCode::INVALID_PARAMS);
            return Err(errors::errorf(
                format!(
                    "{}: invalid log verbosity {}",
                    code.error(),
                    params.verbosity.0
                ),
                vec![code],
            ));
        }
        self.logger.set_verbosity(params.verbosity);
        Ok(())
    }

    // Go: server.go:1889 handleDocumentDiagnostic
    pub fn handle_document_diagnostic(
        self: &Rc<Self>,
        ctx: &Context,
        ls: &ls::LanguageService,
        params: &lsproto::DocumentDiagnosticParams,
    ) -> Result<lsproto::DocumentDiagnosticResponse, GoError> {
        let ctx = crate::frontend::core_context::with_checker_lifetime(
            ctx,
            crate::frontend::core_context::CheckerLifetime::DIAGNOSTICS,
        );
        let flake_logging = self.shared.flake_logging();
        if flake_logging == lsproto::DiagnosticFlakeLogLevel::OFF {
            return ls.provide_diagnostics(&ctx, &params.text_document.uri);
        }
        let direct = ls.provide_diagnostics(&ctx, &params.text_document.uri)?;
        // #4710: Go `languageService.GetProgram().Emit(ctx, ...)`. The emit
        // uses the checkers of the language service program
        // (`ls_program::emit`), so a diagnostic that the emit changes shows
        // up in the second `ProvideDiagnostics`.
        let write_file: program_emit::WriteFile = Arc::new(
            |_file_name: &str,
             _text: &str,
             _data: &mut program_emit::WriteFileData|
             -> Result<(), String> {
                // do nothing
                Ok(())
            },
        );
        ls_program::emit(
            ls.get_program(),
            &ctx,
            program_emit::EmitOptions {
                write_file: Some(write_file),
                ..program_emit::EmitOptions::default()
            },
        );
        let Ok(secondary) = ls.provide_diagnostics(&ctx, &params.text_document.uri) else {
            return Ok(direct);
        };
        let (missing_from_pre, missing_from_post) = lsproto::compare_diagnostics(
            &direct
                .full_document_diagnostic_report
                .as_ref()
                .unwrap_or_else(|| crate::core::go_nil_dereference())
                .items,
            &secondary
                .full_document_diagnostic_report
                .as_ref()
                .unwrap_or_else(|| crate::core::go_nil_dereference())
                .items,
        );
        if missing_from_pre.is_empty() && missing_from_post.is_empty() {
            return Ok(direct);
        }

        let diff = generate_diagnostic_diff_string(
            &missing_from_pre,
            &missing_from_post,
            lsproto::Diagnostic::as_string,
        );

        self.logger.error(&diff);

        if self.telemetry_enabled.get() {
            let sanitized_diff = generate_diagnostic_diff_string(
                &missing_from_pre,
                &missing_from_post,
                lsproto::Diagnostic::code_as_string,
            );
            let _ = send_notification(
                &self.shared,
                &lsproto::TELEMETRY_EVENT_INFO,
                lsproto::TelemetryEvent {
                    request_failure_telemetry_event: Some(lsproto::RequestFailureTelemetryEvent {
                        properties: Some(lsproto::RequestFailureTelemetryProperties {
                            error_code: ErrorCode::INTERNAL_ERROR.string(),
                            request_method: "textDocument.diagnostic.flakeLog".to_string(),
                            stack: sanitized_diff,
                        }),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            );
        }

        if flake_logging == lsproto::DiagnosticFlakeLogLevel::PANIC {
            crate::core::go_panic(format!("flaky diagnostic(s) logged:\n{diff}"));
        }
        Ok(direct)
    }

    // Go: server.go:1947 handleHover
    pub fn handle_hover(
        self: &Rc<Self>,
        ctx: &Context,
        ls: &ls::LanguageService,
        params: &lsproto::HoverParams,
    ) -> Result<lsproto::HoverResponse, GoError> {
        ls.provide_hover(ctx, params)
    }

    // Go: server.go:1951 handlePrepareRename
    pub fn handle_prepare_rename(
        self: &Rc<Self>,
        ctx: &Context,
        language_service: &ls::LanguageService,
        params: &lsproto::PrepareRenameParams,
    ) -> Result<lsproto::PrepareRenameResponse, GoError> {
        let info = language_service.get_rename_info(
            ctx,
            "", /*newName*/
            &params.text_document.uri,
            params.position,
        );
        if !info.can_rename {
            return Err(user_facing_request_failed_error(
                info.localized_error_message,
            ));
        }
        Ok(lsproto::PrepareRenameResponse {
            prepare_rename_placeholder: Some(lsproto::PrepareRenamePlaceholder {
                range: info.trigger_span,
                placeholder: info.display_name,
            }),
            ..Default::default()
        })
    }

    // Go: server.go:1964 handleRename
    pub fn handle_rename(
        self: &Rc<Self>,
        ctx: &Context,
        params: Option<&lsproto::RenameParams>,
        req: &Rc<lsproto::RequestMessage>,
    ) -> Result<lsproto::RenameResponse, GoError> {
        let params = params.unwrap_or_else(|| crate::core::go_nil_dereference());
        let (default_ls, orchestrator) = self.get_language_service_and_cross_project_orchestrator(
            ctx,
            &params.text_document.uri,
            req,
        )?;
        let info = default_ls.get_rename_info(
            ctx,
            &params.new_name,
            &params.text_document.uri,
            params.position,
        );
        if info.can_rename && !info.file_to_rename.is_empty() {
            // We send a `willRenameFiles` request if the client allows;
            // otherwise we directly compute the edits for renaming the file.
            if ls::client_supports_will_rename_files(ctx) {
                let document_changes = vec![
                    lsproto::TextDocumentEditOrCreateFileOrRenameFileOrDeleteFile {
                        rename_file: Some(lsproto::RenameFile {
                            kind: lsproto::StringLiteralRename,
                            old_uri: lsconv::file_name_to_document_uri(&info.file_to_rename),
                            new_uri: lsconv::file_name_to_document_uri(&info.new_file_name),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                ];
                return Ok(lsproto::WorkspaceEditOrNull {
                    workspace_edit: Some(lsproto::WorkspaceEdit {
                        document_changes: Some(document_changes),
                        ..Default::default()
                    }),
                });
            }
            let rename_files_params = lsproto::RenameFilesParams {
                files: vec![Some(lsproto::FileRename {
                    old_uri: lsconv::file_name_to_document_uri(&info.file_to_rename),
                    new_uri: lsconv::file_name_to_document_uri(&info.new_file_name),
                })],
            };
            return self.handle_will_rename_files_worker(
                ctx,
                &rename_files_params,
                req,
                true, /*sendRenameFile*/
            );
        }

        default_ls.provide_rename(ctx, params, Some(&orchestrator))
    }

    // Go: server.go:2001 handleWillRenameFiles
    pub fn handle_will_rename_files(
        self: &Rc<Self>,
        ctx: &Context,
        params: Option<&lsproto::RenameFilesParams>,
        msg: &Rc<lsproto::RequestMessage>,
    ) -> Result<lsproto::WillRenameFilesResponse, GoError> {
        self.handle_will_rename_files_worker(
            ctx,
            params.unwrap_or_else(|| crate::core::go_nil_dereference()),
            msg,
            false, /*sendRenameFile*/
        )
    }

    // Go: server.go:2008 handleWillRenameFilesWorker
    // If `sendRenameFile` is true, the original `willRenameFiles` request is being handled as part of a rename operation
    // where the client doesn't support `willRenameFiles`,
    // so we should include the file rename in the edits we return
    pub fn handle_will_rename_files_worker(
        self: &Rc<Self>,
        ctx: &Context,
        params: &lsproto::RenameFilesParams,
        _req: &Rc<lsproto::RequestMessage>,
        send_rename_file: bool,
    ) -> Result<lsproto::WillRenameFilesResponse, GoError> {
        if params.files.is_empty() {
            return Ok(lsproto::WillRenameFilesResponse::default());
        }

        let mut uris: Vec<lsproto::DocumentUri> = Vec::with_capacity(params.files.len());
        for file in &params.files {
            // Go: file.OldUri (a nil element panics)
            let file = file
                .as_ref()
                .unwrap_or_else(|| crate::core::go_nil_dereference());
            uris.push(file.old_uri.clone());
        }

        if uris.is_empty() {
            return Ok(lsproto::WillRenameFilesResponse::default());
        }

        let services = self
            .session_ref()
            .get_language_services_for_documents_loading_project_tree(ctx, &uris);

        // Go: type editKey struct { uri lsproto.DocumentUri; range_ lsproto.Range }
        let mut seen_edits: FxHashMap<(lsproto::DocumentUri, lsproto::Range), String> =
            FxHashMap::default();
        let mut seen_renames: FxHashSet<lsproto::DocumentUri> = FxHashSet::default();
        let mut document_changes: Vec<
            lsproto::TextDocumentEditOrCreateFileOrRenameFileOrDeleteFile,
        > = Vec::new();

        for language_service in &services {
            // PORT: every service is alive here; make this one's program
            // current while it runs (ls::LanguageService::enter_program).
            let _program = language_service.enter_program();
            for file in &params.files {
                let file = file
                    .as_ref()
                    .unwrap_or_else(|| crate::core::go_nil_dereference());
                let changes =
                    language_service.get_edits_for_file_rename(ctx, &file.old_uri, &file.new_uri);
                for change in changes {
                    if let Some(rename_file) = &change.rename_file {
                        if !seen_renames.contains(&rename_file.old_uri) {
                            seen_renames.insert(rename_file.old_uri.clone());
                            document_changes.push(change);
                        }
                    } else if let Some(text_document_edit) = &change.text_document_edit {
                        let uri = text_document_edit.text_document.uri.clone();
                        let mut deduped: Vec<
                            lsproto::TextEditOrAnnotatedTextEditOrSnippetTextEdit,
                        > = Vec::new();
                        for edit in &text_document_edit.edits {
                            if let Some(text_edit) = &edit.text_edit {
                                let key = (uri.clone(), text_edit.range);
                                if seen_edits
                                    .get(&key)
                                    .is_some_and(|prev| *prev == text_edit.new_text)
                                {
                                    continue;
                                }
                                seen_edits.insert(key, text_edit.new_text.clone());
                            }
                            deduped.push(edit.clone());
                        }
                        if !deduped.is_empty() {
                            document_changes.push(
                                lsproto::TextDocumentEditOrCreateFileOrRenameFileOrDeleteFile {
                                    text_document_edit: Some(lsproto::TextDocumentEdit {
                                        text_document: text_document_edit.text_document.clone(),
                                        edits: deduped,
                                    }),
                                    ..Default::default()
                                },
                            );
                        }
                    }
                }
            }
        }

        if send_rename_file {
            for file in &params.files {
                let file = file
                    .as_ref()
                    .unwrap_or_else(|| crate::core::go_nil_dereference());
                document_changes.push(
                    lsproto::TextDocumentEditOrCreateFileOrRenameFileOrDeleteFile {
                        rename_file: Some(lsproto::RenameFile {
                            kind: lsproto::StringLiteralRename,
                            old_uri: file.old_uri.clone(),
                            new_uri: file.new_uri.clone(),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                );
            }
        }

        if document_changes.is_empty() {
            return Ok(lsproto::WillRenameFilesResponse::default());
        }

        if ls::client_supports_document_changes(ctx) {
            return Ok(lsproto::WillRenameFilesResponse {
                workspace_edit: Some(lsproto::WorkspaceEdit {
                    document_changes: Some(document_changes),
                    ..Default::default()
                }),
            });
        }

        // PORT: Go map order is random; the oracle compares this map
        // without order. Insertion order here.
        let mut changes: IndexMap<lsproto::DocumentUri, Vec<Option<lsproto::TextEdit>>> =
            IndexMap::default();
        for change in &document_changes {
            if let Some(text_document_edit) = &change.text_document_edit {
                let uri = text_document_edit.text_document.uri.clone();
                for edit in &text_document_edit.edits {
                    if let Some(text_edit) = &edit.text_edit {
                        changes
                            .entry(uri.clone())
                            .or_default()
                            .push(Some(text_edit.clone()));
                    }
                }
            }
        }

        Ok(lsproto::WillRenameFilesResponse {
            workspace_edit: Some(lsproto::WorkspaceEdit {
                changes: Some(changes),
                ..Default::default()
            }),
        })
    }

    // Go: server.go:2110 handleSignatureHelp
    pub fn handle_signature_help(
        self: &Rc<Self>,
        ctx: &Context,
        language_service: &ls::LanguageService,
        params: &lsproto::SignatureHelpParams,
    ) -> Result<lsproto::SignatureHelpResponse, GoError> {
        language_service.provide_signature_help(
            ctx,
            &params.text_document.uri,
            params.position,
            params.context.as_ref(),
        )
    }

    // Go: server.go:2119 handleFoldingRange
    pub fn handle_folding_range(
        self: &Rc<Self>,
        ctx: &Context,
        ls: &ls::LanguageService,
        params: &lsproto::FoldingRangeParams,
    ) -> Result<lsproto::FoldingRangeResponse, GoError> {
        ls.provide_folding_range(ctx, &params.text_document.uri)
    }

    // Go: server.go:2123 handleVSOnAutoInsert
    pub fn handle_vs_on_auto_insert(
        self: &Rc<Self>,
        ctx: &Context,
        ls: &ls::LanguageService,
        params: &lsproto::VSOnAutoInsertParams,
    ) -> Result<lsproto::VSOnAutoInsertResponse, GoError> {
        ls.provide_on_auto_insert(ctx, params)
    }

    // Go: server.go:2127 handleLinkedEditingRange
    pub fn handle_linked_editing_range(
        self: &Rc<Self>,
        ctx: &Context,
        ls: &ls::LanguageService,
        params: &lsproto::LinkedEditingRangeParams,
    ) -> Result<lsproto::LinkedEditingRangeResponse, GoError> {
        ls.provide_linked_editing_range(ctx, params)
    }

    // Go: server.go:2131 handleDefinition
    pub fn handle_definition(
        self: &Rc<Self>,
        ctx: &Context,
        ls: &ls::LanguageService,
        params: &lsproto::DefinitionParams,
    ) -> Result<lsproto::DefinitionResponse, GoError> {
        ls.provide_definition(ctx, &params.text_document.uri, params.position)
    }

    // Go: server.go:2135 handleSourceDefinition
    pub fn handle_source_definition(
        self: &Rc<Self>,
        ctx: &Context,
        ls: &ls::LanguageService,
        params: &lsproto::TextDocumentPositionParams,
    ) -> Result<lsproto::CustomTextDocumentSourceDefinitionResponse, GoError> {
        let resp = ls.provide_source_definition(ctx, &params.text_document.uri, params.position)?;
        Ok(Some(resp))
    }

    // Go: server.go:2143 handleTypeDefinition
    pub fn handle_type_definition(
        self: &Rc<Self>,
        ctx: &Context,
        ls: &ls::LanguageService,
        params: &lsproto::TypeDefinitionParams,
    ) -> Result<lsproto::TypeDefinitionResponse, GoError> {
        ls.provide_type_definition(ctx, &params.text_document.uri, params.position)
    }

    // Go: server.go:2147 handleCompletion
    pub fn handle_completion(
        self: &Rc<Self>,
        ctx: &Context,
        language_service: &ls::LanguageService,
        params: &lsproto::CompletionParams,
    ) -> Result<lsproto::CompletionResponse, GoError> {
        language_service.provide_completion(
            ctx,
            &params.text_document.uri,
            params.position,
            params.context.as_ref(),
        )
    }

    // Go: server.go:2156 handleCompletionItemResolve
    pub fn handle_completion_item_resolve(
        self: &Rc<Self>,
        ctx: &Context,
        params: Option<&lsproto::CompletionItem>,
        req_msg: &Rc<lsproto::RequestMessage>,
    ) -> Result<lsproto::CompletionResolveResponse, GoError> {
        let params = params.unwrap_or_else(|| crate::core::go_nil_dereference());
        let Some(data) = params.data.clone() else {
            return Err(errors::new("completion item data is nil"));
        };
        // ts#64544: the file name must be absolute, and a dynamic one must
        // decode to a URI (server.go:2161). ts#64159: it is rooted and
        // normalized first (TryRootedFilePathFromAbsolute).
        // PORT: Go also passes the normalized name to ResolveCompletionItem
        // (ls lane), which finds the file by it; the port's
        // `resolve_completion_item` reads `data.file_name`.
        let Some(file_name) = lsproto::try_rooted_path_from_absolute(&data.file_name) else {
            return Err(errors::new(
                "completion item data fileName must be absolute",
            ));
        };
        let uri = if tspath::is_dynamic_file_name(&file_name) {
            match lsproto::try_dynamic_file_name_to_document_uri(&file_name) {
                Some(uri) => uri,
                None => {
                    return Err(errors::new(
                        "completion item data fileName must be a valid dynamic path",
                    ));
                }
            }
        } else {
            lsconv::file_name_to_document_uri(&file_name)
        };
        let language_service = self.session_ref().get_language_service(ctx, &uri)?;
        self.recover_guard(
            req_msg,
            || Ok(None),
            || {
                language_service
                    .resolve_completion_item(ctx, params.clone(), Some(data))
                    .map(Some)
            },
        )
    }

    // Go: server.go:2182 handleDocumentFormat
    pub fn handle_document_format(
        self: &Rc<Self>,
        ctx: &Context,
        ls: &ls::LanguageService,
        params: &lsproto::DocumentFormattingParams,
    ) -> Result<lsproto::DocumentFormattingResponse, GoError> {
        ls.provide_format_document(
            ctx,
            &params.text_document.uri,
            params
                .options
                .as_ref()
                .unwrap_or_else(|| crate::core::go_nil_dereference()),
        )
    }

    // Go: server.go:2190 handleDocumentRangeFormat
    pub fn handle_document_range_format(
        self: &Rc<Self>,
        ctx: &Context,
        ls: &ls::LanguageService,
        params: &lsproto::DocumentRangeFormattingParams,
    ) -> Result<lsproto::DocumentRangeFormattingResponse, GoError> {
        ls.provide_format_document_range(
            ctx,
            &params.text_document.uri,
            params
                .options
                .as_ref()
                .unwrap_or_else(|| crate::core::go_nil_dereference()),
            params.range,
        )
    }

    // Go: server.go:2199 handleDocumentOnTypeFormat
    pub fn handle_document_on_type_format(
        self: &Rc<Self>,
        ctx: &Context,
        ls: &ls::LanguageService,
        params: &lsproto::DocumentOnTypeFormattingParams,
    ) -> Result<lsproto::DocumentOnTypeFormattingResponse, GoError> {
        ls.provide_format_document_on_type(
            ctx,
            &params.text_document.uri,
            params
                .options
                .as_ref()
                .unwrap_or_else(|| crate::core::go_nil_dereference()),
            params.position,
            &params.ch,
        )
    }

    // Go: server.go:2209 handleWorkspaceSymbol
    pub fn handle_workspace_symbol(
        self: &Rc<Self>,
        ctx: &Context,
        params: Option<&lsproto::WorkspaceSymbolParams>,
        req_msg: &Rc<lsproto::RequestMessage>,
    ) -> Result<lsproto::WorkspaceSymbolResponse, GoError> {
        let params = params.unwrap_or_else(|| crate::core::go_nil_dereference());
        let mut resp = lsproto::WorkspaceSymbolResponse::default();
        let mut ls_err: Option<GoError> = None;
        // PORT: Go maps the projects to their programs before this call (a
        // nil program stays nil) and reads a nil program in
        // `ProvideWorkspaceSymbols`, under the recover. The port's list
        // holds no nil program, so `programs` reads each project's program
        // inside the recover, where a `None` program panics as Go does.
        let mut provide_symbols =
            |snapshot: &Rc<Snapshot>, programs: &dyn Fn() -> Vec<Rc<compiler::NewProgram>>| {
                self.recover_guard(
                    req_msg,
                    || (),
                    || match ls::provide_workspace_symbols(
                        ctx,
                        &programs(),
                        &snapshot.converters(),
                        &snapshot.user_preferences(),
                        &params.query,
                    ) {
                        Ok(r) => {
                            resp = r;
                            ls_err = None;
                        }
                        Err(err) => {
                            resp = lsproto::WorkspaceSymbolResponse::default();
                            ls_err = Some(err);
                        }
                    },
                );
            };
        let session = self.session_ref();
        if let Some(text_document) = &params.text_document
            && session.config().workspace_symbols_scope
                == lsutil::WorkspaceSymbolsScope::CURRENT_PROJECT
        {
            let uri = &text_document.uri;
            session.with_snapshot_for_document(ctx, uri, &mut |snapshot: &Rc<Snapshot>| {
                // Go: core.Map(snapshot.GetLanguageServiceProjectsContainingFile(uri), ls.Project.GetProgram) (ts#64204)
                let projects = snapshot.get_language_service_projects_containing_file(uri);
                provide_symbols(snapshot, &|| {
                    projects
                        .iter()
                        .map(|p| {
                            p.get_program()
                                .unwrap_or_else(|| crate::core::go_nil_dereference())
                        })
                        .collect()
                });
            });
        } else {
            session.with_snapshot_loading_project_tree(ctx, None, &mut |snapshot: &Rc<
                Snapshot,
            >| {
                // Go: core.Map(snapshot.ProjectCollection.LanguageServiceProjects(), (*project.Project).GetProgram) (ts#64204)
                let projects = snapshot.project_collection.language_service_projects();
                provide_symbols(snapshot, &|| {
                    projects
                        .iter()
                        .map(|p| {
                            p.borrow()
                                .get_program()
                                .unwrap_or_else(|| crate::core::go_nil_dereference())
                        })
                        .collect()
                });
            });
        }
        match ls_err {
            Some(err) => Err(err),
            None => Ok(resp),
        }
    }

    // Go: server.go:2237 handleDocumentSymbol
    pub fn handle_document_symbol(
        self: &Rc<Self>,
        ctx: &Context,
        ls: &ls::LanguageService,
        params: &lsproto::DocumentSymbolParams,
    ) -> Result<lsproto::DocumentSymbolResponse, GoError> {
        ls.provide_document_symbols(ctx, &params.text_document.uri)
    }

    // Go: server.go:2241 handleDocumentHighlight
    pub fn handle_document_highlight(
        self: &Rc<Self>,
        ctx: &Context,
        ls: &ls::LanguageService,
        params: &lsproto::DocumentHighlightParams,
    ) -> Result<lsproto::DocumentHighlightResponse, GoError> {
        ls.provide_document_highlights(ctx, &params.text_document.uri, params.position)
    }

    // Go: server.go:2245 handleMultiDocumentHighlight
    pub fn handle_multi_document_highlight(
        self: &Rc<Self>,
        ctx: &Context,
        ls: &ls::LanguageService,
        params: &lsproto::MultiDocumentHighlightParams,
    ) -> Result<lsproto::CustomMultiDocumentHighlightResponse, GoError> {
        ls.provide_multi_document_highlights(
            ctx,
            &params.text_document.uri,
            params.position,
            &params.files_to_search,
        )
    }

    // Go: server.go:2249 handleSelectionRange
    pub fn handle_selection_range(
        self: &Rc<Self>,
        ctx: &Context,
        ls: &ls::LanguageService,
        params: &lsproto::SelectionRangeParams,
    ) -> Result<lsproto::SelectionRangeResponse, GoError> {
        ls.provide_selection_ranges(ctx, params)
    }

    // Go: server.go:2253 handleCodeAction
    pub fn handle_code_action(
        self: &Rc<Self>,
        ctx: &Context,
        ls: &ls::LanguageService,
        params: &lsproto::CodeActionParams,
    ) -> Result<lsproto::CodeActionResponse, GoError> {
        ls.provide_code_actions(ctx, params)
    }

    // Go: server.go:2257 handleInlayHint
    pub fn handle_inlay_hint(
        self: &Rc<Self>,
        ctx: &Context,
        language_service: &ls::LanguageService,
        params: &lsproto::InlayHintParams,
    ) -> Result<lsproto::InlayHintResponse, GoError> {
        language_service.provide_inlay_hint(ctx, params)
    }

    // Go: server.go:2265 handleCodeLens
    pub fn handle_code_lens(
        self: &Rc<Self>,
        ctx: &Context,
        ls: &ls::LanguageService,
        params: &lsproto::CodeLensParams,
    ) -> Result<lsproto::CodeLensResponse, GoError> {
        ls.provide_code_lenses(ctx, &params.text_document.uri)
    }

    // Go: server.go:2269 handleCodeLensResolve
    pub fn handle_code_lens_resolve(
        self: &Rc<Self>,
        ctx: &Context,
        code_lens: Option<&lsproto::CodeLens>,
        req_msg: &Rc<lsproto::RequestMessage>,
    ) -> Result<lsproto::CodeLensResolveResponse, GoError> {
        let code_lens = code_lens.unwrap_or_else(|| crate::core::go_nil_dereference());
        let result = self.get_language_service_and_cross_project_orchestrator(
            ctx,
            &code_lens
                .data
                .as_ref()
                .unwrap_or_else(|| crate::core::go_nil_dereference())
                .uri,
            req_msg,
        );
        if let Some(err) = ctx.err() {
            return Err(err);
        }
        let (default_ls, orchestrator) = match result {
            Ok(v) => v,
            Err(_) => {
                // This can happen if a codeLens/resolve request comes in after a program change.
                // While it's true that handlers should latch onto a specific snapshot
                // while processing requests, we just set `Data.Uri` based on
                // some older snapshot's contents. The content could have been modified,
                // or the file itself could have been removed from the session entirely.
                // Note this won't bail out on every change, but will prevent crashing
                // based on non-existent files and line maps from shortened files.
                // PORT: Go also returns `codeLens`; the error wins.
                return Err(errors::from_value(ErrorCode::CONTENT_MODIFIED));
            }
        };
        self.recover_guard(
            req_msg,
            || Ok(None),
            || {
                default_ls
                    .resolve_code_lens(
                        ctx,
                        code_lens.clone(),
                        self.shared
                            .initialization_options()
                            .code_lens_show_locations_command_name
                            .clone(),
                        Some(&orchestrator),
                    )
                    .map(Some)
            },
        )
    }

    // Go: server.go:2293 handlePrepareCallHierarchy
    pub fn handle_prepare_call_hierarchy(
        self: &Rc<Self>,
        ctx: &Context,
        language_service: &ls::LanguageService,
        params: &lsproto::CallHierarchyPrepareParams,
    ) -> Result<lsproto::CallHierarchyPrepareResponse, GoError> {
        language_service.provide_prepare_call_hierarchy(
            ctx,
            &params.text_document.uri,
            params.position,
        )
    }

    // Go: server.go:2301 handleCallHierarchyIncomingCalls
    pub fn handle_call_hierarchy_incoming_calls(
        self: &Rc<Self>,
        ctx: &Context,
        params: Option<&lsproto::CallHierarchyIncomingCallsParams>,
        req_msg: &Rc<lsproto::RequestMessage>,
    ) -> Result<lsproto::CallHierarchyIncomingCallsResponse, GoError> {
        let item = params
            .unwrap_or_else(|| crate::core::go_nil_dereference())
            .item
            .as_ref()
            .unwrap_or_else(|| crate::core::go_nil_dereference());
        let (default_ls, orchestrator) =
            self.get_language_service_and_cross_project_orchestrator(ctx, &item.uri, req_msg)?;
        default_ls.provide_call_hierarchy_incoming_calls(ctx, item, Some(&orchestrator))
    }

    // Go: server.go:2313 handleCallHierarchyOutgoingCalls
    pub fn handle_call_hierarchy_outgoing_calls(
        self: &Rc<Self>,
        ctx: &Context,
        params: Option<&lsproto::CallHierarchyOutgoingCallsParams>,
        _req: &Rc<lsproto::RequestMessage>,
    ) -> Result<lsproto::CallHierarchyOutgoingCallsResponse, GoError> {
        let item = params
            .unwrap_or_else(|| crate::core::go_nil_dereference())
            .item
            .as_ref()
            .unwrap_or_else(|| crate::core::go_nil_dereference());
        let language_service = self.session_ref().get_language_service(ctx, &item.uri)?;
        language_service.provide_call_hierarchy_outgoing_calls(ctx, item)
    }

    // Go: server.go:2325 handleSemanticTokensFull
    pub fn handle_semantic_tokens_full(
        self: &Rc<Self>,
        ctx: &Context,
        ls: &ls::LanguageService,
        params: &lsproto::SemanticTokensParams,
    ) -> Result<lsproto::SemanticTokensResponse, GoError> {
        ls.provide_semantic_tokens(ctx, &params.text_document.uri)
    }

    // Go: server.go:2329 handleSemanticTokensRange
    pub fn handle_semantic_tokens_range(
        self: &Rc<Self>,
        ctx: &Context,
        ls: &ls::LanguageService,
        params: &lsproto::SemanticTokensRangeParams,
    ) -> Result<lsproto::SemanticTokensRangeResponse, GoError> {
        ls.provide_semantic_tokens_range(ctx, &params.text_document.uri, params.range)
    }

    // Go: server.go:2333 handleInitializeAPISession
    // PORT: `apiSessionsMu` is dropped (dispatch thread).
    pub fn handle_initialize_api_session(
        self: &Rc<Self>,
        _ctx: &Context,
        params: Option<&lsproto::InitializeAPISessionParams>,
        _req: &Rc<lsproto::RequestMessage>,
    ) -> Result<lsproto::CustomInitializeAPISessionResponse, GoError> {
        if self.api_sessions.borrow().is_none() {
            *self.api_sessions.borrow_mut() = Some(FxHashMap::default());
        }

        // ts#64163
        let api_session = api::new_lsp_session(self.session_ref(), None);

        // Use provided pipe path or generate a unique one
        let params = params.unwrap_or_else(|| crate::core::go_nil_dereference());
        let pipe_path = match &params.pipe {
            Some(pipe) if !pipe.is_empty() => pipe.clone(),
            _ => self.generate_api_pipe_path(),
        };

        let transport = match ipc::new_pipe_transport(&pipe_path) {
            Ok(transport) => transport,
            Err(err) => {
                return Err(errors::errorf(
                    format!("failed to create API transport: {}", err.error()),
                    vec![err],
                ));
            }
        };

        // ts#64544: the session's state is stored before its goroutine starts.
        let (api_ctx, api_cancel) = context::with_cancel(&self.shared.background_ctx());
        let state = Rc::new(ApiSessionState {
            session: api_session.clone(),
            cancel: api_cancel,
            api_ctx,
            connection: RefCell::new(None),
            stopped: Cell::new(false),
            ended: Cell::new(false),
            conn_state: RefCell::new(None),
        });
        self.api_sessions
            .borrow_mut()
            .as_mut()
            .expect("created above")
            .insert(api_session.id(), state);

        // Start accepting connections in the background
        // PORT: `transport.Accept()` runs on its own thread. The connection
        // reads the project session, which lives on the dispatch thread, so
        // the accepted connection goes back to the dispatch loop
        // (`QueuedRequest::ApiAccepted`, `serve_api_connection`).
        {
            let shared = self.shared.clone();
            let session_id = api_session.id();
            crate::core::GoThread::new()
                .name("api-accept".to_string())
                .spawn(move || {
                    let accept_result = transport.accept();
                    let _ = transport.close();
                    let rwc = match accept_result {
                        Ok(rwc) => Some(rwc),
                        Err(accept_err) => {
                            shared.logger.errorf(&format!(
                                "API session {}: failed to accept connection: {}",
                                session_id,
                                accept_err.error()
                            ));
                            None
                        }
                    };
                    let ctx = shared.background_ctx();
                    let accepted = ApiAccepted { session_id, rwc };
                    let _ = shared.queue_request(&ctx, QueuedRequest::ApiAccepted(accepted));
                });
        }

        Ok(Some(lsproto::InitializeAPISessionResult {
            session_id: api_session.id(),
            pipe: pipe_path,
        }))
    }

    /// PORT: the rest of Go's API session goroutine after
    /// `transport.Accept()` (`handle_initialize_api_session`), on the
    /// dispatch thread. It returns when the connection ends. Meanwhile the
    /// connection runs the dispatch loop whenever it waits for a message
    /// (`ApiConnProtocol`), so LSP messages are served as in Go.
    /// ts#64544: a session that `close_api_sessions` stopped is gone from
    /// `api_sessions`, so a connection that its accept thread still gave is
    /// closed (Go `attachConnection` returns false).
    fn serve_api_connection(self: &Rc<Self>, accepted: ApiAccepted) {
        let state = self
            .api_sessions
            .borrow()
            .as_ref()
            .and_then(|api_sessions| api_sessions.get(&accepted.session_id).cloned());
        let Some(state) = state else {
            if let Some(rwc) = accepted.rwc {
                let _ = rwc.close();
            }
            return;
        };
        let api_session = state.session.clone();
        let lsp_panic = match accepted.rwc {
            Some(rwc) if state.attach_connection(rwc.clone()) => {
                let lsp_panic = self.run_api_connection(&state, rwc.clone());
                // Go: defer rwc.Close() (ts#64544)
                let _ = rwc.close();
                lsp_panic
            }
            _ => None,
        };
        // PORT: when the server ends while the connection waits, Go's
        // process exits and this defer never runs (the project session may
        // be closed by then), so the port skips it too.
        if !self.dispatch_ended() {
            // Go: defer { apiSession.Close(); s.removeAPISession(apiSession.ID()) }
            api_session.close();
            self.remove_api_session(&api_session.id());
        }
        // Go: defer apiCancel(); defer close(state.done)
        (state.cancel)();
        state.ended.set(true);
        if !self.dispatch_ended() {
            self.close_session_after_api_sessions();
        }
        if let Some(payload) = lsp_panic {
            std::panic::resume_unwind(payload);
        }
    }

    /// PORT: true when the dispatch loop ended (or never ran). Go's process
    /// exits then, before the API session goroutine logs or cleans up.
    fn dispatch_ended(&self) -> bool {
        match self.dispatch_ctx.borrow().as_ref() {
            Some((ctx, _)) => ctx.err().is_some(),
            None => true,
        }
    }

    /// PORT: Go's `conn.Run(apiCtx)` with its panic recovery. Returns the
    /// panic of an LSP message that the connection's wait served
    /// (`ApiConnProtocol`): it is not the API's, so the caller raises it
    /// again after the cleanup.
    fn run_api_connection(
        self: &Rc<Self>,
        session_state: &ApiSessionState,
        rwc: Arc<dyn ipc::ReadWriteCloser>,
    ) -> Option<Box<dyn Any + Send>> {
        // ts#64544: the context is the session state's.
        let api_session = &session_state.session;
        let api_ctx = &session_state.api_ctx;
        let api_cancel = &session_state.cancel;
        let state = Rc::new(ApiConnState::default());
        *session_state.conn_state.borrow_mut() = Some(state.clone());
        let lsp_panic = Rc::new(RefCell::new(None));

        // Run the connection with panic recovery
        let result = catch_unwind(AssertUnwindSafe(|| {
            let protocol = ApiConnProtocol {
                inner: ipc::new_jsonrpc_protocol(rwc.clone()),
                inbox: start_api_reader(&self.shared, rwc.clone()),
                server: Rc::downgrade(self),
                state: state.clone(),
                lsp_panic: lsp_panic.clone(),
            };
            let conn = ipc::new_async_conn_with_protocol(
                rwc.clone(),
                Box::new(protocol),
                api_session.clone(),
            );
            // ts#64299
            api_session.set_connection(Rc::new(ApiSessionConn {
                conn: conn.clone(),
                state: state.clone(),
            }));
            // PORT: when the dispatch loop ended while the connection
            // waited (stdin EOF while a call to the client waits), Go's
            // process exits before this goroutine logs the error.
            if let Err(api_err) = conn.run(api_ctx)
                && !self.dispatch_ended()
            {
                self.logger.errorf(&format!(
                    "API session {}: {}",
                    api_session.id(),
                    api_err.error()
                ));
            }
        }));
        if let Err(r) = result {
            let stack = std::backtrace::Backtrace::capture().to_string();
            self.logger.errorf(&format!(
                "API session {}: panic: {}\n{}",
                api_session.id(),
                panic_value_string(r.as_ref()),
                stack
            ));
            // Cancel the context to shut down the connection
            api_cancel();
            // Close the underlying connection
            let _ = rwc.close();
        }
        lsp_panic.take()
    }

    // Go: server.go:2410 generateAPIPipePath
    // PORT: Go `rand.Uint64()`; the port has no rand crate and takes 64
    // random bits from std's randomly keyed hasher.
    pub fn generate_api_pipe_path(&self) -> String {
        use std::hash::{BuildHasher, Hasher};
        // Generate a high-entropy path using time and random source
        let now = match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(d) => d.as_nanos() as i64,
            Err(e) => -(e.duration().as_nanos() as i64),
        };
        let rnd = std::collections::hash_map::RandomState::new()
            .build_hasher()
            .finish();
        ipc::generate_pipe_path(&format!("tsgo-api-{now:x}-{rnd:x}"))
    }

    // Go: server.go:2417 removeAPISession
    pub fn remove_api_session(&self, id: &str) {
        if let Some(api_sessions) = self.api_sessions.borrow_mut().as_mut() {
            api_sessions.remove(id);
        }
    }

    // Go: server.go:2423 closeAPISessions (ts#64544)
    // PORT: Go ranges over the map (random order); the port stops the
    // sessions in ID order.
    pub fn close_api_sessions(&self) {
        let mut api_sessions: Vec<(String, Rc<ApiSessionState>)> = self
            .api_sessions
            .borrow_mut()
            .as_mut()
            .map(|api_sessions| api_sessions.drain().collect())
            .unwrap_or_default();
        api_sessions.sort_by(|(a, _), (b, _)| a.cmp(b));

        for (_, state) in api_sessions {
            state.stop();
            if !state.ended.get() {
                self.stopping_api_sessions.borrow_mut().push(state);
            }
        }
    }

    /// PORT: the rest of Go's Shutdown after `closeAPISessions` waited for
    /// the API sessions: closes the project session when Shutdown left it
    /// open and no stopped API connection runs any more.
    fn close_session_after_api_sessions(&self) {
        if !self.close_session_after_api_sessions.get() {
            return;
        }
        self.stopping_api_sessions
            .borrow_mut()
            .retain(|state| !state.ended.get());
        if self.stopping_api_sessions.borrow().is_empty() {
            self.close_session_after_api_sessions.set(false);
            self.session_ref().close();
        }
    }

    // Go: server.go:2438 SetCompilerOptionsForInferredProjects
    // !!! temporary; remove when we have `handleDidChangeConfiguration`/implicit project config support
    pub fn set_compiler_options_for_inferred_projects(
        &self,
        ctx: &Context,
        options: Option<Rc<CompilerOptions>>,
    ) {
        *self.compiler_options_for_inferred_projects.borrow_mut() = options.clone();
        if let Some(session) = self.session() {
            session.did_change_compiler_options_for_inferred_projects(ctx, options);
        }
    }
}

/// PORT: the messages that an API connection's `api-reader` thread read.
/// Go's `conn.Run` goroutine reads them; here the reads stay off the
/// dispatch thread, so a client that sends nothing does not stop the LSP.
#[derive(Default)]
struct ApiInbox {
    messages: Mutex<VecDeque<Result<ipc::Message, GoError>>>,
}

/// PORT: starts the thread that reads the messages of an API connection
/// into an inbox, up to the first read error. After each message it queues
/// a `Wake`, so a dispatch loop that waits for the request queue looks at
/// the inbox again.
fn start_api_reader(
    shared: &Arc<ServerShared>,
    rwc: Arc<dyn ipc::ReadWriteCloser>,
) -> Arc<ApiInbox> {
    let inbox = Arc::new(ApiInbox::default());
    let shared = shared.clone();
    let thread_inbox = inbox.clone();
    // The Go stack size, as the LSP reader thread has.
    crate::core::GoThread::new()
        .name("api-reader".to_string())
        .stack_size(crate::gostd::stack::max_stack_size())
        .spawn(move || {
            let mut protocol = ipc::new_jsonrpc_protocol(rwc);
            loop {
                let read = protocol.read_message();
                let end = read.is_err();
                lock(&thread_inbox.messages).push_back(read);
                let ctx = shared.background_ctx();
                if shared.queue_request(&ctx, QueuedRequest::Wake).is_err() || end {
                    return;
                }
            }
        });
    inbox
}

/// PORT: the protocol of an API connection of the LSP server. Writes go to
/// the JSON-RPC protocol. A read takes the next message from the inbox.
/// While the inbox is empty and no call to the client waits, the read runs
/// the dispatch loop (`dispatch_next`), so LSP messages, exit, stdin
/// EOF, SIGTERM and the parent watchdog work while the connection waits,
/// as in Go, where the connection has its own goroutine. A read for a call
/// to the client (`AsyncConn::call` in a request handler) serves only the
/// LSP messages that do not need the session, in order, and `shutdown` and
/// `exit` (`ServerShared::wait_during_api_call`), because the handler is in
/// the middle of its work on the session. When the dispatch loop ends, the
/// read returns EOF, which ends the connection. A panic of an LSP message
/// served in a read ends the connection too, and `run_api_connection`
/// raises it again.
///
/// Limits of the one dispatch thread: LSP messages wait while an API
/// request runs (as they wait for a slow LSP request), and the other LSP
/// messages wait while a call to the client waits, until the client
/// answers (Go answers them). A connection that is accepted while another
/// waits runs inside the other's wait, so the first one's messages wait
/// until the second one ends; a client that waits for the first before it
/// closes the second deadlocks. A call to the client from an LSP message
/// served in the wait fails (`ApiSessionConn`).
struct ApiConnProtocol {
    inner: ipc::JSONRPCProtocol,
    inbox: Arc<ApiInbox>,
    server: Weak<Server>,
    state: Rc<ApiConnState>,
    /// The panic of an LSP message served in a read (`run_api_connection`).
    lsp_panic: Rc<RefCell<Option<Box<dyn Any + Send>>>>,
}

impl ipc::Protocol for ApiConnProtocol {
    fn read_message(&mut self) -> Result<ipc::Message, GoError> {
        let Some(server) = self.server.upgrade() else {
            return Err(errors::EOF.clone());
        };
        let Some((ctx, lsp_exit)) = server.dispatch_ctx.borrow().clone() else {
            return Err(errors::EOF.clone());
        };
        loop {
            // After an LSP message panicked in a read, the connection ends.
            if ctx.err().is_some() || self.lsp_panic.borrow().is_some() {
                return Err(errors::EOF.clone());
            }
            if let Some(msg) = lock(&self.inbox.messages).pop_front() {
                return msg;
            }
            self.state.serving_lsp.set(true);
            let served = catch_unwind(AssertUnwindSafe(|| {
                if self.state.calls.get() == 0 {
                    return server.dispatch_next(&ctx, &lsp_exit);
                }
                if let Some(req) = server.shared.wait_during_api_call(&ctx, &self.inbox) {
                    server.dispatch_request(&ctx, &lsp_exit, &Rc::new(req));
                }
                Ok(())
            }));
            self.state.serving_lsp.set(false);
            match served {
                Ok(Ok(())) => {}
                // The dispatch loop ended.
                Ok(Err(_)) => return Err(errors::EOF.clone()),
                Err(payload) => {
                    *self.lsp_panic.borrow_mut() = Some(payload);
                    return Err(errors::EOF.clone());
                }
            }
        }
    }

    fn write_request(
        &mut self,
        id: Option<&jsonrpc::ID>,
        method: &str,
        params: Option<Box<dyn AnyValue>>,
    ) -> Result<(), GoError> {
        self.inner.write_request(id, method, params)
    }

    fn write_notification(
        &mut self,
        method: &str,
        params: Option<Box<dyn AnyValue>>,
    ) -> Result<(), GoError> {
        self.inner.write_notification(method, params)
    }

    fn write_response(
        &mut self,
        id: Option<&jsonrpc::ID>,
        result: Option<Box<dyn AnyValue>>,
    ) -> Result<(), GoError> {
        self.inner.write_response(id, result)
    }

    fn write_error(
        &mut self,
        id: Option<&jsonrpc::ID>,
        err: &jsonrpc::ResponseError,
    ) -> Result<(), GoError> {
        self.inner.write_error(id, err)
    }
}

/// PORT: the stream check of `gostd::local::drop_after_pause`, which
/// `dispatch_loop` installs: whether the client already sent its next edit,
/// so a notification (the next didChange) waits in `queue`. Then the client
/// sends a stream, not one burst of requests. A request, a `Wake` and an
/// accepted API connection do not count. `$/cancelRequest` never waits
/// here: the read loop handles it. An API message is in the inbox of its
/// connection, not here (see `gostd::local::drop_after_pause`).
fn next_edit_waits(queue: &DynamicQueue<QueuedRequest>) -> bool {
    queue.with_items(|items| {
        items
            .iter()
            .any(|item| matches!(item, QueuedRequest::Request(req) if req.id.is_none()))
    })
}

/// PORT: what an API connection of the LSP server is doing, for its
/// protocol (`ApiConnProtocol`) and its session connection
/// (`ApiSessionConn`).
#[derive(Default)]
struct ApiConnState {
    /// The calls to the client that wait for their answer.
    calls: Cell<u32>,
    /// Whether a read runs the dispatch loop.
    serving_lsp: Cell<bool>,
}

/// PORT: the connection of an API session of the LSP server
/// (`apiSession.SetConnection(conn)`). It counts the calls to the client
/// for `ApiConnProtocol`. While a read of the connection runs the dispatch
/// loop, `AsyncConn` holds its protocol, so a call or notification to the
/// client from an LSP message (a callback module resolver of a project
/// that the API session opened, when an LSP request rebuilds it) returns
/// an error here instead of a panic. Go makes the call.
struct ApiSessionConn {
    conn: Rc<ipc::AsyncConn>,
    state: Rc<ApiConnState>,
}

impl ApiSessionConn {
    fn check_idle(&self) -> Result<(), GoError> {
        if self.state.serving_lsp.get() {
            return Err(errors::new(
                "ipc: the API connection cannot write while an LSP message runs in its read",
            ));
        }
        Ok(())
    }
}

/// Counts one call to the client until it is dropped (also on a panic).
struct PendingCall<'a>(&'a Cell<u32>);

impl Drop for PendingCall<'_> {
    fn drop(&mut self) {
        self.0.set(self.0.get() - 1);
    }
}

impl ipc::Conn for ApiSessionConn {
    fn run(&self, ctx: &Context) -> Result<(), GoError> {
        self.conn.run(ctx)
    }

    fn call(
        &self,
        ctx: &Context,
        method: &str,
        params: Option<Box<dyn AnyValue>>,
    ) -> Result<json_ext::JsonValue, GoError> {
        self.check_idle()?;
        self.state.calls.set(self.state.calls.get() + 1);
        let _pending = PendingCall(&self.state.calls);
        self.conn.call(ctx, method, params)
    }

    fn notify(
        &self,
        ctx: &Context,
        method: &str,
        params: Option<Box<dyn AnyValue>>,
    ) -> Result<(), GoError> {
        self.check_idle()?;
        self.conn.notify(ctx, method, params)
    }
}

impl ata::NpmExecutor for Server {
    // Go: server.go:2446 NpmInstall
    // NpmInstall implements ata.NpmExecutor
    fn npm_install(&self, ctx: &Context, cwd: &str, args: &[String]) -> (Vec<u8>, Option<GoError>) {
        (self
            .npm_install
            .as_ref()
            .unwrap_or_else(|| crate::core::go_nil_dereference()))(ctx, cwd, args)
    }

    // PORT: see `ata::NpmExecutor::npm_install_func`.
    fn npm_install_func(&self) -> Option<ata::NpmInstallFunc> {
        self.npm_install.clone()
    }
}

impl Server {
    // Go: server.go:2452 contentMapperSpawner (tsgo#4712)
    // contentMapperSpawner adapts the server's spawn callback to a content mapper spawner, or returns nil when
    // the server cannot spawn processes.
    pub fn content_mapper_spawner(&self) -> Option<Rc<dyn contentmapper::Spawner>> {
        let spawn = self.spawn.clone()?;
        Some(Rc::new(contentmapper::SpawnerFunc(Box::new(
            move |command: &[String], dir: &str, stderr: Option<Box<dyn Write + Send>>| {
                spawn(command, dir, stderr)
            },
        ))))
    }

    // Go: server.go:2459 contentMapperLogger (tsgo#4712)
    pub fn content_mapper_logger(&self) -> contentmapper::Logger {
        let logger = self.logger.clone();
        Arc::new(move |message: &str| {
            if logger.is_tracing() {
                logger.info(message);
            }
        })
    }
}

// Developer/debugging command handlers

impl Server {
    // Go: server.go:2469 handleRunGC
    pub fn handle_run_gc(
        self: &Rc<Self>,
        _ctx: &Context,
        _params: Option<&lsproto::NoParams>,
        _req: &Rc<lsproto::RequestMessage>,
    ) -> Result<lsproto::RunGCResponse, GoError> {
        crate::pprof::run_gc();
        self.logger.info("GC triggered");
        Ok(lsproto::Null)
    }

    // Go: server.go:2475 handleSaveHeapProfile
    pub fn handle_save_heap_profile(
        self: &Rc<Self>,
        _ctx: &Context,
        params: Option<&lsproto::ProfileParams>,
        _req: &Rc<lsproto::RequestMessage>,
    ) -> Result<lsproto::SaveHeapProfileResponse, GoError> {
        let file_path = crate::pprof::save_heap_profile(
            &params
                .unwrap_or_else(|| crate::core::go_nil_dereference())
                .dir,
        )?;
        self.logger
            .info(&format!("Heap profile saved to: {file_path}"));
        Ok(Some(lsproto::ProfileResult { file: file_path }))
    }

    // Go: server.go:2484 handleSaveAllocProfile
    pub fn handle_save_alloc_profile(
        self: &Rc<Self>,
        _ctx: &Context,
        params: Option<&lsproto::ProfileParams>,
        _req: &Rc<lsproto::RequestMessage>,
    ) -> Result<lsproto::SaveAllocProfileResponse, GoError> {
        let file_path = crate::pprof::save_alloc_profile(
            &params
                .unwrap_or_else(|| crate::core::go_nil_dereference())
                .dir,
        )?;
        self.logger
            .info(&format!("Allocation profile saved to: {file_path}"));
        Ok(Some(lsproto::ProfileResult { file: file_path }))
    }

    // Go: server.go:2493 handleStartCPUProfile
    pub fn handle_start_cpu_profile(
        self: &Rc<Self>,
        _ctx: &Context,
        params: Option<&lsproto::ProfileParams>,
        _req: &Rc<lsproto::RequestMessage>,
    ) -> Result<lsproto::StartCPUProfileResponse, GoError> {
        let params = params.unwrap_or_else(|| crate::core::go_nil_dereference());
        self.cpu_profiler.start_cpu_profile(&params.dir)?;
        self.logger.info(&format!(
            "CPU profiling started, will save to: {}",
            params.dir
        ));
        Ok(lsproto::Null)
    }

    // Go: server.go:2502 handleStopCPUProfile
    pub fn handle_stop_cpu_profile(
        self: &Rc<Self>,
        _ctx: &Context,
        _params: Option<&lsproto::NoParams>,
        _req: &Rc<lsproto::RequestMessage>,
    ) -> Result<lsproto::StopCPUProfileResponse, GoError> {
        let file_path = self.cpu_profiler.stop_cpu_profile()?;
        self.logger
            .info(&format!("CPU profile saved to: {file_path}"));
        Ok(Some(lsproto::ProfileResult { file: file_path }))
    }

    // Go: server.go:2511 handleProjectInfo
    pub fn handle_project_info(
        self: &Rc<Self>,
        ctx: &Context,
        params: Option<&lsproto::ProjectInfoParams>,
        _req: &Rc<lsproto::RequestMessage>,
    ) -> Result<lsproto::CustomProjectInfoResponse, GoError> {
        let uri = &params
            .unwrap_or_else(|| crate::core::go_nil_dereference())
            .text_document
            .uri;
        let (default_project, _, _) = self
            .session_ref()
            .get_language_service_and_projects_for_file(ctx, uri)?;
        let mut config_file_path = String::new();
        let default_project = default_project.borrow();
        if default_project.kind == project::Kind::CONFIGURED {
            config_file_path = default_project.config_file_name();
        }
        Ok(Some(lsproto::ProjectInfoResult { config_file_path }))
    }

    // Go: server.go:2526 handleSetContentMapperContributions (tsgo#4712)
    pub fn handle_set_content_mapper_contributions(
        self: &Rc<Self>,
        ctx: &Context,
        params: Option<&lsproto::SetContentMapperContributionsParams>,
        _req: &Rc<lsproto::RequestMessage>,
    ) -> Result<lsproto::CustomSetContentMapperContributionsResponse, GoError> {
        let params = params.unwrap_or_else(|| crate::core::go_nil_dereference());
        let contributions = parse_content_mapper_contributions(&params.contributions)?;
        let documents: Vec<lsproto::DocumentUri> = params
            .open_documents
            .iter()
            .map(|document| document.uri.clone())
            .collect();
        self.session_ref()
            .set_content_mapper_contributions(ctx, contributions, documents);
        Ok(lsproto::Null)
    }
}

// Go: server.go:2536 parseContentMapperContributions (tsgo#4712)
// PORT: Go `json.Marshal` of the options map (`LSPObject`) writes the keys in Go map
// order (random); `IndexMap` writes them in the order the client sent them.
pub fn parse_content_mapper_contributions(
    values: &[Option<lsproto::ContentMapperContribution>],
) -> Result<project::ContentMapperContributions, GoError> {
    let mut result = project::ContentMapperContributions::default();
    let mut claimed_extensions: FxHashSet<String> = FxHashSet::default();
    for (index, value) in values.iter().enumerate() {
        let Some(value) = value
            .as_ref()
            .filter(|value| !value.contributor_id.is_empty())
        else {
            return Err(errors::new(
                "content mapper contribution requires a contributorId",
            ));
        };
        let identity = format!("{}[{}]", value.contributor_id, index);
        let mut valid_extensions: Vec<String> = Vec::with_capacity(value.extensions.len());
        for extension in &value.extensions {
            if !is_valid_contributed_content_mapper_extension(extension) {
                return Err(errors::new(format!(
                    "content mapper contribution {} has invalid extension {}",
                    gostd::strconv::quote(&identity),
                    gostd::strconv::quote(extension)
                )));
            }
            valid_extensions.push(extension.clone());
        }
        let Some(inferred_project) = &value.inferred_project_contribution else {
            continue;
        };
        let manifest = inferred_project
            .manifest
            .as_ref()
            .unwrap_or_else(|| crate::core::go_nil_dereference());
        if manifest.name.is_empty() || manifest.exec.is_empty() {
            return Err(errors::new(format!(
                "content mapper contribution {} requires a manifest name and exec",
                gostd::strconv::quote(&identity)
            )));
        }
        for option in manifest.compiler_options.as_deref().unwrap_or_default() {
            if tsoptions::COMMAND_LINE_COMPILER_OPTIONS_MAP
                .get(option)
                .is_none()
            {
                return Err(errors::new(format!(
                    "content mapper contribution {} requests unknown compiler option {}",
                    gostd::strconv::quote(&identity),
                    gostd::strconv::quote(option)
                )));
            }
        }
        for extension in &valid_extensions {
            // ts#63936: Go `strings.ToLower` (rune by rune).
            let lowered: String = extension
                .chars()
                .map(crate::gostd::unicode::to_lower)
                .collect();
            if !claimed_extensions.insert(lowered) {
                return Err(errors::new(format!(
                    "content mapper contributions both claim extension {}",
                    gostd::strconv::quote(extension)
                )));
            }
            result.extensions.push(extension.clone());
        }
        let mut options = b"{}".to_vec();
        if let Some(inferred_options) = &inferred_project.options {
            match crate::frontend::json::json_marshal(inferred_options, &[]) {
                Ok(marshaled) => options = marshaled.into_bytes(),
                Err(_) => {
                    return Err(errors::new(format!(
                        "content mapper contribution {} has invalid options",
                        gostd::strconv::quote(&identity)
                    )));
                }
            }
        }
        let mut mapper = contentmapper::Mapper {
            definition: contentmapper::Definition {
                package: identity.clone(),
                extensions: valid_extensions,
                options: json_ext::JsonValue(options),
            },
            manifest: contentmapper::Manifest {
                name: manifest.name.clone(),
                version: manifest.version.clone().unwrap_or_default(),
                exec: manifest.exec.clone(),
                compiler_options: manifest.compiler_options.clone().unwrap_or_default(),
                dynamic_config: manifest.dynamic_config.unwrap_or_default(),
            },
            contribution_id: identity.clone(),
            ..Default::default()
        };
        if let Some(cwd) = &manifest.cwd {
            if !tspath::path_is_absolute(cwd) {
                return Err(errors::new(format!(
                    "content mapper contribution {} has non-absolute cwd",
                    gostd::strconv::quote(&identity)
                )));
            }
            // ts#64159: the directory is rooted and normalized
            // (RootedDirectoryPathFromAbsolute, server.go:2591).
            mapper.package_directory = lsproto::rooted_path_from_absolute(cwd);
        }
        result.mappers.push(Rc::new(mapper));
    }
    result.extensions.sort();
    Ok(result)
}

// Go: server.go:2599 isValidContributedContentMapperExtension (tsgo#4712)
pub fn is_valid_contributed_content_mapper_extension(extension: &str) -> bool {
    if extension.len() <= 1
        || !extension.starts_with('.')
        || tspath::get_any_extension_from_path(&format!("file{extension}"), &[], false) != extension
    {
        return false;
    }
    // ts#63936: Go `strings.EqualFold`.
    !tspath::ALL_SUPPORTED_EXTENSIONS_WITH_JSON
        .iter()
        .flat_map(|group| group.iter())
        .any(|native_extension| {
            crate::frontend::stringutil_ls::equate_string_case_insensitive(
                native_extension,
                extension,
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A queued LSP message: a request with `id`, or a notification.
    fn message(method: lsproto::Method, id: Option<i32>) -> QueuedRequest {
        QueuedRequest::Request(lsproto::RequestMessage {
            id: id.map(crate::jsonrpc::new_id_int),
            method,
            ..lsproto::RequestMessage::default()
        })
    }

    // followups21 (R168 reviewer): the stream check of `drop_after_pause`
    // (freecheck2) sees only a waiting notification. A waiting request or
    // wake-up keeps the release after a pause; the next didChange drops it
    // at once.
    #[test]
    fn stream_check_sees_only_a_waiting_notification() {
        let ctx = context::background();
        let queue: DynamicQueue<QueuedRequest> = new_dynamic_queue();
        assert!(!next_edit_waits(&queue), "empty queue");
        queue.put(&ctx, QueuedRequest::Wake).unwrap();
        queue
            .put(&ctx, message(lsproto::Method::TEXT_DOCUMENT_HOVER, Some(1)))
            .unwrap();
        assert!(!next_edit_waits(&queue), "a request and a wake-up");
        queue
            .put(
                &ctx,
                message(lsproto::Method::TEXT_DOCUMENT_DID_CHANGE, None),
            )
            .unwrap();
        assert!(next_edit_waits(&queue), "the next didChange waits");
    }

    /// Logs its name when it is dropped.
    struct Freed(&'static str, Rc<RefCell<Vec<&'static str>>>);

    impl Drop for Freed {
        fn drop(&mut self) {
            self.1.borrow_mut().push(self.0);
        }
    }

    // PORT: no Go counterpart (lswarm1). An idle job that starts at once
    // (the eager auto-import warm) runs before the frees that the last
    // message left, and a job that waits for a quiet period runs after
    // them. On the bitecs T1 repro the frees took 0.2 to 0.35 ms, and the
    // next didChange came 0.3 to 0.4 ms after the answer.
    #[test]
    fn an_eager_idle_job_runs_before_the_frees() {
        std::thread::spawn(|| {
            let ctx = context::background();
            gostd::local::keep_garbage();
            for (start, want) in [
                (gostd::local::IdleStart::AtOnce, ["warm", "free"]),
                (gostd::local::IdleStart::AfterQuiet, ["free", "warm"]),
            ] {
                let log = Rc::new(RefCell::new(Vec::new()));
                gostd::local::drop_later(Box::new(Freed("free", log.clone())));
                let job_log = log.clone();
                gostd::local::go_idle(start, Box::new(move || job_log.borrow_mut().push("warm")));
                let mut quiet = Vec::new();
                run_idle_work(
                    &ctx,
                    || false,
                    |period| {
                        quiet.push(period);
                        true
                    },
                );
                assert_eq!(*log.borrow(), want, "{start:?}");
                let period = match start {
                    gostd::local::IdleStart::AtOnce => Duration::ZERO,
                    gostd::local::IdleStart::AfterQuiet => IDLE_QUIET_PERIOD,
                };
                assert_eq!(quiet, [period], "{start:?}");
            }
        })
        .join()
        .expect("idle test thread");
    }
}
