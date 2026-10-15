//! Go `cmd/tsc/main.go`, and the parts of the Go standard library that
//! package main needs and the crate does not have yet: `flag` (bool, int
//! and string flags, `Parse`, the default usage text) and
//! `signal.NotifyContext`.

use crate::cmd::tsgo::prelude::*;

use crate::cmd::tsgo::api::run_api;
use crate::cmd::tsgo::lsp::run_lsp;
use crate::gostd::context::{self, CancelFunc};
use crate::gostd::errors;
#[cfg(not(target_family = "wasm"))]
use signal_hook::consts::{SIGINT, SIGTERM};
#[cfg(unix)]
use signal_hook::iterator::Signals;
use std::cell::Cell;
use std::sync::{Arc, LazyLock};

// Go: cmd/tsc/main.go:14 main
// PORT: `bin/goport.rs` `main` calls `run_main` before its own compile
// path and exits with the status it returns.

// Go: cmd/tsc/main.go:18 runMain
// PORT: `args` is Go `osutil.Args()[1:]`. `None` means Go continues with
// `execute.CommandLine`; goport continues with its own compile path, which
// replaces it. Go `core.ApplyDebugStackLimit()` (the TS_GO_DEBUG_STACK_LIMIT
// override) becomes the stack of the thread that runs the command
// (`gostd::stack::max_stack_size`).
// PORT: Go `signal.NotifyContext` before `execute.CommandLine` is in
// `bin/tsgo.rs`, which runs that path; the goport compile path has none.
pub fn run_main(args: &[String]) -> Option<i32> {
    if !args.is_empty() {
        match args[0].as_str() {
            "--lsp" => return Some(run_on_big_stack(args[1..].to_vec(), |args| run_lsp(&args))),
            "--api" => return Some(run_on_big_stack(args[1..].to_vec(), |args| run_api(&args))),
            _ => {}
        }
    }
    None
}

// PORT: runs `f` on a new thread with the Go maximum stack
// (`gostd::stack::max_stack_size`) and returns its status. Go runs it on
// the main goroutine. A thread that cannot start ends the process as the
// Go runtime does (`GoThread`).
// A panic that reaches the top of that thread ends Go with a crash. A Go
// panic (`core::go_panic`) ends it as the Go runtime does, as `bin/tsgo.rs`
// does: `panic: <message>` on stderr and `EXIT_GO_PANIC` (2). Any other
// panic is a port gap: goport returns `EXIT_UNPORTED` (70), as
// `bin/goport.rs` does for a failed worker.
fn run_on_big_stack(args: Vec<String>, f: fn(Vec<String>) -> i32) -> i32 {
    let worker = std::thread::Builder::new()
        .name("tsgo".to_string())
        .stack_size(crate::gostd::stack::max_stack_size())
        .spawn(move || f(args))
        .expect("failed to spawn tsgo worker thread");
    match worker.join() {
        Ok(code) => code,
        Err(payload) if crate::core::print_go_panic(payload.as_ref()) => crate::core::EXIT_GO_PANIC,
        Err(_) => crate::execute::tsc::EXIT_UNPORTED,
    }
}

// Go: os/signal/signal.go:293 NotifyContext (go1.27.1)
// NotifyContext returns a copy of the parent context that is marked done
// (its Done channel is closed) when one of the listed signals arrives,
// when the returned stop function is called, or when the parent context's
// Done channel is closed, whichever happens first.
// PORT: the signals are fixed. Every tsgo caller passes SIGINT
// (`os.Interrupt`) and SIGTERM. Go `Notify(c.ch, ...)` is a signal-hook
// `Signals`, and the goroutine is a thread that waits on it. Closing its
// handle ends the wait: that is the `<-c.Done()` case of the `select`.
// PORT: after Go `Stop(c.ch)`, a later SIGINT or SIGTERM kills the process
// (the Go runtime default). The signal-hook handler stays installed, so
// the port ignores such a signal. Every caller returns right after `stop`.
#[cfg(unix)]
pub fn notify_context(parent: &Context) -> (Context, CancelFunc) {
    let (ctx, cancel) = context::with_cancel_cause(parent);
    // Go: c.ch = make(chan os.Signal, 1); Notify(c.ch, c.signals...)
    let mut signals =
        Signals::new([SIGINT, SIGTERM]).expect("signal.Notify: cannot register SIGINT and SIGTERM");
    let handle = signals.handle();
    if ctx.err().is_none() {
        // Go: the `<-c.Done()` case of the `select` below.
        if let Some(done) = ctx.done() {
            let handle = handle.clone();
            let _ = done.register_waker(move || handle.close());
        }
        let cancel = cancel.clone();
        std::thread::Builder::new()
            .name("signal.NotifyContext".to_string())
            .spawn(move || {
                // Go: select { case s := <-c.ch: ...; case <-c.Done(): }
                if let Some(s) = signals.forever().next() {
                    let text = format!("{} signal received", signal_string(s));
                    cancel(Some(errors::from_value(SignalError(text))));
                }
            })
            .expect("failed to spawn signal.NotifyContext thread");
    }
    // Go: signal.go:322 signalCtx.stop
    let stop: CancelFunc = Arc::new(move || {
        cancel(None);
        // Go: Stop(c.ch)
        handle.close();
    });
    (ctx, stop)
}

// PORT: off unix, signal-hook has no `Signals` iterator, so no signal
// cancels the context: Ctrl+C ends the process (the OS default) where Go
// cancels it. `stop` cancels it, as in Go. Not run on such a target.
#[cfg(not(unix))]
pub fn notify_context(parent: &Context) -> (Context, CancelFunc) {
    let (ctx, cancel) = context::with_cancel_cause(parent);
    let stop: CancelFunc = Arc::new(move || cancel(None));
    (ctx, stop)
}

// Go: os/signal/signal.go:352 signalError
// PORT: Go `Is(target error) bool` (true for `context.Canceled`) has no
// port form. No port code reads the cause.
#[cfg(not(target_family = "wasm"))]
#[derive(Debug, PartialEq)]
struct SignalError(String);

#[cfg(not(target_family = "wasm"))]
impl std::fmt::Display for SignalError {
    // Go: signal.go:354 signalError.Error
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

// Go: syscall/syscall_unix.go Signal.String, with the Linux `signalList`
// names of the two signals that `notify_context` registers.
#[cfg(not(target_family = "wasm"))]
fn signal_string(s: i32) -> String {
    match s {
        SIGINT => "interrupt".to_string(),
        SIGTERM => "terminated".to_string(),
        _ => format!("signal {s}"),
    }
}

// Go: core.Must(os.Getwd()), for the LSP and the API.
// PORT: Go `os.Getwd` is `frontend::vfs::os_current_dir`, the same one the
// compile path reads (a symlinked cwd keeps the `$PWD` link path; the value
// is in the port form of the Go bytes). Go `core.Must` panics with the
// error value, so the run ends with `panic: <err.Error()>` and exit 2
// (`core::go_panic`, `getwd_error_text`).
pub fn must_getwd() -> String {
    crate::frontend::vfs::os_current_dir()
        .unwrap_or_else(|err| crate::core::go_panic(crate::frontend::vfs::getwd_error_text(&err)))
}

// Go: flag/flag.go (go1.27.1), the part that package main uses.

// Go: flag.go:101 ErrHelp
// ErrHelp is the error returned if the -help or -h flag is invoked
// but no such flag is defined.
pub static ERR_HELP: LazyLock<GoError> = LazyLock::new(|| errors::new("flag: help requested"));

// Go: flag.go:105 errParse
// errParse is returned by Set if a flag's value fails to parse, such as with an invalid integer for Int.
// It then gets wrapped through failf to provide more information.
pub static ERR_PARSE: LazyLock<GoError> = LazyLock::new(|| errors::new("parse error"));

// Go: flag.go:109 errRange
// errRange is returned by Set if a flag's value is out of range.
// It then gets wrapped through failf to provide more information.
pub static ERR_RANGE: LazyLock<GoError> = LazyLock::new(|| errors::new("value out of range"));

// Go: flag.go ErrorHandling
// ErrorHandling defines how [FlagSet.Parse] behaves if the parse fails.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorHandling {
    ContinueOnError, // Return a descriptive error.
    ExitOnError,     // Call os.Exit(2) or for -h/-help Exit(0).
    PanicOnError,    // Call panic with a descriptive error.
}

// Go: flag.go boolValue, intValue and stringValue, the `Value` kinds
// package main defines. The value lives behind the pointer that `Bool`,
// `Int` and `String` return.
// PORT: Go `int` is 64 bits on the release targets (`strconv.IntSize`), so
// the int flag is `i64`.
#[derive(Clone)]
pub enum FlagValue {
    Bool(Rc<Cell<bool>>),
    Int(Rc<Cell<i64>>),
    String(Rc<RefCell<String>>),
}

impl FlagValue {
    // Go: flag.go:133 boolValue.Set, intValue.Set and stringValue.Set
    pub fn set(&self, s: &str) -> Result<(), GoError> {
        match self {
            FlagValue::Bool(b) => {
                let (v, ok) = parse_bool(s);
                b.set(v);
                if !ok {
                    return Err(ERR_PARSE.clone());
                }
                Ok(())
            }
            FlagValue::Int(i) => {
                // Go: strconv.ParseInt(s, 0, strconv.IntSize), then numError
                let (v, err) = parse_int(s);
                i.set(v);
                match err {
                    None => Ok(()),
                    Some(NumError::Syntax) => Err(ERR_PARSE.clone()),
                    Some(NumError::Range) => Err(ERR_RANGE.clone()),
                }
            }
            FlagValue::String(v) => {
                *v.borrow_mut() = s.to_string();
                Ok(())
            }
        }
    }

    // Go: boolValue.String (strconv.FormatBool), intValue.String
    // (strconv.Itoa) and stringValue.String
    pub fn string(&self) -> String {
        match self {
            FlagValue::Bool(b) => b.get().to_string(),
            FlagValue::Int(i) => i.get().to_string(),
            FlagValue::String(v) => v.borrow().clone(),
        }
    }

    // Go: boolValue.IsBoolFlag
    pub fn is_bool_flag(&self) -> bool {
        matches!(self, FlagValue::Bool(_))
    }
}

/// Go `strconv.ErrSyntax` and `strconv.ErrRange`, the `*NumError` causes
/// that `numError` maps to `errParse` and `errRange`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NumError {
    Syntax,
    Range,
}

// Go: internal/strconv/atoi.go:11 lower (go1.27.1)
fn lower(c: u8) -> u8 {
    c | (b'x' - b'X')
}

// Go: internal/strconv/atoi.go:172 ParseInt (go1.27.1), with base 0 and bitSize 64
// PORT: only the cause of a Go `*NumError` is kept (see `NumError`).
fn parse_int(s: &str) -> (i64, Option<NumError>) {
    if s.is_empty() {
        return (0, Some(NumError::Syntax));
    }

    // Pick off leading sign.
    let mut rest = s;
    let mut neg = false;
    if rest.as_bytes()[0] == b'+' {
        rest = &rest[1..];
    } else if rest.as_bytes()[0] == b'-' {
        neg = true;
        rest = &rest[1..];
    }

    // Convert unsigned and check range.
    let (un, err) = parse_uint(rest);
    if err.is_some_and(|err| err != NumError::Range) {
        return (0, err);
    }

    let cutoff: u64 = 1 << 63;
    if !neg && un >= cutoff {
        return ((cutoff - 1) as i64, Some(NumError::Range));
    }
    if neg && un > cutoff {
        return (i64::MIN, Some(NumError::Range));
    }
    let mut n = un as i64;
    if neg {
        n = n.wrapping_neg();
    }
    (n, None)
}

// Go: internal/strconv/atoi.go:47 ParseUint (go1.27.1), with base 0 and bitSize 64
fn parse_uint(s: &str) -> (u64, Option<NumError>) {
    if s.is_empty() {
        return (0, Some(NumError::Syntax));
    }

    let s0 = s;
    let mut s = s.as_bytes();
    // Look for octal, hex prefix.
    let mut base: u8 = 10;
    if s[0] == b'0' {
        if s.len() >= 3 && lower(s[1]) == b'b' {
            base = 2;
            s = &s[2..];
        } else if s.len() >= 3 && lower(s[1]) == b'o' {
            base = 8;
            s = &s[2..];
        } else if s.len() >= 3 && lower(s[1]) == b'x' {
            base = 16;
            s = &s[2..];
        } else {
            base = 8;
            s = &s[1..];
        }
    }

    // Cutoff is the smallest number such that cutoff*base > maxUint64.
    let cutoff = u64::MAX / u64::from(base) + 1;
    let max_val = u64::MAX;

    let mut underscores = false;
    let mut n: u64 = 0;
    for &c in s {
        let d = if c == b'_' {
            // base0 is always true here.
            underscores = true;
            continue;
        } else if c.is_ascii_digit() {
            c - b'0'
        } else if (b'a'..=b'z').contains(&lower(c)) {
            lower(c) - b'a' + 10
        } else {
            return (0, Some(NumError::Syntax));
        };

        if d >= base {
            return (0, Some(NumError::Syntax));
        }

        if n >= cutoff {
            // n*base overflows
            return (max_val, Some(NumError::Range));
        }
        n *= u64::from(base);

        // PORT: Go also checks `n1 > maxVal`, which cannot hold with
        // bitSize 64 (maxVal is the largest uint64).
        let (n1, overflow) = n.overflowing_add(u64::from(d));
        if overflow {
            // n+d overflows
            return (max_val, Some(NumError::Range));
        }
        n = n1;
    }

    if underscores && !underscore_ok(s0) {
        return (0, Some(NumError::Syntax));
    }

    (n, None)
}

// Go: internal/strconv/atoi.go:252 underscoreOK (go1.27.1)
// underscoreOK reports whether the underscores in s are allowed.
// Checking them in this one function lets all the parsers skip over them simply.
// Underscore must appear only between digits or between a base prefix and a digit.
fn underscore_ok(s: &str) -> bool {
    // saw tracks the last character (class) we saw:
    // ^ for beginning of number,
    // 0 for a digit or base prefix,
    // _ for an underscore,
    // ! for none of the above.
    let mut saw = b'^';
    let mut i = 0;
    let mut s = s.as_bytes();

    // Optional sign.
    if !s.is_empty() && (s[0] == b'-' || s[0] == b'+') {
        s = &s[1..];
    }

    // Optional base prefix.
    let mut hex = false;
    if s.len() >= 2
        && s[0] == b'0'
        && (lower(s[1]) == b'b' || lower(s[1]) == b'o' || lower(s[1]) == b'x')
    {
        i = 2;
        saw = b'0'; // base prefix counts as a digit for "underscore as digit separator"
        hex = lower(s[1]) == b'x';
    }

    // Number proper.
    while i < s.len() {
        // Digits are always okay.
        if s[i].is_ascii_digit() || (hex && (b'a'..=b'f').contains(&lower(s[i]))) {
            saw = b'0';
            i += 1;
            continue;
        }
        // Underscore must follow digit.
        if s[i] == b'_' {
            if saw != b'0' {
                return false;
            }
            saw = b'_';
            i += 1;
            continue;
        }
        // Underscore must also be followed by digit.
        if saw == b'_' {
            return false;
        }
        // Saw non-digit, non-underscore.
        saw = b'!';
        i += 1;
    }
    saw != b'_'
}

// Go: strconv/atob.go ParseBool
// PORT: Go returns `(bool, error)`; the error is a `*NumError` that
// boolValue.Set replaces with errParse, so only `ok` is kept.
fn parse_bool(s: &str) -> (bool, bool) {
    match s {
        "1" | "t" | "T" | "TRUE" | "true" | "True" => (true, true),
        "0" | "f" | "F" | "FALSE" | "false" | "False" => (false, true),
        _ => (false, false),
    }
}

// Go: flag.go Flag
// A Flag represents the state of a flag.
pub struct Flag {
    pub name: String,      // name as it appears on command line
    pub usage: String,     // help message
    pub value: FlagValue,  // value as set
    pub def_value: String, // default value (as text); for usage message
}

// Go: flag.go FlagSet
// A FlagSet represents a set of defined flags.
// PORT: Go `Usage` is always `defaultUsage` here and `output` is always
// stderr; `undef` (from `Set` before a definition) has no caller.
pub struct FlagSet {
    pub name: String,
    pub parsed: bool,
    // PORT: Go `actual map[string]*Flag`; only the names are kept.
    pub actual: FxHashSet<String>,
    pub formal: FxHashMap<String, Flag>,
    pub args: Vec<String>, // arguments after flags
    pub error_handling: ErrorHandling,
}

// Go: flag.go:431 FlagSet.Output (go1.27.1)
/// Writes one message of the flag set to Go `os.Stderr` (the flag sets
/// here set no output): one write of the Go bytes (an argument can hold raw
/// bytes), as Go `fmt.Fprint` does. Go ignores the write error, and a
/// stderr pipe with no reader ends the process by SIGPIPE (`stdio`), not by
/// a panic of `eprint!`.
fn output(text: &str) {
    let _ = tsc::write_go_output(&mut tsc::stdio::Stderr, text.as_bytes());
}

// Go: flag.go:1223 NewFlagSet
// NewFlagSet returns a new, empty flag set with the specified name and
// error handling property. If the name is not empty, it will be printed
// in the default usage message and in error messages.
pub fn new_flag_set(name: &str, error_handling: ErrorHandling) -> FlagSet {
    FlagSet {
        name: name.to_string(),
        parsed: false,
        actual: FxHashSet::default(),
        formal: FxHashMap::default(),
        args: Vec::new(),
        error_handling,
    }
}

impl FlagSet {
    // Go: flag.go FlagSet.Bool
    // Bool defines a bool flag with specified name, default value, and usage string.
    // The return value is the address of a bool variable that stores the value of the flag.
    pub fn bool(&mut self, name: &str, value: bool, usage: &str) -> Rc<Cell<bool>> {
        let p = Rc::new(Cell::new(value));
        self.var(FlagValue::Bool(p.clone()), name, usage);
        p
    }

    // Go: flag.go FlagSet.Int
    // Int defines an int flag with specified name, default value, and usage string.
    // The return value is the address of an int variable that stores the value of the flag.
    pub fn int(&mut self, name: &str, value: i64, usage: &str) -> Rc<Cell<i64>> {
        let p = Rc::new(Cell::new(value));
        self.var(FlagValue::Int(p.clone()), name, usage);
        p
    }

    // Go: flag.go FlagSet.String
    // String defines a string flag with specified name, default value, and usage string.
    // The return value is the address of a string variable that stores the value of the flag.
    pub fn string(&mut self, name: &str, value: &str, usage: &str) -> Rc<RefCell<String>> {
        let p = Rc::new(RefCell::new(value.to_string()));
        self.var(FlagValue::String(p.clone()), name, usage);
        p
    }

    // Go: flag.go:1010 FlagSet.Var
    pub fn var(&mut self, value: FlagValue, name: &str, usage: &str) {
        // Flag must not begin "-" or contain "=".
        if name.starts_with('-') {
            panic!(
                "{}",
                self.sprintf(format!(
                    "flag {} begins with -",
                    gostd::strconv::quote(name)
                ))
            );
        } else if name.contains('=') {
            panic!(
                "{}",
                self.sprintf(format!("flag {} contains =", gostd::strconv::quote(name)))
            );
        }

        // Remember the default value as a string; it won't change.
        let def_value = value.string();
        let flag = Flag {
            name: name.to_string(),
            usage: usage.to_string(),
            value,
            def_value,
        };
        if self.formal.contains_key(name) {
            let msg = if self.name.is_empty() {
                self.sprintf(format!("flag redefined: {name}"))
            } else {
                self.sprintf(format!("{} flag redefined: {name}", self.name))
            };
            panic!("{msg}"); // Happens only if flags are declared with identical names
        }
        self.formal.insert(name.to_string(), flag);
    }

    // Go: flag.go:1050 FlagSet.sprintf
    // sprintf formats the message, prints it to output, and returns it.
    fn sprintf(&self, msg: String) -> String {
        output(&format!("{msg}\n"));
        msg
    }

    // Go: flag.go:1058 FlagSet.failf
    // failf prints to standard error a formatted error and usage message and
    // returns the error.
    fn failf(&self, msg: String) -> GoError {
        let msg = self.sprintf(msg);
        self.usage();
        errors::new(msg)
    }

    // Go: flag.go:1066 FlagSet.usage
    // usage calls the Usage method for the flag set if one is specified,
    // or the appropriate default usage function otherwise.
    fn usage(&self) {
        self.default_usage();
    }

    // Go: flag.go:684 FlagSet.defaultUsage
    // defaultUsage is the default function to print a usage message.
    fn default_usage(&self) {
        if self.name.is_empty() {
            output("Usage:\n");
        } else {
            output(&format!("Usage of {}:\n", self.name));
        }
        self.print_defaults();
    }

    // Go: flag.go:607 FlagSet.PrintDefaults
    // PrintDefaults prints, to standard error unless configured otherwise, the
    // default values of all defined command-line flags in the set.
    // PORT: `isZeroValue` cannot fail for the three value kinds here.
    pub fn print_defaults(&self) {
        // Go: VisitAll visits the flags in lexicographical order.
        let mut flags: Vec<&Flag> = self.formal.values().collect();
        // Go: flag/flag.go:423 sortFlags: slices.SortFunc(result, strings.Compare on the names)
        crate::gostd::slices::sort_func(&mut flags, |a, b| a.name.cmp(&b.name) as i32);
        for flag in flags {
            let mut b = String::new();
            b.push_str(&format!("  -{}", flag.name)); // Two spaces before -; see next two comments.
            let (name, usage) = unquote_usage(flag);
            if !name.is_empty() {
                b.push(' ');
                b.push_str(&name);
            }
            // Boolean flags of one ASCII letter are so common we
            // treat them specially, putting their usage on the same line.
            if b.len() <= 4 {
                // space, space, '-', 'x'.
                b.push('\t');
            } else {
                // Four spaces before the tab triggers good alignment
                // for both 4- and 8-space tab stops.
                b.push_str("\n    \t");
            }
            b.push_str(&usage.replace('\n', "\n    \t"));

            // Print the default value only if it differs to the zero value
            // for this flag type.
            if !is_zero_value(flag, &flag.def_value) {
                if let FlagValue::String(_) = flag.value {
                    // put quotes on the value
                    b.push_str(&format!(
                        " (default {})",
                        gostd::strconv::quote(&flag.def_value)
                    ));
                } else {
                    b.push_str(&format!(" (default {})", flag.def_value));
                }
            }
            output(&format!("{b}\n"));
        }
    }

    // Go: flag.go:1075 FlagSet.parseOne
    // parseOne parses one flag. It reports whether a flag was seen.
    // PORT: Go `(bool, error)`; an error always comes with `false`.
    fn parse_one(&mut self) -> Result<bool, GoError> {
        if self.args.is_empty() {
            return Ok(false);
        }
        let s = self.args[0].clone();
        if s.len() < 2 || s.as_bytes()[0] != b'-' {
            return Ok(false);
        }
        let mut num_minuses = 1;
        if s.as_bytes()[1] == b'-' {
            num_minuses += 1;
            if s.len() == 2 {
                // "--" terminates the flags
                self.args.remove(0);
                return Ok(false);
            }
        }
        let mut name = &s[num_minuses..];
        if name.is_empty() || name.as_bytes()[0] == b'-' || name.as_bytes()[0] == b'=' {
            return Err(self.failf(format!("bad flag syntax: {s}")));
        }

        // it's a flag. does it have an argument?
        self.args.remove(0);
        let mut has_value = false;
        let mut value = String::new();
        for i in 1..name.len() {
            // equals cannot be first
            if name.as_bytes()[i] == b'=' {
                value = name[i + 1..].to_string();
                has_value = true;
                name = &name[..i];
                break;
            }
        }

        let Some(flag_value) = self.formal.get(name).map(|flag| flag.value.clone()) else {
            if name == "help" || name == "h" {
                // special case for nice help message.
                self.usage();
                return Err(ERR_HELP.clone());
            }
            return Err(self.failf(format!("flag provided but not defined: -{name}")));
        };

        if flag_value.is_bool_flag() {
            // special case: doesn't need an arg
            if has_value {
                if let Err(err) = flag_value.set(&value) {
                    return Err(self.failf(format!(
                        "invalid boolean value {} for -{name}: {}",
                        gostd::strconv::quote(&value),
                        err.error()
                    )));
                }
            } else if let Err(err) = flag_value.set("true") {
                return Err(self.failf(format!("invalid boolean flag {name}: {}", err.error())));
            }
        } else {
            // It must have a value, which might be the next argument.
            if !has_value && !self.args.is_empty() {
                // value is the next arg
                has_value = true;
                value = self.args.remove(0);
            }
            if !has_value {
                return Err(self.failf(format!("flag needs an argument: -{name}")));
            }
            if let Err(err) = flag_value.set(&value) {
                return Err(self.failf(format!(
                    "invalid value {} for flag -{name}: {}",
                    gostd::strconv::quote(&value),
                    err.error()
                )));
            }
        }
        self.actual.insert(name.to_string());
        Ok(true)
    }

    // Go: flag.go:1153 FlagSet.Parse
    // Parse parses flag definitions from the argument list, which should not
    // include the command name. Must be called after all flags in the [FlagSet]
    // are defined and before flags are accessed by the program.
    // The return value will be [ErrHelp] if -help or -h were set but not defined.
    pub fn parse(&mut self, arguments: &[String]) -> Result<(), GoError> {
        self.parsed = true;
        self.args = arguments.to_vec();
        loop {
            match self.parse_one() {
                Ok(true) => continue,
                Ok(false) => break,
                Err(err) => match self.error_handling {
                    ErrorHandling::ContinueOnError => return Err(err),
                    ErrorHandling::ExitOnError => {
                        if err == *ERR_HELP {
                            std::process::exit(0);
                        }
                        std::process::exit(2);
                    }
                    ErrorHandling::PanicOnError => panic!("{}", err.error()),
                },
            }
        }
        Ok(())
    }
}

// Go: flag.go:568 UnquoteUsage
// UnquoteUsage extracts a back-quoted name from the usage
// string for a flag and returns it and the un-quoted usage.
// Given "a `name` to show" it returns ("name", "a name to show").
// If there are no back quotes, the name is an educated guess of the
// type of the flag's value, or the empty string if the flag is boolean.
pub fn unquote_usage(flag: &Flag) -> (String, String) {
    // Look for a back-quoted name, but avoid the strings package.
    let usage = flag.usage.as_bytes();
    for i in 0..usage.len() {
        if usage[i] == b'`' {
            for j in i + 1..usage.len() {
                if usage[j] == b'`' {
                    let name = flag.usage[i + 1..j].to_string();
                    let usage = format!("{}{}{}", &flag.usage[..i], name, &flag.usage[j + 1..]);
                    return (name, usage);
                }
            }
            break; // Only one back quote; use type name.
        }
    }
    // No explicit name, so use type if we can find one.
    let name = match flag.value {
        FlagValue::Bool(_) => "",
        FlagValue::Int(_) => "int",
        FlagValue::String(_) => "string",
    };
    (name.to_string(), flag.usage.clone())
}

// Go: flag.go:538 isZeroValue
// isZeroValue determines whether the string represents the zero
// value for a flag.
// PORT: the zero values print as "false" (bool), "0" (int) and "" (string).
pub fn is_zero_value(flag: &Flag, value: &str) -> bool {
    let zero = match flag.value {
        FlagValue::Bool(_) => "false",
        FlagValue::Int(_) => "0",
        FlagValue::String(_) => "",
    };
    value == zero
}
