//! Port of Go `internal/project/refcountcache_test.go` (`TestRefCountingCaches`).
//!
//! PORT: Go `file.Hash` is `xxh3_128(file.Text())` for a program file (see
//! `project::parsecache::HashedSourceFile`); the Rust `ParsedSourceFile`
//! has no hash field, so `key` computes it.

use std::rc::Rc;

use rustc_hash::FxHashSet;
use ts_goport::ast::{ContentMapperSourceFileInfo, source_file_info, source_file_is_bound};
use ts_goport::contentmapper::SourceFiles;
use ts_goport::flags::ScriptKind;
use ts_goport::frontend::compiler::DuplicateSourceFile;
use ts_goport::frontend::parser::{self, ParsedSourceFile, SourceFileParseOptions};
use ts_goport::frontend::tspath::Path;
use ts_goport::frontend::vfs::Fs;
use ts_goport::gostd::GoError;
use ts_goport::lsp::lsproto;
use ts_goport::options::Tristate;
use ts_goport::project::{
    APICreateProgramRequest, APISnapshotRequest, ConfiguredProjectID, ContentMappedParseCache,
    ContentMappedParseCacheKey, FileChangeSummary, FileHandle, HashedSourceFile, ParseCache,
    ParseCacheKey, ProgramUpdateKind, RefCountCacheEntry, RefCountCacheOptions, ResourceRequest,
    Session, SnapshotChange, UpdateReason, acquire_bound,
    content_mapped_parse_cache_key_for_duplicate, content_mapped_parse_cache_key_for_file,
    new_cached_file_handle, new_content_mapped_parse_cache, new_overlay, new_parse_cache,
    new_parse_cache_key, new_ref_count_cache, set_source_file_hash,
};

use super::projecttestutil::{FileMap, files};
use super::util::*;

// Go: refcountcache_test.go:21 TestContentMappedParseCacheBundleLifetime (tsgo#4712)
// PORT: Go uses empty `&ast.SourceFile{}` values and does not parse. Here two
// parsed empty files. They need absolute, normalized names, because the parser
// keeps the Go NewSourceFile check on the file name. The test only checks the
// identity of the files, so the names do not change what it tests.
#[test]
fn test_content_mapped_parse_cache_bundle_lifetime() {
    let cache = new_content_mapped_parse_cache(RefCountCacheOptions::default());
    let key = ContentMappedParseCacheKey::new(
        &SourceFileParseOptions {
            file_name: "/component.vue".to_string(),
            path: Path("/component.vue".to_string()),
            ..Default::default()
        },
        0,
    );
    let canonical = Rc::new(parser::parse_source_file(
        &SourceFileParseOptions {
            file_name: "/canonical.ts".to_string(),
            path: Path("/canonical.ts".to_string()),
            ..Default::default()
        },
        "",
        ScriptKind::TS,
    ));
    let supplemental = Rc::new(parser::parse_source_file(
        &SourceFileParseOptions {
            file_name: "/supplemental.ts".to_string(),
            path: Path("/supplemental.ts".to_string()),
            ..Default::default()
        },
        "",
        ScriptKind::TS,
    ));
    let produced = SourceFiles {
        canonical: Some(canonical.clone()),
        supplemental: vec![supplemental.clone()],
    };

    // The cache owns the complete transform result as one value, so reuse preserves every file's identity.
    let acquired = cache
        .acquire_or_error(key.clone(), || Ok::<_, GoError>(produced))
        .expect("assert.NilError");
    assert!(Rc::ptr_eq(
        acquired.canonical.as_ref().expect("canonical"),
        &canonical
    ));
    assert!(Rc::ptr_eq(&acquired.supplemental[0], &supplemental));
    let reused = cache
        .acquire_or_error(key.clone(), || -> Result<SourceFiles, GoError> {
            panic!("cached bundle should be reused")
        })
        .expect("assert.NilError");
    assert!(Rc::ptr_eq(
        reused.canonical.as_ref().expect("canonical"),
        &canonical
    ));
    assert!(Rc::ptr_eq(&reused.supplemental[0], &supplemental));

    // Canonical and supplemental files share the bundle's refcount and disappear after its final release.
    ContentMappedParseCache::deref(&cache, &key);
    assert!(cache.has(&key));
    ContentMappedParseCache::deref(&cache, &key);
    assert!(!cache.has(&key));
}

// Go: refcountcache_test.go:48 TestContentMappedParseCacheKeyReconstruction (tsgo#4712)
// PORT: Go `file.Hash = hash` is `set_source_file_hash`. A Rust
// `DuplicateSourceFile` has the file text, not Go's `Hash`; its hash is the
// hash of that text, so the duplicate shares the file's text.
#[test]
fn test_content_mapped_parse_cache_key_reconstruction() {
    let acquire_options = SourceFileParseOptions {
        file_name: "/component.box".to_string(),
        path: Path("/component.box".to_string()),
        ..Default::default()
    };
    let mut mapped_options = acquire_options.clone();
    mapped_options.external_module_indicator_options.force = true;
    let hash = xxhash_rust::xxh3::xxh3_128(b"cache key");
    let file = parser::parse_source_file(&mapped_options, "export {};", ScriptKind::TS);
    set_source_file_hash(&file, hash);
    file.set_content_mapper_info(ContentMapperSourceFileInfo {
        content_mapper: "mapper".to_string(),
        parse_options: acquire_options.clone(),
        ..Default::default()
    });
    let expected = ContentMappedParseCacheKey::new(&acquire_options, hash);
    assert_eq!(content_mapped_parse_cache_key_for_file(&file), expected);

    let duplicate = DuplicateSourceFile {
        parse_options: mapped_options,
        content_mapper_parse_options: acquire_options,
        text: file.text.clone(),
        hash: file.hash.get(),
        script_kind: ScriptKind::UNKNOWN,
        content_mapper: "mapper".to_string(),
        is_content_mapper_failure_stub: false,
    };
    assert_eq!(
        content_mapped_parse_cache_key_for_duplicate(&duplicate),
        expected
    );
}

// Go: refcountcache_test.go:73 TestParseCacheBindsBeforePublishing (ts#63952)
// PORT: Go `cache.Acquire` is `acquire_bound`. The port's `acquire` does not
// bind, because a program load binds its files later in file order
// (`program::bind_all`). `acquire_bound` binds before it returns the
// file, as Go binds before the entry is published. This is a partial port:
// it covers the API lease path only. Program loads and the auto-import
// registry use `acquire` and bind later.
#[test]
fn test_parse_cache_binds_before_publishing() {
    const FILE_NAME: &str = "/index.js";
    let file_handle: Rc<dyn FileHandle> = Rc::new(new_overlay(
        FILE_NAME,
        "module.exports = 0;".to_string(),
        1,
        ScriptKind::JS,
    ));
    let parse_options = SourceFileParseOptions {
        file_name: FILE_NAME.to_string(),
        path: Path(FILE_NAME.to_string()),
        ..Default::default()
    };
    let key = new_parse_cache_key(&parse_options, file_handle.hash(), file_handle.kind());
    let cache = new_parse_cache(RefCountCacheOptions::default());

    let file = acquire_bound(&cache, key.clone(), file_handle, "/").file;

    assert!(source_file_is_bound(file.root));
    assert!(
        source_file_info(file.root)
            .common_js_module_indicator
            .is_some()
    );
    ParseCache::deref(&cache, &key);
}

// Go: refcountcache_test.go:92 TestParseCacheAcquireExistingUsesFullKey (ts#64518)
// PORT: Go `cache.Acquire` binds; the port's `acquire` does not (see
// `test_parse_cache_binds_before_publishing`). The test checks only the
// identity of the cached file. Go runs the mismatch subtests in parallel.
#[test]
fn test_parse_cache_acquire_existing_uses_full_key() {
    const FILE_NAME: &str = "/index.ts";
    let file_handle = new_cached_file_handle(FILE_NAME, "export {};");
    let key = new_parse_cache_key(
        &SourceFileParseOptions {
            file_name: FILE_NAME.to_string(),
            path: Path(FILE_NAME.to_string()),
            ..Default::default()
        },
        file_handle.hash(),
        ScriptKind::TS,
    );
    let cache = new_parse_cache(RefCountCacheOptions::default());
    let file = cache.acquire(key.clone(), file_handle);

    let acquired = cache.acquire_existing(&key).expect("assert.Assert(t, ok)");
    assert!(Rc::ptr_eq(&acquired.file, &file.file));
    ParseCache::deref(&cache, &key);

    let mismatches = [
        (
            "file name",
            ParseCacheKey {
                file_name: "/INDEX.ts".to_string(),
                ..key.clone()
            },
        ),
        (
            "path",
            ParseCacheKey {
                path: Path("/INDEX.ts".to_string()),
                ..key.clone()
            },
        ),
        (
            "hash",
            ParseCacheKey {
                hash: xxhash_rust::xxh3::xxh3_128(b"different"),
                ..key.clone()
            },
        ),
        (
            "script kind",
            ParseCacheKey {
                script_kind: ScriptKind::TSX,
                ..key.clone()
            },
        ),
        (
            "jsx parse option",
            ParseCacheKey {
                jsx: true,
                ..key.clone()
            },
        ),
        (
            "force parse option",
            ParseCacheKey {
                force: true,
                ..key.clone()
            },
        ),
    ];
    for (name, mismatch) in &mismatches {
        assert!(cache.acquire_existing(mismatch).is_none(), "{name}");
        assert!(!cache.has(mismatch), "{name}");
    }

    ParseCache::deref(&cache, &key);
    assert!(!cache.has(&key));
}

// Go: refcountcache_test.go:166 TestRefCountCacheAcquireExisting (ts#64518)
#[test]
fn test_ref_count_cache_acquire_existing() {
    let parse_count = Rc::new(std::cell::Cell::new(0));
    let cache = {
        let parse_count = parse_count.clone();
        new_ref_count_cache(
            RefCountCacheOptions::default(),
            move |_key: &String, value: i32| {
                parse_count.set(parse_count.get() + 1);
                value
            },
        )
    };

    assert_eq!(cache.acquire_existing(&"missing".to_string()), None);
    assert_eq!(parse_count.get(), 0);

    assert_eq!(cache.acquire("key".to_string(), 1), 1);
    assert_eq!(cache.acquire_existing(&"key".to_string()), Some(1));
    assert_eq!(parse_count.get(), 1);

    cache.deref(&"key".to_string());
    assert!(cache.has(&"key".to_string()));
    cache.deref(&"key".to_string());
    assert!(!cache.has(&"key".to_string()));

    assert_eq!(cache.acquire_existing(&"key".to_string()), None);
    assert_eq!(parse_count.get(), 1);
}

// Go: refcountcache_test.go:197 TestRefCountCacheAcquireExistingRacesFinalRelease (ts#64518)
// PORT: one thread (see `project/refcountcache.rs`), so the goroutine race
// is the two orders it can take, each checked once: AcquireExisting before
// the final Deref, and after it.
#[test]
fn test_ref_count_cache_acquire_existing_races_final_release() {
    for acquire_first in [true, false] {
        let cache = new_ref_count_cache(
            RefCountCacheOptions::default(),
            |_key: &String, value: Rc<i32>| value,
        );
        let value = Rc::new(1);
        cache.acquire("key".to_string(), value);

        let mut acquired = false;
        if acquire_first {
            acquired = cache.acquire_existing(&"key".to_string()).is_some();
        }
        cache.deref(&"key".to_string());
        if !acquire_first {
            acquired = cache.acquire_existing(&"key".to_string()).is_some();
        }
        assert_eq!(acquired, acquire_first);
        if acquired {
            cache.deref(&"key".to_string());
        }
        assert!(!cache.has(&"key".to_string()));
    }
}

// Go: refcountcache_test.go:23 setup
fn setup(files: FileMap) -> Rc<Session> {
    bare_session(files)
}

/// Go `NewParseCacheKey(f.ParseOptions(), f.Hash, f.ScriptKind)`.
fn key(f: &ParsedSourceFile) -> ParseCacheKey {
    new_parse_cache_key(
        f.parse_options(),
        xxhash_rust::xxh3::xxh3_128(f.text.as_bytes()),
        f.script_kind,
    )
}

/// Go `NewParseCacheKey(dup.ParseOptions, dup.Hash, dup.ScriptKind)`.
fn dup_key(dup: &DuplicateSourceFile) -> ParseCacheKey {
    new_parse_cache_key(
        &dup.parse_options,
        xxhash_rust::xxh3::xxh3_128(dup.text.as_bytes()),
        dup.script_kind,
    )
}

/// Go `session.parseCache.entries.Load(key)`.
fn load(
    session: &Session,
    key: &ParseCacheKey,
) -> Option<Rc<RefCountCacheEntry<HashedSourceFile>>> {
    session.parse_cache.entries.borrow().get(key).cloned()
}

fn ref_count(entry: &RefCountCacheEntry<HashedSourceFile>) -> i32 {
    entry.ref_count.get()
}

const MAIN: &str = "/user/username/projects/myproject/src/main.ts";
const UTILS: &str = "/user/username/projects/myproject/src/utils.ts";
const MAIN_URI: &str = "file:///user/username/projects/myproject/src/main.ts";
const UTILS_URI: &str = "file:///user/username/projects/myproject/src/utils.ts";

// Go: refcountcache_test.go:42 files
fn parse_cache_files() -> FileMap {
    files(&[(MAIN, "const x = 1;"), (UTILS, "export function util() {}")])
}

fn inferred_program(session: &Session) -> Rc<ts_goport::frontend::compiler::NewProgram> {
    session
        .snapshot()
        .project_collection
        .inferred_project()
        .expect("inferred project")
        .borrow()
        .program
        .clone()
        .expect("inferred program")
}

child_test! {
    // Go: refcountcache_test.go:48 TestRefCountingCaches/parseCache/reuse unchanged file
    fn parse_cache_reuse_unchanged_file() {
        let session = setup(parse_cache_files());
        open(&session, MAIN_URI, "const x = 1;");
        open(&session, UTILS_URI, "export function util() {}");
        let program = inferred_program(&session);
        let main = program.get_source_file(MAIN).unwrap();
        let utils = program.get_source_file(UTILS).unwrap();
        let main_entry = load(&session, &key(&main)).expect("main entry");
        let utils_entry = load(&session, &key(&utils)).expect("utils entry");
        assert_eq!(ref_count(&main_entry), 1);
        assert_eq!(ref_count(&utils_entry), 1);

        edit(&session, MAIN_URI, 2, (0, 0), (0, 12), "const x = 2;");
        let p = program_of(&session, MAIN_URI);
        session.wait_for_background_tasks();
        let new_main = p.get_source_file(MAIN).unwrap();
        let new_main_entry = load(&session, &key(&new_main)).expect("new main entry");
        assert!(!Rc::ptr_eq(&new_main, &main));
        assert!(!Rc::ptr_eq(&new_main_entry, &main_entry));
        assert!(Rc::ptr_eq(&p.get_source_file(UTILS).unwrap(), &utils));
        // Old snapshot is deref'd immediately when replaced by UpdateSnapshot,
        // so old mainEntry is already disposed and utils refCount is already 1.
        assert_eq!(ref_count(&main_entry), 0);
        assert_eq!(ref_count(&new_main_entry), 1);
        assert_eq!(ref_count(&utils_entry), 1);
    }
}

child_test! {
    // Go: refcountcache_test.go:89 TestRefCountingCaches/parseCache/release file on close
    fn parse_cache_release_file_on_close() {
        let session = setup(parse_cache_files());
        open(&session, MAIN_URI, "const x = 1;");
        open(&session, UTILS_URI, "export function util() {}");
        let program = inferred_program(&session);
        let main = program.get_source_file(MAIN).unwrap();
        let utils = program.get_source_file(UTILS).unwrap();
        let main_entry = load(&session, &key(&main)).expect("main entry");
        let utils_entry = load(&session, &key(&utils)).expect("utils entry");
        assert_eq!(ref_count(&main_entry), 1);
        assert_eq!(ref_count(&utils_entry), 1);

        close(&session, MAIN_URI);
        let _ = language_service(&session, UTILS_URI);
        session.wait_for_background_tasks();
        assert_eq!(ref_count(&utils_entry), 1);
        assert_eq!(ref_count(&main_entry), 0);
        assert!(load(&session, &key(&main)).is_none());
    }
}

child_test! {
    // Go: refcountcache_test.go:114 TestRefCountingCaches/parseCache/unchanged program does not over-ref
    fn parse_cache_unchanged_program_does_not_over_ref() {
        let session = setup(parse_cache_files());
        open(&session, MAIN_URI, "const x = 1;");
        open(&session, UTILS_URI, "export function util() {}");

        // Get first snapshot and capture the program/entries
        let program1 = inferred_program(&session);
        let main = program1.get_source_file(MAIN).unwrap();
        let main_entry = load(&session, &key(&main)).expect("main entry");
        assert_eq!(ref_count(&main_entry), 1, "initial refCount should be 1");

        // Change utils.ts to trigger a new snapshot, but main.ts stays the same
        // so main's source file should be reused.
        edit(&session, UTILS_URI, 2, (0, 0), (0, 25), "export function util2() {}");

        // Get second snapshot - main.ts should be reused (program is new but shares source files)
        let program2 = program_of(&session, MAIN_URI);
        session.wait_for_background_tasks();
        let main2 = program2.get_source_file(MAIN).unwrap();
        assert!(Rc::ptr_eq(&main, &main2), "main.ts source file should be reused");

        // main.ts refCount should be 1: the old snapshot was immediately deref'd
        // when replaced, so only the new snapshot holds a ref.
        let main_entry = load(&session, &key(&main)).expect("main entry");
        assert_eq!(ref_count(&main_entry), 1, "refCount should be 1 (only new snapshot)");

        // Close files to trigger cleanup
        close(&session, MAIN_URI);
        close(&session, UTILS_URI);
        open(&session, "untitled:Untitled-1", "");
        session.wait_for_background_tasks();

        // Entry should now be gone (refCount 0, deleted)
        let entry = load(&session, &key(&main));
        assert!(
            entry.is_none(),
            "entry should be deleted after program is disposed (refCount {:?})",
            entry.map(|e| ref_count(&e))
        );
    }
}

child_test! {
    // Go: refcountcache_test.go:172 TestRefCountingCaches/parseCache/fallback rebuild does not double-ref changed file
    fn parse_cache_fallback_rebuild_does_not_double_ref_changed_file() {
        let session = setup(files(&[(MAIN, "const x = 1;"), (UTILS, "export const util = 1;")]));
        open(&session, MAIN_URI, "const x = 1;");

        let _ = language_service(&session, MAIN_URI);

        session.did_change_file(
            &bg(),
            &uri(MAIN_URI),
            2,
            &[lsproto::TextDocumentContentChangePartialOrWholeDocument {
                partial: None,
                whole_document: Some(lsproto::TextDocumentContentChangeWholeDocument {
                    text: "import { util } from \"./utils\";\nconst x = util;".to_string(),
                }),
            }],
        );

        let p_after = program_of(&session, MAIN_URI);
        session.wait_for_background_tasks();

        let project = session
            .snapshot()
            .project_collection
            .inferred_project()
            .expect("inferred project");
        assert_eq!(project.borrow().program_update_kind, ProgramUpdateKind::NEW_FILES);

        let main = p_after.get_source_file(MAIN).unwrap();
        let main_key = key(&main);
        let main_entry = load(&session, &main_key).expect("main entry");
        assert_eq!(ref_count(&main_entry), 1);

        close(&session, MAIN_URI);
        open(&session, "untitled:Untitled-1", "");
        session.wait_for_background_tasks();

        assert!(load(&session, &main_key).is_none());
    }
}

fn project_entries(session: &Session) -> usize {
    session
        .parse_cache
        .entries
        .borrow()
        .keys()
        .filter(|key| {
            key.file_name
                .starts_with("/user/username/projects/myproject/src/")
        })
        .count()
}

child_test! {
    // Go: refcountcache_test.go:216 TestRefCountingCaches/parseCache/case-only duplicate loads are released on dispose
    fn parse_cache_case_only_duplicate_loads_are_released_on_dispose() {
        let main_text = "import { util as a } from \"./utils\";\nimport { util as b } from \"./UTILS\";\nconst x = a + b;";
        let session = setup(files(&[(MAIN, main_text), (UTILS, "export const util = 1;")]));
        open(&session, MAIN_URI, main_text);

        let p = program_of(&session, MAIN_URI);

        assert_eq!(project_entries(&session), 3);

        assert!(p.get_source_file(UTILS).is_some());

        close(&session, MAIN_URI);
        open(&session, "untitled:Untitled-1", "");
        session.wait_for_background_tasks();

        assert_eq!(project_entries(&session), 0);
    }
}

const ENTRY: &str = "/user/username/projects/myproject/src/entry.ts";
const ENTRY_URI: &str = "file:///user/username/projects/myproject/src/entry.ts";

/// Go `session.DidChangeFile(ctx, entryURI, version, ...)` with one whole-document change.
fn change_entry(session: &Rc<Session>, version: i32, text: &str) {
    session.did_change_file(
        &bg(),
        &uri(ENTRY_URI),
        version,
        &[lsproto::TextDocumentContentChangePartialOrWholeDocument {
            partial: None,
            whole_document: Some(lsproto::TextDocumentContentChangeWholeDocument {
                text: text.to_string(),
            }),
        }],
    );
}

child_test! {
    // Go: refcountcache_test.go:256 TestRefCountingCaches/parseCache/case-only duplicate imported from multiple files is refcounted once
    fn parse_cache_case_only_duplicate_imported_from_multiple_files_is_refcounted_once() {
        // A file reached through a case-only-different file name from more than one
        // import site is parsed and acquired in the parse cache exactly once (same-casing
        // loads dedupe), but it must also be recorded as a duplicate exactly once.
        // Recording it once per import site would release it from the parse cache more
        // times than it was acquired, deleting the live entry out from under a program
        // that still references it and panicking the next time it is ref'd during a clone.
        let entry_text =
            "import { dep } from './sub/dep';\nimport './a';\nimport './b';\nexport const e = dep;";
        let session = setup(files(&[
            // entry.ts imports the canonical casing first, then pulls in a.ts and b.ts,
            // which both import the same file through an upper-cased name.
            (ENTRY, entry_text),
            (
                "/user/username/projects/myproject/src/a.ts",
                "import { dep } from './sub/DEP';\nexport const a = dep;",
            ),
            (
                "/user/username/projects/myproject/src/b.ts",
                "import { dep } from './sub/DEP';\nexport const b = dep;",
            ),
            ("/user/username/projects/myproject/src/sub/dep.ts", "export const dep = 1;"),
            ("/user/username/projects/myproject/src/c.ts", "export const c = 1;"),
        ]));
        open(&session, ENTRY_URI, entry_text);

        // The upper-cased name is recorded as a duplicate, and it should appear exactly once.
        let program = Rc::clone(&language_service(&session, ENTRY_URI).program);
        let dup_keys: Vec<ParseCacheKey> = program
            .duplicate_source_files()
            .iter()
            .filter(|dup| dup.parse_options.file_name.ends_with("/sub/DEP.ts"))
            .map(dup_key)
            .collect();
        assert_eq!(dup_keys.len(), 1, "case-only duplicate should be recorded exactly once");
        let dup_entry =
            load(&session, &dup_keys[0]).expect("duplicate entry should exist in the parse cache");
        assert_eq!(ref_count(&dup_entry), 1);

        // Force a full program rebuild (adding an import changes the file's module
        // structure). The old snapshot is disposed, releasing each of its source and
        // duplicate files exactly once. If the duplicate were recorded twice, the
        // shared cache entry would be released to zero and deleted here even though
        // the new program still references it.
        change_entry(
            &session,
            2,
            "import { dep } from \"./sub/dep\";\nimport \"./a\";\nimport \"./b\";\nimport \"./c\";\nexport const e = dep;",
        );
        let rebuilt_program = Rc::clone(&language_service(&session, ENTRY_URI).program);
        session.wait_for_background_tasks();

        // Every parse-cache key referenced by the live program must still exist.
        let assert_key_alive = |key: ParseCacheKey| {
            assert!(
                load(&session, &key).is_some(),
                "live program references a deleted parse-cache entry: {}",
                key.file_name
            );
        };
        for file in rebuilt_program.source_files() {
            assert_key_alive(key(file));
        }
        for dup in rebuilt_program.duplicate_source_files() {
            assert_key_alive(dup_key(dup));
        }

        // An incremental (clone) update re-references the duplicate files; this must
        // not panic with "cache entry not found".
        change_entry(
            &session,
            3,
            "import { dep } from './sub/dep';\nimport './a';\nimport './b';\nimport './c';\nexport const e = dep + 0;",
        );
        let _ = language_service(&session, ENTRY_URI);
        session.wait_for_background_tasks();

        // Closing the project releases everything cleanly.
        // (The configured project is not disposed until another file in another project is opened,
        // so we open an untitled file to trigger that.)
        close(&session, ENTRY_URI);
        open(&session, "untitled:Untitled-1", "");
        session.wait_for_background_tasks();

        assert_eq!(project_entries(&session), 0);
    }
}

/// The reference counts of the parse cache entries whose file name ends
/// with `suffix`.
fn ref_counts_of(session: &Session, suffix: &str) -> Vec<i32> {
    session
        .parse_cache
        .entries
        .borrow()
        .iter()
        .filter(|(key, _)| key.file_name.ends_with(suffix))
        .map(|(_, entry)| ref_count(entry))
        .collect()
}

child_test! {
    // PORT: no Go counterpart. An auto-import registry clone keeps one
    // reference to each file that it parsed
    // (`AutoImportRegistryCloneHost::dispose`), so a later clone gets the
    // same parse from the cache. Before, each clone released the entry, and
    // a cancelled idle warm parsed, bound and kept the same node_modules
    // files again on every edit.
    fn parse_cache_keeps_auto_import_files_after_clone() {
        let session = setup(files(&[
            ("/home/projects/app/tsconfig.json", "{}"),
            ("/home/projects/app/index.ts", ""),
            ("/home/projects/node_modules/foo/package.json", r#"{ "types": "index.d.ts" }"#),
            ("/home/projects/node_modules/foo/index.d.ts", "export const foo = 0;"),
        ]));
        let index_uri = "file:///home/projects/app/index.ts";
        open(&session, index_uri, "");
        assert!(ref_counts_of(&session, "/node_modules/foo/index.d.ts").is_empty());

        session
            .get_current_language_service_with_auto_imports(&bg(), &uri(index_uri))
            .unwrap_or_else(|err| panic!("{}", err.error()));

        // The program does not include the file, so the only reference is
        // the one the session keeps.
        assert_eq!(ref_counts_of(&session, "/node_modules/foo/index.d.ts"), vec![1]);
        session.close();
    }
}

// Go: refcountcache_test.go:355 files (extendedConfigCache)
fn extended_config_files() -> FileMap {
    files(&[
        (
            "/user/username/projects/myproject/tsconfig.json",
            r#"{
				"extends": "./tsconfig.base.json"
			}"#,
        ),
        (
            "/user/username/projects/myproject/tsconfig.base.json",
            r#"{
				"compilerOptions": {}
			}"#,
        ),
        (MAIN, "const x = 1;"),
    ])
}

fn extended_owners(session: &Session, p: &str) -> Option<usize> {
    session
        .extended_config_cache
        .entries
        .borrow()
        .get(&path(p))
        .map(|entry| entry.owners.borrow().len())
}

child_test! {
    // Go: refcountcache_test.go:365 TestRefCountingCaches/extendedConfigCache/release extended configs with project close
    fn extended_config_cache_release_extended_configs_with_project_close() {
        let session = setup(extended_config_files());
        open(&session, MAIN_URI, "const x = 1;");
        let config = session
            .snapshot()
            .config_file_registry
            .get_config(&path("/user/username/projects/myproject/tsconfig.json"))
            .expect("config");
        assert_eq!(
            config.extended_source_files()[0],
            "/user/username/projects/myproject/tsconfig.base.json"
        );
        assert_eq!(
            extended_owners(&session, "/user/username/projects/myproject/tsconfig.base.json"),
            Some(1)
        );

        close(&session, MAIN_URI);
        open(&session, "untitled:Untitled-1", "");
        session.wait_for_background_tasks();
        assert_eq!(
            extended_owners(&session, "/user/username/projects/myproject/tsconfig.base.json"),
            None
        );
    }
}

child_test! {
    // Go: refcountcache_test.go:383 TestRefCountingCaches/extendedConfigCache/release cache entries for unretained clone
    fn extended_config_cache_release_cache_entries_for_unretained_clone() {
        let session = setup(extended_config_files());
        let u = uri(MAIN_URI);
        let base_snapshot = session.snapshot();
        let extended_config_path = "/user/username/projects/myproject/tsconfig.base.json";
        let clone = base_snapshot.clone_(
            &bg(),
            SnapshotChange {
                reason: UpdateReason::REQUESTED_LANGUAGE_SERVICE_PROJECT_NOT_LOADED,
                resource_request: ResourceRequest {
                    documents: vec![u.clone()],
                    ..Default::default()
                },
                ..Default::default()
            },
            &base_snapshot.overlays(),
            None,
            None,
        );

        let project = clone.get_default_project(&u).expect("default project");
        assert_eq!(project.borrow().program_last_update, clone.id);

        let main = project.borrow().program.as_ref().expect("program").get_source_file(MAIN).unwrap();
        let main_key = key(&main);
        let main_entry = load(&session, &main_key).expect("main entry");
        assert_eq!(ref_count(&main_entry), 1);

        assert_eq!(extended_owners(&session, extended_config_path), Some(1));

        clone.deref();

        assert!(load(&session, &main_key).is_none());

        assert_eq!(extended_owners(&session, extended_config_path), None);
    }
}

child_test! {
    // Go: refcountcache_test.go:494 TestRefCountingCaches/extendedConfigCache/createProgram retains and reloads extended configs from referenced projects (ts#63950, ts#64319)
    fn extended_config_cache_create_program_retains_and_reloads_extended_configs_from_referenced_projects() {
        const APP_CONFIG_PATH: &str = "/user/username/projects/app/tsconfig.json";
        const APP_FILE_PATH: &str = "/user/username/projects/app/index.ts";
        const LIB_CONFIG_PATH: &str = "/user/username/projects/lib/tsconfig.json";
        const LIB_BASE_CONFIG_PATH: &str = "/user/username/projects/lib/tsconfig.base.json";
        const LIB_FILE_PATH: &str = "/user/username/projects/lib/index.ts";
        let session = setup(files(&[
            (
                APP_CONFIG_PATH,
                r#"{"compilerOptions":{"noLib":true},"files":["index.ts"],"references":[{"path":"../lib"}]}"#,
            ),
            (APP_FILE_PATH, "export const app = 1;"),
            (LIB_CONFIG_PATH, r#"{"extends":"./tsconfig.base.json","files":["index.ts"]}"#),
            (LIB_BASE_CONFIG_PATH, r#"{"compilerOptions":{"composite":true,"noLib":true}}"#),
            (LIB_FILE_PATH, "export const lib = 1;"),
        ]));
        let ctx = bg();

        let base_snapshot = session
            .api_update(
                &ctx,
                FileChangeSummary::default(),
                Some(&APISnapshotRequest {
                    open_projects: Some(FxHashSet::from_iter([APP_CONFIG_PATH.to_string()])),
                    ..Default::default()
                }),
            )
            .unwrap_or_else(|err| panic!("APIUpdate: {}", err.error()));
        let app_project = base_snapshot
            .project_collection
            .get_project(&ConfiguredProjectID(base_snapshot.to_path(APP_CONFIG_PATH)).as_id())
            .expect("app project");

        let create_request = {
            let app_project = app_project.borrow();
            let command_line = app_project.command_line.as_ref().expect("command line");
            APISnapshotRequest {
                create_programs: vec![APICreateProgramRequest {
                    root_file_names: command_line.file_names().to_vec(),
                    compiler_options: command_line.compiler_options().clone(),
                    project_references: command_line.project_references().to_vec(),
                    config_file_parsing_diagnostics: command_line.errors.clone(),
                    module_resolver_factory: None,
                    module_resolver_id: 0,
                }],
                ..Default::default()
            }
        };
        let (program_snapshot, err) = session.clone_snapshot(
            &ctx,
            &base_snapshot,
            FileChangeSummary::default(),
            Some(&create_request),
        );
        assert!(err.is_none(), "CloneSnapshot: {:?}", err.map(|err| err.error()));
        let program_project = program_snapshot.created_programs()[0].clone();
        assert!(!Rc::ptr_eq(
            program_project.borrow().program.as_ref().expect("program"),
            app_project.borrow().program.as_ref().expect("app program"),
        ));

        let extended_config_entry = session
            .extended_config_cache
            .entries
            .borrow()
            .get(&path(LIB_BASE_CONFIG_PATH))
            .cloned()
            .expect("extended config entry");
        let (owned_by_base_snapshot, owned_by_program_snapshot, owner_count) = {
            let owners = extended_config_entry.owners.borrow();
            (
                owners.contains(&base_snapshot.id),
                owners.contains(&program_snapshot.id),
                owners.len(),
            )
        };
        assert!(owned_by_base_snapshot);
        assert!(owned_by_program_snapshot);
        assert_eq!(owner_count, 2);

        Fs::write_file(
            &*session.fs,
            LIB_BASE_CONFIG_PATH,
            r#"{"compilerOptions":{"composite":true,"noLib":true,"strict":true}}"#,
        )
        .unwrap();
        let mut file_changes = FileChangeSummary::default();
        file_changes
            .changed
            .insert(uri(&format!("file://{LIB_BASE_CONFIG_PATH}")));
        let program_project_id = program_project.borrow().id();
        let update_request = APISnapshotRequest {
            ensure_programs: Some(FxHashSet::from_iter([program_project_id.clone()])),
            ..Default::default()
        };
        let (updated_program_snapshot, err) = session.clone_snapshot(
            &ctx,
            &program_snapshot,
            file_changes,
            Some(&update_request),
        );
        assert!(err.is_none(), "CloneSnapshot: {:?}", err.map(|err| err.error()));
        let updated_program_project = updated_program_snapshot
            .project_collection
            .get_project(&program_project_id)
            .expect("updated program project");
        let updated_program = updated_program_project.borrow().program.clone().expect("program");
        assert!(!Rc::ptr_eq(
            &updated_program,
            program_project.borrow().program.as_ref().expect("program"),
        ));
        let updated_references = updated_program.get_resolved_project_references();
        assert_eq!(updated_references.len(), 1);
        assert_eq!(
            updated_references[0].as_ref().expect("reference").compiler_options().strict,
            Tristate::True
        );

        // Go: defer updatedProgramSnapshot.Deref(); defer programSnapshot.Deref();
        // defer baseSnapshot.Deref(); defer session.Close()
        updated_program_snapshot.deref();
        program_snapshot.deref();
        base_snapshot.deref();
        session.close();
    }
}
