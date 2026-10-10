//! ts#64649: one emit context per file emit. Go makes one emit context and
//! one emit resolver for the JS and the d.ts part of a file
//! (compiler/emitter.go:50-53), so the d.ts transforms and printer see what
//! the JS transforms wrote on parse-tree nodes:
//! - `EFNoTrailingSourceMap` on the names of parameters that a decorator
//!   transform changed (legacydecorators.go:145, esdecorator.go:1821): the
//!   d.ts map loses the segment after the name;
//! - `EFNoLeadingComments` on member names of a class with decorators
//!   (esdecorator.go:1439): the d.ts loses the doc comment before the name;
//! - the type node of a typed variable name (typeeraser.go:192): the d.ts
//!   repeats the comment after the type.
//!
//! The port runs the two parts in two emitters on the emit pool (split
//! files) and with the d.ts twins. There the d.ts part imports the JS
//! part's emit nodes of parse-tree nodes before its transforms
//! (`EmitContext::export_parse_emit_nodes`). An emit with no JS transforms
//! (`--emitDeclarationOnly`, a d.ts that is pending alone in an incremental
//! emit, the API with only the d.ts) has no such data, as in Go.
//!
//! Each project of `fixtures/emit_context` holds the outputs of Go N'
//! (fed0bf24149f): `expected/all` (as configured) and `expected/dts`
//! (`--emitDeclarationOnly`). The bump D emit2 lane writes them with its
//! round 2 tool `gen-expected.sh`.
//! - `legacy-param` and `deco-legacy` are the emit2 skeptic's probes
//!   `c-legacy-param` and `s-deco-legacy`.
//! - In `legacy`, `es`, `legacy-param` and `deco-legacy` the JS part of
//!   each file needs the checker (import elision), so with the twins on the
//!   checker runs both transforms and its twin prints them.
//! - In `legacy-vms` and `es-vms` (`verbatimModuleSyntax`) the JS part of
//!   each file runs on the emit pool and the d.ts part on the checker.
//!   `js-split` has both: `a.js` (allowJs, isolatedModules, `@type` tags on
//!   variables) splits, `b.ts` (import elision) does not.
//!
//! The tests need the emit pool on: do not set `GOPORT_EMIT_THREADS=0`.

use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use ts_goport::core::enter_program;
use ts_goport::emitter::program_emit::{
    DtsTwinMode, EmitOptions, WriteFile, WriteFileData, emit, js_twin_print_count,
    set_dts_twin_mode,
};
use ts_goport::options::{CompilerOptions, Tristate};
use ts_goport::program::{
    emit_pool_job_count, format_diagnostic, release_program, try_load_version,
};

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/emit_context");

/// A fixture project and the emit paths that its files take with the emit
/// pool on.
struct Project {
    name: &'static str,
    /// A JS part runs on the emit pool, apart from its d.ts part.
    split: bool,
    /// A JS part runs on the checker, and its twin prints it.
    twin: bool,
}

const PROJECTS: [Project; 7] = [
    Project {
        name: "legacy-param",
        split: false,
        twin: true,
    },
    Project {
        name: "deco-legacy",
        split: false,
        twin: true,
    },
    Project {
        name: "legacy",
        split: false,
        twin: true,
    },
    Project {
        name: "legacy-vms",
        split: true,
        twin: false,
    },
    Project {
        name: "es",
        split: false,
        twin: true,
    },
    Project {
        name: "es-vms",
        split: true,
        twin: false,
    },
    Project {
        name: "js-split",
        split: true,
        twin: true,
    },
];

/// The Go N' outputs to compare with: `expected/all` or `expected/dts`.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    All,
    Dts,
}

/// One in-process emit setup.
struct Mode {
    name: &'static str,
    twins: DtsTwinMode,
    edit: fn(&mut CompilerOptions),
    kind: Kind,
    /// The JS parts can run on the emit pool and the twins.
    pool: bool,
}

const MODES: [Mode; 6] = [
    Mode {
        name: "pool, twins on",
        twins: DtsTwinMode::On,
        edit: |_| {},
        kind: Kind::All,
        pool: true,
    },
    Mode {
        name: "pool, twins off",
        twins: DtsTwinMode::Off,
        edit: |_| {},
        kind: Kind::All,
        pool: true,
    },
    Mode {
        name: "pool, twin check",
        twins: DtsTwinMode::Check,
        edit: |_| {},
        kind: Kind::All,
        pool: true,
    },
    Mode {
        name: "--checkers 1",
        twins: DtsTwinMode::On,
        edit: |options| options.checkers = Some(1),
        kind: Kind::All,
        pool: true,
    },
    Mode {
        name: "--singleThreaded (one emitter per file)",
        twins: DtsTwinMode::On,
        edit: |options| options.single_threaded = Tristate::True,
        kind: Kind::All,
        pool: false,
    },
    Mode {
        name: "--emitDeclarationOnly",
        twins: DtsTwinMode::On,
        edit: |options| options.emit_declaration_only = Tristate::True,
        kind: Kind::Dts,
        pool: false,
    },
];

#[test]
fn emit_matches_go_in_every_mode() {
    for project in &PROJECTS {
        for mode in &MODES {
            let what = format!("{} with {}", project.name, mode.name);
            set_dts_twin_mode(Some(mode.twins));
            let js_prints = js_twin_print_count();
            let run = emit_project(project.name, mode.edit);
            let js_prints = js_twin_print_count() - js_prints;
            set_dts_twin_mode(None);

            assert_eq!(run.diagnostics, Vec::<String>::new(), "{what}: diagnostics");
            assert_files(&run.files, &expected(project.name, mode.kind), &what);
            if mode.pool && project.split {
                assert!(run.pool_jobs > 0, "{what}: the emit pool got no job");
            }
            if mode.pool && project.twin && mode.twins != DtsTwinMode::Off {
                assert!(js_prints > 0, "{what}: the twins printed no JS part");
            }
        }
    }
}

#[test]
fn tsgo_matches_go_with_any_emit_pool() {
    for project in &PROJECTS {
        for env in [
            ("GOPORT_EMIT_THREADS", "0"),
            ("GOPORT_EMIT_THREADS", "1"),
            ("GOPORT_EMIT_THREADS", "8"),
            ("GOPORT_EARLY_EMIT", "0"),
        ] {
            let what = format!("{} with {}={}", project.name, env.0, env.1);
            let root = copy_project(project.name);
            let output = tsgo(&root, &["-p", "tsconfig.json"], Some(env));
            assert_success(&output, &what);
            assert_files(
                &read_tree(&root.join("out")),
                &expected(project.name, Kind::All),
                &what,
            );
            remove(&root);
        }
    }
}

/// `tsc -b` emits JS and d.ts together, so the d.ts sees the JS data. When
/// a first `tsc -b` or `tsc -p --incremental` emits only the JS, the next
/// one emits the d.ts alone (only it is pending): no JS transforms run in
/// that emit, so its d.ts equals the `--emitDeclarationOnly` output, as in
/// Go.
#[test]
fn build_and_incremental_emit_match_go() {
    for name in ["legacy-param", "es-vms"] {
        let root = copy_project(name);
        let output = tsgo(&root, &["-b", "tsconfig.json", "--incremental"], None);
        assert_success(&output, &format!("{name}: tsc -b"));
        assert_files(
            &read_tree(&root.join("out")),
            &expected(name, Kind::All),
            &format!("{name}: tsc -b"),
        );
        remove(&root);

        for flag in ["-b", "-p"] {
            let what = format!("{name}: tsc {flag}, JS then the d.ts alone");
            let root = copy_project(name);
            let first = [
                flag,
                "tsconfig.json",
                "--incremental",
                "--declaration",
                "false",
                "--declarationMap",
                "false",
            ];
            assert_success(&tsgo(&root, &first, None), &what);
            assert_success(
                &tsgo(&root, &[flag, "tsconfig.json", "--incremental"], None),
                &what,
            );
            let all = expected(name, Kind::All);
            let dts = expected(name, Kind::Dts);
            let mixed: BTreeMap<String, String> = all
                .keys()
                .map(|file| {
                    let source = if file.ends_with(".d.ts") || file.ends_with(".d.ts.map") {
                        &dts
                    } else {
                        &all
                    };
                    (file.clone(), source[file].clone())
                })
                .collect();
            assert_files(&read_tree(&root.join("out")), &mixed, &what);
            remove(&root);
        }
    }
}

/// The API `emitToString` with no `emitOnly` emits JS and d.ts together
/// (Go api/session.go:4109 `getEmitOnly` gives `EmitAll`), and with
/// `emitOnly` 2 the d.ts alone.
#[test]
fn api_emit_to_string_matches_go() {
    for project in &PROJECTS {
        let dir = Path::new(FIXTURES).join(project.name);
        let mut api = Api::start(&dir);
        api.call("initialize", None);
        let config = dir.join("tsconfig.json");
        let snapshot = api.call(
            "createSnapshot",
            Some(format!(
                r#"{{"openProjects":[{}]}}"#,
                json_string(&config.display().to_string())
            )),
        );
        let snapshot_id = snapshot.get("snapshot").number().to_string();
        let project_id = snapshot.get("projects").array()[0]
            .get("id")
            .str()
            .to_string();
        for (emit_only, kind) in [("", Kind::All), (r#","emitOnly":2"#, Kind::Dts)] {
            let what = format!("{}: emitToString{emit_only}", project.name);
            let result = api.call(
                "emitToString",
                Some(format!(
                    r#"{{"snapshot":{snapshot_id},"project":{}{emit_only}}}"#,
                    json_string(&project_id)
                )),
            );
            assert!(
                matches!(result.get("emitSkipped"), Json::Bool(false)),
                "{what}: emitSkipped"
            );
            assert_eq!(result.get("diagnostics").array().len(), 0, "{what}");
            let out = format!("{}/out/", dir.display());
            let files: BTreeMap<String, String> = result
                .get("outputFiles")
                .array()
                .iter()
                .map(|file| {
                    let name = file.get("fileName").str();
                    let name = name
                        .strip_prefix(&out)
                        .unwrap_or_else(|| panic!("{what}: {name} is not under {out}"));
                    (name.to_string(), file.get("text").str().to_string())
                })
                .collect();
            assert_files(&files, &expected(project.name, kind), &what);
        }
        api.stop();
    }
}

/// What one in-process emit wrote and returned.
struct Run {
    /// The written text by path relative to the project's `out` dir.
    files: BTreeMap<String, String>,
    diagnostics: Vec<String>,
    pool_jobs: usize,
}

/// Loads the project `name` with `edit` applied to its options, emits it
/// with a write callback that keeps the writes, and releases it.
fn emit_project(name: &str, edit: fn(&mut CompilerOptions)) -> Run {
    let config = format!("{FIXTURES}/{name}/tsconfig.json");
    let program = try_load_version(&config, edit)
        .unwrap_or_else(|error| panic!("cannot load {config}: {error}"));
    let writes: Arc<Mutex<BTreeMap<String, String>>> = Arc::default();
    let sink = Arc::clone(&writes);
    let out = format!("{FIXTURES}/{name}/out/");
    let write_file: WriteFile =
        Arc::new(move |file: &str, text: &str, _data: &mut WriteFileData| {
            let file = file
                .strip_prefix(&out)
                .unwrap_or_else(|| panic!("{file} is not under {out}"));
            sink.lock()
                .expect("writes lock")
                .insert(file.to_string(), text.to_string());
            Ok(())
        });
    let run = {
        let _scope = enter_program(Some(program));
        let result = emit(EmitOptions {
            write_file: Some(write_file),
            ..EmitOptions::default()
        });
        Run {
            files: std::mem::take(&mut *writes.lock().expect("writes lock")),
            diagnostics: result.diagnostics.iter().map(format_diagnostic).collect(),
            pool_jobs: emit_pool_job_count(),
        }
    };
    release_program(program);
    run
}

/// The Go N' outputs of the project `name`.
fn expected(name: &str, kind: Kind) -> BTreeMap<String, String> {
    let kind = match kind {
        Kind::All => "all",
        Kind::Dts => "dts",
    };
    read_tree(&Path::new(FIXTURES).join(name).join("expected").join(kind))
}

/// Asserts that `files` and `expected` have the same files with the same
/// text, and names each file that differs.
fn assert_files(files: &BTreeMap<String, String>, expected: &BTreeMap<String, String>, what: &str) {
    assert_eq!(
        files.keys().collect::<Vec<_>>(),
        expected.keys().collect::<Vec<_>>(),
        "{what}: written files"
    );
    for (file, text) in files {
        assert_eq!(text, &expected[file], "{what}: {file} differs from Go N'");
    }
}

/// The text of each file under `dir`, by relative path.
fn read_tree(dir: &Path) -> BTreeMap<String, String> {
    fn walk(root: &Path, dir: &Path, files: &mut BTreeMap<String, String>) {
        let entries =
            fs::read_dir(dir).unwrap_or_else(|error| panic!("read {}: {error}", dir.display()));
        for entry in entries {
            let path = entry.expect("a dir entry").path();
            if path.is_dir() {
                walk(root, &path, files);
            } else {
                let name = path
                    .strip_prefix(root)
                    .expect("under the root")
                    .to_string_lossy()
                    .into_owned();
                let text = fs::read_to_string(&path)
                    .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
                files.insert(name, text);
            }
        }
    }
    let mut files = BTreeMap::new();
    walk(dir, dir, &mut files);
    files
}

/// A copy of the project `name` (its config and `src`) in a new scratch
/// dir, so `tsgo` can write there.
fn copy_project(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after 1970")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "goport-emit-context-{name}-{}-{nanos}",
        std::process::id()
    ));
    let from = Path::new(FIXTURES).join(name);
    fs::create_dir_all(root.join("src")).expect("create the scratch dir");
    fs::copy(from.join("tsconfig.json"), root.join("tsconfig.json")).expect("copy the config");
    for (file, text) in read_tree(&from.join("src")) {
        fs::write(root.join("src").join(file), text).expect("copy a source file");
    }
    root
}

fn remove(root: &Path) {
    fs::remove_dir_all(root).unwrap_or_else(|error| panic!("remove {}: {error}", root.display()));
}

/// Runs `tsgo` in `root` with `args`, and the environment variable `env`.
fn tsgo(root: &Path, args: &[&str], env: Option<(&str, &str)>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_tsgo"));
    command
        .current_dir(root)
        .args(args)
        .args(["--pretty", "false"]);
    if let Some((name, value)) = env {
        command.env(name, value);
    }
    command.output().expect("run tsgo")
}

fn assert_success(output: &Output, what: &str) {
    assert_eq!(
        (
            output.status.code(),
            String::from_utf8_lossy(&output.stdout)
        ),
        (Some(0), "".into()),
        "{what}: tsgo failed"
    );
}

/// A `tsgo --api --async` server on stdio (JSON-RPC).
struct Api {
    child: std::process::Child,
    stdout: BufReader<std::process::ChildStdout>,
    id: u32,
}

impl Api {
    fn start(dir: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_tsgo"))
            .args(["--api", "--async", "--cwd"])
            .arg(dir)
            .current_dir(dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("start tsgo --api");
        let stdout = BufReader::new(child.stdout.take().expect("stdout"));
        Api {
            child,
            stdout,
            id: 0,
        }
    }

    /// Sends a request with `params` (JSON text) and returns the result of
    /// its response.
    fn call(&mut self, method: &str, params: Option<String>) -> Json {
        self.id += 1;
        let params = params.map_or_else(String::new, |params| format!(r#","params":{params}"#));
        let body = format!(
            r#"{{"jsonrpc":"2.0","id":{},"method":"{method}"{params}}}"#,
            self.id
        );
        let stdin = self.child.stdin.as_mut().expect("stdin");
        write!(stdin, "Content-Length: {}\r\n\r\n{body}", body.len()).expect("write a request");
        stdin.flush().expect("flush a request");
        loop {
            let mut length = 0;
            loop {
                let mut line = String::new();
                self.stdout.read_line(&mut line).expect("read a header");
                assert!(!line.is_empty(), "tsgo --api closed during {method}");
                let line = line.trim_end();
                if line.is_empty() {
                    break;
                }
                if let Some(value) = line.strip_prefix("Content-Length:") {
                    length = value.trim().parse().expect("a content length");
                }
            }
            let mut body = vec![0; length];
            self.stdout.read_exact(&mut body).expect("read a message");
            let message = Json::parse(&String::from_utf8(body).expect("UTF-8 message"));
            if matches!(message.get("id"), Json::Number(id) if *id == self.id.to_string()) {
                assert!(
                    matches!(message.get("error"), Json::Null),
                    "{method}: {message:?}"
                );
                return message.get("result").clone();
            }
        }
    }

    fn stop(mut self) {
        drop(self.child.stdin.take());
        self.child.wait().expect("wait for tsgo --api");
    }
}

/// `text` as a JSON string.
fn json_string(text: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if u32::from(c) < 0x20 => {
                write!(out, "\\u{:04x}", u32::from(c)).expect("write to a String");
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A JSON value: enough to read the API responses. A number keeps its text.
#[derive(Clone, Debug)]
enum Json {
    Null,
    Bool(bool),
    Number(String),
    String(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}

impl Json {
    fn parse(text: &str) -> Json {
        let mut parser = JsonParser {
            chars: text.chars().collect(),
            pos: 0,
        };
        let value = parser.value();
        parser.space();
        assert_eq!(parser.pos, parser.chars.len(), "text after the JSON value");
        value
    }

    /// The field `key` of an object, or `Null`.
    fn get(&self, key: &str) -> &Json {
        match self {
            Json::Object(fields) => fields
                .iter()
                .find(|(name, _)| name == key)
                .map_or(&Json::Null, |(_, value)| value),
            _ => &Json::Null,
        }
    }

    fn str(&self) -> &str {
        match self {
            Json::String(text) => text,
            other => panic!("not a JSON string: {other:?}"),
        }
    }

    fn number(&self) -> &str {
        match self {
            Json::Number(text) => text,
            other => panic!("not a JSON number: {other:?}"),
        }
    }

    fn array(&self) -> &[Json] {
        match self {
            Json::Array(items) => items,
            other => panic!("not a JSON array: {other:?}"),
        }
    }
}

struct JsonParser {
    chars: Vec<char>,
    pos: usize,
}

impl JsonParser {
    fn space(&mut self) {
        while self.chars.get(self.pos).is_some_and(|c| c.is_whitespace()) {
            self.pos += 1;
        }
    }

    fn next(&mut self) -> char {
        let c = *self.chars.get(self.pos).expect("JSON ends early");
        self.pos += 1;
        c
    }

    fn word(&mut self, word: &str, value: Json) -> Json {
        for expected in word.chars() {
            assert_eq!(self.next(), expected, "bad JSON literal");
        }
        value
    }

    fn value(&mut self) -> Json {
        self.space();
        match self.chars.get(self.pos).copied().expect("JSON ends early") {
            '{' => {
                self.pos += 1;
                let mut fields = Vec::new();
                loop {
                    self.space();
                    match self.next() {
                        '}' => return Json::Object(fields),
                        ',' => {}
                        '"' => {
                            let key = self.string();
                            self.space();
                            assert_eq!(self.next(), ':', "bad JSON object");
                            fields.push((key, self.value()));
                        }
                        c => panic!("bad JSON object at {c:?}"),
                    }
                }
            }
            '[' => {
                self.pos += 1;
                let mut items = Vec::new();
                loop {
                    self.space();
                    match self.chars.get(self.pos) {
                        Some(']') => {
                            self.pos += 1;
                            return Json::Array(items);
                        }
                        Some(',') => self.pos += 1,
                        _ => items.push(self.value()),
                    }
                }
            }
            '"' => {
                self.pos += 1;
                Json::String(self.string())
            }
            't' => self.word("true", Json::Bool(true)),
            'f' => self.word("false", Json::Bool(false)),
            'n' => self.word("null", Json::Null),
            _ => {
                let start = self.pos;
                while self
                    .chars
                    .get(self.pos)
                    .is_some_and(|c| c.is_ascii_digit() || "+-.eE".contains(*c))
                {
                    self.pos += 1;
                }
                assert!(self.pos > start, "bad JSON value");
                Json::Number(self.chars[start..self.pos].iter().collect())
            }
        }
    }

    /// The rest of a string after its opening quote.
    fn string(&mut self) -> String {
        let mut out = String::new();
        loop {
            match self.next() {
                '"' => return out,
                '\\' => match self.next() {
                    'n' => out.push('\n'),
                    't' => out.push('\t'),
                    'r' => out.push('\r'),
                    'b' => out.push('\u{8}'),
                    'f' => out.push('\u{c}'),
                    'u' => {
                        let unit = self.hex4();
                        let code = if (0xd800..0xdc00).contains(&unit) {
                            assert_eq!((self.next(), self.next()), ('\\', 'u'), "lone surrogate");
                            0x10000 + ((unit - 0xd800) << 10) + (self.hex4() - 0xdc00)
                        } else {
                            unit
                        };
                        out.push(char::from_u32(code).expect("a JSON escape is a char"));
                    }
                    c => out.push(c),
                },
                c => out.push(c),
            }
        }
    }

    fn hex4(&mut self) -> u32 {
        (0..4).fold(0, |value, _| {
            value * 16 + self.next().to_digit(16).expect("a hex digit")
        })
    }
}
