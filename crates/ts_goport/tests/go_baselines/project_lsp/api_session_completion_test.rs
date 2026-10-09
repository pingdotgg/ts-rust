//! Port of Go `internal/api/session_completion_test.go`.
//! Bump C: the tests use the N session API (ts#64163, ts#64204). The N tests
//! `TestCompletionRetriesWithAutoImports` (ts#64133) and
//! `TestCompletionWithSymbolsAndExistingImportDoesNotDeadlock` (ts#64178)
//! are new at N.
//!
//! Bump D: ts#64554 adds `CreateSnapshotParams.UserPreferences` and
//! `PrepareAutoImports`, and 6 tests (Go N' fed0bf24149f).
//!
//! PORT: the tests are in `project_lsp` because they use `projecttestutil`
//! and `child_test!`. Go `bundled.Embedded` is always true in the port, so
//! the skip is dropped.

use ts_goport::api::{
    self, CreateSnapshotParams, DocumentIdentifier, GetCompletionsAtPositionParams,
    GetCurrentLanguageServerSnapshotParams, GetDefaultProjectForFileParams, GetTypeOfSymbolParams,
    SnapshotID, SnapshotRequestChangesParams, UpdateSnapshotParams,
};
use ts_goport::gostd::GoError;
use ts_goport::ls::autoimport;
use ts_goport::ls::lsutil;
use ts_goport::lsp::lsproto;
use ts_goport::options::Tristate;

use super::api_util::{error_contains, no_lib, program_params, snapshot_of};
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

// Go: session_completion_test.go:28 TestCompletionSymbolTypeIsResolvable
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

// Go: session_completion_test.go:101 TestCompletionOnInferredProject
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

// Go: session_completion_test.go:143 TestCompletionRetriesWithAutoImports
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

/// The project of the ts#64554 tests: `src/index.ts` holds "someV".
const P_FILE: &str = "/home/projects/p/src/index.ts";
const P_CONTENT: &str = "someV";

fn some_value_project() -> std::rc::Rc<ts_goport::project::Session> {
    projecttestutil::setup(files(&[
        (
            "/home/projects/p/tsconfig.json",
            r#"{ "compilerOptions": { "module": "esnext", "target": "esnext" } }"#,
        ),
        (
            "/home/projects/p/src/export.ts",
            "export const someValue = 1;",
        ),
        (P_FILE, P_CONTENT),
    ]))
    .0
}

/// Go `lsutil.UserPreferences{IncludeCompletionsForModuleExports: value}`.
fn module_exports_preferences(value: Tristate) -> lsutil::UserPreferences {
    lsutil::UserPreferences {
        include_completions_for_module_exports: value,
        ..Default::default()
    }
}

/// The handleCreateSnapshot, handleGetDefaultProjectForFile and
/// handleGetCompletionsAtPosition (IncludeSymbol) steps of the first 3
/// ts#64554 tests.
fn complete_some_v(
    params: CreateSnapshotParams,
) -> Result<Option<api::CompletionInfoResponse>, GoError> {
    let project_session = some_value_project();
    let session = api::new_lsp_session(project_session.clone(), None);
    let ctx = bg();
    let snapshot_resp = nil_error(session.handle_create_snapshot(&ctx, &params));
    let proj = nil_error(session.handle_get_default_project_for_file(
        &ctx,
        &GetDefaultProjectForFileParams {
            snapshot: snapshot_resp.snapshot,
            file: doc(P_FILE),
        },
    ))
    .expect("file should resolve to a default project");
    let result = session.handle_get_completions_at_position(
        &ctx,
        &GetCompletionsAtPositionParams {
            snapshot: snapshot_resp.snapshot,
            project: proj.id.clone(),
            file: doc(P_FILE),
            position: P_CONTENT.len() as u32,
            include_symbol: true,
            ..Default::default()
        },
    );
    session.close();
    project_session.close();
    result
}

fn open_files(names: &[&str]) -> SnapshotRequestChangesParams {
    SnapshotRequestChangesParams {
        open_files: Some(names.iter().map(|name| doc(name)).collect()),
        ..Default::default()
    }
}

// Go: session_completion_test.go:192 TestCompletionWithSymbolsRequiresPreparedAutoImports (ts#64554)
child_test! {
    fn completion_with_symbols_requires_prepared_auto_imports() {
        error_contains(
            complete_some_v(CreateSnapshotParams {
                snapshot_request_changes_params: open_files(&[P_FILE]),
                user_preferences: Some(module_exports_preferences(Tristate::True)),
                ..Default::default()
            }),
            "snapshot is not prepared for auto-imports",
        );
    }
}

// Go: session_completion_test.go:233 TestCompletionWithSymbolsUsesPreparedSnapshot (ts#64554)
child_test! {
    fn completion_with_symbols_uses_prepared_snapshot() {
        let completions = nil_error(complete_some_v(CreateSnapshotParams {
            snapshot_request_changes_params: open_files(&[P_FILE]),
            prepare_auto_imports: Some(doc(P_FILE)),
            user_preferences: Some(module_exports_preferences(Tristate::True)),
            ..Default::default()
        }))
        .expect("expected a completion list");
        assert!(
            completions.entries.iter().any(|entry| entry.name == "someValue"),
            "expected auto-import completion for someValue"
        );
    }
}

// Go: session_completion_test.go:282 TestCompletionUsesSnapshotPreferences (ts#64554)
child_test! {
    fn completion_uses_snapshot_preferences() {
        let completions = nil_error(complete_some_v(CreateSnapshotParams {
            snapshot_request_changes_params: open_files(&[P_FILE]),
            user_preferences: Some(module_exports_preferences(Tristate::False)),
            ..Default::default()
        }))
        .expect("expected a completion list");
        for entry in &completions.entries {
            assert!(
                entry.name != "someValue",
                "snapshot preferences should disable auto-import completions"
            );
        }
    }
}

/// Go `snapshot.AutoImportRegistry().IsPreparedForImportingFile(fileName, proj.ID(), snapshot.UserPreferences())`.
fn is_prepared(snapshot: &ts_goport::project::Snapshot, file_uri: &str, file_name: &str) -> bool {
    let proj = snapshot
        .get_default_project(&uri(file_uri))
        .expect("file should resolve to a default project");
    let registry = snapshot.auto_import_registry();
    autoimport::Registry::is_prepared_for_importing_file(
        registry.as_deref(),
        file_name,
        &autoimport::ProjectID(proj.borrow().id().0.clone()),
        &snapshot.user_preferences(),
    )
}

// Go: session_completion_test.go:327 TestSnapshotCreatesProgramsAndPreparesAutoImportsInOneClone (ts#64554)
// PORT: the "create" and "update" subtests are one test each. Go
// `testutil.RecoverAndFail` is the child runner's panic report.
fn snapshot_creates_programs_and_prepares_auto_imports_in_one_clone(update: bool) {
    const FILE_NAME: &str = "/home/projects/p/index.ts";
    let (project_session, _) = projecttestutil::setup(files(&[
        ("/home/projects/p/tsconfig.json", "{}"),
        (FILE_NAME, "someV"),
        ("/home/projects/p/export.ts", "export const someValue = 1;"),
        ("/home/projects/synthetic.ts", "export const x = 1;"),
    ]));
    let session = api::new_lsp_session(project_session.clone(), None);
    let ctx = bg();
    let params = CreateSnapshotParams {
        snapshot_request_changes_params: SnapshotRequestChangesParams {
            open_projects: vec![doc("/home/projects/p/tsconfig.json")],
            create_programs: Some(vec![Some(program_params(
                &["/home/projects/synthetic.ts"],
                no_lib(),
            ))]),
            ..Default::default()
        },
        prepare_auto_imports: Some(doc(FILE_NAME)),
        ..Default::default()
    };
    let (response, expected_id) = if update {
        let base =
            nil_error(session.handle_create_snapshot(&ctx, &CreateSnapshotParams::default()));
        let response = session.handle_update_snapshot(
            &ctx,
            &UpdateSnapshotParams {
                snapshot: base.snapshot,
                changes: Some(params),
            },
        );
        (response, SnapshotID(base.snapshot.0 + 1))
    } else {
        (session.handle_create_snapshot(&ctx, &params), SnapshotID(1))
    };
    let response = nil_error(response);
    assert_eq!(response.snapshot, expected_id);
    assert_eq!(
        response
            .operation
            .as_ref()
            .unwrap()
            .created_programs
            .as_ref()
            .unwrap()
            .len(),
        1
    );
    let snapshot = snapshot_of(&session, response.snapshot);
    assert!(is_prepared(
        &snapshot,
        "file:///home/projects/p/index.ts",
        FILE_NAME
    ));
    session.close();
    project_session.close();
}

child_test! {
    fn snapshot_creates_programs_and_prepares_auto_imports_in_one_clone_create() {
        snapshot_creates_programs_and_prepares_auto_imports_in_one_clone(false);
    }
}

child_test! {
    fn snapshot_creates_programs_and_prepares_auto_imports_in_one_clone_update() {
        snapshot_creates_programs_and_prepares_auto_imports_in_one_clone(true);
    }
}

/// Go `projectSession.DidOpenFile(ctx, "file://"+name, 1, text, lsproto.LanguageKindTypeScript)`.
fn did_open(project_session: &std::rc::Rc<ts_goport::project::Session>, name: &str, text: &str) {
    project_session.did_open_file(
        &bg(),
        &uri(&format!("file://{name}")),
        1,
        text,
        &lsproto::LanguageKind::TYPE_SCRIPT,
    );
}

/// The `my-pkg` dependency project of the last two ts#64554 tests in `dir`.
fn my_pkg_project(dir: &str, file_text: &str) -> std::rc::Rc<ts_goport::project::Session> {
    let tsconfig = format!("{dir}/tsconfig.json");
    let index = format!("{dir}/index.ts");
    let package_json = format!("{dir}/package.json");
    let pkg_json = format!("{dir}/node_modules/my-pkg/package.json");
    let pkg_index = format!("{dir}/node_modules/my-pkg/index.d.ts");
    projecttestutil::setup(files(&[
        (tsconfig.as_str(), "{}"),
        (index.as_str(), file_text),
        (
            package_json.as_str(),
            r#"{"dependencies":{"my-pkg":"1.0.0"}}"#,
        ),
        (
            pkg_json.as_str(),
            r#"{"name":"my-pkg","version":"1.0.0","types":"index.d.ts"}"#,
        ),
        (
            pkg_index.as_str(),
            "export declare const packageValue: number;",
        ),
    ]))
    .0
}

// Go: session_completion_test.go:375 TestPreparedIndependentSnapshotPreservesLSPOverlays (ts#64554)
child_test! {
    fn prepared_independent_snapshot_preserves_lsp_overlays() {
        const FILE_NAME: &str = "/home/projects/p/index.ts";
        let project_session = my_pkg_project("/home/projects/p", "diskOnly");
        did_open(&project_session, FILE_NAME, "packageV");
        let session = api::new_lsp_session(project_session.clone(), None);
        let ctx = bg();
        let base = nil_error(session.handle_get_current_language_server_snapshot(
            &ctx,
            &GetCurrentLanguageServerSnapshotParams::default(),
        ));
        let prepared = nil_error(session.handle_update_snapshot(
            &ctx,
            &UpdateSnapshotParams {
                snapshot: base.snapshot,
                changes: Some(CreateSnapshotParams {
                    prepare_auto_imports: Some(doc(FILE_NAME)),
                    ..Default::default()
                }),
            },
        ));
        let snapshot = snapshot_of(&session, prepared.snapshot);
        let (content, ok) = snapshot.read_file(FILE_NAME);
        assert!(ok);
        assert_eq!(content, "packageV");
        let proj = snapshot
            .get_default_project(&uri("file:///home/projects/p/index.ts"))
            .expect("file should resolve to a default project");
        let program = proj.borrow().get_program().expect("project has a program");
        assert_eq!(text(&program, FILE_NAME), "packageV");
        let project_id = proj.borrow().id();
        let completions = nil_error(session.handle_get_completions_at_position(
            &ctx,
            &GetCompletionsAtPositionParams {
                snapshot: prepared.snapshot,
                project: project_id,
                file: doc(FILE_NAME),
                position: 8,
                include_symbol: true,
                ..Default::default()
            },
        ))
        .expect("expected a completion list");
        assert!(
            completions.entries.iter().any(|entry| entry.name == "packageValue"),
            "expected dependency auto-import completion from an LSP-derived snapshot"
        );
        session.close();
        project_session.close();
    }
}

// Go: session_completion_test.go:420 TestFreshSnapshotIncludesDependencyAutoImports (ts#64554)
// PORT: the "retry" and "prepared" subtests are one test each.
fn fresh_snapshot_includes_dependency_auto_imports(prepare: bool) {
    const FILE_NAME: &str = "/home/projects/MixedCase/index.ts";
    let project_session = my_pkg_project("/home/projects/MixedCase", "packageV");
    did_open(&project_session, FILE_NAME, "editorOnly");
    let session = api::new_lsp_session(project_session.clone(), None);
    let ctx = bg();
    let params = CreateSnapshotParams {
        snapshot_request_changes_params: SnapshotRequestChangesParams {
            open_projects: vec![doc("/home/projects/MixedCase/tsconfig.json")],
            ..Default::default()
        },
        prepare_auto_imports: prepare.then(|| doc(FILE_NAME)),
        ..Default::default()
    };
    let response = nil_error(session.handle_create_snapshot(&ctx, &params));
    let snapshot = snapshot_of(&session, response.snapshot);
    let (content, ok) = snapshot.read_file(FILE_NAME);
    assert!(ok);
    assert_eq!(content, "packageV");
    let proj = snapshot
        .get_default_project(&uri("file:///home/projects/MixedCase/index.ts"))
        .expect("file should resolve to a default project");
    let project_id = proj.borrow().id();
    let completions = nil_error(session.handle_get_completions_at_position(
        &ctx,
        &GetCompletionsAtPositionParams {
            snapshot: response.snapshot,
            project: project_id,
            file: doc(FILE_NAME),
            position: 8,
            include_symbol: prepare,
            ..Default::default()
        },
    ))
    .expect("expected a completion list");
    assert!(
        completions
            .entries
            .iter()
            .any(|entry| entry.name == "packageValue"),
        "expected dependency auto-import completion from a fresh API snapshot"
    );
    session.close();
    project_session.close();
}

child_test! {
    fn fresh_snapshot_includes_dependency_auto_imports_retry() {
        fresh_snapshot_includes_dependency_auto_imports(false);
    }
}

child_test! {
    fn fresh_snapshot_includes_dependency_auto_imports_prepared() {
        fresh_snapshot_includes_dependency_auto_imports(true);
    }
}

// Go: session_completion_test.go:469 TestCompletionWithSymbolsAndExistingImportDoesNotDeadlock
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
        // ts#64554: a completion with symbols needs a prepared snapshot.
        let snapshot_resp = nil_error(session.handle_create_snapshot(
            &ctx,
            &CreateSnapshotParams {
                snapshot_request_changes_params: SnapshotRequestChangesParams {
                    open_files: Some(vec![doc(FILE_NAME)]),
                    ..Default::default()
                },
                prepare_auto_imports: Some(doc(FILE_NAME)),
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
