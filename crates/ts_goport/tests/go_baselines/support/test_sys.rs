//! Go: internal/execute/tsctests/sys.go and fs.go: the test system of the
//! tsc runner (`TestSys`, `TestClock`, `testFs`), its program baselines,
//! its output sanitizer and the edit helpers of the scenarios.
//!
//! PORT: a plain `tsc` run has one program per process, so the runner
//! runs every `execute.CommandLine` in a child process (see child.rs). The
//! test system of the runner process keeps the state and never compiles.
//! The child process rebuilds a test system over the same state, compiles,
//! and sends the state back. The state that every view shares (the map
//! file system, the clock, the written files and the default libraries) is
//! `Send + Sync`, because emit writes arrive on checker threads, which make
//! their own `testFs` view through the osvfs override.
//!
//! PORT: strings are the port form of Go strings (see
//! `ts_goport::scanner_util::GO_STRING_MARKER`). The map file system keeps
//! Go bytes. Baselines are compared as Go bytes (support/baseline.rs).
//!
//! PORT: a watch command keeps its child process across the edits (see
//! child.rs). The child's `TestSys` holds the `MockWatchBackend`; the
//! runner's is not used.

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use std::time::{Duration, SystemTime};

use rustc_hash::{FxHashMap, FxHashSet};
use ts_goport::api::to_rooted_path;
use ts_goport::contentmapper::ProcessExitState;
use ts_goport::core::version;
use ts_goport::diag;
use ts_goport::diagnostics::Message;
use ts_goport::diagnostics_loc::message_localize;
use ts_goport::emitter::program_emit::EmitResult;
use ts_goport::execute::incremental::build_info::BuildInfo;
use ts_goport::execute::incremental::incremental::marshal_build_info;
use ts_goport::execute::incremental::program::{Program, SignatureUpdateKind};
use ts_goport::execute::tsc::compile::{
    CommandLineTesting, ErrorWriter, System, Writer, write_str,
};
use ts_goport::frontend::compiler::TraceFn;
use ts_goport::frontend::json::json_unmarshal;
use ts_goport::frontend::tsoptions::{
    CompilerOptionsValue, LIB_FILES_SET, LIB_MAP, target_to_lib_map,
};
use ts_goport::frontend::tspath::{
    ComparePathsOptions, EXTENSION_TS_BUILD_INFO, Path, file_extension_is,
    relative_path_from_directory, to_path,
};
use ts_goport::frontend::vfs::{Entries, FileInfo, Fs, FsError};
use ts_goport::gostd::GoError;
use ts_goport::locale::{self, Locale};
use ts_goport::scanner_util::{go_string_bytes, go_string_from_bytes};

use crate::support::child;
use crate::support::contentmappertest;
use crate::support::fsbaselineutil::{FsDiffer, sanitize_internal_symbol_name};
use crate::support::harnessutil::{FAKE_TS_VERSION, TracerForBaselining};
use crate::support::mock_watch_backend::MockWatchBackend;
use crate::support::readable_build_info::to_readable_build_info;
use crate::support::runner::{FileMap, TscInput};
use crate::support::stringtestutil::dedent;
use crate::support::vfstest::{Clock, MapFs};

// Go: sys.go:36 tscLibPath
pub const TSC_LIB_PATH: &str = "/home/src/tslibs/TS/Lib";

// Go: sys.go:38 tscDefaultLibContent
pub static TSC_DEFAULT_LIB_CONTENT: LazyLock<String> = LazyLock::new(|| {
    dedent(
        r#"
/// <reference no-default-lib="true"/>
interface Boolean {}
interface Function {}
interface CallableFunction {}
interface NewableFunction {}
interface IArguments {}
interface Number { toExponential: any; }
interface Object {}
interface RegExp {}
interface String { charAt: any; }
interface Array<T> { length: number; [n: number]: T; }
interface ReadonlyArray<T> {}
interface SymbolConstructor {
    (desc?: string | number): symbol;
    for(name: string): symbol;
    readonly toStringTag: symbol;
}
declare var Symbol: SymbolConstructor;
interface Symbol {
    readonly [Symbol.toStringTag]: string;
}
declare const console: { log(msg: any): void; };
"#,
    )
});

/// Go `tscDefaultLibContent` as a `String`.
pub fn tsc_default_lib_content() -> String {
    TSC_DEFAULT_LIB_CONTENT.clone()
}

// Go: sys.go:63 getTestLibPathFor
pub fn get_test_lib_path_for(lib_name: &str) -> String {
    let lib_file = match LIB_MAP.get(lib_name) {
        Some(CompilerOptionsValue::String(value)) => value.clone(),
        // Go `value.(string)` panics on another type.
        Some(_) => panic!("interface conversion: LibMap value is not a string"),
        None => format!("lib.{lib_name}.d.ts"),
    };
    format!("{TSC_LIB_PATH}/{lib_file}")
}

/// Locks a shared value. A poisoned lock (a panicking view on another
/// thread) still gives the value, as Go has no lock poisoning.
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Go `TestSys.currentWrite` (`*strings.Builder`): the output buffer of
/// both `Writer()` and `ErrorWriter()`.
// PORT: `ErrorWriter` is `Send` (the content mapper logger writes to it
// from a mapper's stderr thread), so the buffer is behind a `Mutex`, and
// `Writer` is an `Rc<RefCell<CurrentWrite>>` over the same buffer.
#[derive(Clone, Default)]
struct CurrentWrite(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for CurrentWrite {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        lock(&self.0).extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Go `err.Error()` of a vfs error.
// PORT: copied from `ts_goport::execute::incremental::emit_files`
// `fs_error_text`, which is private to the crate.
pub fn fs_error_text(err: &FsError) -> String {
    match err {
        FsError::Path { op, path, err } => format!("{op} {path}: {err}"),
        FsError::Other(message) => message.clone(),
        other => format!("{other:?}"),
    }
}

// ---------------------------------------------------------------------------
// TestClock
// ---------------------------------------------------------------------------

/// The plain state of a `TestClock`, for the child protocol.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClockState {
    pub start: SystemTime,
    /// `None` is the Go zero time.
    pub now: Option<SystemTime>,
}

// Go: sys.go:73 TestClock
pub struct TestClock {
    state: Mutex<ClockState>,
}

impl TestClock {
    pub fn new(start: SystemTime) -> TestClock {
        TestClock {
            state: Mutex::new(ClockState { start, now: None }),
        }
    }

    pub fn state(&self) -> ClockState {
        *lock(&self.state)
    }

    pub fn set_state(&self, state: ClockState) {
        *lock(&self.state) = state;
    }
}

impl Clock for TestClock {
    // Go: sys.go:79 Now
    fn now(&self) -> SystemTime {
        let mut state = lock(&self.state);
        let now = state.now.unwrap_or(state.start);
        // Simulate some time passing
        let now = now + Duration::from_secs(1);
        state.now = Some(now);
        now
    }

    // Go: sys.go:89 SinceStart
    fn since_start(&self) -> Duration {
        let start = lock(&self.state).start;
        self.now().duration_since(start).unwrap_or_default()
    }
}

// ---------------------------------------------------------------------------
// testFs
// ---------------------------------------------------------------------------

/// The state that every view of one test system shares. It is `Send +
/// Sync`: each thread of a compile makes its own `TestFs` over it.
// PORT: Go shares the `*testFs` and its `*MapFS` between goroutines.
#[derive(Clone)]
pub struct SharedFs {
    pub map_fs: MapFs,
    pub clock: Arc<TestClock>,
    /// Go `testFs.defaultLibs` (`nil` is `None`).
    pub default_libs: Arc<Mutex<Option<FxHashSet<String>>>>,
    /// Go `testFs.writtenFiles`.
    pub written_files: Arc<Mutex<FxHashSet<String>>>,
}

impl SharedFs {
    /// A `testFs` view for the calling thread.
    pub fn test_fs(&self) -> Rc<TestFs> {
        Rc::new(TestFs {
            fs: self.map_fs.fs(),
            default_libs: self.default_libs.clone(),
            written_files: self.written_files.clone(),
        })
    }
}

// Go: fs.go:16 testFs
// PORT: Go embeds the `vfs.FS` of the map (`fs`) and overrides `ReadFile`,
// `WriteFile` and `Remove`; the other methods pass through.
pub struct TestFs {
    fs: Rc<dyn Fs>,
    default_libs: Arc<Mutex<Option<FxHashSet<String>>>>,
    written_files: Arc<Mutex<FxHashSet<String>>>,
}

impl TestFs {
    // Go: fs.go:22 removeIgnoreLibPath
    fn remove_ignore_lib_path(&self, path: &str) {
        if let Some(default_libs) = lock(&self.default_libs).as_mut() {
            default_libs.remove(path);
        }
    }

    // Go: fs.go:35 readFileHandlingBuildInfo
    fn read_file_handling_build_info(&self, path: &str) -> (String, bool) {
        let (mut contents, ok) = self.fs.read_file(path);
        if ok && file_extension_is(path, EXTENSION_TS_BUILD_INFO) {
            // read buildinfo and modify version
            let mut build_info = BuildInfo::default();
            let parsed = json_unmarshal(&go_string_bytes(&contents), &mut build_info, &[]);
            if parsed.is_ok() && build_info.version == FAKE_TS_VERSION {
                build_info.version = version().to_string();
                match marshal_build_info(&build_info) {
                    Ok(new_contents) => contents = new_contents,
                    Err(err) => panic!(
                        "testFs.ReadFile: failed to marshal build info after fixing version: {}",
                        err.message
                    ),
                }
            }
        }
        (contents, ok)
    }

    // Go: fs.go:59 writeFileHandlingBuildInfo
    fn write_file_handling_build_info(&self, path: &str, data: &str) -> Result<(), FsError> {
        let mut data = Cow::Borrowed(data);
        if file_extension_is(path, EXTENSION_TS_BUILD_INFO) {
            let mut build_info = BuildInfo::default();
            match json_unmarshal(&go_string_bytes(&data), &mut build_info, &[]) {
                Ok(()) => {
                    if build_info.version == version() {
                        // Change it to harnessutil.FakeTSVersion
                        build_info.version = FAKE_TS_VERSION.to_string();
                        match marshal_build_info(&build_info) {
                            Ok(new_data) => data = Cow::Owned(new_data),
                            Err(err) => {
                                return Err(FsError::Other(format!(
                                    "testFs.WriteFile: failed to marshal build info after fixing version: {}",
                                    err.message
                                )));
                            }
                        }
                    }
                    // Write readable build info version
                    let readable =
                        to_readable_build_info(&build_info, &sanitize_internal_symbol_name(&data));
                    if let Err(err) =
                        self.write_file(&format!("{path}.readable.baseline.txt"), &readable)
                    {
                        return Err(FsError::Other(format!(
                            "testFs.WriteFile: failed to write readable build info: {}",
                            fs_error_text(&err)
                        )));
                    }
                }
                Err(err) => panic!(
                    "testFs.WriteFile: failed to unmarshal build info: - use underlying FS's write method if this is intended use for testcase{}",
                    err.message
                ),
            }
        }
        self.fs.write_file(path, &data)
    }
}

impl Fs for TestFs {
    fn use_case_sensitive_file_names(&self) -> bool {
        self.fs.use_case_sensitive_file_names()
    }

    fn file_exists(&self, path: &str) -> bool {
        self.fs.file_exists(path)
    }

    // Go: fs.go:30 ReadFile
    fn read_file(&self, path: &str) -> (String, bool) {
        self.remove_ignore_lib_path(path);
        self.read_file_handling_build_info(path)
    }

    // Go: fs.go:53 WriteFile
    fn write_file(&self, path: &str, data: &str) -> Result<(), FsError> {
        self.remove_ignore_lib_path(path);
        lock(&self.written_files).insert(path.to_string());
        self.write_file_handling_build_info(path, data)
    }

    fn append_file(&self, path: &str, data: &str) -> Result<(), FsError> {
        self.fs.append_file(path, data)
    }

    // Go: fs.go:87 Remove
    fn remove(&self, path: &str) -> Result<(), FsError> {
        self.remove_ignore_lib_path(path);
        self.fs.remove(path)
    }

    fn chtimes(
        &self,
        path: &str,
        a_time: Option<SystemTime>,
        m_time: Option<SystemTime>,
    ) -> Result<(), FsError> {
        self.fs.chtimes(path, a_time, m_time)
    }

    fn directory_exists(&self, path: &str) -> bool {
        self.fs.directory_exists(path)
    }

    fn get_accessible_entries(&self, path: &str) -> Entries {
        self.fs.get_accessible_entries(path)
    }

    fn stat(&self, path: &str) -> Option<FileInfo> {
        self.fs.stat(path)
    }

    fn realpath(&self, path: &str) -> String {
        self.fs.realpath(path)
    }
}

// ---------------------------------------------------------------------------
// TestSys
// ---------------------------------------------------------------------------

/// The program baseline of one `OnProgram` call, before the headers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProgramParts {
    /// Go `program.Options().ConfigFilePath`.
    pub config_file_path: String,
    /// The `SemanticDiagnostics::` and `Signatures::` text.
    pub body: String,
    /// The include reason text after its header; empty when every file
    /// has an include reason.
    pub include_body: String,
}

/// Which process a test system is in.
// PORT: the child protocol (child.rs). Go has one process.
pub enum SysMode {
    /// The runner process: it keeps the state and never compiles.
    Runner,
    /// A command child. It compiles, also every project of a `-b` build.
    Child,
}

// Go: sys.go:168 TestSys
pub struct TestSys {
    current_write: CurrentWrite,
    writer: Writer,
    program_baselines: RefCell<String>,
    program_include_baselines: RefCell<String>,
    tracer: Rc<RefCell<TracerForBaselining>>,
    fs_differ: RefCell<FsDiffer>,
    for_incremental_correctness: bool,
    mock_watch_backend: Rc<MockWatchBackend>,

    shared: SharedFs,
    fs: Rc<TestFs>,
    /// Go `fsDiffer.FS`: the map view without the `testFs` handling.
    fs_from_file_map: Rc<dyn Fs>,
    default_library_path: String,
    cwd: String,
    env: BTreeMap<String, String>,
    output_is_tty: bool,

    // PORT: the child protocol (child.rs).
    mode: SysMode,
    /// The modification times of the last FS baseline (Go
    /// `fsDiffer.SerializedDiff().Snap[file].MTime`) when this system
    /// runs in a child; the runner reads its own differ. Each watch cycle
    /// updates it.
    child_serialized_mtimes: SerializedMtimes,
}

/// The modification times of `TestSys::child_serialized_mtimes`: `None`
/// until a child sets them, then Go `fsDiffer.SerializedDiff()` (`None` is
/// Go nil).
pub type SerializedMtimes = Arc<Mutex<Option<Option<FxHashMap<String, Option<SystemTime>>>>>>;

// Go: sys.go:93 NewTscSystem
pub fn new_tsc_system(files: FileMap, use_case_sensitive_file_names: bool, cwd: &str) -> TestSys {
    let clock = Arc::new(TestClock::new(SystemTime::now()));
    let map_fs = MapFs::from_map_with_clock(files, use_case_sensitive_file_names, clock.clone());
    TestSys::new(
        SharedFs {
            map_fs,
            clock,
            default_libs: Arc::new(Mutex::new(None)),
            written_files: Arc::new(Mutex::new(FxHashSet::default())),
        },
        cwd.to_string(),
        String::new(),
        BTreeMap::new(),
        false,
        SysMode::Runner,
    )
}

// Go: sys.go:105 GetFileMapWithBuild
// PORT: the command runs in a child process (child.rs). Go changes the
// caller's map in place and returns it; this takes it by value.
pub fn get_file_map_with_build(mut files: FileMap, command_line_args: &[String]) -> FileMap {
    let sys = new_test_sys(
        &TscInput {
            files: files.clone(),
            ..TscInput::default()
        },
        false,
    );
    if let Err(message) = child::run_command_in_child(&sys, command_line_args) {
        panic!("GetFileMapWithBuild: {message}");
    }
    let written: Vec<String> = lock(&sys.shared.written_files).iter().cloned().collect();
    for key in written {
        let (text, ok) = sys.fs_from_file_map().read_file(&key);
        if ok {
            files.insert(key, text.into());
        }
    }
    files
}

// Go: sys.go:119 newTestSys
pub fn new_test_sys(tsc_input: &TscInput, for_incremental_correctness: bool) -> TestSys {
    let cwd = if tsc_input.cwd.is_empty() {
        "/home/src/workspaces/project"
    } else {
        tsc_input.cwd.as_str()
    };
    let mut lib_path = TSC_LIB_PATH.to_string();
    if !tsc_input.windows_style_root.is_empty() {
        lib_path = format!("{}{}", tsc_input.windows_style_root, &lib_path[1..]);
    }
    let mut sys = new_tsc_system(tsc_input.files.clone(), !tsc_input.ignore_case, cwd);
    sys.default_library_path = lib_path;
    sys.env = tsc_input.env.clone();
    if let Some(output_is_tty) = tsc_input.output_is_tty {
        sys.output_is_tty = output_is_tty;
    }
    sys.for_incremental_correctness = for_incremental_correctness;
    // PORT: `TestSys::new` makes the `mockWatchBackend`.

    // Ensure the default library file is present
    sys.ensure_lib_path_exists("lib.d.ts");
    for lib_file in target_to_lib_map().values() {
        sys.ensure_lib_path_exists(lib_file);
    }
    for lib_file in LIB_FILES_SET.iter() {
        sys.ensure_lib_path_exists(lib_file);
    }
    sys
}

/// Go `vfs.FS.UseCaseSensitiveFileNames` of the `testFs`, and the tracer
/// options of `newTestSys`.
fn compare_paths_options(use_case_sensitive_file_names: bool, cwd: &str) -> ComparePathsOptions {
    ComparePathsOptions {
        use_case_sensitive_file_names,
        current_directory: cwd.to_string(),
    }
}

impl TestSys {
    /// The Go struct literal of `NewTscSystem` and `newTestSys`, and the
    /// system that a child rebuilds (child.rs). Go `NewTscSystem` sets
    /// `outputIsTTY: true` (ts#63941); `set_output_is_tty` changes it.
    pub fn new(
        shared: SharedFs,
        cwd: String,
        default_library_path: String,
        env: BTreeMap<String, String>,
        for_incremental_correctness: bool,
        mode: SysMode,
    ) -> TestSys {
        let current_write = CurrentWrite::default();
        let writer: Writer = Rc::new(RefCell::new(current_write.clone()));
        let use_case_sensitive_file_names = shared.map_fs.use_case_sensitive_file_names();
        // PORT: the tracer gets no `builder_bytes`: the buffer is the
        // `Mutex` of `CurrentWrite`, and the tsc runner never reads the
        // tracer's `String()`.
        let tracer = TracerForBaselining::new(
            compare_paths_options(use_case_sensitive_file_names, &cwd),
            writer.clone(),
            None,
        );
        let fs_differ = FsDiffer::new(
            shared.map_fs.clone(),
            shared.default_libs.clone(),
            shared.written_files.clone(),
        );
        // Go: sys.go:136 `NewMockWatchBackend()` with `DirectoryExists` of
        // the map file system (`sys.fs.FS`), and (sys.go:138)
        // `UseCaseSensitiveFileNames = !tscInput.ignoreCase`, which is the
        // map file system's value.
        let mut mock_watch_backend = MockWatchBackend::new();
        let map_view = shared.map_fs.fs();
        mock_watch_backend.directory_exists =
            Some(Box::new(move |path: &str| map_view.directory_exists(path)));
        mock_watch_backend.use_case_sensitive_file_names = use_case_sensitive_file_names;
        TestSys {
            current_write,
            writer,
            program_baselines: RefCell::new(String::new()),
            program_include_baselines: RefCell::new(String::new()),
            tracer: Rc::new(RefCell::new(tracer)),
            fs_differ: RefCell::new(fs_differ),
            for_incremental_correctness,
            mock_watch_backend: Rc::new(mock_watch_backend),
            fs: shared.test_fs(),
            fs_from_file_map: shared.map_fs.fs(),
            shared,
            default_library_path,
            cwd,
            env,
            output_is_tty: true,
            mode,
            child_serialized_mtimes: Arc::new(Mutex::new(None)),
        }
    }

    // Go: sys.go:178 Now
    pub fn now(&self) -> SystemTime {
        self.shared.clock.now()
    }

    // Go: sys.go:182 SinceStart
    pub fn since_start(&self) -> Duration {
        self.shared.clock.since_start()
    }

    // Go: sys.go:198 FS
    pub fn fs(&self) -> Rc<dyn Fs> {
        self.fs.clone()
    }

    // Go: sys.go:202 fsFromFileMap
    pub fn fs_from_file_map(&self) -> Rc<dyn Fs> {
        self.fs_from_file_map.clone()
    }

    // Go: sys.go:206 mapFs
    pub fn map_fs(&self) -> &MapFs {
        &self.shared.map_fs
    }

    /// The shared state (child.rs).
    pub fn shared(&self) -> &SharedFs {
        &self.shared
    }

    pub fn mode(&self) -> &SysMode {
        &self.mode
    }

    pub fn env(&self) -> &BTreeMap<String, String> {
        &self.env
    }

    pub fn for_incremental_correctness(&self) -> bool {
        self.for_incremental_correctness
    }

    /// Go `sys.outputIsTTY = ...` (ts#63941): a child sets the value of
    /// the runner's system (child.rs).
    pub fn set_output_is_tty(&mut self, output_is_tty: bool) {
        self.output_is_tty = output_is_tty;
    }

    /// Go `sys.mockWatchBackend` (also Go `WatchBackend()`, sys.go:321).
    pub fn mock_watch_backend(&self) -> &Rc<MockWatchBackend> {
        &self.mock_watch_backend
    }

    // Go: sys.go:210 ensureLibPathExists
    fn ensure_lib_path_exists(&self, path: &str) {
        let path = format!("{}/{}", self.default_library_path, path);
        let (_, ok) = self.fs_from_file_map().read_file(&path);
        if !ok {
            lock(&self.shared.default_libs)
                .get_or_insert_with(FxHashSet::default)
                .insert(path.clone());
            if let Err(err) = self
                .fs_from_file_map()
                .write_file(&path, &TSC_DEFAULT_LIB_CONTENT)
            {
                panic!(
                    "Failed to write default library file: {}",
                    fs_error_text(&err)
                );
            }
        }
    }

    // Go: sys.go:334 writeHeaderToBaseline
    fn write_header_to_baseline(&self, builder: &mut String, config_file_path: &str) {
        if !builder.is_empty() {
            builder.push('\n');
        }

        if !config_file_path.is_empty() {
            // ts#64159 (sys.go:340): across roots there is no relative path
            // (R4), so the header is the absolute name.
            match relative_path_from_directory(
                &self.cwd,
                config_file_path,
                self.fs.use_case_sensitive_file_names(),
            ) {
                Some(relative_path) => builder.push_str(&relative_path),
                None => builder.push_str(config_file_path),
            }
            builder.push_str("::\n");
        }
    }

    /// The Go `OnProgram` text of `program` (sys.go:325), without the
    /// headers.
    fn program_parts(&self, program: &Program) -> ProgramParts {
        let testing_data = program
            .get_testing_data()
            .expect("OnProgram: the incremental program has no testing data");
        // PORT: Go `program.GetProgram()` is the current program (a watcher
        // makes its version current for the call).
        let go_program = ts_goport::program::go_frontend_program()
            .expect("OnProgram: the program has no Go frontend program");
        let source_files = go_program.get_source_files();

        let mut body = String::from("SemanticDiagnostics::\n");
        for file in source_files {
            let path = &file.parse_options.path;
            let file_name = &file.parse_options.file_name;
            if let Some(diagnostics) = program.semantic_diagnostics_id(path) {
                let old_diagnostics = testing_data.old_semantic_diagnostics_ids.get(path);
                if old_diagnostics != Some(&diagnostics) {
                    body.push_str("*refresh*    ");
                    body.push_str(file_name);
                    body.push('\n');
                }
            } else {
                body.push_str("*not cached* ");
                body.push_str(file_name);
                body.push('\n');
            }
        }

        // Write signature updates
        body.push_str("Signatures::\n");
        for file in source_files {
            let path = &file.parse_options.path;
            let file_name = &file.parse_options.file_name;
            if let Some(kind) = testing_data.updated_signature_kinds.get(path) {
                match kind {
                    SignatureUpdateKind::ComputedDts => body.push_str("(computed .d.ts) "),
                    SignatureUpdateKind::StoredAtEmit => body.push_str("(stored at emit) "),
                    SignatureUpdateKind::UsedVersion => body.push_str("(used version)   "),
                }
                body.push_str(file_name);
                body.push('\n');
            }
        }

        let mut files_without_include_reason: Vec<String> = Vec::new();
        let mut file_not_in_program_with_include_reason: Vec<String> = Vec::new();
        let include_reasons = go_program.get_include_reasons();
        for file in source_files {
            if !include_reasons.contains_key(&file.parse_options.path) {
                files_without_include_reason.push(file.parse_options.path.as_str().to_string());
            }
        }
        // PORT: Go ranges over a map (random order); this sorts.
        let mut include_reason_paths: Vec<&Path> = include_reasons.keys().collect();
        include_reason_paths.sort();
        for path in include_reason_paths {
            if go_program.get_source_file_by_path(path).is_none()
                && !go_program.is_missing_path(path)
            {
                file_not_in_program_with_include_reason.push(path.as_str().to_string());
            }
        }
        let mut include_body = String::new();
        if !files_without_include_reason.is_empty()
            || !file_not_in_program_with_include_reason.is_empty()
        {
            include_body.push_str(
                "!!! Expected all files to have include reasons\nfilesWithoutIncludeReason::\n",
            );
            for file in &files_without_include_reason {
                include_body.push_str("  ");
                include_body.push_str(file);
                include_body.push('\n');
            }
            include_body.push_str("filesNotInProgramWithIncludeReason::\n");
            for file in &file_not_in_program_with_include_reason {
                include_body.push_str("  ");
                include_body.push_str(file);
                include_body.push('\n');
            }
        }
        ProgramParts {
            config_file_path: program.options().config_file_path.clone(),
            body,
            include_body,
        }
    }

    /// Appends one `OnProgram` text with its headers (Go sys.go:325).
    pub fn append_program_parts(&self, parts: &ProgramParts) {
        {
            let mut program_baselines = self.program_baselines.borrow_mut();
            self.write_header_to_baseline(&mut program_baselines, &parts.config_file_path);
            program_baselines.push_str(&parts.body);
        }
        if !parts.include_body.is_empty() {
            let mut include_baselines = self.program_include_baselines.borrow_mut();
            self.write_header_to_baseline(&mut include_baselines, &parts.config_file_path);
            include_baselines.push_str(&parts.include_body);
        }
    }

    // Go: sys.go:423 baselinePrograms
    pub fn baseline_programs(&self, baseline: &mut String, header: &str) -> String {
        baseline.push_str(&self.program_baselines.borrow());
        self.program_baselines.borrow_mut().clear();
        let mut result = String::new();
        let mut include_baselines = self.program_include_baselines.borrow_mut();
        if !include_baselines.is_empty() {
            result.push_str(&format!(
                "\n\n{header}\n!!! Include reasons expectations don't match pls review!!!\n"
            ));
            result.push_str(&include_baselines);
            include_baselines.clear();
            baseline.push_str(&result);
        }
        result
    }

    // Go: sys.go:436 serializeState
    pub fn serialize_state(&self, baseline: &mut String) {
        self.baseline_output(baseline);
        self.baseline_fs_with_diff(baseline);
        // todo watch
        // this.serializeWatches(baseline);
        // this.timeoutCallbacks.serialize(baseline);
        // this.immediateCallbacks.serialize(baseline);
        // this.pendingInstalls.serialize(baseline);
        // this.service?.baseline();
    }

    // Go: sys.go:465 baselineOutput
    fn baseline_output(&self, baseline: &mut String) {
        baseline.push_str("\nOutput::\n");
        let output = self.get_output(false);
        baseline.push_str(&output);
    }

    /// The output buffer as text (Go `currentWrite.String()`).
    pub fn output_text(&self) -> String {
        String::from_utf8_lossy(&lock(&self.current_write.0)).into_owned()
    }

    /// The output buffer bytes (child protocol).
    pub fn output_bytes(&self) -> Vec<u8> {
        lock(&self.current_write.0).clone()
    }

    /// Replaces the output buffer (child protocol).
    pub fn set_output_bytes(&self, bytes: Vec<u8>) {
        *lock(&self.current_write.0) = bytes;
    }

    // Go: sys.go:556 getOutput
    pub fn get_output(&self, for_comparing: bool) -> String {
        let text = self.output_text();
        let lines: Vec<&str> = text.split('\n').collect();
        let mut transformer = OutputSanitizer {
            for_comparing,
            output_lines: Vec::with_capacity(lines.len()),
            lines,
            index: 0,
        };
        transformer.transform_lines()
    }

    // Go: sys.go:566 clearOutput
    pub fn clear_output(&self) {
        lock(&self.current_write.0).clear();
        self.tracer.borrow_mut().reset();
    }

    // Go: sys.go:571 baselineFSwithDiff
    pub fn baseline_fs_with_diff(&self, baseline: &mut String) {
        self.fs_differ.borrow_mut().baseline_fs_with_diff(baseline);
    }

    /// Go `sys.fsDiffer.ChangedPaths()` (watch mode).
    pub fn changed_paths(&self) -> Vec<crate::support::fsbaselineutil::FileChange> {
        self.fs_differ.borrow().changed_paths()
    }

    /// The program baselines so far (child protocol).
    pub fn program_baseline_texts(&self) -> (String, String) {
        (
            self.program_baselines.borrow().clone(),
            self.program_include_baselines.borrow().clone(),
        )
    }

    /// Replaces the program baselines (child protocol).
    pub fn set_program_baseline_texts(&self, program: String, include: String) {
        *self.program_baselines.borrow_mut() = program;
        *self.program_include_baselines.borrow_mut() = include;
    }

    /// The modification time of each entry of the last FS baseline, or
    /// `None` before the first one (Go `fsDiffer.SerializedDiff()`).
    pub fn serialized_mtimes(&self) -> Option<FxHashMap<String, Option<SystemTime>>> {
        if let Some(mtimes) = &*lock(&self.child_serialized_mtimes) {
            return mtimes.clone();
        }
        let differ = self.fs_differ.borrow();
        differ.serialized_diff().map(|snapshot| {
            snapshot
                .snap
                .iter()
                .map(|(path, entry)| (path.clone(), entry.m_time))
                .collect()
        })
    }

    /// Sets the modification times of the runner's last FS baseline in a
    /// child (see `serialized_mtimes`).
    pub fn set_child_serialized_mtimes(
        &self,
        mtimes: Option<FxHashMap<String, Option<SystemTime>>>,
    ) {
        *lock(&self.child_serialized_mtimes) = Some(mtimes);
    }

    // Go: sys.go:575 writeFileNoError
    // ts#64159: the path is rooted against the current directory (sys.go:576,
    // :582, :588).
    pub fn write_file_no_error(&self, path: &str, content: &str) {
        let path = to_rooted_path(path, &self.get_current_directory());
        if let Err(err) = self.fs_from_file_map().write_file(&path, content) {
            panic!("{}", fs_error_text(&err));
        }
    }

    // Go: sys.go:581 removeNoError
    pub fn remove_no_error(&self, path: &str) {
        let path = to_rooted_path(path, &self.get_current_directory());
        if let Err(err) = self.fs_from_file_map().remove(&path) {
            panic!("{}", fs_error_text(&err));
        }
    }

    // Go: sys.go:587 readFileNoError
    pub fn read_file_no_error(&self, path: &str) -> String {
        let (content, ok) = self
            .fs_from_file_map()
            .read_file(&to_rooted_path(path, &self.get_current_directory()));
        assert!(ok, "File not found: {path}");
        content
    }

    // Go: sys.go:595 renameFileNoError
    pub fn rename_file_no_error(&self, old_path: &str, new_path: &str) {
        self.write_file_no_error(new_path, &self.read_file_no_error(old_path));
        self.remove_no_error(old_path);
    }

    // Go: sys.go:600 replaceFileText
    pub fn replace_file_text(&self, path: &str, old_text: &str, new_text: &str) {
        let content = self.read_file_no_error(path);
        let content = content.replacen(old_text, new_text, 1);
        self.write_file_no_error(path, &content);
    }

    // Go: sys.go:606 replaceFileTextAll
    pub fn replace_file_text_all(&self, path: &str, old_text: &str, new_text: &str) {
        let content = self.read_file_no_error(path);
        let content = content.replace(old_text, new_text);
        self.write_file_no_error(path, &content);
    }

    // Go: sys.go:612 appendFile
    pub fn append_file(&self, path: &str, text: &str) {
        let content = self.read_file_no_error(path);
        self.write_file_no_error(path, &format!("{content}{text}"));
    }

    // Go: sys.go:617 prependFile
    pub fn prepend_file(&self, path: &str, text: &str) {
        let content = self.read_file_no_error(path);
        self.write_file_no_error(path, &format!("{text}{content}"));
    }
}

impl System for TestSys {
    // Go: sys.go:232 Writer
    fn writer(&self) -> Writer {
        self.writer.clone()
    }
    // Go: sys.go:236 ErrorWriter (tsgo#4712)
    fn error_writer(&self) -> ErrorWriter {
        self.current_write.0.clone()
    }
    // Go: sys.go:198 FS
    fn fs(&self) -> Rc<dyn Fs> {
        self.fs.clone()
    }
    // Go: sys.go:224 DefaultLibraryPath
    fn default_library_path(&self) -> String {
        self.default_library_path.clone()
    }
    // Go: sys.go:228 GetCurrentDirectory
    fn get_current_directory(&self) -> String {
        self.cwd.clone()
    }
    // Go: sys.go:240 WriteOutputIsTTY (ts#63941)
    fn write_output_is_tty(&self) -> bool {
        self.output_is_tty
    }
    // Go: sys.go:244 GetWidthOfTerminal
    fn get_width_of_terminal(&self) -> i32 {
        let (width, _) = self.get_environment_variable("TS_TEST_TERMINAL_WIDTH");
        if !width.is_empty() {
            // Go `core.Must(strconv.Atoi(widthStr))`
            return width
                .parse()
                .unwrap_or_else(|err| panic!("strconv.Atoi: parsing {width:?}: {err}"));
        }
        0
    }
    // Go: sys.go:251 GetEnvironmentVariable (ts#63941)
    fn get_environment_variable(&self, name: &str) -> (String, bool) {
        match self.env.get(name) {
            Some(value) => (value.clone(), true),
            None => (String::new(), false),
        }
    }
    // Go: sys.go:259 Spawn (tsgo#4712)
    // Spawn serves the fake content mappers in-process, selecting the implementation by the exec command the
    // mapper package declares (see internal/testutil/contentmappertest), so tests exercise the full IPC stack
    // without spawning a subprocess.
    fn spawn(
        &self,
        command: &[String],
        dir: &str,
        stderr: Option<Box<dyn std::io::Write + Send>>,
    ) -> Result<Arc<dyn ProcessExitState>, GoError> {
        contentmappertest::new_spawner().spawn(command, dir, stderr)
    }
    // Go: sys.go:178 Now
    fn now(&self) -> SystemTime {
        self.shared.clock.now()
    }
    // Go: sys.go:182 SinceStart
    fn since_start(&self) -> Duration {
        self.shared.clock.since_start()
    }
}

// Go: sys.go:171 `_ tsc.CommandLineTesting = (*TestSys)(nil)`
// PORT: `Rc<TestSys>` is the `Rc<dyn CommandLineTesting>` of the child
// hooks (child.rs).
impl CommandLineTesting for TestSys {
    // Go: sys.go:263 OnEmittedFiles
    fn on_emitted_files(
        &self,
        result: &EmitResult,
        m_times_cache: Option<&Mutex<FxHashMap<Path, Option<SystemTime>>>>,
    ) {
        let serialized_mtimes = self.serialized_mtimes();
        for file in &result.emitted_files {
            let mod_time = self.map_fs().get_mod_time(file);
            if let Some(serialized_mtimes) = &serialized_mtimes
                && serialized_mtimes.get(file) == Some(&mod_time)
            {
                // Even though written, timestamp was reverted
                continue;
            }

            // Ensure that the timestamp for emitted files is in the order
            let now = self.now();
            if let Err(err) = self.fs_from_file_map().chtimes(file, None, Some(now)) {
                panic!(
                    "Failed to change time for emitted file: {file}: {}",
                    fs_error_text(&err)
                );
            }
            // Update the mTime cache in --b mode to store the updated timestamp so tests will behave deteministically when finding newest output
            if let Some(m_times_cache) = m_times_cache {
                let path = to_path(
                    file,
                    &self.get_current_directory(),
                    self.fs.use_case_sensitive_file_names(),
                );
                if let Some(entry) = lock(m_times_cache).get_mut(&path) {
                    *entry = Some(now);
                }
            }
        }
    }

    // Go: sys.go:291 OnListFilesStart
    fn on_list_files_start(&self, w: &Writer) {
        write_str(w, &format!("{LIST_FILE_START}\n"));
    }

    // Go: sys.go:295 OnListFilesEnd
    fn on_list_files_end(&self, w: &Writer) {
        write_str(w, &format!("{LIST_FILE_END}\n"));
    }

    // Go: sys.go:299 OnStatisticsStart
    fn on_statistics_start(&self, w: &Writer) {
        write_str(w, &format!("{STATISTICS_START}\n"));
    }

    // Go: sys.go:303 OnStatisticsEnd
    fn on_statistics_end(&self, w: &Writer) {
        write_str(w, &format!("{STATISTICS_END}\n"));
    }

    // Go: sys.go:307 OnBuildStatusReportStart
    fn on_build_status_report_start(&self, w: &Writer) {
        write_str(w, &format!("{BUILD_STATUS_REPORT_START}\n"));
    }

    // Go: sys.go:311 OnBuildStatusReportEnd
    fn on_build_status_report_end(&self, w: &Writer) {
        write_str(w, &format!("{BUILD_STATUS_REPORT_END}\n"));
    }

    // Go: sys.go:315 OnWatchStatusReportStart
    fn on_watch_status_report_start(&self) {
        write_str(&self.writer, &format!("{WATCH_STATUS_REPORT_START}\n"));
    }

    // Go: sys.go:319 OnWatchStatusReportEnd
    fn on_watch_status_report_end(&self) {
        write_str(&self.writer, &format!("{WATCH_STATUS_REPORT_END}\n"));
    }

    // Go: sys.go:323 GetTrace
    fn get_trace(&self, w: Writer, locale: Locale) -> TraceFn {
        let tracer = self.tracer.clone();
        // PORT: Go `w == s.Writer()` compares the interface values.
        let use_package_json_cache = Rc::ptr_eq(&w, &self.writer);
        Rc::new(move |msg: &'static Message, args: Vec<String>| {
            write_str(&w, &format!("{TRACE_START}\n"));
            // With tsc -b building projects in parallel we cannot serialize the package.json lookup trace
            // so trace as if it wasnt cached
            let str = message_localize(msg, &locale, &args);
            tracer
                .borrow_mut()
                .trace_with_writer(&w, &str, use_package_json_cache);
            write_str(&w, &format!("{TRACE_END}\n"));
        })
    }

    // Go: sys.go:353 OnProgram
    fn on_program(&self, program: &Program) {
        let parts = self.program_parts(program);
        self.append_program_parts(&parts);
    }
}

// ---------------------------------------------------------------------------
// Output sanitizer
// ---------------------------------------------------------------------------

// Go: sys.go:419
const FAKE_TIME_STAMP: &str = "HH:MM:SS AM";
const FAKE_DURATION: &str = "d.ddds";

const BUILD_STARTING_AT: &str = "build starting at ";
const BUILD_FINISHED_IN: &str = "build finished in ";
const LIST_FILE_START: &str = "!!! List files start";
const LIST_FILE_END: &str = "!!! List files end";
const STATISTICS_START: &str = "!!! Statistics start";
const STATISTICS_END: &str = "!!! Statistics end";
const BUILD_STATUS_REPORT_START: &str = "!!! Build Status Report Start";
const BUILD_STATUS_REPORT_END: &str = "!!! Build Status Report End";
const WATCH_STATUS_REPORT_START: &str = "!!! Watch Status Report Start";
const WATCH_STATUS_REPORT_END: &str = "!!! Watch Status Report End";
const TRACE_START: &str = "!!! Trace start";
const TRACE_END: &str = "!!! Trace end";

// Go: sys.go:471 outputSanitizer
struct OutputSanitizer<'a> {
    for_comparing: bool,
    lines: Vec<&'a str>,
    index: usize,
    output_lines: Vec<String>,
}

/// Go `englishVersion`, `fakeEnglishVersion`, `czechVersion` and
/// `fakeCzechVersion` (sys.go:450).
struct VersionStrings {
    english_version: String,
    fake_english_version: String,
    czech_version: String,
    fake_czech_version: String,
}

static VERSION_STRINGS: LazyLock<VersionStrings> = LazyLock::new(|| {
    let (czech, _) = locale::parse("cs");
    let localize = |locale: &Locale, version: &str| {
        message_localize(diag::Version_0, locale, &[version.to_string()])
    };
    VersionStrings {
        english_version: localize(&locale::DEFAULT, version()),
        fake_english_version: localize(&locale::DEFAULT, FAKE_TS_VERSION),
        czech_version: localize(&czech, version()),
        fake_czech_version: localize(&czech, FAKE_TS_VERSION),
    }
});

impl OutputSanitizer<'_> {
    // Go: sys.go:486 addOutputLine
    fn add_output_line(&mut self, s: &str) {
        let versions = &*VERSION_STRINGS;
        let s = s.replace(&format!("'{}'", version()), &format!("'{FAKE_TS_VERSION}'"));
        let s = s.replace(&versions.english_version, &versions.fake_english_version);
        let s = s.replace(&versions.czech_version, &versions.fake_czech_version);
        let s = sanitize_internal_symbol_name(&s);
        self.output_lines.push(s);
    }

    // Go: sys.go:494 sanitizeBuildStatusTimeStamp
    // PORT: Go slices the bytes of the line; this slices its Go bytes.
    fn sanitize_build_status_time_stamp(&self) -> String {
        let status_line = go_string_bytes(self.lines[self.index]);
        let hh_separator = match status_line.iter().position(|&b| b == b':') {
            Some(index) if index >= 2 => index,
            _ => panic!("Expected timestamp"),
        };
        let mut bytes = status_line[..hh_separator - 2].to_vec();
        bytes.extend_from_slice(FAKE_TIME_STAMP.as_bytes());
        bytes.extend_from_slice(&status_line[hh_separator + FAKE_TIME_STAMP.len() - 2..]);
        go_string_from_bytes(bytes)
    }

    // Go: sys.go:503 transformLines
    fn transform_lines(&mut self) -> String {
        while self.index < self.lines.len() {
            let line = self.lines[self.index];
            if line.starts_with(BUILD_STARTING_AT) {
                if !self.for_comparing {
                    self.add_output_line(&format!("{BUILD_STARTING_AT}{FAKE_TIME_STAMP}"));
                }
                self.index += 1;
                continue;
            }
            if line.starts_with(BUILD_FINISHED_IN) {
                if !self.for_comparing {
                    self.add_output_line(&format!("{BUILD_FINISHED_IN}{FAKE_DURATION}"));
                }
                self.index += 1;
                continue;
            }
            if !self.add_or_skip_lines_for_comparing(LIST_FILE_START, LIST_FILE_END, false, false)
                && !self.add_or_skip_lines_for_comparing(
                    STATISTICS_START,
                    STATISTICS_END,
                    true,
                    false,
                )
                && !self.add_or_skip_lines_for_comparing(TRACE_START, TRACE_END, false, false)
                && !self.add_or_skip_lines_for_comparing(
                    BUILD_STATUS_REPORT_START,
                    BUILD_STATUS_REPORT_END,
                    false,
                    true,
                )
                && !self.add_or_skip_lines_for_comparing(
                    WATCH_STATUS_REPORT_START,
                    WATCH_STATUS_REPORT_END,
                    false,
                    true,
                )
            {
                self.add_output_line(line);
            }
            self.index += 1;
        }
        self.output_lines.join("\n")
    }

    // Go: sys.go:529 addOrSkipLinesForComparing
    // PORT: Go `sanitizeFirstLine` is always `sanitizeBuildStatusTimeStamp`
    // or nil; this is a flag.
    fn add_or_skip_lines_for_comparing(
        &mut self,
        line_start: &str,
        line_end: &str,
        skip_even_if_not_comparing: bool,
        sanitize_first_line: bool,
    ) -> bool {
        if self.lines[self.index] != line_start {
            return false;
        }
        self.index += 1;
        let mut is_first_line = true;
        while self.index < self.lines.len() {
            if self.lines[self.index] == line_end {
                return true;
            }
            if !self.for_comparing && !skip_even_if_not_comparing {
                let mut line = self.lines[self.index].to_string();
                if is_first_line && sanitize_first_line {
                    line = self.sanitize_build_status_time_stamp();
                    is_first_line = false;
                }
                self.add_output_line(&line);
            }
            self.index += 1;
        }
        panic!("Expected lineEnd{line_end} not found after {line_start}");
    }
}
