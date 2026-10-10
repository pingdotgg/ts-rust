//! Resolve ahead (src/frontend/compiler/resolve_ahead.rs). Not a Go test:
//! the port's language server loads resolve the keys of the previous load
//! on worker threads. A load whose loader takes every worker answer that
//! passes the check (`Mode::Force`) must give the same program, module
//! resolutions, seen files, missing directories and cached files as a load
//! whose loader resolves every key itself (`Mode::Off`).
//!
//! Resolve ahead runs only on the OS file system, so each test writes a
//! project to a temp directory and runs with no OS override (not
//! `child_test!`, which installs one). It is off when the OS file system is
//! case-insensitive (the default macOS volumes), where the tests are
//! skipped (`resolve_ahead_off`).

use std::collections::BTreeSet;
use std::rc::Rc;

use ts_goport::flags::ModuleKind;
use ts_goport::frontend::bundled;
use ts_goport::frontend::compiler::resolve_ahead::{self, LoadStats, Mode};
use ts_goport::frontend::tspath;
use ts_goport::frontend::vfs::osvfs_fs;
use ts_goport::lsp::lsproto;
use ts_goport::project::{self, Session, SessionInit, SessionOptions};

use super::projecttestutil;
use super::util::{CHANGED, DELETED, bg, close, edit, generate_file_events, open, program, uri};

/// A test in a child process with no OS override, with the environment
/// variables `$env` set (after `with_a_worker`). It is skipped, with a
/// message, where resolve ahead cannot run (`resolve_ahead_off`).
macro_rules! os_child_test {
    ($(#[$meta:meta])* fn $name:ident() $body:block) => {
        os_child_test!(env &[]; $(#[$meta])* fn $name() $body);
    };
    (env $env:expr; $(#[$meta:meta])* fn $name:ident() $body:block) => {
        $(#[$meta])*
        #[test]
        fn $name() {
            let path = concat!(module_path!(), "::", stringify!($name));
            let test = path.split_once("::").map_or(path, |(_, rest)| rest);
            if let Some(reason) = resolve_ahead_off() {
                eprintln!("{test}: skipped: {reason}");
                return;
            }
            crate::support::child::run_test_in_child_with_env(test, &with_a_worker($env), || $body);
        }
    };
}

/// `env` after `GOPORT_RESOLVE_AHEAD_THREADS=1` when this process can use
/// only one core (`program::available_cores`, Go `runtime.GOMAXPROCS`, which
/// `GOMAXPROCS=1` sets). Resolve ahead has one worker per parse thread less
/// the loading thread (`resolve_ahead::worker_count`), so on one core it
/// starts none, and the tests check worker answers. A value in `env` wins
/// (the child's environment takes the last one).
// PORT: not in Go (resolve ahead is a port feature).
fn with_a_worker<'a>(env: &[(&'a str, &'a str)]) -> Vec<(&'a str, &'a str)> {
    let one_core = ts_goport::program::available_cores() == 1;
    let mut all = Vec::new();
    if one_core {
        all.push(("GOPORT_RESOLVE_AHEAD_THREADS", "1"));
    }
    all.extend_from_slice(env);
    all
}

/// Why resolve ahead cannot run in these tests, if it cannot: it is off on
/// a case-insensitive file system (project/compilerhost.rs
/// `CompilerHost::resolve_ahead`), which the OS file system decides from
/// the executable's path (`use_case_sensitive_file_names`). The default
/// macOS volumes are case-insensitive; Linux file systems are not, so the
/// tests run there. A temp dir on a case-insensitive volume does not turn
/// resolve ahead off, so it does not skip the tests either
/// (`a_case_insensitive_temp_dir_does_not_skip_the_tests`).
// PORT: not in Go (resolve ahead is a port feature).
fn resolve_ahead_off() -> Option<String> {
    (!ts_goport::frontend::vfs::Fs::use_case_sensitive_file_names(&*osvfs_fs())).then(|| {
        "the OS file system is case-insensitive here, and resolve ahead is off".to_string()
    })
}

const INDEX: &str = r#"import { a } from "./a";
import { b } from "./sub/b";
import { c } from "../lib/c";
import { p } from "pkg";
import { o } from "pkg/other";
import { m } from "@scope/lib";
import { x } from "./missing/x";
import { y } from "not-installed";
export const all = [a, b, c, p, o, m, x, y];
"#;

/// The project of every test: relative imports, a file outside `include`,
/// packages with `types` and `exports`, a missing directory and a missing
/// package.
const FILES: &[(&str, &str)] = &[
    (
        "tsconfig.json",
        r#"{ "compilerOptions": { "module": "esnext", "moduleResolution": "bundler", "noLib": true, "strict": true }, "include": ["src"] }"#,
    ),
    ("src/index.ts", INDEX),
    ("src/a.ts", "export const a = 1;"),
    ("src/sub/b.ts", "export const b = 2;"),
    ("src/sub/d.ts", "export const d = 6;"),
    ("lib/c.ts", "export const c = 3;"),
    (
        "node_modules/pkg/package.json",
        r#"{ "name": "pkg", "version": "1.0.0", "types": "index.d.ts" }"#,
    ),
    (
        "node_modules/pkg/index.d.ts",
        "export declare const p: number;",
    ),
    (
        "node_modules/pkg/other.d.ts",
        "export declare const o: number;",
    ),
    (
        "node_modules/@scope/lib/package.json",
        r#"{ "name": "@scope/lib", "version": "2.0.0", "exports": { ".": { "types": "./dist/main.d.ts" } } }"#,
    ),
    (
        "node_modules/@scope/lib/dist/main.d.ts",
        "export declare const m: number;",
    ),
];

/// A new temp directory with `FILES`, its real path.
fn make_project(label: &str) -> String {
    let root = std::env::temp_dir().join(format!(
        "ts_goport_resolve_ahead_{}_{label}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    for (name, text) in FILES {
        write(&root.to_string_lossy(), name, text);
    }
    std::fs::canonicalize(&root)
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/")
}

pub(super) fn write(root: &str, name: &str, text: &str) {
    let path = std::path::Path::new(root).join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

pub(super) fn file_uri(root: &str, name: &str) -> String {
    format!("file://{root}/{name}")
}

/// A session on the OS file system in `root`, with no client, watch or
/// typings installer.
pub(super) fn os_session(root: &str) -> Rc<Session> {
    project::new_session(&SessionInit {
        background_ctx: bg(),
        options: Rc::new(SessionOptions {
            current_directory: root.to_string(),
            default_library_path: bundled::lib_path(),
            typings_location: String::new(),
            position_encoding: lsproto::PositionEncodingKind::UTF8,
            watch_enabled: false,
            logging_enabled: false,
            ..projecttestutil::session_options(root)
        }),
        fs: bundled::wrap_fs(osvfs_fs()),
        client: None,
        logger: None,
        npm_executor: None,
        spawner: None,
        content_mapper_logger: None,
        parse_cache: None,
        content_mapped_parse_cache: None,
    })
}

/// What a program load leaves: the program's files, their package scopes
/// and module resolutions, the files that its host saw, the directories
/// that it found missing and the snapshot's cached files. Paths are
/// relative to the project root.
#[derive(Debug, PartialEq, Eq)]
struct Observed {
    files: Vec<String>,
    missing_files: Vec<String>,
    /// Per file: the package.json directory and type, and the implied node
    /// format (`SourceFileMetaData`).
    metadata: BTreeSet<String>,
    resolutions: BTreeSet<String>,
    seen: BTreeSet<String>,
    missing_directories: BTreeSet<String>,
    cached_files: BTreeSet<String>,
}

fn observe(session: &Rc<Session>, root: &str) -> Observed {
    observe_project(session, root, "tsconfig.json")
}

/// `observe` for the project of the config file `config` in `root`.
fn observe_project(session: &Rc<Session>, root: &str, config: &str) -> Observed {
    let relative = |name: &str| name.replace(root, "<root>");
    let snapshot = session.snapshot();
    let config = tspath::to_path(&format!("{root}/{config}"), root, true);
    let project = snapshot
        .project_collection
        .configured_project(&config)
        .expect("configured project");
    let project = project.borrow();
    let processed = &project.program.as_ref().expect("program").processed_files;
    let host = project.host.as_ref().expect("host");
    let paths = |set: &rustc_hash::FxHashSet<tspath::Path>| -> BTreeSet<String> {
        set.iter().map(|path| relative(path.as_str())).collect()
    };
    let mut resolutions = BTreeSet::new();
    for (file, cache) in processed.resolved_modules.iter() {
        for (key, module) in cache {
            resolutions.insert(relative(&format!(
                "{} {key:?} -> {} {} {} {:?} {} {} {:?}",
                file.as_str(),
                module.resolved_file_name,
                module.original_path,
                module.extension,
                module.package_id,
                module.is_external_library_import,
                module.resolved_using_ts_extension,
                module.resolution_diagnostics,
            )));
        }
    }
    Observed {
        files: processed
            .files
            .iter()
            .map(|file| relative(file.file_name()))
            .collect(),
        missing_files: processed
            .missing_files
            .iter()
            .map(|name| relative(name))
            .collect(),
        metadata: processed
            .source_file_meta_datas
            .iter()
            .map(|(file, meta)| {
                relative(&format!(
                    "{} {:?} {:?} {:?}",
                    file.as_str(),
                    meta.package_json_directory,
                    meta.package_json_type,
                    meta.implied_node_format,
                ))
            })
            .collect(),
        resolutions,
        // ts#64544: the seen files map each path to its file name.
        seen: paths(
            &host
                .source_fs
                .seen_files
                .borrow()
                .as_ref()
                .unwrap()
                .borrow()
                .keys()
                .cloned()
                .collect(),
        ),
        missing_directories: paths(
            &host
                .source_fs
                .missing_directories
                .as_ref()
                .unwrap()
                .borrow(),
        ),
        cached_files: snapshot
            .fs
            .cache_files
            .keys()
            .map(|path| relative(path.as_str()))
            .collect(),
    }
}

/// Runs `steps` on a new copy of the project in `mode`, and returns what
/// the last load left and its resolve-ahead counts. `steps` gets the
/// session and the project root and makes the loads.
fn run(
    label: &str,
    mode: Mode,
    steps: &dyn Fn(&Rc<Session>, &str),
) -> (Observed, Option<LoadStats>) {
    let root = make_project(&format!("{label}_{mode:?}"));
    resolve_ahead::set_mode(Some(mode));
    let session = os_session(&root);
    steps(&session, &root);
    let observed = observe(&session, &root);
    let stats = resolve_ahead::last_stats();
    resolve_ahead::set_mode(None);
    drop(session);
    std::fs::remove_dir_all(&root).unwrap();
    (observed, stats)
}

/// Runs `steps` with the loader resolving every key itself and with the
/// workers resolving every key first, and checks that the loads are the
/// same. Returns the resolve-ahead counts of the last load.
fn same_with_and_without(label: &str, steps: &dyn Fn(&Rc<Session>, &str)) -> LoadStats {
    let (serial, serial_stats) = run(label, Mode::Off, steps);
    let (ahead, stats) = run(label, Mode::Force, steps);
    assert_eq!(serial_stats, None, "resolve ahead ran in mode 0");
    assert_eq!(ahead, serial);
    let stats = stats.expect("no resolve-ahead load");
    assert!(stats.keys > 0, "{stats:?}");
    stats
}

/// Opens `src/index.ts` (the first load records its keys).
fn open_index(session: &Rc<Session>, root: &str) {
    open(session, &file_uri(root, "src/index.ts"), INDEX);
    program(session, &file_uri(root, "src/index.ts"));
}

/// Adds an import at the top of `src/index.ts` (version 2), which makes a
/// new program load.
fn add_import(session: &Rc<Session>, root: &str) {
    edit_index(session, root, 2, "import { d } from \"./sub/d\";\n");
}

/// Puts `text` at the top of `src/index.ts` as `version`, and loads the
/// program.
fn edit_index(session: &Rc<Session>, root: &str, version: i32, text: &str) {
    let uri = file_uri(root, "src/index.ts");
    edit(session, &uri, version, (0, 0), (0, 0), text);
    program(session, &uri);
}

/// The counts of the last resolve-ahead load on this thread.
fn last_stats() -> LoadStats {
    resolve_ahead::last_stats().expect("no resolve-ahead load")
}

/// Ends this test process when the test has not ended after `seconds`, so
/// a load that waits forever fails the test.
fn watchdog(seconds: u64) {
    use std::io::Write;
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(seconds));
        // stdout: a test can close stderr, and `eprintln!` panics there.
        let _ = writeln!(
            std::io::stdout(),
            "watchdog: the test did not end in {seconds} s"
        );
        std::process::exit(3);
    });
}

os_child_test! {
    /// An import edit: every key of the previous load is taken, and the
    /// load is the same as a serial one.
    fn takes_every_answer_of_an_unchanged_project() {
        let stats = same_with_and_without("unchanged", &|session, root| {
            open_index(session, root);
            add_import(session, root);
        });
        assert_eq!(stats.loader.rejected, 0, "{stats:?}");
        assert_eq!(stats.loader.taken, stats.keys, "{stats:?}");
        assert_eq!(stats.new_keys, stats.keys + 1, "{stats:?}");
    }
}

os_child_test! {
    /// The workers find the package scope of each directory of the previous
    /// load, and an import edit takes each of them: every file keeps the
    /// package.json directory and type of a serial load. `src/sub` has a
    /// package.json of its own, and the package `esm` has `"type":
    /// "module"`, which its files read (they are in node_modules).
    fn takes_the_package_scopes_of_the_previous_load() {
        let stats = same_with_and_without("scopes", &|session, root| {
            write(root, "src/sub/package.json", r#"{ "type": "commonjs" }"#);
            write(
                root,
                "node_modules/esm/package.json",
                r#"{ "name": "esm", "version": "1.0.0", "type": "module", "types": "index.d.ts" }"#,
            );
            write(root, "node_modules/esm/index.d.ts", "export declare const e: number;");
            let uri = file_uri(root, "src/index.ts");
            open(session, &uri, &format!("{INDEX}import {{ e }} from \"esm\";\n"));
            program(session, &uri);
            add_import(session, root);
            let observed = observe(session, root);
            for file in [
                "<root>/src/sub/d.ts \"<root>/src/sub\" \"\"",
                "<root>/node_modules/esm/index.d.ts \"<root>/node_modules/esm\" \"module\"",
            ] {
                assert!(
                    observed.metadata.iter().any(|meta| meta.starts_with(file)),
                    "{file}: {:?}",
                    observed.metadata
                );
            }
        });
        assert!(stats.scopes > 0, "{stats:?}");
        assert_eq!(stats.loader.scopes_rejected, 0, "{stats:?}");
        assert_eq!(stats.loader.scopes_taken, stats.scopes, "{stats:?}");
        // `./sub/d` adds a file to a directory that the load had.
        assert_eq!(stats.new_scopes, stats.scopes, "{stats:?}");
        assert_eq!(stats.loader.taken, stats.keys, "{stats:?}");
    }
}

os_child_test! {
    /// A load keeps the module names and their usages of each parse for the
    /// next loads (compiler/file_loader.rs `import_names`): an import edit
    /// in another file resolves the names of `src/modes.ts` with the modes
    /// of a load that reads the nodes (a serial load keeps none).
    fn keeps_the_module_names_and_modes_of_an_unchanged_file() {
        same_with_and_without("modes", &|session, root| {
            write(
                root,
                "src/modes.ts",
                "import type { p } from \"pkg\" with { \"resolution-mode\": \"require\" };\n\
                 export type O = typeof import(\"pkg/other\", { with: { \"resolution-mode\": \"import\" } });\n\
                 export const lazy = () => import(\"./a\");\n\
                 import b = require(\"./sub/b\");\n\
                 export { p, b };\n",
            );
            let uri = file_uri(root, "src/index.ts");
            open(session, &uri, &format!("{INDEX}import \"./modes\";\n"));
            program(session, &uri);
            add_import(session, root);
            let program = program(session, &uri);
            let modes = tspath::to_path(&format!("{root}/src/modes.ts"), root, true);
            let mut names: Vec<(String, ModuleKind)> = program
                .processed_files
                .resolved_modules
                .get(&modes)
                .expect("resolutions of src/modes.ts")
                .keys()
                .map(|key| (key.name.clone(), key.mode))
                .collect();
            names.sort_by(|a, b| a.0.cmp(&b.0));
            assert_eq!(
                names,
                [
                    ("./a".to_string(), ModuleKind::ES_NEXT),
                    ("./sub/b".to_string(), ModuleKind::COMMON_JS),
                    ("pkg".to_string(), ModuleKind::COMMON_JS),
                    ("pkg/other".to_string(), ModuleKind::ES_NEXT),
                ]
            );
        });
    }
}

os_child_test! {
    env &[("GOPORT_RESOLVE_AHEAD_THREADS", "1")];
    /// One worker resolves every key, so its package.json cache has the
    /// package.json of `pkg` when it resolves `pkg/other`. The answer must
    /// still list the calls of that read (debug builds check each taken
    /// answer against a new resolver's calls, `debug_check_answer`).
    fn one_worker_lists_the_package_json_reads_of_its_cache() {
        let stats = same_with_and_without("oneworker", &|session, root| {
            open_index(session, root);
            add_import(session, root);
        });
        assert_eq!(stats.loader.taken, stats.keys, "{stats:?}");
    }
}

os_child_test! {
    env &[("GOPORT_RESOLVE_AHEAD_THREADS", "1")];
    /// The one worker keeps the package.json parse of `@scope/lib` from
    /// the second load. Its directory is removed with no watch event, so
    /// the snapshot still has the package.json, but Go's lookup asks
    /// whether the directory exists and finds no package: the worker must
    /// not answer from the kept parse.
    fn a_kept_package_json_of_a_removed_directory_is_not_used() {
        let stats = same_with_and_without("keptgone", &|session, root| {
            open_index(session, root);
            add_import(session, root);
            std::fs::remove_dir_all(format!("{root}/node_modules/@scope/lib")).unwrap();
            let uri = file_uri(root, "src/index.ts");
            edit(
                session,
                &uri,
                3,
                (0, 0),
                (0, 0),
                "import { b as b2 } from \"./sub/b\";\n",
            );
            program(session, &uri);
        });
        assert!(stats.loader.missing >= 1, "{stats:?}");
    }
}

os_child_test! {
    /// The edit removes the import of `../lib/c`: the workers resolve its
    /// key, the loader never asks for it, and its file is not seen.
    fn an_untaken_answer_adds_no_seen_file() {
        let stats = same_with_and_without("untaken", &|session, root| {
            open_index(session, root);
            let uri = file_uri(root, "src/index.ts");
            edit(session, &uri, 2, (2, 0), (3, 0), "");
            program(session, &uri);
            let observed = observe(session, root);
            assert!(!observed.seen.contains("<root>/lib/c.ts"), "{observed:?}");
            assert!(observed.files.iter().all(|file| !file.ends_with("lib/c.ts")));
        });
        assert_eq!(stats.loader.taken + 1, stats.keys, "{stats:?}");
    }
}

os_child_test! {
    /// The disk changes between the loads with no watch event, so the
    /// snapshot still has the old texts and files: a changed package.json,
    /// a deleted source file, a new file in a new directory, a removed
    /// package directory. The workers see the new disk, and the check
    /// rejects each answer that the snapshot would make in another way.
    fn rejects_answers_that_the_snapshot_files_contradict() {
        let stats = same_with_and_without("disk", &|session, root| {
            open_index(session, root);
            write(
                root,
                "node_modules/pkg/package.json",
                r#"{ "name": "pkg", "version": "1.0.1", "types": "other.d.ts" }"#,
            );
            std::fs::remove_file(format!("{root}/src/a.ts")).unwrap();
            write(root, "src/missing/x.ts", "export const x = 4;");
            std::fs::remove_dir_all(format!("{root}/node_modules/@scope/lib")).unwrap();
            add_import(session, root);
        });
        assert!(stats.loader.rejected >= 2, "{stats:?}");
        assert!(stats.loader.taken > 0, "{stats:?}");
    }
}

os_child_test! {
    /// More than `EXCESSIVE_CHANGE_THRESHOLD` watch events in node_modules
    /// (an npm install) mark the cached node_modules files for a reload.
    /// The loader's own lookup would reload them (and drop the cached file
    /// that is gone), so the check rejects the answers that look them up
    /// until the loader has reloaded them.
    fn rejects_an_answer_whose_cached_file_needs_a_reload() {
        let stats = same_with_and_without("reload", &|session, root| {
            open_index(session, root);
            write(
                root,
                "node_modules/pkg/package.json",
                r#"{ "name": "pkg", "version": "1.0.2", "types": "other.d.ts" }"#,
            );
            std::fs::remove_file(format!("{root}/node_modules/@scope/lib/dist/main.d.ts")).unwrap();
            let mut events =
                generate_file_events(1001, &file_uri(root, "node_modules/pkg/f%d.d.ts"), CHANGED);
            events.push(Some(lsproto::FileEvent {
                uri: uri(&file_uri(root, "node_modules/pkg/package.json")),
                type_: CHANGED,
            }));
            session.did_change_watched_files(&bg(), &events);
            add_import(session, root);
        });
        assert!(stats.loader.rejected >= 1, "{stats:?}");
    }
}

os_child_test! {
    /// followups19 (R167 reviewer): a package.json that `file_exists` finds
    /// but that no read can open, here a Unix socket (as when a package is
    /// removed between the two calls). The worker resolves `sock` with a
    /// failed read, and the load must not take that answer: the snapshot
    /// does not cache a failed read, so a later read in the load could find
    /// the file. The loader resolves the key itself. The workers do not
    /// know the file from an earlier job, so the answer is not shared
    /// (followups22, `AheadFs::read_file`), and no check rejects it.
    #[cfg(unix)]
    fn a_failed_package_json_read_is_not_taken() {
        let stats = same_with_and_without("fr", &|session, root| {
            write(
                root,
                "node_modules/sock/index.d.ts",
                "export declare const k: number;",
            );
            std::os::unix::net::UnixListener::bind(format!("{root}/node_modules/sock/package.json"))
                .unwrap();
            let uri = file_uri(root, "src/index.ts");
            open(session, &uri, &format!("{INDEX}import {{ k }} from \"sock\";\n"));
            program(session, &uri);
            add_import(session, root);
            let observed = observe(session, root);
            assert!(
                observed
                    .resolutions
                    .iter()
                    .any(|resolution| resolution.contains("\"sock\"")
                        && resolution.contains("-> <root>/node_modules/sock/index.d.ts")),
                "{observed:?}"
            );
        });
        assert_eq!(stats.loader.rejected, 0, "{stats:?}");
        assert_eq!(stats.loader.scopes_rejected, 0, "{stats:?}");
        assert_eq!(stats.loader.taken + 1, stats.keys, "{stats:?}");
    }
}

os_child_test! {
    /// followups22 (R169 reviewer): a package.json that is there but that
    /// no read can open (no read permission). The worker read fails at each
    /// load. The loader resolves the key itself, with Go's answer, as in a
    /// serial load. No rejection drops what the workers keep: the next load
    /// starts with the known files of the one before. When the check
    /// rejected the answer, every load dropped them (`Workers::forget`).
    #[cfg(unix)]
    fn a_never_readable_package_json_does_not_drop_the_kept_state() {
        let stats = same_with_and_without("locked", &|session, root| {
            use std::os::unix::fs::PermissionsExt;
            write(
                root,
                "node_modules/locked/index.d.ts",
                "export declare const k: number;",
            );
            let package_json = format!("{root}/node_modules/locked/package.json");
            write(root, "node_modules/locked/package.json", r#"{ "name": "locked" }"#);
            std::fs::set_permissions(&package_json, std::fs::Permissions::from_mode(0o000))
                .unwrap();
            let locked = std::fs::File::open(&package_json).is_err();
            let uri = file_uri(root, "src/index.ts");
            open(session, &uri, &format!("{INDEX}import {{ k }} from \"locked\";\n"));
            program(session, &uri);
            add_import(session, root);
            if let Some(stats) = resolve_ahead::last_stats().filter(|_| locked) {
                assert_eq!(stats.loader.rejected, 0, "{stats:?}");
                assert_eq!(stats.loader.scopes_rejected, 0, "{stats:?}");
                assert_eq!(stats.loader.taken + 1, stats.keys, "{stats:?}");
            }
            resolve_ahead::wait_for_frees();
            edit_index(session, root, 3, "import { b as b2 } from \"./sub/b\";\n");
            let observed = observe(session, root);
            assert!(
                observed
                    .resolutions
                    .iter()
                    .any(|resolution| resolution.contains("\"locked\"")
                        && resolution.contains("-> <root>/node_modules/locked/index.d.ts")),
                "{observed:?}"
            );
            if !locked {
                eprintln!("skipped: a file without read permission opens here");
            }
        });
        assert_eq!(stats.loader.rejected, 0, "{stats:?}");
        assert!(stats.known_files > 0, "{stats:?}");
    }
}

/// The second project of `the_snapshot_lookups_of_an_earlier_load_reject_an_answer`.
const P2_FILES: &[(&str, &str)] = &[
    (
        "p2/tsconfig.json",
        r#"{ "compilerOptions": { "module": "esnext", "moduleResolution": "bundler", "noLib": true, "strict": true }, "include": ["src"] }"#,
    ),
    (
        "p2/src/index.ts",
        "import { o } from \"pkg/other\";\nexport const v = o;\n",
    ),
];

os_child_test! {
    /// R167 reviewer (followups15a): a test where `agrees` rejects. One
    /// snapshot loads two projects that both resolve `pkg/other`. The disk
    /// changes after the workers of the first load (a test hook):
    /// `node_modules/pkg/other.ts` appears. The workers of the second load
    /// find it, but the snapshot has the first load's answer for the path
    /// (its job's lookups, `AheadLookupLayer::before`), so the check rejects
    /// the answer, and the loader's own resolution finds that answer too. As
    /// in Go, whose snapshot caches the first answer of each call, both
    /// projects resolve `pkg/other` to `other.d.ts`.
    fn the_snapshot_lookups_of_an_earlier_load_reject_an_answer() {
        let root = make_project("agrees");
        for (name, text) in P2_FILES {
            write(&root, name, text);
        }
        resolve_ahead::set_mode(Some(Mode::Force));
        let session = os_session(&root);
        let uris = [
            file_uri(&root, "src/index.ts"),
            file_uri(&root, "p2/src/index.ts"),
        ];
        open(&session, &uris[0], INDEX);
        program(&session, &uris[0]);
        open(&session, &uris[1], P2_FILES[1].1);
        program(&session, &uris[1]);
        // New imports, so each project loads its program again.
        edit(
            &session,
            &uris[0],
            2,
            (0, 0),
            (0, 0),
            "import { d } from \"./sub/d\";\n",
        );
        edit(
            &session,
            &uris[1],
            2,
            (0, 0),
            (0, 0),
            "import { p } from \"pkg\";\n",
        );
        let loads = Rc::new(std::cell::Cell::new(0));
        let hook_loads = loads.clone();
        let hook_root = root.clone();
        resolve_ahead::set_after_workers(Some(Rc::new(move || {
            hook_loads.set(hook_loads.get() + 1);
            write(&hook_root, "node_modules/pkg/other.ts", "export const o = 5;");
        })));
        session.get_snapshot(
            &bg(),
            project::ResourceRequest {
                documents: uris.iter().map(|u| uri(u)).collect(),
                ..Default::default()
            },
            false, /*callerRef*/
        );
        resolve_ahead::set_after_workers(None);
        assert_eq!(loads.get(), 2, "the two loads of one snapshot");
        let stats = last_stats();
        assert_eq!(stats.loader.rejected, 1, "{stats:?}");
        let snapshot = session.snapshot();
        for (config, file) in [
            ("tsconfig.json", "src/index.ts"),
            ("p2/tsconfig.json", "p2/src/index.ts"),
        ] {
            let config = tspath::to_path(&format!("{root}/{config}"), &root, true);
            let project = snapshot
                .project_collection
                .configured_project(&config)
                .expect("configured project");
            let project = project.borrow();
            let program = project.program.as_ref().expect("program");
            let index = tspath::to_path(&format!("{root}/{file}"), &root, true);
            let (_, module) = program
                .processed_files
                .resolved_modules
                .get(&index)
                .and_then(|modules| modules.iter().find(|(key, _)| key.name == "pkg/other"))
                .expect("the resolution of pkg/other");
            assert_eq!(
                module.resolved_file_name,
                format!("{root}/node_modules/pkg/other.d.ts"),
                "{file}"
            );
        }
        resolve_ahead::set_mode(None);
        drop(snapshot);
        drop(session);
        std::fs::remove_dir_all(&root).unwrap();
    }
}

os_child_test! {
    /// Open files that are not on disk: a new file in a new directory, which
    /// the workers see through the open file paths, and an open
    /// package.json, whose text the workers do not have (its answers are
    /// not shared).
    fn open_files_over_the_disk() {
        let stats = same_with_and_without("open", &|session, root| {
            open(session, &file_uri(root, "src/newdir/z.ts"), "export const z = 5;");
            open(
                session,
                &file_uri(root, "node_modules/pkg/package.json"),
                r#"{ "name": "pkg", "version": "1.0.3", "types": "other.d.ts" }"#,
            );
            let text = format!("import {{ z }} from \"./newdir/z\";\n{INDEX}");
            open(session, &file_uri(root, "src/index.ts"), &text);
            program(session, &file_uri(root, "src/index.ts"));
            add_import(session, root);
            let observed = observe(session, root);
            assert!(
                observed
                    .resolutions
                    .iter()
                    .any(|resolution| resolution.contains("\"./newdir/z\"")
                        && resolution.contains("-> <root>/src/newdir/z.ts")),
                "{observed:?}"
            );
        });
        assert!(stats.loader.taken > 0, "{stats:?}");
        assert!(stats.loader.missing > 0, "{stats:?}");
    }
}

os_child_test! {
    /// A rejected answer makes the workers drop what they keep (the known
    /// files and the package.json parses). When a load on another thread
    /// has the workers at the next load, the next load that has them must
    /// still start with none.
    fn a_rejected_answer_drops_the_kept_state_while_the_workers_are_taken() {
        let root = make_project("heldreject");
        resolve_ahead::set_mode(Some(Mode::Force));
        let session = os_session(&root);
        open_index(&session, &root);
        add_import(&session, &root);
        resolve_ahead::wait_for_frees();
        // An npm install: the cached node_modules files need a reload, so
        // the check rejects the answers that look them up.
        let events =
            generate_file_events(1001, &file_uri(&root, "node_modules/pkg/f%d.d.ts"), CHANGED);
        session.did_change_watched_files(&bg(), &events);
        edit_index(&session, &root, 3, "import { b as b2 } from \"./sub/b\";\n");
        let stats = last_stats();
        assert!(stats.loader.rejected >= 1, "{stats:?}");
        assert!(stats.known_files > 0, "{stats:?}");
        resolve_ahead::wait_for_frees();
        let held = resolve_ahead::hold_workers();
        edit_index(&session, &root, 4, "import { a as a2 } from \"./a\";\n");
        let stats = last_stats();
        assert_eq!(stats.loader.taken, 0, "the workers ran: {stats:?}");
        drop(held);
        edit_index(&session, &root, 5, "import { c as c2 } from \"../lib/c\";\n");
        let stats = last_stats();
        assert!(stats.loader.taken > 0, "{stats:?}");
        assert_eq!(stats.known_files, 0, "{stats:?}");
        resolve_ahead::set_mode(None);
        drop(session);
        std::fs::remove_dir_all(&root).unwrap();
    }
}

os_child_test! {
    /// A worker that panics outside a resolution (a port bug; a test hook
    /// makes one) must not make a load that waits for the workers
    /// (`Mode::Force`) wait forever, nor leave the pool: that load resolves
    /// its keys itself, and the next load takes every answer again. The
    /// workers panic after they resolved their keys, so their answers are
    /// there: the load takes none of them (`AheadQueue::fail`).
    fn a_worker_panic_outside_a_resolution_makes_the_load_serial() {
        watchdog(120);
        let stats = same_with_and_without("panic", &|session, root| {
            open_index(session, root);
            resolve_ahead::inject_worker_panic(true);
            add_import(session, root);
            resolve_ahead::inject_worker_panic(false);
            if let Some(stats) = resolve_ahead::last_stats() {
                assert!(stats.worker_panic, "{stats:?}");
                assert_eq!(stats.loader.taken, 0, "{stats:?}");
            }
            edit_index(session, root, 3, "import { b as b2 } from \"./sub/b\";\n");
        });
        assert!(!stats.worker_panic, "{stats:?}");
        assert_eq!(stats.loader.taken, stats.keys, "{stats:?}");
    }
}

os_child_test! {
    env &[("GOPORT_RESOLVE_AHEAD_STATS", "1")];
    /// The debug log (`GOPORT_RESOLVE_AHEAD_STATS=1`) writes to stderr. When
    /// stderr is a pipe with no reader, each write fails (EPIPE). A worker
    /// that caught a panic logs it before it counts itself out of the job
    /// (`run_task`), so a write that panics (R162: `eprintln!`) ends the
    /// worker there, and a load that waits for the workers (`Mode::Force`)
    /// waits forever. The load must resolve its keys itself, and the next
    /// load must take every answer, so no worker left the pool.
    #[cfg(unix)]
    fn a_worker_panic_logs_to_a_closed_stderr_and_the_load_goes_on() {
        watchdog(120);
        close_stderr();
        let root = make_project("closedstderr");
        resolve_ahead::set_mode(Some(Mode::Force));
        let session = os_session(&root);
        open_index(&session, &root);
        resolve_ahead::inject_worker_panic(true);
        add_import(&session, &root);
        resolve_ahead::inject_worker_panic(false);
        let stats = last_stats();
        assert!(stats.worker_panic, "{stats:?}");
        assert_eq!(stats.loader.taken, 0, "{stats:?}");
        edit_index(&session, &root, 3, "import { b as b2 } from \"./sub/b\";\n");
        let stats = last_stats();
        assert!(!stats.worker_panic, "{stats:?}");
        assert_eq!(stats.loader.taken, stats.keys, "{stats:?}");
        resolve_ahead::set_mode(None);
        drop(session);
        std::fs::remove_dir_all(&root).unwrap();
    }
}

/// Makes stderr a pipe with no reader, so each write to it fails with EPIPE
/// (a Rust program ignores SIGPIPE). A closed fd 2 would not do: the
/// standard library drops writes to it (EBADF) with no error.
#[cfg(unix)]
fn close_stderr() {
    let (reader, writer) = std::io::pipe().expect("a pipe");
    drop(reader);
    rustix::stdio::dup2_stderr(&writer).expect("stderr to the pipe");
}

/// The project of `a_released_project_drops_the_kept_state` in `other/`,
/// with its own package.
const OTHER_MAIN: &str =
    "import { q } from \"./q\";\nimport { r } from \"opkg\";\nexport const both = [q, r];\n";
const OTHER_FILES: &[(&str, &str)] = &[
    (
        "other/tsconfig.json",
        r#"{ "compilerOptions": { "module": "esnext", "moduleResolution": "bundler", "noLib": true, "strict": true }, "include": ["src"] }"#,
    ),
    ("other/src/main.ts", OTHER_MAIN),
    ("other/src/q.ts", "export const q = 1;"),
    (
        "other/node_modules/opkg/package.json",
        r#"{ "name": "opkg", "version": "1.0.0", "types": "index.d.ts" }"#,
    ),
    (
        "other/node_modules/opkg/index.d.ts",
        "export declare const r: number;",
    ),
];

os_child_test! {
    /// The workers keep the files that the answers of a project's loads
    /// found, from load to load of the project. When the project's
    /// programs are released (its only open file is closed and the file of
    /// another project opens), they drop them: the next job, of the other
    /// project, starts with no known file.
    fn a_released_project_drops_the_kept_state() {
        let root = make_project("released");
        for (name, text) in OTHER_FILES {
            write(&root, name, text);
        }
        resolve_ahead::set_mode(Some(Mode::Force));
        let session = os_session(&root);
        open_index(&session, &root);
        add_import(&session, &root);
        resolve_ahead::wait_for_frees();
        edit_index(&session, &root, 3, "import { b as b2 } from \"./sub/b\";\n");
        let stats = last_stats();
        assert!(stats.known_files > 0, "{stats:?}");
        close(&session, &file_uri(&root, "src/index.ts"));
        let main = file_uri(&root, "other/src/main.ts");
        open(&session, &main, OTHER_MAIN);
        program(&session, &main);
        let config = tspath::to_path(&format!("{root}/tsconfig.json"), &root, true);
        assert!(
            session
                .snapshot()
                .project_collection
                .configured_project(&config)
                .is_none(),
            "the first project is still open"
        );
        resolve_ahead::wait_for_frees();
        edit(&session, &main, 2, (0, 0), (0, 0), "import { q as q2 } from \"./q\";\n");
        program(&session, &main);
        let stats = last_stats();
        assert!(stats.loader.taken > 0, "{stats:?}");
        assert_eq!(stats.known_files, 0, "{stats:?}");
        resolve_ahead::set_mode(None);
        drop(session);
        std::fs::remove_dir_all(&root).unwrap();
    }
}

const SPEC_MAIN: &str = "import { p } from \"pkg\";\nexport const foo = p;\n";
const SPEC_HELPER: &str = "export const bar = 2;\n";
const SPEC_TEST: &str =
    "import { foo } from \"./main\";\nimport { bar } from \"./helper\";\nfoo + bar;\n";

/// A solution with a build and a spec project over one `src`, as in hono:
/// the project search of a file open in the build project makes the spec
/// project, loads it and deletes it.
const SPEC_FILES: &[(&str, &str)] = &[
    (
        "tsconfig.json",
        r#"{ "files": [], "references": [{ "path": "./tsconfig.build.json" }, { "path": "./tsconfig.spec.json" }] }"#,
    ),
    (
        "tsconfig.build.json",
        r#"{ "compilerOptions": { "module": "esnext", "moduleResolution": "bundler", "noLib": true, "types": [] }, "include": ["src/**/*.ts"], "exclude": ["src/**/*.test.ts"] }"#,
    ),
    (
        "tsconfig.spec.json",
        r#"{ "compilerOptions": { "module": "esnext", "moduleResolution": "bundler", "noLib": true, "types": [], "jsx": "react-jsx" }, "include": ["src/**/*.ts"] }"#,
    ),
    ("src/main.ts", SPEC_MAIN),
    ("src/helper.ts", SPEC_HELPER),
    ("src/main.test.ts", SPEC_TEST),
    (
        "node_modules/pkg/package.json",
        r#"{ "name": "pkg", "version": "1.0.0", "types": "index.d.ts" }"#,
    ),
    (
        "node_modules/pkg/index.d.ts",
        "export declare const p: number;",
    ),
];

os_child_test! {
    /// A project that a clone made and deleted gives its keys and its
    /// share in the kept state to the session's stash
    /// (`project::ResolveAheadStash`). The clone that makes it again
    /// resolves ahead the keys of its deleted load, and its workers keep
    /// what the jobs of the deleted project found.
    fn a_project_made_again_takes_the_answers_of_its_deleted_load() {
        let root = std::env::temp_dir().join(format!(
            "ts_goport_resolve_ahead_{}_made_again",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        for (name, text) in SPEC_FILES {
            write(&root.to_string_lossy(), name, text);
        }
        let root = std::fs::canonicalize(&root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        resolve_ahead::set_mode(Some(Mode::Force));
        let session = os_session(&root);
        let spec = tspath::to_path(&format!("{root}/tsconfig.spec.json"), &root, true);
        let spec_is_open = || {
            session
                .snapshot()
                .project_collection
                .configured_project(&spec)
                .is_some()
        };

        // The first spec load has no keys; the clone deletes the project.
        open(&session, &file_uri(&root, "src/main.ts"), SPEC_MAIN);
        assert!(!spec_is_open(), "the first open keeps the spec project");

        // The clone of the next open makes the spec project again, and its
        // load takes every answer. The clone deletes it again.
        resolve_ahead::wait_for_frees();
        open(&session, &file_uri(&root, "src/helper.ts"), SPEC_HELPER);
        assert!(!spec_is_open(), "the second open keeps the spec project");
        let stats = last_stats();
        assert!(stats.keys > 0, "{stats:?}");
        assert_eq!(stats.loader.rejected, 0, "{stats:?}");
        assert_eq!(stats.loader.taken, stats.keys, "{stats:?}");

        // A file of the spec project only: the spec project made again
        // keeps it. Its job starts with the files that the job of the
        // deleted project found.
        resolve_ahead::wait_for_frees();
        open(&session, &file_uri(&root, "src/main.test.ts"), SPEC_TEST);
        assert!(spec_is_open(), "the spec project is not open");
        let stats = last_stats();
        assert!(stats.keys > 0, "{stats:?}");
        assert_eq!(stats.loader.taken, stats.keys, "{stats:?}");
        assert!(stats.known_files > 0, "{stats:?}");
        resolve_ahead::set_mode(None);
        drop(session);
        std::fs::remove_dir_all(&root).unwrap();
    }
}

/// `SPEC_TEST` with an import of `tpkg`, a package that only the spec
/// project reads.
const SPEC_TEST_TPKG: &str = "import { foo } from \"./main\";\nimport { bar } from \"./helper\";\nimport { t } from \"tpkg\";\nfoo + bar + t;\n";

os_child_test! {
    /// The disk changes between the delete and the remake of a project
    /// whose keys and kept share are in the stash
    /// (`project::ResolveAheadStash`): `tpkg/index.d.ts`, which the job of
    /// the deleted project found, is deleted. The remade project's workers
    /// start with it as a known file and answer with it; the loader's
    /// check rejects that answer, so the load is the load of a loader that
    /// resolves every key itself.
    fn a_change_between_delete_and_remake_gives_the_fresh_answer() {
        let run = |mode: Mode| {
            let dir = std::env::temp_dir().join(format!(
                "ts_goport_resolve_ahead_{}_remake_{mode:?}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            let files = SPEC_FILES.iter().copied().chain([
                ("src/main.test.ts", SPEC_TEST_TPKG),
                (
                    "node_modules/tpkg/package.json",
                    r#"{ "name": "tpkg", "version": "1.0.0", "types": "index.d.ts" }"#,
                ),
                ("node_modules/tpkg/index.d.ts", "export declare const t: number;"),
            ]);
            for (name, text) in files {
                write(&dir.to_string_lossy(), name, text);
            }
            let root = std::fs::canonicalize(&dir)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            resolve_ahead::set_mode(Some(mode));
            let session = os_session(&root);

            // The search makes the spec project, loads it and deletes it,
            // twice. The second load resolves ahead, and its job finds the
            // files of tpkg, which the stash keeps for the next job.
            open(&session, &file_uri(&root, "src/main.ts"), SPEC_MAIN);
            resolve_ahead::wait_for_frees();
            open(&session, &file_uri(&root, "src/helper.ts"), SPEC_HELPER);
            resolve_ahead::wait_for_frees();

            std::fs::remove_file(format!("{root}/node_modules/tpkg/index.d.ts")).unwrap();
            session.did_change_watched_files(
                &bg(),
                &[Some(lsproto::FileEvent {
                    uri: uri(&file_uri(&root, "node_modules/tpkg/index.d.ts")),
                    type_: DELETED,
                })],
            );

            // A file of the spec project only: the spec project made again
            // keeps it.
            open(&session, &file_uri(&root, "src/main.test.ts"), SPEC_TEST_TPKG);
            let observed = observe_project(&session, &root, "tsconfig.spec.json");
            let stats = resolve_ahead::last_stats();
            resolve_ahead::set_mode(None);
            drop(session);
            std::fs::remove_dir_all(&dir).unwrap();
            (observed, stats)
        };
        let (serial, serial_stats) = run(Mode::Off);
        let (ahead, stats) = run(Mode::Force);
        assert_eq!(serial_stats, None, "resolve ahead ran in mode 0");
        assert_eq!(ahead, serial);
        let tpkg: Vec<_> = ahead
            .resolutions
            .iter()
            .filter(|line| line.contains("\"tpkg\""))
            .collect();
        assert!(
            !tpkg.is_empty() && tpkg.iter().all(|line| !line.contains("tpkg/index.d.ts")),
            "{tpkg:?}"
        );
        let stats = stats.expect("no resolve-ahead load");
        assert!(stats.known_files > 0, "{stats:?}");
        assert!(stats.loader.rejected > 0, "{stats:?}");
    }
}

/// The import of `rpkg`, a package whose node_modules directory is a
/// symlink (`symlinked_rpkg`).
const RPKG_INDEX: &str = "import { r } from \"rpkg\";\n";

/// Adds `store/rpkg` and `store/rpkg2`, two copies of a package, and
/// `node_modules/rpkg`, a symlink to the first.
#[cfg(unix)]
fn symlinked_rpkg(root: &str) {
    for copy in ["rpkg", "rpkg2"] {
        write(
            root,
            &format!("store/{copy}/package.json"),
            r#"{ "name": "rpkg", "version": "1.0.0", "types": "index.d.ts" }"#,
        );
        write(
            root,
            &format!("store/{copy}/index.d.ts"),
            "export declare const r: number;",
        );
    }
    point_rpkg(root, "rpkg");
}

/// Points the symlink `node_modules/rpkg` to `store/<copy>`.
#[cfg(unix)]
fn point_rpkg(root: &str, copy: &str) {
    let link = format!("{root}/node_modules/rpkg");
    let _ = std::fs::remove_file(&link);
    std::os::unix::fs::symlink(format!("{root}/store/{copy}"), &link).unwrap();
}

/// The resolution of `name` in `src/index.ts`: the resolved file name and
/// the original path, relative to the project root.
#[cfg(unix)]
fn resolution(session: &Rc<Session>, root: &str, name: &str) -> (String, String) {
    let program = program(session, &file_uri(root, "src/index.ts"));
    let index = tspath::to_path(&format!("{root}/src/index.ts"), root, true);
    let modules = program
        .processed_files
        .resolved_modules
        .get(&index)
        .expect("resolutions of src/index.ts");
    let (_, module) = modules
        .iter()
        .find(|(key, _)| key.name == name)
        .unwrap_or_else(|| panic!("no resolution of {name}"));
    let relative = |name: &str| name.replace(root, "<root>");
    (
        relative(&module.resolved_file_name),
        relative(&module.original_path),
    )
}

os_child_test! {
    /// The disk changes during a load: after the workers resolved the keys
    /// of the previous load and before the loader starts (a test hook), a
    /// new directory gets a file, a package gets a new file and a package
    /// symlink points to another copy. The loader takes the worker answers,
    /// which saw the old disk, and then resolves new keys that make the
    /// same `directory_exists`, `file_exists` and `realpath` calls. As in Go,
    /// whose snapshot caches the first answer of each call, each such call
    /// must give the answer that the load took, so the program has one
    /// answer for each path.
    #[cfg(unix)]
    fn a_load_has_one_answer_per_path_when_the_disk_changes_during_it() {
        let root = make_project("onepath");
        symlinked_rpkg(&root);
        resolve_ahead::set_mode(Some(Mode::Force));
        let session = os_session(&root);
        let uri = file_uri(&root, "src/index.ts");
        open(&session, &uri, &format!("{INDEX}{RPKG_INDEX}"));
        program(&session, &uri);
        let changed_root = root.clone();
        resolve_ahead::set_after_workers(Some(Rc::new(move || {
            let root = &changed_root;
            write(root, "src/missing/x.ts", "export const x = 4;");
            write(root, "node_modules/pkg/other.ts", "export const o = 5;");
            point_rpkg(root, "rpkg2");
        })));
        edit(
            &session,
            &uri,
            2,
            (10, 0),
            (10, 0),
            "import { x as x2 } from \"./missing/x.js\";\n\
             import { o as o2 } from \"pkg/other.js\";\n\
             import { r as r2 } from \"rpkg/index.js\";\n",
        );
        program(&session, &uri);
        resolve_ahead::set_after_workers(None);
        let stats = last_stats();
        assert_eq!(stats.loader.taken, stats.keys, "{stats:?}");
        assert_eq!(stats.loader.rejected, 0, "{stats:?}");
        for (taken, own) in [
            ("./missing/x", "./missing/x.js"),
            ("pkg/other", "pkg/other.js"),
            ("rpkg", "rpkg/index.js"),
        ] {
            assert_eq!(
                resolution(&session, &root, own).0,
                resolution(&session, &root, taken).0,
                "{own} and {taken}"
            );
        }
        assert_eq!(
            resolution(&session, &root, "rpkg"),
            (
                "<root>/store/rpkg/index.d.ts".to_string(),
                "<root>/node_modules/rpkg/index.d.ts".to_string()
            )
        );
        resolve_ahead::set_mode(None);
        drop(session);
        std::fs::remove_dir_all(&root).unwrap();
    }
}

os_child_test! {
    /// A load takes the answers of the workers, and the workers' parses of
    /// the package.json files stay on the workers. A later lookup of the
    /// program's resolver (`Program::get_package_json_info`, as auto-imports
    /// make) must find the package.json lookups of the taken answers, as Go's
    /// one cache of the load's resolutions has them. Here the disk changed
    /// after the load and the snapshot's lookup cache is empty, so only the
    /// load job's lookups (`AheadLookupLayer`) give the taken answers; a disk
    /// call would find the new disk. `broken` has a
    /// package.json but no types file, and `not-installed` has no directory;
    /// no file of the program is in them, so the loader did not look them up
    /// itself.
    fn a_later_package_json_lookup_uses_the_taken_answers() {
        let root = make_project("pjlookup");
        write(
            &root,
            "node_modules/broken/package.json",
            r#"{ "name": "broken", "version": "1.0.0", "types": "gone.d.ts" }"#,
        );
        resolve_ahead::set_mode(Some(Mode::Force));
        let session = os_session(&root);
        let uri = file_uri(&root, "src/index.ts");
        open(
            &session,
            &uri,
            &format!("{INDEX}import {{ z }} from \"broken\";\n"),
        );
        program(&session, &uri);
        edit_index(&session, &root, 2, "import { d } from \"./sub/d\";\n");
        let stats = last_stats();
        assert_eq!(stats.loader.taken, stats.keys, "{stats:?}");
        let program = program(&session, &uri);
        let broken = format!("{root}/node_modules/broken/package.json");
        let not_installed = format!("{root}/node_modules/not-installed/package.json");
        let resolver = program
            .processed_files
            .resolver
            .as_ref()
            .and_then(|resolver| resolver.as_default_resolver())
            .expect("the default resolver");
        for name in [&broken, &not_installed] {
            assert!(
                resolver.package_json_info_cache.get(name).is_none(),
                "the loader looked up {name} itself"
            );
        }
        std::fs::remove_dir_all(format!("{root}/node_modules/broken")).unwrap();
        write(
            &root,
            "node_modules/not-installed/package.json",
            r#"{ "name": "not-installed", "version": "1.0.0" }"#,
        );
        let snapshot = session.snapshot();
        let config = tspath::to_path(&format!("{root}/tsconfig.json"), &root, true);
        let project = snapshot
            .project_collection
            .configured_project(&config)
            .expect("configured project");
        let source = project
            .borrow()
            .host
            .as_ref()
            .expect("host")
            .source_fs
            .source
            .borrow()
            .clone();
        let lookups = source.fs();
        ts_goport::frontend::vfs::Fs::as_any(&*lookups)
            .and_then(|fs| fs.downcast_ref::<project::snapshotfs::CachedLayeredFileSystem>())
            .expect("the snapshot's lookup cache")
            .fs
            .clear_cache();
        let info = program
            .get_package_json_info(&broken)
            .expect("the package.json of broken");
        let (name, _) = info
            .get_contents()
            .expect("contents")
            .header_fields
            .name
            .get_value();
        assert_eq!(name, "broken");
        assert!(
            program.get_package_json_info(&not_installed).is_none(),
            "a package.json in a directory that the load found missing"
        );
        resolve_ahead::set_mode(None);
        drop(program);
        drop(project);
        drop(snapshot);
        drop(session);
        std::fs::remove_dir_all(&root).unwrap();
    }
}

/// The skip of the tests above (`resolve_ahead_off`) must not hide them on
/// Linux, whose file systems are case-sensitive.
#[cfg(target_os = "linux")]
#[test]
fn resolve_ahead_runs_on_linux() {
    assert_eq!(resolve_ahead_off(), None);
}

/// followups22 (R169 reviewer): the skip (`resolve_ahead_off`) is only the
/// production condition. With the executable on a case-sensitive file
/// system and the temp dir on a case-insensitive one, resolve ahead runs,
/// so the tests must run too. The child runs one of them with `TMPDIR` on
/// a casefold tmpfs (Linux 6.13 and later) in a new user and mount
/// namespace. The skip that also probed the temp dir skipped it there.
/// Where the namespace or the casefold directory cannot be made, this test
/// is skipped.
#[cfg(target_os = "linux")]
#[test]
fn a_case_insensitive_temp_dir_does_not_skip_the_tests() {
    const READY: &str = "casefold temp dir ready";
    const TEST: &str = "project_lsp::resolveahead_test::takes_every_answer_of_an_unchanged_project";
    let mount = std::env::temp_dir().join(format!(
        "ts_goport_resolve_ahead_casefold_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir(&mount);
    std::fs::create_dir(&mount).unwrap();
    // $1 the mount point, $2 this test binary, $3 the test, $4 READY.
    let script = r#"mount -t tmpfs -o casefold tmpfs "$1" 2>/dev/null || exit 0
mkdir "$1/t" && chattr +F "$1/t" 2>/dev/null && : > "$1/t/Probe" && test -e "$1/t/pROBE" || exit 0
echo "$4"
TMPDIR="$1/t" exec "$2" --exact "$3" --nocapture --test-threads=1"#;
    let output = std::process::Command::new("unshare")
        .args(["-rm", "sh", "-c", script, "sh"])
        .arg(&mount)
        .arg(std::env::current_exe().unwrap())
        .args([TEST, READY])
        .stdin(std::process::Stdio::null())
        .output();
    let _ = std::fs::remove_dir(&mount);
    let output = match output {
        Ok(output) => output,
        Err(err) => {
            eprintln!("skipped: unshare: {err}");
            return;
        }
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let text = format!("{stdout}{}", String::from_utf8_lossy(&output.stderr));
    if !stdout.starts_with(READY) {
        eprintln!("skipped: no casefold temp dir in a new namespace here:\n{text}");
        return;
    }
    assert!(output.status.success(), "{text}");
    assert!(text.contains("test result: ok. 1 passed"), "{text}");
    assert!(!text.contains("skipped"), "{text}");
}
