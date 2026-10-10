//! Port-only tests of the `tsc -b` build info prefetch (orchestrator.rs
//! `BuildInfoPrefetch`, build_task.rs `StatusPrefetch`) when a task of the
//! build writes a file that the prefetch read before the write
//! (followups18 item 5).
//!
//! PORT: no Go counterpart: Go has no prefetch. Go reads the build info and
//! the input mtimes of a project in its check, after its upstream projects
//! are built (build/buildtask.go `loadOrStoreBuildInfo`, build/host.go:102
//! `loadOrStoreMTime`). The prefetch runs only on the OS file system with
//! more than one builder, so these tests run `tsgo` on a temp dir with
//! `--builders 4`. The expected status lines are Go N's.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

/// A new empty dir under the system temp dir; removed on drop.
struct TmpDir(PathBuf);

impl TmpDir {
    fn new(name: &str) -> TmpDir {
        let dir = std::env::temp_dir().join(format!("goport-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("mkdir {}: {e}", dir.display()));
        TmpDir(crate::support::eval_symlinks(&dir).expect("a real path"))
    }

    fn write(&self, name: &str, text: &str) {
        let path = self.0.join(name);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
        std::fs::write(&path, text).unwrap_or_else(|e| panic!("write {name}: {e}"));
    }
}

impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// `tsgo -b --verbose --builders 4` in `dir`: the status lines, without
/// their time.
fn build(dir: &Path) -> Vec<String> {
    let output = Command::new(env!("CARGO_BIN_EXE_tsgo"))
        .args(["-b", "--verbose", "--builders", "4", "--pretty", "false"])
        .current_dir(dir)
        .output()
        .expect("run tsgo -b");
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.split_once(" - ").map(|(_, status)| status.to_string()))
        .collect()
}

/// The two projects of each test, built once, then `p0/index.ts` edited.
fn two_projects(name: &str, p1_config: &str, extra: impl FnOnce(&TmpDir)) -> TmpDir {
    let dir = TmpDir::new(name);
    dir.write(
        "tsconfig.json",
        r#"{"files":[],"references":[{"path":"p0"},{"path":"p1"}]}"#,
    );
    dir.write(
        "p0/tsconfig.json",
        r#"{"compilerOptions":{"composite":true,"types":[],"outDir":"out"}}"#,
    );
    dir.write("p0/index.ts", "export const v0 = 1;\n");
    dir.write("p1/tsconfig.json", p1_config);
    extra(&dir);
    build(&dir.0);
    // Past the mtime granularity of the file system.
    std::thread::sleep(Duration::from_millis(50));
    dir.write("p0/index.ts", "export const v0 = 2;\n");
    dir
}

/// A source of p1 that is a symbolic link to p0's `.d.ts` output: p0
/// writes it again in the build, before p1's check reads its mtime.
#[test]
fn a_source_link_to_an_upstream_output_is_read_after_the_upstream_build() {
    let dir = two_projects(
        "prefetch-link",
        r#"{"compilerOptions":{"composite":true,"types":[],"outDir":"out"},"references":[{"path":"../p0"}],"include":["*.ts"]}"#,
        |dir| {
            dir.write("p1/index.ts", "export const v1 = 1;\n");
            std::os::unix::fs::symlink("../p0/out/index.d.ts", dir.0.join("p1/link.ts"))
                .expect("symlink");
        },
    );
    let status = build(&dir.0);
    assert!(
        status.contains(&"Project 'p1/tsconfig.json' is out of date because output 'p1/out/tsconfig.tsbuildinfo' is older than input 'p1/link.ts'".to_string()),
        "{status:#?}"
    );
}

/// p1's build info file is p0's `index.js` output: p0 writes it in the
/// build, before p1's check reads it.
#[test]
fn a_build_info_that_another_project_writes_is_read_after_that_build() {
    let dir = two_projects(
        "prefetch-buildinfo",
        r#"{"compilerOptions":{"composite":true,"types":[],"outDir":"out","tsBuildInfoFile":"../p0/out/index.js"},"references":[{"path":"../p0"}],"files":["index.ts"]}"#,
        |dir| {
            dir.write(
                "p1/index.ts",
                "import { v0 } from \"../p0/index\";\nexport const v1 = v0;\n",
            )
        },
    );
    let status = build(&dir.0);
    assert!(
        status.contains(&"Project 'p1/tsconfig.json' is out of date because output file 'p0/out/index.js' does not exist".to_string()),
        "{status:#?}"
    );
}
