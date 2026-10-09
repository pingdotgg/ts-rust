//! Port-only tests of a diagnostic that an API checker stores for a file
//! version that dies later (apisym1c skeptic problem 1, diagfix1). Go's
//! `DiagnosticsCollection.Add` compares a new diagnostic with the stored
//! ones of the same location key, by file name, position, code and message
//! (ast/diagnostic.go:269 Add, 405 EqualDiagnosticsNoRelatedInfo). The
//! stored `*ast.Diagnostic` keeps its old `*ast.SourceFile` alive, so the
//! compare can read its name. Here the version dies with its snapshot, and
//! the collection reads the name that it kept when it stored the diagnostic.
//!
//! ts#64518 (Go N' api/session.go:777 checkerSetup.resolveSymbolHandle): `h`
//! is a file-owned symbol of x.ts, which only A's program has, so B answers
//! it with the client error "source file is not part of the requested
//! program" and stores no diagnostic (see `api_session_crossproject_test`).
//! The test keeps its name, the death of the version and A's answer.
//!
//! PORT: the tests call the session handlers directly, as
//! `api_session_crossproject_test` does.

use std::rc::Rc;

use ts_goport::api::requestfilesystem::{Kind, RequestFileSystem};
use ts_goport::api::{
    self, CreateSnapshotParams, EnsurePrograms, GetSymbolAtPositionParams, GetTypeOfSymbolParams,
    SnapshotID, SnapshotRequestChangesParams, SymbolReference, TypeToTypeNodeParams,
    UpdateSnapshotParams,
};
use ts_goport::ast::file_version_probe;
use ts_goport::gostd::Context;
use ts_goport::project;

use super::api_util::{doc, error_contains, nil_error, project_program, snapshot_of};
use super::projecttestutil::{self, files};
use super::requestfilesystem_test::files as request_files;
use super::util::{bg, text};

const A_CONFIG: &str = "/home/projects/p/tsconfig.a.json";
const B_CONFIG: &str = "/home/projects/p/tsconfig.b.json";
const X_TS: &str = "/home/projects/p/x.ts";
const B_TS: &str = "/home/projects/p/b.ts";
/// `Missing` names no type in A's or B's program, so each checker that
/// resolves the return type of `h` adds "Cannot find name 'Missing'." at the
/// same location in each version.
const X_TEXT: &str = "export function h(): Missing { return null!; }\n";
const B_TEXT: &str = "export const g = 1;\n";
const H_TYPE: &str = "() => Missing";

/// The x.ts text of version `n`: `X_TEXT` and a line after it, so `h` and
/// its diagnostic keep their positions.
fn x_text(n: u32) -> String {
    format!("{X_TEXT}export const v{n} = {n};\n")
}

/// An API session with A (x.ts) and B (b.ts) open: two projects that share
/// no file. An edit of x.ts keeps B's program and its API checker.
struct Api {
    project_session: Rc<project::Session>,
    session: Rc<api::Session>,
    ctx: Context,
    first: SnapshotID,
    a: project::ID,
    b: project::ID,
}

impl Api {
    fn new() -> Self {
        let options = r#"{ "compilerOptions": { "noLib": true }, "files": "#;
        let (project_session, _) = projecttestutil::setup(files(&[
            (A_CONFIG, &format!(r#"{options}["x.ts"] }}"#)),
            (B_CONFIG, &format!(r#"{options}["b.ts"] }}"#)),
            (X_TS, X_TEXT),
            (B_TS, B_TEXT),
        ]));
        let session = api::new_lsp_session(project_session.clone(), None);
        let ctx = bg();
        let created = nil_error(session.handle_create_snapshot(
            &ctx,
            &CreateSnapshotParams {
                snapshot_request_changes_params: SnapshotRequestChangesParams {
                    open_projects: vec![doc(A_CONFIG), doc(B_CONFIG)],
                    ..Default::default()
                },
                ..Default::default()
            },
        ));
        let id = |config: &str| {
            created
                .projects
                .iter()
                .find(|p| p.config_file_name == config)
                .unwrap_or_else(|| panic!("no project {config}"))
                .id
                .clone()
        };
        let (a, b) = (id(A_CONFIG), id(B_CONFIG));
        Self {
            project_session,
            session,
            ctx,
            first: created.snapshot,
            a,
            b,
        }
    }

    /// Go `updateSnapshot` of `snapshot` with a layer that gives x.ts the
    /// text `x`, and `ensurePrograms.all`. A gets a new program with a new
    /// x.ts version; B keeps its program.
    fn edit(&self, snapshot: SnapshotID, x: &str) -> SnapshotID {
        let b_before = project_program(&snapshot_of(&self.session, snapshot), &self.b.0);
        let next = nil_error(self.session.handle_update_snapshot(
            &self.ctx,
            &UpdateSnapshotParams {
                snapshot,
                changes: Some(CreateSnapshotParams {
                    snapshot_request_changes_params: SnapshotRequestChangesParams {
                        ensure_programs: Some(EnsurePrograms {
                            all: true,
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                    file_system: Some(RequestFileSystem {
                        kind: Kind::LAYER,
                        files: request_files(&[(X_TS, x)]),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
            },
        ))
        .snapshot;
        let next_snapshot = snapshot_of(&self.session, next);
        assert_eq!(text(&project_program(&next_snapshot, &self.a.0), X_TS), x);
        assert!(Rc::ptr_eq(
            &project_program(&next_snapshot, &self.b.0),
            &b_before
        ));
        next
    }

    /// The symbol at the first `name` in x.ts text `x`, on A in `snapshot`.
    fn symbol(&self, snapshot: SnapshotID, x: &str, name: &str) -> SymbolReference {
        nil_error(self.session.handle_get_symbol_at_position(
            &self.ctx,
            &GetSymbolAtPositionParams {
                snapshot,
                project: self.a.clone(),
                file: doc(X_TS),
                position: x.find(name).expect("the name") as u32,
            },
        ))
        .expect("a symbol")
        .reference
    }

    /// Go `typeToString(getTypeOfSymbol(symbol))` on `project` in
    /// `snapshot`.
    /// ts#64518: `project` in `snapshot` has no file that owns `symbol`.
    fn rejects(&self, snapshot: SnapshotID, project: &project::ID, symbol: &SymbolReference) {
        error_contains(
            self.session.handle_get_type_of_symbol(
                &self.ctx,
                &GetTypeOfSymbolParams {
                    snapshot,
                    project: project.clone(),
                    symbol: symbol.clone(),
                },
            ),
            "source file is not part of the requested program",
        );
    }

    fn type_text(
        &self,
        snapshot: SnapshotID,
        project: &project::ID,
        symbol: &SymbolReference,
    ) -> String {
        let t = nil_error(self.session.handle_get_type_of_symbol(
            &self.ctx,
            &GetTypeOfSymbolParams {
                snapshot,
                project: project.clone(),
                symbol: symbol.clone(),
            },
        ))
        .expect("a type");
        let text = nil_error(self.session.handle_type_to_string(
            &self.ctx,
            &TypeToTypeNodeParams {
                snapshot,
                project: project.clone(),
                type_: t.id,
                location: Default::default(),
                flags: 0,
            },
        ))
        .expect("a text");
        text.downcast_ref::<String>().expect("a string").clone()
    }

    /// The number of diagnostics that B's API checker stores.
    fn b_diagnostic_count(&self, snapshot: SnapshotID) -> i32 {
        let setup = nil_error(self.session.setup_checker(&self.ctx, snapshot, &self.b));
        setup.checker.borrow().diagnostics.count
    }

    fn close(self) {
        self.session.close();
        self.project_session.close();
    }
}

child_test! {
    // k3-pre: B's API checker is made after x.ts versions 2 and 3 are bound.
    // It resolves the return type of `h` of version 2 and stores the
    // "Cannot find name 'Missing'." diagnostic of that version. The release
    // of the snapshot of version 2 frees the version. B then resolves `h` of
    // version 3: its diagnostic has the same location key, so `add`
    // compares the two. R175 read the file name of the dead version there
    // and panicked ("file version N is released"), and the next answer was
    // `() => any`. Go answers the type each time, and `Add` returns the
    // stored diagnostic of version 2 for the equal one of version 3, so B
    // stores no new diagnostic. ts#64518: B now rejects `h` (module header),
    // so it stores none; the version still dies with its snapshot.
    fn api_checker_diagnostic_of_released_file_version() {
        let api = Api::new();
        let (x2, x3) = (x_text(2), x_text(3));
        let s2 = api.edit(api.first, &x2);
        let h2 = api.symbol(s2, &x2, "h(");
        let s3 = api.edit(s2, &x3);
        let h3 = api.symbol(s3, &x3, "h(");
        // The global errors of `noLib`, which B's checker adds when it is
        // made.
        let before = api.b_diagnostic_count(s2);
        // ts#64518: B rejects the file-owned `h` (module header) and stores
        // no diagnostic.
        api.rejects(s2, &api.b, &h2);
        let stored = api.b_diagnostic_count(s2);
        assert_eq!(stored, before);
        let version = {
            let program = project_program(&snapshot_of(&api.session, s2), &api.a.0);
            let file = program.get_source_file(X_TS).expect("x.ts");
            file_version_probe(file.root).expect("x.ts version 2 is freeable")
        };
        nil_error(api.session.release_snapshot(s2));
        assert!(version.is_freed(), "x.ts version 2 dies with its snapshot");
        for _ in 0..2 {
            api.rejects(s3, &api.b, &h3);
        }
        assert_eq!(api.b_diagnostic_count(s3), stored);
        assert_eq!(api.type_text(s3, &api.a, &h3), H_TYPE);
        api.close();
    }
}
