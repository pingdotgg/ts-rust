//! API sessions on the session thread: `tsgo --api` (`ts_goport::cmd::tsgo::api`
//! `run_api` and `api::server` `StdioServer::run`) over two in-memory pipes
//! per session instead of stdin and stdout.
//!
//! All sessions of the process run on one thread. `ts_goport` loads programs
//! on one thread at a time (`ast/store.rs` `BuildStores::next_id`: "another
//! thread published node stores while this thread built stores"), as in
//! tsgo, where one process has one session. The JS thread drives the
//! sessions one request at a time, so they need no more: a session's
//! connection reads its frames until its input is empty between two
//! messages, then gives the thread back (`MemoryConn::read`), and the next
//! request runs it again (`Command::Work`).
//!
//! A callback of one session may make a request to another session. The
//! thread waits in the first session's callback then, so the JS thread wakes
//! that wait (`wake_waiting`) and the thread runs the other session inside
//! it (`SessionThread::run_queued`), as it runs a nested request of the same
//! session (ts#64299).
//!
//! Everything `Rc` of a session lives on that thread. The JS thread holds
//! only the pipes.

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::io::{Error, ErrorKind};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use ts_goport::api::{
    CallbackFS, Session, SessionOptions, new_callback_fs, new_message_pack_protocol,
    new_standalone_session,
};
use ts_goport::contentmapper::{ProcessExitState, Spawner};
use ts_goport::execute::incremental::emit_files::ClientWrites;
use ts_goport::execute::tsc::{self, OsSystem, System};
use ts_goport::frontend::bundled;
use ts_goport::frontend::json_ext::{AnyValue, JsonValue};
use ts_goport::frontend::tspath::to_rooted_path;
use ts_goport::frontend::vfs::{self, Fs};
use ts_goport::gostd::{Context, GoError, context, errors};
use ts_goport::ipc::{Conn, Handler, ReadWriteCloser, SyncConn, new_sync_conn, recovered_value};
use ts_goport::lsp::lsproto;
use ts_goport::program::ThreadBudget;
use ts_goport::project;

use crate::pipe::Pipe;

/// The `tsgo --api` flags that a session takes (`cmd/tsgo/api.rs`
/// `parse_api_flags`). `--async` and `--pipe` are refused: the channel is
/// the sync protocol over memory.
#[derive(Debug, Default)]
pub struct SessionConfig {
    cwd: Option<String>,
    callbacks: Vec<String>,
    case_sensitive: Option<bool>,
    timing: bool,
    run_external_code: bool,
}

/// Parses the arguments as Go's `flag` package does for `tsgo --api`: one
/// or two dashes, `-name=value`, `-name value` for a string flag and
/// `-name` or `-name=<bool>` for a boolean one. A leading `--api` is the
/// mode, as on the tsgo command line.
pub fn parse_args(args: &[String]) -> Result<SessionConfig, String> {
    let mut config = SessionConfig::default();
    let mut rest = args.iter();
    let mut first = true;
    while let Some(arg) = rest.next() {
        if std::mem::take(&mut first) && arg == "--api" {
            continue;
        }
        if arg == "--" {
            break;
        }
        let Some(flag) = arg.strip_prefix("--").or_else(|| arg.strip_prefix('-')) else {
            return Err(format!("unexpected argument: {arg}"));
        };
        let (name, inline) = match flag.split_once('=') {
            Some((name, value)) => (name, Some(value.to_string())),
            None => (flag, None),
        };
        let mut string_value = || {
            inline
                .clone()
                .or_else(|| rest.next().cloned())
                .ok_or_else(|| format!("flag needs an argument: -{name}"))
        };
        let bool_value = || {
            inline
                .as_deref()
                .map_or(Ok(true), |value| parse_bool(name, value))
        };
        match name {
            "cwd" => config.cwd = Some(string_value()?),
            "callbacks" => {
                let value = string_value()?;
                config.callbacks = if value.is_empty() {
                    Vec::new()
                } else {
                    value.split(',').map(str::to_string).collect()
                };
            }
            "useCaseSensitiveFileNames" => config.case_sensitive = Some(bool_value()?),
            "timing" => config.timing = bool_value()?,
            "runExternalCode" => config.run_external_code = bool_value()?,
            "async" | "pipe" => {
                return Err(format!(
                    "-{name} is not supported by the in-process channel (it speaks the sync protocol over memory)"
                ));
            }
            _ => return Err(format!("flag provided but not defined: -{name}")),
        }
    }
    Ok(config)
}

/// Go `strconv.ParseBool`.
fn parse_bool(name: &str, value: &str) -> Result<bool, String> {
    match value {
        "1" | "t" | "T" | "true" | "TRUE" | "True" => Ok(true),
        "0" | "f" | "F" | "false" | "FALSE" | "False" => Ok(false),
        _ => Err(format!(
            "invalid boolean value {value:?} for -{name}: parse error"
        )),
    }
}

/// The frames of one direction of a session.
pub type FramePipe = Pipe<Vec<u8>>;

/// The work of the session thread.
enum Command {
    /// Opens session `id` with its pipes.
    Start {
        id: u64,
        config: SessionConfig,
        input: Arc<FramePipe>,
        output: Arc<FramePipe>,
    },
    /// Session `id` has frames to read (or its input closed).
    Work(u64),
}

/// The command queue of the session thread, once it runs.
static COMMANDS: Mutex<Option<Arc<Pipe<Command>>>> = Mutex::new(None);

/// A session thread with no session retires when its synthetic nodes
/// (`ast::synthetic`) pass this count. `ts_goport` never frees the nodes that
/// some requests make (`printNode` decodes its AST into synthetic nodes),
/// and every later program copies them into each of its checker threads
/// (`synthetic_seed`): after `printNode` of TypeScript's own compiler
/// sources (about 2 million nodes) each new program took 2 s more. A
/// session thread holds about 15,000 after ordinary sessions. A new thread
/// costs a parse of the lib files that `ts_goport` keeps (the first parse of
/// a path on a thread), so the thread is not replaced after every session.
const RETIRE_SYNTHETIC_NODES: usize = 250_000;

/// The JS thread's handle of one session.
pub struct Handle {
    id: u64,
}

impl Handle {
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Tells the session thread that the session has frames.
    pub fn schedule(&self) -> std::io::Result<()> {
        submit(Command::Work(self.id))
    }

    /// Tells the session thread that the session's input closed. With no
    /// session thread there is no session to end, and none starts for it.
    pub fn closed(&self) {
        let commands = lock(&COMMANDS).clone();
        if let Some(commands) = commands {
            let _ = commands.send(Command::Work(self.id));
        }
    }
}

/// Opens a session on the session thread (started on first use). It reads
/// frames from `input` and writes frames to `output`, and closes both pipes
/// with the reason when it ends.
pub fn start(
    config: SessionConfig,
    input: Arc<FramePipe>,
    output: Arc<FramePipe>,
) -> std::io::Result<Handle> {
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    submit(Command::Start {
        id,
        config,
        input,
        output,
    })?;
    Ok(Handle { id })
}

/// Queues `command` on the current session thread. A thread that retires
/// closes its queue, so the command goes to the next one.
fn submit(mut command: Command) -> std::io::Result<()> {
    loop {
        match session_thread()?.send(command) {
            Ok(()) => return Ok(()),
            Err(returned) => command = returned,
        }
    }
}

fn session_thread() -> std::io::Result<Arc<Pipe<Command>>> {
    let mut slot = lock(&COMMANDS);
    if let Some(commands) = &*slot {
        return Ok(commands.clone());
    }
    let commands = Arc::new(Pipe::new());
    let queue = commands.clone();
    std::thread::Builder::new()
        .name("tsc-rs-api".to_string())
        .stack_size(ts_goport::gostd::stack::max_stack_size())
        .spawn(move || run_sessions(&queue))?;
    *slot = Some(commands.clone());
    Ok(commands)
}

/// Tells the session thread, which waits in a callback of the session that
/// reads `input`, to run the commands queued meanwhile: the callback made a
/// request to another session. An empty chunk is the signal; the JS thread
/// sends only frames, which are never empty.
pub fn wake_waiting(input: &FramePipe) {
    let _ = input.send(Vec::new());
}

/// The state of the session thread.
struct SessionThread {
    commands: Arc<Pipe<Command>>,
    ctx: Context,
    sessions: RefCell<HashMap<u64, Hosted>>,
    /// The sessions whose connection runs now, the inner one last. Each
    /// reads its own input; a `Work` for one of them is old.
    running: RefCell<Vec<u64>>,
}

thread_local! {
    /// The session thread's state, on the session thread.
    static CURRENT: RefCell<Option<Rc<SessionThread>>> = const { RefCell::new(None) };
    /// The messages that run now on this thread, in all its sessions. The
    /// frees wait until the outer one ends (`FreeAfterMessage`).
    static MESSAGES: Cell<u32> = const { Cell::new(0) };
}

/// The session thread: opens sessions and runs their messages, until it
/// retires.
fn run_sessions(commands: &Arc<Pipe<Command>>) {
    // Go: tsgo runs its API server under the one-program budget
    // (`bin/tsgo.rs` main), with the frees of `StdioServer::run`.
    ThreadBudget::one_program(1).install();
    ts_goport::gostd::local::keep_garbage();
    ts_goport::ast::free_released_versions_in_background();
    let thread = Rc::new(SessionThread {
        commands: commands.clone(),
        ctx: context::background(),
        sessions: RefCell::new(HashMap::new()),
        running: RefCell::new(Vec::new()),
    });
    CURRENT.with(|current| *current.borrow_mut() = Some(thread.clone()));
    while let Ok(command) = commands.pop() {
        if thread.run_command(command)
            && thread.sessions.borrow().is_empty()
            && ts_goport::ast::synthetic_live_slot_count() > RETIRE_SYNTHETIC_NODES
        {
            CURRENT.with(|current| current.borrow_mut().take());
            retire(commands);
            return;
        }
    }
}

impl SessionThread {
    /// Runs one command. True when a session ended.
    fn run_command(&self, command: Command) -> bool {
        let id = match command {
            Command::Start {
                id,
                config,
                input,
                output,
            } => {
                let opened = catch_unwind(AssertUnwindSafe(|| {
                    open(&self.ctx, &config, input.clone(), output.clone())
                }));
                match opened {
                    Ok(Ok(hosted)) => {
                        self.sessions.borrow_mut().insert(id, hosted);
                    }
                    Ok(Err(err)) => end_pipes(&input, &output, &err.error()),
                    Err(payload) => end_pipes(&input, &output, &panic_text(payload.as_ref())),
                }
                // A request can be queued before its session started
                // (`retire`).
                id
            }
            Command::Work(id) => id,
        };
        self.run(id)
    }

    /// Runs the messages of session `id` until its input is empty. True
    /// when the session ended.
    fn run(&self, id: u64) -> bool {
        if self.running.borrow().contains(&id) {
            return false;
        }
        let Some((conn, input)) = self
            .sessions
            .borrow()
            .get(&id)
            .map(|hosted| (hosted.conn.clone(), hosted.input.clone()))
        else {
            return false;
        };
        self.running.borrow_mut().push(id);
        let result = catch_unwind(AssertUnwindSafe(|| conn.run(&self.ctx)));
        self.running.borrow_mut().pop();
        drop(conn);
        let reason = match result {
            Ok(Ok(())) if !input.is_finished() => return false,
            Ok(Ok(())) => "the session ended".to_string(),
            Ok(Err(err)) => err.error(),
            Err(payload) => panic_text(payload.as_ref()),
        };
        let Some(hosted) = self.sessions.borrow_mut().remove(&id) else {
            return false;
        };
        hosted.finish(&self.ctx, &reason);
        true
    }

    /// Runs the commands queued now, inside the wait of a session for its
    /// callback (`wake_waiting`).
    fn run_queued(&self) {
        while let Some(Ok(command)) = self.commands.pop_now() {
            self.run_command(command);
        }
    }
}

/// `SessionThread::run_queued` on the session thread.
fn run_queued() {
    let thread = CURRENT.with(|current| current.borrow().clone());
    if let Some(thread) = thread {
        thread.run_queued();
    }
}

/// Runs the frees that the messages left, when no message runs on this
/// thread.
fn drop_garbage_between_messages() {
    if MESSAGES.get() == 0 {
        ts_goport::gostd::local::drop_garbage(|| false);
    }
}

/// Ends this session thread, which has no session: the next command starts
/// a new one, and the sessions queued here meanwhile move to it. The
/// thread's state (its synthetic nodes among it) is freed when it ends.
fn retire(commands: &Arc<Pipe<Command>>) {
    {
        let mut slot = lock(&COMMANDS);
        if slot
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, commands))
        {
            *slot = None;
        }
    }
    commands.close("the session thread retired");
    while let Some(Ok(command)) = commands.pop_now() {
        if let Command::Start { input, output, .. } = &command {
            let (input, output) = (input.clone(), output.clone());
            if let Err(err) = submit(command) {
                end_pipes(
                    &input,
                    &output,
                    &format!("cannot start the tsc-rs API session thread: {err}"),
                );
            }
        }
    }
}

fn end_pipes(input: &FramePipe, output: &FramePipe, reason: &str) {
    output.close(reason);
    input.close(reason);
}

fn panic_text(payload: &(dyn Any + Send)) -> String {
    let value = recovered_value(payload);
    if value.is_empty() {
        "panic".to_string()
    } else {
        format!("panic: {value}")
    }
}

/// One open session on the session thread.
struct Hosted {
    session: Rc<Session>,
    callback_fs: Rc<CallbackFS>,
    conn: Rc<SyncConn>,
    input: Arc<FramePipe>,
    output: Arc<FramePipe>,
}

/// The first half of `StdioServer::run` (the server of the sync protocol)
/// over the session's pipes: the file system, the session and the
/// connection. `Hosted::conn` runs its messages.
fn open(
    ctx: &Context,
    config: &SessionConfig,
    input: Arc<FramePipe>,
    output: Arc<FramePipe>,
) -> Result<Hosted, GoError> {
    let (spawner, system_cwd): (Rc<dyn Spawner>, String) = match tsc::new_os_system() {
        Ok(system) => {
            let cwd = System::get_current_directory(&system);
            (Rc::new(OsSystemSpawner(system)), cwd)
        }
        Err(status) => {
            return Err(errors::new(format!(
                "cannot read the current directory (exit status {})",
                status.code()
            )));
        }
    };
    let cwd = to_rooted_path(config.cwd.as_deref().unwrap_or(&system_cwd), &system_cwd);
    let case_sensitive = config
        .case_sensitive
        .unwrap_or_else(|| vfs::osvfs_fs().use_case_sensitive_file_names());

    // ts#64447: tsgo always passes the case setting, so the callback FS
    // always wraps the base FS.
    let base: Rc<dyn Fs> = bundled::wrap_fs_exported(vfs::osvfs_fs());
    let callback_fs = new_callback_fs(base, &config.callbacks, Some(case_sensitive));
    let session_init = project::SessionInit {
        background_ctx: ctx.clone(),
        logger: None,
        fs: callback_fs.clone(),
        options: Rc::new(project::SessionOptions {
            current_directory: cwd,
            default_library_path: bundled::lib_path_exported(),
            position_encoding: lsproto::PositionEncodingKind::UTF8,
            logging_enabled: false,
            run_external_code: config.run_external_code,
            typings_location: String::new(),
            watch_enabled: false,
            telemetry_enabled: false,
            push_diagnostics_enabled: false,
            debounce_delay: Duration::ZERO,
            checker_pool_options: project::CheckerPoolOptions::default(),
        }),
        spawner: Some(spawner),
        client: None,
        npm_executor: None,
        content_mapper_logger: None,
        parse_cache: None,
        content_mapped_parse_cache: None,
    };
    let session = new_standalone_session(
        &session_init,
        Some(&SessionOptions {
            use_binary_responses: true,
        }),
    );

    let depth = Arc::new(AtomicU32::new(0));
    let handler: Rc<dyn Handler> = Rc::new(FreeAfterMessage {
        inner: session.clone(),
        depth: depth.clone(),
    });
    let rwc: Arc<dyn ReadWriteCloser> =
        Arc::new(MemoryConn::new(input.clone(), output.clone(), depth));
    let protocol = new_message_pack_protocol(rwc.clone());
    let conn = new_sync_conn(rwc, Box::new(protocol), handler);
    conn.set_collect_timing(config.timing);

    callback_fs.set_connection(ctx, conn.clone());
    session.set_connection(conn.clone());
    if config.callbacks.iter().any(|name| name == "writeFile") {
        session.client_writes.set(Some(ClientWrites::Serialized));
    }
    Ok(Hosted {
        session,
        callback_fs,
        conn,
        input,
        output,
    })
}

impl Hosted {
    /// The end of `StdioServer::run`: closes the session and ends its
    /// pipes with `reason`.
    fn finish(self, ctx: &Context, reason: &str) {
        let Hosted {
            session,
            callback_fs,
            conn,
            input,
            output,
        } = self;
        session.close();
        // Not in tsgo, which exits here. The session, the callback FS and
        // the connection hold each other (session -> snapshot host ->
        // callback FS -> connection -> handler -> session); cutting the
        // links to the connection frees them.
        *session.conn.borrow_mut() = None;
        callback_fs.set_connection(ctx, Rc::new(ClosedConn));
        drop(conn);
        drop(session);
        drop(callback_fs);
        // A session can end inside another session's message
        // (`SessionThread::run_queued`), whose data the frees may hold.
        drop_garbage_between_messages();
        end_pipes(&input, &output, reason);
    }
}

/// Go `newSystem()` as the content mapper spawner of the API server
/// (`cmd/tsgo/api.rs` `OsSystemSpawner`).
struct OsSystemSpawner(OsSystem);

impl Spawner for OsSystemSpawner {
    fn spawn(
        &self,
        command: &[String],
        dir: &str,
        stderr: Option<Box<dyn std::io::Write + Send>>,
    ) -> Result<Arc<dyn ProcessExitState>, GoError> {
        System::spawn(&self.0, command, dir, stderr)
    }
}

/// The handler of `api::server` (`FreeAfterMessage`, private there): runs
/// the frees that a message leaves when the top-level message of the thread
/// ends. `depth` counts the messages of this session that run now; the
/// connection reads it.
struct FreeAfterMessage {
    inner: Rc<dyn Handler>,
    depth: Arc<AtomicU32>,
}

impl FreeAfterMessage {
    fn enter(&self) -> MessageGuard<'_> {
        self.depth.fetch_add(1, Ordering::Relaxed);
        MESSAGES.set(MESSAGES.get() + 1);
        MessageGuard(&self.depth)
    }
}

struct MessageGuard<'a>(&'a AtomicU32);

impl Drop for MessageGuard<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
        MESSAGES.set(MESSAGES.get() - 1);
        drop_garbage_between_messages();
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

/// The connection of a closed session: what the callback FS keeps after the
/// session ends, so that it no longer holds the session.
struct ClosedConn;

impl Conn for ClosedConn {
    fn run(&self, _ctx: &Context) -> Result<(), GoError> {
        Ok(())
    }

    fn call(
        &self,
        _ctx: &Context,
        _method: &str,
        _params: Option<Box<dyn AnyValue>>,
    ) -> Result<JsonValue, GoError> {
        Err(ts_goport::ipc::ERR_CONN_CLOSED.clone())
    }

    fn notify(
        &self,
        _ctx: &Context,
        _method: &str,
        _params: Option<Box<dyn AnyValue>>,
    ) -> Result<(), GoError> {
        Err(ts_goport::ipc::ERR_CONN_CLOSED.clone())
    }
}

/// Writes that the server makes before a flush: small writes go into one
/// open chunk, a large write is a chunk of its own (the reader takes a
/// payload that fills a chunk without a copy).
#[derive(Default)]
struct Pending {
    chunks: Vec<Vec<u8>>,
    /// The last chunk takes more small writes.
    open: bool,
}

/// A write of this size or more is its own chunk. The msgpack protocol's
/// 4 KiB `BufWriter` passes such writes through whole.
const LARGE_WRITE: usize = 4096;

/// The connection of a session: `input` holds the frames from the JS
/// thread, `output` gets the frames for it, one push per flush. Between two
/// messages (`depth` 0) an empty input reads as the end, so the
/// connection's run returns and the thread can run other sessions; inside a
/// message (a callback waits for its answer) a read waits, and runs the
/// queued commands when `wake_waiting` signals.
struct MemoryConn {
    input: Arc<FramePipe>,
    output: Arc<FramePipe>,
    depth: Arc<AtomicU32>,
    reading: Mutex<(Vec<u8>, usize)>,
    pending: Mutex<Pending>,
}

impl MemoryConn {
    fn new(input: Arc<FramePipe>, output: Arc<FramePipe>, depth: Arc<AtomicU32>) -> Self {
        MemoryConn {
            input,
            output,
            depth,
            reading: Mutex::new((Vec::new(), 0)),
            pending: Mutex::new(Pending::default()),
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl ReadWriteCloser for MemoryConn {
    fn read(&self, buf: &mut [u8]) -> std::io::Result<usize> {
        let mut reading = lock(&self.reading);
        let (chunk, pos) = &mut *reading;
        while *pos >= chunk.len() {
            let next = if self.depth.load(Ordering::Relaxed) == 0 {
                match self.input.pop_now() {
                    Some(next) => next,
                    None => return Ok(0),
                }
            } else {
                self.input.pop()
            };
            match next {
                Ok(next) if next.is_empty() => run_queued(),
                Ok(next) => {
                    *chunk = next;
                    *pos = 0;
                }
                Err(_) => return Ok(0),
            }
        }
        let n = (chunk.len() - *pos).min(buf.len());
        buf[..n].copy_from_slice(&chunk[*pos..*pos + n]);
        *pos += n;
        Ok(n)
    }

    fn write(&self, buf: &[u8]) -> std::io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let mut pending = lock(&self.pending);
        if buf.len() >= LARGE_WRITE {
            pending.chunks.push(buf.to_vec());
            pending.open = false;
        } else if pending.open
            && let Some(last) = pending.chunks.last_mut()
        {
            last.extend_from_slice(buf);
        } else {
            let mut chunk = Vec::with_capacity(LARGE_WRITE);
            chunk.extend_from_slice(buf);
            pending.chunks.push(chunk);
            pending.open = true;
        }
        Ok(buf.len())
    }

    fn flush(&self) -> std::io::Result<()> {
        let chunks = std::mem::take(&mut *lock(&self.pending)).chunks;
        if chunks.is_empty()
            || self
                .output
                .push(chunks.into_iter().filter(|c| !c.is_empty()))
        {
            Ok(())
        } else {
            Err(Error::new(ErrorKind::BrokenPipe, "the channel is closed"))
        }
    }

    fn close(&self) -> Result<(), GoError> {
        self.output.close("the server closed the connection");
        self.input.close("the server closed the connection");
        Ok(())
    }
}
