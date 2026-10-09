//! Port of Go `internal/api/session_requestfilesystem_test.go` (ts#64115,
//! ts#64204, ts#64277, ts#64291, ts#64391).
//!
//! PORT: the tests are in `project_lsp` because they use `projecttestutil`
//! and `child_test!`. A Go `defer ...Close()` is a call at the end of the
//! test, in the Go defer order. Go compares programs and file handles by
//! pointer; the port uses `Rc::ptr_eq`. A Go subtest of a table is one
//! `#[test]` named `<test>_<subtest>`.

use std::rc::Rc;

use ts_goport::api::requestfilesystem::{
    Kind, RequestFileSystem, RequestSymlink, has_full_file_system,
};
use ts_goport::api::{
    self, CreateSnapshotParams, CreateSnapshotResponse, EmitParams, EnsurePrograms,
    GetCurrentLanguageServerSnapshotParams, LanguageServerSnapshotChanges, ReleaseParams,
    SnapshotID, SnapshotRequestChangesParams, UpdateSnapshotParams,
};
use ts_goport::gostd::{Context, GoError};
use ts_goport::lsp::lsproto;
use ts_goport::project::{self, FileHandle};

use super::api_util::{doc, nil_error, no_lib, program_params, project_program, snapshot_of};
use super::projecttestutil::{self, files};
use super::requestfilesystem_test::{files as request_files, removed};
use super::util::{bg, path, text, uri};

// Go: session_requestfilesystem_test.go:19 updateCurrentLanguageServerSnapshot
fn update_current_language_server_snapshot(
    ctx: &Context,
    session: &api::Session,
    changes: &CreateSnapshotParams,
) -> Result<CreateSnapshotResponse, GoError> {
    let base = session.handle_get_current_language_server_snapshot(
        ctx,
        &GetCurrentLanguageServerSnapshotParams {
            changes: Some(LanguageServerSnapshotChanges {
                snapshot_request_changes_params: changes.snapshot_request_changes_params.clone(),
            }),
            ..Default::default()
        },
    )?;
    let mut request_changes = changes.snapshot_request_changes_params.clone();
    request_changes.ensure_programs = Some(EnsurePrograms {
        all: true,
        ..Default::default()
    });
    session.handle_update_snapshot(
        ctx,
        &UpdateSnapshotParams {
            snapshot: base.snapshot,
            changes: Some(CreateSnapshotParams {
                snapshot_request_changes_params: request_changes,
                file_system: changes.file_system.clone(),
                ..Default::default()
            }),
        },
    )
}

/// Go `&CreateSnapshotParams{OpenProjects: {{FileName: name}...}, FileSystem: fileSystem}`.
fn open_projects(names: &[&str], file_system: Option<RequestFileSystem>) -> CreateSnapshotParams {
    CreateSnapshotParams {
        snapshot_request_changes_params: SnapshotRequestChangesParams {
            open_projects: names.iter().map(|name| doc(name)).collect(),
            ..Default::default()
        },
        file_system,
        ..Default::default()
    }
}

/// Go `&CreateSnapshotParams{FileSystem: fileSystem}`.
fn with_file_system(file_system: RequestFileSystem) -> CreateSnapshotParams {
    CreateSnapshotParams {
        file_system: Some(file_system),
        ..Default::default()
    }
}

/// Go `&CreateSnapshotParams{EnsurePrograms: &EnsurePrograms{All: true}, FileSystem: fileSystem}`.
fn ensure_all(file_system: RequestFileSystem) -> CreateSnapshotParams {
    CreateSnapshotParams {
        snapshot_request_changes_params: SnapshotRequestChangesParams {
            ensure_programs: Some(EnsurePrograms {
                all: true,
                ..Default::default()
            }),
            ..Default::default()
        },
        file_system: Some(file_system),
        ..Default::default()
    }
}

/// Go `&UpdateSnapshotParams{Snapshot: snapshot, Changes: changes}`.
fn update(snapshot: SnapshotID, changes: Option<CreateSnapshotParams>) -> UpdateSnapshotParams {
    UpdateSnapshotParams { snapshot, changes }
}

/// Go `requestfilesystem.RequestFileSystem{Kind: kind, Files: files}`.
fn request_file_system(kind: Kind, entries: &[(&str, &str)]) -> RequestFileSystem {
    RequestFileSystem {
        kind,
        files: request_files(entries),
        ..Default::default()
    }
}

/// Go `session.handleRelease(ctx, &ReleaseParams{Snapshot: snapshot})` with `assert.NilError`.
fn release(session: &api::Session, snapshot: SnapshotID) {
    nil_error(session.handle_release(&bg(), Some(&ReleaseParams { snapshot })));
}

/// Go `session.snapshots[id].fileSystem`.
fn file_system_of(
    session: &api::Session,
    id: SnapshotID,
) -> Option<Rc<dyn ts_goport::frontend::vfs::Fs>> {
    session.snapshots.borrow()[&id].file_system.clone()
}

/// Go `a == b` of two `project.FileHandle` values.
fn same_file(a: &Option<Rc<dyn FileHandle>>, b: &Option<Rc<dyn FileHandle>>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => Rc::ptr_eq(a, b),
        (None, None) => true,
        _ => false,
    }
}

/// Go `projectSession.DidChangeFile(ctx, uri, version, {{WholeDocument: {Text: text}}})`.
fn change_whole(project_session: &Rc<project::Session>, u: &str, version: i32, content: &str) {
    project_session.did_change_file(
        &bg(),
        &uri(u),
        version,
        &[lsproto::TextDocumentContentChangePartialOrWholeDocument {
            partial: None,
            whole_document: Some(lsproto::TextDocumentContentChangeWholeDocument {
                text: content.to_string(),
            }),
        }],
    );
}

/// Go `projectSession.DidOpenFile(ctx, uri, 1, content, lsproto.LanguageKindTypeScript)`.
fn open(project_session: &Rc<project::Session>, u: &str, content: &str) {
    project_session.did_open_file(
        &bg(),
        &uri(u),
        1,
        content,
        &lsproto::LanguageKind::TYPE_SCRIPT,
    );
}

// Go: session_requestfilesystem_test.go:37 TestEditorChangeInvalidatesRequestSymlinkAlias
child_test! {
    fn editor_change_invalidates_request_symlink_alias() {
        let ctx = bg();
        let (project_session, _) = projecttestutil::setup(files(&[(
            "/tsconfig.json",
            r#"{ "compilerOptions": { "noLib": true }, "files": ["alias.ts"] }"#,
        )]));
        open(&project_session, "file:///target.ts", "old");
        let session = api::new_lsp_session(project_session.clone(), None);
        let alias_layer = || RequestFileSystem {
            kind: Kind::LAYER,
            symlinks: [(
                "/alias.ts".to_string(),
                RequestSymlink {
                    target: "/target.ts".to_string(),
                    host: false,
                },
            )]
            .into_iter()
            .collect(),
            ..Default::default()
        };

        let base = nil_error(update_current_language_server_snapshot(
            &ctx,
            &session,
            &open_projects(&["/tsconfig.json"], Some(alias_layer())),
        ));
        assert_eq!(
            text(&project_program(&snapshot_of(&session, base.snapshot), "/tsconfig.json"), "/alias.ts"),
            "old"
        );

        change_whole(&project_session, "file:///target.ts", 2, "new");
        let updated = nil_error(update_current_language_server_snapshot(
            &ctx,
            &session,
            &open_projects(&["/tsconfig.json"], Some(alias_layer())),
        ));
        assert_eq!(
            text(&project_program(&snapshot_of(&session, updated.snapshot), "/tsconfig.json"), "/alias.ts"),
            "new"
        );
        session.close();
        project_session.close();
    }
}

// Go: session_requestfilesystem_test.go:77 TestLargeRequestLayerUpdateRetainsChanges
child_test! {
    fn large_request_layer_update_retains_changes() {
        const FILLER_COUNT: usize = 1000;
        let mut base_files = Vec::with_capacity(FILLER_COUNT);
        let mut updated_files = Vec::with_capacity(FILLER_COUNT + 1);
        for i in 0..FILLER_COUNT {
            let file_name = format!("/unused/file{i}.ts");
            base_files.push((file_name.clone(), "old".to_string()));
            updated_files.push((file_name, "new".to_string()));
        }
        updated_files.push(("/index.ts".to_string(), "new".to_string()));

        let ctx = bg();
        let (project_session, _) = projecttestutil::setup(files(&[(
            "/tsconfig.json",
            r#"{ "compilerOptions": { "noLib": true }, "files": ["index.ts"] }"#,
        )]));
        open(&project_session, "file:///index.ts", "old");
        let session = api::new_lsp_session(project_session.clone(), None);

        let base = nil_error(update_current_language_server_snapshot(
            &ctx,
            &session,
            &open_projects(
                &["/tsconfig.json"],
                Some(RequestFileSystem {
                    kind: Kind::LAYER,
                    files: base_files.into_iter().collect(),
                    ..Default::default()
                }),
            ),
        ));

        let updated = nil_error(session.handle_update_snapshot(
            &ctx,
            &update(
                base.snapshot,
                Some(ensure_all(RequestFileSystem {
                    kind: Kind::LAYER,
                    files: updated_files.into_iter().collect(),
                    ..Default::default()
                })),
            ),
        ));
        let (contents, ok) = snapshot_of(&session, updated.snapshot).read_file("/index.ts");
        assert!(ok);
        assert_eq!(contents, "new");
        assert_eq!(
            text(&project_program(&snapshot_of(&session, updated.snapshot), "/tsconfig.json"), "/index.ts"),
            "new"
        );
        session.close();
        project_session.close();
    }
}

// Go: session_requestfilesystem_test.go:119 TestAutoImportCloneRetainsRequestFileSystem
child_test! {
    fn auto_import_clone_retains_request_file_system() {
        let ctx = bg();
        let (project_session, _) = projecttestutil::setup(files(&[]));
        let session = api::new_lsp_session(project_session.clone(), None);

        let base = nil_error(update_current_language_server_snapshot(
            &ctx,
            &session,
            &with_file_system(request_file_system(Kind::FULL, &[("/index.ts", "request")])),
        ));
        let base_snapshot = snapshot_of(&session, base.snapshot);

        let clone = session.snapshot_host.clone_snapshot_with_auto_imports(
            &ctx,
            &base_snapshot,
            &uri("file:///index.ts"),
            None,
        );
        let (content, ok) = clone.read_file("/index.ts");
        assert!(ok);
        assert_eq!(content, "request");
        clone.deref();
        session.close();
        project_session.close();
    }
}

// Go: session_requestfilesystem_test.go:146 TestUnmaskedOverlayContinuesUpdating
child_test! {
    fn unmasked_overlay_continues_updating() {
        let ctx = bg();
        let (project_session, _) = projecttestutil::setup(files(&[
            (
                "/tsconfig.json",
                r#"{ "compilerOptions": { "noLib": true }, "files": ["index.ts"] }"#,
            ),
            ("/index.ts", "host"),
        ]));
        open(&project_session, "file:///index.ts", "overlay1");
        let session = api::new_lsp_session(project_session.clone(), None);

        let masked = nil_error(update_current_language_server_snapshot(
            &ctx,
            &session,
            &open_projects(
                &["/tsconfig.json"],
                Some(request_file_system(Kind::LAYER, &[("/index.ts", "request")])),
            ),
        ));
        assert_eq!(
            text(&project_program(&snapshot_of(&session, masked.snapshot), "/tsconfig.json"), "/index.ts"),
            "request"
        );

        let unmasked = nil_error(update_current_language_server_snapshot(
            &ctx,
            &session,
            &open_projects(&["/tsconfig.json"], None),
        ));
        assert_eq!(
            text(&project_program(&snapshot_of(&session, unmasked.snapshot), "/tsconfig.json"), "/index.ts"),
            "overlay1"
        );

        change_whole(&project_session, "file:///index.ts", 2, "overlay2");
        let updated = nil_error(update_current_language_server_snapshot(
            &ctx,
            &session,
            &open_projects(&["/tsconfig.json"], None),
        ));
        assert_eq!(
            text(&project_program(&snapshot_of(&session, updated.snapshot), "/tsconfig.json"), "/index.ts"),
            "overlay2"
        );
        session.close();
        project_session.close();
    }
}

// Go: session_requestfilesystem_test.go:185 TestRequestHostMountReadsEditorOverlays
fn request_host_mount_reads_editor_overlays(kind: Kind) {
    let ctx = bg();
    let (project_session, _) = projecttestutil::setup(files(&[("/host/index.ts", "host")]));
    open(&project_session, "file:///host/index.ts", "overlay");
    open(&project_session, "file:///host/new.ts", "new overlay");
    let session = api::new_lsp_session(project_session.clone(), None);

    let base = nil_error(update_current_language_server_snapshot(
        &ctx,
        &session,
        &with_file_system(RequestFileSystem {
            kind,
            files: request_files(&[("/host/index.ts", "request"), ("/host/new.ts", "request")]),
            symlinks: [(
                "/mounted".to_string(),
                RequestSymlink {
                    target: "/host".to_string(),
                    host: true,
                },
            )]
            .into_iter()
            .collect(),
            ..Default::default()
        }),
    ));
    let snapshot = snapshot_of(&session, base.snapshot);
    let (content, ok) = snapshot.read_file("/mounted/index.ts");
    assert!(ok);
    assert_eq!(content, "overlay");
    let (content, ok) = snapshot.read_file("/mounted/new.ts");
    assert!(ok);
    assert_eq!(content, "new overlay");
    session.close();
    project_session.close();
}

// Go: session_requestfilesystem_test.go:189 TestRequestHostMountReadsEditorOverlays/full
child_test! {
    fn request_host_mount_reads_editor_overlays_full() {
        request_host_mount_reads_editor_overlays(Kind::FULL);
    }
}

// Go: session_requestfilesystem_test.go:189 TestRequestHostMountReadsEditorOverlays/layer
child_test! {
    fn request_host_mount_reads_editor_overlays_layer() {
        request_host_mount_reads_editor_overlays(Kind::LAYER);
    }
}

// Go: session_requestfilesystem_test.go:226 TestCreateSnapshotUsesFullFileSystem
child_test! {
    fn create_snapshot_uses_full_file_system() {
        let (project_session, _) = projecttestutil::setup(files(&[("/host.ts", "host")]));
        let session = api::new_lsp_session(project_session.clone(), None);

        let mut response = nil_error(session.handle_create_snapshot(
            &bg(),
            &open_projects(
                &["/tsconfig.json"],
                Some(request_file_system(
                    Kind::FULL,
                    &[
                        (
                            "/tsconfig.json",
                            r#"{ "compilerOptions": { "noLib": true }, "files": ["src/index.ts"] }"#,
                        ),
                        ("/src/index.ts", r#"export const value = "memory";"#),
                        ("/src/other.ts", "export const other = true;"),
                    ],
                )),
            ),
        ));
        assert_eq!(response.projects.len(), 1);
        assert_eq!(response.projects[0].config_file_name.as_deref(), Some("/tsconfig.json"));

        let snapshot = snapshot_of(&session, response.snapshot);
        let (contents, ok) = snapshot.read_file("/src/index.ts");
        assert!(ok);
        assert_eq!(contents, r#"export const value = "memory";"#);
        let (_, ok) = snapshot.read_file("/host.ts");
        assert!(!ok);

        // Carrying the same filesystem forward without a delta must preserve
        // incremental state instead of forcing a full program rebuild.
        let program = project_program(&snapshot, "/tsconfig.json");
        let unchanged = nil_error(session.handle_update_snapshot(&bg(), &update(response.snapshot, None)));
        let unchanged_snapshot = snapshot_of(&session, unchanged.snapshot);
        assert!(Rc::ptr_eq(&project_program(&unchanged_snapshot, "/tsconfig.json"), &program));
        response = unchanged;

        // Supplying a new filesystem replaces inherited snapshot file caches even
        // when the caller does not redundantly list every file in FileChanges.
        response = nil_error(session.handle_update_snapshot(
            &bg(),
            &update(
                response.snapshot,
                Some(ensure_all(request_file_system(
                    Kind::FULL,
                    &[
                        (
                            "/tsconfig.json",
                            r#"{ "compilerOptions": { "noLib": true }, "files": ["src/index.ts", "src/other.ts"] }"#,
                        ),
                        ("/src/index.ts", r#"export const value = "updated";"#),
                        ("/src/other.ts", "export const other = true;"),
                    ],
                ))),
            ),
        ));
        let snapshot = snapshot_of(&session, response.snapshot);
        let (contents, ok) = snapshot.read_file("/src/index.ts");
        assert!(ok);
        assert_eq!(contents, r#"export const value = "updated";"#);

        // A new layer retains the base snapshot's supplied filesystem for every file
        // other than its override.
        let temporary = nil_error(session.handle_update_snapshot(
            &bg(),
            &update(
                response.snapshot,
                Some(with_file_system(request_file_system(
                    Kind::LAYER,
                    &[("/src/index.ts", r#"export const value = "temporary";"#)],
                ))),
            ),
        ));
        let temporary_snapshot = snapshot_of(&session, temporary.snapshot);
        let (contents, ok) = temporary_snapshot.read_file("/src/index.ts");
        assert!(ok);
        assert_eq!(contents, r#"export const value = "temporary";"#);
        let (contents, ok) = temporary_snapshot.read_file("/src/other.ts");
        assert!(ok);
        assert_eq!(contents, "export const other = true;");
        session.close();
        project_session.close();
    }
}

// Go: session_requestfilesystem_test.go:310 TestUpdateSnapshotRequestFileOverridesOpenOverlay
child_test! {
    fn update_snapshot_request_file_overrides_open_overlay() {
        let (project_session, _) = projecttestutil::setup(files(&[("/index.ts", "host")]));
        open(&project_session, "file:///index.ts", "overlay");

        let session = api::new_lsp_session(project_session.clone(), None);
        let response = nil_error(session.handle_create_snapshot(
            &bg(),
            &with_file_system(request_file_system(Kind::LAYER, &[("/index.ts", "request")])),
        ));

        let snapshot = snapshot_of(&session, response.snapshot);
        let (contents, ok) = snapshot.read_file("/index.ts");
        assert!(ok);
        assert_eq!(contents, "request");
        session.close();
        project_session.close();
    }
}

// Go: session_requestfilesystem_test.go:337 TestUpdateSnapshotRequestTombstoneRemovesOpenOverlay
child_test! {
    fn update_snapshot_request_tombstone_removes_open_overlay() {
        let (project_session, _) = projecttestutil::setup(files(&[("/index.ts", "host")]));
        open(&project_session, "file:///index.ts", "overlay");

        let session = api::new_lsp_session(project_session.clone(), None);
        let response = nil_error(session.handle_create_snapshot(
            &bg(),
            &with_file_system(RequestFileSystem {
                kind: Kind::LAYER,
                removed_paths: removed(&["/index.ts"]),
                ..Default::default()
            }),
        ));

        let snapshot = snapshot_of(&session, response.snapshot);
        let (_, ok) = snapshot.read_file("/index.ts");
        assert!(!ok);
        assert!(snapshot.get_default_project(&uri("file:///index.ts")).is_none());
        session.close();
        project_session.close();
    }
}

// Go: session_requestfilesystem_test.go:362 TestUpdateSnapshotRequestTombstoneRemovesHostlessOpenOverlay
child_test! {
    fn update_snapshot_request_tombstone_removes_hostless_open_overlay() {
        let (project_session, _) = projecttestutil::setup(files(&[]));
        open(&project_session, "file:///index.ts", "overlay");

        let session = api::new_lsp_session(project_session.clone(), None);
        let response = nil_error(session.handle_create_snapshot(
            &bg(),
            &with_file_system(RequestFileSystem {
                kind: Kind::LAYER,
                removed_paths: removed(&["/index.ts"]),
                ..Default::default()
            }),
        ));

        let snapshot = snapshot_of(&session, response.snapshot);
        let (_, ok) = snapshot.read_file("/index.ts");
        assert!(!ok);
        assert!(snapshot.get_default_project(&uri("file:///index.ts")).is_none());
        session.close();
        project_session.close();
    }
}

// Go: session_requestfilesystem_test.go:385 TestUpdateSnapshotRequestFileMasksHostlessOpenOverlayDirectory
child_test! {
    fn update_snapshot_request_file_masks_hostless_open_overlay_directory() {
        let (project_session, _) = projecttestutil::setup(files(&[]));
        open(&project_session, "file:///src/index.ts", "overlay");

        let session = api::new_lsp_session(project_session.clone(), None);
        let response = nil_error(session.handle_create_snapshot(
            &bg(),
            &with_file_system(request_file_system(Kind::LAYER, &[("/src", "request")])),
        ));

        let snapshot = snapshot_of(&session, response.snapshot);
        let (contents, ok) = snapshot.read_file("/src");
        assert!(ok);
        assert_eq!(contents, "request");
        let (_, ok) = snapshot.read_file("/src/index.ts");
        assert!(!ok);
        assert!(snapshot.get_default_project(&uri("file:///src/index.ts")).is_none());
        session.close();
        project_session.close();
    }
}

// Go: session_requestfilesystem_test.go:411 TestUpdateSnapshotRequestMaskUpdatesOpenConfiguredProjects
child_test! {
    fn update_snapshot_request_mask_updates_open_configured_projects() {
        let (project_session, _) = projecttestutil::setup(files(&[
            (
                "/tsconfig.json",
                r#"{ "compilerOptions": { "noLib": true }, "files": ["index.ts"] }"#,
            ),
            ("/index.ts", "host"),
        ]));
        open(&project_session, "file:///index.ts", "overlay");

        let session = api::new_lsp_session(project_session.clone(), None);
        let open_tsconfig = || GetCurrentLanguageServerSnapshotParams {
            changes: Some(LanguageServerSnapshotChanges {
                snapshot_request_changes_params: SnapshotRequestChangesParams {
                    open_projects: vec![doc("/tsconfig.json")],
                    ..Default::default()
                },
            }),
            ..Default::default()
        };
        let has_open_tsconfig = |snapshot: &project::Snapshot| {
            snapshot
                .project_collection
                .get_open_configured_projects()
                .contains(&project::ConfiguredProjectID(path("/tsconfig.json")))
        };
        let base = nil_error(session.handle_get_current_language_server_snapshot(&bg(), &open_tsconfig()));
        let base_snapshot = snapshot_of(&session, base.snapshot);
        assert!(has_open_tsconfig(&base_snapshot));

        let masked = nil_error(session.handle_update_snapshot(
            &bg(),
            &update(
                base.snapshot,
                Some(with_file_system(request_file_system(Kind::LAYER, &[("/index.ts", "request")]))),
            ),
        ));
        let masked_snapshot = snapshot_of(&session, masked.snapshot);
        assert!(!has_open_tsconfig(&masked_snapshot));

        let unmasked =
            nil_error(session.handle_get_current_language_server_snapshot(&bg(), &open_tsconfig()));
        let unmasked_snapshot = snapshot_of(&session, unmasked.snapshot);
        assert!(has_open_tsconfig(&unmasked_snapshot));
        session.close();
        project_session.close();
    }
}

// Go: session_requestfilesystem_test.go:457 TestUpdateSnapshotConfigChangeSkipsMaskedOpenOverlay
child_test! {
    fn update_snapshot_config_change_skips_masked_open_overlay() {
        let (project_session, _) = projecttestutil::setup(files(&[("/index.ts", "host")]));
        open(&project_session, "file:///index.ts", "overlay");

        let session = api::new_lsp_session(project_session.clone(), None);
        let base = nil_error(session.handle_create_snapshot(&bg(), &CreateSnapshotParams::default()));

        let updated = nil_error(session.handle_update_snapshot(
            &bg(),
            &update(
                base.snapshot,
                Some(with_file_system(RequestFileSystem {
                    kind: Kind::LAYER,
                    files: request_files(&[(
                        "/tsconfig.json",
                        r#"{ "compilerOptions": { "noLib": true }, "files": ["index.ts"] }"#,
                    )]),
                    removed_paths: removed(&["/index.ts"]),
                    ..Default::default()
                })),
            ),
        ));

        let snapshot = snapshot_of(&session, updated.snapshot);
        let (_, ok) = snapshot.read_file("/index.ts");
        assert!(!ok);
        session.close();
        project_session.close();
    }
}

// Go: session_requestfilesystem_test.go:490 TestCreateProgramRetainsFullFileSystem
child_test! {
    fn create_program_retains_full_file_system() {
        let (project_session, _) = projecttestutil::setup(files(&[]));
        let session = api::new_lsp_session(project_session.clone(), None);

        let ctx = bg();
        let base = nil_error(session.handle_create_snapshot(
            &ctx,
            &CreateSnapshotParams {
                snapshot_request_changes_params: SnapshotRequestChangesParams {
                    open_files: Some(vec![doc("/old.ts")]),
                    ..Default::default()
                },
                file_system: Some(request_file_system(
                    Kind::FULL,
                    &[
                        ("/old.ts", "export const oldValue = 1;"),
                        ("/new.ts", "export const newValue = 2;"),
                    ],
                )),
                ..Default::default()
            },
        ));
        assert_eq!(base.projects.len(), 1);

        let created = nil_error(session.handle_update_snapshot(
            &ctx,
            &update(
                base.snapshot,
                Some(CreateSnapshotParams {
                    snapshot_request_changes_params: SnapshotRequestChangesParams {
                        create_programs: Some(vec![Some(program_params(&["/new.ts"], no_lib()))]),
                        ..Default::default()
                    },
                    ..Default::default()
                }),
            ),
        ));

        let snapshot = nil_error(session.get_snapshot_data(created.snapshot));
        let program = nil_error(snapshot.get_program(
            &created.operation.as_ref().unwrap().created_programs.as_ref().unwrap()[0].as_id(),
        ));
        assert!(program.get_source_file("/new.ts").is_some());
        session.close();
        project_session.close();
    }
}

// Go: session_requestfilesystem_test.go:530 TestSnapshotUpdateFullFileSystemIsTotal
child_test! {
    fn snapshot_update_full_file_system_is_total() {
        let (project_session, _) = projecttestutil::setup(files(&[("/host.ts", "host")]));
        let session = api::new_lsp_session(project_session.clone(), None);

        let base = nil_error(session.handle_create_snapshot(&bg(), &CreateSnapshotParams::default()));
        let replaced = nil_error(session.handle_update_snapshot(
            &bg(),
            &update(
                base.snapshot,
                Some(with_file_system(request_file_system(Kind::FULL, &[("/memory.ts", "memory")]))),
            ),
        ));

        let snapshot = snapshot_of(&session, replaced.snapshot);
        let (contents, ok) = snapshot.read_file("/memory.ts");
        assert!(ok);
        assert_eq!(contents, "memory");
        let (_, ok) = snapshot.read_file("/host.ts");
        assert!(!ok);
        session.close();
        project_session.close();
    }
}

// Go: session_requestfilesystem_test.go:561 TestSnapshotUpdateCarriesHostFileSystemWithoutOverride
child_test! {
    fn snapshot_update_carries_host_file_system_without_override() {
        let (project_session, _) = projecttestutil::setup(files(&[
            (
                "/tsconfig.json",
                r#"{ "compilerOptions": { "noLib": true }, "files": ["index.ts"] }"#,
            ),
            ("/index.ts", "export const value = true;"),
        ]));
        let session = api::new_lsp_session(project_session.clone(), None);

        let base = nil_error(session.handle_create_snapshot(&bg(), &open_projects(&["/tsconfig.json"], None)));
        let base_snapshot = snapshot_of(&session, base.snapshot);
        let program = project_program(&base_snapshot, "/tsconfig.json");

        let updated = nil_error(session.handle_update_snapshot(&bg(), &update(base.snapshot, None)));
        let updated_snapshot = snapshot_of(&session, updated.snapshot);
        assert!(Rc::ptr_eq(&project_program(&updated_snapshot, "/tsconfig.json"), &program));
        session.close();
        project_session.close();
    }
}

// Go: session_requestfilesystem_test.go:585 TestSnapshotFileSystemLayersPreserveIncrementalState
fn snapshot_file_system_layers_preserve_incremental_state(base_kind: Kind) {
    let file_entries = [
        (
            "/a/tsconfig.json",
            r#"{ "compilerOptions": { "noLib": true }, "include": ["**/*.ts"] }"#,
        ),
        ("/a/index.ts", "export const value = 1;"),
        ("/a/removed/nested.ts", "export const nested = true;"),
        ("/a/removed/deep/file.ts", "export const deep = true;"),
        (
            "/b/tsconfig.json",
            r#"{ "compilerOptions": { "noLib": true }, "files": ["index.ts"] }"#,
        ),
        ("/b/index.ts", "export const unrelated = true;"),
    ];
    let a_index = "export const value = 1;";
    let (project_session, _) = projecttestutil::setup(files(&file_entries));
    let session = api::new_lsp_session(project_session.clone(), None);
    let ctx = bg();
    let mut params = open_projects(&["/a/tsconfig.json", "/b/tsconfig.json"], None);
    if base_kind.0 != "host" {
        params.file_system = Some(request_file_system(base_kind.clone(), &file_entries));
    }
    let base = nil_error(session.handle_create_snapshot(&ctx, &params));
    let base_snapshot = snapshot_of(&session, base.snapshot);
    let base_program = project_program(&base_snapshot, "/a/tsconfig.json");
    let unrelated_program = project_program(&base_snapshot, "/b/tsconfig.json");
    let unrelated_file = base_snapshot.get_file("/b/index.ts");

    let unchanged = nil_error(session.handle_update_snapshot(
        &ctx,
        &update(
            base.snapshot,
            Some(ensure_all(request_file_system(
                Kind::LAYER,
                &[("/a/index.ts", a_index)],
            ))),
        ),
    ));
    let unchanged_snapshot = snapshot_of(&session, unchanged.snapshot);
    assert!(Rc::ptr_eq(
        &project_program(&unchanged_snapshot, "/a/tsconfig.json"),
        &base_program
    ));
    assert!(Rc::ptr_eq(
        &project_program(&unchanged_snapshot, "/b/tsconfig.json"),
        &unrelated_program
    ));
    assert!(same_file(
        &unchanged_snapshot.get_file("/b/index.ts"),
        &unrelated_file
    ));

    const UPDATED_TEXT: &str = "export const value = 2;";
    let updated = nil_error(session.handle_update_snapshot(
        &ctx,
        &update(
            unchanged.snapshot,
            Some(ensure_all(request_file_system(
                Kind::LAYER,
                &[("/a/index.ts", UPDATED_TEXT)],
            ))),
        ),
    ));
    let updated_snapshot = snapshot_of(&session, updated.snapshot);
    let updated_project = updated_snapshot
        .project_collection
        .get_project(&project::ID("/a/tsconfig.json".to_string()))
        .unwrap();
    let updated_program = updated_project.borrow().get_program().unwrap();
    assert!(!Rc::ptr_eq(&updated_program, &base_program));
    assert_eq!(
        updated_project.borrow().program_update_kind,
        project::ProgramUpdateKind::CLONED
    );
    assert_eq!(text(&updated_program, "/a/index.ts"), UPDATED_TEXT);
    assert!(Rc::ptr_eq(
        &project_program(&updated_snapshot, "/b/tsconfig.json"),
        &unrelated_program
    ));
    assert!(same_file(
        &updated_snapshot.get_file("/b/index.ts"),
        &unrelated_file
    ));

    let removed_response = nil_error(session.handle_update_snapshot(
        &ctx,
        &update(
            updated.snapshot,
            Some(ensure_all(RequestFileSystem {
                kind: Kind::LAYER,
                removed_paths: removed(&["/a/removed"]),
                ..Default::default()
            })),
        ),
    ));
    let removed_snapshot = snapshot_of(&session, removed_response.snapshot);
    let removed_program = project_program(&removed_snapshot, "/a/tsconfig.json");
    for path in ["/a/removed/nested.ts", "/a/removed/deep/file.ts"] {
        assert!(removed_program.get_source_file(path).is_none(), "{path}");
        assert!(removed_snapshot.get_file(path).is_none(), "{path}");
        assert!(base_program.get_source_file(path).is_some(), "{path}");
    }
    assert!(Rc::ptr_eq(
        &project_program(&removed_snapshot, "/b/tsconfig.json"),
        &unrelated_program
    ));
    assert!(same_file(
        &removed_snapshot.get_file("/b/index.ts"),
        &unrelated_file
    ));

    // A request without a base snapshot returns to the host, so the old
    // layer's changed contents and directory tombstones must not survive.
    let restored = nil_error(session.handle_create_snapshot(
        &ctx,
        &CreateSnapshotParams {
            snapshot_request_changes_params: params.snapshot_request_changes_params.clone(),
            ..Default::default()
        },
    ));
    let restored_snapshot = snapshot_of(&session, restored.snapshot);
    assert!(!restored_snapshot.has_file_system_override());
    let restored_program = project_program(&restored_snapshot, "/a/tsconfig.json");
    assert_eq!(text(&restored_program, "/a/index.ts"), a_index);
    assert!(
        restored_program
            .get_source_file("/a/removed/deep/file.ts")
            .is_some()
    );
    session.close();
    project_session.close();
}

// Go: session_requestfilesystem_test.go:589 TestSnapshotFileSystemLayersPreserveIncrementalState/host
child_test! {
    fn snapshot_file_system_layers_preserve_incremental_state_host() {
        snapshot_file_system_layers_preserve_incremental_state(Kind("host".into()));
    }
}

// Go: session_requestfilesystem_test.go:589 TestSnapshotFileSystemLayersPreserveIncrementalState/full
child_test! {
    fn snapshot_file_system_layers_preserve_incremental_state_full() {
        snapshot_file_system_layers_preserve_incremental_state(Kind::FULL);
    }
}

// Go: session_requestfilesystem_test.go:589 TestSnapshotFileSystemLayersPreserveIncrementalState/layer
child_test! {
    fn snapshot_file_system_layers_preserve_incremental_state_layer() {
        snapshot_file_system_layers_preserve_incremental_state(Kind::LAYER);
    }
}

// Go: session_requestfilesystem_test.go:691 TestSnapshotFileSystemLayerWithoutBaseUpdatesHostState
child_test! {
    fn snapshot_file_system_layer_without_base_updates_host_state() {
        let (project_session, _) = projecttestutil::setup(files(&[
            (
                "/tsconfig.json",
                r#"{ "compilerOptions": { "noLib": true }, "files": ["index.ts"] }"#,
            ),
            ("/index.ts", "export const value = 1;"),
        ]));
        let session = api::new_lsp_session(project_session.clone(), None);
        let ctx = bg();
        nil_error(session.handle_create_snapshot(&ctx, &open_projects(&["/tsconfig.json"], None)));

        const UPDATED_TEXT: &str = "export const value = 2;";
        let updated = nil_error(session.handle_create_snapshot(
            &ctx,
            &open_projects(
                &["/tsconfig.json"],
                Some(request_file_system(Kind::LAYER, &[("/index.ts", UPDATED_TEXT)])),
            ),
        ));
        let snapshot = snapshot_of(&session, updated.snapshot);
        let updated_project = snapshot
            .project_collection
            .get_project(&project::ID("/tsconfig.json".to_string()))
            .unwrap();
        assert_eq!(
            text(&updated_project.borrow().get_program().unwrap(), "/index.ts"),
            UPDATED_TEXT
        );
        assert_eq!(
            updated_project.borrow().program_update_kind,
            project::ProgramUpdateKind::NEW_FILES
        );
        session.close();
        project_session.close();
    }
}

// Go: session_requestfilesystem_test.go:722 TestEmitFromLayerOverFullFileSystemReturnsFileContents
child_test! {
    fn emit_from_layer_over_full_file_system_returns_file_contents() {
        let (project_session, _) = projecttestutil::setup(files(&[]));
        let session = api::new_lsp_session(project_session.clone(), None);
        let ctx = bg();

        let base = nil_error(session.handle_create_snapshot(
            &ctx,
            &open_projects(
                &["/tsconfig.json"],
                Some(request_file_system(
                    Kind::FULL,
                    &[
                        (
                            "/tsconfig.json",
                            r#"{ "compilerOptions": { "noLib": true, "outDir": "/out" }, "files": ["src/main.ts"] }"#,
                        ),
                        ("/src/main.ts", "export const value: number = 1;"),
                    ],
                )),
            ),
        ));
        let layered = nil_error(session.handle_update_snapshot(
            &ctx,
            &update(
                base.snapshot,
                Some(with_file_system(request_file_system(Kind::LAYER, &[]))),
            ),
        ));
        assert_eq!(layered.projects.len(), 0);

        let emit_params = EmitParams {
            snapshot: layered.snapshot,
            project: base.projects[0].id.clone(),
            ..Default::default()
        };
        let emitted = nil_error(session.handle_emit(&ctx, &emit_params));
        assert_eq!(emitted.emitted_files, vec!["/out/src/main.js"]);
        assert_eq!(emitted.emitted_files_contents, vec!["export const value = 1;\n"]);

        release(&session, base.snapshot);
        let emitted_after_release = nil_error(session.handle_emit(&ctx, &emit_params));
        assert_eq!(emitted_after_release.emitted_files, emitted.emitted_files);
        assert_eq!(
            emitted_after_release.emitted_files_contents,
            emitted.emitted_files_contents
        );
        session.close();
        project_session.close();
    }
}

// Go: session_requestfilesystem_test.go:771 TestReleaseSnapshotCompactsSoleLayeredFileSystem
child_test! {
    fn release_snapshot_compacts_sole_layered_file_system() {
        let (project_session, _) = projecttestutil::setup(files(&[("/host.ts", "host")]));
        let session = api::new_lsp_session(project_session.clone(), None);

        let base = nil_error(session.handle_create_snapshot(
            &bg(),
            &with_file_system(request_file_system(
                Kind::FULL,
                &[
                    ("/inherited.ts", "inherited"),
                    ("/changed.ts", "old"),
                    ("/removed.ts", "removed"),
                ],
            )),
        ));
        let base_file_system = file_system_of(&session, base.snapshot);
        assert!(base_file_system.is_some());

        let layered = nil_error(session.handle_update_snapshot(
            &bg(),
            &update(
                base.snapshot,
                Some(with_file_system(RequestFileSystem {
                    kind: Kind::LAYER,
                    files: request_files(&[("/changed.ts", "new"), ("/added.ts", "added")]),
                    removed_paths: removed(&["/removed.ts"]),
                    ..Default::default()
                })),
            ),
        ));
        let layered_snapshot = snapshot_of(&session, layered.snapshot);
        let layered_file_system = file_system_of(&session, layered.snapshot);
        assert!(layered_file_system.is_some());
        assert_eq!(session.snapshots.borrow()[&base.snapshot].ref_count.get(), 1);

        release(&session, base.snapshot);
        assert!(!session.snapshots.borrow().contains_key(&base.snapshot));

        for (path, expected) in [
            ("/inherited.ts", "inherited"),
            ("/changed.ts", "new"),
            ("/added.ts", "added"),
        ] {
            let (contents, read_ok) = layered_snapshot.read_file(path);
            assert!(read_ok, "{path}");
            assert_eq!(contents, expected);
        }
        let (_, ok) = layered_snapshot.read_file("/removed.ts");
        assert!(!ok);
        let (_, ok) = layered_snapshot.read_file("/host.ts");
        assert!(!ok);
        assert!(has_full_file_system(layered_file_system.as_deref()));
        session.close();
        project_session.close();
    }
}

// Go: session_requestfilesystem_test.go:833 TestEagerSnapshotReleaseDoesNotRetainFileSystemHistory
child_test! {
    fn eager_snapshot_release_does_not_retain_file_system_history() {
        let (project_session, _) = projecttestutil::setup(files(&[]));
        let session = api::new_lsp_session(project_session.clone(), None);

        let mut response = nil_error(session.handle_create_snapshot(
            &bg(),
            &with_file_system(request_file_system(Kind::FULL, &[("/pkg/index.ts", "")])),
        ));

        let mut content = String::new();
        for character in "export const x = 1".chars() {
            let old_snapshot = response.snapshot;
            content.push(character);
            response = nil_error(session.handle_update_snapshot(
                &bg(),
                &update(
                    old_snapshot,
                    Some(with_file_system(request_file_system(
                        Kind::LAYER,
                        &[("/pkg/index.ts", &content)],
                    ))),
                ),
            ));
            release(&session, old_snapshot);

            assert_eq!(session.snapshots.borrow().len(), 1);
            let current = session.snapshots.borrow().get(&response.snapshot).cloned();
            assert!(current.is_some());
            let current = current.unwrap();
            assert_eq!(current.ref_count.get(), 1);
            let file_system = current.file_system.clone();
            assert!(file_system.is_some());
            assert!(has_full_file_system(file_system.as_deref()));
            let (actual, ok) = current.snapshot.read_file("/pkg/index.ts");
            assert!(ok);
            assert_eq!(actual, content);
        }
        session.close();
        project_session.close();
    }
}

// Go: session_requestfilesystem_test.go:881 TestSnapshotReleaseCompactsChainedFileSystems
child_test! {
    fn snapshot_release_compacts_chained_file_systems() {
        let (project_session, _) = projecttestutil::setup(files(&[]));
        let session = api::new_lsp_session(project_session.clone(), None);

        let mut responses: Vec<CreateSnapshotResponse> = Vec::with_capacity(4);
        responses.push(nil_error(session.handle_create_snapshot(
            &bg(),
            &with_file_system(request_file_system(Kind::FULL, &[("/pkg/index.ts", "0")])),
        )));
        for i in 1..4 {
            let previous = responses[i - 1].snapshot;
            let content = i.to_string();
            responses.push(nil_error(session.handle_update_snapshot(
                &bg(),
                &update(
                    previous,
                    Some(with_file_system(request_file_system(
                        Kind::LAYER,
                        &[("/pkg/index.ts", &content)],
                    ))),
                ),
            )));
        }

        release(&session, responses[0].snapshot);
        assert!(!session.snapshots.borrow().contains_key(&responses[0].snapshot));

        for (i, response) in responses.iter().enumerate().skip(1) {
            let current = session.snapshots.borrow().get(&response.snapshot).cloned();
            assert!(current.is_some());
            let current = current.unwrap();
            assert_eq!(current.ref_count.get(), 1);
            let file_system = current.file_system.clone();
            assert!(file_system.is_some());
            assert!(has_full_file_system(file_system.as_deref()));
            let (contents, ok) = current.snapshot.read_file("/pkg/index.ts");
            assert!(ok);
            assert_eq!(contents, i.to_string());
        }
        session.close();
        project_session.close();
    }
}

// Go: session_requestfilesystem_test.go:926 TestTemporarySnapshotRetainsLayeredFileSystemHistory
child_test! {
    fn temporary_snapshot_retains_layered_file_system_history() {
        let (project_session, _) = projecttestutil::setup(files(&[]));
        let session = api::new_lsp_session(project_session.clone(), None);

        let base = nil_error(session.handle_create_snapshot(
            &bg(),
            &with_file_system(request_file_system(Kind::FULL, &[("/pkg/index.ts", "base")])),
        ));
        let layered = nil_error(session.handle_update_snapshot(
            &bg(),
            &update(
                base.snapshot,
                Some(with_file_system(request_file_system(
                    Kind::LAYER,
                    &[("/pkg/index.ts", "layered")],
                ))),
            ),
        ));
        let temporary = nil_error(session.handle_update_snapshot(
            &bg(),
            &update(
                layered.snapshot,
                Some(with_file_system(request_file_system(
                    Kind::LAYER,
                    &[("/pkg/index.ts", "temporary")],
                ))),
            ),
        ));

        release(&session, layered.snapshot);
        release(&session, base.snapshot);

        let current = session.snapshots.borrow().get(&temporary.snapshot).cloned();
        assert!(current.is_some());
        let current = current.unwrap();
        let file_system = current.file_system.clone();
        assert!(file_system.is_some());
        assert!(has_full_file_system(file_system.as_deref()));
        let (contents, ok) = current.snapshot.read_file("/pkg/index.ts");
        assert!(ok);
        assert_eq!(contents, "temporary");
        session.close();
        project_session.close();
    }
}

// Go: session_requestfilesystem_test.go:973 TestSnapshotReleaseCompactionSupportsConcurrentReaders
// PORT: the port is one thread, so there is no reader goroutine. The reader's
// checks run on the file system before the release and again after it, on
// the same `fileSystem` value the Go reader holds.
child_test! {
    fn snapshot_release_compaction_supports_concurrent_readers() {
        let (project_session, _) = projecttestutil::setup(files(&[]));
        let session = api::new_lsp_session(project_session.clone(), None);

        let entries: Vec<(String, String)> = (0..1024)
            .map(|index| (format!("/pkg/file{index}.ts"), index.to_string()))
            .collect();
        let base = nil_error(session.handle_create_snapshot(
            &bg(),
            &with_file_system(RequestFileSystem {
                kind: Kind::FULL,
                files: entries.into_iter().collect(),
                ..Default::default()
            }),
        ));
        let layered = nil_error(session.handle_update_snapshot(
            &bg(),
            &update(
                base.snapshot,
                Some(with_file_system(request_file_system(
                    Kind::LAYER,
                    &[("/pkg/file0.ts", "updated")],
                ))),
            ),
        ));
        let file_system = file_system_of(&session, layered.snapshot).unwrap();

        let reader = || -> Result<(), String> {
            let (contents, ok) = file_system.read_file("/pkg/file0.ts");
            if !ok || contents != "updated" {
                return Err(format!("unexpected overridden file: {contents:?}, {ok}"));
            }
            if !file_system.file_exists("/pkg/file1023.ts") {
                return Err("inherited file disappeared".to_string());
            }
            Ok(())
        };
        let before = reader();
        release(&session, base.snapshot);
        let after = reader();
        assert_eq!(before, Ok(()));
        assert_eq!(after, Ok(()));
        session.close();
        project_session.close();
    }
}
