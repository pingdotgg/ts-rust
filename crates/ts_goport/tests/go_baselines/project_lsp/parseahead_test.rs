//! Parse ahead (src/frontend/compiler/files_parser.rs). Not a Go test: the
//! parse workers of the port's program loads parse queued files ahead of
//! the loader. A project that a snapshot clone makes again gets its files
//! from the parse cache, so its load starts no worker parse
//! (`CompilerHost::cached_source_file_refs`): the loader would not take
//! them, and their nodes would stay in the workers' AST arenas
//! (projsearch1b, hono's tsconfig.spec.json).
//!
//! The workers read the OS file system, so each test writes a project to a
//! temp directory and runs with no OS override (not `child_test!`).

use ts_goport::frontend::compiler::{PrefetchCounts, prefetch_counts};
use ts_goport::lsp::lsproto;

use super::resolveahead_test::{os_session, write};
use super::util::{edit, open, open_kind};

/// The `file:` URI of `name` in `root`. A Windows root (`C:/a/b`) has no
/// leading slash, so the URI needs the third one (Go lsp.go:57
/// fixWindowsURIPath strips it).
fn file_uri(root: &str, name: &str) -> String {
    let slash = if root.starts_with('/') { "" } else { "/" };
    format!("file://{slash}{root}/{name}")
}

/// A test in a child process with no OS override and two parse workers.
macro_rules! os_child_test {
    ($(#[$meta:meta])* fn $name:ident() $body:block) => {
        $(#[$meta])*
        #[test]
        fn $name() {
            let path = concat!(module_path!(), "::", stringify!($name));
            let test = path.split_once("::").map_or(path, |(_, rest)| rest);
            crate::support::child::run_test_in_child_with_env(
                test,
                &[("GOPORT_PARSE_THREADS", "2")],
                || $body,
            );
        }
    };
}

const MAIN: &str = "export const foo = 1;\n";
const HELPER: &str = "export const bar = 2;\n";
const TEST: &str = "import { foo } from './main';\nimport { bar } from './helper';\nfoo + bar;\n";

/// A solution with a build and a spec project over one `src`, as in hono.
/// The spec project has another jsx option, so its parse cache keys differ
/// from the build project's.
const FILES: &[(&str, &str)] = &[
    (
        "tsconfig.json",
        r#"{ "files": [], "references": [{ "path": "./tsconfig.build.json" }, { "path": "./tsconfig.spec.json" }] }"#,
    ),
    (
        "tsconfig.build.json",
        r#"{ "compilerOptions": { "noLib": true, "types": [] }, "include": ["src/**/*.ts"], "exclude": ["src/**/*.test.ts"] }"#,
    ),
    (
        "tsconfig.spec.json",
        r#"{ "compilerOptions": { "noLib": true, "types": [], "jsx": "react-jsx" }, "include": ["src/**/*.ts"] }"#,
    ),
    ("src/main.ts", MAIN),
    ("src/helper.ts", HELPER),
    ("src/main.test.ts", TEST),
];

/// The change of the counts from `before` to now.
fn since(before: PrefetchCounts) -> PrefetchCounts {
    let now = prefetch_counts();
    PrefetchCounts {
        pool_loads: now.pool_loads - before.pool_loads,
        cached_loads: now.cached_loads - before.cached_loads,
        reads: now.reads - before.reads,
        untaken: now.untaken - before.untaken,
        untaken_bytes: now.untaken_bytes - before.untaken_bytes,
    }
}

os_child_test! {
    fn made_again_project_parses_nothing_ahead() {
        let root = std::env::temp_dir().join(format!(
            "ts_goport_parse_ahead_{}_made_again",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        for (name, text) in FILES {
            write(&root.to_string_lossy(), name, text);
        }
        let root = crate::support::eval_symlinks(&root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        let session = os_session(&root);
        let main = file_uri(&root, "src/main.ts");
        let helper = file_uri(&root, "src/helper.ts");
        let test = file_uri(&root, "src/main.test.ts");
        let start = prefetch_counts();

        // The search makes and loads both projects of the level. The spec
        // project's files are not in the cache with its jsx option, so its
        // first load parses ahead too, and takes the parses. Then the clone
        // deletes it (main.ts is in the build project), and its files stay
        // in the cache, as in Go.
        let before = prefetch_counts();
        open(&session, &main, MAIN);
        let first = since(before);
        assert_eq!(
            (first.pool_loads, first.cached_loads),
            (2, 0),
            "build and spec parse ahead: {first:?}"
        );

        // The search makes the spec project again. The cache has every file
        // of it with its options, so its load starts no worker.
        let before = prefetch_counts();
        open(&session, &helper, HELPER);
        let second = since(before);
        assert_eq!(
            (second.pool_loads, second.cached_loads, second.reads),
            (0, 1, 0),
            "the spec project made again parses nothing ahead: {second:?}"
        );

        // An open file whose text is not the cached text is not a cached
        // file, so the spec load parses ahead again.
        edit(&session, &helper, 2, (0, 0), (0, 0), "// edited\n");
        let before = prefetch_counts();
        open(&session, &test, TEST);
        let third = since(before);
        assert_eq!(
            (third.pool_loads, third.cached_loads),
            (1, 0),
            "an edited open file is parsed ahead: {third:?}"
        );

        let all = since(start);
        assert_eq!(
            (all.untaken, all.untaken_bytes),
            (0, 0),
            "every worker parse was taken: {all:?}"
        );
        drop(session);
        std::fs::remove_dir_all(&root).unwrap();
    }
}

/// Writes `files` to a new temp directory named for `name`, opens each of
/// `opens` (file, text, language) in turn, and returns the change of the
/// counts at each open and over all of them.
fn counts_of_opens(
    name: &str,
    files: &[(&str, &str)],
    opens: &[(&str, &str, lsproto::LanguageKind)],
) -> (Vec<PrefetchCounts>, PrefetchCounts) {
    let dir = std::env::temp_dir().join(format!(
        "ts_goport_parse_ahead_{}_{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    for (file, text) in files {
        write(&dir.to_string_lossy(), file, text);
    }
    let root = crate::support::eval_symlinks(&dir)
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/");
    let session = os_session(&root);
    let start = prefetch_counts();
    let each = opens
        .iter()
        .map(|(file, text, kind)| {
            let before = prefetch_counts();
            open_kind(&session, &file_uri(&root, file), text, kind.clone());
            since(before)
        })
        .collect();
    let all = since(start);
    drop(session);
    std::fs::remove_dir_all(&dir).unwrap();
    (each, all)
}

const TS: lsproto::LanguageKind = lsproto::LanguageKind::TYPE_SCRIPT;

os_child_test! {
    // The build and spec projects differ only in `moduleDetection`, so the
    // cache keys of the build project's files have another `force` (Go
    // project/parsecache.go:22). The spec load cannot take them, so its
    // workers parse ahead, and the loader takes their parses.
    fn a_cached_file_with_another_force_is_parsed_ahead() {
        let files = [
            FILES[0],
            (
                "tsconfig.build.json",
                r#"{ "compilerOptions": { "noLib": true, "types": [], "moduleDetection": "force" }, "include": ["src/**/*.ts"] }"#,
            ),
            (
                "tsconfig.spec.json",
                r#"{ "compilerOptions": { "noLib": true, "types": [], "moduleDetection": "legacy" }, "include": ["src/**/*.ts"] }"#,
            ),
            ("src/main.ts", MAIN),
            ("src/helper.ts", HELPER),
        ];
        let (each, all) = counts_of_opens("force", &files, &[("src/main.ts", MAIN, TS)]);
        assert_eq!(
            (each[0].pool_loads, each[0].cached_loads),
            (2, 0),
            "build and spec parse ahead: {each:?}"
        );
        assert_eq!(all.untaken, 0, "every worker parse was taken: {all:?}");
    }
}

os_child_test! {
    // Go reads the package.json `type` only for node16 to nodenext
    // resolution or a `/node_modules/` path (fileloader.go:398 to :401).
    // With bundler resolution a `"type": "module"` scope sets no `force`,
    // so the build project's keys (`moduleDetection` force) do not fit the
    // spec project's load, and its workers parse ahead. A guess that let
    // the scope set `force` here made it a cached load whose loader parsed
    // every file itself (followups23 skeptic).
    fn a_module_scope_sets_no_force_for_bundler_resolution() {
        let files = [
            ("package.json", r#"{ "name": "esm", "type": "module" }"#),
            FILES[0],
            (
                "tsconfig.build.json",
                r#"{ "compilerOptions": { "noLib": true, "types": [], "module": "esnext", "moduleResolution": "bundler", "moduleDetection": "force" }, "include": ["src/**/*.ts"] }"#,
            ),
            (
                "tsconfig.spec.json",
                r#"{ "compilerOptions": { "noLib": true, "types": [], "module": "esnext", "moduleResolution": "bundler", "moduleDetection": "auto" }, "include": ["src/**/*.ts"] }"#,
            ),
            ("src/main.ts", MAIN),
            ("src/helper.ts", HELPER),
        ];
        let (each, all) = counts_of_opens("bundler", &files, &[("src/main.ts", MAIN, TS)]);
        assert_eq!(
            (each[0].pool_loads, each[0].cached_loads),
            (2, 0),
            "build and spec parse ahead: {each:?}"
        );
        assert_eq!(all.untaken, 0, "every worker parse was taken: {all:?}");
    }
}

os_child_test! {
    // In a `"type": "module"` scope with `module` nodenext and
    // `moduleDetection` auto, every key has `force` set (Go
    // ast/parseoptions.go:46 isFileForcedToBeModuleByFormat), which the host
    // cannot know before the load finds the scope. The spec project made
    // again still takes every file from the cache.
    fn a_made_again_project_in_an_esm_scope_parses_nothing_ahead() {
        let files = [
            ("package.json", r#"{ "name": "esm", "type": "module" }"#),
            FILES[0],
            (
                "tsconfig.build.json",
                r#"{ "compilerOptions": { "noLib": true, "types": [], "module": "nodenext", "moduleDetection": "auto" }, "include": ["src/**/*.ts"] }"#,
            ),
            (
                "tsconfig.spec.json",
                r#"{ "compilerOptions": { "noLib": true, "types": [], "module": "nodenext", "moduleDetection": "auto", "jsx": "react-jsx" }, "include": ["src/**/*.ts"] }"#,
            ),
            ("src/main.ts", MAIN),
            ("src/helper.ts", HELPER),
        ];
        let (each, all) = counts_of_opens(
            "esm",
            &files,
            &[("src/main.ts", MAIN, TS), ("src/helper.ts", HELPER, TS)],
        );
        assert_eq!(
            (each[1].pool_loads, each[1].cached_loads, each[1].reads),
            (0, 1, 0),
            "the spec project made again parses nothing ahead: {each:?}"
        );
        assert_eq!(all.untaken, 0, "every worker parse was taken: {all:?}");
    }
}

os_child_test! {
    // An open file's key has the kind of its language id (Go
    // project/overlayfs.go:178 `Overlay.Kind`), here TSX for a `.ts` name.
    // The spec project made again takes it from the cache.
    fn a_made_again_project_takes_an_open_file_of_another_kind_from_the_cache() {
        let (each, all) = counts_of_opens(
            "kind",
            FILES,
            &[
                ("src/helper.ts", HELPER, lsproto::LanguageKind::TYPE_SCRIPT_REACT),
                ("src/main.ts", MAIN, TS),
            ],
        );
        assert_eq!(
            (each[1].pool_loads, each[1].cached_loads, each[1].reads),
            (0, 1, 0),
            "the spec project made again parses nothing ahead: {each:?}"
        );
        assert_eq!(all.untaken, 0, "every worker parse was taken: {all:?}");
    }
}

os_child_test! {
    // The app project uses the sources of its lib reference, which it parses
    // with lib's options (Go compiler/fileloader.go:418,
    // projectreferencefilemapper.go:80 getCompilerOptionsForFile), not its
    // own jsx. The lib project loads first, so the app load takes every
    // file from the cache.
    fn a_project_with_references_takes_their_sources_from_the_cache() {
        let files = [
            (
                "tsconfig.json",
                r#"{ "files": [], "references": [{ "path": "./tsconfig.lib.json" }, { "path": "./tsconfig.app.json" }] }"#,
            ),
            (
                "tsconfig.lib.json",
                r#"{ "compilerOptions": { "composite": true, "noLib": true, "types": [] }, "include": ["src/**/*.ts"] }"#,
            ),
            (
                "tsconfig.app.json",
                r#"{ "compilerOptions": { "noLib": true, "types": [], "jsx": "react-jsx" }, "include": ["src/**/*.ts"], "references": [{ "path": "./tsconfig.lib.json" }] }"#,
            ),
            ("src/main.ts", MAIN),
            ("src/helper.ts", HELPER),
        ];
        let (each, all) = counts_of_opens("refs", &files, &[("src/main.ts", MAIN, TS)]);
        assert_eq!(
            (each[0].pool_loads, each[0].cached_loads),
            (1, 1),
            "lib parses ahead, app takes the cache: {each:?}"
        );
        assert_eq!(all.untaken, 0, "every worker parse was taken: {all:?}");
    }
}
