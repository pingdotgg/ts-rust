//! Port-only test of the names of files under a directory named like a
//! drive (`c:`).
//!
//! PORT: no Go counterpart. Go (ts#64159) writes the name of a file relative
//! to the current directory with RelativePathFromDirectory
//! (tspath/relative_path.go:82), which gives the absolute name when the
//! relative name looks rooted: `<cwd>/c:/a.ts` is `c:/a.ts` relative, which
//! has a DOS root. The port's diagnostic writer (diagnosticwriter.go:27
//! formatFileNameRelativeTo), `--explainFiles` (program.go:2159,
//! fileInclude.go:155) and `tsc -b` (build/orchestrator.go:94) printed
//! `c:/a.ts`. A file reached through module resolution gets the real path
//! (`<cwd>/c:/...`, osvfs os.go:142), which is how the fuzzer found it. Only
//! a one-letter name followed by `:` is a DOS root (`x:y`, `ab:c` are not).
//!
//! The test runs `tsgo` in a scratch dir, as the names come from the OS
//! realpath. Every expected output is the output of `tsgo-oracle-3e9236361fa9`
//! on the same files, with its scratch dir replaced.

use std::path::PathBuf;
use std::process::{Command, Stdio};

/// A scratch dir with the files of one case. It is deleted on drop.
struct Project(PathBuf);

impl Project {
    fn new(name: &str, files: &[(&str, &str)]) -> Project {
        let dir = std::env::temp_dir().join(format!(
            "tsgo_drive_like_segment_{}_{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // The names have the real path of the dir, as Go's realpath.
        let dir = std::fs::canonicalize(&dir).unwrap();
        for (file, text) in files {
            let path = dir.join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        Project(dir)
    }

    fn dir(&self) -> String {
        self.0.to_str().unwrap().to_string()
    }

    /// Runs `tsgo` with `args` and gives the exit code and the output.
    fn tsc(&self, args: &[&str]) -> (Option<i32>, String) {
        let output = Command::new(env!("CARGO_BIN_EXE_tsgo"))
            .args(args)
            .current_dir(&self.0)
            .stdin(Stdio::null())
            .output()
            .expect("run tsgo");
        let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
        text.push_str(&String::from_utf8_lossy(&output.stderr));
        (output.status.code(), text)
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// `text` without its ANSI escapes.
fn strip_escapes(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            for c in chars.by_ref() {
                if c == 'm' {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// The fuzzer's repro: a module resolved to `<cwd>/c:/node_modules/...`.
#[test]
fn resolved_module_under_a_drive_like_dir_has_its_absolute_name() {
    let project = Project::new(
        "resolved",
        &[
            ("c:/root/a.ts", "import \"file4\";\n"),
            (
                "c:/node_modules/file4/index.d.ts",
                "let x: number = \"s\";\n",
            ),
            (
                "tsconfig.json",
                r#"{"compilerOptions":{"noEmit":true},"files":["c:/root/a.ts"]}"#,
            ),
        ],
    );
    let dir = project.dir();
    let file4 = format!("{dir}/c:/node_modules/file4/index.d.ts");
    let diagnostics = format!(
        "{file4}(1,1): error TS1046: Top-level declarations in .d.ts files must start with either a 'declare' or 'export' modifier.
{file4}(1,5): error TS2322: Type 'string' is not assignable to type 'number'.
{file4}(1,17): error TS1039: Initializers are not allowed in ambient contexts.
"
    );

    let (code, output) = project.tsc(&["-p", "tsconfig.json", "--pretty", "false"]);
    assert_eq!(output, diagnostics);
    assert_eq!(code, Some(2));

    let (code, output) =
        project.tsc(&["-p", "tsconfig.json", "--pretty", "false", "--explainFiles"]);
    assert!(output.starts_with(&diagnostics), "{output}");
    let explained = format!(
        "{file4}
   Imported via \"file4\" from file 'c:/root/a.ts'
c:/root/a.ts
   Part of 'files' list in tsconfig.json
"
    );
    assert!(output.ends_with(&explained), "{output}");
    assert_eq!(code, Some(2));
}

/// A root file named by its absolute path, and names with a `:` that are
/// not a DOS root, in the pretty output and its summary.
#[test]
fn pretty_names_under_a_drive_like_dir_are_absolute() {
    let project = Project::new(
        "pretty",
        &[
            ("c:/root/a.ts", "import \"./b\";\nlet y: string = 1;\n"),
            ("c:/root/b.ts", "export {}; let z: string = 2;\n"),
            ("x:y/c.ts", "let w: string = 3;\n"),
            ("ab:c/e.ts", "let v: string = 4;\n"),
        ],
    );
    let dir = project.dir();
    std::fs::write(
        project.0.join("tsconfig.json"),
        format!(
            r#"{{"compilerOptions":{{"noEmit":true}},"files":["{dir}/c:/root/a.ts","x:y/c.ts","ab:c/e.ts"]}}"#
        ),
    )
    .unwrap();

    let (code, output) = project.tsc(&["-p", "tsconfig.json", "--pretty", "true"]);
    assert_eq!(
        strip_escapes(&output),
        format!(
            "ab:c/e.ts:1:5 - error TS2322: Type 'number' is not assignable to type 'string'.

1 let v: string = 4;
      ~

{dir}/c:/root/a.ts:2:5 - error TS2322: Type 'number' is not assignable to type 'string'.

2 let y: string = 1;
      ~

{dir}/c:/root/b.ts:1:16 - error TS2322: Type 'number' is not assignable to type 'string'.

1 export {{}}; let z: string = 2;
                 ~

x:y/c.ts:1:5 - error TS2322: Type 'number' is not assignable to type 'string'.

1 let w: string = 3;
      ~


Found 4 errors in 4 files.

Errors  Files
     1  ab:c/e.ts:1
     1  {dir}/c:/root/a.ts:2
     1  {dir}/c:/root/b.ts:1
     1  x:y/c.ts:1

"
        )
    );
    assert_eq!(code, Some(2));
}

/// `tsc -b --verbose` of a project under a drive-like dir.
#[test]
fn build_names_under_a_drive_like_dir_are_absolute() {
    let project = Project::new(
        "build",
        &[
            ("c:/p/a.ts", "export const a = 1;\nlet x: number = \"s\";\n"),
            (
                "c:/p/tsconfig.json",
                r#"{"compilerOptions":{"outDir":"out"},"files":["a.ts"]}"#,
            ),
        ],
    );
    let dir = project.dir();
    let config = format!("{dir}/c:/p");
    let (code, output) = project.tsc(&["-b", &config, "--verbose", "--pretty", "false"]);
    // Without the time of each status line.
    let output = output
        .lines()
        .map(|line| match line.split_once(" - ") {
            Some((time, rest)) if time.ends_with("AM") || time.ends_with("PM") => rest,
            _ => line,
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(
        output,
        format!(
            "Projects in this build: \n    * {config}/tsconfig.json\n\n\
             Project '{config}/tsconfig.json' is out of date because output file '{config}/out/tsconfig.tsbuildinfo' does not exist\n\n\
             Building project '{config}/tsconfig.json'...\n\n\
             {config}/a.ts(2,5): error TS2322: Type 'string' is not assignable to type 'number'."
        )
    );
    assert_eq!(code, Some(2));
}
