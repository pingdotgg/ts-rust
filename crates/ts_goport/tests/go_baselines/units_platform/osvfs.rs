//! Go: `internal/vfs/osvfs/{os,realpath,helpers}_test.go` (the unix tests;
//! `reparsepoint_windows_test.go` is Windows only).
//!
//! PORT: Go `osvfs.FS()` is `osvfs_fs()`. Go `t.TempDir()` is `TempDir`.
//! Go `runtime.GOOS` is `std::env::consts::OS`.

use std::path::{Path, PathBuf};

use ts_goport::frontend::tspath;
use ts_goport::frontend::vfs::osvfs_fs;

use crate::astnav_api::jstest::TempDir;
use crate::astnav_api::repo;

fn normalize(p: &Path) -> String {
    tspath::normalize_path(p.to_str().unwrap())
}

// Go: helpers_test.go:12 mklink (the non-Windows branch)
#[cfg(unix)]
#[must_use]
fn mklink(target: &Path, link: &Path) -> bool {
    std::os::unix::fs::symlink(target, link).expect("symlink");
    true
}

// Go: helpers_test.go:12 mklink (the Windows branch): a junction for a
// directory, a symlink for a file.
// PORT: Go skips the test when the file symlink needs elevation or developer
// mode (ERROR_PRIVILEGE_NOT_HELD). libtest has no skip: this returns false
// and the test returns. `mklink` takes no `/` separators, so the paths are
// built again from their components.
#[cfg(windows)]
#[must_use]
fn mklink(target: &Path, link: &Path) -> bool {
    const ERROR_PRIVILEGE_NOT_HELD: i32 = 1314;
    if target.is_dir() {
        let native = |p: &Path| p.components().collect::<PathBuf>();
        let status = std::process::Command::new("cmd")
            .args(["/c", "mklink", "/J"])
            .arg(native(link))
            .arg(native(target))
            .stdout(std::process::Stdio::null())
            .status()
            .expect("run mklink");
        assert!(status.success(), "mklink /J");
        return true;
    }
    match std::os::windows::fs::symlink_file(target, link) {
        Ok(()) => true,
        Err(err) if err.raw_os_error() == Some(ERROR_PRIVILEGE_NOT_HELD) => {
            eprintln!(
                "skip: file symlink support is not enabled without elevation or developer mode: {err}"
            );
            false
        }
        Err(err) => panic!("symlink: {err}"),
    }
}

// Go: os_test.go:17 TestOS
#[test]
fn test_os() {
    let fs = osvfs_fs();

    // ReadFile
    let go_mod = repo::root_path().join("go.mod");
    let go_mod_path = normalize(&go_mod);
    let expected = std::fs::read_to_string(&go_mod).expect("read go.mod");
    let (contents, ok) = fs.read_file(&go_mod_path);
    assert!(ok);
    assert_eq!(contents, expected);

    // Realpath
    if let Some(home) = std::env::var_os("HOME") {
        let home = normalize(Path::new(&home));
        assert_eq!(fs.realpath(&home), home);
    } else {
        println!("SKIP TestOS/Realpath: no home directory");
    }

    // UseCaseSensitiveFileNames
    // Just check that it works.
    let case_sensitive = fs.use_case_sensitive_file_names();
    match std::env::consts::OS {
        "windows" => assert!(!case_sensitive),
        "linux" => assert!(case_sensitive),
        _ => {}
    }
}

// Go: realpath_test.go:40 setupSymlinks
fn setup_symlinks(tmp: &Path) -> (PathBuf, PathBuf) {
    let target = tmp.join("target");
    let target_file = target.join("file");
    let link = tmp.join("link");
    let link_file = link.join("file");
    std::fs::create_dir_all(&target).unwrap();
    std::fs::write(&target_file, "hello").unwrap();
    assert!(
        mklink(&target, &link),
        "a directory link needs no privilege"
    );
    (target_file, link_file)
}

// Go: realpath_test.go:15 TestSymlinkRealpath
#[test]
fn test_symlink_realpath() {
    let tmp = TempDir::new();
    let (target_file, link_file) = setup_symlinks(tmp.path());

    let got_contents = std::fs::read_to_string(&link_file).unwrap();
    assert_eq!(got_contents, "hello");

    let fs = osvfs_fs();
    let target_realpath = fs.realpath(&normalize(&target_file));
    let link_realpath = fs.realpath(&normalize(&link_file));
    assert_eq!(
        target_realpath, link_realpath,
        "expected realpath of target and link to be equal"
    );
}

// Go: realpath_test.go:106 TestGetAccessibleEntries
#[test]
fn test_get_accessible_entries() {
    let tmp = TempDir::new();
    let target = tmp.path().join("target");
    let link = tmp.path().join("link");
    std::fs::create_dir_all(&target).unwrap();
    std::fs::create_dir_all(&link).unwrap();

    let target_file1 = target.join("file1");
    let target_file2 = target.join("file2");
    std::fs::write(&target_file1, "hello").unwrap();
    std::fs::write(&target_file2, "world").unwrap();

    let target_dir1 = target.join("dir1");
    let target_dir2 = target.join("dir2");
    std::fs::create_dir_all(&target_dir1).unwrap();
    std::fs::create_dir_all(&target_dir2).unwrap();

    if !(mklink(&target_file1, &link.join("file1"))
        && mklink(&target_file2, &link.join("file2"))
        && mklink(&target_dir1, &link.join("dir1"))
        && mklink(&target_dir2, &link.join("dir2")))
    {
        return;
    }

    let fs = osvfs_fs();

    let entries = fs.get_accessible_entries(&normalize(&link));
    assert_eq!(entries.directories, vec!["dir1", "dir2"]);
    assert_eq!(entries.files, vec!["file1", "file2"]);
    let symlinks = entries
        .symlinks
        .expect("expected Symlinks to be set for directory with symlinks");
    assert_eq!(symlinks.len(), 4);
    for name in ["file1", "file2", "dir1", "dir2"] {
        assert!(
            symlinks.contains(name),
            "expected {name:?} to be in Symlinks"
        );
    }

    // Non-symlink directory should have empty Symlinks.
    let entries = fs.get_accessible_entries(&normalize(&target));
    assert_eq!(entries.directories, vec!["dir1", "dir2"]);
    assert_eq!(entries.files, vec!["file1", "file2"]);
    let symlinks = entries
        .symlinks
        .expect("expected Symlinks to be non-nil for directory without symlinks");
    assert_eq!(symlinks.len(), 0);
}
