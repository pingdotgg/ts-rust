//! The JS-thread side of a session: a port of TS7's `SyncRpcChannel`
//! request loop (`src/api/syncChannel.ts` `requestBytesSync`) over the
//! session's pipes. The JS thread blocks in `request`, and answers each
//! callback of the session inline through `on_call`, on the JS thread. A
//! callback may make requests of its own (ts#64299): the protocol is last
//! in, first out, so they nest. A request to another session runs inside the
//! wait of the session whose callback makes it (`session::wake_waiting`).

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use napi::Error;

use crate::frame::{self, FrameReader, ReadError};
use crate::session::{self, FramePipe, Handle};

thread_local! {
    /// The sessions whose callbacks run on this JS thread now, the inner
    /// one last, with their inputs.
    static IN_CALLBACK: RefCell<Vec<(u64, Arc<FramePipe>)>> = const { RefCell::new(Vec::new()) };
}

/// What a read callback answers when `on_call` fails, as `channel.js`
/// `readAnswers` answers a callback that throws. `on_call` fails when JS
/// cannot run (its execution terminates: `worker.terminate()`, a `vm`
/// timeout), and `tsc-rs` reads a failed read callback as a Go panic, which
/// inside program loading ends the process (`api/callbackfs.rs`
/// `call_decoded`). The other callbacks get `CALL_ERROR`, which `tsc-rs`
/// returns as an error.
fn read_answer(name: &str) -> Option<&'static str> {
    match name {
        "readFile" | "stat" => Some(r#"{"kind":"missing"}"#),
        "fileExists" | "directoryExists" => Some(r#"{"kind":"value","value":false}"#),
        "getAccessibleEntries" => Some(r#"{"kind":"value","value":{"files":[],"directories":[]}}"#),
        "realpath" => Some(r#"{"kind":"identity"}"#),
        _ => None,
    }
}

pub struct Channel {
    session: Handle,
    to_session: Arc<FramePipe>,
    from_session: Arc<FramePipe>,
    reader: Mutex<FrameReader>,
    /// Requests that run now: the outer one and those that callbacks make.
    depth: AtomicU32,
    /// `close` came during a request; the outer request closes at its end.
    close_pending: AtomicBool,
}

impl Channel {
    /// Starts a session with the `tsgo --api` arguments `args`.
    pub fn open(args: &[String]) -> Result<Channel, String> {
        let config = session::parse_args(args)?;
        let to_session = Arc::new(FramePipe::new());
        let from_session = Arc::new(FramePipe::new());
        let session = session::start(config, to_session.clone(), from_session.clone())
            .map_err(|err| format!("cannot start the tsc-rs API session thread: {err}"))?;
        Ok(Channel {
            session,
            to_session,
            from_session,
            reader: Mutex::new(FrameReader::default()),
            depth: AtomicU32::new(0),
            close_pending: AtomicBool::new(false),
        })
    }

    /// Sends a request and returns the payload of its response. `on_call`
    /// answers each callback the session makes meanwhile. When it fails,
    /// the session gets a harmless answer (`read_answer`) or a
    /// `CALL_ERROR`, the request runs to its end, and the first failure is
    /// the result.
    pub fn request(
        &self,
        method: &str,
        payload: &[u8],
        on_call: &mut dyn FnMut(String, String) -> napi::Result<String>,
    ) -> napi::Result<Vec<u8>> {
        if self.to_session.is_closed() {
            return Err(self.closed_error());
        }
        let waiting = self.waiting_session()?;
        self.depth.fetch_add(1, Ordering::SeqCst);
        let result = self.exchange(method, payload, waiting.as_deref(), on_call);
        if self.depth.fetch_sub(1, Ordering::SeqCst) == 1
            && self.close_pending.swap(false, Ordering::SeqCst)
        {
            self.shut();
        }
        result
    }

    /// The input of the session whose callback makes this request, when that
    /// is another session: all sessions run on one thread, and it waits in
    /// that callback. A session below it on the stack waits for the inner
    /// one and cannot run.
    fn waiting_session(&self) -> napi::Result<Option<Arc<FramePipe>>> {
        let id = self.session.id();
        IN_CALLBACK.with(|stack| {
            let stack = stack.borrow();
            match stack.last() {
                Some((inner, _)) if *inner == id => Ok(None),
                Some(_) if stack.iter().any(|(outer, _)| *outer == id) => Err(Error::from_reason(
                    "a callback of one tsc-rs API session cannot make a request to a session that waits for that callback",
                )),
                Some((_, input)) => Ok(Some(input.clone())),
                None => Ok(None),
            }
        })
    }

    fn exchange(
        &self,
        method: &str,
        payload: &[u8],
        waiting: Option<&FramePipe>,
        on_call: &mut dyn FnMut(String, String) -> napi::Result<String>,
    ) -> napi::Result<Vec<u8>> {
        self.send(frame::REQUEST, method.as_bytes(), payload)?;
        self.session.schedule().map_err(|err| {
            Error::from_reason(format!("cannot reach the tsc-rs API session thread: {err}"))
        })?;
        if let Some(waiting) = waiting {
            session::wake_waiting(waiting);
        }
        let mut failure: Option<Error> = None;
        loop {
            let frame = self.read_frame()?;
            match frame.kind {
                frame::RESPONSE | frame::ERROR if frame.method != method.as_bytes() => {
                    return Err(Error::from_reason(format!(
                        "name mismatch for response: expected `{method}`, got `{}`",
                        text(frame.method)
                    )));
                }
                frame::RESPONSE => return failure.map_or(Ok(frame.payload), Err),
                frame::ERROR => {
                    return Err(failure.unwrap_or_else(|| Error::from_reason(text(frame.payload))));
                }
                frame::CALL => {
                    let name = text(frame.method);
                    let entry = (self.session.id(), self.to_session.clone());
                    IN_CALLBACK.with(|stack| stack.borrow_mut().push(entry));
                    let answer = on_call(name.clone(), text(frame.payload));
                    IN_CALLBACK.with(|stack| stack.borrow_mut().pop());
                    match answer {
                        Ok(answer) => {
                            self.send(frame::CALL_RESPONSE, name.as_bytes(), answer.as_bytes())?;
                        }
                        Err(err) => {
                            if let Some(answer) = read_answer(&name) {
                                self.send(
                                    frame::CALL_RESPONSE,
                                    name.as_bytes(),
                                    answer.as_bytes(),
                                )?;
                            } else {
                                let message = err.reason.trim().to_string();
                                self.send(frame::CALL_ERROR, name.as_bytes(), message.as_bytes())?;
                            }
                            failure.get_or_insert(err);
                        }
                    }
                }
                kind => {
                    return Err(Error::from_reason(format!(
                        "Invalid message type from the tsc-rs API session: {kind}"
                    )));
                }
            }
        }
    }

    fn send(&self, kind: u8, method: &[u8], payload: &[u8]) -> napi::Result<()> {
        let bytes = frame::encode(kind, method, payload).map_err(Error::from_reason)?;
        if self.to_session.push([bytes]) {
            Ok(())
        } else {
            Err(self.closed_error())
        }
    }

    fn read_frame(&self) -> napi::Result<frame::Frame> {
        let result = self.lock_reader().read(&self.from_session);
        result.map_err(|err| match err {
            ReadError::Closed(reason) => {
                self.to_session.close(&reason);
                ended_error(&reason)
            }
            ReadError::Invalid(text) => Error::from_reason(text),
        })
    }

    fn lock_reader(&self) -> MutexGuard<'_, FrameReader> {
        self.reader.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn closed_error(&self) -> Error {
        match self.to_session.reason() {
            Some(reason) if reason != CLOSED => ended_error(&reason),
            _ => Error::from_reason("the tsc-rs API channel is closed"),
        }
    }

    /// Ends the session. During a request (a callback calls it), the outer
    /// request ends it when it returns.
    pub fn close(&self) {
        if self.depth.load(Ordering::SeqCst) > 0 {
            self.close_pending.store(true, Ordering::SeqCst);
            return;
        }
        self.shut();
    }

    pub fn is_closed(&self) -> bool {
        self.to_session.is_closed()
    }

    /// Closes the session's input: the session thread reads the end and
    /// closes the session. The JS thread does not wait for that.
    fn shut(&self) {
        if !self.to_session.is_closed() {
            self.to_session.close(CLOSED);
            self.session.closed();
        }
    }
}

impl Drop for Channel {
    fn drop(&mut self) {
        self.shut();
    }
}

/// The close reason of a channel that its owner closed.
const CLOSED: &str = "the channel was closed";

fn ended_error(reason: &str) -> Error {
    Error::from_reason(format!("the tsc-rs API session ended: {reason}"))
}

/// UTF-8 text of protocol bytes, as `Buffer.toString("utf-8")` decodes it.
pub fn text(bytes: Vec<u8>) -> String {
    String::from_utf8(bytes)
        .unwrap_or_else(|err| String::from_utf8_lossy(err.as_bytes()).into_owned())
}
