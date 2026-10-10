//! Go panics and unported-code hits: `GoPanic`, `go_panic`, `unported!`
//! and the unported registry. They live in `goport_util` as the module
//! `core`, so util files keep their `crate::core::` paths and
//! `$crate::core::record_unported` resolves. `ts_goport`'s `core.rs`
//! re-exports them.

/// Unported hits of every thread, by Go name.
static UNPORTED_NAMES: std::sync::Mutex<std::collections::BTreeMap<&'static str, u64>> =
    std::sync::Mutex::new(std::collections::BTreeMap::new());

/// Records one hit of unported Go code. The runner reports every name.
/// A run with any hit is not a match.
pub fn record_unported(go_name: &'static str) {
    let mut names = UNPORTED_NAMES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *names.entry(go_name).or_default() += 1;
}

/// All unported names hit so far on any thread, with hit counts.
#[must_use]
pub fn unported_report() -> Vec<(&'static str, u64)> {
    let names = UNPORTED_NAMES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    names.iter().map(|(k, v)| (*k, *v)).collect()
}

/// Puts back the unported hits that `unported_report` returned. Work that
/// is thrown away and redone uses it, so the hits are not counted twice.
pub fn restore_unported(report: &[(&'static str, u64)]) {
    let mut names = UNPORTED_NAMES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *names = report.iter().copied().collect();
}

/// Marks unported Go code. It records the hit, then panics so the gap is
/// loud. Use only where a port is missing, never as a fallback.
#[macro_export]
macro_rules! unported {
    ($go_name:expr) => {{
        $crate::core::record_unported($go_name);
        panic!("unported Go code: {}", $go_name)
    }};
}

/// The panic payload of `go_panic`.
pub struct GoPanic {
    /// The Go panic value as the Go runtime prints it (port form). It is
    /// also the Go `%v` of the value that `recover()` returns.
    pub message: String,
    /// The Go type of a value whose type is a named string type, such as
    /// `lsproto.DocumentUri` (`go_panic_typed`). The runtime prints it as
    /// `<type>("<message>")`. `None` for a plain string or an error.
    pub go_type: Option<&'static str>,
    /// A Go `recover()` raised the value again with `panic(r)`
    /// (`go_repanic`). The runtime adds ` [recovered, repanicked]`.
    pub repanicked: bool,
    /// The values (`GoPanic::value_text`) of the earlier panics that were
    /// recovered while this one started: it came from the deferred
    /// function that recovered them (Go `_panic.link`), oldest first. The
    /// runtime prints a line `panic: <value> [recovered]` for each, before
    /// the line of this panic (Go `printpanics`).
    pub recovered_before: Vec<String>,
    /// The port site, for the stderr report.
    pub location: &'static std::panic::Location<'static>,
}

impl GoPanic {
    /// The value as Go `printpanicval` prints it: a typed value is
    /// `<type>("<message>")`, and each newline in the message is followed
    /// by a tab (Go `printindented`).
    #[must_use]
    pub fn value_text(&self) -> String {
        let message = self.message.replace('\n', "\n\t");
        match self.go_type {
            Some(go_type) => format!("{go_type}(\"{message}\")"),
            None => message,
        }
    }
}

/// Go `panic(message)` at a site where the pinned Go panics on the same
/// input. It is not a port gap, so the run ends as the Go runtime ends it:
/// guards that keep a run going after a port gap pass it on
/// (`resume_go_panic`), and the bins write the output so far, print it with
/// `print_go_panic` and exit `EXIT_GO_PANIC`. Other panics stay port gaps
/// (`execute::tsc::EXIT_UNPORTED`).
#[track_caller]
pub fn go_panic(message: String) -> ! {
    std::panic::panic_any(GoPanic {
        message,
        go_type: None,
        repanicked: false,
        recovered_before: Vec::new(),
        location: std::panic::Location::caller(),
    })
}

/// `go_panic` with a value of the named Go string type `go_type`, for
/// example `panic("overlay not found: " + uri)` where `uri` is a
/// `lsproto.DocumentUri`. `recover()` gives the same text, but the runtime
/// prints `panic: <go_type>("<message>")`.
#[track_caller]
pub fn go_panic_typed(go_type: &'static str, message: String) -> ! {
    std::panic::panic_any(GoPanic {
        message,
        go_type: Some(go_type),
        repanicked: false,
        recovered_before: Vec::new(),
        location: std::panic::Location::caller(),
    })
}

/// Go `if r := recover(); r != nil { ...; panic(r) }`: raises a caught
/// panic again. A `go_panic` value is marked, so the runtime line gets
/// ` [recovered, repanicked]`. Any other payload continues as it is.
pub fn go_repanic(mut payload: Box<dyn std::any::Any + Send>) -> ! {
    if let Some(panic) = payload.downcast_mut::<GoPanic>() {
        panic.repanicked = true;
    }
    std::panic::resume_unwind(payload)
}

/// Runs `f` as the rest of a Go deferred function after its `recover()`
/// took the panic `recovered` (`go_recover`), for a body that can panic
/// again (the panic answer of an IPC request). A Go panic in `f` keeps the
/// recovered value first in its chain (`GoPanic::recovered_before`), so
/// the runtime prints `panic: <recovered> [recovered]` before its own line,
/// as Go `printpanics` does. Any other payload continues as it is.
pub fn go_after_recover<R>(recovered: &(dyn std::any::Any + Send), f: impl FnOnce() -> R) -> R {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(value) => value,
        Err(mut payload) => {
            if let Some(panic) = payload.downcast_mut::<GoPanic>() {
                let value = if let Some(earlier) = recovered.downcast_ref::<GoPanic>() {
                    earlier.value_text()
                } else if let Some(text) = recovered.downcast_ref::<&str>() {
                    text.replace('\n', "\n\t")
                } else if let Some(text) = recovered.downcast_ref::<String>() {
                    text.replace('\n', "\n\t")
                } else {
                    String::new()
                };
                panic.recovered_before.insert(0, value);
            }
            std::panic::resume_unwind(payload)
        }
    }
}

/// Runs `f` as the goroutine of Go `sync.WaitGroup.Go(f)`. At go1.27.1 that
/// goroutine has a deferred recover that panics again with the value of a
/// panic in `f` (`go_repanic`), so the runtime line ends with
/// ` [recovered, repanicked]`. The port runs the task on the calling thread.
pub fn go_wait_group_task<R>(f: impl FnOnce() -> R) -> R {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(value) => value,
        Err(payload) => go_repanic(payload),
    }
}

/// Runs `f` as the goroutine of Go `sync.WaitGroup.Go(f)` where the port
/// runs it inline, inside work that a caller's `recover()` guards (the
/// autoimport registry build under a request). A Go `recover()` sees only
/// its own goroutine, so in Go a panic in `f` ends the process whatever the
/// caller recovers: the runtime prints the value with
/// ` [recovered, repanicked]` (see `go_wait_group_task`) and exits
/// `EXIT_GO_PANIC`. The port does the same for a `go_panic` value. Any
/// other payload is a port gap and continues as it is, so the caller's
/// guard still catches it.
pub fn go_wait_group_goroutine<R>(f: impl FnOnce() -> R) -> R {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(value) => value,
        Err(mut payload) => {
            let Some(panic) = payload.downcast_mut::<GoPanic>() else {
                std::panic::resume_unwind(payload)
            };
            panic.repanicked = true;
            print_go_panic(payload.as_ref());
            std::process::exit(EXIT_GO_PANIC)
        }
    }
}

thread_local! {
    /// How many `go_recover` calls this thread is inside.
    static GO_RECOVER_DEPTH: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// Go `defer func() { if r := recover(); r != nil { ... } }()` around `f`,
/// for a recover that answers the panic and goes on (the IPC request
/// handlers). Returns the payload of a panic in `f`. The Go runtime prints
/// nothing for a recovered panic, so the bins' panic hooks stay quiet while
/// `in_go_recover` is true.
// PORT: Go's request recovers (ipc/conn_async.go:206-223,
// ipc/conn_sync.go:119-136, api/session.go:1148-1153) put
// `panic: <value>\n<stack>` in the error response and write nothing to
// stderr. In `tsgo` a plain Rust panic in `f` (a port gap that is not
// `unported!`) is quiet too: its message is in the error response, and
// `GOPORT_TRACE=1` prints it with the backtrace of the panic site on
// stderr. The `goport` dev bin prints it with its port site.
pub fn go_recover<R>(f: impl FnOnce() -> R) -> std::thread::Result<R> {
    GO_RECOVER_DEPTH.with(|depth| depth.set(depth.get() + 1));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    GO_RECOVER_DEPTH.with(|depth| depth.set(depth.get() - 1));
    result
}

/// True inside `go_recover` on this thread.
#[must_use]
pub fn in_go_recover() -> bool {
    GO_RECOVER_DEPTH.with(|depth| depth.get() > 0)
}

/// Runs `f` as a task of Go `core.parallelWorkGroup` (`sync.WaitGroup.Go`).
/// Inside `go_recover` this is `go_wait_group_goroutine`: Go's recover sees
/// only its own goroutine, so a Go panic in `f` ends the process. Elsewhere
/// it is `go_wait_group_task`: the panic reaches the bin, which writes the
/// output so far and prints it as Go prints it.
pub fn go_work_group_task<R>(f: impl FnOnce() -> R) -> R {
    if in_go_recover() {
        go_wait_group_goroutine(f)
    } else {
        go_wait_group_task(f)
    }
}

/// The Go runtime when the OS refuses a new thread (runtime/os_linux.go
/// `newosproc`): it prints the error and the thread count, then
/// `throw("newosproc")` ends the process with exit 2. The port site takes
/// the place of the goroutine dump. `GoThread` calls it after Go's retry;
/// use `GoThread` to start a thread for work that Go runs on goroutines,
/// so the run fails as Go's does.
/// PORT: unlike tsgo's `throw` and panic hook, it does not write the stdout
/// bytes that a report keeps on a regular file (`stdio::CliStdout`), so a
/// fatal start inside a report loses them.
#[cold]
#[inline(never)]
#[track_caller]
pub fn go_fatal_newosproc(err: &std::io::Error) -> ! {
    // PORT: Go prints `mcount()`, its own count of threads.
    let threads = std::fs::read_dir("/proc/self/task").map_or(0, Iterator::count);
    let errno = err.raw_os_error().unwrap_or(0);
    let mut text = format!(
        "runtime: failed to create new OS thread (have {threads} already; errno={errno})\n"
    );
    if err.kind() == std::io::ErrorKind::WouldBlock {
        text.push_str("runtime: may need to increase max user processes (ulimit -u)\n");
    }
    let location = std::panic::Location::caller();
    text.push_str(&format!(
        "fatal error: newosproc\n\n\t{}:{}\n",
        location.file(),
        location.line()
    ));
    use std::io::Write;
    let _ = std::io::stderr().write_all(text.as_bytes());
    std::process::exit(EXIT_GO_PANIC)
}

/// `std::thread::Builder` for a thread that runs work Go runs on
/// goroutines. A start that the OS refuses goes as a Go runtime thread
/// start goes (`newosproc`): it tries again while the error is EAGAIN, then
/// ends the process with Go's text (`go_fatal_newosproc`). Do not start one
/// while this thread holds a lock that one of its thread-local destructors
/// takes: the exit runs those destructors (glibc `exit`), and the process
/// hangs.
#[derive(Default)]
pub struct GoThread {
    name: Option<String>,
    stack_size: Option<usize>,
}

impl GoThread {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// `std::thread::Builder::name`.
    #[must_use]
    pub fn name(mut self, name: String) -> Self {
        self.name = Some(name);
        self
    }

    /// `std::thread::Builder::stack_size`.
    #[must_use]
    pub fn stack_size(mut self, size: usize) -> Self {
        self.stack_size = Some(size);
        self
    }

    /// `std::thread::Builder::spawn`, with Go's retry and fatal error.
    #[track_caller]
    pub fn spawn<F, T>(self, f: F) -> std::thread::JoinHandle<T>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        self.start(f, std::thread::Builder::spawn, std::thread::sleep)
    }

    /// `std::thread::Builder::spawn_scoped`, with Go's retry and fatal
    /// error.
    #[track_caller]
    pub fn spawn_scoped<'scope, 'env, F, T>(
        self,
        scope: &'scope std::thread::Scope<'scope, 'env>,
        f: F,
    ) -> std::thread::ScopedJoinHandle<'scope, T>
    where
        F: FnOnce() -> T + Send + 'scope,
        T: Send + 'scope,
    {
        self.start(
            f,
            |builder, run| builder.spawn_scoped(scope, run),
            std::thread::sleep,
        )
    }

    /// Starts a thread that runs `f`: `clone` is `Builder::spawn` or
    /// `spawn_scoped`, and `sleep` is `std::thread::sleep`. The tests pass
    /// a `clone` that fails and a `sleep` that records.
    #[cfg(not(target_family = "wasm"))]
    #[track_caller]
    fn start<'a, F, T, H>(
        &self,
        f: F,
        mut clone: impl FnMut(std::thread::Builder, ThreadMain<'a, T>) -> std::io::Result<H>,
        sleep: impl FnMut(std::time::Duration),
    ) -> H
    where
        F: FnOnce() -> T + Send + 'a,
        T: 'a,
    {
        let slot = std::sync::Arc::new(std::sync::Mutex::new(Some(f)));
        match retry_on_eagain(|| clone(self.builder(), Box::new(take_once(&slot))), sleep) {
            Ok(handle) => handle,
            Err(err) => go_fatal_newosproc(&err),
        }
    }

    /// wasm32-wasip1 has no threads: `Builder::spawn` always fails there
    /// with an `Unsupported` error, which is not EAGAIN, so the start ends
    /// the process at once with the same text. This leaves std's thread
    /// start and the thread functions out of the wasm module.
    // PORT: not in Go.
    #[cfg(target_family = "wasm")]
    #[track_caller]
    fn start<'a, F, T, H>(
        &self,
        _f: F,
        _clone: impl FnMut(std::thread::Builder, ThreadMain<'a, T>) -> std::io::Result<H>,
        _sleep: impl FnMut(std::time::Duration),
    ) -> H
    where
        F: FnOnce() -> T + Send + 'a,
        T: 'a,
    {
        go_fatal_newosproc(&std::io::ErrorKind::Unsupported.into())
    }

    fn builder(&self) -> std::thread::Builder {
        let mut builder = std::thread::Builder::new();
        if let Some(name) = &self.name {
            builder = builder.name(name.clone());
        }
        if let Some(size) = self.stack_size {
            builder = builder.stack_size(size);
        }
        builder
    }
}

/// The function of one try to start a thread (`GoThread::start`).
type ThreadMain<'a, T> = Box<dyn FnOnce() -> T + Send + 'a>;

/// The function of one try to start a thread. `Builder::spawn` drops the
/// function of a thread that it could not start, so each try takes `f`
/// from a shared slot, and only the started thread takes it.
fn take_once<F: FnOnce() -> T, T>(
    slot: &std::sync::Arc<std::sync::Mutex<Option<F>>>,
) -> impl FnOnce() -> T + use<F, T> {
    let slot = std::sync::Arc::clone(slot);
    move || {
        let f = slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        f.expect("only the started thread takes its function")()
    }
}

// Go: runtime/retry.go:14 retryOnEAGAIN (go1.27.1)
// retryOnEAGAIN retries a function until it does not return EAGAIN.
// It will use an increasing delay between calls, and retry up to 20 times.
// The function argument is expected to return an errno value,
// and retryOnEAGAIN will return any errno value other than EAGAIN.
// If all retries return EAGAIN, then retryOnEAGAIN will return EAGAIN.
// PORT: Go `newosproc` (runtime/os_linux.go:170) calls it with `clone`, and
// throws on the error it returns (`GoThread::start`). `sleep` is Go
// `usleep_no_g`. Go on Windows tries again on ERROR_ACCESS_DENIED instead
// (runtime/os_windows.go:794 createThread); the port does not.
fn retry_on_eagain<H>(
    mut f: impl FnMut() -> std::io::Result<H>,
    mut sleep: impl FnMut(std::time::Duration),
) -> std::io::Result<H> {
    let mut tries = 0;
    loop {
        match f() {
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                tries += 1;
                sleep(std::time::Duration::from_millis(tries)); // milliseconds
                if tries == 20 {
                    return Err(err);
                }
            }
            result => return result,
        }
    }
}

/// `go_panic` with the Go runtime text for a nil pointer dereference, at a
/// site where the pinned Go dereferences nil on the same input. It is cold
/// and out of line, so the nil check at a hot site is one compare.
#[cold]
#[inline(never)]
#[track_caller]
pub fn go_nil_dereference() -> ! {
    go_panic("runtime error: invalid memory address or nil pointer dereference".to_string())
}

/// The Go runtime exit code after a panic that nothing recovers.
pub const EXIT_GO_PANIC: i32 = 2;

/// Continues a caught `go_panic`. Returns any other payload.
pub fn resume_go_panic(payload: Box<dyn std::any::Any + Send>) -> Box<dyn std::any::Any + Send> {
    if payload.is::<GoPanic>() {
        std::panic::resume_unwind(payload);
    }
    payload
}

/// Prints a caught `go_panic` to stderr and returns true. The first lines
/// are the Go runtime ones (Go `printpanics`): `panic: <value> [recovered]`
/// for each earlier panic in `recovered_before`, each next line after a
/// tab, then `panic: <value>` (`GoPanic::value_text`), which ends with
/// ` [recovered, repanicked]` for a value raised again after a recover.
/// The port site takes the place of the goroutine trace. False for any
/// other payload.
pub fn print_go_panic(payload: &(dyn std::any::Any + Send)) -> bool {
    let Some(panic) = payload.downcast_ref::<GoPanic>() else {
        return false;
    };
    let text = format!(
        "{}\n\n\t{}:{}\n",
        go_panic_lines(panic),
        panic.location.file(),
        panic.location.line()
    );
    use std::io::Write;
    let _ = std::io::stderr().write_all(&crate::scanner_util::go_string_bytes(&text));
    true
}

/// The `printpanics` lines of `panic`, without the last newline.
fn go_panic_lines(panic: &GoPanic) -> String {
    let suffix = if panic.repanicked {
        " [recovered, repanicked]"
    } else {
        ""
    };
    let mut lines: Vec<String> = panic
        .recovered_before
        .iter()
        .map(|value| format!("panic: {value} [recovered]"))
        .collect();
    lines.push(format!("panic: {}{suffix}", panic.value_text()));
    lines.join("\n\t")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Error, ErrorKind};
    use std::time::Duration;

    fn eagain() -> Error {
        Error::from(ErrorKind::WouldBlock)
    }

    fn ms(range: std::ops::RangeInclusive<u64>) -> Vec<Duration> {
        range.map(Duration::from_millis).collect()
    }

    // Go: runtime/retry.go:14 retryOnEAGAIN: 20 calls while the error is
    // EAGAIN, with a sleep of 1, 2, ... 20 ms after each, then EAGAIN.
    /// The payload of `f`'s panic.
    fn panic_of(f: impl FnOnce()) -> Box<dyn std::any::Any + Send> {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).expect_err("no panic")
    }

    /// The `printpanics` lines of a `GoPanic` payload after Go's
    /// `WaitGroup.Go` raised it again.
    fn repanicked_lines(mut payload: Box<dyn std::any::Any + Send>) -> String {
        let panic = payload.downcast_mut::<GoPanic>().expect("a GoPanic");
        panic.repanicked = true;
        go_panic_lines(panic)
    }

    // Go `printpanics` (go1.27.1 runtime/panic.go:734) for a panic in the
    // deferred function that recovered another one, raised again by
    // `WaitGroup.Go`. The texts are Go's stderr for ipc `AsyncConn` with a
    // handler that panics and a panic answer that panics (followups39
    // `go test -overlay`), with a handler value of one and of two lines.
    #[test]
    fn go_after_recover_prints_the_recovered_panic_first() {
        let write_panic = || {
            go_panic("write panic".to_string());
        };
        let payload = panic_of(|| go_after_recover(&"handler panic", write_panic));
        assert_eq!(
            repanicked_lines(payload),
            "panic: handler panic [recovered]\n\tpanic: write panic [recovered, repanicked]"
        );
        let handler = panic_of(|| go_panic("handler\npanic".to_string()));
        let payload = panic_of(|| go_after_recover(handler.as_ref(), write_panic));
        assert_eq!(
            repanicked_lines(payload),
            "panic: handler\n\tpanic [recovered]\n\tpanic: write panic [recovered, repanicked]"
        );
        // No panic, and a plain Rust panic, pass through as they are.
        assert_eq!(go_after_recover(&"handler panic", || 7), 7);
        let payload = panic_of(|| go_after_recover(&"handler panic", || panic!("write panic")));
        assert_eq!(payload.downcast_ref::<&str>(), Some(&"write panic"));
    }

    #[test]
    fn retry_on_eagain_tries_20_times_with_growing_sleeps() {
        let mut calls = 0;
        let mut sleeps = Vec::new();
        let result: std::io::Result<()> = retry_on_eagain(
            || {
                calls += 1;
                Err(eagain())
            },
            |d| sleeps.push(d),
        );
        assert_eq!(result.unwrap_err().kind(), ErrorKind::WouldBlock);
        assert_eq!(calls, 20);
        assert_eq!(sleeps, ms(1..=20));
    }

    #[test]
    fn retry_on_eagain_returns_another_error_or_a_start_at_once() {
        let mut sleeps = Vec::new();
        let result: std::io::Result<()> = retry_on_eagain(
            || Err(Error::from(ErrorKind::OutOfMemory)),
            |d| sleeps.push(d),
        );
        assert_eq!(result.unwrap_err().kind(), ErrorKind::OutOfMemory);
        assert!(sleeps.is_empty());

        let mut calls = 0;
        let result = retry_on_eagain(
            || {
                calls += 1;
                if calls < 20 { Err(eagain()) } else { Ok(calls) }
            },
            |d| sleeps.push(d),
        );
        assert_eq!(result.unwrap(), 20);
        assert_eq!(sleeps, ms(1..=19));
    }

    // A failed `Builder::spawn` drops the function it was given. The
    // function of the thread stays in the slot across the failed starts,
    // and only the started thread runs it, once.
    #[test]
    fn go_thread_keeps_the_function_across_failed_starts() {
        let runs = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let counted = runs.clone();
        let mut tries = 0;
        let mut sleeps = Vec::new();
        let handle = GoThread::new().name("retry-test".to_string()).start(
            move || {
                counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                std::thread::current().name().map(str::to_string)
            },
            |builder, run| {
                tries += 1;
                if tries <= 5 {
                    drop(run);
                    return Err(eagain());
                }
                builder.spawn(run)
            },
            |d| sleeps.push(d),
        );
        assert_eq!(handle.join().unwrap().as_deref(), Some("retry-test"));
        assert_eq!(runs.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(tries, 6);
        assert_eq!(sleeps, ms(1..=5));
    }

    #[test]
    fn go_thread_scoped_keeps_the_function_across_failed_starts() {
        let mut value = 0;
        let mut tries = 0;
        let mut sleeps = Vec::new();
        std::thread::scope(|scope| {
            let handle = GoThread::new().start(
                || {
                    value += 1;
                    value
                },
                |builder, run| {
                    tries += 1;
                    if tries <= 19 {
                        drop(run);
                        return Err(eagain());
                    }
                    builder.spawn_scoped(scope, run)
                },
                |d| sleeps.push(d),
            );
            assert_eq!(handle.join().unwrap(), 1);
        });
        assert_eq!(value, 1);
        assert_eq!(tries, 20);
        assert_eq!(sleeps, ms(1..=19));
    }
}
