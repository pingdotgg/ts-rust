//! Port of Go `internal/project/extendedconfigcache_test.go` (`TestExtendedConfigCacheOwnership`).

use std::collections::HashMap;
use std::rc::Rc;

use ts_goport::frontend::tsoptions;
use ts_goport::frontend::tspath;
use ts_goport::frontend::vfs::Fs;
use ts_goport::gostd::{Context, GoError};
use ts_goport::lsp::lsproto;
use ts_goport::project::{self, Client, Session, SessionInit, Snapshot, WatcherID, logging};

use super::projecttestutil::{self, FileMap, files};
use super::util::*;

// Go: extendedconfigcache_test.go:19 noopClient
struct NoopClient;

impl Client for NoopClient {
    fn watch_files(
        &self,
        _: &Context,
        _: WatcherID,
        _: &[lsproto::FileSystemWatcher],
    ) -> Result<(), GoError> {
        Ok(())
    }
    fn unwatch_files(&self, _: &Context, _: WatcherID) -> Result<(), GoError> {
        Ok(())
    }
    // Go: extendedconfigcache_test.go:28 noopClient.RegisterContentMapperExtensions (tsgo#4712)
    fn register_content_mapper_extensions(&self, _: &Context, _: &[String]) -> Result<(), GoError> {
        Ok(())
    }
    fn refresh_diagnostics(&self, _: &Context) -> Result<(), GoError> {
        Ok(())
    }
    fn publish_diagnostics(
        &self,
        _: &Context,
        _: lsproto::PublishDiagnosticsParams,
    ) -> Result<(), GoError> {
        Ok(())
    }
    fn refresh_inlay_hints(&self, _: &Context) -> Result<(), GoError> {
        Ok(())
    }
    fn refresh_code_lens(&self, _: &Context) -> Result<(), GoError> {
        Ok(())
    }
    fn progress_start(&self, _: &'static ts_goport::diagnostics::Message, _: Vec<String>) {}
    fn progress_finish(&self, _: &'static ts_goport::diagnostics::Message, _: Vec<String>) {}
    fn send_telemetry(&self, _: &Context, _: lsproto::TelemetryEvent) -> Result<(), GoError> {
        Ok(())
    }
    fn is_active(&self) -> bool {
        true
    }
    // Go: extendedconfigcache_test.go:52 noopClient.GetLocale
    fn get_locale(&self) -> ts_goport::locale::Locale {
        ts_goport::locale::DEFAULT
    }
    // Go: extendedconfigcache_test.go:54 noopClient.SetLocale
    fn set_locale(&self, _: &str) {}
}

// Go: extendedconfigcache_test.go:57 setup
fn setup(files: FileMap) -> Rc<Session> {
    let (_, fs) = projecttestutil::wrapped_map_fs(files, false /*useCaseSensitiveFileNames*/);
    let logger: Rc<dyn logging::Logger> = logging::new_test_logger();
    let client: Rc<dyn Client> = Rc::new(NoopClient);
    project::new_session(&SessionInit {
        background_ctx: bg(),
        options: Rc::new(project::SessionOptions {
            watch_enabled: false,
            logging_enabled: false,
            ..projecttestutil::session_options("/")
        }),
        fs,
        client: Some(client),
        logger: Some(logger),
        npm_executor: None,
        spawner: None,
        content_mapper_logger: None,
        parse_cache: None,
        content_mapped_parse_cache: None,
    })
}

// Go: extendedconfigcache_test.go:78 openUntitled / flushCloseProject
fn flush_close_project(session: &Rc<Session>, file_uri: &str, untitled_seq: &mut i32) {
    close(session, file_uri);
    *untitled_seq += 1;
    open(session, &format!("untitled:Untitled-{untitled_seq}"), "");
}

// Go: extendedconfigcache_test.go:91 ownerCount
fn owner_count(session: &Session, path: &tspath::Path) -> usize {
    session
        .extended_config_cache
        .entries
        .borrow()
        .get(path)
        .map_or(0, |entry| entry.owners.borrow().len())
}

// Go: extendedconfigcache_test.go:99 assertNoEntry
fn assert_no_entry(session: &Session, file_name: &str) {
    let path = (session.to_path)(file_name);
    assert!(
        !session
            .extended_config_cache
            .entries
            .borrow()
            .contains_key(&path),
        "extended config cache still has {file_name}"
    );
}

// Go: extendedconfigcache_test.go:106 expectedExtendedOwnerCounts
fn expected_extended_owner_counts(
    session: &Session,
    snapshot: &Snapshot,
) -> HashMap<tspath::Path, usize> {
    let mut result = HashMap::new();
    for cfg in snapshot.config_file_registry.configs.values() {
        let cfg = cfg.borrow();
        let Some(command_line) = &cfg.command_line else {
            continue;
        };
        if command_line.config_file.is_none() {
            continue;
        }
        for file in command_line.extended_source_files() {
            *result.entry((session.to_path)(file)).or_insert(0) += 1;
        }
    }
    result
}

// Go: extendedconfigcache_test.go:120 assertExtendedOwnerCountsMatchRegistry
fn assert_extended_owner_counts_match_registry(session: &Session, snapshot: &Snapshot) {
    let expected = expected_extended_owner_counts(session, snapshot);
    for (path, want) in expected {
        let got = owner_count(session, &path);
        assert_eq!(got, want, "extended config {} owner count mismatch", path.0);
    }
}

child_test! {
    // Go: extendedconfigcache_test.go:129 TestExtendedConfigCacheOwnership/multi-extends shared ancestor counted once
    fn multi_extends_shared_ancestor_counted_once() {
        let files = files(&[
            (
                "/project/tsconfig.json",
                r#"{
				"extends": ["./tsconfig.base1.json", "./tsconfig.base2.json"]
			}"#,
            ),
            (
                "/project/tsconfig.base1.json",
                r#"{
				"extends": "./tsconfig.root.json",
				"compilerOptions": {"strict": true}
			}"#,
            ),
            (
                "/project/tsconfig.base2.json",
                r#"{
				"extends": "./tsconfig.root.json",
				"compilerOptions": {"noImplicitAny": true}
			}"#,
            ),
            (
                "/project/tsconfig.root.json",
                r#"{
				"compilerOptions": {"target": "ES2020"}
			}"#,
            ),
            ("/project/src/main.ts", "export const x = 1;"),
        ]);

        let session = setup(files);
        let mut untitled_seq = 0;
        open(&session, "file:///project/src/main.ts", "export const x = 1;");
        let snapshot = session.snapshot();

        let config = snapshot
            .config_file_registry
            .get_config(&path("/project/tsconfig.json"))
            .expect("config");
        // Shared root should only appear once in the flattened list.
        let root_count = config
            .extended_source_files()
            .iter()
            .filter(|f| **f == "/project/tsconfig.root.json")
            .count();
        assert_eq!(root_count, 1);

        // And the cache owner counts should match the registry's deduped list.
        assert_extended_owner_counts_match_registry(&session, &snapshot);

        flush_close_project(&session, "file:///project/src/main.ts", &mut untitled_seq);
        assert_no_entry(&session, "/project/tsconfig.base1.json");
        assert_no_entry(&session, "/project/tsconfig.base2.json");
        assert_no_entry(&session, "/project/tsconfig.root.json");
    }
}

// Go: extendedconfigcache_test.go:307 testParseConfigHost
struct TestParseConfigHost {
    fs: Rc<dyn Fs>,
    cwd: String,
}

impl tsoptions::ParseConfigHost for TestParseConfigHost {
    fn fs(&self) -> Rc<dyn Fs> {
        self.fs.clone()
    }
    fn get_current_directory(&self) -> String {
        self.cwd.clone()
    }
}

child_test! {
    // Go: extendedconfigcache_test.go:177 TestExtendedConfigCacheOwnership/ExtendedSourceFiles can contain same path twice (case-insensitive)
    fn extended_source_files_can_contain_same_path_twice_case_insensitive() {
        let files = files(&[
            (
                "/project/tsconfig.json",
                r#"{
				"extends": ["./Shared.json", "./shared.json"]
			}"#,
            ),
            (
                "/project/shared.json",
                r#"{
				"compilerOptions": {"strict": true}
			}"#,
            ),
        ]);

        // This test intentionally bypasses the project system's ExtendedConfigCache so we can
        // observe how ExtendedSourceFiles behaves when the same underlying file is referenced
        // with different casing on a case-insensitive FS.
        let (_, fs) = projecttestutil::wrapped_map_fs(files, false /*useCaseSensitiveFileNames*/);

        // Minimal ParseConfigHost implementation.
        let h = TestParseConfigHost {
            fs,
            cwd: "/".to_string(),
        };
        let (cmd, diags) = tsoptions::get_parsed_command_line_of_config_file(
            "/project/tsconfig.json",
            None,
            None,
            &h,
            None, /*extendedConfigCache*/
        );
        assert_eq!(diags.len(), 0);
        let cmd = cmd.expect("parsed command line");

        let extended = cmd.extended_source_files();
        assert_eq!(extended.len(), 2);
        assert_eq!(extended[0], "/project/Shared.json");
        assert_eq!(extended[1], "/project/shared.json");
    }
}

child_test! {
    // Go: extendedconfigcache_test.go:210 TestExtendedConfigCacheOwnership/project system dedupes case-only extends via cache
    fn project_system_dedupes_case_only_extends_via_cache() {
        let files = files(&[
            (
                "/project/tsconfig.json",
                r#"{
				"extends": ["./Shared.json", "./shared.json"]
			}"#,
            ),
            (
                "/project/shared.json",
                r#"{
				"compilerOptions": {"strict": true}
			}"#,
            ),
            ("/project/src/main.ts", "export const x = 1;"),
        ]);

        let session = setup(files);
        open(&session, "file:///project/src/main.ts", "export const x = 1;");
        let snapshot = session.snapshot();

        let config = snapshot
            .config_file_registry
            .get_config(&path("/project/tsconfig.json"))
            .expect("config");
        let extended = config.extended_source_files();
        assert_eq!(extended.len(), 1);
        assert_eq!((session.to_path)(&extended[0]), (session.to_path)("/project/shared.json"));
    }
}

child_test! {
    // Go: extendedconfigcache_test.go:234 TestExtendedConfigCacheOwnership/transitive extended config ownership with new project
    fn transitive_extended_config_ownership_with_new_project() {
        let files = files(&[
            (
                "/user/username/projects/shared/tsconfig.common.json",
                r#"{
					"compilerOptions": { "strict": true }
				}"#,
            ),
            (
                "/user/username/projects/shared/tsconfig.base.json",
                r#"{
					"extends": "./tsconfig.common.json",
					"compilerOptions": { "target": "ES2020" }
				}"#,
            ),
            (
                "/user/username/projects/projectA/tsconfig.json",
                r#"{
					"extends": "../shared/tsconfig.base.json"
				}"#,
            ),
            ("/user/username/projects/projectA/src/main.ts", "const a = 1;"),
            (
                "/user/username/projects/projectB/tsconfig.json",
                r#"{
					"extends": "../shared/tsconfig.base.json"
				}"#,
            ),
            ("/user/username/projects/projectB/src/main.ts", "const b = 2;"),
            ("/user/username/projects/other/src/main.ts", "const other = 3;"),
        ]);

        let session = setup(files);

        // Step 1: Open file in projectA - this parses the full extends chain
        open(&session, "file:///user/username/projects/projectA/src/main.ts", "const a = 1;");

        // Verify extended configs are in cache with correct owner counts
        let base = path("/user/username/projects/shared/tsconfig.base.json");
        let common = path("/user/username/projects/shared/tsconfig.common.json");
        assert!(
            session.extended_config_cache.entries.borrow().contains_key(&base),
            "tsconfig.base.json should be in cache"
        );
        assert!(
            session.extended_config_cache.entries.borrow().contains_key(&common),
            "tsconfig.common.json should be in cache"
        );
        assert_eq!(owner_count(&session, &base), 1);
        assert_eq!(owner_count(&session, &common), 1);

        // Step 2: Open file in projectB - this should acquire tsconfig.base.json from cache
        // (not reparse it), and should also acquire tsconfig.common.json.
        open(&session, "file:///user/username/projects/projectB/src/main.ts", "const b = 2;");

        // Step 3: Close projectA file and open an unrelated file to force projectA cleanup
        close(&session, "file:///user/username/projects/projectA/src/main.ts");
        // Opening another file triggers cleanup of closed projects
        open(&session, "file:///user/username/projects/other/src/main.ts", "const other = 3;");

        // Close the other file too so only projectB remains
        close(&session, "file:///user/username/projects/other/src/main.ts");

        // Step 4: Trigger another snapshot clone for projectB
        edit(
            &session,
            "file:///user/username/projects/projectB/src/main.ts",
            2,
            (0, 0),
            (0, 12),
            "const b = 3;",
        );
        // This call triggered the panic
        let _ = language_service(&session, "file:///user/username/projects/projectB/src/main.ts");
    }
}
