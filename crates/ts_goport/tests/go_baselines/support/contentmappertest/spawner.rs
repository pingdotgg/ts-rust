//! Go: internal/testutil/contentmappertest/spawner.go (tsgo#4712).

use std::io::{Read as _, Write};
use std::net::Shutdown;
#[cfg(unix)]
use std::os::unix::net::UnixStream as Stream;
// Windows has no socket pair in std: a connected loopback TCP pair gives the
// same two-ended byte stream.
#[cfg(windows)]
use std::net::TcpStream as Stream;
use std::rc::Rc;

use ts_goport::contentmapper::ProcessExitState;
use ts_goport::gostd::context::background;

use super::dynamic_verbatim::ProjectLifecycle;
use super::prelude::*;
use super::protocol::{ServerHandler, StaticProjectHandler};
use super::registry::{DYNAMIC_VERBATIM_MAPPER, handler_for_mapper};
use super::transforming::Handler;

// Go: spawner.go:13 Serve
// Serve drives the transforming mapper over the connection until it closes or ctx is cancelled.
pub fn serve(ctx: &Context, rwc: Arc<dyn ipc::ReadWriteCloser>) -> Result<(), GoError> {
    let handler = ServerHandler(Box::new(StaticProjectHandler {
        handler: Box::new(Handler::default()),
    }));
    ipc::new_async_conn(rwc, Rc::new(handler)).run(ctx)
}

// Go: spawner.go:18 NewSpawner
// NewSpawner returns an in-process spawner for the test mapper implementations.
pub fn new_spawner() -> Rc<dyn contentmapper::Spawner> {
    Rc::new(Spawner { lifecycle: None })
}

// Go: spawner.go:23 NewSpawnerWithProjectLifecycle
// NewSpawnerWithProjectLifecycle returns an in-process spawner that records project protocol calls.
pub fn new_spawner_with_project_lifecycle(
    lifecycle: Arc<ProjectLifecycle>,
) -> Rc<dyn contentmapper::Spawner> {
    Rc::new(Spawner {
        lifecycle: Some(lifecycle),
    })
}

// Go: spawner.go:27 spawner
// PORT: Go `*ProjectLifecycle` is `Option<Arc<ProjectLifecycle>>` (nil is
// `None`); the mapper thread counts on it.
struct Spawner {
    lifecycle: Option<Arc<ProjectLifecycle>>,
}

impl contentmapper::Spawner for Spawner {
    // Go: spawner.go:31 spawner.Spawn
    // PORT: the mapper side runs on its own thread (Go: a goroutine). The
    // handler moves there, and the thread makes the connection around it.
    fn spawn(
        &self,
        command: &[String],
        _dir: &str,
        _stderr: Option<Box<dyn Write + Send>>,
    ) -> Result<Arc<dyn ProcessExitState>, GoError> {
        let mut handler = handler_for_mapper(command, self.lifecycle.as_ref())?;
        if command[0] != DYNAMIC_VERBATIM_MAPPER {
            handler = Box::new(StaticProjectHandler { handler });
        }
        let (client, server) = net_pipe();
        std::thread::spawn(move || {
            let server: Arc<dyn ipc::ReadWriteCloser> = server;
            let _ = ipc::new_async_conn(server, Rc::new(ServerHandler(handler))).run(&background());
        });
        Ok(client)
    }
}

/// One end of Go `net.Pipe`.
/// PORT: a Unix socket pair. Closing one end ends the reads of the other.
pub(super) struct PipeEnd {
    stream: Stream,
}

impl ipc::ReadWriteCloser for PipeEnd {
    fn read(&self, buf: &mut [u8]) -> std::io::Result<usize> {
        (&self.stream).read(buf)
    }

    fn write(&self, buf: &[u8]) -> std::io::Result<usize> {
        (&self.stream).write(buf)
    }

    fn flush(&self) -> std::io::Result<()> {
        (&self.stream).flush()
    }

    // Go `net.Pipe` Close returns nil.
    fn close(&self) -> Result<(), GoError> {
        let _ = self.stream.shutdown(Shutdown::Both);
        Ok(())
    }
}

// Go `net.Pipe` has no `ExitCode` method.
impl ProcessExitState for PipeEnd {}

/// Go `net.Pipe()`: the client end and the server end.
fn net_pipe() -> (Arc<PipeEnd>, Arc<PipeEnd>) {
    let (client, server) = stream_pair().expect("contentmappertest: socket pair");
    (
        Arc::new(PipeEnd { stream: client }),
        Arc::new(PipeEnd { stream: server }),
    )
}

#[cfg(unix)]
fn stream_pair() -> std::io::Result<(Stream, Stream)> {
    Stream::pair()
}

#[cfg(windows)]
fn stream_pair() -> std::io::Result<(Stream, Stream)> {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
    let client = Stream::connect(listener.local_addr()?)?;
    let (server, _) = listener.accept()?;
    Ok((client, server))
}
