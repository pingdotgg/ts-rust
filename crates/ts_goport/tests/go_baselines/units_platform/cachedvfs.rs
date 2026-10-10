//! Go: `internal/vfs/cachedvfs/cachedvfs_test.go`.
//!
//! PORT: Go `cachedvfs.From` is `cachedvfs_from`; the Go `vfsmock.FSMock`
//! is `super::vfsmock::FsMock`.

use std::rc::Rc;

use ts_goport::frontend::vfs::{CachedFs, Fs, cachedvfs_from};

use super::vfsmock::{FsMock, wrap};

// Go: cachedvfs_test.go:13 createMockFS
fn create_mock_fs() -> Rc<FsMock> {
    wrap(crate::support::vfstest::from_map(
        [("/some/path/file.txt", "hello world")],
        true,
    ))
}

fn setup() -> (Rc<FsMock>, Rc<CachedFs>) {
    let underlying = create_mock_fs();
    let cached = cachedvfs_from(Rc::clone(&underlying) as Rc<dyn Fs>);
    (underlying, cached)
}

// Go: cachedvfs_test.go:19 TestDirectoryExists
#[test]
fn test_directory_exists() {
    let (underlying, cached) = setup();

    let _ = cached.directory_exists("/some/path");
    assert_eq!(1, underlying.calls.borrow().directory_exists.len());

    let _ = cached.directory_exists("/some/path");
    assert_eq!(1, underlying.calls.borrow().directory_exists.len());

    cached.clear_cache();
    let _ = cached.directory_exists("/some/path");
    assert_eq!(2, underlying.calls.borrow().directory_exists.len());

    let _ = cached.directory_exists("/other/path");
    assert_eq!(3, underlying.calls.borrow().directory_exists.len());

    cached.disable_and_clear_cache();
    let _ = cached.directory_exists("/some/path");
    assert_eq!(4, underlying.calls.borrow().directory_exists.len());

    let _ = cached.directory_exists("/some/path");
    assert_eq!(5, underlying.calls.borrow().directory_exists.len());

    cached.enable();
    let _ = cached.directory_exists("/some/path");
    assert_eq!(6, underlying.calls.borrow().directory_exists.len());

    let _ = cached.directory_exists("/some/path");
    assert_eq!(6, underlying.calls.borrow().directory_exists.len());
}

// Go: cachedvfs_test.go:53 TestFileExists
#[test]
fn test_file_exists() {
    let (underlying, cached) = setup();

    let _ = cached.file_exists("/some/path/file.txt");
    assert_eq!(1, underlying.calls.borrow().file_exists.len());

    let _ = cached.file_exists("/some/path/file.txt");
    assert_eq!(1, underlying.calls.borrow().file_exists.len());

    cached.clear_cache();
    let _ = cached.file_exists("/some/path/file.txt");
    assert_eq!(2, underlying.calls.borrow().file_exists.len());

    let _ = cached.file_exists("/other/path/file.txt");
    assert_eq!(3, underlying.calls.borrow().file_exists.len());

    cached.disable_and_clear_cache();
    let _ = cached.file_exists("/some/path/file.txt");
    assert_eq!(4, underlying.calls.borrow().file_exists.len());

    let _ = cached.file_exists("/some/path/file.txt");
    assert_eq!(5, underlying.calls.borrow().file_exists.len());

    cached.enable();
    let _ = cached.file_exists("/some/path/file.txt");
    assert_eq!(6, underlying.calls.borrow().file_exists.len());

    let _ = cached.file_exists("/some/path/file.txt");
    assert_eq!(6, underlying.calls.borrow().file_exists.len());
}

// Go: cachedvfs_test.go:87 TestGetAccessibleEntries
#[test]
fn test_get_accessible_entries() {
    let (underlying, cached) = setup();

    let _ = cached.get_accessible_entries("/some/path");
    assert_eq!(1, underlying.calls.borrow().get_accessible_entries.len());

    let _ = cached.get_accessible_entries("/some/path");
    assert_eq!(1, underlying.calls.borrow().get_accessible_entries.len());

    cached.clear_cache();
    let _ = cached.get_accessible_entries("/some/path");
    assert_eq!(2, underlying.calls.borrow().get_accessible_entries.len());

    let _ = cached.get_accessible_entries("/other/path");
    assert_eq!(3, underlying.calls.borrow().get_accessible_entries.len());

    cached.disable_and_clear_cache();
    let _ = cached.get_accessible_entries("/some/path");
    assert_eq!(4, underlying.calls.borrow().get_accessible_entries.len());

    let _ = cached.get_accessible_entries("/some/path");
    assert_eq!(5, underlying.calls.borrow().get_accessible_entries.len());

    cached.enable();
    let _ = cached.get_accessible_entries("/some/path");
    assert_eq!(6, underlying.calls.borrow().get_accessible_entries.len());

    let _ = cached.get_accessible_entries("/some/path");
    assert_eq!(6, underlying.calls.borrow().get_accessible_entries.len());
}

// Go: cachedvfs_test.go:121 TestRealpath
#[test]
fn test_realpath() {
    let (underlying, cached) = setup();

    let _ = cached.realpath("/some/path");
    assert_eq!(1, underlying.calls.borrow().realpath.len());

    let _ = cached.realpath("/some/path");
    assert_eq!(1, underlying.calls.borrow().realpath.len());

    cached.clear_cache();
    let _ = cached.realpath("/some/path");
    assert_eq!(2, underlying.calls.borrow().realpath.len());

    let _ = cached.realpath("/other/path");
    assert_eq!(3, underlying.calls.borrow().realpath.len());

    cached.disable_and_clear_cache();
    let _ = cached.realpath("/some/path");
    assert_eq!(4, underlying.calls.borrow().realpath.len());

    let _ = cached.realpath("/some/path");
    assert_eq!(5, underlying.calls.borrow().realpath.len());

    cached.enable();
    let _ = cached.realpath("/some/path");
    assert_eq!(6, underlying.calls.borrow().realpath.len());

    let _ = cached.realpath("/some/path");
    assert_eq!(6, underlying.calls.borrow().realpath.len());
}

// Go: cachedvfs_test.go:155 TestStat
#[test]
fn test_stat() {
    let (underlying, cached) = setup();

    let _ = cached.stat("/some/path");
    assert_eq!(1, underlying.calls.borrow().stat.len());

    let _ = cached.stat("/some/path");
    assert_eq!(1, underlying.calls.borrow().stat.len());

    cached.clear_cache();
    let _ = cached.stat("/some/path");
    assert_eq!(2, underlying.calls.borrow().stat.len());

    let _ = cached.stat("/other/path");
    assert_eq!(3, underlying.calls.borrow().stat.len());

    cached.disable_and_clear_cache();
    let _ = cached.stat("/some/path");
    assert_eq!(4, underlying.calls.borrow().stat.len());

    let _ = cached.stat("/some/path");
    assert_eq!(5, underlying.calls.borrow().stat.len());

    cached.enable();
    let _ = cached.stat("/some/path");
    assert_eq!(6, underlying.calls.borrow().stat.len());

    let _ = cached.stat("/some/path");
    assert_eq!(6, underlying.calls.borrow().stat.len());
}

// Go: cachedvfs_test.go:189 TestReadFile
#[test]
fn test_read_file() {
    let (underlying, cached) = setup();

    let _ = cached.read_file("/some/path/file.txt");
    assert_eq!(1, underlying.calls.borrow().read_file.len());

    let _ = cached.read_file("/some/path/file.txt");
    assert_eq!(2, underlying.calls.borrow().read_file.len());

    cached.clear_cache();
    let _ = cached.read_file("/some/path/file.txt");
    assert_eq!(3, underlying.calls.borrow().read_file.len());

    cached.disable_and_clear_cache();
    let _ = cached.read_file("/some/path/file.txt");
    assert_eq!(4, underlying.calls.borrow().read_file.len());

    let _ = cached.read_file("/some/path/file.txt");
    assert_eq!(5, underlying.calls.borrow().read_file.len());

    cached.enable();
    let _ = cached.read_file("/some/path/file.txt");
    assert_eq!(6, underlying.calls.borrow().read_file.len());

    let _ = cached.read_file("/some/path/file.txt");
    assert_eq!(7, underlying.calls.borrow().read_file.len());
}

// Go: cachedvfs_test.go:220 TestCaseSensitivity (ts#64159 renames
// TestUseCaseSensitiveFileNames; the port keeps use_case_sensitive_file_names)
#[test]
fn test_case_sensitivity() {
    let (underlying, cached) = setup();

    let _ = cached.use_case_sensitive_file_names();
    assert_eq!(1, underlying.calls.borrow().use_case_sensitive_file_names);

    let _ = cached.use_case_sensitive_file_names();
    assert_eq!(2, underlying.calls.borrow().use_case_sensitive_file_names);

    cached.clear_cache();
    let _ = cached.use_case_sensitive_file_names();
    assert_eq!(3, underlying.calls.borrow().use_case_sensitive_file_names);

    cached.disable_and_clear_cache();
    let _ = cached.use_case_sensitive_file_names();
    assert_eq!(4, underlying.calls.borrow().use_case_sensitive_file_names);

    let _ = cached.use_case_sensitive_file_names();
    assert_eq!(5, underlying.calls.borrow().use_case_sensitive_file_names);

    cached.enable();
    let _ = cached.use_case_sensitive_file_names();
    assert_eq!(6, underlying.calls.borrow().use_case_sensitive_file_names);

    let _ = cached.use_case_sensitive_file_names();
    assert_eq!(7, underlying.calls.borrow().use_case_sensitive_file_names);
}

// Go: cachedvfs_test.go:286 TestRemove
#[test]
fn test_remove() {
    let (underlying, cached) = setup();

    let _ = cached.remove("/some/path/file.txt");
    assert_eq!(1, underlying.calls.borrow().remove.len());

    let _ = cached.remove("/some/path/file.txt");
    assert_eq!(2, underlying.calls.borrow().remove.len());

    cached.clear_cache();
    let _ = cached.remove("/some/path/file.txt");
    assert_eq!(3, underlying.calls.borrow().remove.len());

    cached.disable_and_clear_cache();
    let _ = cached.remove("/some/path/file.txt");
    assert_eq!(4, underlying.calls.borrow().remove.len());

    let _ = cached.remove("/some/path/file.txt");
    assert_eq!(5, underlying.calls.borrow().remove.len());

    cached.enable();
    let _ = cached.remove("/some/path/file.txt");
    assert_eq!(6, underlying.calls.borrow().remove.len());

    let _ = cached.remove("/some/path/file.txt");
    assert_eq!(7, underlying.calls.borrow().remove.len());
}

// Go: cachedvfs_test.go:317 TestWriteFile
#[test]
fn test_write_file() {
    let (underlying, cached) = setup();

    let _ = cached.write_file("/some/path/file.txt", "new content");
    assert_eq!(1, underlying.calls.borrow().write_file.len());

    let _ = cached.write_file("/some/path/file.txt", "another content");
    assert_eq!(2, underlying.calls.borrow().write_file.len());

    cached.clear_cache();
    let _ = cached.write_file("/some/path/file.txt", "third content");
    assert_eq!(3, underlying.calls.borrow().write_file.len());

    let call = underlying.calls.borrow().write_file[2].clone();
    assert_eq!("/some/path/file.txt", call.path);
    assert_eq!("third content", call.data);

    cached.disable_and_clear_cache();
    let _ = cached.write_file("/some/path/file.txt", "fourth content");
    assert_eq!(4, underlying.calls.borrow().write_file.len());

    let _ = cached.write_file("/some/path/file.txt", "fifth content");
    assert_eq!(5, underlying.calls.borrow().write_file.len());

    cached.enable();
    let _ = cached.write_file("/some/path/file.txt", "sixth content");
    assert_eq!(6, underlying.calls.borrow().write_file.len());

    let _ = cached.write_file("/some/path/file.txt", "seventh content");
    assert_eq!(7, underlying.calls.borrow().write_file.len());
}
