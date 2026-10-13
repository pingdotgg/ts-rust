//! `runTsc`: the tsc command line in this process, as `tsgo` runs it
//! (`bin/tsgo.rs` `run_main` and `finish`), on a thread with the Go stack
//! size, with the OS file system, stdout, stderr and the bundled libs.

use std::io::Write;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;

use ts_goport::execute::execute_tsc::{GoTsc, command_line};
use ts_goport::execute::tsc::{EXIT_UNPORTED, System, new_os_system};
use ts_goport::gostd::context;
use ts_goport::prelude::{EXIT_GO_PANIC, print_go_panic, record_unported, unported_report};
use ts_goport::program::ThreadBudget;
use ts_goport::scanner_util::go_string_from_utf8;

use crate::hooks::{UNPORTED_PREFIX, payload_message};

/// Runs tsc with `args` (the arguments after `tsc`) and returns its exit
/// status. The JS thread waits for it.
pub fn run(args: Vec<String>) -> Result<i32, String> {
    let thread = std::thread::Builder::new()
        .name("tsc-rs".to_string())
        .stack_size(ts_goport::gostd::stack::max_stack_size())
        .spawn(move || run_main(args))
        .map_err(|err| format!("cannot start the tsc-rs thread: {err}"))?;
    thread.join().map_err(|payload| {
        format!(
            "the tsc-rs thread failed: {}",
            payload_message(payload.as_ref())
        )
    })
}

/// `bin/tsgo.rs` `run_main` without its signal context: Node keeps its own
/// signal handlers.
fn run_main(args: Vec<String>) -> i32 {
    ThreadBudget::one_program(1).install();
    ts_goport::execute::tsc::stdio::init();
    let mut args: Vec<String> = args.into_iter().map(go_string_from_utf8).collect();
    ts_goport::fast_mode::take_flag(&mut args);
    let sys = match new_os_system() {
        Ok(sys) => sys,
        Err(status) => return status.code(),
    };
    let sys: Rc<dyn System> = Rc::new(sys);
    let result = catch_unwind(AssertUnwindSafe(|| {
        command_line(&context::background(), sys.clone(), &args, &GoTsc)
            .status
            .code()
    }));
    let _ = sys.writer().borrow_mut().flush();
    ts_goport::execute::tsc::stdio::flush_cli_stdout_at_exit();
    finish(result)
}

/// `bin/tsgo.rs` `finish`: prints a Go panic and the unported counts and
/// returns the exit status.
fn finish(result: std::thread::Result<i32>) -> i32 {
    let code = match result {
        Ok(code) => code,
        Err(payload) if print_go_panic(payload.as_ref()) => EXIT_GO_PANIC,
        Err(payload) => {
            if !payload_message(payload.as_ref()).starts_with(UNPORTED_PREFIX) {
                record_unported("panic");
            }
            EXIT_UNPORTED
        }
    };
    let unported = unported_report();
    let mut stderr = std::io::stderr().lock();
    for (name, count) in &unported {
        let _ = writeln!(stderr, "unported: {name} {count}");
    }
    if unported.is_empty() {
        code
    } else {
        EXIT_UNPORTED
    }
}
