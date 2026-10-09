//! Go `cmd/tsc/api.go`.

use crate::cmd::tsgo::prelude::*;

use crate::cmd::tsgo::main::{ErrorHandling, must_getwd, new_flag_set, notify_context};
use crate::contentmapper::{self, ProcessExitState};
use crate::execute::tsc::stdio;
use crate::frontend::bundled;
use crate::gostd::context;
use std::io::Write;
use std::sync::Arc;

// Go: cmd/tsc/api.go:19 apiFlags (tsgo#4712)
struct ApiFlags {
    cwd: String,
    pipe_path: String,
    callbacks: String,
    // ts#64447
    case_sensitive: bool,
    async_: bool,
    timing: bool,
    run_external_code: bool,
}

// Go: cmd/tsc/api.go:29 parseAPIFlags (tsgo#4712)
// PORT: Go `StringVar` and `BoolVar` write into `result` as the flags are
// parsed. The port's flag set has only `String` and `Bool`, so the values
// are read into the result after the parse. The flag order and texts are
// Go's.
fn parse_api_flags(args: &[String]) -> Result<ApiFlags, GoError> {
    let mut flags = new_flag_set("api", ErrorHandling::ContinueOnError);
    let cwd = flags.string("cwd", &must_getwd(), "current working directory");
    let pipe_path = flags.string(
        "pipe",
        "",
        "use named pipe or Unix domain socket for communication instead of stdio",
    );
    let callbacks = flags.string(
        "callbacks",
        "",
        "comma-separated list of FS callbacks and defaults to enable",
    );
    // ts#64447
    let case_sensitive = flags.bool(
        "useCaseSensitiveFileNames",
        crate::frontend::vfs::osvfs_fs().use_case_sensitive_file_names(),
        "treat filesystem paths as case-sensitive",
    );
    let async_ = flags.bool(
        "async",
        false,
        "use JSON-RPC protocol instead of MessagePack (for async API)",
    );
    let timing = flags.bool(
        "timing",
        false,
        "collect per-request server processing time, folded into the client's timing snapshot",
    );
    let run_external_code = flags.bool(
        "runExternalCode",
        false,
        "allow projects to execute configured external plugins",
    );
    flags.parse(args)?;
    Ok(ApiFlags {
        cwd: cwd.borrow().clone(),
        pipe_path: pipe_path.borrow().clone(),
        callbacks: callbacks.borrow().clone(),
        case_sensitive: case_sensitive.get(),
        async_: async_.get(),
        timing: timing.get(),
        run_external_code: run_external_code.get(),
    })
}

/// Go `newSystem()` as the `contentmapper.Spawner` of the API server (Go
/// passes the `*osSys`, whose `Spawn` method makes it a `Spawner`).
struct OsSystemSpawner(tsc::OsSystem);

impl contentmapper::Spawner for OsSystemSpawner {
    // Go: cmd/tsc/sys.go:68 osSys.Spawn
    fn spawn(
        &self,
        command: &[String],
        dir: &str,
        stderr: Option<Box<dyn std::io::Write + Send>>,
    ) -> Result<Arc<dyn ProcessExitState>, GoError> {
        tsc::System::spawn(&self.0, command, dir, stderr)
    }
}

// Go: cmd/tsc/api.go:45 runAPI
pub fn run_api(args: &[String]) -> i32 {
    let Ok(flags) = parse_api_flags(args) else {
        return 2;
    };

    let default_library_path = bundled::lib_path_exported();

    // Parse callbacks list
    let mut callbacks_list: Vec<String> = Vec::new();
    if !flags.callbacks.is_empty() {
        callbacks_list = flags.callbacks.split(',').map(str::to_string).collect();
    }

    // Go: ContentMapperSpawner: newSystem()
    // PORT: Go `newSystem` exits the process with
    // `ExitStatusInvalidProject_OutputsSkipped` when the current directory
    // cannot be read; `new_os_system` prints the same text and returns that
    // status, which is returned here as the exit code.
    // ts#64159: the system's current directory roots `--cwd` (Go N'
    // cmd/tsc/api.go:62).
    let (content_mapper_spawner, system_cwd): (Rc<dyn contentmapper::Spawner>, String) =
        match tsc::new_os_system() {
            Ok(sys) => {
                let cwd = tsc::System::get_current_directory(&sys);
                (Rc::new(OsSystemSpawner(sys)), cwd)
            }
            Err(status) => return status.code(),
        };

    // PORT: Go `In io.ReadCloser`, `Out io.WriteCloser` and `Err io.Writer`
    // are nil-able interfaces (`None`).
    let mut options = crate::api::StdioServerOptions {
        in_: None,
        out: None,
        err: Some(Box::new(stdio::Stderr)),
        // Go: tspath.ToRootedDirectoryPath(flags.cwd, system.cwd). An empty
        // `--cwd=` is a Go panic.
        cwd: crate::api::to_rooted_path(&flags.cwd, &system_cwd),
        default_library_path,
        pipe_path: String::new(),
        callbacks: callbacks_list,
        use_case_sensitive_file_names: Some(flags.case_sensitive),
        async_: flags.async_,
        collect_timing: flags.timing,
        run_external_code: flags.run_external_code,
        content_mapper_spawner: Some(content_mapper_spawner),
    };
    if !flags.pipe_path.is_empty() {
        options.pipe_path = flags.pipe_path;
    } else {
        // Go: os.Stdin and os.Stdout (see `execute::tsc::stdio`).
        options.in_ = Some(Box::new(stdio::Stdin));
        options.out = Some(Box::new(stdio::Stdout));
    }

    let mut s = crate::api::new_stdio_server(options);

    // Go: ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
    let (ctx, stop) = notify_context(&context::background());

    let result = s.run(&ctx);
    // Go: defer stop()
    stop();
    if let Err(err) = result {
        let _ = writeln!(stdio::Stderr, "{}", err.error());
        return 1;
    }
    0
}
