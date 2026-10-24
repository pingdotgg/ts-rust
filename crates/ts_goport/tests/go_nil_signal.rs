//! gopanicout2. Go's runtime lines for a nil dereference that nothing
//! recovers: line 1 is the panic value, line 2 is the signal line of the
//! SIGSEGV with the fault address of the faulting load at the pinned Go
//! (`code=0x1 addr=<address>`, Linux amd64). The port prints `pc=0x0`,
//! because the `pc` is an instruction of Go's own binary
//! (`core::go_nil_dereference_at`).
//!
//! Each case is a `tsgo` run that Go N' (fed0bf24149f) ends with these two
//! lines and exit code 2 in each run (gopanicout2 `go-model.md` section 1,
//! `tools/cliprobe.py` and `tools/lspprobe.py`), and Go at 3e9236361fa9 with
//! the same addresses (nilsig1, 5 runs of each case).
#![cfg(target_os = "linux")]

use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

const NIL_DEREFERENCE: &str =
    "panic: runtime error: invalid memory address or nil pointer dereference";

/// The signal line of a nil dereference at `address`, with the port's `pc`.
fn signal(address: u64) -> String {
    format!("[signal SIGSEGV: segmentation violation code=0x1 addr={address:#x} pc=0x0]")
}

/// A new empty project directory, removed on drop.
struct Project(PathBuf);

impl Project {
    fn new() -> Project {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "go-nil-signal-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create the project dir");
        Project(dir)
    }

    fn write(&self, name: &str, text: &str) {
        let path = self.0.join(name);
        std::fs::create_dir_all(path.parent().expect("a file in the project"))
            .expect("create a project subdir");
        std::fs::write(path, text).expect("write a project file");
    }

    fn read(&self, name: &str) -> String {
        std::fs::read_to_string(self.0.join(name)).expect("read a project file")
    }

    /// `tsgo <args>` in the project: the exit code and the first two
    /// stderr lines.
    fn tsgo(&self, args: &[&str]) -> (Option<i32>, Vec<String>) {
        let output = Command::new(env!("CARGO_BIN_EXE_tsgo"))
            .args(args)
            .current_dir(&self.0)
            .env_remove("GOPORT_TRACE")
            .output()
            .expect("run tsgo");
        (output.status.code(), first_lines(&output.stderr))
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn first_lines(stderr: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(stderr)
        .lines()
        .take(2)
        .map(str::to_string)
        .collect()
}

/// `tsc -b` after the config of a referenced project is removed: the
/// upstream task never loads a build info, and the downstream task reads
/// `t.buildInfoEntry.dtsTime` (build/buildtask.go:895, offset 0x40). With
/// more than 1 builder the task runs in a `WaitGroup.Go` goroutine.
#[test]
fn build_with_a_removed_reference_config() {
    let project = Project::new();
    project.write(
        "a/tsconfig.json",
        r#"{"compilerOptions":{"composite":true},"files":["a.ts"],"references":[{"path":"../b"}]}"#,
    );
    project.write(
        "a/a.ts",
        "import { b } from \"../b/b\";\nexport const a = b;\n",
    );
    project.write(
        "b/tsconfig.json",
        r#"{"compilerOptions":{"composite":true},"files":["b.ts"]}"#,
    );
    project.write("b/b.ts", "export const b = 1;\n");
    assert_eq!(project.tsgo(&["-b", "a"]).0, Some(0), "the first build");
    std::fs::remove_file(project.0.join("b/tsconfig.json")).expect("remove b's config");
    for (builders, suffix) in [("4", " [recovered, repanicked]"), ("1", "")] {
        assert_eq!(
            project.tsgo(&["-b", "a", "--builders", builders]),
            (
                Some(2),
                vec![format!("{NIL_DEREFERENCE}{suffix}"), signal(0x40)]
            ),
            "--builders {builders}"
        );
    }
}

/// An empty `files` list in a config whose top level is not an object:
/// `nodeValue` is nil, and `CreateDiagnosticForNodeInSourceFile` reads
/// `node.Loc.Pos()` (tsoptions/errors.go:93, offset 0x8).
#[test]
fn empty_files_list_in_a_config_that_is_not_an_object() {
    let project = Project::new();
    project.write("tsconfig.json", r#"[{"files": []}]"#);
    project.write("a.ts", "export const a = 1;\n");
    assert_eq!(
        project.tsgo(&["-p", "tsconfig.json"]),
        (Some(2), vec![NIL_DEREFERENCE.to_string(), signal(0x8)])
    );
}

/// A build info whose `semanticDiagnosticsPerFile` holds the plain file id
/// 0: Go reads `diagnostic.Diagnostics.FileId` of a nil `Diagnostics`
/// (incremental/buildinfotosnapshot.go:163, offset 0).
#[test]
fn build_info_with_a_plain_file_id_0_in_the_semantic_diagnostics() {
    let project = Project::new();
    project.write(
        "tsconfig.json",
        r#"{"compilerOptions":{"incremental":true,"strict":true},"files":["a.ts","b.ts"]}"#,
    );
    project.write("a.ts", "export const a: number = 1;\n");
    project.write("b.ts", "export const b: string = 2;\n");
    project.tsgo(&["-p", "tsconfig.json"]);
    let build_info = project.read("tsconfig.tsbuildinfo");
    let key = r#""semanticDiagnosticsPerFile":"#;
    let start = build_info
        .find(key)
        .expect("semantic diagnostics in the build info")
        + key.len();
    let end = start + json_value_len(&build_info[start..]);
    project.write(
        "tsconfig.tsbuildinfo",
        &format!("{}[0]{}", &build_info[..start], &build_info[end..]),
    );
    project.write("a.ts", "export const a: number = 1;\nexport const z = 3;\n");
    for args in [
        &["-p", "tsconfig.json"][..],
        &["-p", "tsconfig.json", "--singleThreaded"],
    ] {
        assert_eq!(
            project.tsgo(args),
            (Some(2), vec![NIL_DEREFERENCE.to_string(), signal(0x0)]),
            "{args:?}"
        );
    }
}

/// The length of the JSON array or object at the start of `text`.
fn json_value_len(text: &str) -> usize {
    let mut depth = 0;
    let mut in_string = false;
    let mut escaped = false;
    for (index, c) in text.char_indices() {
        if in_string {
            match c {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => in_string = true,
            '[' | '{' => depth += 1,
            ']' | '}' => {
                depth -= 1;
                if depth == 0 {
                    return index + 1;
                }
            }
            _ => {}
        }
    }
    panic!("no JSON value end in {text}")
}

/// The text of `a.ts` in the LSP project (`lsp_session`).
const A_TS: &str = "const x = 1;\nexport function f(a: number) { return a + x; }\n";

/// The `initialized` notification.
const INITIALIZED: &str = r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#;

/// `lsp_session` with `a.ts` open: sends `messages` after `initialize`,
/// `initialized` and `didOpen`.
fn lsp_run(messages: &[String]) -> (Option<i32>, Vec<String>) {
    lsp_session(|root| {
        let text = A_TS.replace('\n', "\\n");
        let mut all = vec![
            format!(
                r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"processId":null,"rootUri":"{root}","capabilities":{{}},"workspaceFolders":[{{"uri":"{root}","name":"p"}}]}}}}"#
            ),
            INITIALIZED.to_string(),
            format!(
                r#"{{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{{"textDocument":{{"uri":"{root}/a.ts","languageId":"typescript","version":1,"text":"{text}"}}}}}}"#
            ),
        ];
        all.extend_from_slice(messages);
        all
    })
}

/// `tsgo --lsp --stdio` in a project with `a.ts` (`A_TS`): sends the
/// messages that `messages` makes from the project's root URI, answers the
/// server's requests, and returns the exit code and the first two stderr
/// lines when the server exits (at most 60 s).
fn lsp_session(messages: impl FnOnce(&str) -> Vec<String>) -> (Option<i32>, Vec<String>) {
    let project = Project::new();
    project.write("a.ts", A_TS);
    project.write("tsconfig.json", r#"{"compilerOptions":{"strict":true}}"#);
    let root = format!("file://{}", project.0.display());
    let mut child = Command::new(env!("CARGO_BIN_EXE_tsgo"))
        .args(["--lsp", "--stdio"])
        .current_dir(&project.0)
        .env_remove("GOPORT_TRACE")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run tsgo --lsp");
    let stdin = std::sync::Arc::new(std::sync::Mutex::new(child.stdin.take().expect("stdin")));
    let send = |stdin: &std::sync::Mutex<std::process::ChildStdin>, body: &str| {
        let mut stdin = stdin.lock().expect("the stdin lock");
        let _ = write!(stdin, "Content-Length: {}\r\n\r\n{body}", body.len());
        let _ = stdin.flush();
    };
    let mut stdout = BufReader::new(child.stdout.take().expect("stdout"));
    let answers = stdin.clone();
    std::thread::spawn(move || {
        while let Some(body) = read_message(&mut stdout) {
            if let (Some(id), Some(method)) = (json_field(&body, "id"), json_field(&body, "method"))
            {
                let result = if method == "\"workspace/configuration\"" {
                    let items = body.matches("\"section\"").count().max(1);
                    format!("[{}]", vec!["null"; items].join(","))
                } else {
                    "null".to_string()
                };
                send(
                    &answers,
                    &format!(r#"{{"jsonrpc":"2.0","id":{id},"result":{result}}}"#),
                );
            }
        }
    });
    let mut stderr = child.stderr.take().expect("stderr");
    let stderr = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stderr.read_to_end(&mut bytes);
        bytes
    });
    for message in messages(&root) {
        send(&stdin, &message);
    }
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().expect("wait for tsgo --lsp") {
            break status;
        }
        if start.elapsed() > Duration::from_mins(1) {
            let _ = child.kill();
            break child.wait().expect("wait for the killed tsgo --lsp");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    (
        status.code(),
        first_lines(&stderr.join().expect("the stderr reader")),
    )
}

/// One Content-Length message body. None at the end.
fn read_message(stdout: &mut impl BufRead) -> Option<String> {
    let mut length = 0;
    loop {
        let mut line = String::new();
        if stdout.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some(value) = line.strip_prefix("Content-Length:") {
            length = value.trim().parse().ok()?;
        }
    }
    let mut body = vec![0; length];
    stdout.read_exact(&mut body).ok()?;
    String::from_utf8(body).ok()
}

/// The raw JSON text of the top-level `field` of a message whose fields
/// are scalars before `params`: enough for `id` and `method`.
fn json_field(body: &str, field: &str) -> Option<String> {
    let head = body.split(r#""params""#).next()?;
    let start = head.find(&format!(r#""{field}":"#))? + field.len() + 3;
    let rest = &head[start..];
    let end = if let Some(text) = rest.strip_prefix('"') {
        text.find('"')? + 2
    } else {
        rest.find([',', '}'])?
    };
    Some(rest[..end].to_string())
}

/// LSP notifications and requests from a client with a nil element or a
/// missing pointer field, which the sync part of the Go handler reads
/// (no `recover`): `change.Type` (project/session.go:428, offset 0x10),
/// `file.OldUri` (lsp/server.go:2016, offset 0) and `codeLens.Data.Uri`
/// (lsp/server.go:2271, offset 0x10).
#[test]
fn lsp_messages_with_a_nil_element() {
    let range = r#"{"start":{"line":0,"character":0},"end":{"line":0,"character":5}}"#;
    for (message, address) in [
        (
            r#"{"jsonrpc":"2.0","method":"workspace/didChangeWatchedFiles","params":{"changes":[null]}}"#
                .to_string(),
            0x10,
        ),
        (
            r#"{"jsonrpc":"2.0","id":9,"method":"workspace/willRenameFiles","params":{"files":[null]}}"#
                .to_string(),
            0x0,
        ),
        (
            format!(
                r#"{{"jsonrpc":"2.0","id":9,"method":"codeLens/resolve","params":{{"range":{range}}}}}"#
            ),
            0x10,
        ),
    ] {
        assert_eq!(
            lsp_run(std::slice::from_ref(&message)),
            (
                Some(2),
                vec![NIL_DEREFERENCE.to_string(), signal(address)]
            ),
            "{message}"
        );
    }
}

/// gopanicout3. The handler of `initialized` (no `recover`) reads what
/// `initialize` sets: `s.initializationOptions`, nil before `initialize`
/// (lsp/server.go:1717, offset 0), and, when the client has the workspace
/// folders capability, the only folder of `workspaceFolders: [null]`
/// (lsp/server.go:1753, offset 0x8: the length word of `Uri`).
#[test]
fn lsp_initialized_reads_a_nil_initialize_state() {
    assert_eq!(
        lsp_session(|_| vec![INITIALIZED.to_string()]),
        (Some(2), vec![NIL_DEREFERENCE.to_string(), signal(0x0)]),
        "initialized before initialize"
    );
    assert_eq!(
        lsp_session(|root| vec![
            format!(
                r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"processId":null,"rootUri":"{root}","capabilities":{{"workspace":{{"workspaceFolders":true}}}},"workspaceFolders":[null]}}}}"#
            ),
            INITIALIZED.to_string(),
        ]),
        (Some(2), vec![NIL_DEREFERENCE.to_string(), signal(0x8)]),
        "workspaceFolders [null]"
    );
}
