//! Go: `internal/vfs/vfstest/vfstest_test.go` and
//! `internal/vfs/iovfs/iofs_test.go`. They check the port of the test file
//! system itself.
//!
//! PORT: Go `t.Run` subtests are blocks of one test. Go `t.Parallel` is not
//! needed: libtest runs the tests in parallel.

use std::collections::BTreeSet;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use ts_goport::frontend::vfs::{DirEntry, FileMode, Fs, FsError};

use crate::support::iovfs::{
    self, GoFs, IoFileInfo, IoVfs, OpenFor, fs_error_text, fs_read_dir, fs_read_file, fs_stat,
    fs_sub,
};
use crate::support::vfstest::{
    FstestMapFs, MapFile, MapFs, convert_map_fs, from_map, go_quote, symlink,
};

// ---------------------------------------------------------------------------
// Helpers (gotest.tools/v3/assert and internal/testutil)
// ---------------------------------------------------------------------------

// Go: internal/testutil/testutil.go AssertPanics
fn assert_panics(f: impl FnOnce(), expected: &str) {
    let payload = match catch_unwind(AssertUnwindSafe(f)) {
        Ok(()) => panic!("expected a panic with {expected:?}"),
        Err(payload) => payload,
    };
    // Go `recover()` gives the panic value; a Go panic of the port
    // (`core::go_panic`) carries it as its message.
    let got = if let Some(panic) = payload.downcast_ref::<ts_goport::core::GoPanic>() {
        panic.message.clone()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else {
        panic!("expected a panic with {expected:?}, got a non-string panic");
    };
    assert_eq!(got, expected);
}

// Go: assert.NilError
fn assert_nil_error<T>(result: Result<T, FsError>) -> T {
    match result {
        Ok(value) => value,
        Err(err) => panic!("expected no error, got {:?}", fs_error_text(&err)),
    }
}

// Go: assert.Error (the exact message)
fn assert_error<T>(result: Result<T, FsError>, expected: &str) {
    match result {
        Ok(_) => panic!("expected error {expected:?}, got none"),
        Err(err) => assert_eq!(fs_error_text(&err), expected),
    }
}

// Go: assert.ErrorContains
fn assert_error_contains<T>(result: Result<T, FsError>, expected: &str) {
    match result {
        Ok(_) => panic!("expected an error containing {expected:?}, got none"),
        Err(err) => {
            let text = fs_error_text(&err);
            assert!(
                text.contains(expected),
                "expected an error containing {expected:?}, got {text:?}"
            );
        }
    }
}

// Go: vfstest_test.go dirEntriesToNames
fn dir_entries_to_names(entries: &[DirEntry]) -> Vec<String> {
    entries
        .iter()
        .map(|entry| entry.name().to_string())
        .collect()
}

/// Go `FromMap[any](nil, false)`.
fn empty_fs() -> Rc<dyn Fs> {
    from_map(Vec::<(String, MapFile)>::new(), false)
}

/// Go `&fstest.MapFile{Data: data, Sys: 1234}` as a `convertMapFS` input.
fn input(path: &str, data: &[u8]) -> (String, MapFile, Option<i64>) {
    (path.to_string(), MapFile::from(data), Some(1234))
}

// Go: testing/fstest/testfs.go TestFS
// PORT: part of the Go standard library checker. Ported: the walk from
// ".", the expected names, child name checks, Open+Stat against the
// directory entry and `fs.Stat`, a second listing with `fs.ReadDir`
// (sorted, same entries), file contents from Open against `fs.ReadFile`,
// the bad path checks, and the `fs.Sub` pass. Not ported: `ReadDir(n)`
// chunks, `Glob`, `Lstat` and `iotest.TestReader` (the port has no
// `fs.File` reader).
fn test_fs(fsys: Arc<dyn GoFs>, expected: &[&str]) -> Result<(), String> {
    test_fs_inner(fsys.as_ref(), expected)?;
    for name in expected {
        if let Some(i) = name.find('/') {
            let (dir, dir_slash) = (&name[..i], &name[..=i]);
            let sub_expected: Vec<&str> = expected
                .iter()
                .filter_map(|name| name.strip_prefix(dir_slash))
                .collect();
            let sub = fs_sub(Arc::clone(&fsys), dir).map_err(|err| fs_error_text(&err))?;
            test_fs_inner(sub.as_ref(), &sub_expected)
                .map_err(|err| format!("testing fs.Sub(fsys, {dir}): {err}"))?;
            break; // one sub-test is enough
        }
    }
    Ok(())
}

// Go: testing/fstest/testfs.go testFS
fn test_fs_inner(fsys: &dyn GoFs, expected: &[&str]) -> Result<(), String> {
    let mut t = FsTester {
        fsys,
        errors: Vec::new(),
        dirs: Vec::new(),
        files: Vec::new(),
    };
    t.check_dir(".");
    t.check_open(".");
    let mut found: BTreeSet<String> = t.dirs.iter().chain(&t.files).cloned().collect();
    found.remove(".");
    if expected.is_empty() && !found.is_empty() {
        let list: Vec<String> = found.iter().cloned().collect();
        t.errors.push(format!(
            "expected empty file system but found files:\n{}",
            list.join("\n")
        ));
    }
    for name in expected {
        if !found.contains(*name) {
            t.errors.push(format!("expected but not found: {name}"));
        }
    }
    if t.errors.is_empty() {
        return Ok(());
    }
    Err(format!("TestFS found errors:\n{}", t.errors.join("\n")))
}

// Go: testing/fstest/testfs.go fsTester
struct FsTester<'a> {
    fsys: &'a dyn GoFs,
    errors: Vec<String>,
    dirs: Vec<String>,
    files: Vec<String>,
}

// Go: testing/fstest/testfs.go formatEntry and formatInfoEntry
fn format_entry(name: &str, mode: FileMode) -> String {
    format!("{} IsDir={} Type={:?}", name, mode.is_dir(), mode.type_())
}

// Go: testing/fstest/testfs.go formatInfo
fn format_info(info: &IoFileInfo) -> String {
    format!(
        "{} IsDir={} Mode={:?} Size={} ModTime={:?}",
        info.info.name,
        info.info.is_dir(),
        info.info.mode,
        info.info.size,
        info.info.mod_time
    )
}

impl FsTester<'_> {
    // Go: testing/fstest/testfs.go checkDir
    fn check_dir(&mut self, dir: &str) {
        // Read entire directory.
        self.dirs.push(dir.to_string());
        let file = match self.fsys.open(dir, OpenFor::ReadDir) {
            Ok(file) => file,
            Err(err) => {
                self.errors
                    .push(format!("{dir}: Open: {}", fs_error_text(&err)));
                return;
            }
        };
        let Some(list) = file.entries else {
            self.errors.push(format!(
                "{dir}: Open returned a file that is not a fs.ReadDirFile"
            ));
            return;
        };

        // Check all children.
        let prefix = if dir == "." {
            String::new()
        } else {
            format!("{dir}/")
        };
        for info in &list {
            let name = info.info.name.as_str();
            if name == "." || name == ".." || name.is_empty() {
                self.errors.push(format!(
                    "{dir}: ReadDir: child has invalid name: {}",
                    go_quote(name)
                ));
                continue;
            }
            if name.contains('/') {
                self.errors.push(format!(
                    "{dir}: ReadDir: child name contains slash: {}",
                    go_quote(name)
                ));
                continue;
            }
            if name.contains('\\') {
                self.errors.push(format!(
                    "{dir}: ReadDir: child name contains backslash: {}",
                    go_quote(name)
                ));
                continue;
            }
            let path = format!("{prefix}{name}");
            self.check_stat(&path, info);
            self.check_open(&path);
            let typ = info.info.mode.type_();
            if typ == FileMode::DIR {
                self.check_dir(&path);
            } else if typ == FileMode::SYMLINK {
                // No further processing.
                // Avoid following symlinks to avoid potentially unbounded recursion.
                self.files.push(path);
            } else {
                self.check_file(&path);
            }
        }

        // Check fs.ReadDir as well.
        match fs_read_dir(self.fsys, dir) {
            Err(err) => {
                self.errors
                    .push(format!("{dir}: fs.ReadDir: {}", fs_error_text(&err)));
            }
            Ok(list2) => {
                for pair in list2.windows(2) {
                    if pair[0].name() >= pair[1].name() {
                        self.errors.push(format!(
                            "{dir}: fs.ReadDir: list not sorted: {} before {}",
                            pair[0].name(),
                            pair[1].name()
                        ));
                    }
                }
                // Go: checkDirList
                let mut old: Vec<String> = list
                    .iter()
                    .map(|info| format_entry(&info.info.name, info.info.mode))
                    .collect();
                let mut new: Vec<String> = list2
                    .iter()
                    .map(|entry| format_entry(entry.name(), entry.type_()))
                    .collect();
                for entry in &list2 {
                    if entry.is_dir() != entry.type_().is_dir() {
                        self.errors.push(format!(
                            "{dir}: ReadDir returned {} with IsDir() and Type() that differ",
                            entry.name()
                        ));
                    }
                }
                old.sort();
                new.sort();
                if old != new {
                    self.errors.push(format!(
                        "{dir}: diff first Open+ReadDir(-1) vs fs.ReadDir:\n\t{old:?}\n\t{new:?}"
                    ));
                }
            }
        }
    }

    // Go: testing/fstest/testfs.go checkStat
    fn check_stat(&mut self, path: &str, entry: &IoFileInfo) {
        let info = match self.fsys.open(path, OpenFor::Stat) {
            Ok(file) => file.info,
            Err(err) => {
                self.errors
                    .push(format!("{path}: Open: {}", fs_error_text(&err)));
                return;
            }
        };
        let is_symlink = entry.info.mode.type_().intersects(FileMode::SYMLINK);
        let fentry = format_entry(&entry.info.name, entry.info.mode);
        let fientry = format_entry(&info.info.name, info.info.mode);
        // Note: mismatch here is OK for symlink, because Open dereferences symlink.
        if fentry != fientry && !is_symlink {
            self.errors.push(format!(
                "{path}: mismatch:\n\tentry = {fentry}\n\tfile.Stat() = {fientry}"
            ));
        }

        // PORT: `entry.Info()` of a map entry is the entry itself.
        let einfo = entry;
        let finfo = format_info(&info);
        if is_symlink {
            // For symlink, just check that entry.Info matches entry on common fields.
            // Open deferences symlink, so info itself may differ.
            let feentry = format_entry(&einfo.info.name, einfo.info.mode);
            if fentry != feentry {
                self.errors.push(format!(
                    "{path}: mismatch\n\tentry = {fentry}\n\tentry.Info() = {feentry}\n"
                ));
            }
        } else {
            let feinfo = format_info(einfo);
            if feinfo != finfo {
                self.errors.push(format!(
                    "{path}: mismatch:\n\tentry.Info() = {feinfo}\n\tfile.Stat() = {finfo}\n"
                ));
            }
        }

        // Stat should be the same as Open+Stat, even for symlinks.
        match fs_stat(self.fsys, path) {
            Err(err) => {
                self.errors
                    .push(format!("{path}: fs.Stat: {}", fs_error_text(&err)));
            }
            Ok(info2) => {
                let finfo2 = format_info(&info2);
                if finfo2 != finfo {
                    self.errors
                        .push(format!("{path}: fs.Stat(...) = {finfo2}\n\twant {finfo}"));
                }
            }
        }
    }

    // Go: testing/fstest/testfs.go checkFile
    fn check_file(&mut self, file: &str) {
        self.files.push(file.to_string());

        // Read entire file.
        let data = match self.fsys.open(file, OpenFor::ReadFile) {
            Ok(opened) => {
                if let Some(data) = opened.data {
                    data
                } else {
                    self.errors.push(format!("{file}: Open+ReadAll: no data"));
                    return;
                }
            }
            Err(err) => {
                self.errors
                    .push(format!("{file}: Open: {}", fs_error_text(&err)));
                return;
            }
        };

        // Check that fs.ReadFile works with t.fsys.
        match fs_read_file(self.fsys, file) {
            Err(err) => {
                self.errors
                    .push(format!("{file}: fs.ReadFile: {}", fs_error_text(&err)));
            }
            Ok(data2) => {
                if data != data2 {
                    self.errors.push(format!(
                        "{file}: ReadAll vs fs.ReadFile: different data returned"
                    ));
                }
            }
        }
    }

    // Go: testing/fstest/testfs.go checkOpen
    fn check_open(&mut self, file: &str) {
        let fsys = self.fsys;
        self.check_bad_path(file, "Open", |name| fsys.open(name, OpenFor::Stat).is_err());
    }

    // Go: testing/fstest/testfs.go checkBadPath
    // PORT: `fails` reports whether opening the name fails.
    fn check_bad_path(&mut self, file: &str, desc: &str, fails: impl Fn(&str) -> bool) {
        let mut bad = vec![format!("/{file}"), format!("{file}/.")];
        if file == "." {
            bad.push("/".to_string());
        }
        if let Some(i) = file.find('/') {
            bad.push(format!("{}//{}", &file[..i], &file[i + 1..]));
            bad.push(format!("{}/./{}", &file[..i], &file[i + 1..]));
            bad.push(format!("{}\\{}", &file[..i], &file[i + 1..]));
            bad.push(format!("{}/../{}", &file[..i], file));
        }
        if let Some(i) = file.rfind('/') {
            bad.push(format!("{}//{}", &file[..i], &file[i + 1..]));
            bad.push(format!("{}/./{}", &file[..i], &file[i + 1..]));
            bad.push(format!("{}\\{}", &file[..i], &file[i + 1..]));
            bad.push(format!("{}/../{}", file, &file[i + 1..]));
        }

        for b in bad {
            if !fails(&b) {
                self.errors
                    .push(format!("{file}: {desc}({b}) succeeded, want error"));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// vfstest_test.go
// ---------------------------------------------------------------------------

// Go: vfstest_test.go TestInsensitive
#[test]
fn test_insensitive() {
    let contents = b"bar".to_vec();

    let vfs = convert_map_fs(
        vec![
            input("foo/bar/baz", &contents),
            input("foo/bar2/baz2", &contents),
            input("foo/bar3/baz3", &contents),
        ],
        false, /*useCaseSensitiveFileNames*/
        None,
    );

    let sensitive = assert_nil_error(fs_read_file(&vfs, "foo/bar/baz"));
    assert_eq!(sensitive, contents);
    let sensitive_info = assert_nil_error(fs_stat(&vfs, "foo/bar/baz"));
    assert_eq!(sensitive_info.sys, Some(1234));
    let sensitive_real_path = assert_nil_error(vfs.realpath("foo/bar/baz"));
    assert_eq!(sensitive_real_path, "foo/bar/baz");
    let entries = assert_nil_error(fs_read_dir(&vfs, "foo"));
    assert_eq!(dir_entries_to_names(&entries), ["bar", "bar2", "bar3"]);

    assert_error_contains(vfs.realpath("does/not/exist"), "file does not exist");
    assert_error_contains(fs_stat(&vfs, "does/not/exist"), "file does not exist");

    test_fs(Arc::new(vfs.clone()), &["foo/bar/baz"]).unwrap();

    let insensitive = assert_nil_error(fs_read_file(&vfs, "Foo/Bar/Baz"));
    assert_eq!(insensitive, contents);
    let insensitive_info = assert_nil_error(fs_stat(&vfs, "Foo/Bar/Baz"));
    assert_eq!(insensitive_info.sys, Some(1234));
    let insensitive_real_path = assert_nil_error(vfs.realpath("Foo/Bar/Baz"));
    assert_eq!(insensitive_real_path, "foo/bar/baz");
    let entries = assert_nil_error(fs_read_dir(&vfs, "Foo"));
    assert_eq!(dir_entries_to_names(&entries), ["bar", "bar2", "bar3"]);

    assert_error_contains(vfs.realpath("Does/Not/Exist"), "file does not exist");
    assert_error_contains(fs_stat(&vfs, "Does/Not/Exist"), "file does not exist");

    // TODO: TestFS doesn't understand case-insensitive file systems.
    // This same thing would happen with an os.Dir on Windows.
    // assert.NilError(t, fstest.TestFS(vfs, "Foo/Bar/Baz"))
}

// Go: vfstest_test.go TestInsensitiveUpper
#[test]
fn test_insensitive_upper() {
    let contents = b"bar".to_vec();

    let vfs = convert_map_fs(
        vec![
            input("Foo/Bar/Baz", &contents),
            input("Foo/Bar2/Baz2", &contents),
            input("Foo/Bar3/Baz3", &contents),
        ],
        false, /*useCaseSensitiveFileNames*/
        None,
    );

    let sensitive = assert_nil_error(fs_read_file(&vfs, "foo/bar/baz"));
    assert_eq!(sensitive, contents);
    let sensitive_info = assert_nil_error(fs_stat(&vfs, "foo/bar/baz"));
    assert_eq!(sensitive_info.sys, Some(1234));
    let entries = assert_nil_error(fs_read_dir(&vfs, "foo"));
    assert_eq!(dir_entries_to_names(&entries), ["Bar", "Bar2", "Bar3"]);

    // assert.NilError(t, fstest.TestFS(vfs, "foo/bar/baz"))

    let insensitive = assert_nil_error(fs_read_file(&vfs, "Foo/Bar/Baz"));
    assert_eq!(insensitive, contents);
    let insensitive_info = assert_nil_error(fs_stat(&vfs, "Foo/Bar/Baz"));
    assert_eq!(insensitive_info.sys, Some(1234));
    let entries = assert_nil_error(fs_read_dir(&vfs, "Foo"));
    assert_eq!(dir_entries_to_names(&entries), ["Bar", "Bar2", "Bar3"]);

    test_fs(Arc::new(vfs), &["Foo/Bar/Baz"]).unwrap();
}

// Go: vfstest_test.go TestSensitive
#[test]
fn test_sensitive() {
    let contents = b"bar".to_vec();

    let vfs = convert_map_fs(
        vec![
            input("foo/bar/baz", &contents),
            input("foo/bar2/baz2", &contents),
            input("foo/bar3/baz3", &contents),
        ],
        true, /*useCaseSensitiveFileNames*/
        None,
    );

    let sensitive = assert_nil_error(fs_read_file(&vfs, "foo/bar/baz"));
    assert_eq!(sensitive, contents);
    let sensitive_info = assert_nil_error(fs_stat(&vfs, "foo/bar/baz"));
    assert_eq!(sensitive_info.sys, Some(1234));

    test_fs(Arc::new(vfs.clone()), &["foo/bar/baz"]).unwrap();

    assert_error_contains(fs_read_file(&vfs, "Foo/Bar/Baz"), "file does not exist");
}

// Go: vfstest_test.go TestSensitiveDuplicatePath
#[test]
fn test_sensitive_duplicate_path() {
    let testfs = vec![
        ("foo".to_string(), MapFile::from(b"bar".to_vec()), None),
        ("Foo".to_string(), MapFile::from(b"baz".to_vec()), None),
    ];

    assert_panics(
        || {
            convert_map_fs(testfs, false /*useCaseSensitiveFileNames*/, None);
        },
        r#"duplicate path: "Foo" and "foo" have the same canonical path"#,
    );
}

// Go: vfstest_test.go TestInsensitiveDuplicatePath
#[test]
fn test_insensitive_duplicate_path() {
    let testfs = vec![
        ("foo".to_string(), MapFile::from(b"bar".to_vec()), None),
        ("Foo".to_string(), MapFile::from(b"baz".to_vec()), None),
    ];

    convert_map_fs(testfs, true /*useCaseSensitiveFileNames*/, None);
}

// Go: vfstest_test.go TestWritableFS
#[test]
fn test_writable_fs() {
    let fs = empty_fs();

    assert_nil_error(fs.write_file("/foo/bar/baz", "hello, world"));

    let (content, ok) = fs.read_file("/foo/bar/baz");
    assert!(ok);
    assert_eq!(content, "hello, world");

    assert_nil_error(fs.write_file("/foo/bar/baz", "goodbye, world"));

    let (content, ok) = fs.read_file("/foo/bar/baz");
    assert!(ok);
    assert_eq!(content, "goodbye, world");

    assert_error_contains(
        fs.write_file("/foo/bar/baz/oops", "goodbye, world"),
        r#"mkdir "foo/bar/baz": path exists but is not a directory"#,
    );
}

// Go: vfstest_test.go TestWritableFSDelete
#[test]
fn test_writable_fs_delete() {
    let fs = empty_fs();

    let _ = fs.write_file("/foo/bar/file.ts", "remove");
    assert!(fs.file_exists("/foo/bar/file.ts"));
    assert_nil_error(fs.remove("/foo/bar/file.ts"));
    assert!(!fs.file_exists("/foo/bar/file.ts"));

    let _ = fs.write_file("/foo/bar/test/remove2.ts", "remove2");
    assert!(fs.directory_exists("/foo/bar/test"));
    assert_nil_error(fs.remove("/foo/bar/test"));
    assert!(!fs.file_exists("/foo/bar/test/remove2.ts"));
    assert!(!fs.directory_exists("/foo/bar/test"));

    // no errors when removing file/dir that does not exist
    assert_nil_error(fs.remove("/foo/bar/test"));
    assert_nil_error(fs.remove("/foo/bar/file.ts"));

    let _ = fs.write_file("/foo/barbar", "remove2");
    let _ = fs.remove("/foo/bar");
    assert!(fs.file_exists("/foo/barbar"));
}

/// A small xorshift generator for the shuffle in `test_stress`.
// PORT: Go `math/rand/v2`; the test only needs a different order per worker.
struct XorShift(u64);

impl XorShift {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    // Go: rand.Shuffle
    fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            let j = (self.next() % (i as u64 + 1)) as usize;
            items.swap(i, j);
        }
    }
}

// Go: vfstest_test.go TestStress
// PORT: Go shares the `vfs.FS` between goroutines. `IoVfs` is `Send + Sync`,
// so the threads share one value the same way.
#[test]
fn test_stress() {
    let fs = MapFs::from_map(Vec::<(String, MapFile)>::new(), false).io_vfs();

    let ops: [fn(&IoVfs); 8] = [
        |fs: &IoVfs| {
            let _ = fs.write_file("/foo/bar/baz.txt", "hello, world");
        },
        |fs: &IoVfs| {
            fs.read_file("/foo/bar/baz.txt");
        },
        |fs: &IoVfs| {
            fs.directory_exists("/foo/bar");
        },
        |fs: &IoVfs| {
            fs.file_exists("/foo/bar");
        },
        |fs: &IoVfs| {
            fs.file_exists("/foo/bar/baz.txt");
        },
        |fs: &IoVfs| {
            fs.get_accessible_entries("/foo/bar");
        },
        |fs: &IoVfs| {
            fs.realpath("/foo/bar/baz.txt");
        },
        |fs: &IoVfs| {
            fs.stat("/foo/bar/baz.txt");
        },
    ];

    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(1, |d| d.as_nanos() as u64)
        | 1;
    let workers = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
    std::thread::scope(|scope| {
        for worker in 0..workers {
            let fs = &fs;
            let ops = &ops;
            scope.spawn(move || {
                let mut random_ops = *ops;
                XorShift(
                    seed.wrapping_add((worker as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)) | 1,
                )
                .shuffle(&mut random_ops);

                for i in 0..10000 {
                    random_ops[i % random_ops.len()](fs);
                }
            });
        }
    });
}

// Go: vfstest_test.go TestParentDirFile
#[test]
fn test_parent_dir_file() {
    let testfs = vec![
        ("foo".to_string(), MapFile::from(b"bar".to_vec()), None),
        ("foo/oops".to_string(), MapFile::from(b"baz".to_vec()), None),
    ];

    assert_panics(
        || {
            convert_map_fs(testfs, false /*useCaseSensitiveFileNames*/, None);
        },
        r#"failed to create intermediate directories for "foo/oops": mkdir "foo": path exists but is not a directory"#,
    );
}

// Go: vfstest_test.go TestFromMap
#[test]
fn test_from_map() {
    // t.Run("POSIX")
    {
        let fs = from_map(
            [
                ("/string", MapFile::from("hello, world")),
                ("/bytes", MapFile::from(b"hello, world".to_vec())),
                (
                    "/mapfile",
                    MapFile {
                        data: b"hello, world".to_vec(),
                        ..MapFile::default()
                    },
                ),
            ],
            false,
        );

        let (content, ok) = fs.read_file("/string");
        assert!(ok);
        assert_eq!(content, "hello, world");

        let (content, ok) = fs.read_file("/bytes");
        assert!(ok);
        assert_eq!(content, "hello, world");

        let (content, ok) = fs.read_file("/mapfile");
        assert!(ok);
        assert_eq!(content, "hello, world");
    }

    // t.Run("Windows")
    {
        let fs = from_map(
            [
                ("c:/string", MapFile::from("hello, world")),
                ("d:/bytes", MapFile::from(b"hello, world".to_vec())),
                (
                    "e:/mapfile",
                    MapFile {
                        data: b"hello, world".to_vec(),
                        ..MapFile::default()
                    },
                ),
            ],
            false,
        );

        let (content, ok) = fs.read_file("c:/string");
        assert!(ok);
        assert_eq!(content, "hello, world");

        let (content, ok) = fs.read_file("d:/bytes");
        assert!(ok);
        assert_eq!(content, "hello, world");

        let (content, ok) = fs.read_file("e:/mapfile");
        assert!(ok);
        assert_eq!(content, "hello, world");
    }

    // t.Run("Mixed")
    assert_panics(
        || {
            from_map(
                [
                    ("/string", MapFile::from("hello, world")),
                    ("c:/bytes", MapFile::from(b"hello, world".to_vec())),
                ],
                false,
            );
        },
        "mixed posix and windows paths",
    );

    // t.Run("NonRooted")
    assert_panics(
        || {
            from_map([("string", "hello, world")], false);
        },
        r#"non-rooted path "string""#,
    );

    // t.Run("NonNormalized")
    assert_panics(
        || {
            from_map([("/string/", "hello, world")], false);
        },
        r#"non-normalized path "/string/""#,
    );

    // t.Run("NonNormalized2")
    assert_panics(
        || {
            from_map([("/string/../foo", "hello, world")], false);
        },
        r#"non-normalized path "/string/../foo""#,
    );

    // t.Run("InvalidFile")
    // PORT: Go panics with "invalid file type int" for an int file. The
    // port's `Into<MapFile>` bound makes that a compile error.
}

// Go: vfstest_test.go TestVFSTestMapFS
#[test]
fn test_vfs_test_map_fs() {
    let fs = from_map(
        [
            ("/foo.ts", "hello, world"),
            ("/dir1/file1.ts", "export const foo = 42;"),
            ("/dir1/file2.ts", "export const foo = 42;"),
            ("/dir2/file1.ts", "export const foo = 42;"),
        ],
        false, /*useCaseSensitiveFileNames*/
    );

    // t.Run("ReadFile")
    {
        let (content, ok) = fs.read_file("/foo.ts");
        assert!(ok);
        assert_eq!(content, "hello, world");

        let (content, ok) = fs.read_file("/does/not/exist.ts");
        assert!(!ok);
        assert_eq!(content, "");
    }

    // t.Run("Realpath")
    {
        let realpath = fs.realpath("/foo.ts");
        assert_eq!(realpath, "/foo.ts");

        let realpath = fs.realpath("/Foo.ts");
        assert_eq!(realpath, "/foo.ts");

        let realpath = fs.realpath("/does/not/exist.ts");
        assert_eq!(realpath, "/does/not/exist.ts");
    }

    // t.Run("CaseSensitivity") (ts#64159 renames "UseCaseSensitiveFileNames")
    {
        assert!(!fs.use_case_sensitive_file_names());
    }
}

// Go: vfstest_test.go TestVFSTestMapFSWindows
#[test]
fn test_vfs_test_map_fs_windows() {
    let fs = from_map(
        [
            ("c:/foo.ts", "hello, world"),
            ("c:/dir1/file1.ts", "export const foo = 42;"),
            ("c:/dir1/file2.ts", "export const foo = 42;"),
            ("c:/dir2/file1.ts", "export const foo = 42;"),
        ],
        false,
    );

    // t.Run("ReadFile")
    {
        let (content, ok) = fs.read_file("c:/foo.ts");
        assert!(ok);
        assert_eq!(content, "hello, world");

        let (content, ok) = fs.read_file("c:/does/not/exist.ts");
        assert!(!ok);
        assert_eq!(content, "");
    }

    // t.Run("Realpath")
    {
        let realpath = fs.realpath("c:/foo.ts");
        assert_eq!(realpath, "c:/foo.ts");

        let realpath = fs.realpath("c:/Foo.ts");
        assert_eq!(realpath, "c:/foo.ts");

        let realpath = fs.realpath("c:/does/not/exist.ts");
        assert_eq!(realpath, "c:/does/not/exist.ts");
    }
}

// Go: vfstest_test.go TestBOM
#[test]
fn test_bom() {
    const EXPECTED: &str = "hello, world";

    let tests: [(&str, bool, [u8; 2]); 2] = [
        ("BigEndian", true, [0xFE, 0xFF]),
        ("LittleEndian", false, [0xFF, 0xFE]),
    ];

    for (name, big_endian, bom) in tests {
        // t.Run(tt.name)
        let code_points: Vec<u16> = EXPECTED.encode_utf16().collect();

        let mut buf = bom.to_vec();

        for r in code_points {
            if big_endian {
                buf.extend_from_slice(&r.to_be_bytes());
            } else {
                buf.extend_from_slice(&r.to_le_bytes());
            }
        }

        let fs = from_map([("/foo.ts", buf)], true);

        let (content, ok) = fs.read_file("/foo.ts");
        assert!(ok, "{name}");
        assert_eq!(content, EXPECTED, "{name}");
    }

    // t.Run("UTF8")
    {
        let fs = from_map(
            [(
                "/foo.ts",
                [b"\xEF\xBB\xBF".as_slice(), EXPECTED.as_bytes()].concat(),
            )],
            true,
        );

        let (content, ok) = fs.read_file("/foo.ts");
        assert!(ok);
        assert_eq!(content, EXPECTED);
    }
}

// Go: vfstest_test.go TestSymlink
#[test]
fn test_symlink() {
    let fs = from_map(
        [
            ("/foo.ts", MapFile::from("hello, world")),
            ("/symlink.ts", symlink("/foo.ts")),
            ("/some/dir/file.ts", MapFile::from("hello, world")),
            ("/some/dirlink", symlink("/some/dir")),
            ("/a", symlink("/b")),
            ("/b", symlink("/c")),
            ("/c", symlink("/d")),
            ("/d/existing.ts", MapFile::from("this is existing.ts")),
        ],
        false,
    );

    // t.Run("ReadFile")
    {
        let (content, ok) = fs.read_file("/symlink.ts");
        assert!(ok);
        assert_eq!(content, "hello, world");

        let (content, ok) = fs.read_file("/some/dirlink/file.ts");
        assert!(ok);
        assert_eq!(content, "hello, world");

        let (content, ok) = fs.read_file("/a/existing.ts");
        assert!(ok);
        assert_eq!(content, "this is existing.ts");
    }

    // t.Run("Realpath")
    {
        let realpath = fs.realpath("/symlink.ts");
        assert_eq!(realpath, "/foo.ts");

        let realpath = fs.realpath("/some/dirlink");
        assert_eq!(realpath, "/some/dir");

        let realpath = fs.realpath("/some/dirlink/file.ts");
        assert_eq!(realpath, "/some/dir/file.ts");
    }

    // t.Run("FileExists")
    {
        assert!(fs.file_exists("/symlink.ts"));
        assert!(fs.file_exists("/some/dirlink/file.ts"));
        assert!(fs.file_exists("/a/existing.ts"));
    }

    // t.Run("DirectoryExists")
    {
        assert!(fs.directory_exists("/some/dirlink"));
        assert!(fs.directory_exists("/d"));
        assert!(fs.directory_exists("/c"));
        assert!(fs.directory_exists("/b"));
        assert!(fs.directory_exists("/a"));
    }
}

// Go: vfstest_test.go TestWritableFSSymlink
#[test]
fn test_writable_fs_symlink() {
    let fs = from_map(
        [
            ("/some/dir/other.ts", MapFile::from("NOTHING")),
            ("/other.ts", symlink("/some/dir/other.ts")),
            ("/some/dirlink", symlink("/some/dir")),
            ("/brokenlink", symlink("/does/not/exist")),
            ("/a", symlink("/b")),
            ("/b", symlink("/c")),
            ("/c", symlink("/d")),
            ("/d/existing.ts", MapFile::from("hello, world")),
        ],
        false,
    );

    assert_nil_error(fs.write_file("/some/dirlink/file.ts", "hello, world"));

    let (content, ok) = fs.read_file("/some/dirlink/file.ts");
    assert!(ok);
    assert_eq!(content, "hello, world");

    let (content, ok) = fs.read_file("/some/dir/file.ts");
    assert!(ok);
    assert_eq!(content, "hello, world");

    assert_nil_error(fs.write_file("/some/dirlink/file.ts", "goodbye, world"));

    let (content, ok) = fs.read_file("/some/dirlink/file.ts");
    assert!(ok);
    assert_eq!(content, "goodbye, world");

    assert_nil_error(fs.write_file("/other.ts", "hello, world"));

    let (content, ok) = fs.read_file("/other.ts");
    assert!(ok);
    assert_eq!(content, "hello, world");

    let (content, ok) = fs.read_file("/some/dir/other.ts");
    assert!(ok);
    assert_eq!(content, "hello, world");

    assert_error(
        fs.write_file("/some/dirlink", "hello, world"),
        r#"write "some/dirlink": path exists but is not a regular file"#,
    );

    // Can't write inside a broken dir symlink
    assert_error(
        fs.write_file("/brokenlink/file.ts", "hello, world"),
        r#"broken symlink "brokenlink" -> "does/not/exist""#,
    );

    assert_error(
        fs.write_file("/brokenlink/also/wrong/file.ts", "hello, world"),
        r#"broken symlink "brokenlink" -> "does/not/exist""#,
    );

    // But we can write to a broken file symlink
    assert_nil_error(fs.write_file("/brokenlink", "hello, world"));
    let (content, ok) = fs.read_file("/brokenlink");
    assert!(ok);
    assert_eq!(content, "hello, world");
    let (content, ok) = fs.read_file("/does/not/exist");
    assert!(ok);
    assert_eq!(content, "hello, world");
}

// Go: vfstest_test.go TestWritableFSSymlinkChain
#[test]
fn test_writable_fs_symlink_chain() {
    let fs = from_map(
        [
            ("/a", symlink("/b")),
            ("/b", symlink("/c")),
            ("/c", symlink("/d")),
            ("/d/existing.ts", MapFile::from("hello, world")),
        ],
        false,
    );

    assert_nil_error(fs.write_file("/a/foo/bar/new.ts", "this is new.ts"));
    let (content, ok) = fs.read_file("/a/foo/bar/new.ts");
    assert!(ok);
    assert_eq!(content, "this is new.ts");
    let (content, ok) = fs.read_file("/b/foo/bar/new.ts");
    assert!(ok);
    assert_eq!(content, "this is new.ts");
    let (content, ok) = fs.read_file("/d/foo/bar/new.ts");
    assert!(ok);
    assert_eq!(content, "this is new.ts");
}

// Go: vfstest_test.go TestWritableFSSymlinkChainNotDir
#[test]
fn test_writable_fs_symlink_chain_not_dir() {
    let fs = from_map(
        [
            ("/a", symlink("/b")),
            ("/b", symlink("/c")),
            ("/c", symlink("/d")),
            ("/d", MapFile::from("hello, world")),
        ],
        false,
    );

    assert_error(
        fs.write_file("/a/foo/bar/new.ts", "this is new.ts"),
        r#"mkdir "d": path exists but is not a directory"#,
    );
}

// Go: vfstest_test.go TestWritableFSSymlinkDelete
#[test]
fn test_writable_fs_symlink_delete() {
    let fs = from_map(
        [
            ("/some/dir/other.ts", MapFile::from("NOTHING")),
            ("/other.ts", symlink("/some/dir/other.ts")),
            ("/some/dirlink", symlink("/some/dir")),
            ("/brokenlink", symlink("/does/not/exist")),
            ("/a", symlink("/b")),
            ("/b", symlink("/c")),
            ("/c", symlink("/d")),
            ("/d/existing.ts", MapFile::from("hello, world")),
        ],
        false,
    );

    assert_nil_error(fs.remove("/a"));
    assert!(!fs.directory_exists("/a"));
    assert!(fs.directory_exists("/b"));
    assert!(fs.directory_exists("/c"));
    assert!(fs.file_exists("/d/existing.ts"));

    // symlinks should still exist even if underlying file/dir is deleted
    assert_nil_error(fs.remove("/d"));
    assert!(!fs.directory_exists("/b"));
    assert!(!fs.directory_exists("/c"));
    assert!(!fs.directory_exists("/d"));
    assert!(!fs.file_exists("/d/again.ts"));
    assert_nil_error(fs.write_file("/d/again.ts", "d exists again"));
    assert!(fs.directory_exists("/b"));
    assert!(fs.directory_exists("/c"));
    let (content, _) = fs.read_file("/b/again.ts");
    assert_eq!(content, "d exists again");

    assert!(!fs.file_exists("/brokenlink"));
    assert!(!fs.directory_exists("/brokenlink"));
    assert_nil_error(fs.remove("/does/not/exist")); // should do nothing
    assert!(!fs.file_exists("/brokenlink"));
    assert!(!fs.directory_exists("/brokenlink"));
    assert_nil_error(fs.write_file("/does/not/exist", "hello, world"));
    assert!(fs.file_exists("/brokenlink"));
}

// ---------------------------------------------------------------------------
// iofs_test.go
// ---------------------------------------------------------------------------

// Go: iofs_test.go TestIOFS
#[test]
fn test_iofs() {
    let testfs = FstestMapFs::new([
        ("foo.ts", MapFile::from(b"hello, world".to_vec())),
        (
            "dir1/file1.ts",
            MapFile::from(b"export const foo = 42;".to_vec()),
        ),
        (
            "dir1/file2.ts",
            MapFile::from(b"export const foo = 42;".to_vec()),
        ),
        (
            "dir2/file1.ts",
            MapFile::from(b"export const foo = 42;".to_vec()),
        ),
    ]);

    let fs = iovfs::from(Arc::new(testfs), true);

    // t.Run("ReadFile")
    {
        let (content, ok) = fs.read_file("/foo.ts");
        assert!(ok);
        assert_eq!(content, "hello, world");

        let (content, ok) = fs.read_file("/does/not/exist.ts");
        assert!(!ok);
        assert_eq!(content, "");
    }

    // t.Run("ReadFileUnrooted")
    assert_panics(
        || {
            fs.read_file("bar");
        },
        r#"vfs: path "bar" is not absolute"#,
    );

    // t.Run("FileExists")
    {
        assert!(fs.file_exists("/foo.ts"));
        assert!(!fs.file_exists("/bar"));
    }

    // t.Run("DirectoryExists")
    {
        assert!(fs.directory_exists("/"));
        assert!(fs.directory_exists("/dir1"));
        assert!(fs.directory_exists("/dir1/"));
        assert!(fs.directory_exists("/dir1/./"));
        assert!(!fs.directory_exists("/bar"));
    }

    // t.Run("GetAccessibleEntries")
    {
        let entries = fs.get_accessible_entries("/");
        assert_eq!(entries.directories, ["dir1", "dir2"]);
        assert_eq!(entries.files, ["foo.ts"]);
    }

    // t.Run("Realpath")
    {
        let realpath = fs.realpath("/foo.ts");
        assert_eq!(realpath, "/foo.ts");
    }

    // t.Run("CaseSensitivity") (ts#64159 renames "UseCaseSensitiveFileNames")
    {
        assert!(fs.use_case_sensitive_file_names());
    }
}
