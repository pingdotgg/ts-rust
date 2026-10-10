//! Sub-track S2: ports of the typescript-go unit tests of the platform
//! packages. Each file ports the Go `_test.go` files of one Go package and
//! names them at the top. The expected values are the Go test literals.
//!
//! PORT: each Go `Test` function is one `#[test]` named in snake case. Go
//! `t.Run` subtests run in order inside it; `Failures` collects every failed
//! subtest, as Go reports each one. Go `t.Parallel` is dropped (libtest runs
//! the tests in parallel). Go benchmarks and fuzz targets are not ported.
//!
//! A test that shows a port bug is `#[ignore = "bug: S2-NNN ..."]`; the bug
//! list is `target/continuation-r97-goport/tests2/bugs/S2.md`. A Go test that
//! needs a private Rust item is listed there as blocked.
//!
//! The darwin FSEvents tests (`fswatch/fsevents_darwin_{nfd,shared}_test.go`)
//! run on macOS; `fswatch_fsevents_darwin.rs` lists those that are not
//! ported. `fswatch/fsevents_darwin_ffi_arm64_test.go` (Go's assembly) and
//! `nativepath/symlink_windows_test.go` (4 tests, `windows`) are not ported.

mod bundled;
mod cachedvfs;
mod collections;
mod core;
mod debug;
mod diagnostics;
mod diagnostics_locale;
mod fswatch_eventlist;
mod fswatch_fallback;
// These two files test `walkdir_unix`, which Windows does not have (Go
// `walkdir_windows.go` is not ported).
#[cfg(unix)]
mod fswatch_n;
#[cfg(unix)]
mod fswatch_walkdir;
mod fswatch_watcher;
mod jsnum;
mod lspwatcher;
mod osvfs;
mod packagejson;
mod semver;
mod stringutil;
mod symlinks;
mod tracing;
mod tspath;
mod vfs_walkdir;
mod vfsmatch;
mod vfsmock;
mod watchmanager;

use std::fmt::Debug;

/// This test binary, for tests that run a child process of it. On Linux
/// `/proc/self/exe` still names the running binary when a concurrent build
/// replaces the file on disk.
pub(crate) fn self_exe() -> std::path::PathBuf {
    let proc_exe = std::path::Path::new("/proc/self/exe");
    if proc_exe.exists() {
        return proc_exe.to_path_buf();
    }
    std::env::current_exe().expect("current test binary")
}

/// The failed subtests of one Go test function.
pub(crate) struct Failures {
    test: String,
    failures: Vec<String>,
}

impl Failures {
    pub(crate) fn new(test: &str) -> Self {
        Failures {
            test: test.to_string(),
            failures: Vec::new(),
        }
    }

    /// Go `assert.Equal` or `if got != want { t.Errorf }` in subtest `name`.
    pub(crate) fn check_eq<T: PartialEq + Debug>(&mut self, name: &str, got: T, want: T) {
        if got != want {
            self.failures
                .push(format!("{name}: got {got:?}, want {want:?}"));
        }
    }

    /// Go `t.Errorf` in subtest `name`.
    pub(crate) fn fail(&mut self, name: &str, message: String) {
        self.failures.push(format!("{name}: {message}"));
    }

    /// Panics once with every failed subtest.
    pub(crate) fn finish(self) {
        if !self.failures.is_empty() {
            panic!(
                "{}: {} failed subtest(s):\n{}",
                self.test,
                self.failures.len(),
                self.failures.join("\n")
            );
        }
    }
}
