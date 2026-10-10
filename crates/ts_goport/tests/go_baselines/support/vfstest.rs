//! Go: internal/vfs/vfstest/vfstest.go, with the Go standard library
//! `testing/fstest.MapFS` that it wraps.
//!
//! PORT: strings. Paths are port form strings (see
//! `ts_goport::scanner_util::GO_STRING_MARKER`). File data is the Go bytes:
//! text given to `write_file`, `append_file` and the `From` impls of
//! `MapFile` is converted with `go_string_bytes`. The `iovfs` view reads
//! through the production `vfs::Common`, which decodes BOMs and returns the
//! port form, as Go `vfs/internal` does.
//!
//! PORT: sharing. Go shares one `*MapFS` between goroutines. `MapFs` is a
//! cheap `Clone` handle over `Arc<RwLock<..>>` and is `Send + Sync`, so emit
//! writes from checker threads reach the same map.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::ops::Bound;
use std::rc::Rc;
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ts_goport::frontend::tspath::{
    get_canonical_file_name, is_rooted_disk_path, normalize_path, path_is_absolute,
    remove_trailing_directory_separator,
};
use ts_goport::frontend::vfs::{FileInfo, FileMode, Fs, FsError, io_fs_valid_path};
use ts_goport::scanner_util::{
    GoUnit, compare_go_strings, go_string_bytes, go_string_from_bytes, go_unit_at,
};

use crate::support::iovfs::{
    self, GoFs, IoFile, IoFileInfo, IoVfs, OpenFor, RealpathFs, WritableFs, fs_error_text,
    go_path_base, go_path_dir, go_path_join, path_error,
};

// Go: vfstest.go:22 MapFS
// PORT: a shared handle. The Go `mu sync.RWMutex` and the fields it
// protects are `inner`.
#[derive(Clone)]
pub struct MapFs {
    inner: Arc<RwLock<Inner>>,
}

struct Inner {
    // keys in m are canonicalPaths
    m: BTreeMap<String, FsEntry>,

    use_case_sensitive_file_names: bool,

    // PORT: Go ranges over this map in random order when it looks for a
    // symlinked parent; the port uses the sorted order.
    symlinks: BTreeMap<String, String>,

    clock: Arc<dyn Clock>,
}

// Go: vfstest.go:37 Clock
pub trait Clock: Send + Sync {
    fn now(&self) -> SystemTime;
    fn since_start(&self) -> Duration;
}

// Go: vfstest.go:42 clockImpl
// PORT: Go `time.Time` keeps a monotonic reading; `since_start` uses an
// `Instant` for it.
pub struct ClockImpl {
    start: Instant,
}

impl ClockImpl {
    pub fn new() -> ClockImpl {
        ClockImpl {
            start: Instant::now(),
        }
    }
}

impl Default for ClockImpl {
    fn default() -> Self {
        ClockImpl::new()
    }
}

impl Clock for ClockImpl {
    // Go: vfstest.go:46 clockImpl.Now
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }

    // Go: vfstest.go:50 clockImpl.SinceStart
    fn since_start(&self) -> Duration {
        self.start.elapsed()
    }
}

/// The default real clock (Go `&clockImpl{start: time.Now()}`).
pub fn default_clock() -> Arc<dyn Clock> {
    Arc::new(ClockImpl::new())
}

// Go: testing/fstest/mapfs.go MapFile
// PORT: Go standard library type without its `Sys any` field; the map keeps
// that next to the file (`FsEntry`). `mod_time: None` is the Go zero time.
// `data` is the Go bytes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MapFile {
    pub data: Vec<u8>,
    pub mode: FileMode,
    pub mod_time: Option<SystemTime>,
}

/// Go `string` file content: the Go bytes of the port form text.
impl From<&str> for MapFile {
    fn from(text: &str) -> MapFile {
        MapFile {
            data: go_string_bytes(text).into_owned(),
            ..MapFile::default()
        }
    }
}

impl From<String> for MapFile {
    fn from(text: String) -> MapFile {
        MapFile::from(text.as_str())
    }
}

impl From<&String> for MapFile {
    fn from(text: &String) -> MapFile {
        MapFile::from(text.as_str())
    }
}

/// Go `[]byte` file content.
impl From<Vec<u8>> for MapFile {
    fn from(data: Vec<u8>) -> MapFile {
        MapFile {
            data,
            ..MapFile::default()
        }
    }
}

impl From<&[u8]> for MapFile {
    fn from(data: &[u8]) -> MapFile {
        MapFile::from(data.to_vec())
    }
}

// Go: vfstest.go:59 sys
// PORT: Go `original any` is the `Sys` value of the input file. Only
// vfstest_test.go sets it, to an int.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Sys {
    pub original: Option<i64>,
    pub realpath: String,
}

// PORT: a `*fstest.MapFile` in the map with its `Sys` field. vfstest sets
// `sys` on every entry (`setEntry`); a plain `fstest.MapFS` has none.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FsEntry {
    pub file: MapFile,
    pub sys: Option<Sys>,
}

// Go: vfstest.go:70 FromMap
// FromMap creates a new [vfs.FS] from a map of paths to file contents.
// Those file contents may be strings, byte slices, or [fstest.MapFile]s.
//
// The paths must be normalized absolute paths according to the tspath package,
// without trailing directory separators.
// The paths must be all POSIX-style or all Windows-style, but not both.
// PORT: returns the `iovfs` view. Use `MapFs::from_map` to keep the map.
pub fn from_map<K: Into<String>, F: Into<MapFile>>(
    m: impl IntoIterator<Item = (K, F)>,
    use_case_sensitive_file_names: bool,
) -> Rc<dyn Fs> {
    MapFs::from_map(m, use_case_sensitive_file_names).fs()
}

// Go: vfstest.go:80 FromMapWithClock
// PORT: returns the `iovfs` view. Use `MapFs::from_map_with_clock` to keep
// the map.
pub fn from_map_with_clock<K: Into<String>, F: Into<MapFile>>(
    m: impl IntoIterator<Item = (K, F)>,
    use_case_sensitive_file_names: bool,
    clock: Arc<dyn Clock>,
) -> Rc<dyn Fs> {
    MapFs::from_map_with_clock(m, use_case_sensitive_file_names, clock).fs()
}

// Go: vfstest.go:143 convertMapFS
// PORT: the Go input is an `fstest.MapFS`; each entry here is the path,
// the file and the file's `Sys` value. A nil clock is `None`.
pub fn convert_map_fs(
    input: Vec<(String, MapFile, Option<i64>)>,
    use_case_sensitive_file_names: bool,
    clock: Option<Arc<dyn Clock>>,
) -> MapFs {
    let clock = clock.unwrap_or_else(default_clock);
    let mut m = Inner {
        m: BTreeMap::new(),
        use_case_sensitive_file_names,
        symlinks: BTreeMap::new(),
        clock,
    };

    // PORT: a Go map has no duplicate keys; the last entry wins.
    let input: BTreeMap<String, (MapFile, Option<i64>)> = input
        .into_iter()
        .map(|(path, file, sys)| (path, (file, sys)))
        .collect();

    // Verify that the input is well-formed.
    let mut canonical_paths: BTreeMap<String, String> = BTreeMap::new();
    for path in input.keys() {
        let canonical = m.get_canonical_path(path);
        if let Some(other) = canonical_paths.get(&canonical) {
            // Ensure consistent panic messages
            let (path, other) = if compare_go_strings(path, other) == Ordering::Greater {
                (other.as_str(), path.as_str())
            } else {
                (path.as_str(), other.as_str())
            };
            panic!(
                "duplicate path: {} and {} have the same canonical path",
                go_quote(path),
                go_quote(other)
            );
        }
        canonical_paths.insert(canonical, path.clone());
    }

    // Sort the input by depth and path so we ensure parent dirs are created
    // before their children, if explicitly specified by the input.
    let mut input_keys: Vec<&String> = input.keys().collect();
    input_keys.sort_by(|a, b| compare_paths_by_parts(a, b));

    for p in input_keys {
        let (file, original) = &input[p];

        // Create all missing intermediate directories so we can attach the realpath to each of them.
        // fstest.MapFS doesn't require this as it synthesizes directories on the fly, but it's a lot
        // harder to reapply a realpath onto those when we're deep in some FileInfo method.
        let dir = dir_name(p);
        if !dir.is_empty()
            && let Err(err) = m.mkdir_all(dir, FileMode(0o777))
        {
            panic!(
                "failed to create intermediate directories for {}: {}",
                go_quote(p),
                fs_error_text(&err)
            );
        }
        let canonical = m.get_canonical_path(p);
        m.set_entry(p, canonical, file.clone(), *original);
    }

    MapFs {
        inner: Arc::new(RwLock::new(m)),
    }
}

// Go: vfstest.go:187 comparePathsByParts
pub fn compare_paths_by_parts(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a, b);
    loop {
        match (a.split_once('/'), b.split_once('/')) {
            (Some((a_start, a_end)), Some((b_start, b_end))) => {
                let r = compare_go_strings(a_start, b_start);
                if r != Ordering::Equal {
                    return r;
                }
                a = a_end;
                b = b_end;
            }
            _ => return compare_go_strings(a, b),
        }
    }
}

// Go: vfstest.go:237 Symlink
pub fn symlink(target: &str) -> MapFile {
    MapFile {
        data: go_string_bytes(target).into_owned(),
        mode: FileMode::SYMLINK,
        mod_time: None,
    }
}

// Go: vfstest.go:248 brokenSymlinkError, plus the fs.ErrNotExist that
// getFollowingSymlinks returns.
#[derive(Clone, Debug, PartialEq, Eq)]
enum FollowError {
    NotExist,
    Broken { from: String, to: String },
}

impl FollowError {
    // Go: errors.Is(err, fs.ErrNotExist)
    fn is_not_exist(&self) -> bool {
        matches!(self, FollowError::NotExist)
    }

    // Go: vfstest.go:252 brokenSymlinkError.Error, and fs.ErrNotExist.
    fn into_fs_error(self) -> FsError {
        match self {
            FollowError::NotExist => FsError::NotExist,
            FollowError::Broken { from, to } => FsError::Other(format!(
                "broken symlink {} -> {}",
                go_quote(&from),
                go_quote(&to)
            )),
        }
    }
}

// Go: vfstest.go:307 splitPath
fn split_path(s: &str, offset: usize) -> (String, String) {
    match s[offset..].find('/') {
        None => (s.to_string(), String::new()),
        Some(idx) => (
            s[..idx + offset].to_string(),
            s[idx + 1 + offset..].to_string(),
        ),
    }
}

// Go: vfstest.go:315 dirName
fn dir_name(p: &str) -> &str {
    // Go: path.Split
    let dir = match p.rfind('/') {
        Some(i) => &p[..=i],
        None => "",
    };
    dir.strip_suffix('/').unwrap_or(dir)
}

// Go: vfstest.go:320 baseName
fn base_name(p: &str) -> &str {
    // Go: path.Split
    match p.rfind('/') {
        Some(i) => &p[i + 1..],
        None => p,
    }
}

// Go: vfstest.go:493 umask
const UMASK: u32 = 0o022;

// Go: `perm &^ umask`
fn without_umask(perm: FileMode) -> FileMode {
    FileMode(perm.0 & !UMASK)
}

impl Inner {
    // Go: vfstest.go:206 getCanonicalPath
    fn get_canonical_path(&self, p: &str) -> String {
        get_canonical_file_name(p, self.use_case_sensitive_file_names)
    }

    // Go: vfstest.go:214 remove
    fn remove(&mut self, path: &str) -> Result<(), FsError> {
        let canonical = self.get_canonical_path(path);
        let Some(file_info) = self.m.remove(&canonical) else {
            // file does not exist
            return Ok(());
        };
        self.symlinks.remove(&canonical);

        if file_info.file.mode.is_dir() {
            let prefix = format!("{canonical}/");
            let children: Vec<String> = self
                .m
                .range::<str, _>((Bound::Included(prefix.as_str()), Bound::Unbounded))
                .take_while(|(path, _)| path.starts_with(&prefix))
                .map(|(path, _)| path.clone())
                .collect();
            for path in children {
                self.m.remove(&path);
                self.symlinks.remove(&path);
            }
        }
        Ok(())
    }

    // Go: vfstest.go:244 getFollowingSymlinks
    // PORT: Go returns (file, canonicalPath, error); the port returns the
    // file or the error, and the canonical path.
    fn get_following_symlinks(&self, p: &str) -> (Result<&FsEntry, FollowError>, String) {
        self.get_following_symlinks_worker(p.to_string(), "", "")
    }

    // Go: vfstest.go:261 getFollowingSymlinksWorker
    fn get_following_symlinks_worker(
        &self,
        p: String,
        symlink_from: &str,
        symlink_to: &str,
    ) -> (Result<&FsEntry, FollowError>, String) {
        if let Some(file) = self.m.get(&p)
            && !file.file.mode.intersects(FileMode::SYMLINK)
        {
            return (Ok(file), p);
        }

        if let Some(target) = self.symlinks.get(&p) {
            return self.get_following_symlinks_worker(target.clone(), &p, target);
        }

        // This could be a path underneath a symlinked directory.
        for (other, target) in &self.symlinks {
            if other.len() < p.len()
                && p.as_bytes()[..other.len()] == *other.as_bytes()
                && p.as_bytes()[other.len()] == b'/'
            {
                let next = format!("{}{}", target, &p[other.len()..]);
                return self.get_following_symlinks_worker(next, other, target);
            }
        }

        let err = if symlink_from.is_empty() {
            FollowError::NotExist
        } else {
            FollowError::Broken {
                from: symlink_from.to_string(),
                to: symlink_to.to_string(),
            }
        };
        (Err(err), p)
    }

    // Go: vfstest.go:288 setEntry
    // PORT: `original` is the Go `file.Sys` value.
    fn set_entry(
        &mut self,
        realpath: &str,
        canonical: String,
        file: MapFile,
        original: Option<i64>,
    ) {
        assert!(!(realpath.is_empty() || canonical.is_empty()), "empty path");

        let symlink_target = if file.mode.intersects(FileMode::SYMLINK) {
            Some(self.get_canonical_path(&go_string_from_bytes(file.data.clone())))
        } else {
            None
        };
        self.m.insert(
            canonical.clone(),
            FsEntry {
                file,
                sys: Some(Sys {
                    original,
                    realpath: realpath.to_string(),
                }),
            },
        );

        if let Some(target) = symlink_target {
            self.symlinks.insert(canonical, target);
        }
    }

    // Go: vfstest.go:325 mkdirAll
    fn mkdir_all(&mut self, p: &str, perm: FileMode) -> Result<(), FsError> {
        assert!(!p.is_empty(), "empty path");

        // Fast path; already exists.
        if let (Ok(other), _) = self.get_following_symlinks(&self.get_canonical_path(p)) {
            if !other.file.mode.is_dir() {
                return Err(FsError::Other(format!(
                    "mkdir {}: path exists but is not a directory",
                    go_quote(p)
                )));
            }
            return Ok(());
        }

        let mut p = p.to_string();
        let mut to_create: Vec<String> = Vec::new();
        let mut offset = 0usize;
        loop {
            let (dir, rest) = split_path(&p, offset);
            let canonical = self.get_canonical_path(&dir);
            match self.get_following_symlinks(&canonical) {
                (Err(err), _) => {
                    if !err.is_not_exist() {
                        return Err(err.into_fs_error());
                    }
                    to_create.push(dir.clone());
                }
                (Ok(other), other_path) => {
                    if !other.file.mode.is_dir() {
                        return Err(FsError::Other(format!(
                            "mkdir {}: path exists but is not a directory",
                            go_quote(&other_path)
                        )));
                    }
                    if canonical != other_path {
                        // We have a symlinked parent, reset and start again.
                        p = format!("{}/{}", entry_realpath(other), rest);
                        to_create.clear();
                        offset = 0;
                        continue;
                    }
                }
            }
            if rest.is_empty() {
                break;
            }
            offset = dir.len() + 1;
        }

        for dir in to_create {
            let canonical = self.get_canonical_path(&dir);
            let file = MapFile {
                data: Vec::new(),
                mode: FileMode::DIR | without_umask(perm),
                mod_time: Some(self.clock.now()),
            };
            self.set_entry(&dir, canonical, file, None);
        }

        Ok(())
    }

    // Go: vfstest.go:513 WriteFile (under the lock)
    fn write_file(&mut self, path: &str, data: &str, perm: FileMode) -> Result<(), FsError> {
        let parent = dir_name(path);
        if !parent.is_empty() {
            let canonical = self.get_canonical_path(parent);
            match self.get_following_symlinks(&canonical).0 {
                Err(err) => {
                    return Err(FsError::Other(format!(
                        "write {}: {}",
                        go_quote(path),
                        fs_error_text(&err.into_fs_error())
                    )));
                }
                Ok(parent_file) => {
                    if !parent_file.file.mode.is_dir() {
                        return Err(FsError::Other(format!(
                            "write {}: parent path exists but is not a directory",
                            go_quote(path)
                        )));
                    }
                }
            }
        }

        let (file, cp) = self.get_following_symlinks(&self.get_canonical_path(path));
        match file {
            // PORT: Go panics on any error other than fs.ErrNotExist and a
            // broken symlink; getFollowingSymlinks returns no other error.
            Err(_) => {}
            Ok(file) => {
                if !file.file.mode.is_regular() {
                    return Err(FsError::Other(format!(
                        "write {}: path exists but is not a regular file",
                        go_quote(path)
                    )));
                }
            }
        }

        let file = MapFile {
            data: go_string_bytes(data).into_owned(),
            mod_time: Some(self.clock.now()),
            mode: without_umask(perm),
        };
        self.set_entry(path, cp, file, None);

        Ok(())
    }

    // Go: vfstest.go:549 AppendFile (under the lock)
    fn append_file(&mut self, path: &str, data: &str, perm: FileMode) -> Result<(), FsError> {
        let parent = dir_name(path);
        if !parent.is_empty() {
            let canonical = self.get_canonical_path(parent);
            match self.get_following_symlinks(&canonical).0 {
                Err(err) => {
                    return Err(FsError::Other(format!(
                        "append {}: {}",
                        go_quote(path),
                        fs_error_text(&err.into_fs_error())
                    )));
                }
                Ok(parent_file) => {
                    if !parent_file.file.mode.is_dir() {
                        return Err(FsError::Other(format!(
                            "append {}: parent path exists but is not a directory",
                            go_quote(path)
                        )));
                    }
                }
            }
        }

        let mut existing: Vec<u8> = Vec::new();
        let mut existing_mode = FileMode(0);
        let (file, cp) = self.get_following_symlinks(&self.get_canonical_path(path));
        match file {
            // PORT: see write_file.
            Err(_) => {}
            Ok(file) => {
                if !file.file.mode.is_regular() {
                    return Err(FsError::Other(format!(
                        "append {}: path exists but is not a regular file",
                        go_quote(path)
                    )));
                }
                existing = file.file.data.clone();
                existing_mode = file.file.mode;
            }
        }

        let mut combined = existing;
        combined.extend_from_slice(&go_string_bytes(data));

        let mut mode = existing_mode;
        if mode == FileMode(0) {
            mode = without_umask(perm);
        }

        let file = MapFile {
            data: combined,
            mod_time: Some(self.clock.now()),
            mode,
        };
        self.set_entry(path, cp, file, None);

        Ok(())
    }
}

/// The realpath that vfstest attaches to every entry (Go
/// `file.Sys.(*sys).realpath`).
fn entry_realpath(entry: &FsEntry) -> &str {
    match &entry.sys {
        Some(sys) => &sys.realpath,
        None => panic!("vfstest: entry without realpath"),
    }
}

impl MapFs {
    fn read(&self) -> RwLockReadGuard<'_, Inner> {
        self.inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn write(&self) -> RwLockWriteGuard<'_, Inner> {
        self.inner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    // Go: vfstest.go:70 FromMap, keeping the map.
    pub fn from_map<K: Into<String>, F: Into<MapFile>>(
        m: impl IntoIterator<Item = (K, F)>,
        use_case_sensitive_file_names: bool,
    ) -> MapFs {
        MapFs::from_map_with_clock(m, use_case_sensitive_file_names, default_clock())
    }

    // Go: vfstest.go:80 FromMapWithClock, keeping the map.
    // PORT: Go panics with "invalid file type" for a value that is not a
    // string, a byte slice or an `*fstest.MapFile`; the `Into<MapFile>`
    // bound rejects those at compile time.
    pub fn from_map_with_clock<K: Into<String>, F: Into<MapFile>>(
        m: impl IntoIterator<Item = (K, F)>,
        use_case_sensitive_file_names: bool,
        clock: Arc<dyn Clock>,
    ) -> MapFs {
        let mut posix = false;
        let mut windows = false;

        let mut check_path = |p: &str| {
            assert!(is_rooted_disk_path(p), "non-rooted path {}", go_quote(p));

            let normal = normalize_path(p);
            assert!(
                remove_trailing_directory_separator(&normal) == p,
                "non-normalized path {}",
                go_quote(p)
            );

            if p.starts_with('/') {
                posix = true;
            } else {
                windows = true;
            }
        };

        let mut entries: Vec<(String, MapFile)> = m
            .into_iter()
            .map(|(path, file)| (path.into(), file.into()))
            .collect();
        // Sorted creation to ensure times are always guaranteed to be in order.
        entries.sort_by(|a, b| compare_paths_by_parts(&a.0, &b.0));
        let mut mfs: Vec<(String, MapFile, Option<i64>)> = Vec::with_capacity(entries.len());
        for (p, f) in entries {
            check_path(&p);

            // PORT: every Go case copies the file and sets ModTime.
            let mut file = f;
            file.mod_time = Some(clock.now());

            if file.mode.intersects(FileMode::SYMLINK) {
                let target = go_string_from_bytes(file.data.clone());
                check_path(&target);

                let target = target.strip_prefix('/').unwrap_or(&target);
                file.data = go_string_bytes(target).into_owned();
            }

            let p = p.strip_prefix('/').map(str::to_string).unwrap_or(p);
            mfs.push((p, file, None));
        }

        assert!(!(posix && windows), "mixed posix and windows paths");

        convert_map_fs(mfs, use_case_sensitive_file_names, Some(clock))
    }

    /// The `iovfs` view (Go `FromMap` result).
    // PORT: the case flag is read now; `import_state` does not change a view
    // made before it.
    pub fn io_vfs(&self) -> IoVfs {
        iovfs::from(Arc::new(self.clone()), self.use_case_sensitive_file_names())
    }

    /// The `iovfs` view as a `vfs.FS`.
    pub fn fs(&self) -> Rc<dyn Fs> {
        Rc::new(self.io_vfs())
    }

    pub fn use_case_sensitive_file_names(&self) -> bool {
        self.read().use_case_sensitive_file_names
    }

    pub fn clock(&self) -> Arc<dyn Clock> {
        Arc::clone(&self.read().clock)
    }

    // Go: vfstest.go:428 Open
    // PORT: returns the opened file's `Stat()` and, as `want` asks, its
    // contents or its `ReadDir(-1)` entries.
    pub fn open(&self, name: &str, want: OpenFor) -> Result<IoFile, FsError> {
        let m = self.read();

        let (_, cp) = m.get_following_symlinks(&m.get_canonical_path(name));
        // Go: vfstest.go:210 open
        let opened = fstest_open(&m.m, &cp, want)?;

        match opened {
            Opened::File { file, .. } => {
                let Some(info) = convert_info(Some(file)) else {
                    // PORT: Go takes the synthesized dir branch and fails a
                    // type assertion; a file without sys cannot be in the map.
                    panic!("unexpected synthesized dir: {}", go_quote(name));
                };
                Ok(IoFile {
                    info,
                    data: (want == OpenFor::ReadFile).then(|| file.file.data.clone()),
                    entries: None,
                })
            }
            Opened::Dir { elem, file, list } => {
                let info = if let Some(info) = convert_info(file) {
                    info
                } else {
                    // This is a synthesized dir.
                    assert!(
                        name == ".",
                        "unexpected synthesized dir: {}",
                        go_quote(name)
                    );
                    let raw = map_file_info(&elem, file);
                    IoFileInfo {
                        info: FileInfo {
                            name: base_name(".").to_string(),
                            ..raw.info
                        },
                        sys: raw.sys,
                    }
                };
                // Go: vfstest.go:409 readDirFile.ReadDir
                let entries = list.map(|list| {
                    list.into_iter()
                        .map(|(elem, file)| {
                            convert_info(file).unwrap_or_else(|| {
                                panic!(
                                    "unexpected synthesized dir: {}",
                                    go_quote(&go_path_base(&elem))
                                )
                            })
                        })
                        .collect()
                });
                Ok(IoFile {
                    info,
                    data: None,
                    entries,
                })
            }
        }
    }

    // Go: vfstest.go:470 Realpath
    pub fn realpath(&self, name: &str) -> Result<String, FsError> {
        let m = self.read();

        match m.get_following_symlinks(&m.get_canonical_path(name)).0 {
            Err(err) => Err(err.into_fs_error()),
            Ok(file) => Ok(entry_realpath(file).to_string()),
        }
    }

    // Go: vfstest.go:495 MkdirAll
    pub fn mkdir_all(&self, path: &str, perm: FileMode) -> Result<(), FsError> {
        self.write().mkdir_all(path, perm)
    }

    // Go: vfstest.go:502 AddSymlink
    pub fn add_symlink(&self, path: &str, target: &str) {
        let mut m = self.write();

        let canonical = m.get_canonical_path(path);
        m.set_entry(
            path,
            canonical,
            MapFile {
                data: go_string_bytes(target).into_owned(),
                mode: FileMode::SYMLINK,
                mod_time: None,
            },
            None,
        );
    }

    // Go: vfstest.go:513 WriteFile
    // PORT: `path` is a Go `MapFS` path, without the leading '/'. `data` is
    // port form text; the map keeps its Go bytes.
    pub fn write_file(&self, path: &str, data: &str, perm: FileMode) -> Result<(), FsError> {
        self.write().write_file(path, data, perm)
    }

    // Go: vfstest.go:549 AppendFile
    pub fn append_file(&self, path: &str, data: &str, perm: FileMode) -> Result<(), FsError> {
        self.write().append_file(path, data, perm)
    }

    // Go: vfstest.go:598 Remove
    pub fn remove(&self, path: &str) -> Result<(), FsError> {
        self.write().remove(path)
    }

    // Go: vfstest.go:605 Chtimes
    // PORT: `None` is the Go zero time. The access time is not kept.
    pub fn chtimes(
        &self,
        path: &str,
        _a_time: Option<SystemTime>,
        m_time: Option<SystemTime>,
    ) -> Result<(), FsError> {
        let mut m = self.write();
        let canonical = m.get_canonical_path(path);
        let Some(file_info) = m.m.get_mut(&canonical) else {
            // file does not exist
            return Err(FsError::NotExist);
        };
        file_info.file.mod_time = m_time;
        Ok(())
    }

    // Go: vfstest.go:619 GetTargetOfSymlink
    pub fn get_target_of_symlink(&self, path: &str) -> Option<String> {
        let path = path.strip_prefix('/').unwrap_or(path);
        let m = self.read();
        let canonical = m.get_canonical_path(path);
        if let Some(file_info) = m.m.get(&canonical)
            && file_info.file.mode.intersects(FileMode::SYMLINK)
        {
            return Some(format!(
                "/{}",
                go_string_from_bytes(file_info.file.data.clone())
            ));
        }
        None
    }

    // Go: vfstest.go:633 GetModTime
    // PORT: `None` is the Go zero time.
    pub fn get_mod_time(&self, path: &str) -> Option<SystemTime> {
        let path = path.strip_prefix('/').unwrap_or(path);
        let m = self.read();
        let canonical = m.get_canonical_path(path);
        m.m.get(&canonical)
            .and_then(|file_info| file_info.file.mod_time)
    }

    // Go: vfstest.go:645 Entries
    // PORT: Go yields each entry while it holds the read lock; the port
    // returns copies in the same order.
    pub fn entries(&self) -> Vec<(String, MapFile)> {
        let m = self.read();
        let mut input_keys: Vec<&String> = m.m.keys().collect();
        input_keys.sort_by(|a, b| compare_paths_by_parts(a, b));

        input_keys
            .into_iter()
            .map(|p| {
                let file = &m.m[p];
                let mut path = entry_realpath(file).to_string();
                if !path_is_absolute(&path) {
                    path = format!("/{path}");
                }
                (path, file.file.clone())
            })
            .collect()
    }

    // Go: vfstest.go:665 GetFileInfo
    pub fn get_file_info(&self, path: &str) -> Option<MapFile> {
        let path = path.strip_prefix('/').unwrap_or(path);
        let m = self.read();
        let canonical = m.get_canonical_path(path);
        m.m.get(&canonical).map(|file_info| file_info.file.clone())
    }

    /// The whole map as plain data, for the child process protocol.
    pub fn export_state(&self) -> MapFsState {
        let m = self.read();
        MapFsState {
            use_case_sensitive_file_names: m.use_case_sensitive_file_names,
            files: m
                .m
                .iter()
                .map(|(canonical, entry)| {
                    let sys = entry.sys.clone().unwrap_or_default();
                    MapFsStateFile {
                        canonical: canonical.clone(),
                        data: entry.file.data.clone(),
                        mode: entry.file.mode.0,
                        mod_time: entry.file.mod_time,
                        realpath: sys.realpath,
                        sys_original: sys.original,
                    }
                })
                .collect(),
            symlinks: m
                .symlinks
                .iter()
                .map(|(from, to)| (from.clone(), to.clone()))
                .collect(),
        }
    }

    /// Replaces the whole map with `state` (see `export_state`). The clock
    /// stays.
    pub fn import_state(&self, state: MapFsState) {
        let mut m = self.write();
        m.use_case_sensitive_file_names = state.use_case_sensitive_file_names;
        m.m = state
            .files
            .into_iter()
            .map(|file| {
                (
                    file.canonical,
                    FsEntry {
                        file: MapFile {
                            data: file.data,
                            mode: FileMode(file.mode),
                            mod_time: file.mod_time,
                        },
                        sys: Some(Sys {
                            original: file.sys_original,
                            realpath: file.realpath,
                        }),
                    },
                )
            })
            .collect();
        m.symlinks = state.symlinks.into_iter().collect();
    }
}

/// The state of a `MapFs` as plain data (see `MapFs::export_state`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MapFsState {
    pub use_case_sensitive_file_names: bool,
    /// Sorted by `canonical`.
    pub files: Vec<MapFsStateFile>,
    /// Canonical symlink path and canonical target, sorted by path.
    pub symlinks: Vec<(String, String)>,
}

/// One map entry of a `MapFsState`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MapFsStateFile {
    /// The map key (Go `canonicalPath`, without the leading '/').
    pub canonical: String,
    /// The Go bytes.
    pub data: Vec<u8>,
    /// Go `fs.FileMode` bits.
    pub mode: u32,
    /// `None` is the Go zero time. See `unix_nanos` and `from_unix_nanos`.
    pub mod_time: Option<SystemTime>,
    /// Go `sys.realpath`.
    pub realpath: String,
    /// Go `sys.original`.
    pub sys_original: Option<i64>,
}

/// Go `t.UnixNano()` of a time, for the child process protocol.
pub fn unix_nanos(t: SystemTime) -> i64 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_nanos() as i64,
        Err(err) => -(err.duration().as_nanos() as i64),
    }
}

/// Go `time.Unix(0, nanos)`.
pub fn from_unix_nanos(nanos: i64) -> SystemTime {
    if nanos >= 0 {
        UNIX_EPOCH + Duration::from_nanos(nanos as u64)
    } else {
        UNIX_EPOCH - Duration::from_nanos(nanos.unsigned_abs())
    }
}

impl GoFs for MapFs {
    fn open(&self, name: &str, want: OpenFor) -> Result<IoFile, FsError> {
        MapFs::open(self, name, want)
    }

    // Go: vfstest.go:55 _ iovfs.RealpathFS = (*MapFS)(nil)
    fn as_realpath_fs(&self) -> Option<&dyn RealpathFs> {
        Some(self)
    }

    // Go: vfstest.go:55 _ iovfs.WritableFS = (*MapFS)(nil)
    fn as_writable_fs(&self) -> Option<&dyn WritableFs> {
        Some(self)
    }
}

impl RealpathFs for MapFs {
    fn realpath(&self, path: &str) -> Result<String, FsError> {
        MapFs::realpath(self, path)
    }
}

impl WritableFs for MapFs {
    fn write_file(&self, path: &str, data: &str, perm: FileMode) -> Result<(), FsError> {
        MapFs::write_file(self, path, data, perm)
    }

    fn append_file(&self, path: &str, data: &str, perm: FileMode) -> Result<(), FsError> {
        MapFs::append_file(self, path, data, perm)
    }

    fn mkdir_all(&self, path: &str, perm: FileMode) -> Result<(), FsError> {
        MapFs::mkdir_all(self, path, perm)
    }

    fn remove(&self, path: &str) -> Result<(), FsError> {
        MapFs::remove(self, path)
    }

    fn chtimes(
        &self,
        path: &str,
        a_time: Option<SystemTime>,
        m_time: Option<SystemTime>,
    ) -> Result<(), FsError> {
        MapFs::chtimes(self, path, a_time, m_time)
    }
}

// Go: vfstest.go:481 convertInfo
// PORT: `None` is Go `ok == false`: the file has no vfstest `sys` (a
// synthesized directory).
fn convert_info(file: Option<&FsEntry>) -> Option<IoFileInfo> {
    let file = file?;
    let sys = file.sys.as_ref()?;
    Some(IoFileInfo {
        info: FileInfo {
            // Go: vfstest.go:383 fileInfo.Name
            name: base_name(&sys.realpath).to_string(),
            size: file.file.data.len() as i64,
            mode: file.file.mode,
            mod_time: file.file.mod_time,
        },
        // Go: vfstest.go:387 fileInfo.Sys
        sys: sys.original,
    })
}

// ---------------------------------------------------------------------------
// Go standard library: testing/fstest.MapFS
// ---------------------------------------------------------------------------

// Go: testing/fstest/mapfs.go MapFS
// PORT: Go standard library. A plain map file system, with synthesized
// parent directories. iofs_test.go uses it; vfstest wraps the same `Open`.
pub struct FstestMapFs {
    m: BTreeMap<String, FsEntry>,
}

impl FstestMapFs {
    pub fn new<K: Into<String>>(files: impl IntoIterator<Item = (K, MapFile)>) -> FstestMapFs {
        FstestMapFs {
            m: files
                .into_iter()
                .map(|(path, file)| (path.into(), FsEntry { file, sys: None }))
                .collect(),
        }
    }
}

impl GoFs for FstestMapFs {
    // Go: testing/fstest/mapfs.go MapFS.Open
    fn open(&self, name: &str, want: OpenFor) -> Result<IoFile, FsError> {
        match fstest_open(&self.m, name, want)? {
            Opened::File { elem, file } => Ok(IoFile {
                info: map_file_info(&elem, Some(file)),
                data: (want == OpenFor::ReadFile).then(|| file.file.data.clone()),
                entries: None,
            }),
            Opened::Dir { elem, file, list } => Ok(IoFile {
                info: map_file_info(&elem, file),
                data: None,
                entries: list.map(|list| {
                    list.into_iter()
                        .map(|(elem, file)| map_file_info(&elem, file))
                        .collect()
                }),
            }),
        }
    }
}

// PORT: a Go `fs.File` from `fstest.MapFS.Open`. `None` is a synthesized
// directory (`&MapFile{Mode: fs.ModeDir | 0555}`).
enum Opened<'a> {
    // Go: openMapFile
    File {
        elem: String,
        file: &'a FsEntry,
    },
    // Go: mapDir. `list` is only built for `OpenFor::ReadDir`.
    Dir {
        elem: String,
        file: Option<&'a FsEntry>,
        list: Option<Vec<(String, Option<&'a FsEntry>)>>,
    },
}

// Go: testing/fstest/mapfs.go mapFileInfo
fn map_file_info(name: &str, file: Option<&FsEntry>) -> IoFileInfo {
    match file {
        Some(file) => IoFileInfo {
            info: FileInfo {
                name: go_path_base(name),
                size: file.file.data.len() as i64,
                mode: file.file.mode,
                mod_time: file.file.mod_time,
            },
            sys: file.sys.as_ref().and_then(|sys| sys.original),
        },
        None => IoFileInfo {
            info: FileInfo {
                name: go_path_base(name),
                size: 0,
                mode: FileMode::DIR | FileMode(0o555),
                mod_time: None,
            },
            sys: None,
        },
    }
}

// Go: testing/fstest/mapfs.go MapFS.Open
// Open opens the named file after following any symbolic links.
// PORT: `want` skips the directory listing when the caller does not read it.
fn fstest_open<'a>(
    fsys: &'a BTreeMap<String, FsEntry>,
    name: &str,
    want: OpenFor,
) -> Result<Opened<'a>, FsError> {
    if !io_fs_valid_path(name) {
        return Err(path_error("open", name, FsError::NotExist));
    }
    let (real_name, ok) = fstest_resolve_symlinks(fsys, name);
    if !ok {
        return Err(path_error("open", name, FsError::NotExist));
    }

    let file = fsys.get(&real_name);
    if let Some(file) = file
        && !file.file.mode.is_dir()
    {
        // Ordinary file
        return Ok(Opened::File {
            elem: go_path_base(name),
            file,
        });
    }

    // Directory, possibly synthesized.
    // Note that file can be nil here: the map need not contain explicit parent directories for all its files.
    // But file can also be non-nil, in case the user wants to set metadata for the directory explicitly.
    // Either way, we need to construct the list of children of this directory.
    let want_list = want == OpenFor::ReadDir;
    let mut list: Vec<(String, Option<&'a FsEntry>)> = Vec::new();
    let mut need: BTreeSet<String> = BTreeSet::new();
    if real_name == "." {
        if want_list {
            for (fname, f) in fsys {
                match fname.find('/') {
                    None => {
                        if fname != "." {
                            list.push((fname.clone(), Some(f)));
                        }
                    }
                    Some(i) => {
                        need.insert(fname[..i].to_string());
                    }
                }
            }
        }
    } else {
        let prefix = format!("{real_name}/");
        // PORT: a directory in the map exists without a look at its
        // children; otherwise one child is enough.
        if want_list || file.is_none() {
            for (fname, f) in fsys
                .range::<str, _>((Bound::Included(prefix.as_str()), Bound::Unbounded))
                .take_while(|(fname, _)| fname.starts_with(&prefix))
            {
                let felem = &fname[prefix.len()..];
                match felem.find('/') {
                    None => list.push((felem.to_string(), Some(f))),
                    Some(i) => {
                        need.insert(felem[..i].to_string());
                    }
                }
                if !want_list {
                    break;
                }
            }
        }
        // If the directory name is not in the map,
        // and there are no children of the name in the map,
        // then the directory is treated as not existing.
        if file.is_none() && list.is_empty() && need.is_empty() {
            return Err(path_error("open", name, FsError::NotExist));
        }
    }

    let elem = if name == "." {
        ".".to_string()
    } else {
        name[name.rfind('/').map_or(0, |i| i + 1)..].to_string()
    };

    if !want_list {
        return Ok(Opened::Dir {
            elem,
            file,
            list: None,
        });
    }

    for (fi_name, _) in &list {
        need.remove(fi_name);
    }
    for name in need {
        list.push((name, None));
    }
    list.sort_by(|a, b| compare_go_strings(&a.0, &b.0));

    Ok(Opened::Dir {
        elem,
        file,
        list: Some(list),
    })
}

// Go: testing/fstest/mapfs.go MapFS.resolveSymlinks
fn fstest_resolve_symlinks(fsys: &BTreeMap<String, FsEntry>, name: &str) -> (String, bool) {
    // Fast path: if a symlink is in the map, resolve it.
    if let Some(file) = fsys.get(name)
        && file.file.mode.type_() == FileMode::SYMLINK
    {
        let target = go_string_from_bytes(file.file.data.clone());
        if target.starts_with('/') {
            return (String::new(), false);
        }
        return fstest_resolve_symlinks(
            fsys,
            &go_path_join(&[go_path_dir(name).as_str(), target.as_str()]),
        );
    }

    // Check if each parent directory (starting at root) is a symlink.
    let mut i = 0usize;
    while i < name.len() {
        let dir = match name[i..].find('/') {
            None => {
                i = name.len();
                name
            }
            Some(j) => {
                let dir = &name[..i + j];
                i += j;
                dir
            }
        };
        if let Some(file) = fsys.get(dir)
            && file.file.mode.type_() == FileMode::SYMLINK
        {
            let target = go_string_from_bytes(file.file.data.clone());
            if target.starts_with('/') {
                return (String::new(), false);
            }
            let joined = go_path_join(&[go_path_dir(dir).as_str(), target.as_str()]);
            return fstest_resolve_symlinks(fsys, &format!("{}{}", joined, &name[i..]));
        }
        i += "/".len();
    }
    (name.to_string(), io_fs_valid_path(name))
}

// ---------------------------------------------------------------------------
// Go fmt helpers
// ---------------------------------------------------------------------------

/// Go `fmt.Sprintf("%q", s)` (`strconv.Quote`) of the port form `s`.
// PORT: a char is escaped when it is an ASCII control char or a Unicode
// control char (Go `unicode.IsPrint` also escapes some other chars).
pub fn go_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    let mut i = 0usize;
    while i < s.len() {
        let (unit, size) = go_unit_at(s, i);
        match unit {
            GoUnit::Char(c) => match c {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '\u{7}' => out.push_str("\\a"),
                '\u{8}' => out.push_str("\\b"),
                '\u{c}' => out.push_str("\\f"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                '\u{b}' => out.push_str("\\v"),
                c if (c as u32) < 0x20 || c == '\u{7f}' => {
                    out.push_str(&format!("\\x{:02x}", c as u32));
                }
                c if c.is_control() => {
                    if (c as u32) < 0x10000 {
                        out.push_str(&format!("\\u{:04x}", c as u32));
                    } else {
                        out.push_str(&format!("\\U{:08x}", c as u32));
                    }
                }
                c => out.push(c),
            },
            GoUnit::InvalidByte(_) | GoUnit::Surrogate(_) => {
                let mut bytes = Vec::new();
                unit.push_go_bytes(&mut bytes);
                for b in bytes {
                    out.push_str(&format!("\\x{b:02x}"));
                }
            }
        }
        i += size;
    }
    out.push('"');
    out
}
