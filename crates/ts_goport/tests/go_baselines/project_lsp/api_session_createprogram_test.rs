//! Port of Go `internal/api/session_createprogram_test.go` (ts#64204,
//! ts#64319, ts#64391).
//!
//! PORT: the tests are in `project_lsp` because they use `projecttestutil`
//! and `child_test!`. Go `bundled.Embedded` is always true in the port, so
//! the skip is dropped. A Go `defer ...Close()` is a call at the end of the
//! test, in the Go defer order.

use std::rc::Rc;

use ts_goport::api::{
    self, CreateSnapshotParams, DocumentIdentifier, EnsurePrograms, FileNotifications,
    ReconfigureSnapshotProgramParams, SnapshotID, SnapshotRequestChangesParams,
    UpdateSnapshotParams,
};
use ts_goport::frontend::json::{json_marshal, json_unmarshal};
use ts_goport::gostd::errors;
use ts_goport::lsp::lsproto;
use ts_goport::options::{CompilerOptions, Tristate};
use ts_goport::project;

use super::api_util::{doc, error_contains, nil_error, no_lib, program_params};
use super::projecttestutil::{self, TypingsInstallerOptions, files};
use super::util::bg;

// Go: session_apistate_test.go:24 syntheticProjectID
fn synthetic_project_id(id: i32) -> project::SyntheticProjectID {
    project::new_synthetic_project_id(id)
}

/// Go `core.CompilerOptions{NoLib: core.TSTrue, Strict: core.TSTrue}`.
fn no_lib_strict() -> CompilerOptions {
    CompilerOptions {
        no_lib: Tristate::True,
        strict: Tristate::True,
        ..Default::default()
    }
}

/// Go `&CreateSnapshotParams{CreatePrograms: programs}`.
fn create_programs(programs: Vec<api::CreateSnapshotProgramParams>) -> CreateSnapshotParams {
    CreateSnapshotParams {
        snapshot_request_changes_params: SnapshotRequestChangesParams {
            create_programs: Some(programs.into_iter().map(Some).collect()),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// Go `json.Marshal(response.Operation)` with `assert.NilError`.
fn marshal_operation(operation: &Option<api::SnapshotOperationResponse>) -> String {
    json_marshal(operation, &[]).unwrap_or_else(|err| panic!("marshal: {err:?}"))
}

// Go: session_createprogram_test.go:15 TestCreateSnapshotUsesIndependentRoots
child_test! {
    fn create_snapshot_uses_independent_roots() {
        let (init, _) = projecttestutil::get_session_init_options(
            files(&[("/home/projects/p/src/index.ts", "export const x = 1;")]),
            None,
            TypingsInstallerOptions::default(),
        );
        let session = api::new_standalone_session(&init, None);

        let first_response = nil_error(session.handle_create_snapshot(
            &bg(),
            &create_programs(vec![program_params(
                &["/home/projects/p/src/index.ts"],
                no_lib(),
            )]),
        ));
        assert_eq!(first_response.snapshot, SnapshotID(1));
        assert_eq!(first_response.projects.len(), 1);

        let response =
            nil_error(session.handle_create_snapshot(&bg(), &CreateSnapshotParams::default()));
        assert_eq!(response.snapshot, SnapshotID(2));
        assert_eq!(response.projects.len(), 0);
        assert_eq!(first_response.projects.len(), 1);
        session.close();
    }
}

// Go: session_createprogram_test.go:44 TestCreateSnapshotCreatesPrograms
child_test! {
    fn create_snapshot_creates_programs() {
        const FILE_A: &str = "/home/projects/p/a.ts";
        const FILE_B: &str = "/home/projects/p/b.ts";
        let (project_session, _) = projecttestutil::setup(files(&[
            (FILE_A, "export const a = 1;"),
            (FILE_B, "export const b = 1;"),
        ]));

        let session = api::new_lsp_session(project_session.clone(), None);

        let response = nil_error(session.handle_create_snapshot(
            &bg(),
            &create_programs(vec![
                program_params(&[FILE_A, FILE_B], no_lib_strict()),
                program_params(&[FILE_B], no_lib()),
            ]),
        ));
        assert_eq!(response.projects.len(), 2);
        assert_eq!(
            response.operation.as_ref().unwrap().created_programs.as_ref().unwrap(),
            &vec![synthetic_project_id(1), synthetic_project_id(2)]
        );
        // ts#64159: nil (Go session_createprogram_test.go:76).
        assert_eq!(response.projects[0].config_file_name, None);
        assert_eq!(response.projects[1].config_file_name, None);
        assert_eq!(response.projects[0].root_files, vec![FILE_A, FILE_B]);
        assert_eq!(
            response.projects[0].compiler_options.as_ref().unwrap().strict,
            Tristate::True
        );
        assert_eq!(response.projects[1].root_files, vec![FILE_B]);

        let snapshot = nil_error(session.get_snapshot_data(response.snapshot));
        assert_eq!(snapshot.snapshot.created_programs().len(), 2);
        for project_response in &response.projects {
            assert!(
                snapshot
                    .snapshot
                    .project_collection
                    .get_project(&project_response.id)
                    .is_some()
            );
        }
        session.close();
        project_session.close();
    }
}

// Go: session_createprogram_test.go:89 TestCreateSnapshotPreservesWindowsRootDriveLetterCase
child_test! {
    fn create_snapshot_preserves_windows_root_drive_letter_case() {
        const FILE_NAME: &str = "D:/repo/index.ts";
        let (mut init, _) = projecttestutil::get_session_init_options(
            files(&[(FILE_NAME, "export const value = 1;")]),
            None,
            TypingsInstallerOptions::default(),
        );
        // Go: init.Options.CurrentDirectory = "D:/repo"
        Rc::get_mut(&mut init.options)
            .expect("the init options are not shared yet")
            .current_directory = "D:/repo".to_string();
        let session = api::new_standalone_session(&init, None);

        let response = nil_error(session.handle_create_snapshot(
            &bg(),
            &create_programs(vec![api::CreateSnapshotProgramParams {
                root_files: vec![DocumentIdentifier {
                    uri: lsproto::DocumentUri("file:///D%3A/repo/index.ts".to_string()),
                    ..Default::default()
                }],
                compiler_options: no_lib(),
                options: None,
            }]),
        ));

        let snapshot = nil_error(session.get_snapshot_data(response.snapshot));
        let program = nil_error(snapshot.get_program(&response.projects[0].id));
        assert_eq!(
            program.get_source_file(FILE_NAME).unwrap().file_name(),
            FILE_NAME
        );
        session.close();
    }
}

// Go: session_createprogram_test.go:115 TestSnapshotOperationResponseOmitsUnrequestedFields
child_test! {
    fn snapshot_operation_response_omits_unrequested_fields() {
        let (project_session, _) = projecttestutil::setup(files(&[]));
        let session = api::new_lsp_session(project_session.clone(), None);

        let response =
            nil_error(session.handle_create_snapshot(&bg(), &CreateSnapshotParams::default()));
        let encoded = marshal_operation(&response.operation);
        assert_eq!(encoded, "{}");

        let response = nil_error(session.handle_create_snapshot(
            &bg(),
            &CreateSnapshotParams {
                snapshot_request_changes_params: SnapshotRequestChangesParams {
                    create_programs: Some(vec![]),
                    reconfigure_programs: vec![],
                    open_files: Some(vec![]),
                    ..Default::default()
                },
                ..Default::default()
            },
        ));
        let encoded = marshal_operation(&response.operation);
        assert_eq!(encoded, r#"{"createdPrograms":[],"openedFiles":[]}"#);
        session.close();
        project_session.close();
    }
}

// Go: session_createprogram_test.go:140 TestUpdateSnapshotReconfiguresSyntheticProgram
child_test! {
    fn update_snapshot_reconfigures_synthetic_program() {
        let (project_session, _) = projecttestutil::setup(files(&[
            ("/home/projects/p/a.ts", "export const a = 1;"),
            ("/home/projects/p/b.ts", "export const b = 2;"),
        ]));
        let session = api::new_lsp_session(project_session.clone(), None);

        let created = nil_error(session.handle_create_snapshot(
            &bg(),
            &create_programs(vec![program_params(&["/home/projects/p/a.ts"], no_lib())]),
        ));
        let program_id =
            created.operation.as_ref().unwrap().created_programs.as_ref().unwrap()[0].clone();

        let reconfigured = nil_error(session.handle_update_snapshot(
            &bg(),
            &UpdateSnapshotParams {
                snapshot: created.snapshot,
                changes: Some(CreateSnapshotParams {
                    snapshot_request_changes_params: SnapshotRequestChangesParams {
                        reconfigure_programs: vec![Some(ReconfigureSnapshotProgramParams {
                            id: program_id.clone(),
                            root_files: vec![doc("/home/projects/p/b.ts")],
                            compiler_options: no_lib_strict(),
                            options: None,
                        })],
                        ..Default::default()
                    },
                    ..Default::default()
                }),
            },
        ));
        assert_eq!(reconfigured.projects[0].id, program_id.as_id());
        assert_eq!(
            reconfigured.projects[0].root_files,
            vec!["/home/projects/p/b.ts"]
        );
        assert_eq!(
            reconfigured.projects[0].compiler_options.as_ref().unwrap().strict,
            Tristate::True
        );
        session.close();
        project_session.close();
    }
}

// Go: session_createprogram_test.go:176 TestReconfigureSyntheticProgramValidation
child_test! {
    fn reconfigure_synthetic_program_validation() {
        let (project_session, _) = projecttestutil::setup(files(&[]));
        let session = api::new_lsp_session(project_session.clone(), None);

        let program = ReconfigureSnapshotProgramParams {
            id: project::SyntheticProjectID("/dev/null/synthetic/1".to_string()),
            ..Default::default()
        };
        let mut null_reconfigure = SnapshotRequestChangesParams::default();
        json_unmarshal(br#"{"reconfigurePrograms":[null]}"#, &mut null_reconfigure, &[])
            .unwrap_or_else(|err| panic!("unmarshal: {err:?}"));
        error_contains(
            session.to_api_snapshot_request(&bg(), &null_reconfigure),
            "reconfigurePrograms[0] must not be null",
        );

        error_contains(
            session.to_api_snapshot_request(
                &bg(),
                &SnapshotRequestChangesParams {
                    reconfigure_programs: vec![Some(ReconfigureSnapshotProgramParams {
                        id: project::SyntheticProjectID("/tsconfig.json".to_string()),
                        ..Default::default()
                    })],
                    ..Default::default()
                },
            ),
            "invalid synthetic project handle",
        );

        error_contains(
            session.to_api_snapshot_request(
                &bg(),
                &SnapshotRequestChangesParams {
                    reconfigure_programs: vec![Some(program.clone()), Some(program.clone())],
                    ..Default::default()
                },
            ),
            "reconfigured more than once",
        );

        error_contains(
            session.to_api_snapshot_request(
                &bg(),
                &SnapshotRequestChangesParams {
                    reconfigure_programs: vec![Some(program.clone())],
                    remove_programs: vec![program.id.clone()],
                    ..Default::default()
                },
            ),
            "cannot be reconfigured and removed",
        );

        error_contains(
            session.handle_create_snapshot(
                &bg(),
                &CreateSnapshotParams {
                    snapshot_request_changes_params: SnapshotRequestChangesParams {
                        create_programs: Some(vec![Some(Default::default())]),
                        reconfigure_programs: vec![Some(program.clone())],
                        ..Default::default()
                    },
                    ..Default::default()
                },
            ),
            "not found for reconfiguration",
        );
        session.close();
        project_session.close();
    }
}

// Go: session_createprogram_test.go:213 TestCreateSyntheticProgramValidation
child_test! {
    fn create_synthetic_program_validation() {
        let (project_session, _) = projecttestutil::setup(files(&[]));
        let session = api::new_lsp_session(project_session.clone(), None);

        let mut null_create = SnapshotRequestChangesParams::default();
        json_unmarshal(br#"{"createPrograms":[null]}"#, &mut null_create, &[])
            .unwrap_or_else(|err| panic!("unmarshal: {err:?}"));
        let result = session.to_api_snapshot_request(&bg(), &null_create);
        let err = result.as_ref().err().cloned();
        error_contains(result, "createPrograms[0] must not be null");
        assert!(errors::is(&err.unwrap(), &api::ERR_CLIENT_ERROR));
        session.close();
        project_session.close();
    }
}

// Go: session_createprogram_test.go:228 TestCreateSnapshotRejectsRemovingProgramFromIndependentRoot
child_test! {
    fn create_snapshot_rejects_removing_program_from_independent_root() {
        let (project_session, _) = projecttestutil::setup(files(&[]));

        let session = api::new_lsp_session(project_session.clone(), None);

        error_contains(
            session.handle_create_snapshot(
                &bg(),
                &CreateSnapshotParams {
                    snapshot_request_changes_params: SnapshotRequestChangesParams {
                        remove_programs: vec![synthetic_project_id(1)],
                        ..Default::default()
                    },
                    ..Default::default()
                },
            ),
            "synthetic program not found for removal: /dev/null/synthetic/1",
        );
        session.close();
        project_session.close();
    }
}

// Go: session_createprogram_test.go:243 TestUpdateSnapshotEnsuresSyntheticProgram
child_test! {
    fn update_snapshot_ensures_synthetic_program() {
        const FILE_NAME: &str = "/home/projects/p/index.ts";
        let (project_session, utils) =
            projecttestutil::setup(files(&[(FILE_NAME, "export const value = 1;")]));
        let session = api::new_lsp_session(project_session.clone(), None);

        let created = nil_error(session.handle_create_snapshot(
            &bg(),
            &create_programs(vec![program_params(&[FILE_NAME], no_lib())]),
        ));
        assert!(!created.projects[0].dirty);
        let project_id = created.projects[0].id.clone();

        utils
            .fs()
            .write_file(FILE_NAME, "export const value = 2;")
            .unwrap();
        let dirty = nil_error(session.handle_update_snapshot(
            &bg(),
            &UpdateSnapshotParams {
                snapshot: created.snapshot,
                changes: Some(CreateSnapshotParams {
                    file_notifications: Some(FileNotifications {
                        changed: vec![doc(FILE_NAME)],
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
            },
        ));
        assert!(dirty.projects[0].dirty);

        let ensured = nil_error(session.handle_update_snapshot(
            &bg(),
            &UpdateSnapshotParams {
                snapshot: dirty.snapshot,
                changes: Some(CreateSnapshotParams {
                    snapshot_request_changes_params: SnapshotRequestChangesParams {
                        ensure_programs: Some(EnsurePrograms {
                            projects: vec![project_id],
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                    ..Default::default()
                }),
            },
        ));
        assert!(!ensured.projects[0].dirty);
        session.close();
        project_session.close();
    }
}
