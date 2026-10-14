//! The file system of the JavaScript host.
//!
//! Each file operation is one call of the import `ts_host.fs(op, ptr, len)`.
//! The request is the UTF-8 bytes at `ptr..ptr + len`. The host keeps the
//! result and returns its length. `ts_host.fs_take(ptr)` then copies the
//! result to `ptr`. When the operation fails (for example a missing file),
//! the host returns -1, or `-2 - n` when it keeps an error text of `n` bytes
//! for `fs_take`: Go's text of the error, such as `open /a.js: permission
//! denied`. `npm/wasm/core.js` is the host side.
//!
//! | op | request | result |
//! | --- | --- | --- |
//! | `Read` | path | the file bytes |
//! | `Stat` | path (links followed) | `<kind> <size> <mtime ns>`, kind `f`, `d` or `o` (other) |
//! | `ReadDir` | path | `<kind><name>` entries, each ended by NUL; kind `f`, `d`, `l` (link) or `o` |
//! | `Realpath` | path | the real path |
//! | `Write`, `Append` | path, NUL, data | empty |
//! | `Remove` | path (recursive) | empty |
//! | `Chtimes` | path, NUL, atime ns, NUL, mtime ns (empty: unchanged) | empty |

use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

use ts_goport::frontend::vfs::{
    Common, DirEntry, DirEntryInfo, Entries, FileInfo, FileMode, Fs, FsError, IoFs,
};
use ts_goport::scanner_util::go_string_bytes;

/// A host file operation (see the module table).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Read = 0,
    Stat = 1,
    ReadDir = 2,
    Realpath = 3,
    Write = 4,
    Append = 5,
    Remove = 6,
    Chtimes = 7,
}

/// Runs `op` on `request` in the host.
///
/// # Errors
///
/// When the host reports a failure. The error holds the host's error text,
/// which is empty when the host gave none.
#[cfg(target_family = "wasm")]
#[allow(unsafe_code)]
pub fn call(op: Op, request: &[u8]) -> Result<Vec<u8>, String> {
    #[link(wasm_import_module = "ts_host")]
    unsafe extern "C" {
        fn fs(op: u32, ptr: *const u8, len: usize) -> i32;
        fn fs_take(ptr: *mut u8);
    }
    // SAFETY: the host only reads the `request.len()` bytes at the pointer.
    let status = unsafe { fs(op as u32, request.as_ptr(), request.len()) };
    let take = |len: i32| {
        let mut bytes = vec![0u8; usize::try_from(len).unwrap_or(0)];
        // SAFETY: the host writes exactly `len` bytes, the length it gave.
        unsafe { fs_take(bytes.as_mut_ptr()) };
        bytes
    };
    match status {
        -1 => Err(String::new()),
        ..-1 => Err(String::from_utf8_lossy(&take(-2 - status)).into_owned()),
        _ => Ok(take(status)),
    }
}

/// Native builds (the crate's tests) have no JavaScript host: `test_host`
/// answers in its place.
///
/// # Errors
///
/// When the test host has no such file or directory, with no error text.
#[cfg(not(target_family = "wasm"))]
pub fn call(op: Op, request: &[u8]) -> Result<Vec<u8>, String> {
    test_host::call(op, request).ok_or_else(String::new)
}

/// Whether the host file system tells apart names that differ only in case.
/// Set once per instance by the request (`crate::run`); true by default, as
/// Go's osvfs assumes on wasm.
pub static CASE_SENSITIVE: OnceLock<bool> = OnceLock::new();

/// The host file system. Reads go through Go's `vfs/internal.Common`, as
/// the OS file system does, so BOM handling, text decoding and link
/// following match a native run.
pub struct HostFs {
    common: Common,
}

impl HostFs {
    #[must_use]
    pub fn new() -> Self {
        HostFs {
            common: Common {
                root_for: host_root,
                is_reparse_point: None,
            },
        }
    }
}

impl Default for HostFs {
    fn default() -> Self {
        Self::new()
    }
}

// `Common::root_for` has this type.
#[allow(clippy::unnecessary_wraps)]
fn host_root(root: &str) -> Option<Box<dyn IoFs>> {
    Some(Box::new(HostRoot(root.to_string())))
}

/// The UTF-8 bytes of a port-form path or text (see
/// `ts_goport::scanner_util::GO_STRING_MARKER`).
fn bytes(text: &str) -> Vec<u8> {
    go_string_bytes(text).into_owned()
}

/// The error of a failed host write: the host's Go error text, or a
/// general one when it gave none.
fn host_error(op: &'static str, path: &str, text: String) -> FsError {
    if text.is_empty() {
        FsError::path(op, path, std::io::Error::other("host file system error"))
    } else {
        FsError::Other(text)
    }
}

/// A time in nanoseconds since the Unix epoch. `tsc -b` compares mtimes, so
/// they keep the precision that the OS gives, as in Go.
fn time_from_ns(ns: &str) -> Option<SystemTime> {
    SystemTime::UNIX_EPOCH.checked_add(Duration::from_nanos(ns.parse().ok()?))
}

/// Parses a `Stat` result: `<kind> <size> <mtime ns>`.
fn parse_stat(name: &str, result: &[u8]) -> Option<FileInfo> {
    let text = std::str::from_utf8(result).ok()?;
    let mut parts = text.split(' ');
    let mode = match parts.next()? {
        "d" => FileMode::DIR | FileMode(0o755),
        "f" => FileMode(0o644),
        // A FIFO or a device: Go's stat gives a file that is not a
        // directory, so it exists as a file.
        "o" => FileMode::IRREGULAR | FileMode(0o644),
        _ => return None,
    };
    let size = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let mod_time = parts.next().and_then(time_from_ns);
    Some(FileInfo {
        name: name.to_string(),
        size,
        mode,
        mod_time,
    })
}

/// One root of the host file system (`/`, or `c:/` on Windows), for
/// `Common`.
struct HostRoot(String);

impl HostRoot {
    /// The full path of `name`, which `Common` gives relative to the root
    /// (`.` for the root itself).
    fn path(&self, name: &str) -> String {
        if name == "." {
            self.0.clone()
        } else {
            format!("{}{name}", self.0)
        }
    }
}

impl IoFs for HostRoot {
    fn stat(&self, name: &str) -> Result<FileInfo, FsError> {
        let base = name.rsplit('/').next().unwrap_or(name);
        call(Op::Stat, &bytes(&self.path(name)))
            .ok()
            .and_then(|result| parse_stat(base, &result))
            .ok_or(FsError::NotExist)
    }

    fn read_dir(&self, name: &str) -> Result<Vec<DirEntry>, FsError> {
        let result = call(Op::ReadDir, &bytes(&self.path(name))).map_err(|_| FsError::NotExist)?;
        let text = String::from_utf8_lossy(&result);
        let mut entries: Vec<DirEntry> = text
            .split('\0')
            .filter(|entry| entry.len() > 1)
            .map(|entry| {
                let (kind, name) = entry.split_at(1);
                let typ = match kind {
                    "d" => FileMode::DIR,
                    "l" => FileMode::SYMLINK,
                    "o" => FileMode::IRREGULAR,
                    _ => FileMode(0),
                };
                DirEntry {
                    name: name.to_string(),
                    typ,
                    info: DirEntryInfo::Known(FileInfo {
                        name: name.to_string(),
                        mode: typ,
                        ..FileInfo::default()
                    }),
                }
            })
            .collect();
        // Go's os.ReadDir sorts by name.
        ts_goport::gostd::slices::stable_sort_by(&mut entries, |a, b| a.name.cmp(&b.name));
        Ok(entries)
    }

    fn read_file(&self, name: &str) -> Result<Vec<u8>, FsError> {
        call(Op::Read, &bytes(&self.path(name))).map_err(|_| FsError::NotExist)
    }
}

/// `path`, NUL, `data`: the request of `Write` and `Append`.
fn path_and_data(path: &str, data: &str) -> Vec<u8> {
    let mut request = bytes(path);
    request.push(0);
    request.extend_from_slice(&go_string_bytes(data));
    request
}

fn ns_since_epoch(time: Option<SystemTime>) -> String {
    time.and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos().to_string())
        .unwrap_or_default()
}

impl Fs for HostFs {
    fn use_case_sensitive_file_names(&self) -> bool {
        *CASE_SENSITIVE.get_or_init(|| true)
    }

    fn file_exists(&self, path: &str) -> bool {
        self.common.file_exists(path)
    }

    fn read_file(&self, path: &str) -> (String, bool) {
        self.common.read_file(path)
    }

    fn write_file(&self, path: &str, data: &str) -> Result<(), FsError> {
        call(Op::Write, &path_and_data(path, data))
            .map(drop)
            .map_err(|text| host_error("open", path, text))
    }

    fn append_file(&self, path: &str, data: &str) -> Result<(), FsError> {
        call(Op::Append, &path_and_data(path, data))
            .map(drop)
            .map_err(|text| host_error("open", path, text))
    }

    fn remove(&self, path: &str) -> Result<(), FsError> {
        call(Op::Remove, &bytes(path))
            .map(drop)
            .map_err(|text| host_error("remove", path, text))
    }

    fn chtimes(
        &self,
        path: &str,
        a_time: Option<SystemTime>,
        m_time: Option<SystemTime>,
    ) -> Result<(), FsError> {
        let request = format!(
            "{}\0{}\0{}",
            String::from_utf8_lossy(&bytes(path)),
            ns_since_epoch(a_time),
            ns_since_epoch(m_time)
        );
        call(Op::Chtimes, request.as_bytes())
            .map(drop)
            .map_err(|text| host_error("chtimes", path, text))
    }

    fn directory_exists(&self, path: &str) -> bool {
        self.common.directory_exists(path)
    }

    fn get_accessible_entries(&self, path: &str) -> Entries {
        self.common.get_accessible_entries(path)
    }

    fn stat(&self, path: &str) -> Option<FileInfo> {
        self.common.stat(path)
    }

    fn realpath(&self, path: &str) -> String {
        call(Op::Realpath, &bytes(path)).ok().map_or_else(
            || path.to_string(),
            ts_goport::scanner_util::go_string_from_bytes,
        )
    }
}

/// An in-memory host for native tests: a map from absolute path to bytes.
/// Directories are the parents of the files.
#[cfg(not(target_family = "wasm"))]
pub mod test_host {
    use super::Op;
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Mutex, PoisonError};

    static FILES: Mutex<BTreeMap<String, Vec<u8>>> = Mutex::new(BTreeMap::new());

    /// Bumped at each `set_files`: the mtime that `Stat` reports (in
    /// nanoseconds), so a file that a later `set_files` wrote is a parse
    /// cache miss (`program::multirun`), even when its size is the same.
    static GENERATION: AtomicU64 = AtomicU64::new(1);

    /// Replaces the files of the test host.
    pub fn set_files(files: impl IntoIterator<Item = (String, String)>) {
        GENERATION.fetch_add(1, Ordering::Relaxed);
        let mut map = FILES.lock().unwrap_or_else(PoisonError::into_inner);
        *map = files
            .into_iter()
            .map(|(path, text)| (path, text.into_bytes()))
            .collect();
    }

    /// The text of `path` in the test host.
    #[must_use]
    pub fn file(path: &str) -> Option<String> {
        let map = FILES.lock().unwrap_or_else(PoisonError::into_inner);
        map.get(path)
            .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
    }

    fn dir_prefix(path: &str) -> String {
        if path.ends_with('/') {
            path.to_string()
        } else {
            format!("{path}/")
        }
    }

    pub(super) fn call(op: Op, request: &[u8]) -> Option<Vec<u8>> {
        let request = String::from_utf8_lossy(request);
        let mut map = FILES.lock().unwrap_or_else(PoisonError::into_inner);
        let (path, data) = request.split_once('\0').unwrap_or((&request, ""));
        let prefix = dir_prefix(path);
        let is_dir = map.keys().any(|key| key.starts_with(&prefix));
        match op {
            Op::Read => map.get(path).cloned(),
            Op::Stat => match map.get(path) {
                Some(bytes) => Some(
                    format!("f {} {}", bytes.len(), GENERATION.load(Ordering::Relaxed))
                        .into_bytes(),
                ),
                None => is_dir
                    .then(|| format!("d 0 {}", GENERATION.load(Ordering::Relaxed)).into_bytes()),
            },
            Op::ReadDir => {
                if !is_dir {
                    return None;
                }
                let mut names: Vec<String> = Vec::new();
                for key in map.keys().filter(|key| key.starts_with(&prefix)) {
                    let entry = match key[prefix.len()..].split_once('/') {
                        Some((dir, _)) => format!("d{dir}"),
                        None => format!("f{}", &key[prefix.len()..]),
                    };
                    if names.last() != Some(&entry) {
                        names.push(entry);
                    }
                }
                let mut result = Vec::new();
                for name in names {
                    result.extend_from_slice(name.as_bytes());
                    result.push(0);
                }
                Some(result)
            }
            Op::Realpath => (map.contains_key(path) || is_dir).then(|| path.as_bytes().to_vec()),
            Op::Write => {
                map.insert(path.to_string(), data.as_bytes().to_vec());
                Some(Vec::new())
            }
            Op::Append => {
                map.entry(path.to_string())
                    .or_default()
                    .extend_from_slice(data.as_bytes());
                Some(Vec::new())
            }
            Op::Remove => {
                map.retain(|key, _| key != path && !key.starts_with(&prefix));
                Some(Vec::new())
            }
            Op::Chtimes => Some(Vec::new()),
        }
    }
}
