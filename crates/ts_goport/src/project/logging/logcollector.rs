//! Go `internal/project/logging/logcollector.go`.

use crate::project::logging::prelude::*;
use std::cell::Cell;
use std::time::{Duration, UNIX_EPOCH};

// Go: project/logging/logcollector.go:9 LogCollector
// PORT: Go `fmt.Stringer` is the `string` method.
pub trait LogCollector: Logger {
    fn string(&self) -> String;
}

// Go: project/logging/logcollector.go:14 logCollector
// PORT: Go unexported type behind the `LogCollector` interface, so the Rust
// name is `LogCollectorImpl`. The embedded Go `logger` is the `logger`
// field; its promoted methods are the `Logger` impl below.
pub struct LogCollectorImpl {
    pub logger: LoggerImpl,
    pub builder: Rc<RefCell<String>>,
}

// Go: project/logging/logcollector.go:19 String
impl LogCollector for LogCollectorImpl {
    fn string(&self) -> String {
        self.builder.borrow().clone()
    }
}

// PORT: Go promotes the embedded `logger` methods. `Verbose` returns the
// embedded logger, as in Go.
impl Logger for LogCollectorImpl {
    fn error(&self, msg: &str) {
        self.logger.error(msg);
    }

    fn errorf(&self, msg: &str) {
        self.logger.errorf(msg);
    }

    fn warn(&self, msg: &str) {
        self.logger.warn(msg);
    }

    fn warnf(&self, msg: &str) {
        self.logger.warnf(msg);
    }

    fn info(&self, msg: &str) {
        self.logger.info(msg);
    }

    fn infof(&self, msg: &str) {
        self.logger.infof(msg);
    }

    fn log(&self, msg: &str) {
        self.logger.log(msg);
    }

    fn logf(&self, msg: &str) {
        self.logger.logf(msg);
    }

    fn verbose(&self) -> Option<&dyn Logger> {
        self.logger.verbose()
    }

    fn is_verbose(&self) -> bool {
        self.logger.is_verbose()
    }

    fn set_verbose(&self, verbose: bool) {
        self.logger.set_verbose(verbose);
    }
}

// PORT: Go uses a `*strings.Builder` as the logger's `io.Writer`. This
// writer appends to the shared builder. Log text is UTF-8, and `write!`
// passes whole string pieces, so no character is split.
struct StringBuilderWriter(Rc<RefCell<String>>);

impl std::io::Write for StringBuilderWriter {
    // One `write_all` of the whole text per `write!`, as Go's `fmt.Fprintf`
    // makes one write (std's default writes each format piece; the bytes are
    // the same).
    fn write_fmt(&mut self, args: std::fmt::Arguments<'_>) -> std::io::Result<()> {
        self.write_all(std::fmt::format(args).as_bytes())
    }

    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.borrow_mut().push_str(&String::from_utf8_lossy(buf));
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

// Go: project/logging/logcollector.go:23 NewTestLogger
pub fn new_test_logger() -> Rc<dyn LogCollector> {
    let builder = Rc::new(RefCell::new(String::new()));
    Rc::new(LogCollectorImpl {
        logger: LoggerImpl {
            verbose: Cell::new(false),
            writer: RefCell::new(Box::new(StringBuilderWriter(builder.clone()))),
            prefix: Box::new(|| format_time(UNIX_EPOCH + Duration::from_secs(1_349_085_672))),
        },
        builder,
    })
}
