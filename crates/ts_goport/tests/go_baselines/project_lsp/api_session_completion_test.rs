//! Port of Go `internal/api/session_completion_test.go`.
//! Bump C: the tests use the N session API (ts#64163, ts#64204). The N tests
//! `TestCompletionRetriesWithAutoImports` (ts#64133) and
//! `TestCompletionWithSymbolsAndExistingImportDoesNotDeadlock` (ts#64178)
//! are new at N.
//!
//! PORT: the tests are in `project_lsp` because they use `projecttestutil`
//! and `child_test!`. Go `bundled.Embedded` is always true in the port, so
//! the skip is dropped.

use ts_goport::api::{
    self, CreateSnapshotParams, DocumentIdentifier, GetCompletionsAtPositionParams,
    GetDefaultProjectForFileParams, GetTypeOfSymbolParams, SnapshotRequestChangesParams,
};
use ts_goport::gostd::GoError;
use ts_goport::ls::lsutil;
use ts_goport::options::Tristate;

use super::projecttestutil::{self, files};
use super::util::*;

/// Go `assert.NilError(t, err)` on a result.
fn nil_error<T>(result: Result<T, GoError>) -> T {
    result.unwrap_or_else(|err| panic!("unexpected error: {}", err.error()))
}

/// Go `DocumentIdentifier{FileName: name}`.
fn doc(name: &str) -> DocumentIdentifier {
    DocumentIdentifier {
        file_name: name.to_string(),
        ..Default::default()
    }
}

// Go: session_completion_test.go:26 TestCompletionSymbolTypeIsResolvable
// TestCompletionSymbolTypeIsResolvable reproduces a crash where requesting the
// type of a completion-provided symbol panicked with a nil pointer dereference.
//
// Completion ran on an ephemeral query checker (default lifetime), so members of
// a generic type such as `string[]` (= Array<string>) were returned as
// *instantiated* symbols whose per-checker instantiation links live only on that
// query checker. GetTypeOfSymbol runs on the persistent API checker — a
// different instance — where those links are absent, so getTypeOfInstantiatedSymbol
// dereferenced a nil target and brought down the connection.
//
// The fix pins symbol-producing completion to the API checker, so the returned
// handles resolve on the same checker the client re-queries.
child_test! {
    fn completion_symbol_type_is_resolvable() {
        const FILE_NAME: &str = "/home/projects/p/src/index.ts";
        // The caret sits right after `people.`, requesting members of `string[]`.
        const CONTENT: &str = "declare const people: string[];\npeople.";

        let (project_session, _) = projecttestutil::setup(files(&[
            (
                "/home/projects/p/tsconfig.json",
                r#"{ "compilerOptions": { "strict": true } }"#,
            ),
            (FILE_NAME, CONTENT),
        ]));
        // ts#64163: NewLSPSession.
        let session = api::new_lsp_session(project_session.clone(), None);

        let ctx = bg();

        // ts#64204: createSnapshot replaces updateSnapshot.
        let snapshot_resp = nil_error(session.handle_create_snapshot(
            &ctx,
            &CreateSnapshotParams {
                snapshot_request_changes_params: SnapshotRequestChangesParams {
                    open_files: Some(vec![doc(FILE_NAME)]),
                    ..Default::default()
                },
                ..Default::default()
            },
        ));

        let proj = nil_error(session.handle_get_default_project_for_file(
            &ctx,
            &GetDefaultProjectForFileParams {
                snapshot: snapshot_resp.snapshot,
                file: doc(FILE_NAME),
            },
        ))
        .expect("file should resolve to a default project");

        // content is pure ASCII, so the UTF-16 caret offset equals the byte length.
        let completions = nil_error(session.handle_get_completions_at_position(
            &ctx,
            &GetCompletionsAtPositionParams {
                snapshot: snapshot_resp.snapshot,
                project: proj.id.clone(),
                file: doc(FILE_NAME),
                position: CONTENT.len() as u32,
                include_symbol: true,
                ..Default::default()
            },
        ))
        .expect("expected a completion list for array members");

        // Resolving the type of every completion symbol must not panic, and known
        // members like `push` must produce a concrete type.
        let mut saw_symbol = false;
        let mut saw_push = false;
        for entry in &completions.entries {
            let Some(symbol) = &entry.symbol else {
                continue;
            };
            saw_symbol = true;
            let type_resp = nil_error(session.handle_get_type_of_symbol(
                &ctx,
                &GetTypeOfSymbolParams {
                    snapshot: snapshot_resp.snapshot,
                    project: proj.id.clone(),
                    symbol: symbol.reference.clone(),
                },
            ));
            assert!(
                type_resp.is_some(),
                "type of completion symbol {:?} should resolve",
                entry.name
            );
            if entry.name == "push" {
                saw_push = true;
            }
        }
        assert!(saw_symbol, "completion entries should include resolvable symbols");
        assert!(saw_push, "array member completions should include `push`");
        session.close();
        project_session.close();
    }
}

// Go: session_completion_test.go:99 TestCompletionOnInferredProject
// TestCompletionOnInferredProject reproduces a crash where requesting completions
// for a loose file — one not part of any tsconfig.json, so it resolves to an
// inferred project — panicked with "ConfigFilePath called on non-configured
// project".
//
// setupLanguageService called Project.ConfigFilePath(), which is only valid for
// configured projects and panics for inferred ones. The fix uses Project.ID(),
// which returns the project's path for both configured and inferred projects without panicking.
child_test! {
    fn completion_on_inferred_project() {
        // No tsconfig.json anywhere, so this file belongs to an inferred project.
        const FILE_NAME: &str = "/home/projects/p/src/index.ts";
        const CONTENT: &str = "declare const people: string[];\npeople.";

        let (project_session, _) = projecttestutil::setup(files(&[(FILE_NAME, CONTENT)]));
        // ts#64163: NewLSPSession.
        let session = api::new_lsp_session(project_session.clone(), None);

        let ctx = bg();

        // ts#64204: createSnapshot replaces updateSnapshot.
        let snapshot_resp = nil_error(session.handle_create_snapshot(
            &ctx,
            &CreateSnapshotParams {
                snapshot_request_changes_params: SnapshotRequestChangesParams {
                    open_files: Some(vec![doc(FILE_NAME)]),
                    ..Default::default()
                },
                ..Default::default()
            },
        ));

        let proj = nil_error(session.handle_get_default_project_for_file(
            &ctx,
            &GetDefaultProjectForFileParams {
                snapshot: snapshot_resp.snapshot,
                file: doc(FILE_NAME),
            },
        ))
        .expect("file should resolve to an inferred default project");

        // This request previously panicked in setupLanguageService.
        // content is pure ASCII, so the UTF-16 caret offset equals the byte length.
        let completions = nil_error(session.handle_get_completions_at_position(
            &ctx,
            &GetCompletionsAtPositionParams {
                snapshot: snapshot_resp.snapshot,
                project: proj.id.clone(),
                file: doc(FILE_NAME),
                position: CONTENT.len() as u32,
                ..Default::default()
            },
        ));
        assert!(
            completions.is_some(),
            "expected a completion list for array members"
        );
        session.close();
        project_session.close();
    }
}

/// Go `lsutil.UserPreferences{IncludeCompletionsForModuleExports: core.TSTrue, IncludeCompletionsForImportStatements: core.TSTrue}`.
fn auto_import_preferences() -> lsutil::UserPreferences {
    lsutil::UserPreferences {
        include_completions_for_module_exports: Tristate::True,
        include_completions_for_import_statements: Tristate::True,
        ..Default::default()
    }
}

// Go: session_completion_test.go:141 TestCompletionRetriesWithAutoImports
child_test! {
    fn completion_retries_with_auto_imports() {
        const FILE_NAME: &str = "/home/projects/p/src/index.ts";
        const CONTENT: &str = "someV";
        let (project_session, _) = projecttestutil::setup(files(&[
            (
                "/home/projects/p/tsconfig.json",
                r#"{ "compilerOptions": { "module": "esnext", "target": "esnext" } }"#,
            ),
            ("/home/projects/p/src/export.ts", "export const someValue = 1;"),
            (FILE_NAME, CONTENT),
        ]));
        project_session.configure(auto_import_preferences());

        let session = api::new_lsp_session(project_session.clone(), None);

        let ctx = bg();
        let snapshot_resp = nil_error(session.handle_create_snapshot(
            &ctx,
            &CreateSnapshotParams {
                snapshot_request_changes_params: SnapshotRequestChangesParams {
                    open_files: Some(vec![doc(FILE_NAME)]),
                    ..Default::default()
                },
                ..Default::default()
            },
        ));
        let proj = nil_error(session.handle_get_default_project_for_file(
            &ctx,
            &GetDefaultProjectForFileParams {
                snapshot: snapshot_resp.snapshot,
                file: doc(FILE_NAME),
            },
        ))
        .expect("file should resolve to a default project");

        let completions = nil_error(session.handle_get_completions_at_position(
            &ctx,
            &GetCompletionsAtPositionParams {
                snapshot: snapshot_resp.snapshot,
                project: proj.id.clone(),
                file: doc(FILE_NAME),
                position: CONTENT.len() as u32,
                ..Default::default()
            },
        ))
        .expect("expected a completion list");
        assert!(
            completions.entries.iter().any(|entry| entry.name == "someValue"),
            "expected auto-import completion for someValue"
        );
        session.close();
        project_session.close();
    }
}

// Go: session_completion_test.go:190 TestCompletionWithSymbolsAndExistingImportDoesNotDeadlock
// PORT: the port is one thread, so the request runs on the test thread
// instead of a goroutine with a 10 second timeout. A deadlock (a checker
// taken twice) is a panic or a hang here, and the child runner's timeout
// ends a hang.
child_test! {
    fn completion_with_symbols_and_existing_import_does_not_deadlock() {
        const FILE_NAME: &str = "/home/projects/p/src/index.ts";
        const CONTENT: &str = "import { otherValue } from \"./export\";\nsomeV";
        let (project_session, _) = projecttestutil::setup(files(&[
            (
                "/home/projects/p/tsconfig.json",
                r#"{ "compilerOptions": { "module": "esnext", "target": "esnext" } }"#,
            ),
            (
                "/home/projects/p/src/export.ts",
                "export const otherValue = 0; export const someValue = 1;",
            ),
            (FILE_NAME, CONTENT),
        ]));
        project_session.configure(auto_import_preferences());

        let session = api::new_lsp_session(project_session.clone(), None);

        let ctx = bg();
        let snapshot_resp = nil_error(session.handle_create_snapshot(
            &ctx,
            &CreateSnapshotParams {
                snapshot_request_changes_params: SnapshotRequestChangesParams {
                    open_files: Some(vec![doc(FILE_NAME)]),
                    ..Default::default()
                },
                ..Default::default()
            },
        ));
        let proj = nil_error(session.handle_get_default_project_for_file(
            &ctx,
            &GetDefaultProjectForFileParams {
                snapshot: snapshot_resp.snapshot,
                file: doc(FILE_NAME),
            },
        ))
        .expect("file should resolve to a default project");

        // IncludeSymbol pins completion to the single persistent API checker. When
        // ranking the auto-import completion, the existing import makes the view
        // consult that checker. This used to try to acquire the same checker again
        // and deadlock.
        let completions = nil_error(session.handle_get_completions_at_position(
            &ctx,
            &GetCompletionsAtPositionParams {
                snapshot: snapshot_resp.snapshot,
                project: proj.id.clone(),
                file: doc(FILE_NAME),
                position: CONTENT.len() as u32,
                include_symbol: true,
                ..Default::default()
            },
        ))
        .expect("expected a completion list");
        assert!(
            completions.entries.iter().any(|entry| entry.name == "someValue"),
            "expected auto-import completion for someValue"
        );
        session.close();
        project_session.close();
    }
}
