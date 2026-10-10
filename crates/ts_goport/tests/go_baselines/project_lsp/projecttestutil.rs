//! Port of Go `internal/testutil/projecttestutil` (projecttestutil.go,
//! clientmock_generated.go, npmexecutormock_generated.go).
//!
//! PORT: the moq mocks are hand-written. Like moq `-stub`, a method with
//! no function set records the call and returns the zero value
//! (`is_active` is false). Call lists are read with the `*_calls` methods.
//!
//! PORT: `setup*` must run in a child process (`child_test!`): it points
//! the process-wide OS override at the new map file system.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use ts_goport::frontend::bundled;
use ts_goport::frontend::tsoptions::parsed_command_line::glob_parse;
use ts_goport::frontend::tspath;
use ts_goport::frontend::vfs::{Entries, FileInfo, Fs, FsError, OsOverride, install_os_override};
use ts_goport::gostd::{self, Context, GoError, context};
use ts_goport::lsp::lsproto;
use ts_goport::project::{
    self, Client, Session, SessionInit, SessionOptions, WatcherID, ata, logging,
};

use crate::support::vfstest::{MapFile, MapFs};

// Go: projecttestutil.go:33 TestTypingsLocation
pub const TEST_TYPINGS_LOCATION: &str = "/home/src/Library/Caches/typescript";

/// Go `map[string]any` of file contents (strings, bytes or `vfstest.Symlink`).
pub type FileMap = BTreeMap<String, MapFile>;

/// Builds a `FileMap` from path and text pairs.
pub fn files(entries: &[(&str, &str)]) -> FileMap {
    entries
        .iter()
        .map(|(path, text)| (path.to_string(), MapFile::from(*text)))
        .collect()
}

// ---------------------------------------------------------------------------
// OS override
// ---------------------------------------------------------------------------

/// The map file system that `osvfs_fs()` reads in this child process.
fn current_map_fs() -> &'static Mutex<Option<MapFs>> {
    static CURRENT: OnceLock<Mutex<Option<MapFs>>> = OnceLock::new();
    CURRENT.get_or_init(|| Mutex::new(None))
}

/// The view of the current map file system. An empty case-insensitive map
/// before the first `setup`.
fn current_fs() -> Rc<dyn Fs> {
    let current = current_map_fs()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    match current {
        Some(map) => map.fs(),
        None => MapFs::from_map(Vec::<(String, MapFile)>::new(), false).fs(),
    }
}

/// An `Fs` that forwards every call to the current map file system, so
/// one process-wide override can serve each `setup` in turn.
struct SwitchFs;

impl Fs for SwitchFs {
    fn use_case_sensitive_file_names(&self) -> bool {
        current_fs().use_case_sensitive_file_names()
    }
    fn file_exists(&self, path: &str) -> bool {
        current_fs().file_exists(path)
    }
    fn read_file(&self, path: &str) -> (String, bool) {
        current_fs().read_file(path)
    }
    fn write_file(&self, path: &str, data: &str) -> Result<(), FsError> {
        current_fs().write_file(path, data)
    }
    fn append_file(&self, path: &str, data: &str) -> Result<(), FsError> {
        current_fs().append_file(path, data)
    }
    fn remove(&self, path: &str) -> Result<(), FsError> {
        current_fs().remove(path)
    }
    fn chtimes(
        &self,
        path: &str,
        a_time: Option<SystemTime>,
        m_time: Option<SystemTime>,
    ) -> Result<(), FsError> {
        current_fs().chtimes(path, a_time, m_time)
    }
    fn directory_exists(&self, path: &str) -> bool {
        current_fs().directory_exists(path)
    }
    fn get_accessible_entries(&self, path: &str) -> Entries {
        current_fs().get_accessible_entries(path)
    }
    fn stat(&self, path: &str) -> Option<FileInfo> {
        current_fs().stat(path)
    }
    fn realpath(&self, path: &str) -> String {
        current_fs().realpath(path)
    }
}

/// Installs the process-wide OS override (once). Only a child process of
/// `child_test!` calls it.
pub fn install_fs_override() {
    static INSTALLED: OnceLock<()> = OnceLock::new();
    INSTALLED.get_or_init(|| {
        install_os_override(OsOverride {
            fs: Arc::new(|| -> Rc<dyn Fs> { Rc::new(SwitchFs) }),
            current_directory: "/".to_string(),
        });
    });
}

fn set_current_map_fs(map: &MapFs) {
    *current_map_fs()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(map.clone());
}

// ---------------------------------------------------------------------------
// ClientMock (Go clientmock_generated.go)
// ---------------------------------------------------------------------------

/// Go `ClientMock.calls.WatchFiles` entry.
#[derive(Clone)]
pub struct WatchFilesCall {
    pub id: WatcherID,
    pub watchers: Vec<lsproto::FileSystemWatcher>,
}

/// Go `ClientMock.calls.ProgressStart` / `ProgressFinish` entry.
#[derive(Clone)]
pub struct ProgressCall {
    pub message: &'static ts_goport::diagnostics::Message,
    pub args: Vec<String>,
}

pub type WatchFilesFunc =
    Box<dyn Fn(&Context, &WatcherID, &[lsproto::FileSystemWatcher]) -> Result<(), GoError>>;

pub type GetLocaleFunc = Box<dyn Fn() -> ts_goport::locale::Locale>;

pub type SetLocaleFunc = Box<dyn Fn(&str)>;

pub type RefreshCodeLensFunc = Box<dyn Fn(&Context) -> Result<(), GoError>>;

pub type RegisterContentMapperExtensionsFunc =
    Box<dyn Fn(&Context, &[String]) -> Result<(), GoError>>;

// Go: clientmock_generated.go:58 ClientMock
#[derive(Default)]
pub struct ClientMock {
    pub watch_files_func: RefCell<Option<WatchFilesFunc>>,
    // Go `GetLocaleFunc` (#4660).
    pub get_locale_func: RefCell<Option<GetLocaleFunc>>,
    // Go `SetLocaleFunc`.
    pub set_locale_func: RefCell<Option<SetLocaleFunc>>,
    // Go `RefreshCodeLensFunc`.
    pub refresh_code_lens_func: RefCell<Option<RefreshCodeLensFunc>>,
    // Go `RegisterContentMapperExtensionsFunc` (tsgo#4712).
    pub register_content_mapper_extensions_func:
        RefCell<Option<RegisterContentMapperExtensionsFunc>>,
    watch_files: RefCell<Vec<WatchFilesCall>>,
    unwatch_files: RefCell<Vec<WatcherID>>,
    register_content_mapper_extensions: RefCell<Vec<Vec<String>>>,
    refresh_diagnostics: RefCell<usize>,
    publish_diagnostics: RefCell<Vec<lsproto::PublishDiagnosticsParams>>,
    refresh_inlay_hints: RefCell<usize>,
    refresh_code_lens: RefCell<usize>,
    progress_start: RefCell<Vec<ProgressCall>>,
    progress_finish: RefCell<Vec<ProgressCall>>,
    send_telemetry: RefCell<Vec<lsproto::TelemetryEvent>>,
    get_locale: RefCell<usize>,
    set_locale: RefCell<Vec<String>>,
}

impl ClientMock {
    pub fn watch_files_calls(&self) -> Vec<WatchFilesCall> {
        self.watch_files.borrow().clone()
    }
    pub fn unwatch_files_calls(&self) -> Vec<WatcherID> {
        self.unwatch_files.borrow().clone()
    }
    /// Go `RegisterContentMapperExtensionsCalls()`: the `Extensions` of each call.
    pub fn register_content_mapper_extensions_calls(&self) -> Vec<Vec<String>> {
        self.register_content_mapper_extensions.borrow().clone()
    }
    /// Go `len(RefreshDiagnosticsCalls())`.
    pub fn refresh_diagnostics_calls(&self) -> usize {
        *self.refresh_diagnostics.borrow()
    }
    pub fn publish_diagnostics_calls(&self) -> Vec<lsproto::PublishDiagnosticsParams> {
        self.publish_diagnostics.borrow().clone()
    }
    /// Go `len(RefreshInlayHintsCalls())`.
    pub fn refresh_inlay_hints_calls(&self) -> usize {
        *self.refresh_inlay_hints.borrow()
    }
    /// Go `len(RefreshCodeLensCalls())`.
    pub fn refresh_code_lens_calls(&self) -> usize {
        *self.refresh_code_lens.borrow()
    }
    pub fn progress_start_calls(&self) -> Vec<ProgressCall> {
        self.progress_start.borrow().clone()
    }
    pub fn progress_finish_calls(&self) -> Vec<ProgressCall> {
        self.progress_finish.borrow().clone()
    }
    /// Go `len(GetLocaleCalls())`.
    pub fn get_locale_calls(&self) -> usize {
        *self.get_locale.borrow()
    }
    /// Go `SetLocaleCalls()`: the `LocaleMoqParam` of each call.
    pub fn set_locale_calls(&self) -> Vec<String> {
        self.set_locale.borrow().clone()
    }
}

impl Client for ClientMock {
    fn watch_files(
        &self,
        ctx: &Context,
        id: WatcherID,
        watchers: &[lsproto::FileSystemWatcher],
    ) -> Result<(), GoError> {
        self.watch_files.borrow_mut().push(WatchFilesCall {
            id: id.clone(),
            watchers: watchers.to_vec(),
        });
        match &*self.watch_files_func.borrow() {
            Some(f) => f(ctx, &id, watchers),
            None => Ok(()),
        }
    }
    fn unwatch_files(&self, _ctx: &Context, id: WatcherID) -> Result<(), GoError> {
        self.unwatch_files.borrow_mut().push(id);
        Ok(())
    }
    // Go: clientmock_generated.go:463 ClientMock.RegisterContentMapperExtensions (tsgo#4712)
    fn register_content_mapper_extensions(
        &self,
        ctx: &Context,
        extensions: &[String],
    ) -> Result<(), GoError> {
        self.register_content_mapper_extensions
            .borrow_mut()
            .push(extensions.to_vec());
        match &*self.register_content_mapper_extensions_func.borrow() {
            Some(f) => f(ctx, extensions),
            None => Ok(()),
        }
    }
    fn refresh_diagnostics(&self, _ctx: &Context) -> Result<(), GoError> {
        *self.refresh_diagnostics.borrow_mut() += 1;
        Ok(())
    }
    fn publish_diagnostics(
        &self,
        _ctx: &Context,
        params: lsproto::PublishDiagnosticsParams,
    ) -> Result<(), GoError> {
        self.publish_diagnostics.borrow_mut().push(params);
        Ok(())
    }
    fn refresh_inlay_hints(&self, _ctx: &Context) -> Result<(), GoError> {
        *self.refresh_inlay_hints.borrow_mut() += 1;
        Ok(())
    }
    fn refresh_code_lens(&self, ctx: &Context) -> Result<(), GoError> {
        *self.refresh_code_lens.borrow_mut() += 1;
        match &*self.refresh_code_lens_func.borrow() {
            Some(f) => f(ctx),
            None => Ok(()),
        }
    }
    fn progress_start(&self, message: &'static ts_goport::diagnostics::Message, args: Vec<String>) {
        self.progress_start
            .borrow_mut()
            .push(ProgressCall { message, args });
    }
    fn progress_finish(
        &self,
        message: &'static ts_goport::diagnostics::Message,
        args: Vec<String>,
    ) {
        self.progress_finish
            .borrow_mut()
            .push(ProgressCall { message, args });
    }
    fn send_telemetry(
        &self,
        _ctx: &Context,
        telemetry: lsproto::TelemetryEvent,
    ) -> Result<(), GoError> {
        self.send_telemetry.borrow_mut().push(telemetry);
        Ok(())
    }
    fn is_active(&self) -> bool {
        false
    }
    fn set_locale(&self, locale: &str) {
        self.set_locale.borrow_mut().push(locale.to_string());
        if let Some(f) = &*self.set_locale_func.borrow() {
            f(locale);
        }
    }
    fn get_locale(&self) -> ts_goport::locale::Locale {
        *self.get_locale.borrow_mut() += 1;
        match &*self.get_locale_func.borrow() {
            Some(f) => f(),
            None => ts_goport::locale::Locale::default(),
        }
    }
}

// ---------------------------------------------------------------------------
// NpmExecutorMock (Go npmexecutormock_generated.go)
// ---------------------------------------------------------------------------

/// Go `NpmExecutorMock.calls.NpmInstall` entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NpmInstallCall {
    pub cwd: String,
    pub args: Vec<String>,
}

pub type NpmInstallFunc = Box<dyn Fn(&Context, &str, &[String]) -> (Vec<u8>, Option<GoError>)>;

// Go: npmexecutormock_generated.go:28 NpmExecutorMock
#[derive(Default)]
pub struct NpmExecutorMock {
    pub npm_install_func: RefCell<Option<NpmInstallFunc>>,
    /// PORT: no Go counterpart. When set, ATA runs npm through it on a
    /// helper thread (`ata::NpmExecutor::npm_install_func`), as with the
    /// LSP server's executor, and `npm_install` is not called.
    pub npm_install_on_thread: RefCell<Option<ata::NpmInstallFunc>>,
    npm_install: RefCell<Vec<NpmInstallCall>>,
}

impl NpmExecutorMock {
    pub fn npm_install_calls(&self) -> Vec<NpmInstallCall> {
        self.npm_install.borrow().clone()
    }
}

// ts#64544: NpmInstall takes a ctx.
// PORT: Go records the ctx of each call too; no test reads it, and the port
// keeps `NpmInstallCall` comparable, so it is not recorded.
impl ata::NpmExecutor for NpmExecutorMock {
    fn npm_install(&self, ctx: &Context, cwd: &str, args: &[String]) -> (Vec<u8>, Option<GoError>) {
        self.npm_install.borrow_mut().push(NpmInstallCall {
            cwd: cwd.to_string(),
            args: args.to_vec(),
        });
        match &*self.npm_install_func.borrow() {
            Some(f) => f(ctx, cwd, args),
            None => (Vec::new(), None),
        }
    }

    fn npm_install_func(&self) -> Option<ata::NpmInstallFunc> {
        self.npm_install_on_thread.borrow().clone()
    }
}

// ---------------------------------------------------------------------------
// SessionUtils
// ---------------------------------------------------------------------------

// Go: projecttestutil.go:36 TypingsInstallerOptions
#[derive(Clone, Default)]
pub struct TypingsInstallerOptions {
    pub types_registry: Vec<String>,
    /// PORT: Go map; ranged in key order here (Go order is random).
    pub package_to_file: BTreeMap<String, String>,
}

// Go: projecttestutil.go:41 SessionUtils
pub struct SessionUtils {
    current_directory: String,
    fs_from_file_map: Option<MapFs>,
    fs: Rc<dyn Fs>,
    client: Rc<ClientMock>,
    npm_executor: Rc<NpmExecutorMock>,
    ti_options: Option<Rc<TypingsInstallerOptions>>,
    logger: Rc<dyn logging::LogCollector>,
}

impl SessionUtils {
    // Go: projecttestutil.go:51 FsFromFileMap
    pub fn fs_from_file_map(&self) -> &MapFs {
        self.fs_from_file_map.as_ref().expect("no file map")
    }

    // Go: projecttestutil.go:55 Client
    pub fn client(&self) -> &Rc<ClientMock> {
        &self.client
    }

    // Go: projecttestutil.go:59 NpmExecutor
    pub fn npm_executor(&self) -> &Rc<NpmExecutorMock> {
        &self.npm_executor
    }

    // Go: projecttestutil.go:63 SetupNpmExecutorForTypingsInstaller
    fn setup_npm_executor_for_typings_installer(&self) {
        let Some(ti_options) = self.ti_options.clone() else {
            return;
        };
        let fs = self.fs.clone();
        *self.npm_executor.npm_install_func.borrow_mut() = Some(Box::new(
            move |_ctx: &Context,
                  cwd: &str,
                  package_names: &[String]|
                  -> (Vec<u8>, Option<GoError>) {
                // packageNames is actually npmInstallArgs due to interface misnaming
                let npm_install_args = package_names;
                let len_npm_install_args = npm_install_args.len();
                if len_npm_install_args < 3 {
                    return (
                        Vec::new(),
                        Some(gostd::errors::new(format!(
                            "unexpected npm install: {cwd} {}",
                            go_string_slice(npm_install_args)
                        ))),
                    );
                }

                if len_npm_install_args == 3 && npm_install_args[2] == "types-registry@latest" {
                    // Write typings file
                    let err = fs.write_file(
                        &format!("{cwd}/node_modules/types-registry/index.json"),
                        &create_types_registry_file_content(&ti_options),
                    );
                    return (Vec::new(), err.err().map(fs_error));
                }

                // Find the packages: they start at index 2 and continue until we hit a flag starting with --
                let mut package_end = len_npm_install_args;
                for (i, arg) in npm_install_args.iter().enumerate().skip(2) {
                    if arg.starts_with("--") {
                        package_end = i;
                        break;
                    }
                }

                for at_types_package_ts in &npm_install_args[2..package_end] {
                    // @types/packageName@TsVersionToUse
                    let mut at_types_package = at_types_package_ts.as_str();
                    // Remove version suffix
                    if let Some(version_index) = at_types_package.rfind('@') {
                        if version_index > 6 {
                            // "@types/".length is 7, so version @ must be after
                            at_types_package = &at_types_package[..version_index];
                        }
                    }
                    // Extract package name from @types/packageName
                    let package_base_name = &at_types_package[7..]; // Remove "@types/" prefix
                    let Some(content) = ti_options.package_to_file.get(package_base_name) else {
                        return (
                            Vec::new(),
                            Some(gostd::errors::new(format!(
                                "content not provided for {package_base_name}"
                            ))),
                        );
                    };
                    if let Err(err) = fs.write_file(
                        &format!("{cwd}/node_modules/@types/{package_base_name}/index.d.ts"),
                        content,
                    ) {
                        return (Vec::new(), Some(fs_error(err)));
                    }
                }
                (Vec::new(), None)
            },
        ));
    }

    // Go: projecttestutil.go:113 ToPath
    pub fn to_path(&self, file_name: &str) -> tspath::Path {
        tspath::to_path(
            file_name,
            &self.current_directory,
            self.fs.use_case_sensitive_file_names(),
        )
    }

    // Go: projecttestutil.go:117 FS
    pub fn fs(&self) -> &Rc<dyn Fs> {
        &self.fs
    }

    // Go: projecttestutil.go:125 WatchesFile
    // WatchesFile reports whether any registered file watcher would match the given
    // file path. It handles both absolute glob patterns and relative patterns with
    // a base URI. On case-insensitive file systems the paths in glob patterns are
    // lowercased, so callers should pass the lowercased path.
    pub fn watches_file(&self, file_path: &str) -> bool {
        for call in self.client.watch_files_calls() {
            for watcher in &call.watchers {
                if let Some(pattern) = &watcher.glob_pattern.pattern {
                    if let Ok(g) = glob_parse(pattern) {
                        if g.match_(file_path) {
                            return true;
                        }
                    }
                } else if let Some(rp) = &watcher.glob_pattern.relative_pattern {
                    let base_uri = rp
                        .base_uri
                        .uri
                        .as_ref()
                        .expect("relative pattern without base URI")
                        .0
                        .clone();
                    // Convert base URI (e.g. "file:///home/projects") to a directory path
                    // with trailing separator for proper prefix matching on path boundaries.
                    let base_dir = lsproto::DocumentUri(base_uri).file_name();
                    let base_dir = tspath::ensure_trailing_directory_separator(&base_dir);
                    if let Some(relative_path) = file_path.strip_prefix(base_dir.as_str()) {
                        if let Ok(g) = glob_parse(&rp.pattern) {
                            if g.match_(relative_path) {
                                return true;
                            }
                        }
                    }
                }
            }
        }
        false
    }

    // Go: projecttestutil.go:151 Logs
    pub fn logs(&self) -> String {
        self.logger.string()
    }
}

/// Go `fmt.Sprint([]string)`: `[a b c]`.
fn go_string_slice(items: &[String]) -> String {
    format!("[{}]", items.join(" "))
}

fn fs_error(err: FsError) -> GoError {
    gostd::errors::new(format!("{err:?}"))
}

// Go: projecttestutil.go:166 TypesRegistryConfigText
// PORT: Go ranges over the `TypesRegistryConfig` map in random order; the
// port uses the Go literal order.
pub fn types_registry_config_text() -> String {
    let mut result = String::new();
    for (key, value) in types_registry_config() {
        if !result.is_empty() {
            result.push(',');
        }
        result.push_str(&format!("\n      \"{key}\": \"{value}\""));
    }
    result
}

// Go: projecttestutil.go:186 TypesRegistryConfig
pub fn types_registry_config() -> Vec<(&'static str, &'static str)> {
    vec![
        ("latest", "1.3.0"),
        ("ts2.0", "1.0.0"),
        ("ts2.1", "1.0.0"),
        ("ts2.2", "1.2.0"),
        ("ts2.3", "1.3.0"),
        ("ts2.4", "1.3.0"),
        ("ts2.5", "1.3.0"),
        ("ts2.6", "1.3.0"),
        ("ts2.7", "1.3.0"),
    ]
}

// Go: projecttestutil.go:203 createTypesRegistryFileContent
pub fn create_types_registry_file_content(ti_options: &TypingsInstallerOptions) -> String {
    let mut builder = String::new();
    builder.push_str("{\n  \"entries\": {");
    for (index, entry) in ti_options.types_registry.iter().enumerate() {
        append_types_registry_config(&mut builder, index, entry);
    }
    let mut index = ti_options.types_registry.len();
    for key in ti_options.package_to_file.keys() {
        if !ti_options.types_registry.contains(key) {
            append_types_registry_config(&mut builder, index, key);
            index += 1;
        }
    }
    builder.push_str("\n  }\n}");
    builder
}

// Go: projecttestutil.go:220 appendTypesRegistryConfig
fn append_types_registry_config(builder: &mut String, index: usize, entry: &str) {
    if index > 0 {
        builder.push(',');
    }
    builder.push_str(&format!(
        "\n    \"{entry}\": {{{}\n    }}",
        types_registry_config_text()
    ));
}

// Go: projecttestutil.go:227 Setup
pub fn setup(files: FileMap) -> (Rc<Session>, SessionUtils) {
    setup_with_typings_installer(files, TypingsInstallerOptions::default())
}

// Go: projecttestutil.go:265 SetupWithOptions
pub fn setup_with_options(files: FileMap, options: SessionOptions) -> (Rc<Session>, SessionUtils) {
    setup_with_options_and_typings_installer(
        files,
        Some(options),
        TypingsInstallerOptions::default(),
    )
}

// Go: projecttestutil.go:269 SetupWithTypingsInstaller
pub fn setup_with_typings_installer(
    files: FileMap,
    ti_options: TypingsInstallerOptions,
) -> (Rc<Session>, SessionUtils) {
    setup_with_options_and_typings_installer(files, None, ti_options)
}

// Go: projecttestutil.go:273 SetupWithOptionsAndTypingsInstaller
pub fn setup_with_options_and_typings_installer(
    files: FileMap,
    options: Option<SessionOptions>,
    ti_options: TypingsInstallerOptions,
) -> (Rc<Session>, SessionUtils) {
    let (init, session_utils) = get_session_init_options(files, options, ti_options);
    let session = project::new_session(&init);
    (session, session_utils)
}

// Go: projecttestutil.go:280 WithRequestID
pub fn with_request_id(ctx: &Context) -> Context {
    ts_goport::frontend::core_context::with_request_id(ctx, "0")
}

/// The Go `project.SessionOptions` literal of `GetSessionInitOptions`,
/// with the Go zero values for the other fields.
pub fn default_session_options() -> SessionOptions {
    SessionOptions {
        current_directory: "/".to_string(),
        default_library_path: bundled::lib_path(),
        typings_location: TEST_TYPINGS_LOCATION.to_string(),
        position_encoding: lsproto::PositionEncodingKind::UTF8,
        watch_enabled: true,
        logging_enabled: true,
        telemetry_enabled: false,
        push_diagnostics_enabled: true,
        run_external_code: false,
        debounce_delay: Duration::ZERO,
        checker_pool_options: Default::default(),
    }
}

/// A Go `&project.SessionOptions{...}` literal: the given fields, zero
/// values for the rest (`push_diagnostics_enabled` false).
pub fn session_options(current_directory: &str) -> SessionOptions {
    SessionOptions {
        current_directory: current_directory.to_string(),
        push_diagnostics_enabled: false,
        ..default_session_options()
    }
}

// Go: projecttestutil.go:284 GetSessionInitOptions
pub fn get_session_init_options(
    files: FileMap,
    options: Option<SessionOptions>,
    ti_options: TypingsInstallerOptions,
) -> (SessionInit, SessionUtils) {
    assert!(
        std::env::var_os("TSCTEST_IN_CHILD").is_some(),
        "projecttestutil setup must run in a child process (child_test!)"
    );
    let fs_from_file_map = MapFs::from_map(files, false /*useCaseSensitiveFileNames*/);
    set_current_map_fs(&fs_from_file_map);
    let fs = bundled::wrap_fs(fs_from_file_map.fs());
    let client_mock = Rc::new(ClientMock::default());
    let npm_executor_mock = Rc::new(NpmExecutorMock::default());
    let session_utils = SessionUtils {
        current_directory: "/".to_string(),
        fs_from_file_map: Some(fs_from_file_map),
        fs: fs.clone(),
        client: client_mock.clone(),
        npm_executor: npm_executor_mock.clone(),
        ti_options: Some(Rc::new(ti_options)),
        logger: logging::new_test_logger(),
    };

    // Configure the npm executor mock to handle typings installation
    session_utils.setup_npm_executor_for_typings_installer();

    // Use provided options or create default ones
    let options = options.unwrap_or_else(default_session_options);

    let logger: Rc<dyn logging::Logger> = session_utils.logger.clone();
    let client: Rc<dyn Client> = client_mock;
    let npm_executor: Rc<dyn ata::NpmExecutor> = npm_executor_mock;
    (
        SessionInit {
            background_ctx: context::background(),
            options: Rc::new(options),
            fs,
            client: Some(client),
            logger: Some(logger),
            npm_executor: Some(npm_executor),
            spawner: None,
            content_mapper_logger: None,
            parse_cache: None,
            content_mapped_parse_cache: None,
        },
        session_utils,
    )
}

/// Go `bundled.WrapFS(vfstest.FromMap(files, useCaseSensitiveFileNames))`
/// for a test that builds its own session. It also points the OS override
/// at the map (see `setup`), so it must run in a child process.
pub fn wrapped_map_fs(files: FileMap, use_case_sensitive_file_names: bool) -> (MapFs, Rc<dyn Fs>) {
    assert!(
        std::env::var_os("TSCTEST_IN_CHILD").is_some(),
        "wrapped_map_fs must run in a child process (child_test!)"
    );
    let map = MapFs::from_map(files, use_case_sensitive_file_names);
    set_current_map_fs(&map);
    let fs = bundled::wrap_fs(map.fs());
    (map, fs)
}

/// The map file system of the last `setup` or `wrapped_map_fs` in this
/// child process (Go keeps the `*vfstest.MapFS` from `FromMap`).
pub fn current_map_fs_for_test() -> MapFs {
    current_map_fs()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
        .expect("no map file system")
}
