//! Port of Go `internal/api/session_apistate_test.go` at microsoft/TypeScript
//! 673a5f17d713. ts#64204 (898322c5e4) replaced `updateSnapshot` and
//! rewrote the Go file: the O tests (`TestSessionTracksAndReleasesAPIRefs`
//! and `TestUpdateSnapshotResponseSkipsUnloadedAncestorProject`) are gone
//! upstream, and these 15 tests replace them.
//!
//! PORT: the tests are in `project_lsp` because they use `projecttestutil`
//! and `child_test!`. Go `bundled.Embedded` is always true in the port, so
//! the skip is dropped. Go `session.openProjects.Len()` and
//! `session.openFiles.Len()` are the lengths of the `open_projects` and
//! `open_files` sets. A Go `defer ...Close()` is a call at the end of the
//! test, in the Go defer order.

use std::rc::Rc;

use ts_goport::api::{
    self, CreateSnapshotProgramParams, CreateSnapshotResponse, DocumentIdentifier,
    GetCurrentLanguageServerSnapshotParams, LanguageServerSnapshotChanges,
    ReconfigureSnapshotProgramParams, SnapshotRequestChangesParams,
};
use ts_goport::gostd::GoError;
use ts_goport::lsp::lsproto;
use ts_goport::options::{CompilerOptions, Tristate};
use ts_goport::project;

use super::projecttestutil::{self, TypingsInstallerOptions, files};
use super::util::*;

// Go: session_apistate_test.go:16 configuredProjectID
fn configured_project_id(p: &str) -> project::ID {
    project::ConfiguredProjectID(path(p)).as_id()
}

// Go: session_apistate_test.go:20 inferredProjectID
fn inferred_project_id() -> project::ID {
    project::ID("/dev/null/inferred".to_string())
}

// Go: session_apistate_test.go:24 syntheticProjectID
fn synthetic_project_id(id: i32) -> project::SyntheticProjectID {
    project::new_synthetic_project_id(id)
}

/// Go `assert.NilError(t, err)` on a result.
fn nil_error<T>(result: Result<T, GoError>) -> T {
    result.unwrap_or_else(|err| panic!("unexpected error: {}", err.error()))
}

/// Go `assert.ErrorContains(t, err, want)`.
fn error_contains<T>(result: Result<T, GoError>, want: &str) {
    match result {
        Ok(_) => panic!("expected an error containing {want:?}, got nil"),
        Err(err) => assert!(
            err.error().contains(want),
            "expected an error containing {want:?}, got {:?}",
            err.error()
        ),
    }
}

/// Go `DocumentIdentifier{FileName: name}`.
fn doc(name: &str) -> DocumentIdentifier {
    DocumentIdentifier {
        file_name: name.to_string(),
        ..Default::default()
    }
}

/// Go `DocumentIdentifier{FileName: name}.ToURI(projectSession.GetCurrentDirectory())`.
fn doc_uri(project_session: &Rc<project::Session>, name: &str) -> lsproto::DocumentUri {
    doc(name).to_uri(&project_session.get_current_directory())
}

/// Go `projectSession.DidChangeWatchedFiles(ctx, []*lsproto.FileEvent{{Uri: ..., Type: lsproto.FileChangeTypeChanged}})`.
fn changed_watched_file(project_session: &Rc<project::Session>, name: &str) {
    project_session.did_change_watched_files(
        &bg(),
        &[Some(lsproto::FileEvent {
            uri: doc_uri(project_session, name),
            type_: CHANGED,
        })],
    );
}

/// Go `projectSession.Snapshot().ProjectCollection.ConfiguredProject(tspath.Path(p)).IsDirty()`.
fn configured_project_is_dirty(project_session: &Rc<project::Session>, p: &str) -> bool {
    configured_project(project_session, p)
        .unwrap_or_else(|| panic!("no configured project {p}"))
        .borrow()
        .is_dirty()
}

/// Go `len(projectSession.Snapshot().ProjectCollection.SyntheticProjects())`.
fn synthetic_projects_len(project_session: &Rc<project::Session>) -> usize {
    project_session
        .snapshot()
        .project_collection
        .synthetic_projects()
        .len()
}

/// Go `&GetCurrentLanguageServerSnapshotParams{BaseSnapshot: base, Changes: &LanguageServerSnapshotChanges{...}}`.
fn params(
    base_snapshot: api::SnapshotID,
    changes: Option<SnapshotRequestChangesParams>,
) -> GetCurrentLanguageServerSnapshotParams {
    GetCurrentLanguageServerSnapshotParams {
        base_snapshot,
        changes: changes.map(|changes| LanguageServerSnapshotChanges {
            snapshot_request_changes_params: changes,
        }),
    }
}

/// Go `session.handleGetCurrentLanguageServerSnapshot(context.Background(), params)`.
fn get_current(
    session: &api::Session,
    params: &GetCurrentLanguageServerSnapshotParams,
) -> Result<CreateSnapshotResponse, GoError> {
    session.handle_get_current_language_server_snapshot(&bg(), params)
}

/// Go `[]*CreateSnapshotProgramParams{{RootFiles: {{FileName: name}}, CompilerOptions: core.CompilerOptions{NoLib: core.TSTrue}}}`.
fn create_no_lib_program(name: &str) -> Option<Vec<Option<CreateSnapshotProgramParams>>> {
    Some(vec![Some(CreateSnapshotProgramParams {
        root_files: vec![doc(name)],
        compiler_options: CompilerOptions {
            no_lib: Tristate::True,
            ..Default::default()
        },
        compiler_options_input: None,
        options: None,
    })])
}

/// Go `(*response.Operation.OpenedFiles)`.
fn opened_files(response: &CreateSnapshotResponse) -> &Vec<api::OpenedFileOperationResult> {
    response
        .operation
        .as_ref()
        .and_then(|operation| operation.opened_files.as_ref())
        .expect("the response has no opened files")
}

// Go: session_apistate_test.go:28 TestGetCurrentLanguageServerSnapshotAdoptsChanges
child_test! {
    fn get_current_language_server_snapshot_adopts_changes() {
        const CONFIG_FILE_NAME: &str = "/home/projects/p/tsconfig.json";
        const FILE_NAME: &str = "/home/projects/p/src/index.ts";
        let (project_session, utils) = projecttestutil::setup(files(&[
            (CONFIG_FILE_NAME, r#"{ "compilerOptions": { "strict": true } }"#),
            (FILE_NAME, "export const x = 1;"),
        ]));

        let session = api::new_lsp_session(project_session.clone(), None);
        let response = nil_error(get_current(
            &session,
            &params(
                Default::default(),
                Some(SnapshotRequestChangesParams {
                    open_projects: vec![doc(CONFIG_FILE_NAME)],
                    ..Default::default()
                }),
            ),
        ));
        assert_eq!(session.open_projects.borrow().len(), 1);
        assert_eq!(
            response.snapshot,
            api::snapshot_handle(&project_session.snapshot())
        );
        assert!(configured_project(&project_session, CONFIG_FILE_NAME).is_some());
        assert!(utils.fs().write_file(FILE_NAME, "export const x = 2;").is_ok());
        changed_watched_file(&project_session, FILE_NAME);
        let dirty = nil_error(get_current(&session, &params(response.snapshot, None)));
        assert!(configured_project_is_dirty(&project_session, CONFIG_FILE_NAME));

        let unchanged = nil_error(get_current(
            &session,
            &params(
                dirty.snapshot,
                Some(SnapshotRequestChangesParams {
                    open_projects: vec![doc(CONFIG_FILE_NAME)],
                    ..Default::default()
                }),
            ),
        ));
        assert!(!unchanged.projects[0].dirty);
        assert_eq!(session.open_projects.borrow().len(), 1);

        let removed = nil_error(get_current(
            &session,
            &params(
                unchanged.snapshot,
                Some(SnapshotRequestChangesParams {
                    close_projects: vec![doc(CONFIG_FILE_NAME)],
                    ..Default::default()
                }),
            ),
        ));
        assert_eq!(removed.projects.len(), 0);
        assert_eq!(
            removed
                .changes
                .as_ref()
                .expect("the response has no changes")
                .removed_projects,
            vec![configured_project_id(CONFIG_FILE_NAME)]
        );
        assert_eq!(session.open_projects.borrow().len(), 0);

        session.close();
        assert_eq!(session.open_projects.borrow().len(), 0);
        assert!(configured_project(&project_session, CONFIG_FILE_NAME).is_none());
        project_session.close();
    }
}

// Go: session_apistate_test.go:87 TestGetCurrentLanguageServerSnapshotRejectsStandaloneSession
child_test! {
    fn get_current_language_server_snapshot_rejects_standalone_session() {
        let (init, _) = projecttestutil::get_session_init_options(
            files(&[]),
            None,
            TypingsInstallerOptions::default(),
        );
        let session = api::new_standalone_session(&init, None);

        error_contains(
            get_current(&session, &GetCurrentLanguageServerSnapshotParams::default()),
            "requires an LSP-connected API session",
        );
        session.close();
    }
}

// Go: session_apistate_test.go:98 TestOpenProjectRejectsReservedProjectID
child_test! {
    fn open_project_rejects_reserved_project_id() {
        let (init, _) = projecttestutil::get_session_init_options(
            files(&[]),
            None,
            TypingsInstallerOptions::default(),
        );
        let session = api::new_standalone_session(&init, None);

        error_contains(
            session.to_api_snapshot_request(
                &bg(),
                &SnapshotRequestChangesParams {
                    open_projects: vec![doc("/dev/null/inferred")],
                    ..Default::default()
                },
            ),
            "invalid configured project ID",
        );
        session.close();
    }
}

// Go: session_apistate_test.go:111 TestGetCurrentLanguageServerSnapshotCloseAndReopenProject
child_test! {
    fn get_current_language_server_snapshot_close_and_reopen_project() {
        const CONFIG_FILE_NAME: &str = "/home/projects/p/tsconfig.json";
        let (project_session, _) = projecttestutil::setup(files(&[(CONFIG_FILE_NAME, "{}")]));

        let session = api::new_lsp_session(project_session.clone(), None);
        nil_error(get_current(
            &session,
            &params(
                Default::default(),
                Some(SnapshotRequestChangesParams {
                    open_projects: vec![doc(CONFIG_FILE_NAME)],
                    ..Default::default()
                }),
            ),
        ));

        nil_error(get_current(
            &session,
            &params(
                Default::default(),
                Some(SnapshotRequestChangesParams {
                    close_projects: vec![doc(CONFIG_FILE_NAME)],
                    open_projects: vec![doc(CONFIG_FILE_NAME)],
                    ..Default::default()
                }),
            ),
        ));
        assert_eq!(session.open_projects.borrow().len(), 1);
        assert!(configured_project(&project_session, CONFIG_FILE_NAME).is_some());

        nil_error(get_current(
            &session,
            &params(
                Default::default(),
                Some(SnapshotRequestChangesParams {
                    close_projects: vec![doc(CONFIG_FILE_NAME)],
                    ..Default::default()
                }),
            ),
        ));
        assert_eq!(session.open_projects.borrow().len(), 0);
        session.close();
        project_session.close();
    }
}

// Go: session_apistate_test.go:144 TestGetCurrentLanguageServerSnapshotCloseAndReopenFile
child_test! {
    fn get_current_language_server_snapshot_close_and_reopen_file() {
        const FILE_NAME: &str = "/home/projects/p/index.ts";
        let (project_session, _) = projecttestutil::setup(files(&[
            ("/home/projects/p/tsconfig.json", "{}"),
            (FILE_NAME, "export const value = 1;"),
        ]));
        let session = api::new_lsp_session(project_session.clone(), None);
        nil_error(get_current(
            &session,
            &params(
                Default::default(),
                Some(SnapshotRequestChangesParams {
                    open_files: Some(vec![doc(FILE_NAME)]),
                    ..Default::default()
                }),
            ),
        ));

        nil_error(get_current(
            &session,
            &params(
                Default::default(),
                Some(SnapshotRequestChangesParams {
                    close_files: vec![doc(FILE_NAME)],
                    open_files: Some(vec![doc(FILE_NAME)]),
                    ..Default::default()
                }),
            ),
        ));
        assert_eq!(session.open_files.borrow().len(), 1);
        assert!(
            project_session
                .snapshot()
                .get_default_project(&doc_uri(&project_session, FILE_NAME))
                .is_some()
        );

        nil_error(get_current(
            &session,
            &params(
                Default::default(),
                Some(SnapshotRequestChangesParams {
                    close_files: vec![doc(FILE_NAME)],
                    ..Default::default()
                }),
            ),
        ));
        assert_eq!(session.open_files.borrow().len(), 0);
        session.close();
        project_session.close();
    }
}

// Go: session_apistate_test.go:177 TestGetCurrentLanguageServerSnapshotFlushesPendingLSPChanges
child_test! {
    fn get_current_language_server_snapshot_flushes_pending_lsp_changes() {
        const FILE_NAME: &str = "/home/projects/p/index.ts";
        let (project_session, _) =
            projecttestutil::setup(files(&[(FILE_NAME, "export const value: string = 1;")]));

        project_session.did_open_file(
            &bg(),
            &doc_uri(&project_session, FILE_NAME),
            1,
            r#"export const value: string = "ok";"#,
            &lsproto::LanguageKind::TYPE_SCRIPT,
        );

        let session = api::new_lsp_session(project_session.clone(), None);
        let response = nil_error(get_current(
            &session,
            &GetCurrentLanguageServerSnapshotParams::default(),
        ));
        assert_eq!(
            response.snapshot,
            api::snapshot_handle(&project_session.snapshot())
        );

        let snapshot = nil_error(session.get_snapshot_data(response.snapshot));
        assert_eq!(
            snapshot
                .snapshot
                .get_file(FILE_NAME)
                .expect("the snapshot has no file")
                .content(),
            r#"export const value: string = "ok";"#
        );
        session.close();
        project_session.close();
    }
}

// Go: session_apistate_test.go:205 TestGetCurrentLanguageServerSnapshotReportsOpenedFilesInRequestOrder
child_test! {
    fn get_current_language_server_snapshot_reports_opened_files_in_request_order() {
        const CONFIGURED_FILE: &str = "/home/projects/p/index.ts";
        const INFERRED_FILE: &str = "/home/projects/loose.ts";
        let (project_session, utils) = projecttestutil::setup(files(&[
            ("/home/projects/p/tsconfig.json", "{}"),
            (CONFIGURED_FILE, "export const configured = 1;"),
            (INFERRED_FILE, "export const inferred = 1;"),
        ]));
        let session = api::new_lsp_session(project_session.clone(), None);

        let changes = SnapshotRequestChangesParams {
            open_files: Some(vec![doc(INFERRED_FILE), doc(CONFIGURED_FILE)]),
            ..Default::default()
        };
        let first = nil_error(get_current(
            &session,
            &params(Default::default(), Some(changes.clone())),
        ));
        assert_eq!(opened_files(&first).len(), 2);
        assert_eq!(opened_files(&first)[0].project, inferred_project_id());
        assert_eq!(
            opened_files(&first)[1].project,
            configured_project_id("/home/projects/p/tsconfig.json")
        );
        assert!(
            utils
                .fs()
                .write_file(CONFIGURED_FILE, "export const configured = 2;")
                .is_ok()
        );
        changed_watched_file(&project_session, CONFIGURED_FILE);
        let dirty = nil_error(get_current(&session, &params(first.snapshot, None)));
        assert!(configured_project_is_dirty(
            &project_session,
            "/home/projects/p/tsconfig.json"
        ));

        let reopened = nil_error(get_current(
            &session,
            &params(dirty.snapshot, Some(changes)),
        ));
        assert_eq!(opened_files(&reopened), opened_files(&first));
        assert!(!configured_project_is_dirty(
            &project_session,
            "/home/projects/p/tsconfig.json"
        ));
        assert_eq!(session.open_files.borrow().len(), 2);
        session.close();
        project_session.close();
    }
}

// Go: session_apistate_test.go:246 TestGetCurrentLanguageServerSnapshotCreatesAndRemovesPrograms
child_test! {
    fn get_current_language_server_snapshot_creates_and_removes_programs() {
        const FILE_NAME: &str = "/home/projects/p/index.ts";
        let (project_session, _) =
            projecttestutil::setup(files(&[(FILE_NAME, "export const value = 1;")]));

        let session = api::new_lsp_session(project_session.clone(), None);
        let created = nil_error(get_current(
            &session,
            &params(
                Default::default(),
                Some(SnapshotRequestChangesParams {
                    create_programs: create_no_lib_program(FILE_NAME),
                    ..Default::default()
                }),
            ),
        ));
        assert_eq!(created.projects.len(), 1);
        assert_eq!(synthetic_projects_len(&project_session), 1);

        let removed = nil_error(get_current(
            &session,
            &params(
                Default::default(),
                Some(SnapshotRequestChangesParams {
                    remove_programs: vec![synthetic_project_id(1), synthetic_project_id(1)],
                    ..Default::default()
                }),
            ),
        ));
        assert_eq!(removed.projects.len(), 0);
        assert_eq!(synthetic_projects_len(&project_session), 0);
        session.close();
        project_session.close();
    }
}

// Go: session_apistate_test.go:277 TestOpenFilePreservesWindowsDriveLetterCase
// PORT: Go sets `init.Options.CurrentDirectory` after
// `GetSessionInitOptions`; the Rust options are shared (`Rc`), so the test
// passes the default options with that directory.
child_test! {
    fn open_file_preserves_windows_drive_letter_case() {
        const FILE_NAME: &str = "D:/repo/index.ts";
        let (init, _) = projecttestutil::get_session_init_options(
            files(&[
                ("D:/repo/tsconfig.json", "{}"),
                (FILE_NAME, "export const value = 1;"),
            ]),
            Some(project::SessionOptions {
                current_directory: "D:/repo".to_string(),
                ..projecttestutil::default_session_options()
            }),
            TypingsInstallerOptions::default(),
        );
        let project_session = project::new_session(&init);

        let session = api::new_lsp_session(project_session.clone(), None);

        let response = nil_error(get_current(
            &session,
            &params(
                Default::default(),
                Some(SnapshotRequestChangesParams {
                    open_files: Some(vec![doc(FILE_NAME)]),
                    ..Default::default()
                }),
            ),
        ));

        let project = &response.projects[0];
        let snapshot = nil_error(session.get_snapshot_data(response.snapshot));
        let program = nil_error(snapshot.get_program(&project.id));
        assert_eq!(
            program
                .get_source_file(FILE_NAME)
                .expect("the program has no file")
                .file_name(),
            FILE_NAME
        );
        session.close();
        project_session.close();
    }
}

// Go: session_apistate_test.go:307 TestClosingAPISessionRemovesCreatedLanguageServerPrograms
child_test! {
    fn closing_api_session_removes_created_language_server_programs() {
        const FILE_NAME: &str = "/home/projects/p/index.ts";
        let (project_session, _) =
            projecttestutil::setup(files(&[(FILE_NAME, "export const value = 1;")]));

        let session = api::new_lsp_session(project_session.clone(), None);
        nil_error(get_current(
            &session,
            &params(
                Default::default(),
                Some(SnapshotRequestChangesParams {
                    create_programs: create_no_lib_program(FILE_NAME),
                    ..Default::default()
                }),
            ),
        ));
        assert_eq!(synthetic_projects_len(&project_session), 1);

        session.close();
        assert_eq!(synthetic_projects_len(&project_session), 0);
        project_session.close();
    }
}

// Go: session_apistate_test.go:330 TestLanguageServerProgramOwnershipIsIsolatedByAPISession
child_test! {
    fn language_server_program_ownership_is_isolated_by_api_session() {
        const FILE_NAME: &str = "/home/projects/p/index.ts";
        let (project_session, _) =
            projecttestutil::setup(files(&[(FILE_NAME, "export const value = 1;")]));

        let owner = api::new_lsp_session(project_session.clone(), None);
        nil_error(get_current(
            &owner,
            &params(
                Default::default(),
                Some(SnapshotRequestChangesParams {
                    create_programs: create_no_lib_program(FILE_NAME),
                    ..Default::default()
                }),
            ),
        ));

        let other = api::new_lsp_session(project_session.clone(), None);
        nil_error(get_current(
            &other,
            &params(
                Default::default(),
                Some(SnapshotRequestChangesParams {
                    remove_programs: vec![synthetic_project_id(1)],
                    ..Default::default()
                }),
            ),
        ));
        assert_eq!(synthetic_projects_len(&project_session), 1);

        other.close();
        assert_eq!(synthetic_projects_len(&project_session), 1);
        owner.close();
        assert_eq!(synthetic_projects_len(&project_session), 0);
        project_session.close();
    }
}

// Go: session_apistate_test.go:363 TestLanguageServerProgramReconfigurationIsIsolatedByAPISession
child_test! {
    fn language_server_program_reconfiguration_is_isolated_by_api_session() {
        const FILE_NAME: &str = "/home/projects/p/index.ts";
        let (project_session, _) =
            projecttestutil::setup(files(&[(FILE_NAME, "export const value = 1;")]));

        let owner = api::new_lsp_session(project_session.clone(), None);
        let created = nil_error(get_current(
            &owner,
            &params(
                Default::default(),
                Some(SnapshotRequestChangesParams {
                    create_programs: create_no_lib_program(FILE_NAME),
                    ..Default::default()
                }),
            ),
        ));
        let program_id = project::SyntheticProjectID(created.projects[0].id.0.clone());

        let other = api::new_lsp_session(project_session.clone(), None);
        error_contains(
            get_current(
                &other,
                &params(
                    Default::default(),
                    Some(SnapshotRequestChangesParams {
                        reconfigure_programs: vec![Some(ReconfigureSnapshotProgramParams {
                            id: program_id,
                            root_files: vec![doc(FILE_NAME)],
                            compiler_options: CompilerOptions {
                                no_lib: Tristate::True,
                                strict: Tristate::True,
                                ..Default::default()
                            },
                            compiler_options_input: None,
                            options: None,
                        })],
                        ..Default::default()
                    }),
                ),
            ),
            "not owned by this API session",
        );
        other.close();
        owner.close();
        project_session.close();
    }
}

// Go: session_apistate_test.go:397 TestOpeningProjectOwnedByAnotherAPISessionEnsuresProgram
child_test! {
    fn opening_project_owned_by_another_api_session_ensures_program() {
        const CONFIG_FILE_NAME: &str = "/home/projects/p/tsconfig.json";
        const FILE_NAME: &str = "/home/projects/p/index.ts";
        let (project_session, utils) = projecttestutil::setup(files(&[
            (CONFIG_FILE_NAME, "{}"),
            (FILE_NAME, "export const value = 1;"),
        ]));
        let open_project = SnapshotRequestChangesParams {
            open_projects: vec![doc(CONFIG_FILE_NAME)],
            ..Default::default()
        };

        let owner = api::new_lsp_session(project_session.clone(), None);
        nil_error(get_current(
            &owner,
            &params(Default::default(), Some(open_project.clone())),
        ));
        assert!(utils.fs().write_file(FILE_NAME, "export const value = 2;").is_ok());
        changed_watched_file(&project_session, FILE_NAME);
        nil_error(get_current(
            &owner,
            &GetCurrentLanguageServerSnapshotParams::default(),
        ));
        assert!(configured_project_is_dirty(&project_session, CONFIG_FILE_NAME));

        let other = api::new_lsp_session(project_session.clone(), None);
        let opened = nil_error(get_current(
            &other,
            &params(Default::default(), Some(open_project)),
        ));
        assert!(!opened.projects[0].dirty);
        assert_eq!(other.open_projects.borrow().len(), 1);

        other.close();
        assert!(configured_project(&project_session, CONFIG_FILE_NAME).is_some());
        owner.close();
        assert!(configured_project(&project_session, CONFIG_FILE_NAME).is_none());
        project_session.close();
    }
}

// Go: session_apistate_test.go:435 TestFailedLanguageServerSnapshotOpenIsNotAdopted
child_test! {
    fn failed_language_server_snapshot_open_is_not_adopted() {
        let (project_session, _) = projecttestutil::setup(files(&[("/notes.txt", "text")]));
        let session = api::new_lsp_session(project_session.clone(), None);

        let base_snapshot = project_session.snapshot();
        let response = get_current(
            &session,
            &params(
                Default::default(),
                Some(SnapshotRequestChangesParams {
                    open_files: Some(vec![doc("/notes.txt")]),
                    ..Default::default()
                }),
            ),
        );

        error_contains(response, "no project found for opened file");
        assert!(Rc::ptr_eq(&project_session.snapshot(), &base_snapshot));
        assert_eq!(session.open_files.borrow().len(), 0);
        session.close();
        project_session.close();
    }
}

// Go: session_apistate_test.go:458 TestGetCurrentLanguageServerSnapshotOpeningLSPFileEnsuresConfiguredProgram
child_test! {
    fn get_current_language_server_snapshot_opening_lsp_file_ensures_configured_program() {
        const CONFIG_FILE_NAME: &str = "/home/projects/p/tsconfig.json";
        const FILE_NAME: &str = "/home/projects/p/index.ts";
        let (project_session, _) = projecttestutil::setup(files(&[
            (CONFIG_FILE_NAME, "{}"),
            (FILE_NAME, "export const value = 1;"),
        ]));
        let uri = doc_uri(&project_session, FILE_NAME);
        project_session.did_open_file(
            &bg(),
            &uri,
            1,
            "export const value = 1;",
            &lsproto::LanguageKind::TYPE_SCRIPT,
        );

        let session = api::new_lsp_session(project_session.clone(), None);
        let initial = nil_error(get_current(
            &session,
            &GetCurrentLanguageServerSnapshotParams::default(),
        ));
        assert!(!initial.projects[0].dirty);
        let project_id = initial.projects[0].id.clone();

        project_session.did_change_file(
            &bg(),
            &uri,
            2,
            &[lsproto::TextDocumentContentChangePartialOrWholeDocument {
                partial: None,
                whole_document: Some(lsproto::TextDocumentContentChangeWholeDocument {
                    text: "export const value = 2;".to_string(),
                }),
            }],
        );
        let dirty = nil_error(get_current(&session, &params(initial.snapshot, None)));
        assert!(dirty.projects[0].dirty);

        let ensured = nil_error(get_current(
            &session,
            &params(
                dirty.snapshot,
                Some(SnapshotRequestChangesParams {
                    open_files: Some(vec![doc(FILE_NAME)]),
                    ..Default::default()
                }),
            ),
        ));
        assert!(!ensured.projects[0].dirty);
        assert_eq!(opened_files(&ensured)[0].project, project_id);
        session.close();
        project_session.close();
    }
}
