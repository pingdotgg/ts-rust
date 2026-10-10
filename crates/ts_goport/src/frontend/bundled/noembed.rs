//! Go: internal/bundled/noembed.go (the `noembed` build, cargo feature
//! `noembed`).
//!
//! The lib files are next to the executable, as in Go's release builds and
//! the npm packages (`lib/tsc` and `lib/lib.*.d.ts`). Programs name them by
//! their real path, and they are read like any other file.

use crate::frontend::prelude::*;
use std::sync::OnceLock;

// Go: noembed.go:16 embedded
// PORT: renamed; the Go names `Embedded` and `embedded` share a snake name.
pub(super) const EMBEDDED_UNEXPORTED: bool = false;

// Go: noembed.go:18 wrapFS
pub fn wrap_fs(fs: Rc<dyn Fs>) -> Rc<dyn Fs> {
    fs
}

// Go: noembed.go:22 executableDir
fn executable_dir() -> String {
    let exe = match crate::frontend::osutil::executable() {
        Ok(exe) => exe,
        Err(err) => panic!("bundled: failed to get executable path: {err}"),
    };
    let exe = normalize_slashes(&exe);
    let exe = osvfs_fs().realpath(&exe);
    get_directory_path(&exe)
}

/// Go `libPath()`: the directory of the executable, once. Panics when it
/// has no lib.d.ts.
// Go: noembed.go:31 libPath
// PORT: Go returns TestingLibPath() in a test binary. That branch is not
// ported: the protected tests run on the default (embed) build, and a
// noembed test binary needs the lib files next to it (scripts/copy-libs.sh).
fn lib_dir() -> &'static str {
    static LIB_DIR: OnceLock<String> = OnceLock::new();
    LIB_DIR.get_or_init(|| {
        let dir = executable_dir();
        let libdts = combine_paths(&dir, &["lib.d.ts"]);
        if osvfs_fs().stat(&libdts).is_none() {
            panic!("bundled: {libdts} does not exist; this executable may be misplaced");
        }
        dir
    })
}

// Go: noembed.go:31 libPath
pub fn lib_path() -> String {
    lib_dir().to_string()
}

/// Always `None`: nothing is embedded, so a lib text is read from its file.
// PORT: not in Go (see embed.rs).
#[must_use]
pub fn bundled_text(_path: &str) -> Option<&'static str> {
    None
}

/// Always `None`: nothing is embedded, so the lib snapshots hash the text
/// they read (see embed.rs).
// PORT: not in Go.
#[must_use]
pub fn embedded_text_hash(_text: &str) -> Option<u64> {
    None
}

// Go: noembed.go:45 IsBundled
pub fn is_bundled(_path: &str) -> bool {
    false
}

/// The base name of lib path `path` (`<lib dir>/lib.dom.d.ts` gives
/// `lib.dom.d.ts`), where `<lib dir>` is `lib_path()`. `None` for any other
/// path. The lib parse and bind snapshots find a lib file by this name, and
/// their keys check its text, so a changed lib file is parsed and bound live.
// PORT: not in Go.
#[must_use]
pub fn bundled_lib_name(path: &str) -> Option<&str> {
    let dir = lib_dir();
    let rest = path.strip_prefix(dir)?;
    let name = if dir.ends_with('/') {
        rest
    } else {
        rest.strip_prefix('/')?
    };
    LIB_NAMES.binary_search(&name).is_ok().then_some(name)
}

/// True when `fs` is Go `sys.FS()` of this thread: `wrap_fs(osvfs_fs())`,
/// which is `osvfs_fs()` here, with no OS override installed (see embed.rs).
// PORT: not in Go.
pub fn is_wrapped_os_fs(fs: &Rc<dyn Fs>) -> bool {
    !os_override_installed() && Rc::ptr_eq(fs, &osvfs_fs())
}
