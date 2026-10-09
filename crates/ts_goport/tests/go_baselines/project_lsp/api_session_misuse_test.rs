//! Port-only tests of API requests on input that Go does not expect
//! (optapifuzz1). Go panics, and the API answers the panic text; the port
//! must panic at the same request with the same text. The tests at the end
//! read source text that is not valid UTF-8 (jsdocapi1 open items and
//! residuals), where the answers must hold Go's numbers.
//!
//! PORT: the tests call the session handlers directly, as the Go session
//! tests do, and catch the panic that the server would turn into the
//! `panic: <text>` answer (`handle_batch_request`, `ipc::conn`).

use std::panic::AssertUnwindSafe;
use std::rc::Rc;

use ts_goport::api::requestfilesystem::{Kind, RequestFileSystem, RequestSymlink};
use ts_goport::api::{
    self, CheckerNodeParams, CheckerSymbolParams, CheckerTypeParams, CreateSnapshotParams,
    CreateSourceFileParams, DocumentIdentifier, GetCompletionsAtPositionParams,
    GetContextualTypeForArgumentParams, GetContextualTypeParams,
    GetCurrentLanguageServerSnapshotParams, GetDefaultProjectForFileParams, GetDiagnosticsParams,
    GetSourceFileParams, GetSymbolAtPositionParams, GetTypeAtPositionParams,
    GetTypeFromTypeNodeParams, GetTypePropertyParams, NodeHandle, ParseConfigFileParams,
    ParseJsonConfigFileContentParams, ReadConfigFileParams, SignatureToSignatureDeclarationParams,
    SnapshotID, SnapshotRequestChangesParams, SourceFileResponse, TranspileFromFileParams,
    TypeToTypeNodeParams,
};
use ts_goport::astdata::SyntaxKind;
use ts_goport::flags::ScriptKind;
use ts_goport::frontend::parser::{self, SourceFileParseOptions};
use ts_goport::frontend::tspath::Path;
use ts_goport::gostd::context::Context;
use ts_goport::ls::lsutil;
use ts_goport::lsp::lsproto;
use ts_goport::options::Tristate;
use ts_goport::scanner_util::go_string_from_bytes;
use ts_goport::{program, project};

use super::api_util::{doc, nil_error};
use super::projecttestutil::{self, files};
use super::util::{bg, uri};

/// Go's runtime text for a nil pointer dereference.
const NIL_DEREFERENCE: &str = "runtime error: invalid memory address or nil pointer dereference";

/// An API session with a snapshot that has `file` open, and its project.
struct Api {
    project_session: Rc<project::Session>,
    session: Rc<api::Session>,
    ctx: Context,
    snapshot: SnapshotID,
    project: project::ID,
}

impl Api {
    fn new(entries: &[(&str, &str)], file: &str) -> Self {
        let (project_session, _) = projecttestutil::setup(files(entries));
        let session = api::new_lsp_session(project_session.clone(), None);
        let ctx = bg();
        let snapshot = nil_error(session.handle_create_snapshot(
            &ctx,
            &CreateSnapshotParams {
                snapshot_request_changes_params: SnapshotRequestChangesParams {
                    open_files: Some(vec![doc(file)]),
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
                file: doc(file),
            },
        ))
        .expect("a default project")
        .id;
        Self {
            project_session,
            session,
            ctx,
            snapshot,
            project,
        }
    }

    /// The handle of the SourceFile node of `file`: index 1 of its node
    /// index table.
    fn source_file_handle(file: &str) -> NodeHandle {
        NodeHandle(format!("1.0.{file}"))
    }

    fn node_params(&self, location: NodeHandle) -> CheckerNodeParams {
        CheckerNodeParams {
            snapshot: self.snapshot,
            project: self.project.clone(),
            location,
        }
    }

    fn close(self) {
        self.session.close();
        self.project_session.close();
    }

    /// An API session in an LSP server with the workspace preferences
    /// `preferences` (Go `session.Configure`, as `didChangeConfiguration`
    /// sets them) and the valid UTF-8 file `open` open, on the server's
    /// current snapshot (`getCurrentLanguageServerSnapshot`), with the
    /// default project of `file`. An API snapshot (`createSnapshot`) does
    /// not read the server's preferences.
    fn in_language_server(
        entries: &[(&str, &str)],
        open: (&str, &str),
        file: &str,
        preferences: lsutil::UserPreferences,
    ) -> Self {
        let (project_session, _) = projecttestutil::setup(files(entries));
        project_session.configure(preferences);
        let ctx = bg();
        let open_uri = uri(&format!("file://{}", open.0));
        project_session.did_open_file(
            &ctx,
            &open_uri,
            1,
            open.1,
            &lsproto::LanguageKind::TYPE_SCRIPT,
        );
        // A server request takes the new preferences into the snapshot
        // (Go `flushChanges`); `APIUpdate` drops them.
        nil_error(project_session.get_language_service(&ctx, &open_uri));
        let session = api::new_lsp_session(project_session.clone(), None);
        let snapshot = nil_error(session.handle_get_current_language_server_snapshot(
            &ctx,
            &GetCurrentLanguageServerSnapshotParams::default(),
        ))
        .snapshot;
        let project = nil_error(session.handle_get_default_project_for_file(
            &ctx,
            &GetDefaultProjectForFileParams {
                snapshot,
                file: doc(file),
            },
        ))
        .expect("a default project")
        .id;
        Self {
            project_session,
            session,
            ctx,
            snapshot,
            project,
        }
    }
}

/// The text of the Go panic of `f`.
fn go_panic_text<R>(f: impl FnOnce() -> R) -> String {
    match std::panic::catch_unwind(AssertUnwindSafe(f)) {
        Ok(_) => panic!("no panic"),
        Err(payload) => ts_goport::ipc::conn::recovered_value(payload.as_ref()),
    }
}

const A_TS: &str = "/home/projects/p/a.ts";
const A_TEXT: &str = "export const a = 1;\nexport function f(x: number) { return x; }\nf(1);\n";

fn api_a() -> Api {
    Api::new(
        &[("/home/projects/p/tsconfig.json", "{}"), (A_TS, A_TEXT)],
        A_TS,
    )
}

/// The id of the signature of `f` in `A_TEXT`, after `before` ran.
fn signature_id_of_f(before: impl FnOnce(&Api)) -> String {
    let api = api_a();
    before(&api);
    let f = nil_error(api.session.handle_get_symbol_at_position(
        &api.ctx,
        &GetSymbolAtPositionParams {
            snapshot: api.snapshot,
            project: api.project.clone(),
            file: doc(A_TS),
            position: A_TEXT.find("f(").expect("f") as u32,
        },
    ))
    .expect("the symbol f");
    let declaration = f.declarations[0].clone();
    let signature = nil_error(
        api.session
            .handle_get_signature_from_declaration(&api.ctx, &api.node_params(declaration)),
    )
    .expect("the signature of f");
    api.close();
    format!("{:?}", signature.id)
}

child_test! {
    // optapifuzz1 E1 (d030): Go `declaration.Parameters()` dereferences nil
    // on a node that is not function-like (checker.go:20192). The port read
    // the nil list as empty and made and registered a signature, so the
    // next signature had another id.
    fn get_signature_from_declaration_of_a_node_that_is_not_function_like() {
        let fresh = signature_id_of_f(|_| {});
        let after_panic = signature_id_of_f(|api| {
            let text = go_panic_text(|| {
                api.session.handle_get_signature_from_declaration(
                    &api.ctx,
                    &api.node_params(Api::source_file_handle(A_TS)),
                )
            });
            assert_eq!(text, NIL_DEREFERENCE);
        });
        assert_eq!(after_panic, fresh);
    }
}

child_test! {
    // optapifuzz1 E2 (d026): from the SourceFile, Go's walk in
    // getConditionalFlowTypeOfType reads the nil parent of the SourceFile
    // (checker.go:25424). The port's shortcut for a store with no
    // conditional or mapped type skipped the walk and answered a type.
    fn get_type_from_type_node_of_the_source_file() {
        let api = api_a();
        let text = go_panic_text(|| {
            api.session.handle_get_type_from_type_node(
                &api.ctx,
                &GetTypeFromTypeNodeParams {
                    snapshot: api.snapshot,
                    project: api.project.clone(),
                    location: Api::source_file_handle(A_TS),
                },
            )
        });
        assert_eq!(text, NIL_DEREFERENCE);
        api.close();
    }
}

child_test! {
    // optapifuzz1 E3 (d035): typeToTypeNode with no location builds an
    // import type into node_modules under nodenext, and Go reads the file
    // name of the nil context file (nodebuilderimpl.go:686,
    // compiler/program.go:1744). The port read nil as a file and answered
    // null.
    fn type_to_type_node_of_a_node_modules_type_with_no_location() {
        const DTS: &str = "/home/projects/p/node_modules/pkg/index.d.cts";
        let api = Api::new(
            &[
                (
                    "/home/projects/p/tsconfig.json",
                    r#"{"compilerOptions": {"module": "nodenext", "noEmit": true, "strict": true}, "files": ["node_modules/pkg/index.d.cts", "node_modules/pkg/lib/index.d.ts", "index.ts"]}"#,
                ),
                (
                    "/home/projects/p/node_modules/pkg/package.json",
                    r#"{"name": "pkg", "version": "1.0.0", "main": "index.cjs"}"#,
                ),
                (
                    DTS,
                    "declare const pkg: typeof import(\"./lib/index.js\");\nexport = pkg;\n",
                ),
                (
                    "/home/projects/p/node_modules/pkg/lib/index.d.ts",
                    "declare const configs: { a: number };\nexport { configs };\n",
                ),
                ("/home/projects/p/index.ts", "import * as pkg from 'pkg';\npkg;\n"),
            ],
            "/home/projects/p/index.ts",
        );
        let t = nil_error(api.session.handle_get_type_at_position(
            &api.ctx,
            &GetTypeAtPositionParams {
                snapshot: api.snapshot,
                project: api.project.clone(),
                file: doc(DTS),
                position: 15,
            },
        ))
        .expect("the type of pkg");
        let text = go_panic_text(|| {
            api.session.handle_type_to_type_node(
                &api.ctx,
                &TypeToTypeNodeParams {
                    snapshot: api.snapshot,
                    project: api.project.clone(),
                    type_: t.id,
                    location: NodeHandle(String::new()),
                    flags: 0,
                },
            )
        });
        assert_eq!(text, NIL_DEREFERENCE);
        api.close();
    }
}

/// The handle of the first node of `kind` in `A_TEXT`: its index in the
/// node index table, which depends only on the tree.
fn a_handle_of(kind: SyntaxKind) -> NodeHandle {
    let file = Rc::new(parser::parse_source_file(
        &SourceFileParseOptions {
            file_name: "/index-table/a.ts".to_string(),
            path: Path("/index-table/a.ts".to_string()),
            ..Default::default()
        },
        A_TEXT,
        ScriptKind::TS,
    ));
    program::note_parsed_source_file(&file);
    let table = api::encoder::build_node_index_table(file.root);
    let index = table
        .nodes
        .iter()
        .position(|node| node.is_some() && node.kind() == kind)
        .unwrap_or_else(|| panic!("no {kind:?} in A_TEXT"));
    NodeHandle(format!("{index}.0.{A_TS}"))
}

impl Api {
    fn type_at(&self, position: u32) -> api::TypeID {
        nil_error(self.session.handle_get_type_at_position(
            &self.ctx,
            &GetTypeAtPositionParams {
                snapshot: self.snapshot,
                project: self.project.clone(),
                file: doc(A_TS),
                position,
            },
        ))
        .expect("a type")
        .id
    }

    fn symbol_at(&self, position: u32) -> api::SymbolResponse {
        nil_error(self.session.handle_get_symbol_at_position(
            &self.ctx,
            &GetSymbolAtPositionParams {
                snapshot: self.snapshot,
                project: self.project.clone(),
                file: doc(A_TS),
                position,
            },
        ))
        .expect("a symbol")
    }
}

child_test! {
    // optapifuzz1 D: a type getter on the wrong type kind. A cast to a
    // concrete struct (`t.data.(*ConditionalType)`, types.go:709-727) panics
    // with Go's runtime text. A cast to an embedded struct returns nil, and
    // the caller dereferences it (checker.go:22285 for getTypeArguments).
    // The port panicked with its own text for both.
    fn type_getters_on_the_wrong_type_kind() {
        let api = api_a();
        // The literal type `1` of `a`.
        let t = api.type_at(13);
        let text = go_panic_text(|| {
            api.session.handle_get_check_type_of_type(
                &api.ctx,
                &GetTypePropertyParams {
                    snapshot: api.snapshot,
                    project: api.project.clone(),
                    type_: t,
                },
            )
        });
        assert_eq!(
            text,
            "interface conversion: checker.TypeData is *checker.LiteralType, not *checker.ConditionalType"
        );
        let text = go_panic_text(|| {
            api.session.handle_get_type_arguments(
                &api.ctx,
                &CheckerTypeParams {
                    snapshot: api.snapshot,
                    project: api.project.clone(),
                    type_: t,
                },
            )
        });
        assert_eq!(text, NIL_DEREFERENCE);
        api.close();
    }
}

child_test! {
    // optapifuzz1 F: other requests where Go panics, each with Go's text.
    // F1: getContextualType reads the nil parent of the SourceFile
    // (checker.go:29839). F2: getImmediateAliasedSymbol asserts an alias
    // (checker.go:2197); the port's assert was off in release. F3: argument
    // index -1 (relater.go:1801). F4: a position of 2^31, which Go keeps as
    // an int, past the text in the JSDoc snippet slice (jsdoc_snippet.go:77);
    // the port wrapped it to a negative i32. F5: a kind that is no syntax
    // kind (nodebuilderimpl.go:1972).
    fn misuse_panics_with_go_texts() {
        let api = api_a();
        let text = go_panic_text(|| {
            api.session.handle_get_contextual_type(
                &api.ctx,
                &GetContextualTypeParams {
                    snapshot: api.snapshot,
                    project: api.project.clone(),
                    location: Api::source_file_handle(A_TS),
                },
            )
        });
        assert_eq!(text, NIL_DEREFERENCE, "F1");

        let a = api.symbol_at(13);
        let text = go_panic_text(|| {
            api.session.handle_get_immediate_aliased_symbol(
                &api.ctx,
                &CheckerSymbolParams {
                    snapshot: api.snapshot,
                    project: api.project.clone(),
                    symbol: a.reference.clone(),
                },
            )
        });
        assert_eq!(text, "Debug failure. False expression: Should only get Alias here.", "F2");

        let text = go_panic_text(|| {
            api.session.handle_get_contextual_type_for_argument(
                &api.ctx,
                &GetContextualTypeForArgumentParams {
                    snapshot: api.snapshot,
                    project: api.project.clone(),
                    location: a_handle_of(SyntaxKind::CallExpression),
                    index: -1,
                },
            )
        });
        assert_eq!(text, "runtime error: index out of range [-1]", "F3");

        let text = go_panic_text(|| {
            api.session.handle_get_completions_at_position(
                &api.ctx,
                &GetCompletionsAtPositionParams {
                    snapshot: api.snapshot,
                    project: api.project.clone(),
                    file: doc(A_TS),
                    position: 1 << 31,
                    ..Default::default()
                },
            )
        });
        assert_eq!(
            text,
            format!(
                "runtime error: slice bounds out of range [:2147483648] with length {}",
                A_TEXT.len()
            ),
            "F4"
        );

        let f = api.symbol_at(A_TEXT.find("f(").expect("f") as u32);
        let signature = nil_error(api.session.handle_get_signature_from_declaration(
            &api.ctx,
            &api.node_params(f.declarations[0].clone()),
        ))
        .expect("the signature of f");
        let text = go_panic_text(|| {
            api.session.handle_signature_to_signature_declaration(
                &api.ctx,
                &SignatureToSignatureDeclarationParams {
                    snapshot: api.snapshot,
                    project: api.project.clone(),
                    signature: signature.id,
                    kind: 99999,
                    location: NodeHandle(String::new()),
                    flags: 0,
                },
            )
        });
        assert_eq!(text, "Unhandled kind in signatureToSignatureDeclarationHelper", "F5");
        api.close();
    }
}

child_test! {
    // ts#64159 (api skeptic, bump D wave 2b): Go N' roots each client path
    // with `tspath.ToRootedPath` (rooted_path.go:29), which panics on an
    // empty path (:31) and on a URL path with a query or fragment (:34).
    // The sites: `DocumentIdentifier.ToFileName` (proto.go:357; a request
    // without its file field has the zero identifier), transpileFromFile
    // (session.go:2310), createSourceFile (`ResolveFile`, session.go:2108),
    // parseJsonConfigFileContent's configDirectory (session.go:2026) and the
    // request file system's names (requestfilesystem.go:256,
    // filechanges.go:57, :62, :72). The port read "" as the current
    // directory and kept the URL names.
    fn empty_and_url_suffixed_paths_panic_with_go_texts() {
        const EMPTY: &str = "path must not be empty";
        const URL: &str = "path must not contain a URL query or fragment";
        let api = api_a();
        let named = |file_name: &str| DocumentIdentifier {
            file_name: file_name.to_string(),
            ..Default::default()
        };
        let symbol_at = |file: DocumentIdentifier| {
            go_panic_text(|| {
                api.session.handle_get_symbol_at_position(
                    &api.ctx,
                    &GetSymbolAtPositionParams {
                        snapshot: api.snapshot,
                        project: api.project.clone(),
                        file,
                        position: 1,
                    },
                )
            })
        };
        let metadata = |file: DocumentIdentifier| {
            go_panic_text(|| {
                api.session.handle_get_source_file_metadata(
                    &api.ctx,
                    &GetSourceFileParams {
                        snapshot: api.snapshot,
                        project: api.project.clone(),
                        file,
                    },
                )
            })
        };
        let default_project = |file: DocumentIdentifier| {
            go_panic_text(|| {
                api.session.handle_get_default_project_for_file(
                    &api.ctx,
                    &GetDefaultProjectForFileParams {
                        snapshot: api.snapshot,
                        file,
                    },
                )
            })
        };
        assert_eq!(symbol_at(DocumentIdentifier::default()), EMPTY);
        assert_eq!(metadata(DocumentIdentifier::default()), EMPTY);
        assert_eq!(default_project(DocumentIdentifier::default()), EMPTY);
        let text = go_panic_text(|| {
            api.session
                .handle_read_config_file(&api.ctx, &ReadConfigFileParams::default())
        });
        assert_eq!(text, EMPTY, "readConfigFile");
        let text = go_panic_text(|| {
            api.session
                .handle_parse_config_file(&api.ctx, &ParseConfigFileParams::default())
        });
        assert_eq!(text, EMPTY, "parseConfigFile");
        for file_name in ["file:///a.ts?x", "http://h/a.ts#f"] {
            assert_eq!(metadata(named(file_name)), URL, "{file_name}");
            assert_eq!(default_project(named(file_name)), URL, "{file_name}");
        }

        for (file_name, expected) in [("", EMPTY), ("file:///a.ts?x", URL)] {
            let text = go_panic_text(|| {
                api.session.handle_transpile_from_file(
                    &api.ctx,
                    &TranspileFromFileParams {
                        file_name: file_name.to_string(),
                        ..Default::default()
                    },
                    false,
                )
            });
            assert_eq!(text, expected, "transpileFromFile {file_name:?}");
        }
        let text = go_panic_text(|| {
            api.session.handle_create_source_file(
                &api.ctx,
                &CreateSourceFileParams {
                    file_name: "http://h/a.ts#f".to_string(),
                    source_text: "let a = 1".to_string(),
                    ..Default::default()
                },
            )
        });
        assert_eq!(text, URL, "createSourceFile");
        let text = go_panic_text(|| {
            api.session.handle_parse_json_config_file_content(
                &api.ctx,
                &ParseJsonConfigFileContentParams {
                    config_directory: Some(String::new()),
                    ..Default::default()
                },
            )
        });
        assert_eq!(text, EMPTY, "parseJsonConfigFileContent");

        let file_system = |kind: Kind| RequestFileSystem {
            kind,
            ..Default::default()
        };
        let symlink = |target: &str| RequestSymlink {
            target: target.to_string(),
            host: false,
        };
        let full_with_file = |name: &str| RequestFileSystem {
            files: [(name.to_string(), "x".to_string())].into_iter().collect(),
            ..file_system(Kind::FULL)
        };
        for (label, request, expected) in [
            ("empty file key", full_with_file(""), EMPTY),
            ("URL file key", full_with_file("file:///a.ts?x"), URL),
            (
                "empty directory key",
                RequestFileSystem {
                    directories: [(String::new(), Default::default())].into_iter().collect(),
                    ..file_system(Kind::FULL)
                },
                EMPTY,
            ),
            (
                "empty symlink key",
                RequestFileSystem {
                    symlinks: [(String::new(), symlink("a.ts"))].into_iter().collect(),
                    ..file_system(Kind::LAYER)
                },
                EMPTY,
            ),
            (
                "empty symlink target",
                RequestFileSystem {
                    symlinks: [("l.ts".to_string(), symlink(""))].into_iter().collect(),
                    ..file_system(Kind::LAYER)
                },
                EMPTY,
            ),
            (
                "empty removed path",
                RequestFileSystem {
                    removed_paths: vec![String::new()],
                    ..file_system(Kind::LAYER)
                },
                EMPTY,
            ),
        ] {
            let text = go_panic_text(|| {
                api.session.handle_create_snapshot(
                    &api.ctx,
                    &CreateSnapshotParams {
                        file_system: Some(request),
                        ..Default::default()
                    },
                )
            });
            assert_eq!(text, expected, "createSnapshot {label}");
        }
        api.close();
    }
}

// optapifuzz1 B: a JSON error names Go's type with its package
// (project/project.go:44 `project.SyntheticProjectID`, as in "json: cannot
// unmarshal into Go project.SyntheticProjectID within ..."). The port named
// it without the package.
#[test]
fn project_types_have_go_package_names() {
    use ts_goport::frontend::json_ext::go_type_name;
    assert_eq!(
        go_type_name::<project::SyntheticProjectID>(),
        "project.SyntheticProjectID"
    );
    assert_eq!(go_type_name::<project::ID>(), "project.ID");
}

const B_TS: &str = "/home/projects/p/b.ts";
const B_JS: &str = "/home/projects/p/b.js";

/// An API session with `file` of the Go bytes `text` (in the port form) in
/// a project with the compiler options `options`.
fn api_b(file: &str, options: &str, text: &[u8]) -> Api {
    let text = go_string_from_bytes(text.to_vec());
    Api::new(
        &[
            (
                "/home/projects/p/tsconfig.json",
                &format!("{{\"compilerOptions\":{options}}}"),
            ),
            (file, &text),
        ],
        file,
    )
}

/// Go's panic text of `getCompletionsAtPosition` at `position` of `file`,
/// with the trigger character `trigger`.
fn completions_panic_text(api: &Api, file: &str, position: u32, trigger: Option<&str>) -> String {
    go_panic_text(|| {
        api.session.handle_get_completions_at_position(
            &api.ctx,
            &GetCompletionsAtPositionParams {
                snapshot: api.snapshot,
                project: api.project.clone(),
                file: doc(file),
                position,
                trigger_character: trigger.map(str::to_string),
                ..Default::default()
            },
        )
    })
}

child_test! {
    // jsdocapi1 open item 3 (F4 residual): past the text, with a trigger character
    // that passes Go's trigger check, Go's first read of the position is
    // still the JSDoc snippet slice (jsdoc_snippet.go:77), and "*" reads it
    // in the check (completions.go:3312). The port ran with i32::MAX there.
    // In a text with an invalid byte, the port counted its port bytes in
    // the position and the length. Each text is Go N's answer.
    fn completions_past_the_text_panic_with_go_texts() {
        let api = api_a();
        for trigger in [".", "@", "*"] {
            assert_eq!(
                completions_panic_text(&api, A_TS, 1 << 31, Some(trigger)),
                format!(
                    "runtime error: slice bounds out of range [:2147483648] with length {}",
                    A_TEXT.len()
                ),
                "{trigger}"
            );
        }
        api.close();

        let api = api_b(B_TS, "{}", b"const s = \"\xff\";\n");
        for (position, trigger, bound) in [
            (1000, None, 1000),
            (2147483647, None, 2147483647),
            (2147483647, Some("."), 2147483647),
            (u32::MAX, Some("*"), u32::MAX),
        ] {
            assert_eq!(
                completions_panic_text(&api, B_TS, position, trigger),
                format!("runtime error: slice bounds out of range [:{bound}] with length 15"),
                "{position} {trigger:?}"
            );
        }
        api.close();
    }
}

child_test! {
    // followups12 skeptic problem 1: with JSDoc completions off, Go reads a
    // position past the text first in `text[:position]` of
    // `getWordLengthAndStart` (completions.go:2885). The port indexed past
    // the text there (a Rust panic text), and above i32::MAX it named its
    // own bound, less the port bytes of the invalid byte. Each text is Go
    // N's answer (followups13a probe c5).
    fn completions_past_the_text_with_jsdoc_completions_off_panic_with_go_texts() {
        const O_TS: &str = "/home/projects/p/o.ts";
        const O_TEXT: &str = "export const b = 1;\n";
        let text = go_string_from_bytes(b"const s = \"\xff\";\n".to_vec());
        let api = Api::in_language_server(
            &[
                ("/home/projects/p/tsconfig.json", "{}"),
                (B_TS, &text),
                (O_TS, O_TEXT),
            ],
            (O_TS, O_TEXT),
            B_TS,
            lsutil::UserPreferences {
                enable_js_doc_completions: Tristate::False,
                ..lsutil::new_default_user_preferences()
            },
        );
        for (position, trigger) in [
            (16, None),
            (1000, Some(".")),
            (2147483641, None),
            (2147483647, Some(".")),
            (u32::MAX, Some("@")),
        ] {
            assert_eq!(
                completions_panic_text(&api, B_TS, position, trigger),
                format!("runtime error: slice bounds out of range [:{position}] with length 15"),
                "{position} {trigger:?}"
            );
        }
        api.close();
    }
}

child_test! {
    // jsdocapi1 open item 1: the content hash in the getSourceFile header
    // (bytes 4-19) is Go's xxh3-128 of the Go bytes of the file
    // (project/overlayfs.go:86). The port hashed the port form of an
    // invalid byte, a WTF-8 surrogate and a real U+FDD0. The expected bytes
    // are Go N's.
    fn source_file_hash_is_the_hash_of_the_go_bytes() {
        let api = api_b(B_TS, "{}", b"const s = \"\xff\xed\xa0\x80\xef\xb7\x90\";\n");
        let response = nil_error(api.session.handle_get_source_file(
            &api.ctx,
            &GetSourceFileParams {
                snapshot: api.snapshot,
                project: api.project.clone(),
                file: doc(B_TS),
            },
        ))
        .expect("a source file");
        let data = &response
            .downcast_ref::<SourceFileResponse>()
            .expect("a SourceFileResponse")
            .data;
        let encoded = nil_error(api::base64_std_encoding_decode_string(data));
        let hash: String = encoded[4..20].iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hash, "1751d260c9cc7ac5edbaf1f5e182f05f");
        api.close();
    }
}

child_test! {
    // jsdocapi1 A residual: a JSDoc comment at the end of a JS file whose
    // cut keeps 2 bytes of a real U+FDD0 followed by 1 byte. The cut text
    // position after the first kept byte equals the comment end there, and
    // the port kept it as the comment end. Go's TS1069 (an unexpected token
    // at the first kept byte) ends 1 byte into the U+FDD0. Go N's answer:
    // pos 45, end 46, 1:14 to 1:15.
    fn jsdoc_diagnostic_inside_a_real_fdd0_at_the_cut() {
        let api = api_b(
            B_JS,
            "{\"allowJs\":true,\"checkJs\":true}",
            b"function f(a, b) { return a; }\n/** @template \xef\xb7\x90y",
        );
        let diagnostics = nil_error(api.session.handle_get_semantic_diagnostics(
            &api.ctx,
            &GetDiagnosticsParams {
                snapshot: api.snapshot,
                project: api.project.clone(),
                files: Some(vec![doc(B_JS)]),
            },
        ));
        let found: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.code == 1069)
            .map(|d| {
                let (start, end) = (
                    d.start_position.as_ref().expect("a start position"),
                    d.end_position.as_ref().expect("an end position"),
                );
                (
                    d.pos,
                    d.end,
                    (start.line, start.character),
                    (end.line, end.character),
                )
            })
            .collect();
        assert_eq!(found, [(45, 46, (1, 14), (1, 15))]);
        api.close();
    }
}
