//! Go: internal/testutil/fsbaselineutil/differ.go
//!
//! PORT: text. File contents are the port form of the Go bytes
//! (`go_string_from_bytes`), so the baseline text is the port form of the Go
//! text. Paths are the `vfstest` realpaths with a leading '/'.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::SystemTime;

use rustc_hash::FxHashSet;
use ts_goport::frontend::vfs::FileMode;
use ts_goport::scanner_util::{GoUnit, compare_go_strings, go_string_from_bytes, go_unit_at};

use crate::support::vfstest::MapFs;

// Go: differ.go:19 DiffEntry
// PORT: a zero `time.Time` is `None`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DiffEntry {
    pub content: String,
    pub m_time: Option<SystemTime>,
    pub is_written: bool,
    pub symlink_target: String,
}

// Go: differ.go:26 Snapshot
// PORT: Go `map[string]*DiffEntry` is sorted here. Go always sets
// `DefaultLibs` (to a copy, possibly empty).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub snap: BTreeMap<String, DiffEntry>,
    pub default_libs: FxHashSet<String>,
}

// Go: differ.go:31 FSDiffer
// PORT: Go `FS iovfs.FsWithSys` is only used for its `*vfstest.MapFS`, which
// is `fs` here. Go `DefaultLibs func() *collections.SyncSet[string]` reads
// the harness field each time; the port shares the field, and `None` is a
// nil set. `WrittenFiles` is shared the same way.
pub struct FsDiffer {
    pub fs: MapFs,
    pub default_libs: Arc<Mutex<Option<FxHashSet<String>>>>,
    pub written_files: Arc<Mutex<FxHashSet<String>>>,

    serialized_diff: Option<Snapshot>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl FsDiffer {
    pub fn new(
        fs: MapFs,
        default_libs: Arc<Mutex<Option<FxHashSet<String>>>>,
        written_files: Arc<Mutex<FxHashSet<String>>>,
    ) -> FsDiffer {
        FsDiffer {
            fs,
            default_libs,
            written_files,
            serialized_diff: None,
        }
    }

    // Go: differ.go:39 MapFs
    pub fn map_fs(&self) -> &MapFs {
        &self.fs
    }

    // Go: differ.go:43 SerializedDiff
    pub fn serialized_diff(&self) -> Option<&Snapshot> {
        self.serialized_diff.as_ref()
    }

    /// Sets the last snapshot (child process protocol).
    pub fn set_serialized_diff(&mut self, serialized_diff: Option<Snapshot>) {
        self.serialized_diff = serialized_diff;
    }

    // Go: differ.go:47 BaselineFSwithDiff
    pub fn baseline_fs_with_diff(&mut self, baseline: &mut String) {
        // todo: baselines the entire fs, possibly doesn't correctly diff all cases of emitted files, since emit isn't fully implemented and doesn't always emit the same way as strada
        let mut snap: BTreeMap<String, DiffEntry> = BTreeMap::new();

        let mut diffs: BTreeMap<String, String> = BTreeMap::new();

        for (path, file) in self.map_fs().entries() {
            if file.mode.intersects(FileMode::SYMLINK) {
                let Some(target) = self.map_fs().get_target_of_symlink(&path) else {
                    panic!("Failed to resolve symlink target: {path}");
                };
                let new_entry = DiffEntry {
                    symlink_target: target,
                    ..DiffEntry::default()
                };
                self.add_fs_entry_diff(&mut diffs, Some(&new_entry), &path);
                snap.insert(path, new_entry);
                continue;
            } else if file.mode.is_regular() {
                let content = sanitize_internal_symbol_name(&go_string_from_bytes(file.data));
                let is_written = lock(&self.written_files).contains(&path);
                let new_entry = DiffEntry {
                    content,
                    m_time: file.mod_time,
                    is_written,
                    symlink_target: String::new(),
                };
                self.add_fs_entry_diff(&mut diffs, Some(&new_entry), &path);
                snap.insert(path, new_entry);
            }
        }
        if let Some(serialized_diff) = &self.serialized_diff {
            let deleted: Vec<String> = serialized_diff
                .snap
                .keys()
                .filter(|path| self.map_fs().get_file_info(path).is_none())
                .cloned()
                .collect();
            for path in deleted {
                // report deleted
                self.add_fs_entry_diff(&mut diffs, None, &path);
            }
        }
        let mut default_libs: FxHashSet<String> = FxHashSet::default();
        if let Some(current) = lock(&self.default_libs).as_ref() {
            default_libs.extend(current.iter().cloned());
        }
        self.serialized_diff = Some(Snapshot { snap, default_libs });
        // Go: slices.Sort(diffKeys)
        let mut diff_keys: Vec<&String> = diffs.keys().collect();
        diff_keys.sort_by(|a, b| compare_go_strings(a, b));
        for path in diff_keys {
            baseline.push_str("//// [");
            baseline.push_str(path);
            baseline.push_str("] ");
            baseline.push_str(&diffs[path]);
            baseline.push('\n');
        }
        baseline.push('\n');
        // Reset written files after baseline
        lock(&self.written_files).clear();
    }

    // Go: differ.go:116 addFsEntryDiff
    fn add_fs_entry_diff(
        &self,
        diffs: &mut BTreeMap<String, String>,
        new_dir_content: Option<&DiffEntry>,
        path: &str,
    ) {
        let mut old_dir_content: Option<&DiffEntry> = None;
        let mut default_libs: Option<&FxHashSet<String>> = None;
        if let Some(serialized_diff) = &self.serialized_diff {
            old_dir_content = serialized_diff.snap.get(path);
            default_libs = Some(&serialized_diff.default_libs);
        }
        let current_default_libs = lock(&self.default_libs);
        // todo handle more cases of fs changes
        match (old_dir_content, new_dir_content) {
            (None, new_dir_content) => {
                // PORT: Go reads `newDirContent` here; a deleted path always
                // has an old entry, so it is set.
                let new_dir_content =
                    new_dir_content.expect("addFsEntryDiff: new path without content");
                let is_default_lib = current_default_libs
                    .as_ref()
                    .is_some_and(|libs| libs.contains(path));
                if !is_default_lib {
                    if new_dir_content.symlink_target.is_empty() {
                        diffs.insert(
                            path.to_string(),
                            format!("*new* \n{}", new_dir_content.content),
                        );
                    } else {
                        diffs.insert(
                            path.to_string(),
                            format!("-> {} *new*", new_dir_content.symlink_target),
                        );
                    }
                }
            }
            (Some(_), None) => {
                diffs.insert(path.to_string(), "*deleted*".to_string());
            }
            (Some(old_dir_content), Some(new_dir_content)) => {
                if new_dir_content.content != old_dir_content.content {
                    diffs.insert(
                        path.to_string(),
                        format!("*modified* \n{}", new_dir_content.content),
                    );
                } else if new_dir_content.is_written {
                    diffs.insert(path.to_string(), "*rewrite with same content*".to_string());
                } else if new_dir_content.m_time != old_dir_content.m_time {
                    diffs.insert(path.to_string(), "*mTime changed*".to_string());
                } else if default_libs.is_some_and(|libs| libs.contains(path))
                    && current_default_libs
                        .as_ref()
                        .is_some_and(|libs| !libs.contains(path))
                {
                    // Lib file that was read
                    diffs.insert(
                        path.to_string(),
                        format!("*Lib*\n{}", new_dir_content.content),
                    );
                }
            }
        }
    }

    // Go: differ.go:153 ChangedPaths
    // PORT: Go reports deleted files in map order; the port sorts them.
    pub fn changed_paths(&self) -> Vec<FileChange> {
        let Some(old_snap) = &self.serialized_diff else {
            return Vec::new();
        };

        let mut changes: Vec<FileChange> = Vec::new();

        // Check current files against previous snapshot.
        for (path, file) in self.map_fs().entries() {
            if file.mode.intersects(FileMode::SYMLINK) || !file.mode.is_regular() {
                continue;
            }
            match old_snap.snap.get(&path) {
                None => {
                    // New file.
                    changes.push(FileChange {
                        path,
                        deleted: false,
                    });
                }
                Some(old) => {
                    if go_string_from_bytes(file.data) != old.content || file.mod_time != old.m_time
                    {
                        // Modified or touched file.
                        changes.push(FileChange {
                            path,
                            deleted: false,
                        });
                    }
                }
            }
        }

        // Check for deleted files.
        for path in old_snap.snap.keys() {
            if self.map_fs().get_file_info(path).is_none() {
                changes.push(FileChange {
                    path: path.clone(),
                    deleted: true,
                });
            }
        }

        changes
    }
}

// Go: differ.go:102 internalSymbolRegex
// `\x{FFFD}@[^@]+@[0-9]+`

// Go: differ.go:106 SanitizeInternalSymbolName
// Replaces internal symbol names of shape �@symbolName@123 with �@symbolName@<symbolId>
// // to avoid baselining differences in symbol ids, which can change between runs.
// PORT: no regex crate. The match works on Go units (`go_unit_at`), which
// are the runes that Go regexp reads: a byte that is not valid UTF-8 is one
// RuneError, so it matches `\x{FFFD}` and `[^@]` as in Go.
pub fn sanitize_internal_symbol_name(s: &str) -> String {
    if !s.contains("\u{FFFD}@") {
        return s.to_string();
    }

    // Each unit with its start byte and size in `s`.
    let mut units: Vec<(GoUnit, usize, usize)> = Vec::new();
    let mut i = 0usize;
    while i < s.len() {
        let (unit, size) = go_unit_at(s, i);
        units.push((unit, i, size));
        i += size;
    }

    let mut out = String::with_capacity(s.len());
    let mut copied = 0usize;
    let mut u = 0usize;
    while u < units.len() {
        match match_internal_symbol_name(&units, u) {
            Some((last_at, end)) => {
                // match[:idStart] + "@<symbolId>"
                out.push_str(&s[copied..units[last_at].1]);
                out.push_str("@<symbolId>");
                let (_, start, size) = units[end - 1];
                copied = start + size;
                u = end;
            }
            None => u += 1,
        }
    }
    out.push_str(&s[copied..]);
    out
}

/// The match of `\x{FFFD}@[^@]+@[0-9]+` that starts at unit `start`: the
/// unit index of its last '@' and the unit index after its end.
fn match_internal_symbol_name(
    units: &[(GoUnit, usize, usize)],
    start: usize,
) -> Option<(usize, usize)> {
    let is_at = |u: usize| matches!(units.get(u), Some((GoUnit::Char('@'), _, _)));
    if !matches!(
        units[start].0,
        GoUnit::Char('\u{FFFD}') | GoUnit::InvalidByte(_)
    ) {
        return None;
    }
    if !is_at(start + 1) {
        return None;
    }
    // [^@]+ runs to the next '@', which must follow at least one unit.
    let name_start = start + 2;
    let mut j = name_start;
    while j < units.len() && !is_at(j) {
        j += 1;
    }
    if j == name_start || j >= units.len() {
        return None;
    }
    let last_at = j;
    j += 1;
    let digits_start = j;
    while j < units.len() && matches!(units[j].0, GoUnit::Char(c) if c.is_ascii_digit()) {
        j += 1;
    }
    if j == digits_start {
        return None;
    }
    Some((last_at, j))
}

// Go: differ.go:148 FileChange
// FileChange represents a filesystem change detected between snapshots.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileChange {
    pub path: String,
    pub deleted: bool,
}
