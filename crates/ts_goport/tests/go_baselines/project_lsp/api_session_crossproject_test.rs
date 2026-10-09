//! Port-only tests of a symbol of project A used in a request on project B
//! (editfuzz4 triage G1, apisym1). Go hands the `*ast.Symbol` to B's checker
//! (api/session.go:2439 handleGetTypeOfSymbol), which reads the bound
//! declarations of any file. A port checker reads the binder lineage ids of
//! its copy. B's API checker copies the lineage when it is made
//! (`ls_program::new_api_checker`), and catches up to the lineage before a
//! symbol of another checker enters it (`api::checker_symbol`, apisym1c). So
//! it gets Go's answer also for a file version bound after it was made, and
//! B's next answers do not change.
//!
//! ts#64518 (Go N' api/session.go:777 checkerSetup.resolveSymbolHandle): a
//! binder symbol is now file-owned. Its reference names its source file,
//! and a checker resolves it only through a file of its own program, so B
//! answers a symbol of x.ts or m.d.ts (files of A only) with the client
//! error "source file is not part of the requested program". The tests keep
//! their names and check that error on B and Go's answers on A. Symbol
//! tables of a file-owned symbol need no project (Go
//! resolveSymbolTablePropertyOfSymbol sorts them by declaration).
//!
//! PORT: the tests call the session handlers directly.

use std::rc::Rc;

use ts_goport::api::requestfilesystem::{Kind, RequestFileSystem};
use ts_goport::api::{
    self, CheckerSymbolParams, CreateSnapshotParams, EnsurePrograms, GetSymbolAtPositionParams,
    GetSymbolOfSourceFileParams, GetSymbolPropertyParams, GetTypeOfSymbolParams, SnapshotID,
    SnapshotRequestChangesParams, SymbolOwnerKind, SymbolReference, TypeToTypeNodeParams,
    UpdateSnapshotParams,
};
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
const M_TS: &str = "/home/projects/p/m.d.ts";
const X_TEXT: &str = "export const f = <T,>(x: T, y: string) => x;\n";
/// The edit that `layer` gives x.ts. `f` keeps its type.
const X_EDITED: &str =
    "// edited\nexport const f = <T,>(x: T, y: string) => x;\nexport class K { #p = 1; q = 2; }\n";
const M_TEXT: &str = "declare module \"m\" { export const v: number; }\n";
/// The edit that `layer` gives m.d.ts.
const M_EDITED: &str = "// edited\ndeclare module \"m\" { export const v: number; }\n";
const B_TEXT: &str = "export const g = (n: number) => n;\n";
const F_TYPE: &str = "<T>(x: T, y: string) => T";
const G_TYPE: &str = "(n: number) => number";

/// An API session with A (x.ts, m.d.ts) and B (b.ts) open: two projects
/// that share no file. B never reads x.ts or m.d.ts, so an edit of them
/// keeps B's program and its API checker.
struct Api {
    project_session: Rc<project::Session>,
    session: Rc<api::Session>,
    ctx: Context,
    snapshot: SnapshotID,
    a: project::ID,
    b: project::ID,
}

impl Api {
    fn new() -> Self {
        let options = r#"{ "compilerOptions": { "noLib": true }, "files": "#;
        let (project_session, _) = projecttestutil::setup(files(&[
            (A_CONFIG, &format!(r#"{options}["x.ts", "m.d.ts"] }}"#)),
            (B_CONFIG, &format!(r#"{options}["b.ts"] }}"#)),
            (X_TS, X_TEXT),
            (M_TS, M_TEXT),
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
                .find(|p| p.config_file_name.as_deref() == Some(config))
                .unwrap_or_else(|| panic!("no project {config}"))
                .id
                .clone()
        };
        let (a, b) = (id(A_CONFIG), id(B_CONFIG));
        Self {
            project_session,
            session,
            ctx,
            snapshot: created.snapshot,
            a,
            b,
        }
    }

    /// Go `updateSnapshot` with a layer that gives x.ts the text `X_EDITED`
    /// and m.d.ts `M_EDITED`, and `ensurePrograms.all`. A gets a new program
    /// with new versions of both; B keeps its program.
    fn edit_x(&mut self) {
        self.edit(&[(X_TS, X_EDITED), (M_TS, M_EDITED)]);
    }

    /// `edit_x` with x.ts text `text` only, then Go `release` of the snapshot
    /// before, so the x.ts version before dies.
    fn edit_x_and_release(&mut self, text: &str) {
        let before = self.snapshot;
        self.edit(&[(X_TS, text)]);
        nil_error(self.session.release_snapshot(before));
    }

    /// `updateSnapshot` with a layer of `layer` and `ensurePrograms.all`.
    fn edit(&mut self, layer: &[(&str, &str)]) {
        let b_before = project_program(&snapshot_of(&self.session, self.snapshot), &self.b.0);
        self.snapshot = nil_error(self.session.handle_update_snapshot(
            &self.ctx,
            &UpdateSnapshotParams {
                snapshot: self.snapshot,
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
                        files: request_files(layer),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
            },
        ))
        .snapshot;
        let snapshot = snapshot_of(&self.session, self.snapshot);
        assert_eq!(
            text(&project_program(&snapshot, &self.a.0), X_TS),
            layer[0].1
        );
        assert!(Rc::ptr_eq(
            &project_program(&snapshot, &self.b.0),
            &b_before
        ));
    }

    /// The symbol at the first `name` in `file`, on `project`.
    fn symbol(&self, project: &project::ID, file: &str, text: &str, name: &str) -> SymbolReference {
        nil_error(self.session.handle_get_symbol_at_position(
            &self.ctx,
            &GetSymbolAtPositionParams {
                snapshot: self.snapshot,
                project: project.clone(),
                file: doc(file),
                position: text.find(name).expect("the name") as u32,
            },
        ))
        .expect("a symbol")
        .reference
    }

    /// Go `getTypeOfSymbol(symbol)` on `project`.
    fn type_of(
        &self,
        project: &project::ID,
        symbol: &SymbolReference,
    ) -> Result<Option<api::TypeResponse>, ts_goport::gostd::GoError> {
        self.session.handle_get_type_of_symbol(
            &self.ctx,
            &GetTypeOfSymbolParams {
                snapshot: self.snapshot,
                project: project.clone(),
                symbol: symbol.clone(),
            },
        )
    }

    /// ts#64518: `project` has no file that owns `symbol`.
    fn rejects(&self, project: &project::ID, symbol: &SymbolReference) {
        error_contains(
            self.type_of(project, symbol),
            "source file is not part of the requested program",
        );
    }

    /// Go `typeToString(getTypeOfSymbol(symbol))` on `project`.
    fn type_text(&self, project: &project::ID, symbol: &SymbolReference) -> String {
        let t = nil_error(self.type_of(project, symbol)).expect("a type");
        let text = nil_error(self.session.handle_type_to_string(
            &self.ctx,
            &TypeToTypeNodeParams {
                snapshot: self.snapshot,
                project: project.clone(),
                type_: t.id,
                location: Default::default(),
                flags: 0,
            },
        ))
        .expect("a text");
        text.downcast_ref::<String>().expect("a string").clone()
    }

    /// Go `getFullyQualifiedName(symbol)` on `project`.
    fn qualified_name(
        &self,
        project: &project::ID,
        symbol: &SymbolReference,
    ) -> Result<String, ts_goport::gostd::GoError> {
        self.session.handle_get_fully_qualified_name(
            &self.ctx,
            &CheckerSymbolParams {
                snapshot: self.snapshot,
                project: project.clone(),
                symbol: symbol.clone(),
            },
        )
    }

    /// The symbol of source file `file`, on `project`.
    fn file_symbol(&self, project: &project::ID, file: &str) -> SymbolReference {
        nil_error(self.session.handle_get_symbol_of_source_file(
            &self.ctx,
            &GetSymbolOfSourceFileParams {
                snapshot: self.snapshot,
                project: project.clone(),
                file: doc(file),
            },
        ))
        .expect("a symbol")
        .reference
    }

    /// The (reference, name) of each answer of Go `getExportsOfSymbol`
    /// (`exports`) or `getMembersOfSymbol` of `symbol`.
    fn table(&self, symbol: &SymbolReference, exports: bool) -> Vec<(SymbolReference, String)> {
        let params = GetSymbolPropertyParams {
            symbol: symbol.clone(),
        };
        let answers = if exports {
            self.session
                .handle_get_exports_of_symbol(&self.ctx, &params)
        } else {
            self.session
                .handle_get_members_of_symbol(&self.ctx, &params)
        };
        nil_error(answers)
            .into_iter()
            .map(|answer| {
                let answer = answer.expect("a symbol");
                (answer.reference, answer.name)
            })
            .collect()
    }

    /// The symbol and table chunks that B's API checker holds
    /// (`SymbolArena::live_chunk_count`).
    fn b_live_chunks(&self) -> usize {
        let setup = nil_error(
            self.session
                .setup_checker(&self.ctx, self.snapshot, &self.b),
        );
        setup.checker.borrow().symbols.live_chunk_count()
    }

    fn close(self) {
        self.session.close();
        self.project_session.close();
    }
}

child_test! {
    // r-xc-min, g1-b-order: B's program bound before the x.ts version that A
    // reads, and B's API checker is made after it. The port read the
    // declarations of `f` past B's copy of the lineage and panicked
    // (index out of bounds), then answered `any`. Go answers the type.
    // ts#64518: B rejects the file-owned `f` (module header).
    fn symbol_of_later_file_version_on_new_api_checker() {
        let mut api = Api::new();
        api.edit_x();
        let f = api.symbol(&api.a, X_TS, X_EDITED, "f =");
        assert_eq!(f.kind, SymbolOwnerKind::FILE);
        api.rejects(&api.b, &f);
        api.rejects(&api.b, &f);
        assert_eq!(api.type_text(&api.a, &f), F_TYPE);
        api.close();
    }
}

child_test! {
    // g1-b-alias, g1-z-alias3, r-kl-min (apisym1c): B's API checker exists
    // before the edit, so its copy of the lineage lacks the new x.ts
    // version. Go answers the type. The port catches B up to the lineage
    // and answers the type too, each time, and B and A answer as before.
    fn symbol_of_later_file_version_on_older_api_checker() {
        let mut api = Api::new();
        let g = api.symbol(&api.b, B_TS, B_TEXT, "g =");
        assert_eq!(api.type_text(&api.b, &g), G_TYPE);
        api.edit_x();
        let f = api.symbol(&api.a, X_TS, X_EDITED, "f =");
        // ts#64518: B rejects the file-owned `f` (module header).
        for _ in 0..2 {
            api.rejects(&api.b, &f);
        }
        let g = api.symbol(&api.b, B_TS, B_TEXT, "g =");
        assert_eq!(api.type_text(&api.b, &g), G_TYPE);
        assert_eq!(api.type_text(&api.a, &f), F_TYPE);
        api.close();
    }
}

child_test! {
    // sk-h-late, sk-fx-lib-late, b-fx-names (apisym1 round b, apisym1c): with
    // B's API checker older than the versions, getFullyQualifiedName,
    // getExportsOfSymbol and getMembersOfSymbol on B give Go's answer. So do
    // the type requests after them, and the name of an ambient module, which
    // reads its file symbol.
    fn names_of_later_file_version_on_older_api_checker() {
        let mut api = Api::new();
        let g = api.symbol(&api.b, B_TS, B_TEXT, "g =");
        assert_eq!(api.type_text(&api.b, &g), G_TYPE);
        api.edit_x();
        let f = api.symbol(&api.a, X_TS, X_EDITED, "f =");
        // ts#64518: B rejects the file-owned `f` (module header).
        error_contains(
            api.qualified_name(&api.b, &f),
            "source file is not part of the requested program",
        );
        assert_eq!(
            nil_error(api.qualified_name(&api.a, &f)),
            r#""/home/projects/p/x".f"#
        );
        let x = api.file_symbol(&api.a, X_TS);
        let exports = api.table(&x, true);
        let names: Vec<_> = exports.iter().map(|(_, name)| name.as_str()).collect();
        assert_eq!(names, ["f", "K"]);
        // ts#64518: file-owned answers name their file, not a project.
        for (reference, _) in &exports {
            assert_eq!(reference.kind, SymbolOwnerKind::FILE);
            assert!(reference.project.0.is_empty() && reference.file.is_some());
        }
        let k = exports[1].0.clone();
        let members = api.table(&k, false);
        assert_eq!(members[1].1, "q");
        // The name of `#p` holds the id of its class `K`, as on A.
        assert!(members[0].1.ends_with(&format!("#{}@#p", k.id.0)), "{}", members[0].1);
        assert_eq!(api.table(&k, false), members);
        api.rejects(&api.b, &f);
        api.rejects(&api.b, &members[1].0);
        let v = api.symbol(&api.a, M_TS, M_EDITED, "v:");
        error_contains(
            api.qualified_name(&api.b, &v),
            "source file is not part of the requested program",
        );
        assert_eq!(nil_error(api.qualified_name(&api.a, &v)), r#""m".v"#);
        assert_eq!(api.type_text(&api.a, &f), F_TYPE);
        assert_eq!(api.type_text(&api.a, &members[1].0), "number");
        let g = api.symbol(&api.b, B_TS, B_TEXT, "g =");
        assert_eq!(api.type_text(&api.b, &g), G_TYPE);
        api.close();
    }
}

child_test! {
    // apisym1c: an edit loop with a request on B's older API checker after
    // each edit. B catches up to each new x.ts version, and frees the
    // versions that died with the released snapshots, so the chunks that
    // it holds do not grow with the edits (it gained 2 or more per edit
    // without the frees). ts#64518: B now rejects the file-owned `f`, so
    // it does not catch up, and its chunks still do not grow.
    fn older_api_checker_frees_dead_versions_when_it_catches_up() {
        let mut api = Api::new();
        let g = api.symbol(&api.b, B_TS, B_TEXT, "g =");
        assert_eq!(api.type_text(&api.b, &g), G_TYPE);
        let mut chunks = Vec::new();
        for n in 0..30 {
            let text = format!("{X_EDITED}// {n}\n");
            api.edit_x_and_release(&text);
            let f = api.symbol(&api.a, X_TS, &text, "f =");
            // ts#64518: B rejects the file-owned `f` (module header).
            api.rejects(&api.b, &f);
            chunks.push(api.b_live_chunks());
        }
        assert!(chunks[29] <= chunks[4] + 4, "{chunks:?}");
        api.close();
    }
}
