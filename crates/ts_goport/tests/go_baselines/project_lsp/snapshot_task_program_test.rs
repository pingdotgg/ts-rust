//! PORT: no Go counterpart (lane apimem1, csfree1 lane note 2). Go
//! `updateSnapshot` (project/session.go:1356-1417) queues one task for each
//! snapshot change. The task has Go pointers to the old and the new snapshot
//! and takes no snapshot ref. When the next change disposes the new snapshot
//! before the task runs, the task still reads the programs of that snapshot
//! (`publishProgramDiagnostics`, project/session.go:1863, and
//! `GetProjectDiagnostics`, project/project.go:368), because the GC keeps
//! them. Here the task holds each program of its new snapshot registered
//! until it ends (`ls_program::hold_program`). Go runs each task in its own
//! goroutine, so the publishes have no fixed order there; here they come in
//! enqueue order.

use std::rc::Rc;

use ts_goport::program::ls_program;
use ts_goport::project::Session;

use super::projecttestutil::{self, SessionUtils, files};
use super::util::{edit, open, program};

const INDEX_URI: &str = "file:///home/projects/TS/p1/index.ts";
const INDEX_TEXT: &str = "import { a } from './a';\nexport const x = a + 1;";
/// The config file of the project. ts#64159: Go N' publishes the config
/// diagnostics under the config file name (project/session.go:1978,
/// `publishProjectDiagnostics(ctx, project.ConfigFileName(), ...)`), not its
/// path, which is in lower case on this case-insensitive test file system.
const CONFIG_URI: &str = "file:///home/projects/TS/p1/tsconfig.json";

/// A session with push diagnostics and index.ts open, after two program
/// changes and no wait for the background tasks: the program of index.ts,
/// then a new load (a new import), which disposes the first snapshot and
/// releases its program while the task of the first change is queued.
fn two_program_changes() -> (Rc<Session>, SessionUtils) {
    let (session, utils) = projecttestutil::setup(files(&[
        ("/home/projects/TS/p1/tsconfig.json", "{}"),
        ("/home/projects/TS/p1/index.ts", INDEX_TEXT),
        ("/home/projects/TS/p1/a.ts", "export const a = 1;"),
        ("/home/projects/TS/p1/b.ts", "export const b = 1;"),
    ]));
    open(&session, INDEX_URI, INDEX_TEXT);
    let _ = program(&session, INDEX_URI);
    edit(
        &session,
        INDEX_URI,
        2,
        (0, 0),
        (0, 0),
        "import { b } from './b';\n",
    );
    let _ = program(&session, INDEX_URI);
    (session, utils)
}

child_test! {
    // Before the fix, the first task panicked here: "program was not made by
    // ls_program::new_program, or it is released".
    fn task_reads_the_program_of_a_disposed_snapshot() {
        let (session, utils) = two_program_changes();
        let held = ls_program::registered_programs();
        session.wait_for_background_tasks();
        assert_eq!(
            ls_program::registered_programs(),
            held - 1,
            "the first task's hold released the first program"
        );
        let publishes: Vec<String> = utils
            .client()
            .publish_diagnostics_calls()
            .iter()
            .map(|call| call.uri.0.clone())
            .collect();
        assert_eq!(
            publishes,
            [CONFIG_URI, CONFIG_URI],
            "each change's task publishes the project"
        );
        session.close();
    }
}

child_test! {
    // A task that never runs drops with the queues of its thread when the
    // thread ends; its holds release nothing then.
    fn holds_of_a_task_that_never_runs_drop_at_the_thread_end() {
        let thread = std::thread::Builder::new()
            .stack_size(64 << 20)
            .spawn(|| {
                projecttestutil::install_fs_override();
                let (session, _utils) = two_program_changes();
                drop(session);
            })
            .expect("spawn");
        assert!(thread.join().is_ok());
    }
}
