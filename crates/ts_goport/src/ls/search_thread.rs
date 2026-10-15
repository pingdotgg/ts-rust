//! Search threads of cross-project requests (`crossproject.rs`).
//!
//! PORT: Go runs the search of each project of a references,
//! implementations or rename request on its own goroutine, over the shared
//! program, snapshot and checker pools. Those Rust values use `Rc` and live
//! on the dispatch thread. Here each program that such a search reaches gets
//! one long-lived search thread, as the compile path gives each checker its
//! own thread (`program::create_checkers`):
//! - The thread starts from a copy of the dispatch thread's thread-local
//!   state (`program::WorkerSeed`), made after the program is bound. It
//!   makes its own checker when a job first needs one, and keeps it between
//!   jobs. It drops the checker after `CHECKER_IDLE_TIMEOUT` with no job,
//!   and ends when the program is released (`release_search_thread`).
//! - A job gets plain data (`SearchJob`) and returns plain data
//!   (`ItemOutcome`). The thread makes its own language service over a copy
//!   of the program data (`SearchProgramData`, `SearchView`) and a host
//!   (`WorkerHost`) that reads program files from the AST store. The text in
//!   the store is the text the program was parsed from, which is the
//!   snapshot's text of that file.
//! - The reads that the copy does not hold (files outside the program,
//!   directories, triple-slash resolutions) go to the dispatch thread
//!   (`HostQuery`). It answers them from the item's language service while it
//!   waits for the results (`answer_query`).
//! - No `Rc` value and no checker crosses threads.

use crate::ls::prelude::*;

use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

/// A search thread drops its checker after this long with no job.
// Go: project/checkerpool.go newCheckerPool (the default idle timeout of a
// checker).
const CHECKER_IDLE_TIMEOUT: Duration = Duration::from_secs(30);

/// The index of the next search thread checker. Search checkers get their
/// own id range, so that no id of a dispatch thread checker repeats.
static NEXT_SEARCH_CHECKER_INDEX: AtomicUsize = AtomicUsize::new(1 << 20);

/// The most searches that run on search threads at once (Go: GOMAXPROCS).
pub fn max_in_flight() -> usize {
    crate::program::available_cores()
}

// ---------------------------------------------------------------------------
// Jobs and messages
// ---------------------------------------------------------------------------

/// The item data of phase 2 that `handle_cross_project` has.
pub struct SearchItem {
    pub ctx: Context,
    pub uri: lsproto::DocumentUri,
    pub position: lsproto::Position,
    pub is_rename: bool,
    pub implementations: bool,
    pub options: SymbolEntryTransformOptions,
}

/// What phase 2 of an item needs on a search thread (plain data).
struct SearchJob<Req> {
    item: SearchItem,
    params: Req,
    project_id: crate::ls::autoimport::ProjectID,
    preferences: lsutil::UserPreferences,
    use_case_sensitive_file_names: bool,
    position_encoding: lsproto::PositionEncodingKind,
}

/// The result of phases 1 and 2 of an item, for its commit.
pub struct ItemOutcome<Resp> {
    /// The original definition locations, in search order.
    pub locations: Vec<(lsproto::DocumentUri, lsproto::Position)>,
    /// The response or error; None where Go returns with neither (no
    /// language service, a canceled request, or a panic).
    pub result: Option<Result<Resp, GoError>>,
    /// Go `panicOccured` of the item.
    pub panic: Option<String>,
}

impl<Resp> ItemOutcome<Resp> {
    /// An item that returned before its search.
    pub fn skipped() -> Self {
        ItemOutcome {
            locations: Vec::new(),
            result: None,
            panic: None,
        }
    }

    /// An item that panicked before its search.
    pub fn panicked(panic_occured: String) -> Self {
        ItemOutcome {
            locations: Vec::new(),
            result: None,
            panic: Some(panic_occured),
        }
    }
}

/// Starts phase 2 of item number `item` on the search thread of the
/// language service's program: `start_search::<K>`. The request's params come
/// first.
pub type StartSearch<Req, Resp> =
    fn(&Req, &LanguageService, usize, SearchItem, mpsc::Sender<ToDispatch<Resp>>);

/// Phase 2 of an item on a search thread: `run_search::<K>`.
type SearchOnThread<Req, Resp> = fn(&Rc<SearchView>, SearchJob<Req>) -> ItemOutcome<Resp>;

/// A read that a search thread asks the dispatch thread for.
pub enum HostQuery {
    ReadFile(String),
    FileExists(String),
    DirectoryExists(String),
    GetDirectories(String),
    ReadDirectory {
        current_dir: String,
        path: String,
        extensions: Vec<String>,
        excludes: Vec<String>,
        includes: Vec<String>,
        depth: i32,
    },
    IsSourceFromProjectReference(tspath::Path),
    ReferencesToFile {
        referencing_file: Node,
        target: Node,
    },
    ReferenceAtPosition {
        source_file: Node,
        position: i32,
    },
}

/// The dispatch thread's answer to a `HostQuery`.
pub enum HostAnswer {
    /// The file text is shared, not copied (`FileText`).
    File(FileText, bool),
    Bool(bool),
    Strings(Vec<String>),
    FileReferences(Vec<FileReference>),
    RefInfo(Option<RefInfo>),
    /// The read panicked on the dispatch thread; the search panics again
    /// with the same payload, so a Go panic (`core::GoPanic`) stays one.
    Panic(Box<dyn std::any::Any + Send>),
    /// The request no longer waits (it panicked); the search result is
    /// dropped.
    Gone,
}

/// A message from a search thread to the dispatch thread.
pub enum ToDispatch<Resp> {
    Query {
        item: usize,
        query: HostQuery,
        reply: mpsc::Sender<HostAnswer>,
    },
    Done {
        item: usize,
        outcome: ItemOutcome<Resp>,
    },
}

/// Sends a `HostQuery` of the running job and waits for the answer.
type QueryFn = Box<dyn Fn(HostQuery) -> HostAnswer + Send>;

/// A job for a search thread.
type Job = Box<dyn FnOnce(&Rc<SearchView>) + Send>;

/// Sends the result of a job once. If the job is dropped before it runs (its
/// thread stopped), it sends a panic, so that the dispatch thread never waits
/// for it forever.
struct JobReply<Resp> {
    sender: Option<mpsc::Sender<ToDispatch<Resp>>>,
    item: usize,
}

impl<Resp> JobReply<Resp> {
    fn send(mut self, outcome: ItemOutcome<Resp>) {
        if let Some(sender) = self.sender.take() {
            let _ = sender.send(ToDispatch::Done {
                item: self.item,
                outcome,
            });
        }
    }
}

impl<Resp> Drop for JobReply<Resp> {
    fn drop(&mut self) {
        if let Some(sender) = self.sender.take() {
            let _ = sender.send(ToDispatch::Done {
                item: self.item,
                outcome: ItemOutcome::panicked(
                    "panic handling request: the search thread stopped".to_string(),
                ),
            });
        }
    }
}

// ---------------------------------------------------------------------------
// Dispatch thread side
// ---------------------------------------------------------------------------

thread_local! {
    /// The job queue of each search thread, by program (`ProgramView::identity`).
    static SEARCH_THREADS: RefCell<FxHashMap<usize, mpsc::Sender<Job>>> =
        RefCell::new(FxHashMap::default());
}

/// `StartSearch` of request kind `K`. The result arrives on `sender` as
/// `ToDispatch::Done`, after the host reads (`ToDispatch::Query`) that the
/// search needs.
pub fn start_search<K: CrossProjectSearch>(
    params: &K::Req,
    ls: &LanguageService,
    item: usize,
    search: SearchItem,
    sender: mpsc::Sender<ToDispatch<K::Resp>>,
) {
    let job = SearchJob {
        item: search,
        params: params.clone(),
        project_id: ls.project_id.clone(),
        preferences: ls.active_config.clone(),
        use_case_sensitive_file_names: ls.use_case_sensitive_file_names(),
        position_encoding: ls.converters.position_encoding(),
    };
    send_job(ls, item, job, run_search::<K>, sender);
}

/// Sends phase 2 of item `item` to the search thread of `ls`'s program.
fn send_job<Req: Send + 'static, Resp: Send + 'static>(
    ls: &LanguageService,
    item: usize,
    job: SearchJob<Req>,
    search: SearchOnThread<Req, Resp>,
    sender: mpsc::Sender<ToDispatch<Resp>>,
) {
    let query_sender = sender.clone();
    let query: QueryFn = Box::new(move |query: HostQuery| -> HostAnswer {
        let (reply, answer) = mpsc::channel();
        if query_sender
            .send(ToDispatch::Query { item, query, reply })
            .is_err()
        {
            return HostAnswer::Gone;
        }
        answer.recv().unwrap_or(HostAnswer::Gone)
    });
    let reply = JobReply {
        sender: Some(sender),
        item,
    };
    let task: Job = Box::new(move |view: &Rc<SearchView>| {
        view.begin_job(query);
        let outcome = match std::panic::catch_unwind(AssertUnwindSafe(|| search(view, job))) {
            Ok(outcome) => outcome,
            Err(payload) => ItemOutcome::panicked(panic_occured_text(payload)),
        };
        view.end_job();
        reply.send(outcome);
    });
    let program = &*ls.program;
    SEARCH_THREADS.with(|threads| {
        let mut threads = threads.borrow_mut();
        let jobs = threads
            .entry(program.identity())
            .or_insert_with(|| spawn_search_thread(program));
        if let Err(mpsc::SendError(task)) = jobs.send(task) {
            // The thread stopped outside a job; start a new one.
            let new_jobs = spawn_search_thread(program);
            new_jobs.send(task).expect("a new search thread takes jobs");
            *jobs = new_jobs;
        }
    });
}

/// Ends the search thread of `program` once its queued jobs ran. Called when
/// the program is released (Go frees its checkers with it).
pub fn release_search_thread(program: &compiler::NewProgram) {
    SEARCH_THREADS.with(|threads| threads.borrow_mut().remove(&program.identity()));
}

/// Answers a search thread's read from the item's language service.
pub fn answer_query(ls: &LanguageService, query: HostQuery) -> HostAnswer {
    // The item's program is current, as when the item runs on this thread.
    let _program = ls.enter_program();
    let answered = std::panic::catch_unwind(AssertUnwindSafe(|| match query {
        HostQuery::ReadFile(file_name) => {
            let (text, ok) = ls.host.read_file(&file_name);
            HostAnswer::File(text, ok)
        }
        HostQuery::FileExists(path) => HostAnswer::Bool(ls.host.file_exists(&path)),
        HostQuery::DirectoryExists(path) => HostAnswer::Bool(ls.host.directory_exists(&path)),
        HostQuery::GetDirectories(path) => HostAnswer::Strings(ls.host.get_directories(&path)),
        HostQuery::ReadDirectory {
            current_dir,
            path,
            extensions,
            excludes,
            includes,
            depth,
        } => HostAnswer::Strings(ls.host.read_directory(
            &current_dir,
            &path,
            &extensions,
            &excludes,
            &includes,
            depth,
        )),
        HostQuery::IsSourceFromProjectReference(path) => {
            HostAnswer::Bool(ls.program.is_source_from_project_reference(&path))
        }
        HostQuery::ReferencesToFile {
            referencing_file,
            target,
        } => HostAnswer::FileReferences(ProgramView::references_to_file(
            &*ls.program,
            referencing_file,
            target,
        )),
        HostQuery::ReferenceAtPosition {
            source_file,
            position,
        } => HostAnswer::RefInfo(get_reference_at_position(
            source_file,
            position,
            &ls.program,
        )),
    }));
    answered.unwrap_or_else(HostAnswer::Panic)
}

/// Starts the search thread of `program` and returns its job queue.
fn spawn_search_thread(program: &compiler::NewProgram) -> mpsc::Sender<Job> {
    // Go binds every file before it makes a checker (`BindSourceFiles`).
    // Bind here, so that the seed holds what binding made on this thread.
    ls_program::bind_source_files(program);
    let data = Arc::new(SearchProgramData::new(program));
    let seed = {
        let _program = ls_program::enter(program);
        crate::program::WorkerSeed::take()
    };
    let (jobs, receiver) = mpsc::channel::<Job>();
    std::thread::Builder::new()
        .name("ls-search".to_string())
        .stack_size(crate::gostd::stack::max_stack_size())
        .spawn(move || search_thread_main(seed, data, receiver))
        .expect("failed to spawn ls-search thread");
    jobs
}

// ---------------------------------------------------------------------------
// Search thread side
// ---------------------------------------------------------------------------

fn search_thread_main(
    seed: crate::program::WorkerSeed,
    data: Arc<SearchProgramData>,
    jobs: mpsc::Receiver<Job>,
) {
    seed.install();
    let _program = ls_program::enter_version(data.version);
    let view = Rc::new(SearchView::new(data));
    loop {
        let job = if view.has_checker() {
            match jobs.recv_timeout(CHECKER_IDLE_TIMEOUT) {
                Ok(job) => job,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    view.drop_checker();
                    continue;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        } else {
            match jobs.recv() {
                Ok(job) => job,
                Err(_) => break,
            }
        };
        job(&view);
    }
}

/// Phase 2 of an item of request kind `K` on a search thread
/// (`SearchOnThread`).
fn run_search<K: CrossProjectSearch>(
    view: &Rc<SearchView>,
    job: SearchJob<K::Req>,
) -> ItemOutcome<K::Resp> {
    let item = &job.item;
    // Go: the queued function returns at once when the request is canceled.
    if item.ctx.err().is_some() {
        return ItemOutcome::skipped();
    }
    let mut locations: Vec<(lsproto::DocumentUri, lsproto::Position)> = Vec::new();
    // Go: defer func() { if r := recover(); r != nil { ... } }()
    let searched = std::panic::catch_unwind(AssertUnwindSafe(|| {
        let host: Rc<dyn Host> = Rc::new(WorkerHost::new(view, &job));
        let ls = new_language_service_for_view(
            job.project_id.clone(),
            Rc::clone(view),
            host,
            job.preferences.clone(),
            ls_program::enter_version(view.data.version),
        );
        search_item(
            &ls,
            &item.ctx,
            &job.params,
            &item.uri,
            item.position,
            item.is_rename,
            item.implementations,
            item.options,
            |ls, ctx, params, data, options| K::to_resp(ls, ctx, params, data, options),
            &mut |_| {},
            &mut locations,
        )
    }));
    match searched {
        Ok(result) => ItemOutcome {
            locations,
            result,
            panic: None,
        },
        Err(payload) => {
            // Go keeps using the checker; here a checker that a panic left
            // in the middle of its work is not reused.
            view.drop_checker();
            ItemOutcome {
                locations,
                result: None,
                panic: Some(panic_occured_text(payload)),
            }
        }
    }
}

/// The program data that a search reads (see `ProgramView`), copied from a
/// `NewProgram` on the dispatch thread. All of it is plain data.
pub struct SearchProgramData {
    version: &'static GoProgram,
    current_directory: String,
    use_case_sensitive_file_names: bool,
    /// Go `GetSourceFiles()`, as file roots.
    roots: Vec<Node>,
    /// The first index of each root in `roots`.
    root_indexes: FxHashMap<Node, i32>,
    /// Go `filesByPath`, as file roots.
    by_path: FxHashMap<tspath::Path, Node>,
    /// The paths of `by_path` that are sources of a project reference.
    project_reference_sources: FxHashSet<tspath::Path>,
    /// Go `libFiles` keys.
    default_libraries: FxHashSet<tspath::Path>,
    /// Go `jsxRuntimeImportSpecifiers`, the specifier nodes.
    jsx_runtime_import_specifiers: FxHashMap<tspath::Path, Node>,
    /// Go `importHelpersImportSpecifiers`.
    import_helpers_import_specifiers: FxHashMap<tspath::Path, Node>,
    /// The roots of the files that have `<reference path>` or
    /// `<reference types>` directives.
    files_with_references: FxHashSet<Node>,
}

impl SearchProgramData {
    fn new(p: &compiler::NewProgram) -> Self {
        let roots: Vec<Node> = p.get_source_files().iter().map(|f| f.root).collect();
        let mut root_indexes: FxHashMap<Node, i32> = FxHashMap::default();
        for (i, &root) in roots.iter().enumerate() {
            root_indexes.entry(root).or_insert(i as i32);
        }
        let mut by_path: FxHashMap<tspath::Path, Node> = FxHashMap::default();
        let mut project_reference_sources: FxHashSet<tspath::Path> = FxHashSet::default();
        for (path, file) in &p.files_by_path {
            by_path.insert(path.clone(), file.root);
            if p.is_source_from_project_reference(path) {
                project_reference_sources.insert(path.clone());
            }
        }
        let jsx_runtime_import_specifiers: FxHashMap<tspath::Path, Node> = p
            .jsx_runtime_import_specifiers
            .as_ref()
            .map(|specifiers| {
                specifiers
                    .iter()
                    .map(|(path, specifier)| (path.clone(), specifier.specifier))
                    .collect()
            })
            .unwrap_or_default();
        let import_helpers_import_specifiers: FxHashMap<tspath::Path, Node> = p
            .import_helpers_import_specifiers
            .as_ref()
            .map(|specifiers| {
                specifiers
                    .iter()
                    .map(|(path, &specifier)| (path.clone(), specifier))
                    .collect()
            })
            .unwrap_or_default();
        let files_with_references: FxHashSet<Node> = p
            .get_source_files()
            .iter()
            .filter(|f| !f.referenced_files.is_empty() || !f.type_reference_directives.is_empty())
            .map(|f| f.root)
            .collect();
        SearchProgramData {
            version: ls_program::program_version(p),
            current_directory: p.get_current_directory(),
            use_case_sensitive_file_names: p.use_case_sensitive_file_names(),
            roots,
            root_indexes,
            by_path,
            project_reference_sources,
            default_libraries: p.lib_files.keys().cloned().collect(),
            jsx_runtime_import_specifiers,
            import_helpers_import_specifiers,
            files_with_references,
        }
    }
}

/// The program of a search thread, and its checker. `SearchView` is the
/// `ProgramView` of the thread's language services.
pub struct SearchView {
    data: Arc<SearchProgramData>,
    /// The thread's checker (Go: a pool checker of the program).
    checker: RefCell<Option<Rc<RefCell<Checker>>>>,
    /// LSP line maps of program files, by file root.
    line_maps: RefCell<FxHashMap<Node, Rc<lsconv::LSPLineMap>>>,
    /// ECMAScript line infos of program files, by file root.
    line_infos: RefCell<FxHashMap<Node, Rc<sourcemap::lineinfo::ECMALineInfo>>>,
    /// The running job's link to the dispatch thread.
    job: RefCell<Option<JobHost>>,
}

/// The dispatch thread link of the running job.
struct JobHost {
    query: QueryFn,
    /// Files outside the program that the job read through the dispatch
    /// thread (the item's snapshot does not change during the job).
    files: RefCell<FxHashMap<String, (FileText, bool)>>,
}

impl SearchView {
    fn new(data: Arc<SearchProgramData>) -> Self {
        SearchView {
            data,
            checker: RefCell::new(None),
            line_maps: RefCell::new(FxHashMap::default()),
            line_infos: RefCell::new(FxHashMap::default()),
            job: RefCell::new(None),
        }
    }

    fn begin_job(&self, query: QueryFn) {
        *self.job.borrow_mut() = Some(JobHost {
            query,
            files: RefCell::new(FxHashMap::default()),
        });
    }

    fn end_job(&self) {
        *self.job.borrow_mut() = None;
    }

    fn has_checker(&self) -> bool {
        self.checker.borrow().is_some()
    }

    /// Drops the checker and the caches (idle, or after a panic).
    fn drop_checker(&self) {
        *self.checker.borrow_mut() = None;
        self.line_maps.borrow_mut().clear();
        self.line_infos.borrow_mut().clear();
    }

    /// Asks the dispatch thread. A read that panicked there panics here.
    fn query(&self, query: HostQuery) -> HostAnswer {
        let answer = match &*self.job.borrow() {
            Some(job) => (job.query)(query),
            None => HostAnswer::Gone,
        };
        match answer {
            HostAnswer::Panic(payload) => std::panic::resume_unwind(payload),
            answer => answer,
        }
    }

    /// The root of the program file named `file_name`, if the program has a
    /// file with that path (not a redirect to another path).
    fn program_file(&self, file_name: &str) -> Node {
        let path = tspath::to_path(
            file_name,
            &self.data.current_directory,
            self.data.use_case_sensitive_file_names,
        );
        match self.data.by_path.get(&path) {
            Some(&root) if source_file_info(root).path == path.0 => root,
            _ => Node::NIL,
        }
    }

    // Go: project/snapshot.go ReadFile
    fn read_file(&self, file_name: &str) -> (FileText, bool) {
        let root = self.program_file(file_name);
        if root.is_some() {
            return (source_file_text(root), true);
        }
        self.read_other_file(file_name)
    }

    /// A file outside the program, from the item's snapshot.
    fn read_other_file(&self, file_name: &str) -> (FileText, bool) {
        if let Some(job) = &*self.job.borrow()
            && let Some(file) = job.files.borrow().get(file_name)
        {
            return file.clone();
        }
        let file = match self.query(HostQuery::ReadFile(file_name.to_string())) {
            HostAnswer::File(text, ok) => (text, ok),
            _ => (FileText::default(), false),
        };
        if let Some(job) = &*self.job.borrow() {
            job.files
                .borrow_mut()
                .insert(file_name.to_string(), file.clone());
        }
        file
    }

    // Go: project/snapshot.go:97 (*Snapshot).LSPLineMap
    fn lsp_line_map(&self, file_name: &str) -> Option<Rc<lsconv::LSPLineMap>> {
        let root = self.program_file(file_name);
        if root.is_some() {
            let line_map = self
                .line_maps
                .borrow_mut()
                .entry(root)
                .or_insert_with(|| lsconv::compute_lsp_line_starts(&source_file_text(root)))
                .clone();
            return Some(line_map);
        }
        let (text, ok) = self.read_other_file(file_name);
        ok.then(|| lsconv::compute_lsp_line_starts(&text))
    }

    // Go: project/snapshot.go GetECMALineInfo
    fn ecma_line_info(&self, file_name: &str) -> Option<Rc<sourcemap::lineinfo::ECMALineInfo>> {
        let root = self.program_file(file_name);
        if root.is_some() {
            let line_info = self
                .line_infos
                .borrow_mut()
                .entry(root)
                .or_insert_with(|| {
                    let text = source_file_text(root);
                    Rc::new(sourcemap::lineinfo::create_ecma_line_info(
                        &text,
                        compute_ecma_line_starts(&text),
                    ))
                })
                .clone();
            return Some(line_info);
        }
        let (text, ok) = self.read_other_file(file_name);
        ok.then(|| {
            Rc::new(sourcemap::lineinfo::create_ecma_line_info(
                &text,
                compute_ecma_line_starts(&text),
            ))
        })
    }

    fn query_bool(&self, query: HostQuery) -> bool {
        matches!(self.query(query), HostAnswer::Bool(true))
    }

    fn query_strings(&self, query: HostQuery) -> Vec<String> {
        match self.query(query) {
            HostAnswer::Strings(strings) => strings,
            _ => Vec::new(),
        }
    }
}

impl ProgramView for SearchView {
    fn identity(&self) -> usize {
        self.data.version.id as usize
    }

    fn get_current_directory(&self) -> String {
        self.data.current_directory.clone()
    }

    fn source_file_root(&self, file_name: &str) -> Node {
        let path = tspath::to_path(
            file_name,
            &self.data.current_directory,
            self.data.use_case_sensitive_file_names,
        );
        self.data.by_path.get(&path).copied().unwrap_or(Node::NIL)
    }

    fn source_file_roots(&self) -> Vec<Node> {
        self.data.roots.clone()
    }

    fn source_file_index(&self, file: Node) -> i32 {
        self.data.root_indexes.get(&file).copied().unwrap_or(-1)
    }

    fn is_source_from_project_reference(&self, path: &tspath::Path) -> bool {
        if self.data.by_path.contains_key(path) {
            return self.data.project_reference_sources.contains(path);
        }
        self.query_bool(HostQuery::IsSourceFromProjectReference(path.clone()))
    }

    fn is_source_file_default_library(&self, path: &tspath::Path) -> bool {
        self.data.default_libraries.contains(path)
    }

    fn jsx_runtime_import_specifier(&self, path: &tspath::Path) -> Node {
        self.data
            .jsx_runtime_import_specifiers
            .get(path)
            .copied()
            .unwrap_or(Node::NIL)
    }

    fn import_helpers_import_specifier(&self, path: &tspath::Path) -> Node {
        self.data
            .import_helpers_import_specifiers
            .get(path)
            .copied()
            .unwrap_or(Node::NIL)
    }

    fn references_to_file(&self, referencing_file: Node, target: Node) -> Vec<FileReference> {
        if self.data.root_indexes.contains_key(&referencing_file)
            && !self.data.files_with_references.contains(&referencing_file)
        {
            return Vec::new();
        }
        match self.query(HostQuery::ReferencesToFile {
            referencing_file,
            target,
        }) {
            HostAnswer::FileReferences(references) => references,
            _ => Vec::new(),
        }
    }

    fn reference_at_position(&self, source_file: Node, position: i32) -> Option<RefInfo> {
        match self.query(HostQuery::ReferenceAtPosition {
            source_file,
            position,
        }) {
            HostAnswer::RefInfo(info) => info,
            _ => None,
        }
    }

    fn get_type_checker(&self, _ctx: &Context) -> (Rc<RefCell<Checker>>, ls_program::Release) {
        let mut slot = self.checker.borrow_mut();
        let checker = slot.get_or_insert_with(|| {
            let index = NEXT_SEARCH_CHECKER_INDEX.fetch_add(1, Ordering::Relaxed);
            Rc::new(RefCell::new(Checker::new(index)))
        });
        (Rc::clone(checker), ls_program::Release::noop())
    }
}

/// The `ls::Host` of a search thread language service: program files from
/// the AST store, other reads from the item's snapshot on the dispatch
/// thread.
struct WorkerHost {
    view: Rc<SearchView>,
    preferences: lsutil::UserPreferences,
    use_case_sensitive_file_names: bool,
    converters: Rc<lsconv::Converters>,
}

impl WorkerHost {
    fn new<Req>(view: &Rc<SearchView>, job: &SearchJob<Req>) -> Self {
        let line_map_view = Rc::clone(view);
        WorkerHost {
            view: Rc::clone(view),
            preferences: job.preferences.clone(),
            use_case_sensitive_file_names: job.use_case_sensitive_file_names,
            converters: lsconv::new_converters(
                job.position_encoding.clone(),
                move |file_name: &str| line_map_view.lsp_line_map(file_name),
            ),
        }
    }
}

impl Host for WorkerHost {
    fn use_case_sensitive_file_names(&self) -> bool {
        self.use_case_sensitive_file_names
    }

    fn read_file(&self, path: &str) -> (FileText, bool) {
        self.view.read_file(path)
    }

    fn converters(&self) -> Rc<lsconv::Converters> {
        Rc::clone(&self.converters)
    }

    fn get_preferences(&self, _active_file: &str) -> lsutil::UserPreferences {
        self.preferences.clone()
    }

    fn get_ecma_line_info(&self, file_name: &str) -> Option<Rc<sourcemap::lineinfo::ECMALineInfo>> {
        self.view.ecma_line_info(file_name)
    }

    // PORT: the search does not use auto-imports.
    fn auto_import_registry(&self) -> Option<Rc<autoimport::Registry>> {
        None
    }

    fn read_directory(
        &self,
        current_dir: &str,
        path: &str,
        extensions: &[String],
        excludes: &[String],
        includes: &[String],
        depth: i32,
    ) -> Vec<String> {
        self.view.query_strings(HostQuery::ReadDirectory {
            current_dir: current_dir.to_string(),
            path: path.to_string(),
            extensions: extensions.to_vec(),
            excludes: excludes.to_vec(),
            includes: includes.to_vec(),
            depth,
        })
    }

    fn get_directories(&self, path: &str) -> Vec<String> {
        self.view
            .query_strings(HostQuery::GetDirectories(path.to_string()))
    }

    fn directory_exists(&self, path: &str) -> bool {
        self.view
            .query_bool(HostQuery::DirectoryExists(path.to_string()))
    }

    fn file_exists(&self, path: &str) -> bool {
        self.view
            .query_bool(HostQuery::FileExists(path.to_string()))
    }
}
