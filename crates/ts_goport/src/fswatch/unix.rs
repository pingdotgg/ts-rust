//! The subset of Go `golang.org/x/sys/unix` (v0.46.0, linux) that fswatch
//! uses. There is no Go file for this module in typescript-go. Linux only:
//! the fswatch backends that use it (inotify, fanotify) are Linux only.
//!
//! PORT: D-W1 allows a safe syscall crate. The inotify, pipe, poll, read,
//! write, close, open, getdents, lstat and statfs calls go through `rustix`
//! (safe API) or `std`. rustix has no fanotify or `name_to_handle_at`, so
//! fanotify goes through `nix::sys::fanotify` and `name_to_handle_at` through
//! the `name-to-handle-at` crate (both safe APIs). No `libc`, no `unsafe`.
//! Every syscall keeps its Go name, parameters and result. Types, constants
//! and errno values are the Go values (zerrors_linux.go,
//! zerrors_linux_<arch>.go, ztypes_linux.go, ztypes_linux_<arch>.go). The
//! open(2) and inotify_init1(2) flags differ between targets (O_DIRECTORY
//! and O_NOFOLLOW on arm64 are the amd64 O_DIRECT and O_LARGEFILE bits), so
//! they are rustix's values for the target. A Go untyped constant gets the
//! Rust type of the place fswatch uses it. Go `int` is `i32`, Go `uint` is
//! `u32`. A syscall error is a `GoError` made from an `Errno`
//! (`errors::from_value`, see `fswatch::syscall`), so
//! `errors::is(&err, &errors::from_value(unix::EINTR))` works as in Go.
//! The `from_ne_bytes` readers replace fswatch's `unsafe.Pointer` casts of
//! kernel records. Those records (inotify_event, fanotify_event_metadata,
//! linux_dirent64) have one layout on every Linux target. `Stat_t` and
//! `Statfs_t` are filled from `std` and rustix, not cast, so the amd64
//! field types hold the values of any target. A path is a Go string in the
//! port form, so each call passes its Go bytes (`osvfs::os_path`).

use crate::frontend::vfs::osvfs::os_path;
use crate::fswatch::prelude::*;
pub use crate::fswatch::syscall::*;
use crate::gostd::errors;
use rustix::fs::OFlags;
use rustix::fs::inotify::CreateFlags;
use std::os::fd::AsFd;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

// open(2) flags (Go int), the target's values.
pub const O_RDONLY: i32 = OFlags::RDONLY.bits() as i32;
pub const O_CLOEXEC: i32 = OFlags::CLOEXEC.bits() as i32;
pub const O_NONBLOCK: i32 = OFlags::NONBLOCK.bits() as i32;
pub const O_DIRECTORY: i32 = OFlags::DIRECTORY.bits() as i32;
pub const O_NOCTTY: i32 = OFlags::NOCTTY.bits() as i32;
pub const O_NOFOLLOW: i32 = OFlags::NOFOLLOW.bits() as i32;

// poll(2) (Go PollFd.Events is int16).
pub const POLLIN: i16 = 0x1;

// *at(2) (Go int).
pub const AT_FDCWD: i32 = -0x64;

// Dirent.Type values (Go uint8).
pub const DT_UNKNOWN: u8 = 0x0;
pub const DT_DIR: u8 = 0x4;

// Stat_t.Mode bits (Go uint32).
pub const S_IFMT: u32 = 0xf000;
pub const S_IFDIR: u32 = 0x4000;

// inotify_init1(2) flags (Go int), the target's values.
pub const IN_NONBLOCK: i32 = CreateFlags::NONBLOCK.bits() as i32;
pub const IN_CLOEXEC: i32 = CreateFlags::CLOEXEC.bits() as i32;

// The flags above against the Go values (x/sys v0.46.0 zerrors_linux.go,
// zerrors_linux_amd64.go and zerrors_linux_arm64.go). Only O_DIRECTORY and
// O_NOFOLLOW differ between the two.
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
const _: () = assert!(
    O_RDONLY == 0x0
        && O_CLOEXEC == 0x80000
        && O_NONBLOCK == 0x800
        && O_NOCTTY == 0x100
        && IN_NONBLOCK == 0x800
        && IN_CLOEXEC == 0x80000,
    "assertion failed: O_RDONLY == 0x0 && O_CLOEXEC == 0x80000 && O_NONBLOCK == 0x800 && O_NOCTTY == 0x100 && IN_NONBLOCK == 0x800 && IN_CLOEXEC == 0x80000"
);
#[cfg(target_arch = "x86_64")]
const _: () = assert!(O_DIRECTORY == 0x10000 && O_NOFOLLOW == 0x20000);
#[cfg(target_arch = "aarch64")]
const _: () = assert!(O_DIRECTORY == 0x4000 && O_NOFOLLOW == 0x8000);

// inotify event and watch mask bits (Go uint32).
pub const IN_MODIFY: u32 = 0x2;
pub const IN_MOVED_FROM: u32 = 0x40;
pub const IN_MOVED_TO: u32 = 0x80;
pub const IN_CREATE: u32 = 0x100;
pub const IN_DELETE: u32 = 0x200;
pub const IN_DELETE_SELF: u32 = 0x400;
pub const IN_MOVE_SELF: u32 = 0x800;
pub const IN_Q_OVERFLOW: u32 = 0x4000;
pub const IN_ONLYDIR: u32 = 0x1000000;
pub const IN_DONT_FOLLOW: u32 = 0x2000000;
pub const IN_EXCL_UNLINK: u32 = 0x4000000;
pub const IN_ISDIR: u32 = 0x40000000;

// fanotify_init(2) flags (Go uint).
pub const FAN_CLASS_NOTIF: u32 = 0x0;
pub const FAN_CLOEXEC: u32 = 0x1;
pub const FAN_NONBLOCK: u32 = 0x2;
pub const FAN_REPORT_FID: u32 = 0x200;
pub const FAN_REPORT_DFID_NAME: u32 = 0xc00;

// fanotify_mark(2) flags (Go uint).
pub const FAN_MARK_ADD: u32 = 0x1;
pub const FAN_MARK_REMOVE: u32 = 0x2;
pub const FAN_MARK_DONT_FOLLOW: u32 = 0x4;
pub const FAN_MARK_ONLYDIR: u32 = 0x8;

// fanotify event mask bits (Go uint64).
pub const FAN_MODIFY: u64 = 0x2;
pub const FAN_MOVED_FROM: u64 = 0x40;
pub const FAN_MOVED_TO: u64 = 0x80;
pub const FAN_CREATE: u64 = 0x100;
pub const FAN_DELETE: u64 = 0x200;
pub const FAN_DELETE_SELF: u64 = 0x400;
pub const FAN_MOVE_SELF: u64 = 0x800;
pub const FAN_Q_OVERFLOW: u64 = 0x4000;
pub const FAN_EVENT_ON_CHILD: u64 = 0x8000000;
pub const FAN_RENAME: u64 = 0x10000000;
pub const FAN_ONDIR: u64 = 0x40000000;

// fanotify event records (Go uint8).
pub const FANOTIFY_METADATA_VERSION: u8 = 0x3;
pub const FAN_EVENT_INFO_TYPE_DFID_NAME: u8 = 0x2;
pub const FAN_EVENT_INFO_TYPE_DFID: u8 = 0x3;
pub const FAN_EVENT_INFO_TYPE_OLD_DFID_NAME: u8 = 0xa;
pub const FAN_EVENT_INFO_TYPE_NEW_DFID_NAME: u8 = 0xc;

// Go: ztypes_linux.go SizeofInotifyEvent
pub const SIZEOF_INOTIFY_EVENT: usize = 0x10;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

// Go: ztypes_linux.go PollFd
#[derive(Clone, Copy, Debug, Default)]
pub struct PollFd {
    pub fd: i32,
    pub events: i16,
    pub revents: i16,
}

// Go: ztypes_linux.go InotifyEvent
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct InotifyEvent {
    pub wd: i32,
    pub mask: u32,
    pub cookie: u32,
    pub len: u32,
}

impl InotifyEvent {
    /// PORT: Go casts the record bytes with `unsafe.Pointer`. The port reads
    /// the native-endian fields; bytes past the end of `b` read as zero.
    pub fn from_ne_bytes(b: &[u8]) -> InotifyEvent {
        let r = record::<16>(b);
        InotifyEvent {
            wd: i32::from_ne_bytes([r[0], r[1], r[2], r[3]]),
            mask: u32::from_ne_bytes([r[4], r[5], r[6], r[7]]),
            cookie: u32::from_ne_bytes([r[8], r[9], r[10], r[11]]),
            len: u32::from_ne_bytes([r[12], r[13], r[14], r[15]]),
        }
    }
}

// Go: ztypes_linux.go FanotifyEventMetadata
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct FanotifyEventMetadata {
    pub event_len: u32,
    pub vers: u8,
    pub reserved: u8,
    pub metadata_len: u16,
    pub mask: u64,
    pub fd: i32,
    pub pid: i32,
}

impl FanotifyEventMetadata {
    /// PORT: Go casts the record bytes with `unsafe.Pointer`. The port reads
    /// the native-endian fields; bytes past the end of `b` read as zero.
    pub fn from_ne_bytes(b: &[u8]) -> FanotifyEventMetadata {
        let r = record::<24>(b);
        FanotifyEventMetadata {
            event_len: u32::from_ne_bytes([r[0], r[1], r[2], r[3]]),
            vers: r[4],
            reserved: r[5],
            metadata_len: u16::from_ne_bytes([r[6], r[7]]),
            mask: u64::from_ne_bytes([r[8], r[9], r[10], r[11], r[12], r[13], r[14], r[15]]),
            fd: i32::from_ne_bytes([r[16], r[17], r[18], r[19]]),
            pid: i32::from_ne_bytes([r[20], r[21], r[22], r[23]]),
        }
    }
}

// Go: ztypes_linux_amd64.go Dirent
/// PORT: the record header only. Go's `Name [256]int8` and padding are read
/// from the buffer at `DIRENT_NAME_OFFSET` (Go `unsafe.Offsetof(d.Name)`).
#[derive(Clone, Copy, Debug, Default)]
pub struct Dirent {
    pub ino: u64,
    pub off: i64,
    pub reclen: u16,
    pub type_: u8,
}

/// Go `unsafe.Offsetof(unix.Dirent{}.Name)`: 19 on every Linux target
/// (linux_dirent64).
pub const DIRENT_NAME_OFFSET: usize = 19;

impl Dirent {
    /// PORT: Go casts the record bytes with `unsafe.Pointer`. The port reads
    /// the native-endian header fields; bytes past the end of `b` read as
    /// zero (a zero `reclen` stops the walk).
    pub fn from_ne_bytes(b: &[u8]) -> Dirent {
        let r = record::<19>(b);
        Dirent {
            ino: u64::from_ne_bytes([r[0], r[1], r[2], r[3], r[4], r[5], r[6], r[7]]),
            off: i64::from_ne_bytes([r[8], r[9], r[10], r[11], r[12], r[13], r[14], r[15]]),
            reclen: u16::from_ne_bytes([r[16], r[17]]),
            type_: r[18],
        }
    }
}

// PORT: the first N bytes of a kernel record, zero padded.
fn record<const N: usize>(b: &[u8]) -> [u8; N] {
    let mut r = [0u8; N];
    let n = b.len().min(N);
    r[..n].copy_from_slice(&b[..n]);
    r
}

// Go: ztypes_linux_amd64.go Timespec
#[derive(Clone, Copy, Debug, Default)]
pub struct Timespec {
    pub sec: i64,
    pub nsec: i64,
}

// Go: ztypes_linux_amd64.go Stat_t
// PORT: Go's blank padding fields are left out.
#[allow(non_camel_case_types)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Stat_t {
    pub dev: u64,
    pub ino: u64,
    pub nlink: u64,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub rdev: u64,
    pub size: i64,
    pub blksize: i64,
    pub blocks: i64,
    pub atim: Timespec,
    pub mtim: Timespec,
    pub ctim: Timespec,
}

// Go: ztypes_linux.go Fsid
#[derive(Clone, Copy, Debug, Default)]
pub struct Fsid {
    pub val: [i32; 2],
}

// Go: ztypes_linux_amd64.go Statfs_t
#[allow(non_camel_case_types)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Statfs_t {
    pub type_: i64,
    pub bsize: i64,
    pub blocks: u64,
    pub bfree: u64,
    pub bavail: u64,
    pub files: u64,
    pub ffree: u64,
    pub fsid: Fsid,
    pub namelen: i64,
    pub frsize: i64,
    pub flags: i64,
    pub spare: [i64; 4],
}

// Go: syscall_linux.go:2343 FileHandle
/// FileHandle represents the C struct file_handle used by
/// name_to_handle_at (see NameToHandleAt) and open_by_handle_at (see
/// OpenByHandleAt).
///
/// PORT: Go keeps a `*fileHandle` header followed by the handle bytes in
/// one buffer. The port keeps the header fields and the bytes.
#[derive(Clone, Debug, Default)]
pub struct FileHandle {
    pub handle_type: i32,
    pub handle: Vec<u8>,
}

// Go: syscall_linux.go:2348 NewFileHandle
/// NewFileHandle constructs a FileHandle.
pub fn new_file_handle(handle_type: i32, handle: &[u8]) -> FileHandle {
    FileHandle {
        handle_type,
        handle: handle.to_vec(),
    }
}

impl FileHandle {
    // Go: syscall_linux.go:2358 FileHandle.Size
    pub fn size(&self) -> i32 {
        self.handle.len() as i32
    }

    // Go: syscall_linux.go:2359 FileHandle.Type
    pub fn type_(&self) -> i32 {
        self.handle_type
    }

    // Go: syscall_linux.go:2360 FileHandle.Bytes
    // PORT: Go returns nil for an empty handle; the port returns an empty
    // slice.
    pub fn bytes(&self) -> &[u8] {
        let n = self.size();
        if n == 0 {
            return &[];
        }
        &self.handle[..n as usize]
    }
}

// ---------------------------------------------------------------------------
// Syscalls
// ---------------------------------------------------------------------------

// PORT: Go passes raw fd numbers. Safe Rust cannot turn a number back into a
// file descriptor, so every fd that this shim opens stays in `FDS` under its
// number until `close`. A call that gets a number not in the table fails
// with EBADF, as the kernel does for a closed fd. `poll`, `read` and `write`
// hold their own reference, so a concurrent `close` takes effect when they
// return.
static FDS: std::sync::LazyLock<std::sync::Mutex<FxHashMap<i32, std::sync::Arc<ShimFd>>>> =
    std::sync::LazyLock::new(Default::default);

// PORT: an fd in `FDS`. A fanotify group keeps its `nix` `Fanotify`, because
// `fanotify_mark` needs the `Fanotify::mark` method and safe code cannot make
// a `Fanotify` again from an `OwnedFd`.
enum ShimFd {
    Owned(std::os::fd::OwnedFd),
    Fanotify(nix::sys::fanotify::Fanotify),
}

impl AsFd for ShimFd {
    fn as_fd(&self) -> std::os::fd::BorrowedFd<'_> {
        match self {
            ShimFd::Owned(fd) => fd.as_fd(),
            ShimFd::Fanotify(f) => f.as_fd(),
        }
    }
}

impl From<std::os::fd::OwnedFd> for ShimFd {
    fn from(fd: std::os::fd::OwnedFd) -> Self {
        ShimFd::Owned(fd)
    }
}

impl From<nix::sys::fanotify::Fanotify> for ShimFd {
    fn from(f: nix::sys::fanotify::Fanotify) -> Self {
        ShimFd::Fanotify(f)
    }
}

fn register_fd(fd: impl Into<ShimFd>) -> i32 {
    use std::os::fd::AsRawFd;
    let fd = fd.into();
    let n = fd.as_fd().as_raw_fd();
    FDS.lock().unwrap().insert(n, std::sync::Arc::new(fd));
    n
}

fn lookup_fd(fd: i32) -> Result<std::sync::Arc<ShimFd>, GoError> {
    FDS.lock()
        .unwrap()
        .get(&fd)
        .cloned()
        .ok_or_else(|| errors::from_value(EBADF))
}

fn errno_error(e: rustix::io::Errno) -> GoError {
    errors::from_value(Errno(e.raw_os_error() as usize))
}

fn nix_error(e: nix::errno::Errno) -> GoError {
    errors::from_value(Errno(e as i32 as usize))
}

fn io_error(e: std::io::Error) -> GoError {
    errors::from_value(Errno(e.raw_os_error().unwrap_or(EINVAL.0 as i32) as usize))
}

// Go: syscall_linux.go:139 Pipe2
pub fn pipe2(p: &mut [i32], flags: i32) -> Result<(), GoError> {
    if p.len() != 2 {
        return Err(errors::from_value(EINVAL));
    }
    let flags = rustix::pipe::PipeFlags::from_bits_retain(flags as u32);
    let (r, w) = rustix::pipe::pipe_with(flags).map_err(errno_error)?;
    p[0] = register_fd(r);
    p[1] = register_fd(w);
    Ok(())
}

// Go: syscall_linux.go:161 Poll
pub fn poll(fds: &mut [PollFd], timeout: i32) -> Result<i32, GoError> {
    let owned = fds
        .iter()
        .map(|f| lookup_fd(f.fd))
        .collect::<Result<Vec<_>, _>>()?;
    let mut pollfds: Vec<rustix::event::PollFd<'_>> = owned
        .iter()
        .zip(fds.iter())
        .map(|(fd, f)| {
            rustix::event::PollFd::new(
                &**fd,
                rustix::event::PollFlags::from_bits_retain(f.events as u16),
            )
        })
        .collect();
    // Go: a negative timeout waits forever.
    let ts = (timeout >= 0).then(|| rustix::event::Timespec {
        tv_sec: (timeout / 1000) as _,
        tv_nsec: ((timeout % 1000) as i64 * 1_000_000) as _,
    });
    let n = rustix::event::poll(&mut pollfds, ts.as_ref()).map_err(errno_error)?;
    for (f, pfd) in fds.iter_mut().zip(pollfds.iter()) {
        f.revents = pfd.revents().bits() as i16;
    }
    Ok(n as i32)
}

// Go: syscall_unix.go:166 Read
pub fn read(fd: i32, p: &mut [u8]) -> Result<i32, GoError> {
    let f = lookup_fd(fd)?;
    let n = rustix::io::read(&*f, p).map_err(errno_error)?;
    Ok(n as i32)
}

// Go: syscall_unix.go:179 Write
pub fn write(fd: i32, p: &[u8]) -> Result<i32, GoError> {
    let f = lookup_fd(fd)?;
    let n = rustix::io::write(&*f, p).map_err(errno_error)?;
    Ok(n as i32)
}

// Go: zsyscall_linux.go:615 Close
pub fn close(fd: i32) -> Result<(), GoError> {
    match FDS.lock().unwrap().remove(&fd) {
        Some(_) => Ok(()),
        None => Err(errors::from_value(EBADF)),
    }
}

// Go: syscall_linux.go:117 Open
pub fn open(path: &str, mode: i32, perm: u32) -> Result<i32, GoError> {
    let fd = rustix::fs::open(
        &*os_path(path),
        rustix::fs::OFlags::from_bits_retain(mode as u32),
        rustix::fs::Mode::from_bits_retain(perm),
    )
    .map_err(errno_error)?;
    Ok(register_fd(fd))
}

// Go: syscall_linux.go:123 Openat
pub fn openat(dirfd: i32, path: &str, flags: i32, mode: u32) -> Result<i32, GoError> {
    let flags = rustix::fs::OFlags::from_bits_retain(flags as u32);
    let mode = rustix::fs::Mode::from_bits_retain(mode);
    let path = os_path(path);
    let path = &*path;
    let fd = if dirfd == AT_FDCWD {
        rustix::fs::openat(rustix::fs::CWD, path, flags, mode)
    } else {
        let dir = lookup_fd(dirfd)?;
        rustix::fs::openat(&*dir, path, flags, mode)
    }
    .map_err(errno_error)?;
    Ok(register_fd(fd))
}

// Go: readdirent_getdents.go:10 ReadDirent
/// PORT: one getdents64 call through `rustix::fs::RawDir`, which parses the
/// records. The records are written back to `buf` in the kernel layout
/// (`linux_dirent64`, the same `reclen`), so the batch fits `buf` as it did
/// in the kernel's reply.
pub fn read_dirent(fd: i32, buf: &mut [u8]) -> Result<i32, GoError> {
    let f = lookup_fd(fd)?;
    let mut raw = vec![std::mem::MaybeUninit::<u8>::uninit(); buf.len()];
    let mut dir = rustix::fs::RawDir::new(&*f, &mut raw);
    let mut n = 0usize;
    loop {
        let Some(ent) = dir.next() else { break };
        let ent = ent.map_err(errno_error)?;
        let name = ent.file_name().to_bytes();
        let reclen = (DIRENT_NAME_OFFSET + name.len() + 1 + 7) & !7;
        if n + reclen > buf.len() {
            return Err(errors::from_value(EINVAL));
        }
        let rec = &mut buf[n..n + reclen];
        rec.fill(0);
        rec[0..8].copy_from_slice(&ent.ino().to_ne_bytes());
        rec[8..16].copy_from_slice(&(ent.next_entry_cookie() as i64).to_ne_bytes());
        rec[16..18].copy_from_slice(&(reclen as u16).to_ne_bytes());
        rec[18] = dirent_type(ent.file_type());
        rec[DIRENT_NAME_OFFSET..DIRENT_NAME_OFFSET + name.len()].copy_from_slice(name);
        n += reclen;
        if dir.is_buffer_empty() {
            break;
        }
    }
    Ok(n as i32)
}

// PORT: the `d_type` byte that rustix turned into a `FileType`.
fn dirent_type(t: rustix::fs::FileType) -> u8 {
    use rustix::fs::FileType;
    match t {
        FileType::Fifo => 0x1,
        FileType::CharacterDevice => 0x2,
        FileType::Directory => DT_DIR,
        FileType::BlockDevice => 0x6,
        FileType::RegularFile => 0x8,
        FileType::Symlink => 0xa,
        FileType::Socket => 0xc,
        _ => DT_UNKNOWN,
    }
}

// Go: syscall_linux_amd64.go:26 Lstat
/// PORT: `std::fs::symlink_metadata` makes the same lstat(2) call.
pub fn lstat(path: &str, stat: &mut Stat_t) -> Result<(), GoError> {
    use std::os::unix::fs::MetadataExt;
    let m = std::fs::symlink_metadata(os_path(path)).map_err(io_error)?;
    *stat = Stat_t {
        dev: m.dev(),
        ino: m.ino(),
        nlink: m.nlink(),
        mode: m.mode(),
        uid: m.uid(),
        gid: m.gid(),
        rdev: m.rdev(),
        size: m.size() as i64,
        blksize: m.blksize() as i64,
        blocks: m.blocks() as i64,
        atim: Timespec {
            sec: m.atime(),
            nsec: m.atime_nsec(),
        },
        mtim: Timespec {
            sec: m.mtime(),
            nsec: m.mtime_nsec(),
        },
        ctim: Timespec {
            sec: m.ctime(),
            nsec: m.ctime_nsec(),
        },
    };
    Ok(())
}

// Go: zsyscall_linux.go:1077 InotifyInit1
pub fn inotify_init1(flags: i32) -> Result<i32, GoError> {
    let fd = rustix::fs::inotify::init(rustix::fs::inotify::CreateFlags::from_bits_retain(
        flags as u32,
    ))
    .map_err(errno_error)?;
    Ok(register_fd(fd))
}

// Go: zsyscall_linux.go:1061 InotifyAddWatch
pub fn inotify_add_watch(fd: i32, pathname: &str, mask: u32) -> Result<i32, GoError> {
    let f = lookup_fd(fd)?;
    rustix::fs::inotify::add_watch(
        &*f,
        &*os_path(pathname),
        rustix::fs::inotify::WatchFlags::from_bits_retain(mask),
    )
    .map_err(errno_error)
}

// Go: zsyscall_linux.go:1088 InotifyRmWatch
pub fn inotify_rm_watch(fd: i32, watchdesc: u32) -> Result<i32, GoError> {
    let f = lookup_fd(fd)?;
    rustix::fs::inotify::remove_watch(&*f, watchdesc as i32).map_err(errno_error)?;
    Ok(0)
}

// Go: zsyscall_linux.go:14 FanotifyInit
/// PORT: `nix` `Fanotify::init` makes the same fanotify_init(2) call. nix
/// names no FAN_REPORT_* flag, so the Go bits pass through
/// `from_bits_retain`.
pub fn fanotify_init(flags: u32, event_f_flags: u32) -> Result<i32, GoError> {
    use nix::sys::fanotify::{EventFFlags, Fanotify, InitFlags};
    let f = Fanotify::init(
        InitFlags::from_bits_retain(flags),
        EventFFlags::from_bits_retain(event_f_flags),
    )
    .map_err(nix_error)?;
    Ok(register_fd(f))
}

// Go: syscall_linux.go:53 FanotifyMark
/// PORT: `nix` `Fanotify::mark` makes the same fanotify_mark(2) call. Go
/// passes a nil pathname for "", as `None` does here. An fd that is not a
/// fanotify group fails with EINVAL, as in the kernel.
pub fn fanotify_mark(
    fd: i32,
    flags: u32,
    mask: u64,
    dir_fd: i32,
    pathname: &str,
) -> Result<(), GoError> {
    use nix::sys::fanotify::{MarkFlags, MaskFlags};
    let group = lookup_fd(fd)?;
    let ShimFd::Fanotify(f) = &*group else {
        return Err(errors::from_value(EINVAL));
    };
    let flags = MarkFlags::from_bits_retain(flags);
    let mask = MaskFlags::from_bits_retain(mask);
    let pathname = os_path(pathname);
    let pathname = (!pathname.as_os_str().is_empty()).then_some(&*pathname);
    let res = if dir_fd == AT_FDCWD {
        f.mark(flags, mask, rustix::fs::CWD, pathname)
    } else {
        let dir = lookup_fd(dir_fd)?;
        f.mark(flags, mask, &*dir, pathname)
    };
    res.map_err(nix_error)
}

// Go: syscall_linux.go:2370 NameToHandleAt
/// NameToHandleAt wraps the name_to_handle_at system call; it obtains
/// a handle for a path name.
///
/// PORT: Go `(handle FileHandle, mountID int, err error)`. The
/// `name-to-handle-at` crate makes the call. It asks for the handle size
/// first where Go first tries a 32 byte buffer; the handle is the same. Go's
/// `BytePtrFromString` fails with EINVAL for a NUL in `path`; the crate would
/// cut the path there, so the port checks first.
pub fn name_to_handle_at(dirfd: i32, path: &str, flags: i32) -> Result<(FileHandle, i32), GoError> {
    if path.contains('\0') {
        return Err(errors::from_value(EINVAL));
    }
    let path = os_path(path);
    let path = &*path;
    let (handle, mount_id) = if dirfd == AT_FDCWD {
        ::name_to_handle_at::name_to_handle_at(&rustix::fs::CWD, path, flags)
    } else {
        let dir = lookup_fd(dirfd)?;
        ::name_to_handle_at::name_to_handle_at(&*dir, path, flags)
    }
    .map_err(io_error)?;
    let mount_id = match mount_id {
        ::name_to_handle_at::MountId::Reusable(id) => id as i32,
        ::name_to_handle_at::MountId::Unique(id) => id as i32,
    };
    Ok((
        FileHandle {
            handle_type: handle.handle_type,
            handle: handle.handle,
        },
        mount_id,
    ))
}

// Go: zsyscall_linux_amd64.go:357 Statfs
/// PORT: `rustix::fs::statvfs` makes the same statfs(2) call. rustix keeps
/// the kernel `f_fsid` as `val[0] | val[1] << 32` (u32 halves, rustix 1.1.5
/// linux_raw backend), which gives back Go's `Fsid.Val`. `StatVfs` has no
/// `f_type` or `f_spare`, so `type_` and `spare` stay zero; fswatch reads
/// only `fsid`.
pub fn statfs(path: &str, buf: &mut Statfs_t) -> Result<(), GoError> {
    let st = rustix::fs::statvfs(&*os_path(path)).map_err(errno_error)?;
    *buf = Statfs_t {
        type_: 0,
        bsize: st.f_bsize as i64,
        blocks: st.f_blocks,
        bfree: st.f_bfree,
        bavail: st.f_bavail,
        files: st.f_files,
        ffree: st.f_ffree,
        fsid: Fsid {
            val: [st.f_fsid as u32 as i32, (st.f_fsid >> 32) as u32 as i32],
        },
        namelen: st.f_namemax as i64,
        frsize: st.f_frsize as i64,
        flags: st.f_flag.bits() as i64,
        spare: [0; 4],
    };
    Ok(())
}
