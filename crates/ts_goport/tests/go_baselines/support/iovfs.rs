//! Go: internal/vfs/iovfs/iofs.go, with the Go standard library `io/fs`
//! and `path` pieces it reaches (`fs.FS`, `fs.Sub`, `fs.Stat`,
//! `fs.ReadDir`, `fs.ReadFile`, `path.Join`, `path.Dir`, `path.Base`).
//!
//! PORT: reads go through the production `ts_goport::frontend::vfs::Common`,
//! which decodes BOMs and returns port form text exactly as Go
//! `vfs/internal` does. `Common.root_for` is a `fn` pointer, so it cannot
//! capture the file system. Each `IoVfs` method that calls `Common` pushes
//! its file system on a thread-local stack first and pops it after (also
//! on a panic); `root_for` reads the top of the stack. Nested calls on one
//! thread are safe.

use std::cell::RefCell;
use std::io;
use std::sync::Arc;
use std::time::SystemTime;

use ts_goport::frontend::tspath::{
    get_directory_path, is_url, normalize_path, remove_trailing_directory_separator,
};
use ts_goport::frontend::vfs::{
    Common, DirEntry, Entries, FileInfo, FileMode, Fs, FsError, IoFs, file_info_to_dir_entry,
    io_fs_valid_path, root_length, split_path,
};
use ts_goport::scanner_util::compare_go_strings;

use crate::support::vfstest::go_quote;

// ---------------------------------------------------------------------------
// Go standard library: io/fs
// ---------------------------------------------------------------------------

/// What the caller of `GoFs::open` reads from the opened file. Go opens the
/// same file in every case; the port skips the copies that the caller does
/// not read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenFor {
    /// `Stat()` only (Go `fs.Stat`).
    Stat,
    /// `Stat()` and `ReadDir(-1)` (Go `fs.ReadDir`).
    ReadDir,
    /// `Stat()` and the contents (Go `fs.ReadFile`).
    ReadFile,
}

/// Go `fs.FileInfo` with its `Sys()` value.
// PORT: Go `Sys() any`; only vfstest_test.go reads it, an int or nil.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IoFileInfo {
    pub info: FileInfo,
    pub sys: Option<i64>,
}

/// An opened Go `fs.File`: its `Stat()`, and what `OpenFor` asked for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IoFile {
    pub info: IoFileInfo,
    /// The contents of a regular file (`OpenFor::ReadFile`).
    pub data: Option<Vec<u8>>,
    /// The `ReadDir(-1)` entries of a directory (`OpenFor::ReadDir`), in the
    /// order the file returns them.
    pub entries: Option<Vec<IoFileInfo>>,
}

// Go: io/fs/fs.go FS
// PORT: Go standard library interface, plus the optional interfaces that
// `From` checks with type assertions (`as_realpath_fs`, `as_writable_fs`).
// `Send + Sync` because Go shares one value between goroutines.
pub trait GoFs: Send + Sync {
    // Go: Open(name), then Stat() and, as `want` asks, Read or ReadDir(-1).
    fn open(&self, name: &str, want: OpenFor) -> Result<IoFile, FsError>;

    // Go: fsys.(RealpathFS)
    fn as_realpath_fs(&self) -> Option<&dyn RealpathFs> {
        None
    }

    // Go: fsys.(WritableFS)
    fn as_writable_fs(&self) -> Option<&dyn WritableFs> {
        None
    }
}

// Go: iofs.go:14 RealpathFS
pub trait RealpathFs {
    fn realpath(&self, path: &str) -> Result<String, FsError>;
}

// Go: iofs.go:19 WritableFS
// PORT: `data` is port form text. A zero `time.Time` is `None`.
pub trait WritableFs {
    fn write_file(&self, path: &str, data: &str, perm: FileMode) -> Result<(), FsError>;
    fn append_file(&self, path: &str, data: &str, perm: FileMode) -> Result<(), FsError>;
    fn mkdir_all(&self, path: &str, perm: FileMode) -> Result<(), FsError>;
    // Removes `path` and all its contents. Will return the first error it encounters.
    fn remove(&self, path: &str) -> Result<(), FsError>;
    fn chtimes(
        &self,
        path: &str,
        a_time: Option<SystemTime>,
        m_time: Option<SystemTime>,
    ) -> Result<(), FsError>;
}

// Go: io/fs/stat.go Stat
// PORT: no `GoFs` here implements `fs.StatFS`, so this is Open + Stat.
pub fn fs_stat(fsys: &dyn GoFs, name: &str) -> Result<IoFileInfo, FsError> {
    Ok(fsys.open(name, OpenFor::Stat)?.info)
}

// Go: io/fs/readdir.go ReadDir
// PORT: Open + ReadDir(-1), sorted by name.
pub fn fs_read_dir(fsys: &dyn GoFs, name: &str) -> Result<Vec<DirEntry>, FsError> {
    let file = fsys.open(name, OpenFor::ReadDir)?;
    let Some(list) = file.entries else {
        return Err(path_error(
            "readdir",
            name,
            FsError::Other("not implemented".to_string()),
        ));
    };
    let mut list: Vec<DirEntry> = list
        .into_iter()
        .map(|entry| file_info_to_dir_entry(entry.info))
        .collect();
    list.sort_by(|a, b| compare_go_strings(a.name(), b.name()));
    Ok(list)
}

// Go: io/fs/readfile.go ReadFile
// PORT: Open + Read. Reading a directory fails as Go `mapDir.Read` does.
pub fn fs_read_file(fsys: &dyn GoFs, name: &str) -> Result<Vec<u8>, FsError> {
    let file = fsys.open(name, OpenFor::ReadFile)?;
    if file.info.info.is_dir() {
        return Err(path_error("read", name, FsError::Invalid));
    }
    Ok(file.data.unwrap_or_default())
}

// Go: io/fs/sub.go Sub
// PORT: `fstest.MapFS` implements `fs.SubFS` as `fs.Sub` over the same map,
// so every `GoFs` here gets the plain `subFS`.
pub fn fs_sub(fsys: Arc<dyn GoFs>, dir: &str) -> Result<Arc<dyn GoFs>, FsError> {
    if !io_fs_valid_path(dir) {
        return Err(path_error("sub", dir, FsError::Invalid));
    }
    if dir == "." {
        return Ok(fsys);
    }
    Ok(Arc::new(SubFs {
        fsys,
        dir: dir.to_string(),
    }))
}

// Go: io/fs/sub.go subFS
pub struct SubFs {
    fsys: Arc<dyn GoFs>,
    dir: String,
}

impl SubFs {
    // Go: io/fs/sub.go subFS.fullName
    fn full_name(&self, op: &'static str, name: &str) -> Result<String, FsError> {
        if !io_fs_valid_path(name) {
            return Err(path_error(op, name, FsError::Invalid));
        }
        Ok(go_path_join(&[self.dir.as_str(), name]))
    }

    // Go: io/fs/sub.go subFS.shorten
    fn shorten(&self, name: &str) -> Option<String> {
        if name == self.dir {
            return Some(".".to_string());
        }
        let dir = self.dir.as_bytes();
        let bytes = name.as_bytes();
        if bytes.len() >= dir.len() + 2 && bytes[dir.len()] == b'/' && &bytes[..dir.len()] == dir {
            return Some(name[dir.len() + 1..].to_string());
        }
        None
    }

    // Go: io/fs/sub.go subFS.fixErr
    fn fix_err(&self, err: FsError) -> FsError {
        if let FsError::Path { op, path, err } = &err
            && let Some(short) = self.shorten(path)
        {
            return FsError::Path {
                op,
                path: short,
                err: err.clone(),
            };
        }
        err
    }
}

impl GoFs for SubFs {
    // Go: io/fs/sub.go subFS.Open (and subFS.ReadDir, subFS.ReadFile, which
    // call the helpers on the full name).
    fn open(&self, name: &str, want: OpenFor) -> Result<IoFile, FsError> {
        let op = if want == OpenFor::Stat {
            "open"
        } else {
            "read"
        };
        let full = self.full_name(op, name)?;
        self.fsys.open(&full, want).map_err(|err| self.fix_err(err))
    }
}

/// Go `err.Error()` of a file system error.
pub fn fs_error_text(err: &FsError) -> String {
    match err {
        FsError::Invalid => "invalid argument".to_string(),
        FsError::Permission => "permission denied".to_string(),
        FsError::Exist => "file already exists".to_string(),
        FsError::NotExist => "file does not exist".to_string(),
        FsError::Closed => "file already closed".to_string(),
        FsError::SkipAll => "skip everything and stop the walk".to_string(),
        FsError::SkipDir => "skip this directory".to_string(),
        FsError::Path { op, path, err } => format!("{op} {path}: {err}"),
        FsError::Other(message) => message.clone(),
    }
}

/// Go `&fs.PathError{Op: op, Path: path, Err: err}`.
// PORT: the production `FsError::Path` holds an `io::Error`; its message is
// the Go text of `err`, so `fs_error_text` prints what Go prints.
pub fn path_error(op: &'static str, path: &str, err: FsError) -> FsError {
    let kind = match &err {
        FsError::NotExist => io::ErrorKind::NotFound,
        FsError::Invalid => io::ErrorKind::InvalidInput,
        FsError::Permission => io::ErrorKind::PermissionDenied,
        FsError::Exist => io::ErrorKind::AlreadyExists,
        _ => io::ErrorKind::Other,
    };
    FsError::path(op, path, io::Error::new(kind, fs_error_text(&err)))
}

/// Go `errors.Is(err, fs.ErrNotExist)`.
pub fn is_not_exist(err: &FsError) -> bool {
    match err {
        FsError::NotExist => true,
        FsError::Path { err, .. } => err.kind() == io::ErrorKind::NotFound,
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Go standard library: path
// ---------------------------------------------------------------------------

// Go: path/path.go Clean
// PORT: `/`-only on every host. The `frontend::vfs` `filepath_clean` is
// the host `filepath.Clean`, which writes `\` on Windows.
pub fn go_path_clean(path: &str) -> String {
    if path.is_empty() {
        return ".".to_string();
    }
    let p = path.as_bytes();
    let rooted = p[0] == b'/';
    let n = p.len();
    let mut out: Vec<u8> = Vec::with_capacity(n);
    let (mut r, mut dotdot) = (0usize, 0usize);
    if rooted {
        out.push(b'/');
        r = 1;
        dotdot = 1;
    }
    while r < n {
        if p[r] == b'/' {
            r += 1;
        } else if p[r] == b'.' && (r + 1 == n || p[r + 1] == b'/') {
            r += 1;
        } else if p[r] == b'.' && p[r + 1] == b'.' && (r + 2 == n || p[r + 2] == b'/') {
            r += 2;
            if out.len() > dotdot {
                let mut w = out.len() - 1;
                while w > dotdot && out[w] != b'/' {
                    w -= 1;
                }
                out.truncate(w);
            } else if !rooted {
                if !out.is_empty() {
                    out.push(b'/');
                }
                out.extend_from_slice(b"..");
                dotdot = out.len();
            }
        } else {
            if (rooted && out.len() != 1) || (!rooted && !out.is_empty()) {
                out.push(b'/');
            }
            while r < n && p[r] != b'/' {
                out.push(p[r]);
                r += 1;
            }
        }
    }
    if out.is_empty() {
        return ".".to_string();
    }
    String::from_utf8(out)
        .unwrap_or_else(|err| String::from_utf8_lossy(err.as_bytes()).into_owned())
}

// Go: path/path.go Join
pub fn go_path_join(elem: &[&str]) -> String {
    let size: usize = elem.iter().map(|e| e.len()).sum();
    if size == 0 {
        return String::new();
    }
    let mut buf = String::with_capacity(size + elem.len() - 1);
    for e in elem {
        if !buf.is_empty() || !e.is_empty() {
            if !buf.is_empty() {
                buf.push('/');
            }
            buf.push_str(e);
        }
    }
    go_path_clean(&buf)
}

// Go: path/path.go Dir
pub fn go_path_dir(path: &str) -> String {
    // Go: path.Split
    let dir = match path.rfind('/') {
        Some(i) => &path[..=i],
        None => "",
    };
    go_path_clean(dir)
}

// Go: path/path.go Base
pub fn go_path_base(path: &str) -> String {
    if path.is_empty() {
        return ".".to_string();
    }
    // Strip trailing slashes.
    let mut path = path;
    while !path.is_empty() && path.ends_with('/') {
        path = &path[..path.len() - 1];
    }
    // Find the last element
    if let Some(i) = path.rfind('/') {
        path = &path[i + 1..];
    }
    // If empty now, it had only slashes.
    if path.is_empty() {
        return "/".to_string();
    }
    path.to_string()
}

// ---------------------------------------------------------------------------
// iovfs
// ---------------------------------------------------------------------------

thread_local! {
    // PORT: the file systems of the `IoVfs` calls that are running on this
    // thread (see the module comment).
    static CURRENT_FSYS: RefCell<Vec<Arc<dyn GoFs>>> = const { RefCell::new(Vec::new()) };
}

/// Pops the file system that `with_fsys` pushed, also on a panic.
struct PushedFsys;

impl Drop for PushedFsys {
    fn drop(&mut self) {
        CURRENT_FSYS.with(|stack| {
            stack.borrow_mut().pop();
        });
    }
}

fn with_fsys<R>(fsys: &Arc<dyn GoFs>, f: impl FnOnce() -> R) -> R {
    CURRENT_FSYS.with(|stack| stack.borrow_mut().push(Arc::clone(fsys)));
    let _pushed = PushedFsys;
    f()
}

// Go: iofs.go:108 RootFor
fn root_for(root: &str) -> Option<Box<dyn IoFs>> {
    let fsys = CURRENT_FSYS
        .with(|stack| stack.borrow().last().cloned())
        .expect("iovfs: RootFor called outside an ioFS method");
    if root == "/" {
        return Some(Box::new(IoFsView { fsys }));
    }

    let p = remove_trailing_directory_separator(root);
    match fs_sub(fsys, p) {
        Ok(sub) => Some(Box::new(IoFsView { fsys: sub })),
        Err(err) => {
            if is_url(root) {
                return None;
            }
            panic!(
                "vfs: failed to create sub file system for {}: {}",
                go_quote(p),
                fs_error_text(&err)
            );
        }
    }
}

// PORT: the Go `fs.FS` value that `RootFor` returns, as the production
// `IoFs` (`fs.Stat`, `fs.ReadDir`, `fs.ReadFile` over it).
struct IoFsView {
    fsys: Arc<dyn GoFs>,
}

impl IoFs for IoFsView {
    fn stat(&self, name: &str) -> Result<FileInfo, FsError> {
        fs_stat(self.fsys.as_ref(), name).map(|info| info.info)
    }

    fn read_dir(&self, name: &str) -> Result<Vec<DirEntry>, FsError> {
        fs_read_dir(self.fsys.as_ref(), name)
    }

    fn read_file(&self, name: &str) -> Result<Vec<u8>, FsError> {
        fs_read_file(self.fsys.as_ref(), name)
    }
}

// Go: iofs.go:136 ioFS
// PORT: the Go func fields are methods that check `fsys` the way `From`
// does. `Send + Sync`, like the Go value.
pub struct IoVfs {
    common: Common,

    use_case_sensitive_file_names: bool,
    fsys: Arc<dyn GoFs>,
}

// Go: iofs.go:43 From
// From creates a new FS from an [fs.FS].
//
// For paths like `c:/foo/bar`, fsys will be used as though it's rooted at `/` and the path is `/c:/foo/bar`.
//
// If the provided [fs.FS] implements [RealpathFS], it will be used to implement the Realpath method.
// If the provided [fs.FS] implements [WritableFS], it will be used to implement the WriteFile method.
//
// From does not actually handle case-insensitivity; ensure the passed in [fs.FS]
// respects case-insensitive file names if needed. Consider using [vfstest.FromMap] for testing.
pub fn from(fsys: Arc<dyn GoFs>, use_case_sensitive_file_names: bool) -> IoVfs {
    IoVfs {
        common: Common {
            root_for,
            is_reparse_point: None,
        },
        use_case_sensitive_file_names,
        fsys,
    }
}

impl IoVfs {
    fn with_common<R>(&self, f: impl FnOnce(&Common) -> R) -> R {
        with_fsys(&self.fsys, || f(&self.common))
    }

    // Go: iofs.go:44 realpath (the func that From picks)
    fn realpath_fn(&self, path: &str) -> Result<String, FsError> {
        match self.fsys.as_realpath_fs() {
            Some(fsys) => {
                let (rest, had_slash) = match path.strip_prefix('/') {
                    Some(rest) => (rest, true),
                    None => (path, false),
                };
                let rp = fsys.realpath(rest)?;
                if had_slash {
                    return Ok(format!("/{rp}"));
                }
                Ok(rp)
            }
            None => Ok(path.to_string()),
        }
    }

    fn writable(&self, what: &str) -> &dyn WritableFs {
        match self.fsys.as_writable_fs() {
            Some(fsys) => fsys,
            None => panic!("{what} not supported"),
        }
    }

    // Go: iofs.go:68 writeFile (the func that From picks)
    fn write_file_fn(&self, path: &str, content: &str) -> Result<(), FsError> {
        let fsys = self.writable("writeFile");
        let rest = path.strip_prefix('/').unwrap_or(path);
        fsys.write_file(rest, content, FileMode(0o666))
    }

    // Go: iofs.go:72 appendFile (the func that From picks)
    fn append_file_fn(&self, path: &str, content: &str) -> Result<(), FsError> {
        let fsys = self.writable("appendFile");
        let rest = path.strip_prefix('/').unwrap_or(path);
        fsys.append_file(rest, content, FileMode(0o666))
    }

    // Go: iofs.go:76 mkdirAll (the func that From picks)
    fn mkdir_all_fn(&self, path: &str) -> Result<(), FsError> {
        let fsys = self.writable("mkdirAll");
        let rest = path.strip_prefix('/').unwrap_or(path);
        fsys.mkdir_all(rest, FileMode(0o777))
    }

    // Go: iofs.go:80 remove (the func that From picks)
    fn remove_fn(&self, path: &str) -> Result<(), FsError> {
        let fsys = self.writable("remove");
        let rest = path.strip_prefix('/').unwrap_or(path);
        fsys.remove(rest)
    }

    // Go: iofs.go:84 chtimes (the func that From picks)
    fn chtimes_fn(
        &self,
        path: &str,
        a_time: Option<SystemTime>,
        m_time: Option<SystemTime>,
    ) -> Result<(), FsError> {
        let fsys = self.writable("chtimes");
        let rest = path.strip_prefix('/').unwrap_or(path);
        fsys.chtimes(rest, a_time, m_time)
    }

    // Go: iofs.go:201 writeFileEnsuringDir
    fn write_file_ensuring_dir(
        &self,
        path: &str,
        content: &str,
        write: fn(&IoVfs, &str, &str) -> Result<(), FsError>,
    ) -> Result<(), FsError> {
        let _ = root_length(path); // Assert path is rooted
        if write(self, path, content).is_ok() {
            return Ok(());
        }
        self.mkdir_all_fn(&get_directory_path(&normalize_path(path)))?;
        write(self, path, content)
    }

    // Go: iofs.go:220 FSys
    pub fn fsys(&self) -> &Arc<dyn GoFs> {
        &self.fsys
    }
}

impl Fs for IoVfs {
    // Go: iofs.go:151 UseCaseSensitiveFileNames
    fn use_case_sensitive_file_names(&self) -> bool {
        self.use_case_sensitive_file_names
    }

    // Go: iofs.go:159 FileExists
    fn file_exists(&self, path: &str) -> bool {
        self.with_common(|common| common.file_exists(path))
    }

    // Go: iofs.go:172 ReadFile
    fn read_file(&self, path: &str) -> (String, bool) {
        self.with_common(|common| common.read_file(path))
    }

    // Go: iofs.go:212 WriteFile
    fn write_file(&self, path: &str, content: &str) -> Result<(), FsError> {
        self.write_file_ensuring_dir(path, content, IoVfs::write_file_fn)
    }

    // Go: iofs.go:216 AppendFile
    fn append_file(&self, path: &str, content: &str) -> Result<(), FsError> {
        self.write_file_ensuring_dir(path, content, IoVfs::append_file_fn)
    }

    // Go: iofs.go:180 Remove
    fn remove(&self, path: &str) -> Result<(), FsError> {
        let _ = root_length(path); // Assert path is rooted
        self.remove_fn(path)
    }

    // Go: iofs.go:185 Chtimes
    fn chtimes(
        &self,
        path: &str,
        a_time: Option<SystemTime>,
        m_time: Option<SystemTime>,
    ) -> Result<(), FsError> {
        let _ = root_length(path); // Assert path is rooted
        self.chtimes_fn(path, a_time, m_time)
    }

    // Go: iofs.go:155 DirectoryExists
    fn directory_exists(&self, path: &str) -> bool {
        self.with_common(|common| common.directory_exists(path))
    }

    // Go: iofs.go:163 GetAccessibleEntries
    fn get_accessible_entries(&self, path: &str) -> Entries {
        self.with_common(|common| common.get_accessible_entries(path))
    }

    // Go: iofs.go:167 Stat
    fn stat(&self, path: &str) -> Option<FileInfo> {
        let _ = root_length(path); // Assert path is rooted
        self.with_common(|common| common.stat(path))
    }

    // Go: iofs.go:190 Realpath
    fn realpath(&self, path: &str) -> String {
        let (root, rest) = split_path(path);
        // splitPath normalizes the path into parts (e.g. "c:/foo/bar" -> "c:/", "foo/bar")
        // Put them back together to call realpath.
        match self.realpath_fn(&format!("{root}{rest}")) {
            Ok(realpath) => realpath,
            Err(_) => path.to_string(),
        }
    }
}
