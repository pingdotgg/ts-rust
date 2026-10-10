//! Port of Go `internal/testutil/lsptestutil/lspclient.go`.
//!
//! PORT: the Rust `lsp::Server` is `Rc` and runs its dispatch loop on the
//! thread that calls `run`, so the server is made and run on a thread of
//! its own ("lsp-server"). The pipes are channels of `lsproto::Message`.
//! Go (since tsgo#4471) connects client and server with JSON byte pipes, so
//! every message makes a full marshal/unmarshal round trip. Here each
//! message in either direction is marshaled to JSON and unmarshaled again
//! (`json_round_trip`), so the receiver gets raw params and results, as in
//! Go.
//!
//! Go `<-client.Server.InitComplete()` has no port: the Rust server runs
//! the `initialized` handler to its end before it reads the next message.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use ts_goport::contentmapper;
use ts_goport::frontend::bundled;
use ts_goport::frontend::json::UnmarshalerFrom;
use ts_goport::frontend::json_ext::AnyValue;
use ts_goport::gostd::{Context, GoError, context, errors};
use ts_goport::jsonrpc::{self, ID, MessageKind, ResponseError};
use ts_goport::lsp::{self, lsproto};
use ts_goport::project::ata;

use super::projecttestutil::FileMap;
use crate::support::vfstest::MapFs;

// Go: lspclient.go:21 LSPReader
// LSPReader reads LSP messages from a channel.
struct LspReader {
    c: Receiver<Option<lsproto::Message>>,
}

impl lsp::Reader for LspReader {
    fn read(&mut self) -> (Option<lsproto::Message>, Option<GoError>) {
        match self.c.recv() {
            Ok(Some(msg)) => (Some(msg), None),
            _ => (None, Some(errors::EOF.clone())),
        }
    }
}

/// The message after a trip through its JSON form, as the Go byte pipes
/// give it (see the module comment).
fn json_round_trip(msg: &lsproto::Message) -> lsproto::Message {
    let data = msg
        .marshal_json()
        .unwrap_or_else(|err| panic!("failed to encode message as JSON: {}", err.error()));
    let mut copy = lsproto::Message::default();
    copy.unmarshal_json(&data)
        .unwrap_or_else(|err| panic!("failed to decode message JSON: {}", err.error()));
    copy
}

// Go: lspclient.go:34 LSPWriter
// LSPWriter writes LSP messages to a channel.
// PORT: the message is copied through its JSON form (see the module comment).
struct LspWriter {
    c: SyncSender<Option<lsproto::Message>>,
}

impl lsp::Writer for LspWriter {
    fn write(&mut self, msg: &lsproto::Message) -> Result<(), GoError> {
        // The client may be gone at shutdown; drop the message then.
        let _ = self.c.send(Some(json_round_trip(msg)));
        Ok(())
    }
}

// Go: lspclient.go:60 ServerRequestHandler
// ServerRequestHandler handles server-initiated requests and returns the response to send back.
pub type ServerRequestHandler =
    Arc<dyn Fn(&lsproto::RequestMessage) -> Option<lsproto::ResponseMessage> + Send + Sync>;

// Go: lspclient.go:63 ServerNotificationHandler
// ServerNotificationHandler handles server-initiated notifications (e.g., $/progress).
pub type ServerNotificationHandler = Arc<dyn Fn(&lsproto::RequestMessage) + Send + Sync>;

type Pending = Arc<Mutex<HashMap<ID, SyncSender<lsproto::ResponseMessage>>>>;

/// The server options of a Go `lsp.ServerOptions{...}` literal that the
/// tests set (In, Out and Err are the harness's).
pub struct ServerSetup {
    pub cwd: String,
    /// The map file system; the server gets `bundled.WrapFS` of it. `None`
    /// is the OS file system (Go `bundled.WrapFS(osvfs.FS())`), with the
    /// global typings location and an `npm` runner (`os_server_setup`).
    pub files: Option<MapFs>,
    pub default_library_path: String,
    /// Go `ServerOptions.Spawn` (tsgo#4712): `Some(contentmappertest::new_spawner)`
    /// gives the server `contentmappertest.NewSpawner().Spawn`.
    /// PORT: a spawner is `Rc`, so the server thread makes it.
    pub spawner: Option<fn() -> Rc<dyn contentmapper::Spawner>>,
}

// Go: lspclient.go:66 LSPClient
// LSPClient provides infrastructure for communicating with an LSP server in tests.
pub struct LspClient {
    input_writer: SyncSender<Option<lsproto::Message>>,
    id: std::cell::Cell<i32>,
    pending_requests: Pending,
    cancel: Option<context::CancelFunc>,
    server: Option<JoinHandle<Result<(), GoError>>>,
    router: Option<JoinHandle<()>>,
}

/// Go `lsp.ServerOptions{Err: io.Discard, Cwd: cwd, FS: bundled.WrapFS(vfstest.FromMap(files, false)), DefaultLibraryPath: bundled.LibPath()}`.
/// It also points the OS override at the map (a child process only).
pub fn server_setup(cwd: &str, files: FileMap) -> ServerSetup {
    let (map, _) = super::projecttestutil::wrapped_map_fs(files, false);
    ServerSetup {
        cwd: cwd.to_string(),
        files: Some(map),
        default_library_path: bundled::lib_path(),
        spawner: None,
    }
}

/// Go `lsp.ServerOptions{Cwd: cwd, FS: bundled.WrapFS(osvfs.FS()), DefaultLibraryPath:
/// bundled.LibPath(), TypingsLocation: osvfs.GetGlobalTypingsCacheLocation(), NpmInstall: ...}`
/// of lsp `TestReplay`. PORT: Go `Err` is `os.Stderr`; the harness drops
/// server errors, as in `server_setup`.
pub fn os_server_setup(cwd: &str) -> ServerSetup {
    ServerSetup {
        cwd: cwd.to_string(),
        files: None,
        default_library_path: bundled::lib_path(),
        spawner: None,
    }
}

// Go: lspclient.go:86 NewLSPClient
// NewLSPClient creates an LSPClient wrapping the given server and pipes.
// PORT: Go returns the close function; here it is `LspClient::close` (also
// run on drop). The notification handler is given here (Go sets the
// `OnServerNotification` field).
pub fn new_lsp_client(
    setup: ServerSetup,
    on_server_request: Option<ServerRequestHandler>,
    on_server_notification: Option<ServerNotificationHandler>,
) -> LspClient {
    let (input_writer, input_reader) = sync_channel::<Option<lsproto::Message>>(100);
    let (output_writer, output_reader) = sync_channel::<Option<lsproto::Message>>(100);
    let (ctx, cancel) = context::with_cancel(&context::background());

    // Start server thread
    let server_ctx = ctx.clone();
    let server = std::thread::Builder::new()
        .name("lsp-server".to_string())
        .stack_size(256 * 1024 * 1024)
        .spawn(move || {
            let (fs, typings_location, npm_install) = match &setup.files {
                Some(files) => (bundled::wrap_fs(files.fs()), String::new(), None),
                None => (
                    bundled::wrap_fs(ts_goport::frontend::vfs::osvfs::osvfs_fs()),
                    ts_goport::cmd::tsgo::lsp::get_global_typings_cache_location(),
                    Some(Arc::new(ts_goport::cmd::tsgo::lsp::npm_install) as ata::NpmInstallFunc),
                ),
            };
            let server = lsp::new_server(lsp::ServerOptions {
                in_: Box::new(LspReader { c: input_reader }),
                out: Box::new(LspWriter {
                    c: output_writer.clone(),
                }),
                err: Box::new(std::io::sink()),
                cwd: setup.cwd,
                fs,
                default_library_path: setup.default_library_path,
                typings_location,
                parse_cache: None,
                npm_install,
                spawn: setup.spawner.map(spawn_fn),
                progress_delay: Duration::ZERO,
                set_parent_process_id: None,
            });
            let result = server.run(&server_ctx);
            // Go: defer outputWriter.Close()
            let _ = output_writer.send(None);
            result
        })
        .expect("start the lsp server thread");

    let pending_requests: Pending = Arc::default();

    // Start async message router
    let router_pending = pending_requests.clone();
    let router_input = input_writer.clone();
    let router = std::thread::Builder::new()
        .name("lsp-router".to_string())
        .spawn(move || {
            message_router(
                &ctx,
                &output_reader,
                &router_pending,
                &router_input,
                on_server_request.as_ref(),
                on_server_notification.as_ref(),
            );
        })
        .expect("start the lsp router thread");

    LspClient {
        input_writer,
        id: std::cell::Cell::new(0),
        pending_requests,
        cancel: Some(cancel),
        server: Some(server),
        router: Some(router),
    }
}

/// Go `spawner.Spawn` as a `ServerOptions.Spawn` function.
fn spawn_fn(new_spawner: fn() -> Rc<dyn contentmapper::Spawner>) -> Rc<contentmapper::SpawnFn> {
    let spawner = new_spawner();
    Rc::new(
        move |command: &[String], dir: &str, stderr: Option<Box<dyn std::io::Write + Send>>| {
            contentmapper::Spawner::spawn(&*spawner, command, dir, stderr)
        },
    )
}

// Go: lspclient.go:137 MessageRouter
// MessageRouter runs in a goroutine and routes incoming messages from the server.
fn message_router(
    ctx: &Context,
    output_reader: &Receiver<Option<lsproto::Message>>,
    pending: &Pending,
    input_writer: &SyncSender<Option<lsproto::Message>>,
    on_server_request: Option<&ServerRequestHandler>,
    on_server_notification: Option<&ServerNotificationHandler>,
) {
    loop {
        let Ok(Some(msg)) = output_reader.recv() else {
            return;
        };

        // After context cancellation, keep draining but don't process messages.
        if ctx.err().is_some() {
            continue;
        }

        if msg.kind == MessageKind::RESPONSE {
            handle_response(pending, msg.into_response());
        } else if msg.kind == MessageKind::REQUEST {
            handle_server_request(input_writer, on_server_request, msg.as_request());
        } else if msg.kind == MessageKind::NOTIFICATION {
            if let Some(handler) = on_server_notification {
                handler(msg.as_request());
            }
        }
    }
}

// Go: lspclient.go:180 handleResponse
// handleResponse routes a response message to the waiting request goroutine.
fn handle_response(pending: &Pending, resp: lsproto::ResponseMessage) {
    let Some(id) = resp.id.clone() else {
        return;
    };
    let resp_chan = pending
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(&id);
    if let Some(resp_chan) = resp_chan {
        let _ = resp_chan.send(resp);
    }
}

// Go: lspclient.go:203 handleServerRequest
// handleServerRequest handles requests initiated by the server (e.g., workspace/configuration).
fn handle_server_request(
    input_writer: &SyncSender<Option<lsproto::Message>>,
    on_server_request: Option<&ServerRequestHandler>,
    req: &lsproto::RequestMessage,
) {
    let mut response = on_server_request.and_then(|handler| handler(req));

    if response.is_none() {
        // Default: unknown server request
        response = Some(lsproto::ResponseMessage {
            id: req.id.clone(),
            jsonrpc: req.jsonrpc,
            result: None,
            error: Some(ResponseError {
                code: lsproto::ErrorCode::METHOD_NOT_FOUND.0,
                message: format!("Unknown method: {}", req.method.0),
                data: None,
            }),
        });
    }

    // Send response back to server
    let _ = input_writer.send(Some(json_round_trip(
        &response.expect("response").message(),
    )));
}

impl LspClient {
    // Go: lspclient.go:127 NextID
    // NextID returns the next request ID.
    pub fn next_id(&self) -> i32 {
        let id = self.id.get();
        self.id.set(id + 1);
        id
    }

    // Go: lspclient.go:238 WriteMsg
    // WriteMsg sends a message to the server.
    pub fn write_msg(&self, msg: lsproto::Message) {
        self.input_writer
            .send(Some(json_round_trip(&msg)))
            .unwrap_or_else(|err| panic!("failed to write message: {err}"));
    }

    // Go: lspclient.go:246 SendRequest
    // SendRequest sends a typed request and waits for the response.
    // PORT: returns the response message and the typed result (`None` for
    // Go's `ok == false`).
    pub fn send_request<P: AnyValue, R: UnmarshalerFrom + Default + 'static>(
        &self,
        info: &lsproto::RequestInfo<P, R>,
        params: P,
    ) -> (lsproto::ResponseMessage, Option<R>) {
        self.send_request_async(info, params)()
    }

    // Go: lspclient.go:260 SendRequestAsync
    // SendRequestAsync sends a typed request and returns a waiter for its response.
    pub fn send_request_async<P: AnyValue, R: UnmarshalerFrom + Default + 'static>(
        &self,
        info: &lsproto::RequestInfo<P, R>,
        params: P,
    ) -> impl FnOnce() -> (lsproto::ResponseMessage, Option<R>) + use<P, R> {
        let id = self.next_id();
        let req_id = jsonrpc::new_id_int(id);
        let req = info.new_request_message(Some(req_id.clone()), params);

        let (tx, rx) = sync_channel::<lsproto::ResponseMessage>(1);
        self.pending_requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(req_id, tx);
        self.write_msg(req.message());

        let info = info.clone();
        move || {
            let mut resp = rx
                .recv_timeout(Duration::from_secs(120))
                .unwrap_or_else(|err| panic!("Request cancelled: {err}"));
            let result = resp.result.take();
            let typed = if resp.error.is_none() {
                info.unmarshal_result(result).ok()
            } else {
                None
            };
            (resp, typed)
        }
    }

    // Go: lspclient.go:282 SendRequestWorker
    // SendRequestWorker sends a request message with the given ID and waits
    // for its response. PORT: `None` is Go's `ok == false` (no response in
    // the harness timeout).
    pub fn send_request_worker(
        &self,
        req: lsproto::RequestMessage,
        req_id: ID,
    ) -> Option<lsproto::ResponseMessage> {
        self.send_request_message(req, req_id)
            .recv_timeout(Duration::from_secs(120))
            .ok()
    }

    /// `send_request_worker` without the wait (no Go counterpart): returns
    /// the channel of the response, so a test can check later whether the
    /// server answered.
    pub fn send_request_message(
        &self,
        req: lsproto::RequestMessage,
        req_id: ID,
    ) -> Receiver<lsproto::ResponseMessage> {
        let (tx, rx) = sync_channel::<lsproto::ResponseMessage>(1);
        self.pending_requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(req_id, tx);
        self.write_msg(req.message());
        rx
    }

    // Go: lspclient.go:312 SendNotification
    // SendNotification sends a typed notification.
    pub fn send_notification<P: AnyValue>(&self, info: &lsproto::NotificationInfo<P>, params: P) {
        let notification = info.new_notification_message(params);
        self.write_msg(notification.message());
    }

    /// Waits up to `timeout` for the server's `run` to return by itself,
    /// as after an LSP `exit`, and for the router to pass on all that the
    /// server wrote. True when both ended.
    pub fn wait_server_end(&self, timeout: Duration) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if self.server.as_ref().is_none_or(JoinHandle::is_finished)
                && self.router.as_ref().is_none_or(JoinHandle::is_finished)
            {
                return true;
            }
            if std::time::Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Go `closeClient()`: cancel, close the input and wait for the server.
    /// A context error is not an error (Go `errors.Is(err, context.Canceled)`).
    pub fn close(&mut self) -> Result<(), GoError> {
        let Some(cancel) = self.cancel.take() else {
            return Ok(());
        };
        cancel();
        let _ = self.input_writer.send(None);
        let mut result = Ok(());
        if let Some(server) = self.server.take() {
            match server.join() {
                Ok(Err(err))
                    if !errors::is(&err, &context::CANCELED) && !errors::is(&err, &errors::EOF) =>
                {
                    result = Err(err);
                }
                Ok(_) => {}
                Err(payload) => std::panic::resume_unwind(payload),
            }
        }
        if let Some(router) = self.router.take() {
            let _ = router.join();
        }
        result
    }
}

impl Drop for LspClient {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            let _ = self.close();
        }
    }
}

/// Go `&lsproto.ResponseMessage{ID: req.ID, JSONRPC: req.JSONRPC, Result: result}`.
pub fn result_response(
    req: &lsproto::RequestMessage,
    result: Box<dyn AnyValue>,
) -> lsproto::ResponseMessage {
    lsproto::ResponseMessage {
        id: req.id.clone(),
        jsonrpc: req.jsonrpc,
        result: Some(result),
        error: None,
    }
}
