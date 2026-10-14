//! PORT: not in Go. Multi-run wasm support (`ts_wasm`): one process serves
//! many tsc requests (`ts_run`) through the program-version machinery, the
//! way watch mode and the language server do. The first request loads a
//! program version (`Entry::Version`, not `core::set_prog`), and every
//! later request loads a new version that reuses the parses of unchanged
//! files: the parse cache below keys a parse by path and mtime, as the
//! watch host's source file cache does (`execute::watcher`
//! `WatchCompilerHost::get_source_file`). The version that the last run
//! loaded stays loaded until the run after the next, so each version is
//! built from it (`new_program_version` `previous`), which shares the
//! unchanged frontend data (`GoSharedState::new`).
//!
//! The host enables this once (`enable`); `execute::execute_tsc`
//! `install_program` and `perform_compilation` then take the version paths
//! here. `end_run` releases the version of two runs ago (its checker pool
//! stops; its published file stores and static file versions stay, so the
//! unchanged files of the next run keep their parses).

use std::cell::RefCell;
use std::rc::Rc;
use std::time::SystemTime;

use crate::contentmapper::{Mapper, Project, SourceFiles};
use crate::frontend::prelude::*;
use crate::gostd::GoError;

static ENABLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// True when the run that is ending installed a program version. A run can
/// end without one (an early return such as `--version`): its `end_run`
/// must leave `CURRENT` and `RETIRED` alone, or it would release the
/// version that `CURRENT` still names and store the released version in
/// `RETIRED` as the `previous` of the next version.
static INSTALLED_RUN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

thread_local! {
    /// The program version that the last run loaded. It is the current
    /// program of the thread (`core::set_thread_program`), as `set_prog`
    /// would make a one-program program, and the `previous` of the next
    /// version.
    static CURRENT: std::cell::Cell<Option<&'static crate::core::GoProgram>> =
        const { std::cell::Cell::new(None) };
    /// The version before `CURRENT`, kept loaded while the last run used it
    /// as its `previous`. `end_run` releases it.
    static RETIRED: std::cell::Cell<Option<&'static crate::core::GoProgram>> =
        const { std::cell::Cell::new(None) };
    /// The parses of the file versions that the loads of this process have
    /// seen, by path, with the mtime that the host file system reported.
    /// A load that sees the same mtime takes the parse with no read and no
    /// parse. Files change between runs when the host writes them; the host
    /// file system must then report a changed mtime (a real one, or a
    /// monotonic stamp of the writer).
    static PARSE_CACHE: RefCell<FxHashMap<Path, CachedParse>> = RefCell::new(FxHashMap::default());
}

struct CachedParse {
    m_time: Option<Option<SystemTime>>,
    file: Rc<ParsedSourceFile>,
}

/// Turns the multi-run flow on for the rest of the process, and turns on the
/// freeable file versions of a watch process (`ast::set_watch_process`), so
/// a re-parse of a changed file is freed with its last holder instead of
/// staying static (watcher.rs `note_kept_parse`, the 9 MiB per edit).
pub fn enable() {
    if ENABLED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    crate::ast::set_watch_process();
}

/// True when `enable` has run.
#[must_use]
pub fn is_enabled() -> bool {
    ENABLED.load(std::sync::atomic::Ordering::Relaxed)
}

/// `execute::execute_tsc::install_program` in the multi-run flow: loads the
/// program version of `host` and `config` (the tsc `ProgramOptions`) and
/// makes it the current program of the thread, in place of
/// `program::install_new_program` (`core::set_prog`), which a process can
/// call once. The caller runs the compile with no extra scope, like the
/// installed program.
pub(crate) fn install_program_version(host: Rc<dyn CompilerHost>, config: Rc<ParsedCommandLine>) {
    let np = crate::execute::execute_tsc::new_frontend_program(host, config);
    crate::program::mark_freeable_parses(&np);
    let previous = CURRENT.with(|current| current.get());
    let version = crate::program::new_program_version(&np, previous);
    drop(np);
    crate::core::set_thread_program(Some(version));
    CURRENT.with(|current| current.set(Some(version)));
    INSTALLED_RUN.store(true, std::sync::atomic::Ordering::Release);
}

/// The end of a run: releases the version that the run before the last
/// loaded, and moves the last version there. Its checker pool stops; the
/// published stores and the parse cache stay, so the next run reuses the
/// parses of its unchanged files. Releases nothing before the second run.
/// A run that installed no version (an early return such as `--version`)
/// rotates nothing: `CURRENT` and `RETIRED` keep their programs.
pub fn end_run() {
    if !INSTALLED_RUN.swap(false, std::sync::atomic::Ordering::AcqRel) {
        return;
    }
    let retired = RETIRED.with(|retired| retired.replace(None));
    if let Some(old) = retired {
        crate::program::release_program(old);
    }
    RETIRED.with(|retired| retired.set(CURRENT.with(|current| current.get())));
}

/// The host of a run over the parse cache, in the multi-run flow. Every
/// method but `get_source_file` goes to the host of the run (a fresh
/// `CachedFs` compiler host, as a one-run process makes).
struct MultiRunHost {
    inner: Rc<dyn CompilerHost>,
}

impl MultiRunHost {
    /// The cache with a parse of `path` when its mtime is the one the host
    /// file system reports now and the parse was made with the parse
    /// options of this run. A file that the host deleted, one with a
    /// changed mtime, or one whose earlier parse used other compiler
    /// options (a later request can pass another tsc argument list, and
    /// the options choose `external_module_indicator_options`) is a miss
    /// (the watch host's cache test, plus the options test).
    fn cached(
        &self,
        path: &Path,
        m_time: Option<Option<SystemTime>>,
        opts: &SourceFileParseOptions,
    ) -> Option<Rc<ParsedSourceFile>> {
        PARSE_CACHE.with(|cache| {
            let cache = cache.borrow();
            let cached = cache.get(path)?;
            (cached.m_time == m_time
                && cached
                    .file
                    .parse_options()
                    .external_module_indicator_options
                    == opts.external_module_indicator_options)
                .then(|| cached.file.clone())
        })
    }
}

impl CompilerHost for MultiRunHost {
    fn fs(&self) -> Rc<dyn Fs> {
        self.inner.fs()
    }

    fn default_library_path(&self) -> String {
        self.inner.default_library_path()
    }

    fn get_current_directory(&self) -> String {
        self.inner.get_current_directory()
    }

    fn trace(&self, msg: &'static Message, args: Vec<String>) {
        self.inner.trace(msg, args);
    }

    // The test of the watch host (`WatchCompilerHost::get_source_file`): a
    // stat, then the parse of the cache when the mtime matches, else the
    // parse of the run's host, cached with the new mtime.
    fn get_source_file(&self, opts: &SourceFileParseOptions) -> Option<Rc<ParsedSourceFile>> {
        let m_time = self
            .inner
            .fs()
            .stat(&opts.file_name)
            .map(|info| info.mod_time());
        if let Some(file) = self.cached(&opts.path, m_time, opts) {
            return Some(file);
        }
        let file = self.inner.get_source_file(opts)?;
        if let Some(m_time) = m_time {
            PARSE_CACHE.with(|cache| {
                cache.borrow_mut().insert(
                    opts.path.clone(),
                    CachedParse {
                        m_time: Some(m_time),
                        file: file.clone(),
                    },
                );
            });
        }
        Some(file)
    }

    fn get_content_mapped_source_files(
        &self,
        parse_options: &SourceFileParseOptions,
        mapper: &Rc<Mapper>,
    ) -> Result<SourceFiles, GoError> {
        self.inner
            .get_content_mapped_source_files(parse_options, mapper)
    }

    fn content_mapper_project(&self) -> Option<Rc<dyn Project>> {
        self.inner.content_mapper_project()
    }

    fn get_resolved_project_reference(
        &self,
        file_name: &str,
        path: &Path,
    ) -> Option<Rc<ParsedCommandLine>> {
        self.inner.get_resolved_project_reference(file_name, path)
    }

    // The watch host's test: a load with parses in the cache starts no
    // workers (they would parse the cached files again for nothing).
    fn prefetch_parses(&self) -> bool {
        PARSE_CACHE.with(|cache| cache.borrow().is_empty()) && self.inner.prefetch_parses()
    }

    // The watch hosts return true (see the trait): a worker parse of a path
    // that this process published before is a freeable file version.
    fn freeable_worker_parses(&self) -> bool {
        true
    }
}

/// The compiler host of a run in the multi-run flow: the run's host over the
/// process parse cache. With the flow off, the host unchanged.
pub(crate) fn wrap_host(host: Rc<dyn CompilerHost>) -> Rc<dyn CompilerHost> {
    if !is_enabled() {
        return host;
    }
    Rc::new(MultiRunHost { inner: host })
}
