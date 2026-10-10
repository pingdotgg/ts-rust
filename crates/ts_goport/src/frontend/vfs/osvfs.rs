//! Go: internal/vfs/osvfs/os.go. The realpath and symlink helpers it calls
//! are in `frontend::nativepath`. Off Linux: the Windows branches of os.go
//! and the Windows `path/filepath` pieces (`FromSlash`, `Abs`, `Clean`) are
//! ported; the other targets use the unix ones.
//!
//! The Go standard library pieces that osvfs reaches (`os.DirFS`,
//! `os.RemoveAll`, `filepath.Abs`, `filepath.Clean`) are ported here too.

use crate::frontend::prelude::*;
use std::borrow::Cow;
use std::cell::OnceCell;
use std::ffi::{OsStr, OsString};
use std::io::{self, Write as _};
#[cfg(unix)]
use std::os::unix::ffi::{OsStrExt, OsStringExt};
#[cfg(unix)]
use std::os::unix::fs::FileTypeExt;
use std::path::{Path as OsPath, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::SystemTime;

// PORT: a Go path is a Go string, so it can hold any bytes, and Go passes
// those bytes to the OS unchanged. A port string is the port form of a Go
// string (see `scanner_util::GO_STRING_MARKER`). `os_path` gives the OS the
// Go bytes, and `go_string_from_os` turns OS bytes (file names, link
// targets, the working directory, arguments) into the port form. Every OS
// call in the port goes through them.

/// The OS path of the port form path `path`: its Go bytes.
#[cfg(unix)]
pub fn os_path(path: &str) -> Cow<'_, OsPath> {
    match crate::scanner_util::go_string_bytes(path) {
        Cow::Borrowed(bytes) => Cow::Borrowed(OsPath::new(OsStr::from_bytes(bytes))),
        Cow::Owned(bytes) => Cow::Owned(PathBuf::from(OsString::from_vec(bytes))),
    }
}

/// The value form of the OS string `s` (see `os_path` and
/// `scanner_util::go_value_from_bytes`).
#[cfg(unix)]
pub fn go_string_from_os(s: impl Into<OsString>) -> String {
    match String::from_utf8(s.into().into_vec()) {
        Ok(text) => crate::scanner_util::go_string_from_utf8(text),
        Err(err) => crate::scanner_util::go_value_from_bytes(err.as_bytes()).into_owned(),
    }
}

// PORT divergence: off unix an OS path is text (UTF-16 on Windows), and the
// port converts it lossily. Go on Windows uses WTF-8 (go1.27.1
// syscall/wtf8_windows.go): `UTF16ToString` keeps an unpaired surrogate as
// its 3-byte WTF-8 form, and `UTF16FromString` turns those 3 bytes back into
// the surrogate, so a name read from the OS goes back to the OS unchanged.
// The port makes an unpaired surrogate U+FFFD. For other bytes that are not
// UTF-8, Go makes each byte U+FFFD, and `from_utf8_lossy` makes each bad
// sequence U+FFFD. A port of Go's form would use `encode_wide` and
// `from_wide` (`std::os::windows::ffi`). Not run on such a target.
#[cfg(not(unix))]
pub fn os_path(path: &str) -> Cow<'_, OsPath> {
    match crate::scanner_util::go_string_bytes(path) {
        Cow::Borrowed(_) => Cow::Borrowed(OsPath::new(path)),
        Cow::Owned(bytes) => {
            Cow::Owned(PathBuf::from(String::from_utf8_lossy(&bytes).into_owned()))
        }
    }
}

#[cfg(not(unix))]
pub fn go_string_from_os(s: impl Into<OsString>) -> String {
    crate::scanner_util::go_string_from_utf8(s.into().to_string_lossy().into_owned())
}

/// The process arguments after the program name, in the port form (Go
/// `os.Args[1:]`, see `os_path`).
pub fn os_args() -> Vec<String> {
    std::env::args_os().skip(1).map(go_string_from_os).collect()
}

/// The current directory in the port form (Go `os.Getwd`, see `os_path`).
/// With an OS override installed, the override's directory.
/// `getwd_error_text` gives the Go text of an error.
// Go: os/getwd.go:26 Getwd (go1.27.1)
// PORT: on unix, `$PWD` when it is absolute and names the same file as "."
// (Go `SameFile`: the same device and inode), so a directory reached
// through a symlink keeps the link path. Else `syscall.Getwd` is
// `std::env::current_dir` (getcwd). Go's own walk up the parents, for a
// getcwd that fails with ENAMETOOLONG, is not ported: glibc's getcwd makes
// the same walk. On Windows Go calls `syscall.Getwd` only.
pub fn os_current_dir() -> io::Result<String> {
    if let Some(o) = OS_OVERRIDE.get() {
        return Ok(o.current_directory.clone());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // Clumsy but widespread kludge:
        // if $PWD is set and matches ".", use it.
        if let Some(dir) = std::env::var_os("PWD")
            && dir.as_bytes().first() == Some(&b'/')
        {
            // Go returns the `*PathError` of this stat as it is.
            let dot = std::fs::metadata(".").map_err(|err| {
                let text = format!("stat .: {}", crate::fswatch::syscall::io_error_text(&err));
                io::Error::new(err.kind(), text)
            })?;
            if let Ok(d) = std::fs::metadata(&dir)
                && d.dev() == dot.dev()
                && d.ino() == dot.ino()
            {
                return Ok(go_string_from_os(dir));
            }
        }
    }
    std::env::current_dir().map(go_string_from_os)
}

/// Go `err.Error()` of an `os_current_dir` error: "getwd: <errno text>"
/// (`*os.SyscallError`) when getcwd failed, or "stat .: <errno text>"
/// (`*os.PathError`) when the stat of "." failed.
pub fn getwd_error_text(err: &io::Error) -> String {
    match err.raw_os_error() {
        Some(_) => format!("getwd: {}", crate::fswatch::syscall::io_error_text(err)),
        None => err.to_string(),
    }
}

// PORT: Go reads files and the current directory through `sys.FS()` and
// `sys.GetCurrentDirectory()`, so a Go test swaps the whole OS for its
// `TestSys`. The port's file system is `Rc`, so the parse, checker and emit
// threads cannot share `sys.FS()`: they call `osvfs_fs()` and
// `os_current_dir()` directly. A test process installs an `OsOverride` once
// at start, and those calls then reach the test file system and directory.
// Only test processes install it; a real run never does and keeps the OS
// behavior below unchanged.

/// The file system and current directory that replace the OS in a test
/// process (see `install_os_override`).
pub struct OsOverride {
    /// Makes the file system for one thread. `osvfs_fs` calls it once per
    /// thread. Each value must share one state (for example a map behind
    /// an `Arc<Mutex>`), so that all threads see the same files. It must
    /// not call `osvfs_fs` itself.
    pub fs: Arc<dyn Fn() -> Rc<dyn Fs> + Send + Sync>,
    /// The value of `os_current_dir`.
    pub current_directory: String,
}

static OS_OVERRIDE: OnceLock<OsOverride> = OnceLock::new();

/// Replaces the OS file system and current directory for the rest of the
/// process. Install it before the first `osvfs_fs` or `os_current_dir`
/// call. Panics when an override is already installed.
pub fn install_os_override(o: OsOverride) {
    assert!(
        OS_OVERRIDE.set(o).is_ok(),
        "osvfs: an OS override is already installed"
    );
}

/// True when `install_os_override` has run in this process.
pub fn os_override_installed() -> bool {
    OS_OVERRIDE.get().is_some()
}

// PORT: the Go semaphores `blockingOpSema`, `readSema` and `writeSema`
// (os.go:20) limit concurrent syscalls. The port is single-threaded, so they
// are not ported.

// Go: os.go:30 FS
// FS creates a new FS from the OS file system.
// PORT: the Go package function `osvfs.FS` is `osvfs_fs`. Go returns one
// package-level value; this returns a clone of one per-thread value. With
// an OS override installed (a test process), the per-thread value is the
// one that the override's `fs` makes on the first call on that thread.
pub fn osvfs_fs() -> Rc<dyn Fs> {
    thread_local! {
        // Go: os.go:34 osVFS
        static OS_VFS: Rc<dyn Fs> = Rc::new(OsFs {
            common: Common {
                root_for: os_dir_fs,
                is_reparse_point: Some(is_reparse_point),
            },
        });
        static OVERRIDE_FS: OnceCell<Rc<dyn Fs>> = const { OnceCell::new() };
    }
    if let Some(o) = OS_OVERRIDE.get() {
        return OVERRIDE_FS.with(|cell| Rc::clone(cell.get_or_init(|| (o.fs)())));
    }
    OS_VFS.with(Rc::clone)
}

// Go: os.go:138 isReparsePoint
fn is_reparse_point(path: &str) -> bool {
    crate::frontend::nativepath::is_symlink_or_reparse_point(&filepath_from_slash(path))
}

// Go: os.go:41 osFS
pub struct OsFs {
    common: Common,
}

// Go: os.go:46 isFileSystemCaseSensitive
// We do this right at startup to minimize the chance that executable gets moved or deleted.
// PORT: Go computes this in package init. The port computes it on first use.
fn is_file_system_case_sensitive() -> bool {
    static VALUE: OnceLock<bool> = OnceLock::new();
    *VALUE.get_or_init(|| {
        // win32/win64 are case insensitive platforms
        if cfg!(windows) {
            return false;
        }

        if cfg!(target_family = "wasm") {
            // !!! Who knows; this depends on the host implementation.
            return true;
        }

        // As a proxy for case-insensitivity, we check if the current executable exists under a different case.
        // This is not entirely correct, since different OSs can have differing case sensitivity in different paths,
        // but this is largely good enough for our purposes (and what sys.ts used to do with __filename).
        let exe = match crate::frontend::osutil::executable() {
            Ok(exe) => exe,
            Err(err) => panic!("vfs: failed to get executable path: {err}"),
        };

        // If the current executable exists under a different case, we must be case-insensitive.
        let swapped = swap_case(&exe);
        if let Err(err) = std::fs::metadata(os_path(&swapped)) {
            if err.kind() == io::ErrorKind::NotFound {
                return true;
            }
            panic!("vfs: failed to stat {swapped:?}: {err}");
        }
        false
    })
}

// Go: os.go:77 swapCase
// Convert all lowercase chars to uppercase, and vice-versa
fn swap_case(str: &str) -> String {
    str.chars()
        .map(|r| {
            let upper = simple_to_upper(r);
            if upper == r {
                simple_to_lower(r)
            } else {
                upper
            }
        })
        .collect()
}

// PORT: Go `unicode.ToUpper` uses the simple one-rune case mapping. Rust
// only has the full mapping; a mapping to more than one char is treated as
// no simple mapping.
fn simple_to_upper(r: char) -> char {
    let mut it = r.to_uppercase();
    match (it.next(), it.next()) {
        (Some(c), None) => c,
        _ => r,
    }
}

// PORT: Go `unicode.ToLower`; see `simple_to_upper`.
fn simple_to_lower(r: char) -> char {
    let mut it = r.to_lowercase();
    match (it.next(), it.next()) {
        (Some(c), None) => c,
        _ => r,
    }
}

impl Fs for OsFs {
    // Go: os.go:88 UseCaseSensitiveFileNames
    fn use_case_sensitive_file_names(&self) -> bool {
        is_file_system_case_sensitive()
    }

    // Go: os.go:92 ReadFile
    fn read_file(&self, path: &str) -> (String, bool) {
        self.common.read_file(path)
    }

    // Go: os.go:97 DirectoryExists
    fn directory_exists(&self, path: &str) -> bool {
        self.common.directory_exists(path)
    }

    // Go: os.go:102 FileExists
    fn file_exists(&self, path: &str) -> bool {
        self.common.file_exists(path)
    }

    // Go: os.go:107 GetAccessibleEntries
    fn get_accessible_entries(&self, path: &str) -> Entries {
        self.common.get_accessible_entries(path)
    }

    // Go: os.go:112 Stat
    fn stat(&self, path: &str) -> Option<FileInfo> {
        self.common.stat(path)
    }

    // Go: os.go:117 Realpath
    fn realpath(&self, path: &str) -> String {
        os_fs_realpath(path)
    }

    // Go: os.go:174 WriteFile
    fn write_file(&self, path: &str, content: &str) -> Result<(), FsError> {
        self.write_file_ensuring_dir(path, content, WriteFlag::Truncate)
    }

    // Go: os.go:178 AppendFile
    fn append_file(&self, path: &str, content: &str) -> Result<(), FsError> {
        self.write_file_ensuring_dir(path, content, WriteFlag::Append)
    }

    // Go: os.go:182 Remove
    fn remove(&self, path: &str) -> Result<(), FsError> {
        // todo: #701 add retry mechanism?
        os_remove_all(path)
    }

    // Go: os.go:188 Chtimes
    // PORT: Go `os.Chtimes` is utimensat(AT_FDCWD, path, times, 0) on unix,
    // so it works on a file without read permission; a zero Go time
    // (`None` here) is UTIME_OMIT. Off unix, Rust std sets times through an
    // open file (opened read-only here).
    #[cfg(unix)]
    fn chtimes(
        &self,
        path: &str,
        a_time: Option<SystemTime>,
        m_time: Option<SystemTime>,
    ) -> Result<(), FsError> {
        use rustix::fs::{AtFlags, CWD, Timespec, Timestamps, UTIME_OMIT};
        // Go: syscall.NsecToTimespec(t.UnixNano())
        let timespec = |time: Option<SystemTime>| match time {
            None => Timespec {
                tv_sec: 0,
                tv_nsec: UTIME_OMIT,
            },
            Some(time) => {
                let (sec, nsec) = match time.duration_since(SystemTime::UNIX_EPOCH) {
                    Ok(d) => (d.as_secs() as i64, i64::from(d.subsec_nanos())),
                    Err(err) => {
                        let d = err.duration();
                        let (sec, nsec) = (-(d.as_secs() as i64), -i64::from(d.subsec_nanos()));
                        if nsec < 0 {
                            (sec - 1, nsec + 1_000_000_000)
                        } else {
                            (sec, nsec)
                        }
                    }
                };
                Timespec {
                    tv_sec: sec,
                    tv_nsec: nsec as _,
                }
            }
        };
        let times = Timestamps {
            last_access: timespec(a_time),
            last_modification: timespec(m_time),
        };
        rustix::fs::utimensat(CWD, &*os_path(path), &times, AtFlags::empty())
            .map_err(|err| FsError::path("chtimes", path, io::Error::from(err)))
    }

    #[cfg(not(unix))]
    fn chtimes(
        &self,
        path: &str,
        a_time: Option<SystemTime>,
        m_time: Option<SystemTime>,
    ) -> Result<(), FsError> {
        let file = std::fs::File::open(os_path(path))
            .map_err(|err| FsError::path("chtimes", path, err))?;
        let mut times = std::fs::FileTimes::new();
        if let Some(a_time) = a_time {
            times = times.set_accessed(a_time);
        }
        if let Some(m_time) = m_time {
            times = times.set_modified(m_time);
        }
        file.set_times(times)
            .map_err(|err| FsError::path("chtimes", path, err))
    }
}

/// PORT: not in Go (perf). None (not read) for each normalized absolute
/// path of `paths` whose name is a symbolic link, and `Some` of the mtime
/// for every other path, on the OS file system, as `osvfs_fs().stat(path)`
/// gives it (Go os.go:112 Stat, `os.Stat`; `FileInfo::mod_time`, None when
/// the stat fails). The `tsc -b` prefetch (build_task.rs `StatusPrefetch`)
/// leaves a link to the check: its target can be an output that a task of
/// the build writes before the check. A run of paths in one directory is
/// stat'ed through one open of the directory, so the OS walks the
/// directory part of the path once. A path that this cannot stat so (no
/// directory part, a directory that does not open, a failed stat, a path
/// of `PATH_MAX` bytes or more) is stat'ed by its full path, as `stat`
/// does, unless an `lstat` finds that its name is a link.
pub fn os_mod_times_of_non_links<'a>(
    paths: impl IntoIterator<Item = &'a str>,
) -> Vec<Option<Option<SystemTime>>> {
    mod_times(paths)
}

/// `os_mod_times_of_non_links`.
#[cfg(target_os = "linux")]
fn mod_times<'a>(paths: impl IntoIterator<Item = &'a str>) -> Vec<Option<Option<SystemTime>>> {
    use rustix::fs::{AtFlags, CWD, FileType, Mode, OFlags, StatxFlags, openat, statx};
    use std::os::fd::OwnedFd;
    const PATH_MAX: usize = 4096;
    // `SystemTime::new` of std (`Metadata::modified`), which is not public.
    let system_time = |sec: i64, nsec: u32| {
        if nsec >= 1_000_000_000 {
            return None;
        }
        let base = if sec >= 0 {
            SystemTime::UNIX_EPOCH.checked_add(std::time::Duration::from_secs(sec.unsigned_abs()))
        } else {
            SystemTime::UNIX_EPOCH.checked_sub(std::time::Duration::from_secs(sec.unsigned_abs()))
        };
        base?.checked_add(std::time::Duration::from_nanos(u64::from(nsec)))
    };
    let mut dir: Option<(&str, Option<OwnedFd>)> = None;
    let mut m_times = Vec::new();
    for path in paths {
        let mut m_time = None;
        let mut link = false;
        if let Some(slash) = path.rfind('/')
            && path.len() < PATH_MAX
        {
            let (dir_name, name) = (&path[..slash.max(1)], &path[slash + 1..]);
            if dir.as_ref().is_none_or(|(open, _)| *open != dir_name) {
                let flags = OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC;
                let fd = openat(CWD, &*os_path(dir_name), flags, Mode::empty()).ok();
                dir = Some((dir_name, fd));
            }
            let wanted = StatxFlags::TYPE | StatxFlags::MTIME;
            if let Some((_, Some(fd))) = &dir
                && !name.is_empty()
                && name != "."
                && name != ".."
                && let Ok(stat) = statx(fd, &*os_path(name), AtFlags::SYMLINK_NOFOLLOW, wanted)
                && stat.stx_mask & wanted.bits() == wanted.bits()
            {
                if FileType::from_raw_mode(u32::from(stat.stx_mode)) == FileType::Symlink {
                    link = true;
                } else {
                    m_time = system_time(stat.stx_mtime.tv_sec, stat.stx_mtime.tv_nsec).map(Some);
                }
            }
        }
        if m_time.is_none()
            && (link || std::fs::symlink_metadata(os_path(path)).is_ok_and(|md| md.is_symlink()))
        {
            m_times.push(None);
            continue;
        }
        m_times.push(Some(m_time.unwrap_or_else(|| {
            std::fs::metadata(os_path(path))
                .ok()
                .and_then(|md| md.modified().ok())
        })));
    }
    m_times
}

/// PORT: not in Go (perf). Off Linux, `stat` of each path after an
/// `lstat`.
#[cfg(not(target_os = "linux"))]
fn mod_times<'a>(paths: impl IntoIterator<Item = &'a str>) -> Vec<Option<Option<SystemTime>>> {
    let fs = osvfs_fs();
    paths
        .into_iter()
        .map(|path| {
            if std::fs::symlink_metadata(os_path(path)).is_ok_and(|md| md.is_symlink()) {
                return None;
            }
            Some(fs.stat(path).and_then(|stat| stat.mod_time()))
        })
        .collect()
}

// PORT: the Go `flag int` of `writeFileWithFlag`. Go passes
// `O_WRONLY|O_CREATE|O_TRUNC` or `O_WRONLY|O_CREATE|O_APPEND`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum WriteFlag {
    Truncate,
    Append,
}

// Go: os.go:122 osFSRealpath
pub fn os_fs_realpath(path: &str) -> String {
    let _ = root_length(path); // Assert path is rooted

    let orig = path;
    let path = filepath_from_slash(path);
    let path = match crate::frontend::nativepath::realpath(&path) {
        Ok(path) => path,
        Err(_) => return orig.to_string(),
    };
    let path = match filepath_abs(&path) {
        Ok(path) => path,
        Err(_) => return orig.to_string(),
    };
    normalize_slashes(&path).into()
}

impl OsFs {
    // Go: os.go:142 writeFileWithFlag
    fn write_file_with_flag(
        &self,
        path: &str,
        content: &str,
        flag: WriteFlag,
    ) -> Result<(), FsError> {
        // PORT: Go's perm 0o666 is the std default create mode on unix.
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true);
        match flag {
            WriteFlag::Truncate => options.truncate(true),
            WriteFlag::Append => options.append(true),
        };
        let mut file = options
            .open(os_path(path))
            .map_err(|err| FsError::path("open", path, err))?;

        // PORT: Go writes the string bytes unchanged. `content` is the port
        // form of the Go string (see `scanner_util::GO_STRING_MARKER`), so
        // write its Go bytes.
        file.write_all(&crate::scanner_util::go_string_bytes(content))
            .map_err(|err| FsError::path("write", path, err))?;

        Ok(())
    }

    // Go: os.go:158 ensureDirectoryExists
    fn ensure_directory_exists(&self, directory_path: &str) -> Result<(), FsError> {
        os_mkdir_all(directory_path, 0o777)
    }

    // Go: os.go:163 writeFileEnsuringDir
    fn write_file_ensuring_dir(
        &self,
        path: &str,
        content: &str,
        flag: WriteFlag,
    ) -> Result<(), FsError> {
        let _ = root_length(path); // Assert path is rooted
        if self.write_file_with_flag(path, content, flag).is_ok() {
            return Ok(());
        }
        let normalized: String = normalize_path(path).into();
        let directory: String = get_directory_path(&normalized).into();
        self.ensure_directory_exists(&directory)?;
        self.write_file_with_flag(path, content, flag)
    }
}

// Go: os.go:193 GetGlobalTypingsCacheLocation
// PORT: not ported. Only the language server (cmd/tsc/lsp.go) calls it.

// Go: os/file.go DirFS
// PORT: Go standard library. `os.DirFS(dir)` with the `Stat`, `ReadDir`
// and `ReadFile` methods that `io/fs` helpers use.
pub fn os_dir_fs(dir: &str) -> Option<Box<dyn IoFs>> {
    Some(Box::new(DirFs {
        dir: dir.to_string(),
    }))
}

// Go: os/file.go dirFS
pub struct DirFs {
    dir: String,
}

impl DirFs {
    // Go: os/file.go dirFS.join
    // PORT: Go `filepathlite.Localize` on Unix is `fs.ValidPath` plus a
    // NUL byte check. On Windows it also refuses the names that
    // `windows_localize` refuses, and Go writes the name with `\`. The port
    // keeps `/`, which Windows reads as `\` in a path without the `\\?\`
    // prefix.
    fn join(&self, name: &str) -> Result<String, FsError> {
        if self.dir.is_empty() {
            return Err(FsError::Other("os: DirFS with empty root".to_string()));
        }
        if !io_fs_valid_path(name) || name.contains('\0') {
            return Err(FsError::Invalid);
        }
        #[cfg(windows)]
        if !windows_localize(name) {
            return Err(FsError::Invalid);
        }
        if self.dir.ends_with('/') {
            return Ok(format!("{}{}", self.dir, name));
        }
        Ok(format!("{}/{}", self.dir, name))
    }
}

// Go: internal/filepathlite/path_windows.go:56 localize
// PORT: only its checks: false where Go returns `errInvalidPath`. A rooted
// path that is not a drive root keeps its drive in the name, for example
// `C:/a` under the root `//?/` of `\\?\C:\a`, so Go cannot open it.
#[cfg(windows)]
fn windows_localize(path: &str) -> bool {
    !path.contains([':', '\\', '\0']) && !path.split('/').any(is_reserved_name)
}

// Go: internal/filepathlite/path_windows.go:98 isReservedName
// PORT: Go asks `RtlIsDosDeviceName_U` whether a reserved name with an
// extension is reserved on this Windows (since Windows 11 `CON.txt` is not).
// The port asks `GetFullPathNameW` (`std::path::absolute`), which gives a
// `\\.\` device path for such a name.
#[cfg(windows)]
fn is_reserved_name(name: &str) -> bool {
    // Device names can have arbitrary trailing characters following a dot or colon.
    let base = &name[..name.find([':', '.']).unwrap_or(name.len())];
    // Trailing spaces in the last path element are ignored.
    let base = base.trim_end_matches(' ');
    if !is_reserved_base_name(base) {
        return false;
    }
    if base.len() == name.len() {
        return true;
    }
    std::path::absolute(name).is_ok_and(|p| p.to_string_lossy().starts_with(r"\\.\"))
}

// Go: internal/filepathlite/path_windows.go:128 isReservedBaseName
#[cfg(windows)]
fn is_reserved_base_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    if bytes.len() >= 3 {
        let upper = bytes[..3].to_ascii_uppercase();
        if bytes.len() == 3 && matches!(&upper[..], b"CON" | b"PRN" | b"AUX" | b"NUL") {
            return true;
        }
        if bytes.len() >= 4 && matches!(&upper[..], b"COM" | b"LPT") {
            if bytes.len() == 4 && (b'1'..=b'9').contains(&bytes[3]) {
                return true;
            }
            // Superscript ¹, ², and ³ are considered numbers as well.
            return matches!(&name[3..], "\u{b2}" | "\u{b3}" | "\u{b9}");
        }
    }
    // Passing CONIN$ or CONOUT$ to CreateFile opens a console handle.
    name.eq_ignore_ascii_case("CONIN$") || name.eq_ignore_ascii_case("CONOUT$")
}

impl IoFs for DirFs {
    // Go: os/file.go dirFS.Stat
    fn stat(&self, name: &str) -> Result<FileInfo, FsError> {
        let fullname = self.join(name)?;
        // Go os.Stat follows symlinks.
        match std::fs::metadata(os_path(&fullname)) {
            Ok(md) => Ok(file_info_from_metadata(basename(&fullname), &md)),
            Err(err) => Err(FsError::path("stat", name, err)),
        }
    }

    // Go: os/file.go dirFS.ReadDir
    fn read_dir(&self, name: &str) -> Result<Vec<DirEntry>, FsError> {
        let fullname = self.join(name)?;
        os_read_dir(&fullname).map_err(|err| FsError::path("readdirent", name, err))
    }

    // Go: os/file.go dirFS.ReadFile
    fn read_file(&self, name: &str) -> Result<Vec<u8>, FsError> {
        let fullname = self.join(name)?;
        std::fs::read(os_path(&fullname)).map_err(|err| FsError::path("open", name, err))
    }
}

// Go: os/dir.go ReadDir
// PORT: Go standard library. Returns the entries sorted by file name. The
// entry type comes from the directory entry (d_type) and does not follow
// symlinks. Go skips an entry that is removed before its `lstat`; so does
// the port. Go returns the entries read before an error; the port returns
// only the error (callers here drop the entries on error).
fn os_read_dir(dirname: &str) -> io::Result<Vec<DirEntry>> {
    // PERF (cfgwalk1): the entries share one directory path for `lstat`
    // (`DirEntryInfo::Lstat`).
    let dir: Rc<str> = Rc::from(dirname);
    let mut entries = read_dir_entries(dirname, &dir)?;
    // Go sorts by the name bytes.
    // Go: os/dir.go:122 ReadDir: slices.SortFunc(dirs, bytealg.CompareString on the names)
    // PERF (cfgwalk1): names without a marker are their Go bytes (see
    // `scanner_util::GO_STRING_MARKER`). The names of a directory differ,
    // so any sort gives Go's order.
    if entries
        .iter()
        .any(|e| crate::scanner_util::contains_go_string_marker(&e.name))
    {
        crate::gostd::slices::sort_func(&mut entries, |a, b| {
            crate::scanner_util::go_string_bytes(&a.name)
                .cmp(&crate::scanner_util::go_string_bytes(&b.name)) as i32
        });
    } else {
        entries.sort_unstable_by(|a, b| a.name.as_bytes().cmp(b.name.as_bytes()));
    }
    Ok(entries)
}

/// The entries of `os_read_dir`, not sorted.
/// PERF (cfgwalk1): on Linux, `getdents64` into one buffer, as Go does. std
/// `read_dir` also calls `fstat` for each directory (glibc `opendir`) and
/// copies each name twice.
#[cfg(target_os = "linux")]
fn read_dir_entries(dirname: &str, dir: &Rc<str>) -> io::Result<Vec<DirEntry>> {
    read_dir_entries_typed(dirname, dir, |_, file_type| file_type)
}

/// `read_dir_entries`, with the type of each entry from
/// `d_type(name, file type of its d_type)`. A test gives
/// `FileType::Unknown`, as a file system with no `d_type` does.
#[cfg(target_os = "linux")]
fn read_dir_entries_typed(
    dirname: &str,
    dir: &Rc<str>,
    mut d_type: impl FnMut(&std::ffi::CStr, rustix::fs::FileType) -> rustix::fs::FileType,
) -> io::Result<Vec<DirEntry>> {
    use rustix::fs::{AtFlags, FileType, Mode, OFlags, RawDir, openat, statat};
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC;
    let fd = openat(rustix::fs::CWD, &*os_path(dirname), flags, Mode::empty())?;
    // Go: os/dir_unix.go blockSize. On the stack, so a read allocates no
    // buffer.
    let mut buf = [std::mem::MaybeUninit::<u8>::uninit(); 8192];
    let mut raw = RawDir::new(&fd, &mut buf);
    let mut entries = Vec::new();
    while let Some(entry) = raw.next() {
        let entry = entry?;
        let name = entry.file_name().to_bytes();
        if name == b"." || name == b".." {
            continue;
        }
        let file_type = match d_type(entry.file_name(), entry.file_type()) {
            // Go and std `lstat` an entry of an unknown type.
            // Go: os/file_unix.go:469 newUnixDirent, os/dir_unix.go:141
            FileType::Unknown => match statat(&fd, entry.file_name(), AtFlags::SYMLINK_NOFOLLOW) {
                Ok(stat) => FileType::from_raw_mode(stat.st_mode),
                Err(rustix::io::Errno::NOENT) => continue,
                Err(err) => return Err(err.into()),
            },
            file_type => file_type,
        };
        let typ = match file_type {
            FileType::Directory => FileMode::DIR,
            FileType::Symlink => FileMode::SYMLINK,
            FileType::RegularFile => FileMode(0),
            FileType::BlockDevice => FileMode::DEVICE,
            FileType::CharacterDevice => FileMode::DEVICE | FileMode::CHAR_DEVICE,
            FileType::Fifo => FileMode::NAMED_PIPE,
            FileType::Socket => FileMode::SOCKET,
            FileType::Unknown => FileMode::IRREGULAR,
        };
        entries.push(DirEntry {
            name: go_string_from_os(OsStr::from_bytes(name)),
            typ,
            info: DirEntryInfo::Lstat(Rc::clone(dir)),
        });
    }
    Ok(entries)
}

/// The entries of `os_read_dir`, not sorted.
#[cfg(not(target_os = "linux"))]
fn read_dir_entries(dirname: &str, dir: &Rc<str>) -> io::Result<Vec<DirEntry>> {
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(os_path(dirname))? {
        let entry = entry?;
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
            Err(err) => return Err(err),
        };
        let typ = if file_type.is_dir() {
            FileMode::DIR
        } else if file_type.is_symlink() {
            FileMode::SYMLINK
        } else if file_type.is_file() {
            FileMode(0)
        } else {
            special_file_mode(file_type)
        };
        entries.push(DirEntry {
            name: go_string_from_os(entry.file_name()),
            typ,
            info: DirEntryInfo::Lstat(Rc::clone(dir)),
        });
    }
    Ok(entries)
}

// PORT: the `os_read_dir` mode of an entry that is not a directory, a link
// or a regular file. Linux maps the `d_type` in `read_dir_entries`.
#[cfg(all(unix, not(target_os = "linux")))]
fn special_file_mode(file_type: std::fs::FileType) -> FileMode {
    if file_type.is_block_device() {
        FileMode::DEVICE
    } else if file_type.is_char_device() {
        FileMode::DEVICE | FileMode::CHAR_DEVICE
    } else if file_type.is_fifo() {
        FileMode::NAMED_PIPE
    } else if file_type.is_socket() {
        FileMode::SOCKET
    } else {
        FileMode::IRREGULAR
    }
}

// PORT: off unix, std names no other file type. Not run on such a target.
#[cfg(not(unix))]
fn special_file_mode(_: std::fs::FileType) -> FileMode {
    FileMode::IRREGULAR
}

// Go: os/removeall_at.go RemoveAll
// PORT: Go standard library. Removes path and any children. A missing
// path is not an error. Rust `remove_dir_all` does not follow symlinks,
// like Go.
fn os_remove_all(path: &str) -> Result<(), FsError> {
    if path.is_empty() {
        // fail silently to retain compatibility with previous behavior
        // of RemoveAll. See issue 28830.
        return Ok(());
    }

    // The rmdir system call does not permit removing ".",
    // so we don't permit it either.
    if ends_with_dot(path) {
        return Err(FsError::path(
            "RemoveAll",
            path,
            io::Error::from(io::ErrorKind::InvalidInput),
        ));
    }

    let os = os_path(path);
    let result = match std::fs::symlink_metadata(&os) {
        Err(err) => Err(err),
        Ok(md) if md.is_dir() => std::fs::remove_dir_all(&os),
        Ok(_) => std::fs::remove_file(&os),
    };
    match result {
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(FsError::path("unlinkat", path, err)),
        Ok(()) => Ok(()),
    }
}

// Go: os/path.go:19 MkdirAll (go1.27.1)
// MkdirAll creates a directory named path,
// along with any necessary parents, and returns nil,
// or else returns an error.
// PORT: Go standard library. Go `Stat`, `Mkdir` and `Lstat` are
// `std::fs::metadata`, `DirBuilder::create` and `std::fs::symlink_metadata`.
// Off unix `perm` does not apply, as in Go. pprof.rs uses it too.
pub fn os_mkdir_all(path: &str, perm: u32) -> Result<(), FsError> {
    // Fast path: if we can tell whether path is a directory or file, stop with success or error.
    if let Ok(dir) = std::fs::metadata(os_path(path)) {
        if dir.is_dir() {
            return Ok(());
        }
        return Err(FsError::path(
            "mkdir",
            path,
            io::Error::from_raw_os_error(ENOTDIR),
        ));
    }

    // Slow path: make sure parent exists and then call Mkdir for path.

    // Extract the parent folder from path by first removing any trailing
    // path separator and then scanning backward until finding a path
    // separator or reaching the beginning of the string.
    let p = path.as_bytes();
    let mut i = p.len() as isize - 1;
    while i >= 0 && is_path_separator(p[i as usize]) {
        i -= 1;
    }
    while i >= 0 && !is_path_separator(p[i as usize]) {
        i -= 1;
    }
    if i < 0 {
        i = 0;
    }

    // If there is a parent directory, and it is not the volume name,
    // recurse to ensure parent directory exists.
    let parent = &path[..i as usize];
    if parent.len() > volume_name_len(path) {
        os_mkdir_all(parent, perm)?;
    }

    // Parent now exists; invoke Mkdir and use its result.
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, perm);
    if let Err(err) = builder.create(os_path(path)) {
        // Handle arguments like "foo/." by
        // double-checking that directory doesn't exist.
        if std::fs::symlink_metadata(os_path(path)).is_ok_and(|dir| dir.is_dir()) {
            return Ok(());
        }
        return Err(FsError::path("mkdir", path, err));
    }
    Ok(())
}

// Go: syscall.ENOTDIR (go1.27.1 syscall/zerrors_linux_amd64.go; darwin and
// the BSDs have the same value). On Windows it is ERROR_PATH_NOT_FOUND
// (syscall/zerrors_windows.go).
#[cfg(not(windows))]
const ENOTDIR: i32 = 0x14;
#[cfg(windows)]
const ENOTDIR: i32 = 3;

// Go: os.IsPathSeparator ('/' and, on Windows, '\\')
fn is_path_separator(c: u8) -> bool {
    c == b'/' || (cfg!(windows) && c == b'\\')
}

// Go: len(filepathlite.VolumeName(path)). There is no volume name on unix.
#[cfg(not(windows))]
fn volume_name_len(_: &str) -> usize {
    0
}
#[cfg(windows)]
fn volume_name_len(path: &str) -> usize {
    filepath_volume_name_len(path.as_bytes())
}

// Go: os/path.go endsWithDot
fn ends_with_dot(path: &str) -> bool {
    if path == "." {
        return true;
    }
    let b = path.as_bytes();
    b.len() >= 2 && b[b.len() - 1] == b'.' && b[b.len() - 2] == b'/'
}

// Go: path/filepath/path.go FromSlash
/// FromSlash returns the result of replacing each slash ('/') character in
/// path with a separator character. Multiple slashes are replaced by
/// multiple separators. On unix it is the identity.
pub fn filepath_from_slash(path: &str) -> Cow<'_, str> {
    if cfg!(windows) && path.contains('/') {
        return Cow::Owned(path.replace('/', "\\"));
    }
    Cow::Borrowed(path)
}

// Go: path/filepath/path.go Abs (unix)
// PORT: Go standard library.
#[cfg(not(windows))]
fn filepath_abs(path: &str) -> Result<String, FsError> {
    if path.starts_with('/') {
        return Ok(filepath_clean(path));
    }
    let wd = os_current_dir().map_err(|err| FsError::path("getwd", path, err))?;
    // Go: filepath.Join(wd, path)
    if path.is_empty() {
        return Ok(filepath_clean(&wd));
    }
    Ok(filepath_clean(&format!("{wd}/{path}")))
}

// Go: path/filepath/path_windows.go abs
// PORT: Go standard library. Go `syscall.FullPath` is GetFullPathNameW, which
// `std::path::absolute` calls on Windows.
#[cfg(windows)]
fn filepath_abs(path: &str) -> Result<String, FsError> {
    // syscall.FullPath returns an error on empty path, because it's not a valid path.
    // To implement Abs behavior of returning working directory on empty string input,
    // special-case empty path by changing it to "." path. See golang.org/issue/24441.
    let path = if path.is_empty() { "." } else { path };
    let full_path = std::path::absolute(os_path(path))
        .map_err(|err| FsError::path("GetFullPathName", path, err))?;
    Ok(filepath_clean(&go_string_from_os(full_path)))
}

// Go: path/filepath/path.go Clean (unix)
// PORT: Go standard library. Lexical cleanup only.
#[cfg(not(windows))]
pub fn filepath_clean(path: &str) -> String {
    if path.is_empty() {
        return ".".to_string();
    }
    let p = path.as_bytes();
    let rooted = p[0] == b'/';
    let n = p.len();

    // Invariants:
    //	reading from path; r is index of next byte to process.
    //	writing to out; w is index of next byte to write.
    //	dotdot is index in out where .. must stop, either because
    //		it is the leading slash or it is a leading ../../.. prefix.
    let mut out: Vec<u8> = Vec::with_capacity(n);
    let (mut r, mut dotdot) = (0usize, 0usize);
    if rooted {
        out.push(b'/');
        r = 1;
        dotdot = 1;
    }

    while r < n {
        if p[r] == b'/' {
            // empty path element
            r += 1;
        } else if p[r] == b'.' && (r + 1 == n || p[r + 1] == b'/') {
            // . element
            r += 1;
        } else if p[r] == b'.' && p[r + 1] == b'.' && (r + 2 == n || p[r + 2] == b'/') {
            // .. element: remove to last /
            r += 2;
            if out.len() > dotdot {
                // can backtrack
                let mut w = out.len() - 1;
                while w > dotdot && out[w] != b'/' {
                    w -= 1;
                }
                out.truncate(w);
            } else if !rooted {
                // cannot backtrack, but not rooted, so append .. element.
                if !out.is_empty() {
                    out.push(b'/');
                }
                out.extend_from_slice(b"..");
                dotdot = out.len();
            }
        } else {
            // real path element.
            // add slash if needed
            if (rooted && out.len() != 1) || (!rooted && !out.is_empty()) {
                out.push(b'/');
            }
            // copy element
            while r < n && p[r] != b'/' {
                out.push(p[r]);
                r += 1;
            }
        }
    }

    // Turn empty string into "."
    if out.is_empty() {
        return ".".to_string();
    }
    // The input is valid UTF-8 and the cuts are at ASCII '/' bytes.
    String::from_utf8(out)
        .unwrap_or_else(|err| String::from_utf8_lossy(err.as_bytes()).into_owned())
}

// Go: internal/filepathlite/path.go Clean (windows)
// PORT: Go standard library (go1.27 internal/filepathlite, path_windows.go
// for the volume name and postClean). Go's `lazybuf` is `out` plus
// `changed` (Go's `buf != nil`: the output is no longer a prefix of the
// input).
#[cfg(windows)]
pub fn filepath_clean(path: &str) -> String {
    const SEPARATOR: u8 = b'\\';
    let original_path = path;
    let vol_len = filepath_volume_name_len(path.as_bytes());
    let p = &path.as_bytes()[vol_len..];
    if p.is_empty() {
        let o = original_path.as_bytes();
        if vol_len > 1 && win_is_path_separator(o[0]) && win_is_path_separator(o[1]) {
            // should be UNC
            return original_path.replace('/', "\\");
        }
        return format!("{original_path}.");
    }
    let rooted = win_is_path_separator(p[0]);

    // Invariants:
    //	reading from path; r is index of next byte to process.
    //	writing to buf; w is index of next byte to write.
    //	dotdot is index in buf where .. must stop, either because
    //		it is the leading slash or it is a leading ../../.. prefix.
    let n = p.len();
    let mut out: Vec<u8> = Vec::with_capacity(n);
    let mut changed = false;
    let append = |out: &mut Vec<u8>, changed: &mut bool, c: u8| {
        if !*changed && (out.len() >= n || p[out.len()] != c) {
            *changed = true;
        }
        out.push(c);
    };
    let (mut r, mut dotdot) = (0usize, 0usize);
    if rooted {
        append(&mut out, &mut changed, SEPARATOR);
        r = 1;
        dotdot = 1;
    }

    while r < n {
        if win_is_path_separator(p[r]) {
            // empty path element
            r += 1;
        } else if p[r] == b'.' && (r + 1 == n || win_is_path_separator(p[r + 1])) {
            // . element
            r += 1;
        } else if p[r] == b'.'
            && p[r + 1] == b'.'
            && (r + 2 == n || win_is_path_separator(p[r + 2]))
        {
            // .. element: remove to last separator
            r += 2;
            if out.len() > dotdot {
                // can backtrack
                let mut w = out.len() - 1;
                while w > dotdot && !win_is_path_separator(out[w]) {
                    w -= 1;
                }
                out.truncate(w);
            } else if !rooted {
                // cannot backtrack, but not rooted, so append .. element.
                if !out.is_empty() {
                    append(&mut out, &mut changed, SEPARATOR);
                }
                append(&mut out, &mut changed, b'.');
                append(&mut out, &mut changed, b'.');
                dotdot = out.len();
            }
        } else {
            // real path element.
            // add slash if needed
            if (rooted && out.len() != 1) || (!rooted && !out.is_empty()) {
                append(&mut out, &mut changed, SEPARATOR);
            }
            // copy element
            while r < n && !win_is_path_separator(p[r]) {
                append(&mut out, &mut changed, p[r]);
                r += 1;
            }
        }
    }

    // Turn empty string into "."
    if out.is_empty() {
        append(&mut out, &mut changed, b'.');
    }

    // postClean: avoid creating absolute paths on Windows
    if vol_len == 0 && changed {
        // If a ':' appears in the path element at the start of a path,
        // insert a .\ at the beginning to avoid converting relative paths
        // like a/../c: into c:.
        let first = out
            .iter()
            .position(|&c| win_is_path_separator(c))
            .unwrap_or(out.len());
        if out[..first].contains(&b':') {
            out.splice(0..0, [b'.', SEPARATOR]);
        } else if out.len() >= 3
            && win_is_path_separator(out[0])
            && out[1] == b'?'
            && out[2] == b'?'
        {
            // If a path begins with \??\, insert a \. at the beginning
            // to avoid converting paths like \a\..\??\c:\x into \??\c:\x
            // (equivalent to c:\x).
            out.splice(0..0, [SEPARATOR, b'.']);
        }
    }

    let mut result = original_path.as_bytes()[..vol_len].to_vec();
    result.extend_from_slice(&out);
    // FromSlash. The cuts are at ASCII bytes, so the bytes stay UTF-8.
    String::from_utf8(result)
        .unwrap_or_else(|err| String::from_utf8_lossy(err.as_bytes()).into_owned())
        .replace('/', "\\")
}

// Go: internal/filepathlite/path_windows.go IsPathSeparator
#[cfg(windows)]
pub fn win_is_path_separator(c: u8) -> bool {
    c == b'\\' || c == b'/'
}

// Go: internal/filepathlite/path_windows.go volumeNameLen
#[cfg(windows)]
pub fn filepath_volume_name_len(path: &[u8]) -> usize {
    if path.len() >= 2 && path[1] == b':' {
        // Path starts with a drive letter.
        //
        // Not all Windows functions necessarily enforce the requirement that
        // drive letters be in the set A-Z, and we don't try to here.
        //
        // We don't handle the case of a path starting with a non-ASCII character,
        // in which case the "drive letter" might be multiple bytes long.
        return 2;
    }
    if path.is_empty() || !win_is_path_separator(path[0]) {
        // Path does not have a volume component.
        return 0;
    }
    if win_path_has_prefix_fold(path, br"\\.")
        || win_path_has_prefix_fold(path, br"\\?")
        || win_path_has_prefix_fold(path, br"\??")
    {
        // Path starts with a device prefix: \\.\ for Local Device paths,
        // or \\?\ or \??\ for Root Local Device paths.
        if path.len() == 3 {
            return 3; // exactly \\., \\?, or \??
        }
        if win_path_has_prefix_fold(&path[4..], b"UNC") {
            // We're going to treat the UNC host and share as part of the volume
            // prefix for historical reasons, but this isn't really principled;
            // Windows's own GetFullPathName will happily remove the first
            // component of the path in this space, converting
            // \\.\unc\a\b\..\c into \\.\unc\a\c.
            return win_valid_volume_name_len(path, win_unc_len(path, br"\\.\UNC\".len()));
        }
        // We treat the next component after the device prefix as
        // part of the volume name, which means Clean(`\\?\c:\`)
        // won't remove the trailing \. (See #64028.)
        return match win_cut_path(&path[4..]) {
            None => win_valid_volume_name_len(path, path.len()),
            Some((_, rest)) => win_valid_volume_name_len(path, path.len() - rest.len() - 1),
        };
    }
    if path.len() >= 2 && win_is_path_separator(path[1]) {
        // Path starts with \\, and is a UNC path.
        return win_valid_volume_name_len(path, win_unc_len(path, 2));
    }
    0
}

// Go: internal/filepathlite/path_windows.go validVolumeNameLen
#[cfg(windows)]
fn win_valid_volume_name_len(path: &[u8], n: usize) -> usize {
    let mut p = &path[..n];
    while !p.is_empty() {
        let (part, rest) = win_cut_path(p).unwrap_or((p, &[]));
        if part == b".." {
            return 0;
        }
        p = rest;
    }
    n
}

// Go: internal/filepathlite/path_windows.go pathHasPrefixFold
// pathHasPrefixFold tests whether the path s begins with prefix,
// ignoring case and treating all path separators as equivalent.
// If s is longer than prefix, then s[len(prefix)] must be a path separator.
#[cfg(windows)]
fn win_path_has_prefix_fold(s: &[u8], prefix: &[u8]) -> bool {
    if s.len() < prefix.len() {
        return false;
    }
    for i in 0..prefix.len() {
        if win_is_path_separator(prefix[i]) {
            if !win_is_path_separator(s[i]) {
                return false;
            }
        } else if prefix[i].to_ascii_uppercase() != s[i].to_ascii_uppercase() {
            return false;
        }
    }
    if s.len() > prefix.len() && !win_is_path_separator(s[prefix.len()]) {
        return false;
    }
    true
}

// Go: internal/filepathlite/path_windows.go uncLen
// uncLen returns the length of the volume prefix of a UNC path.
// prefixLen is the prefix prior to the start of the UNC host;
// for example, for "//host/share", the prefixLen is len("//")==2.
#[cfg(windows)]
fn win_unc_len(path: &[u8], prefix_len: usize) -> usize {
    let mut count = 0;
    for (i, &c) in path.iter().enumerate().skip(prefix_len) {
        if win_is_path_separator(c) {
            count += 1;
            if count == 2 {
                return i;
            }
        }
    }
    path.len()
}

// Go: internal/filepathlite/path_windows.go cutPath
// cutPath slices path around the first path separator.
// PORT: Go's `found == false` is `None`.
#[cfg(windows)]
fn win_cut_path(path: &[u8]) -> Option<(&[u8], &[u8])> {
    let i = path.iter().position(|&c| win_is_path_separator(c))?;
    Some((&path[..i], &path[i + 1..]))
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::{is_reserved_name, windows_localize};

    // Go: internal/filepathlite/path_windows.go localize and isReservedName
    #[test]
    fn localize_refuses_what_go_refuses() {
        for name in [
            "C:/a",
            "a/b:c",
            "a\\b",
            "con",
            "a/AUX/b",
            "lpt1",
            "COM\u{b9}",
            "conin$",
        ] {
            assert!(!windows_localize(name), "{name}");
        }
        for name in ["a/b.ts", "console.ts", "com0", "lpt10/x", "auxiliary"] {
            assert!(windows_localize(name), "{name}");
        }
        assert!(is_reserved_name("NUL"));
        assert!(!is_reserved_name("NULL"));
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    // PORT: not in Go. Go `os.Chtimes` (utimensat on the path) sets the
    // mtime of a file that its owner cannot read; so does `chtimes`. The
    // test skips when the file still opens (root, CAP_DAC_OVERRIDE): then a
    // `chtimes` that opens the file would pass too.
    #[test]
    fn chtimes_without_read_permission() {
        let dir = std::env::temp_dir().join(format!("ts_goport_chtimes_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("out.js");
        std::fs::write(&file, "x").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::File::open(&file).is_ok() {
            let _ = std::fs::remove_dir_all(&dir);
            eprintln!("skipped: a file without read permission opens here");
            return;
        }
        let m_time = SystemTime::UNIX_EPOCH + std::time::Duration::new(1_700_000_000, 5);
        let result = osvfs_fs().chtimes(file.to_str().unwrap(), None, Some(m_time));
        let modified = std::fs::symlink_metadata(&file)
            .unwrap()
            .modified()
            .unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(result.is_ok());
        assert_eq!(modified, m_time);
    }

    // PORT: not in Go. `os_mod_times_of_non_links` gives None for a path
    // whose name is a link, and else the mtime that `stat` gives: files in
    // one directory and in another, a directory reached through a link and
    // through 30 links, a link to a file, a missing file, a missing
    // directory, a time before 1970, a path with no file name, names
    // through 30 directory links that are a chain of 30 file links (more
    // than 40 links) and one file link, and a file through 41 directory
    // links (ELOOP). The test keeps the name of the removed `os_mod_times`
    // (followups21), so the protected name stays.
    #[test]
    fn os_mod_times_matches_stat() {
        let dir = std::env::temp_dir().join(format!("ts_goport_mod_times_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("a/b")).unwrap();
        std::fs::write(dir.join("a/x.ts"), "x").unwrap();
        std::fs::write(dir.join("a/y.ts"), "y").unwrap();
        std::fs::write(dir.join("a/b/z.ts"), "z").unwrap();
        std::os::unix::fs::symlink(dir.join("a/b"), dir.join("a/l")).unwrap();
        std::os::unix::fs::symlink(dir.join("a/x.ts"), dir.join("a/lx.ts")).unwrap();
        // Relative targets, so each link is one link whatever the temp dir.
        std::os::unix::fs::symlink("a", dir.join("d0")).unwrap();
        std::os::unix::fs::symlink("x.ts", dir.join("a/f0.ts")).unwrap();
        for i in 1..30 {
            std::os::unix::fs::symlink(format!("d{}", i - 1), dir.join(format!("d{i}"))).unwrap();
            std::os::unix::fs::symlink(format!("f{}.ts", i - 1), dir.join(format!("a/f{i}.ts")))
                .unwrap();
        }
        for i in 30..41 {
            std::os::unix::fs::symlink(format!("d{}", i - 1), dir.join(format!("d{i}"))).unwrap();
        }
        let old = SystemTime::UNIX_EPOCH - std::time::Duration::new(1000, 0)
            + std::time::Duration::new(0, 7);
        osvfs_fs()
            .chtimes(dir.join("a/y.ts").to_str().unwrap(), None, Some(old))
            .unwrap();
        let root = dir.to_str().unwrap();
        let paths: Vec<String> = [
            "a/x.ts",
            "a/y.ts",
            "a/lx.ts",
            "a/b/z.ts",
            "a/l/z.ts",
            "a/x.ts",
            "a/no.ts",
            "no/x.ts",
            "a/b/",
            "a",
            "d29/f29.ts",
            "d29/f0.ts",
            "d29/x.ts",
            "d40/x.ts",
        ]
        .iter()
        .map(|name| format!("{root}/{name}"))
        .collect();
        let m_times = os_mod_times_of_non_links(paths.iter().map(String::as_str));
        let fs = osvfs_fs();
        let want: Vec<Option<Option<SystemTime>>> = paths
            .iter()
            .map(|path| {
                let link = std::fs::symlink_metadata(path).is_ok_and(|md| md.is_symlink());
                (!link).then(|| fs.stat(path).and_then(|stat| stat.mod_time()))
            })
            .collect();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(m_times, want);
        assert_eq!(want[1], Some(Some(old)));
        assert_eq!(want[2], None);
        assert!(want[4].is_some_and(|m_time| m_time.is_some()));
        assert!(want[6] == Some(None) && want[7] == Some(None));
        assert!(want[10].is_none() && want[11].is_none());
        assert!(want[12].is_some_and(|m_time| m_time.is_some()));
        assert_eq!(want[12], want[0]);
        assert_eq!(want[13], Some(None));
    }

    // PORT: not in Go. Go `os.ReadDir` lstats an entry whose `d_type` is
    // unknown (os/file_unix.go:469 newUnixDirent), skips it when the lstat
    // finds no file and fails on any other lstat error (os/dir_unix.go:141).
    // Some file systems give no `d_type`, so the test makes every type
    // unknown (`read_dir_entries_typed`): the entries and their types equal
    // those of the `d_type` read (a directory, a file, a link to the
    // directory, a FIFO), an entry removed before its lstat is skipped, and
    // a directory with no search permission is an error. A probe lstat
    // there picks what the last part expects: the error when the probe
    // fails (a user), else x.ts as a file (root, CAP_DAC_OVERRIDE). The
    // probe, not the read's result, picks it, so a read that turns the
    // error into an entry fails.
    #[cfg(target_os = "linux")]
    #[test]
    fn read_dir_lstats_entries_of_unknown_type() {
        use rustix::fs::FileType;
        let dir = std::env::temp_dir().join(format!("ts_goport_dirent_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("a.ts"), "a").unwrap();
        std::fs::write(dir.join("gone.ts"), "g").unwrap();
        std::os::unix::fs::symlink("sub", dir.join("link")).unwrap();
        rustix::fs::mknodat(
            rustix::fs::CWD,
            dir.join("fifo"),
            FileType::Fifo,
            rustix::fs::Mode::from_raw_mode(0o644),
            0,
        )
        .unwrap();
        let root = dir.to_str().unwrap();
        let names = |entries: io::Result<Vec<DirEntry>>| -> Vec<(String, FileMode)> {
            let mut names: Vec<_> = entries
                .unwrap()
                .into_iter()
                .map(|entry| (entry.name, entry.typ))
                .collect();
            names.sort_by(|a, b| a.0.cmp(&b.0));
            names
        };
        let from_d_type = names(read_dir_entries(root, &Rc::from(root)));
        let mut seen = Vec::new();
        let from_lstat = names(read_dir_entries_typed(root, &Rc::from(root), |name, _| {
            seen.push(name.to_bytes().to_vec());
            if name.to_bytes() == b"gone.ts" {
                std::fs::remove_file(dir.join("gone.ts")).unwrap();
            }
            FileType::Unknown
        }));

        let locked = dir.join("sub");
        std::fs::write(locked.join("x.ts"), "x").unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o444)).unwrap();
        let locked_name = locked.to_str().unwrap();
        let probe = std::fs::symlink_metadata(locked.join("x.ts")).map(|_| ());
        let locked_d_type = read_dir_entries(locked_name, &Rc::from(locked_name)).map(|e| e.len());
        let locked_lstat = read_dir_entries_typed(locked_name, &Rc::from(locked_name), |_, _| {
            FileType::Unknown
        });
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        let _ = std::fs::remove_dir_all(&dir);

        let want = [
            ("a.ts", FileMode(0)),
            ("fifo", FileMode::NAMED_PIPE),
            ("gone.ts", FileMode(0)),
            ("link", FileMode::SYMLINK),
            ("sub", FileMode::DIR),
        ]
        .map(|(name, typ)| (name.to_string(), typ));
        assert_eq!(from_d_type, want);
        assert_eq!(seen.len(), 5, "each entry once");
        let mut want_lstat = want.to_vec();
        want_lstat.retain(|(name, _)| name != "gone.ts");
        assert_eq!(from_lstat, want_lstat);
        assert_eq!(locked_d_type.ok(), Some(1));
        match probe {
            Err(err) => {
                assert_eq!(err.kind(), io::ErrorKind::PermissionDenied, "the probe");
                assert_eq!(
                    locked_lstat.map(|_| ()).map_err(|err| err.kind()),
                    Err(io::ErrorKind::PermissionDenied),
                    "the read fails as the lstat does"
                );
            }
            Ok(()) => {
                assert_eq!(
                    names(locked_lstat),
                    [("x.ts".to_string(), FileMode(0))],
                    "an lstat that works there lists x.ts"
                );
                eprintln!(
                    "skipped the error part: the lstat works in a directory with no search permission"
                );
            }
        }
    }
}
