//! Port-only tests of the build info read side with the Effect plugin
//! (effectfix2; the R179 reviewer's mutants M2 and M3). With the Effect
//! rules on, build info records `<version>+effect-tsgo.<release>`
//! (`build_info_version`), as Effect-TS/tsgo does. A program reuses build
//! info of its own kind, and builds again from build info of the other
//! kind, in `tsgo -p` and in `tsgo -b` (the status check and its prefetch).
//!
//! PORT: no Go counterpart. In `effect_build_info_is_reused` the `-b
//! --verbose` statuses are the ones that effect-tsgo 0.51.1 prints in the
//! same steps. In `build_info_of_the_other_kind_is_built_again` they are not
//! (the effectfix2 design): effect-tsgo 0.51.1 records its suffix with and
//! without the plugin, so it calls steps 2 and 3 up to date and loses
//! TS377068. There the step 2 status is the one effect-tsgo 0.51.1 prints on
//! build info of plain tsgo N, and the step 3 status the one plain tsgo N
//! prints on build info of effect-tsgo 0.51.1. The fixture needs no Effect
//! package: `globalDate` (TS377068) flags `new Date()`.
#![cfg(unix)]

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime};

use ts_goport::execute::incremental::build_info::build_info_version;

/// An mtime older than any build in the test (2001).
const OLD: Duration = Duration::from_secs(1_000_000_000);

/// A composite project in a new dir under the system temp dir; removed on
/// drop.
struct Project(PathBuf);

impl Project {
    fn new(name: &str, effect: bool, source: &str) -> Project {
        let dir = std::env::temp_dir().join(format!("goport-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src"))
            .unwrap_or_else(|e| panic!("mkdir {}: {e}", dir.display()));
        let project = Project(dir);
        project.write("src/index.ts", source);
        project.set_effect(effect);
        project
    }

    fn write(&self, name: &str, text: &str) {
        std::fs::write(self.0.join(name), text).unwrap_or_else(|e| panic!("write {name}: {e}"));
    }

    /// Writes the tsconfig with or without the Effect plugin.
    fn set_effect(&self, effect: bool) {
        let plugins = if effect {
            r#","plugins":[{"name":"@effect/language-service","diagnosticSeverity":{"globalDate":"error"}}]"#
        } else {
            ""
        };
        self.write(
            "tsconfig.json",
            &format!(
                r#"{{"compilerOptions":{{"strict":true,"composite":true,"outDir":"out"{plugins}}},"files":["src/index.ts"]}}"#
            ),
        );
    }

    /// Sets the mtime of `name` to `OLD`.
    fn make_old(&self, name: &str) {
        std::fs::File::options()
            .write(true)
            .open(self.0.join(name))
            .and_then(|file| file.set_modified(SystemTime::UNIX_EPOCH + OLD))
            .unwrap_or_else(|e| panic!("set the mtime of {name}: {e}"));
    }

    fn build_info(&self) -> PathBuf {
        self.0.join("out/tsconfig.tsbuildinfo")
    }

    /// The `version` that the build info records.
    fn build_info_version(&self) -> String {
        let text = std::fs::read_to_string(self.build_info()).expect("build info");
        let start = text.find(r#""version":""#).expect("a version") + r#""version":""#.len();
        text[start..][..text[start..].find('"').expect("the version end")].to_string()
    }

    /// True when the build info was not written after `make_old`.
    fn build_info_is_old(&self) -> bool {
        let modified = std::fs::metadata(self.build_info())
            .and_then(|m| m.modified())
            .expect("build info mtime");
        modified == SystemTime::UNIX_EPOCH + OLD
    }

    /// Runs `tsgo` with `args` and `--pretty false`: the exit code and
    /// stdout without the `-b` time stamps and empty lines.
    fn tsgo(&self, args: &[&str]) -> (Option<i32>, String) {
        let output = Command::new(env!("CARGO_BIN_EXE_tsgo"))
            .args(args)
            .args(["--pretty", "false"])
            .current_dir(&self.0)
            .stdin(Stdio::null())
            .output()
            .expect("run tsgo");
        let stdout = String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|line| !line.is_empty())
            .map(|line| match line.split_once(" - ") {
                Some((time, rest)) if time.ends_with("AM") || time.ends_with("PM") => rest,
                _ => line,
            })
            .collect::<Vec<_>>()
            .join("\n");
        (output.status.code(), stdout)
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const DATE: &str = "export const date = new Date();\n";
const CLEAN: &str = "export const value = 1;\n";
const DATE_ERROR: &str = "src/index.ts(1,21): error TS377068: This code constructs `new Date()`, date values are represented through `DateTime` from Effect. effect(globalDate)";

const STATUS_BUILT: &str = "Projects in this build: \n    * tsconfig.json\nProject 'tsconfig.json' is out of date because output file 'out/tsconfig.tsbuildinfo' does not exist\nBuilding project 'tsconfig.json'...";
const STATUS_UP_TO_DATE: &str = "Projects in this build: \n    * tsconfig.json\nProject 'tsconfig.json' is up to date because newest input 'src/index.ts' is older than output 'out/tsconfig.tsbuildinfo'";

/// The `-b --verbose` status of build info of version `from`, and the build.
fn status_other_version(from: &str, current: &str) -> String {
    format!(
        "Projects in this build: \n    * tsconfig.json\nProject 'tsconfig.json' is out of date because output for it was generated with version '{from}' that differs with current version '{current}'\nBuilding project 'tsconfig.json'..."
    )
}

// M2 (`is_valid_version` ignores the rules) and M3 (the `-b` status check
// and prefetch ignore them) rebuild every Effect project on every run. The
// output is the same, so only the status and the build info write show it.
#[test]
fn effect_build_info_is_reused() {
    let effect = build_info_version(true);
    let project = Project::new("effect-build-info-reused", true, CLEAN);
    assert_eq!(
        project.tsgo(&["-b", "--verbose"]),
        (Some(0), STATUS_BUILT.to_string())
    );
    assert_eq!(project.build_info_version(), effect);
    assert_eq!(
        project.tsgo(&["-b", "--verbose"]),
        (Some(0), STATUS_UP_TO_DATE.to_string())
    );

    // `tsgo -p` reuses the build info and does not write it again.
    let project = Project::new("effect-build-info-reused-p", true, DATE);
    assert_eq!(
        project.tsgo(&["-p", "."]),
        (Some(2), DATE_ERROR.to_string())
    );
    assert_eq!(project.build_info_version(), effect);
    project.make_old("out/tsconfig.tsbuildinfo");
    assert_eq!(
        project.tsgo(&["-p", "."]),
        (Some(2), DATE_ERROR.to_string())
    );
    assert!(
        project.build_info_is_old(),
        "the second run wrote the build info again"
    );
}

// The plugin turns on, then off, and the config mtime stays older than the
// build info, so only the version tells that the build info is of the other
// kind. M2 and M3 call the project up to date, and the Effect error is lost.
#[test]
fn build_info_of_the_other_kind_is_built_again() {
    let plain = build_info_version(false);
    let effect = build_info_version(true);
    let project = Project::new("effect-build-info-other-kind", false, DATE);
    assert_eq!(
        project.tsgo(&["-b", "--verbose"]),
        (Some(0), STATUS_BUILT.to_string())
    );
    assert_eq!(project.build_info_version(), plain);

    project.set_effect(true);
    project.make_old("tsconfig.json");
    project.make_old("src/index.ts");
    assert_eq!(
        project.tsgo(&["-b", "--verbose"]),
        (
            Some(2),
            format!("{}\n{DATE_ERROR}", status_other_version(&plain, &effect))
        )
    );
    assert_eq!(project.build_info_version(), effect);

    project.set_effect(false);
    project.make_old("tsconfig.json");
    assert_eq!(
        project.tsgo(&["-b", "--verbose"]),
        (Some(0), status_other_version(&effect, &plain))
    );
    assert_eq!(project.build_info_version(), plain);
}
