//! The Node-API surface: `NativeChannel`, `runTsc` and `adjustExternalMemory`. The `#[napi]`
//! expansions are unsafe FFI glue; the code written here is safe and
//! forwards to `channel` and `tsc`.

// napi-rs converts each argument to an owned value before the call.
#![allow(clippy::needless_pass_by_value)]

use napi::bindgen_prelude::{Buffer, FnArgs, Function, Uint8Array};
use napi::{Env, Error, Result};
use napi_derive::napi;

use crate::channel::{Channel, text};
use crate::hooks;

/// The callback that answers the session's callbacks: `(name, payload)` to
/// the answer text. Throwing (or JS that cannot run) sends a harmless
/// answer to a read callback and `CALL_ERROR` to the others
/// (`channel::read_answer`).
type OnCall<'a> = Function<'a, FnArgs<(String, String)>, String>;

/// One tsc-rs API session in this process, on its own thread. The methods
/// block the JS thread until the session answers.
#[napi]
pub struct NativeChannel {
    inner: Channel,
}

#[napi]
impl NativeChannel {
    /// Starts a session with the `tsgo --api` arguments (`--cwd`,
    /// `--callbacks`, `--useCaseSensitiveFileNames`, `--timing`,
    /// `--runExternalCode`).
    #[napi(constructor, catch_unwind)]
    pub fn new(args: Vec<String>) -> Result<Self> {
        hooks::claim_api().map_err(Error::from_reason)?;
        let inner = Channel::open(&args).map_err(Error::from_reason)?;
        Ok(NativeChannel { inner })
    }

    /// Sends a request with a JSON text payload and returns the response
    /// text ("" for none).
    #[napi(catch_unwind)]
    pub fn request_sync(
        &self,
        env: Env,
        method: String,
        payload: String,
        on_call: OnCall<'_>,
    ) -> Result<String> {
        let bytes = self
            .inner
            .request(&method, payload.as_bytes(), &mut |name, data| {
                call(env, &on_call, name, data)
            })?;
        Ok(text(bytes))
    }

    /// Sends a request with a byte payload and returns the response bytes
    /// (empty for none) in a Buffer of its own at offset 0.
    #[napi(catch_unwind)]
    pub fn request_binary_sync(
        &self,
        env: Env,
        method: String,
        payload: Uint8Array,
        on_call: OnCall<'_>,
    ) -> Result<Buffer> {
        let bytes = self.inner.request(&method, &payload, &mut |name, data| {
            call(env, &on_call, name, data)
        })?;
        Ok(Buffer::from(bytes))
    }

    /// Ends the session. Inside a callback, the outer request ends it when
    /// it returns.
    #[napi(catch_unwind)]
    pub fn close(&self) {
        self.inner.close();
    }

    #[napi(getter, catch_unwind)]
    pub fn closed(&self) -> bool {
        self.inner.is_closed()
    }
}

/// Calls `on_call` in a handle scope of its own: a request can make
/// thousands of callbacks.
fn call(env: Env, on_call: &OnCall<'_>, name: String, data: String) -> Result<String> {
    env.run_in_scope(|| on_call.call(FnArgs::from((name, data))))
}

/// Runs the tsc command line (the arguments after `tsc`) in this process
/// and returns its exit status. A process runs it at most once, and never
/// in a process with an API session.
#[napi(catch_unwind)]
pub fn run_tsc(args: Vec<String>) -> Result<i32> {
    hooks::claim_tsc().map_err(Error::from_reason)?;
    crate::tsc::run(args).map_err(Error::from_reason)
}

/// Tells V8 that the process holds `change` more bytes (or fewer, when
/// negative) outside the JS heap, and returns the new total. Each program
/// holds a snapshot in tsc-rs that V8 cannot see; without this, the garbage
/// collector rarely runs and the snapshots of unreachable programs pile up.
#[napi(catch_unwind)]
pub fn adjust_external_memory(env: Env, change: i64) -> Result<i64> {
    env.adjust_external_memory(change)
}
