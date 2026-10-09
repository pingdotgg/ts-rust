//! Port of Go `internal/api/session_symbolresponse_test.go` (ts#64518).
//!
//! PORT: Go hands `newSymbolResponse` a bare `*ast.Symbol` and uses a
//! `snapshotData` with no snapshot. Here a symbol is an id in a symbol
//! arena, a snapshot data holds a snapshot, and its registry holds the
//! checker of each symbol. So the tests that need a snapshot data use the
//! snapshot and the API checker of a session with one project
//! (`TestSession`). The file-owned answers read the binder lineage
//! (`program::lineage_for_checker`), as `handle_get_symbol_of_declaration`
//! does. A test file binds outside a program, so each test runs in a child
//! process (`child_test!`).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use ts_goport::api::{
    self, CreateSnapshotParams, GetDefaultProjectForFileParams, SnapshotData, SnapshotID,
    SnapshotRequestChangesParams, SymbolOwnerKind,
};
use ts_goport::ast::ContentMapperSourceFileInfo;
use ts_goport::checker::Checker;
use ts_goport::core::{Node, Symbol};
use ts_goport::flags::{ScriptKind, SymbolFlags};
use ts_goport::frontend::json::json_marshal;
use ts_goport::frontend::parser::{self, ParsedSourceFile, SourceFileParseOptions};
use ts_goport::frontend::tspath::Path;
use ts_goport::{program, project};

use super::api_util::{doc, nil_error};
use super::projecttestutil::{self, files};
use super::util::bg;

const CLASS_TEXT: &str = "export class C { property = 1 }";

// Go: session_symbolresponse_test.go:18 parseAndBind
fn parse_and_bind(file_name: &str, text: &'static str) -> Rc<ParsedSourceFile> {
    let file = parse(file_name, text);
    bind(&file);
    file
}

/// Go `parser.ParseSourceFile` of the TS text `text` named `file_name`.
/// PORT: the parse is recorded (`program::note_parsed_source_file`), so
/// the file keeps its parse record when it is published.
fn parse(file_name: &str, text: &'static str) -> Rc<ParsedSourceFile> {
    let file = Rc::new(parser::parse_source_file(
        &SourceFileParseOptions {
            file_name: file_name.to_string(),
            path: Path(file_name.to_string()),
            ..Default::default()
        },
        text,
        ScriptKind::TS,
    ));
    program::note_parsed_source_file(&file);
    file
}

/// Go `binder.BindSourceFile(file)`.
/// PORT: the file is published with no program, then bound into the
/// binder lineage.
fn bind(file: &ParsedSourceFile) {
    program::publish_parsed_files("/");
    program::bind_file_outside_program(file.root);
}

/// The first statement of `file` (Go `file.Statements.Nodes[0]`).
fn first_statement(file: &ParsedSourceFile) -> Node {
    file.root.statements().get(0)
}

/// The API session whose snapshot and checker the tests use (module
/// comment).
struct TestSession {
    project_session: Rc<project::Session>,
    session: Rc<api::Session>,
    setup: api::CheckerSetup,
}

impl TestSession {
    fn new() -> Self {
        const A_TS: &str = "/home/projects/p/a.ts";
        let (project_session, _) = projecttestutil::setup(files(&[
            ("/home/projects/p/tsconfig.json", "{}"),
            (A_TS, "export {};"),
        ]));
        let session = api::new_lsp_session(project_session.clone(), None);
        let ctx = bg();
        let snapshot = nil_error(session.handle_create_snapshot(
            &ctx,
            &CreateSnapshotParams {
                snapshot_request_changes_params: SnapshotRequestChangesParams {
                    open_files: Some(vec![doc(A_TS)]),
                    ..Default::default()
                },
                ..Default::default()
            },
        ))
        .snapshot;
        let project = nil_error(session.handle_get_default_project_for_file(
            &ctx,
            &GetDefaultProjectForFileParams {
                snapshot,
                file: doc(A_TS),
            },
        ))
        .expect("a default project")
        .id;
        let setup = nil_error(session.setup_checker(&ctx, snapshot, &project));
        Self {
            project_session,
            session,
            setup,
        }
    }

    /// The project's API checker, caught up to the binder lineage, so it
    /// reads the files that the test bound.
    fn checker(&self) -> &Rc<RefCell<Checker>> {
        program::catch_up_checker(&mut self.setup.checker.borrow_mut().symbols);
        &self.setup.checker
    }

    // Go: session_symbolresponse_test.go:29 newTestSnapshotData
    fn new_test_snapshot_data(&self) -> SnapshotData {
        SnapshotData {
            handle: SnapshotID(1),
            snapshot: self.setup.sd.snapshot.clone(),
            file_system: None,
            ref_count: Cell::new(0),
            open_projects: Default::default(),
            open_files: Default::default(),
            symbol_registry: Default::default(),
            symbol_canonical_projects: Default::default(),
            project_registries: Default::default(),
        }
    }

    fn close(self) {
        drop(self.setup);
        self.session.close();
        self.project_session.close();
    }
}

/// Go `project.ID("/tsconfig.json")`, the canonical project of the tests.
fn tsconfig() -> project::ID {
    project::ID("/tsconfig.json".to_string())
}

child_test! {
    // Go: session_symbolresponse_test.go:37 TestFileSymbolResponseSerializesDescriptorOnce
    fn file_symbol_response_serializes_descriptor_once() {
        let source_file = parse_and_bind("/file.ts", "class C { property = 1 }");
        let symbols = program::lineage_for_checker();

        let response =
            api::new_file_symbol_response(&symbols, first_statement(&source_file).symbol());
        let encoded = json_marshal(&response, &[]).unwrap_or_else(|err| panic!("{err:?}"));
        assert_eq!(encoded.matches(r#""contentHash""#).count(), 1, "{encoded}");
    }
}

child_test! {
    // Go: session_symbolresponse_test.go:47 TestFileOwnedSymbolsAreNotRegisteredInSnapshot
    fn file_owned_symbols_are_not_registered_in_snapshot() {
        let source_file = parse_and_bind("/file.ts", CLASS_TEXT);
        let session = TestSession::new();
        let sd = session.new_test_snapshot_data();

        let response = sd
            .new_symbol_response(
                session.checker(),
                first_statement(&source_file).symbol(),
                &tsconfig(),
            )
            .expect("a symbol response");
        assert_eq!(response.reference.kind, SymbolOwnerKind::FILE);
        assert_eq!(sd.symbol_registry.borrow().len(), 0);
        assert_eq!(sd.symbol_canonical_projects.borrow().len(), 0);
        session.close();
    }
}

child_test! {
    // Go: session_symbolresponse_test.go:58 TestTransientSymbolWithFileDeclarationIsSnapshotOwned
    fn transient_symbol_with_file_declaration_is_snapshot_owned() {
        let source_file = parse_and_bind("/file.ts", CLASS_TEXT);
        let class = first_statement(&source_file);
        let session = TestSession::new();
        let checker = session.checker();
        let symbol = checker.borrow_mut().symbols.push_symbol(Symbol {
            flags: SymbolFlags::CLASS | SymbolFlags::TRANSIENT,
            name: "C".into(),
            declarations: vec![class].into(),
            ..Default::default()
        });
        let sd = session.new_test_snapshot_data();

        let response = sd
            .new_symbol_response(checker, symbol, &tsconfig())
            .expect("a symbol response");
        assert_eq!(response.reference.kind, SymbolOwnerKind::SNAPSHOT);
        let (resolved_checker, resolved) =
            nil_error(sd.resolve_symbol_handle(response.reference.id));
        assert!(Rc::ptr_eq(&resolved_checker, checker));
        assert_eq!(resolved, symbol);
        session.close();
    }
}

child_test! {
    // Go: session_symbolresponse_test.go:77 TestSymbolReferencesIdentifyOwnerWithoutDescriptor
    fn symbol_references_identify_owner_without_descriptor() {
        let source_file = parse_and_bind("/file.ts", CLASS_TEXT);
        let symbols = program::lineage_for_checker();
        let class = first_statement(&source_file).symbol();

        let reference = api::new_symbol_reference(&symbols, class).expect("a reference");
        assert_eq!(reference.id, api::symbol_handle(&symbols, class));
        assert_eq!(
            reference.file,
            api::source_file_node_id(source_file.root).to_string()
        );
        let encoded = json_marshal(&reference, &[]).unwrap_or_else(|err| panic!("{err:?}"));
        assert!(!encoded.contains("contentHash"), "{encoded}");

        // A file-owned symbol's relationships are references into the same file.
        let member = symbols.get(symbols.sym(class).members, "property");
        let response = api::new_file_symbol_response(&symbols, member);
        assert_eq!(response.parent, Some(reference));
    }
}

child_test! {
    // Go: session_symbolresponse_test.go:95 TestContentMappedSymbolsAreSnapshotOwned
    fn content_mapped_symbols_are_snapshot_owned() {
        let source_file = parse("/component.vue.ts", CLASS_TEXT);
        source_file.set_content_mapper_info(ContentMapperSourceFileInfo {
            content_mapper: "mapper".to_string(),
            ..Default::default()
        });
        bind(&source_file);
        let class = first_statement(&source_file).symbol();
        let session = TestSession::new();
        let checker = session.checker();
        let sd = session.new_test_snapshot_data();

        let response = sd
            .new_symbol_response(checker, class, &tsconfig())
            .expect("a symbol response");
        assert_eq!(response.reference.kind, SymbolOwnerKind::SNAPSHOT);
        assert!(response.reference.file.is_none());
        assert_eq!(response.reference.snapshot, SnapshotID(1));
        let (_, resolved) = nil_error(sd.resolve_symbol_handle(response.reference.id));
        assert_eq!(resolved, class);

        let reference = {
            let symbols = &checker.borrow().symbols;
            let member = symbols.get(symbols.sym(class).members, "property");
            api::new_symbol_reference(symbols, member).expect("a reference")
        };
        assert_eq!(reference.file, "");
        session.close();
    }
}
