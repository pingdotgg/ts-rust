//! Port of Go `internal/lsp/server_completion_test.go`.
//!
//! PORT: Go answers `workspace/configuration` with `[]any{prefs}` and sends
//! `Settings: map[string]any{"typescript": prefs}` where `prefs` is a
//! `*lsutil.UserPreferences`. `lsutil.ParseUserPreferences` matches only a
//! `map[string]any` or a `UserPreferences` value, so Go ignores these
//! pointers and the server keeps its default preferences. The port sends
//! `null` in their place, which the Rust parser also ignores.

use std::sync::Arc;

use ts_goport::frontend::json_ext::LspAny;
use ts_goport::ls::lsconv;
use ts_goport::lsp::lsproto;

use super::lsptestutil::{self, LspClient, result_response};
use super::projecttestutil::files;

// Go: server_completion_test.go:19 initCompletionClient
pub(super) fn init_completion_client(cwd: &str, entries: &[(&str, &str)]) -> LspClient {
    let on_server_request: lsptestutil::ServerRequestHandler = Arc::new(|req| {
        if req.method == lsproto::Method::WORKSPACE_CONFIGURATION {
            return Some(result_response(req, Box::new(vec![LspAny::Null])));
        }
        if req.method == lsproto::Method::CLIENT_REGISTER_CAPABILITY
            || req.method == lsproto::Method::CLIENT_UNREGISTER_CAPABILITY
        {
            return Some(result_response(req, Box::new(lsproto::Null)));
        }
        None
    });

    let client = lsptestutil::new_lsp_client(
        lsptestutil::server_setup(cwd, files(entries)),
        Some(on_server_request),
        None,
    );

    let (init_msg, _) = client.send_request(
        &lsproto::INITIALIZE_INFO,
        lsproto::InitializeParams {
            capabilities: Some(lsproto::ClientCapabilities::default()),
            ..Default::default()
        },
    );
    assert!(init_msg.error.is_none(), "Initialize failed");
    client.send_notification(
        &lsproto::INITIALIZED_INFO,
        lsproto::InitializedParams::default(),
    );

    let mut settings = indexmap::IndexMap::new();
    settings.insert("typescript".to_string(), LspAny::Null);
    client.send_notification(
        &lsproto::WORKSPACE_DID_CHANGE_CONFIGURATION_INFO,
        lsproto::DidChangeConfigurationParams {
            settings: LspAny::Object(settings),
        },
    );

    client
}

// Go: server_completion_test.go:65 completionItems
fn completion_items(resp: Option<lsproto::CompletionResponse>) -> Vec<lsproto::CompletionItem> {
    let Some(resp) = resp else {
        return Vec::new();
    };
    if let Some(list) = resp.list {
        return list.items;
    }
    resp.items.unwrap_or_default()
}

// Go: server_completion_test.go:75 findCompletionItem
fn find_completion_item<'a>(
    items: &'a [lsproto::CompletionItem],
    label: &str,
) -> Option<&'a lsproto::CompletionItem> {
    items.iter().find(|item| item.label == label)
}

fn open(client: &LspClient, u: &lsproto::DocumentUri, text: &str) {
    client.send_notification(
        &lsproto::TEXT_DOCUMENT_DID_OPEN_INFO,
        lsproto::DidOpenTextDocumentParams {
            text_document: Some(lsproto::TextDocumentItem {
                uri: u.clone(),
                language_id: lsproto::LanguageKind::TYPE_SCRIPT,
                text: text.to_string(),
                ..Default::default()
            }),
        },
    );
}

fn close(client: &LspClient, u: &lsproto::DocumentUri) {
    client.send_notification(
        &lsproto::TEXT_DOCUMENT_DID_CLOSE_INFO,
        lsproto::DidCloseTextDocumentParams {
            text_document: lsproto::TextDocumentIdentifier { uri: u.clone() },
        },
    );
}

fn completion_params(
    u: &lsproto::DocumentUri,
    line: u32,
    character: u32,
) -> lsproto::CompletionParams {
    lsproto::CompletionParams {
        text_document: lsproto::TextDocumentIdentifier { uri: u.clone() },
        position: lsproto::Position { line, character },
        context: Some(lsproto::CompletionContext::default()),
        ..Default::default()
    }
}

/// The checks of the auto-import completion subtests: `someVar` with an
/// auto-import fix from "./a".
fn assert_some_var_auto_import(
    msg: &lsproto::ResponseMessage,
    resp: Option<lsproto::CompletionResponse>,
    message: &str,
) {
    assert!(msg.error.is_none(), "{:?}", msg.error);
    let items = completion_items(resp);
    let item = find_completion_item(&items, "someVar").unwrap_or_else(|| panic!("{message}"));
    let auto_import = item
        .data
        .as_ref()
        .and_then(|data| data.auto_import.as_ref())
        .expect("item.Data.AutoImport");
    assert_eq!(auto_import.module_specifier, "./a");
}

const TSCONFIG: &str = r#"{"compilerOptions": {"module": "esnext", "target": "esnext"}}"#;

child_test! {
    // Go: server_completion_test.go:86 TestCompletionAfterFileClose
    fn completion_after_file_close() {
        let client = init_completion_client(
            "/home/projects",
            &[
                ("/home/projects/tsconfig.json", TSCONFIG),
                ("/home/projects/a.ts", "export const someVar = 10;"),
                ("/home/projects/b.ts", "s"),
            ],
        );

        let a_uri = lsconv::file_name_to_document_uri("/home/projects/a.ts");
        let b_uri = lsconv::file_name_to_document_uri("/home/projects/b.ts");
        open(&client, &a_uri, "export const someVar = 10;");
        open(&client, &b_uri, "s");

        close(&client, &b_uri);

        let (msg, resp) = client.send_request(&lsproto::TEXT_DOCUMENT_COMPLETION_INFO, completion_params(&b_uri, 0, 1));
        assert_some_var_auto_import(&msg, resp, "someVar");
    }
}

child_test! {
    // Go: server_completion_test.go:131 TestCompletionWithConcurrentFileClose
    fn completion_with_concurrent_file_close() {
        let client = init_completion_client(
            "/home/projects",
            &[
                ("/home/projects/tsconfig.json", TSCONFIG),
                ("/home/projects/a.ts", "export const someVar = 10;"),
                ("/home/projects/b.ts", "s"),
            ],
        );

        let a_uri = lsconv::file_name_to_document_uri("/home/projects/a.ts");
        let b_uri = lsconv::file_name_to_document_uri("/home/projects/b.ts");
        open(&client, &a_uri, "export const someVar = 10;");
        open(&client, &b_uri, "s");

        let wait_for_completion =
            client.send_request_async(&lsproto::TEXT_DOCUMENT_COMPLETION_INFO, completion_params(&b_uri, 0, 1));

        close(&client, &b_uri);

        let (msg, resp) = wait_for_completion();
        assert_some_var_auto_import(&msg, resp, "someVar");
    }
}

child_test! {
    // Go: server_completion_test.go:176 TestCompletionForUnopenedFile
    fn completion_for_unopened_file() {
        let client = init_completion_client(
            "/home/projects",
            &[
                ("/home/projects/tsconfig.json", TSCONFIG),
                ("/home/projects/c.ts", "let xyz = 1;\nxy"),
            ],
        );

        let c_uri = lsconv::file_name_to_document_uri("/home/projects/c.ts");
        let (msg, resp) = client.send_request(&lsproto::TEXT_DOCUMENT_COMPLETION_INFO, completion_params(&c_uri, 1, 2));
        assert!(msg.error.is_none(), "{:?}", msg.error);
        assert!(find_completion_item(&completion_items(resp), "xyz").is_some());
    }
}

child_test! {
    // Go: server_completion_test.go:200 TestAutoImportCompletionForUnopenedFile
    fn auto_import_completion_for_unopened_file() {
        let client = init_completion_client(
            "/home/projects",
            &[
                ("/home/projects/tsconfig.json", TSCONFIG),
                ("/home/projects/a.ts", "export const someVar = 10;"),
                ("/home/projects/c.ts", "s"),
            ],
        );

        let c_uri = lsconv::file_name_to_document_uri("/home/projects/c.ts");
        let (msg, resp) = client.send_request(&lsproto::TEXT_DOCUMENT_COMPLETION_INFO, completion_params(&c_uri, 0, 1));
        assert_some_var_auto_import(&msg, resp, "someVar");
    }
}

child_test! {
    // Go: server_completion_test.go:235 TestCompletionSnapshotFreezing
    fn completion_snapshot_freezing() {
        let client = init_completion_client(
            "/home/projects",
            &[
                ("/home/projects/tsconfig.json", TSCONFIG),
                ("/home/projects/a.ts", "export const someVar = 10;"),
                ("/home/projects/b.ts", "someV"),
            ],
        );

        let a_uri = lsconv::file_name_to_document_uri("/home/projects/a.ts");
        let b_uri = lsconv::file_name_to_document_uri("/home/projects/b.ts");
        open(&client, &a_uri, "export const someVar = 10;");
        open(&client, &b_uri, "someV");

        let wait_for_completion =
            client.send_request_async(&lsproto::TEXT_DOCUMENT_COMPLETION_INFO, completion_params(&b_uri, 0, 5));

        client.send_notification(
            &lsproto::TEXT_DOCUMENT_DID_CHANGE_INFO,
            lsproto::DidChangeTextDocumentParams {
                text_document: lsproto::VersionedTextDocumentIdentifier {
                    uri: b_uri.clone(),
                    version: 2,
                },
                content_changes: vec![lsproto::TextDocumentContentChangePartialOrWholeDocument {
                    partial: None,
                    whole_document: Some(lsproto::TextDocumentContentChangeWholeDocument {
                        text: "notMatching".to_string(),
                    }),
                }],
            },
        );

        let (msg, resp) = wait_for_completion();
        assert_some_var_auto_import(
            &msg,
            resp,
            "expected someVar in completions (snapshot freezing should preserve original content)",
        );
    }
}

child_test! {
    // PORT: no Go counterpart (optapifuzz1 A). The user types an
    // unterminated JSDoc comment at the end of a file, and its last 2 bytes
    // split a char. Go cuts the comment text 2 bytes before the end
    // (parser/jsdoc.go:163) and reads the kept bytes as RuneError. The port
    // panicked on that cut: the server exited 70 on the diagnostics of the JS
    // file, which parses JSDoc at once, and the completion at the end of the
    // TS file, which parses it lazily, answered an internal error. Go
    // answers TS1010 at the end of the file and a null completion.
    fn jsdoc_cut_inside_a_char_at_the_end_of_a_file() {
        let client = init_completion_client(
            "/home/projects",
            &[
                (
                    "/home/projects/tsconfig.json",
                    r#"{"compilerOptions": {"allowJs": true, "checkJs": true, "noEmit": true}}"#,
                ),
                ("/home/projects/a.js", "const a = 1;\n"),
                ("/home/projects/b.ts", "const b = 1;\n"),
            ],
        );
        let a_uri = lsconv::file_name_to_document_uri("/home/projects/a.js");
        let b_uri = lsconv::file_name_to_document_uri("/home/projects/b.ts");
        client.send_notification(
            &lsproto::TEXT_DOCUMENT_DID_OPEN_INFO,
            lsproto::DidOpenTextDocumentParams {
                text_document: Some(lsproto::TextDocumentItem {
                    uri: a_uri.clone(),
                    language_id: lsproto::LanguageKind::JAVA_SCRIPT,
                    text: "const a = 1;\n/** 日本語".to_string(),
                    ..Default::default()
                }),
            },
        );
        open(&client, &b_uri, "const b = 1;\n/** Cafés");

        let (msg, report) = client.send_request(
            &lsproto::TEXT_DOCUMENT_DIAGNOSTIC_INFO,
            lsproto::DocumentDiagnosticParams {
                text_document: lsproto::TextDocumentIdentifier { uri: a_uri.clone() },
                ..Default::default()
            },
        );
        assert!(msg.error.is_none(), "{:?}", msg.error);
        let items = report
            .and_then(|report| report.full_document_diagnostic_report)
            .expect("a full document diagnostic report")
            .items;
        let found: Vec<_> = items
            .iter()
            .map(|d| (d.code.as_ref().and_then(|c| c.integer), d.range.start.line, d.range.start.character))
            .collect();
        assert_eq!(found, [(Some(1010), 1, 7)]);

        let (msg, resp) = client.send_request(
            &lsproto::TEXT_DOCUMENT_COMPLETION_INFO,
            completion_params(&b_uri, 1, 9),
        );
        assert!(msg.error.is_none(), "{:?}", msg.error);
        assert!(resp.is_none_or(|resp| resp.items.is_none() && resp.list.is_none()));

        // The server is alive.
        let (msg, _) = client.send_request(
            &lsproto::TEXT_DOCUMENT_HOVER_INFO,
            lsproto::HoverParams {
                text_document: lsproto::TextDocumentIdentifier { uri: a_uri },
                position: lsproto::Position { line: 0, character: 6 },
                ..Default::default()
            },
        );
        assert!(msg.error.is_none(), "{:?}", msg.error);
    }
}

child_test! {
    // PORT: no Go counterpart (followups24, R171 reviewer). A member
    // completion of a name that is not an identifier inserts `[name]`, and
    // Go quotes the name unless it starts with a decimal digit
    // (ls/completions.go:3711 quotePropertyName, `unicode.IsDigit` at
    // go1.27.1, Unicode 17.0.0). U+10D40 and U+1CCF0 are Nd from Unicode
    // 16.0; the port's old Unicode 15.0.0 table quoted them. U+0660 is Nd in
    // both. Each item is Go N's (tsgo-oracle-673a5f17d713, followups24
    // lspcases.py nd-member).
    fn member_completion_of_a_name_that_starts_with_a_unicode_17_digit_is_not_quoted() {
        let client = init_completion_client(
            "/home/projects",
            &[
                ("/home/projects/tsconfig.json", TSCONFIG),
                ("/home/projects/a.ts", "export {};\n"),
            ],
        );
        let a_uri = lsconv::file_name_to_document_uri("/home/projects/a.ts");
        open(
            &client,
            &a_uri,
            "const o = { \"\u{10D40}x\": 1, \"\u{1CCF0}y\": 2, \"\u{660}z\": 3, \"w v\": 4 };\no.",
        );
        let (msg, resp) = client.send_request(
            &lsproto::TEXT_DOCUMENT_COMPLETION_INFO,
            completion_params(&a_uri, 1, 2),
        );
        assert!(msg.error.is_none(), "{:?}", msg.error);
        // Plain text, not `{:?}`: Rust escapes the chars that its own
        // Unicode tables do not know.
        let mut items: Vec<String> = completion_items(resp)
            .into_iter()
            .map(|item| {
                let edit = item
                    .text_edit
                    .and_then(|edit| edit.text_edit)
                    .map(|edit| {
                        let (start, end) = (edit.range.start, edit.range.end);
                        format!(
                            "{}@{}:{}-{}:{}",
                            edit.new_text, start.line, start.character, end.line, end.character
                        )
                    })
                    .unwrap_or_default();
                format!(
                    "{} | {} | {} | {edit}",
                    item.label,
                    item.insert_text.unwrap_or_default(),
                    item.filter_text.unwrap_or_default(),
                )
            })
            .collect();
        items.sort();
        assert_eq!(
            items,
            [
                "w v | [\"w v\"] | .w v | [\"w v\"]@1:1-1:2",
                "\u{660}z | [\u{660}z] | .\u{660}z | [\u{660}z]@1:1-1:2",
                "\u{10D40}x | [\u{10D40}x] | .\u{10D40}x | [\u{10D40}x]@1:1-1:2",
                "\u{1CCF0}y | [\u{1CCF0}y] | .\u{1CCF0}y | [\u{1CCF0}y]@1:1-1:2",
            ]
        );
    }
}

/// A didChange of `u` to `text`, the whole document.
fn change(client: &LspClient, u: &lsproto::DocumentUri, version: i32, text: &str) {
    client.send_notification(
        &lsproto::TEXT_DOCUMENT_DID_CHANGE_INFO,
        lsproto::DidChangeTextDocumentParams {
            text_document: lsproto::VersionedTextDocumentIdentifier {
                uri: u.clone(),
                version,
            },
            content_changes: vec![lsproto::TextDocumentContentChangePartialOrWholeDocument {
                partial: None,
                whole_document: Some(lsproto::TextDocumentContentChangeWholeDocument {
                    text: text.to_string(),
                }),
            }],
        },
    );
}

child_test! {
    // PORT: no Go counterpart (lswarm1, lspsweep2 g3-warm-race). The
    // diagnostic pull flushes the change that imports ./ext/other, and that
    // snapshot change starts the auto-import warm (Go warmAutoImportCache,
    // session.go:2046), which indexes other.ts while the program has it. Go
    // starts the warm on a goroutine before the diagnostic's answer. The
    // next change removes the import 1 s after the answer; Go's
    // registry keeps the exports of a file that left the program
    // (registry.go:1033-1040, :1137), so the last completion offers
    // `widget`. The port started the warm only after `IDLE_QUIET_PERIOD`
    // with no message, so the change cancelled it and the completion did
    // not offer `widget`. This process waits 10 s for quiet
    // (`set_idle_quiet_period`), so the gap stays under the quiet period
    // and the old rule still fails, and the clone has 1 s on a loaded host.
    fn auto_import_warm_runs_before_the_next_change() {
        ts_goport::lsp::set_idle_quiet_period(std::time::Duration::from_secs(10));
        let client = init_completion_client(
            "/home/projects",
            &[
                (
                    "/home/projects/tsconfig.json",
                    r#"{"compilerOptions": {"strict": true, "target": "es2020", "module": "esnext", "moduleResolution": "bundler"}, "files": ["a.ts"]}"#,
                ),
                ("/home/projects/a.ts", "export function main() {\n  return 1;\n}\nwid\n"),
                (
                    "/home/projects/ext/other.ts",
                    "export const other = 1;\nexport function widget(): number { return 2; }\n",
                ),
            ],
        );
        let a_uri = lsconv::file_name_to_document_uri("/home/projects/a.ts");
        let text = "export function main() {\n  return 1;\n}\nwid\n";
        open(&client, &a_uri, text);
        let (msg, _) = client.send_request(
            &lsproto::TEXT_DOCUMENT_COMPLETION_INFO,
            completion_params(&a_uri, 3, 3),
        );
        assert!(msg.error.is_none(), "{:?}", msg.error);

        change(&client, &a_uri, 2, &format!("import {{ other }} from './ext/other';\n{text}"));
        let (msg, _) = client.send_request(
            &lsproto::TEXT_DOCUMENT_DIAGNOSTIC_INFO,
            lsproto::DocumentDiagnosticParams {
                text_document: lsproto::TextDocumentIdentifier { uri: a_uri.clone() },
                ..Default::default()
            },
        );
        assert!(msg.error.is_none(), "{:?}", msg.error);
        std::thread::sleep(std::time::Duration::from_secs(1));
        change(&client, &a_uri, 3, text);

        let (msg, resp) = client.send_request(
            &lsproto::TEXT_DOCUMENT_COMPLETION_INFO,
            completion_params(&a_uri, 3, 3),
        );
        assert!(msg.error.is_none(), "{:?}", msg.error);
        let items = completion_items(resp);
        let widget = find_completion_item(&items, "widget").expect("widget in the completions");
        let auto_import = widget
            .data
            .as_ref()
            .and_then(|data| data.auto_import.as_ref())
            .expect("item.Data.AutoImport");
        assert_eq!(auto_import.module_specifier, "./ext/other");
    }
}

/// A project for the module augmentation tests (aispec1, knownprob1 S3), as
/// in Hono, where each middleware augments `ContextVariableMap`: each
/// `src/mw/<name>/index.ts` re-exports `exports` from `./<name>` and augments
/// `'../..'`, which is `src/index.ts`. Opens `src/main.ts`.
fn augmentation_client(middleware: &[(&str, &[&str])]) -> (LspClient, lsproto::DocumentUri) {
    let mut entries = vec![
        (
            "/home/projects/tsconfig.json".to_string(),
            r#"{"compilerOptions": {"module": "esnext", "moduleResolution": "bundler", "target": "esnext", "strict": true}}"#.to_string(),
        ),
        ("/home/projects/src/index.ts".to_string(), "export type { Vars } from './context'\n".to_string()),
        ("/home/projects/src/context.ts".to_string(), "export interface Vars {}\n".to_string()),
        ("/home/projects/src/main.ts".to_string(), "export const x = 1\n".to_string()),
    ];
    for (name, exports) in middleware {
        entries.push((
            format!("/home/projects/src/mw/{name}/index.ts"),
            format!(
                "export {{ {} }} from './{name}'\n\ndeclare module '../..' {{\n  interface Vars {{\n    {name}: string\n  }}\n}}\n",
                exports.join(", ")
            ),
        ));
        entries.push((
            format!("/home/projects/src/mw/{name}/{name}.ts"),
            exports
                .iter()
                .map(|e| format!("export const {e} = () => '{e}'\n"))
                .collect(),
        ));
    }
    let entries: Vec<(&str, &str)> = entries
        .iter()
        .map(|(p, t)| (p.as_str(), t.as_str()))
        .collect();
    let client = init_completion_client("/home/projects", &entries);
    let main_uri = lsconv::file_name_to_document_uri("/home/projects/src/main.ts");
    open(&client, &main_uri, "export const x = 1\n");
    (client, main_uri)
}

/// Sets the second line of `src/main.ts` to `const y = <prefix>` and asks for
/// completions at its end. Returns the auto-import module specifier of each
/// label of `labels` that is in the list.
fn auto_import_specifiers(
    client: &LspClient,
    u: &lsproto::DocumentUri,
    version: i32,
    prefix: &str,
    labels: &[&str],
) -> Vec<(String, String)> {
    let line = format!("const y = {prefix}");
    client.send_notification(
        &lsproto::TEXT_DOCUMENT_DID_CHANGE_INFO,
        lsproto::DidChangeTextDocumentParams {
            text_document: lsproto::VersionedTextDocumentIdentifier {
                uri: u.clone(),
                version,
            },
            content_changes: vec![lsproto::TextDocumentContentChangePartialOrWholeDocument {
                partial: None,
                whole_document: Some(lsproto::TextDocumentContentChangeWholeDocument {
                    text: format!("export const x = 1\n{line}"),
                }),
            }],
        },
    );
    let (msg, resp) = client.send_request(
        &lsproto::TEXT_DOCUMENT_COMPLETION_INFO,
        completion_params(u, 1, line.len() as u32),
    );
    assert!(msg.error.is_none(), "{:?}", msg.error);
    let items = completion_items(resp);
    labels
        .iter()
        .filter_map(|label| {
            let item = find_completion_item(&items, label)?;
            let auto_import = item.data.as_ref()?.auto_import.as_ref()?;
            Some((label.to_string(), auto_import.module_specifier.clone()))
        })
        .collect()
}

child_test! {
    // PORT: no Go counterpart (aispec1, knownprob1 S3). Go caches the
    // auto-import specifier of each importing file by the export's Path, the
    // file that declares it (ls/autoimport/specifiers.go GetModuleSpecifier),
    // but computes it from the export's ModuleFileName. The `Vars` export of
    // the augmentation has Path `src/mw/rid/index.ts` and ModuleFileName
    // `src/index.ts`. A completion for `V` computes only that export, so it
    // stores "." for `src/mw/rid/index.ts`, and the next completion gives
    // `requestId` the specifier "." in place of "./mw/rid". Go N
    // (tsgo-oracle-673a5f17d713) answers "." in 40 of 40 runs (aispec1
    // augment-one).
    fn augmentation_completion_sets_the_specifier_of_the_declaring_file() {
        let (client, main_uri) = augmentation_client(&[("rid", &["requestId"])]);
        let labels = ["requestId"];
        assert_eq!(auto_import_specifiers(&client, &main_uri, 2, "V", &labels), []);
        assert_eq!(
            auto_import_specifiers(&client, &main_uri, 3, "", &labels),
            [("requestId".to_string(), ".".to_string())]
        );
    }
}

child_test! {
    // PORT: no Go counterpart (aispec1, knownprob1 S3). One completion with
    // no prefix computes the augmentation group and the groups of both
    // files. The merged augmentation export has the Path of one of the two
    // files. When its group is made before the groups of that file, the
    // exports of that file get ".". Go merges a random last Path and
    // computes the groups in map order, so its answer varies. Go N
    // (tsgo-oracle-673a5f17d713, trace aispec1 augment-two, 2 sets of 40
    // runs) gives 3 answers: no "." in 28 and 31 runs, "." for the four
    // exports of `a` in 5 and 7, and of `b` in 7 and 2. The first assert
    // accepts each of them. goport always gives no ".": it merges the last
    // Path in program order (`src/mw/b`) and computes augmentation groups
    // last (autoimport/view.rs get_completions). The second assert keeps
    // that fixed choice.
    fn single_completion_computes_the_augmentation_last() {
        let (client, main_uri) = augmentation_client(&[
            ("a", &["aOne", "aTwo", "aThree", "aFour"]),
            ("b", &["bOne", "bTwo", "bThree", "bFour"]),
        ]);
        let labels = ["aOne", "aTwo", "aThree", "aFour", "bOne", "bTwo", "bThree", "bFour"];
        // The answer when the exports of the file `poisoned` get ".".
        let answer = |poisoned: &str| -> Vec<(String, String)> {
            labels
                .iter()
                .map(|label| {
                    let file = &label[..1];
                    let specifier = if file == poisoned {
                        ".".to_string()
                    } else {
                        format!("./mw/{file}")
                    };
                    (label.to_string(), specifier)
                })
                .collect()
        };
        let got = auto_import_specifiers(&client, &main_uri, 2, "", &labels);
        assert!(["", "a", "b"].map(answer).contains(&got), "{got:?}");
        assert_eq!(got, answer(""), "goport computes augmentation groups last");
    }
}

/// The keyword labels (sorted) at a statement and in a function body. Go N'
/// (tsgo-oracle-fed0bf24149f --lsp) gives these 47 at both positions of
/// `completion_keywords_at_a_statement`.
const STATEMENT_KEYWORDS: [&str; 47] = [
    "as",
    "async",
    "await",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "debugger",
    "default",
    "delete",
    "do",
    "else",
    "enum",
    "export",
    "extends",
    "false",
    "finally",
    "for",
    "function",
    "if",
    "implements",
    "import",
    "in",
    "instanceof",
    "interface",
    "let",
    "new",
    "null",
    "package",
    "return",
    "satisfies",
    "super",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "type",
    "typeof",
    "using",
    "var",
    "void",
    "while",
    "with",
    "yield",
];

child_test! {
    // PORT: no Go counterpart. The keyword items come from each kind from
    // FirstKeyword to LastKeyword through the scanner keyword table. A kind
    // with no table text (KindSourceKeyword of ts#63915, before its "source"
    // entry) gave an extra item with an empty label.
    fn completion_keywords_at_a_statement() {
        let text = "function f() {\n  \n}\n";
        let client = init_completion_client(
            "/home/projects",
            &[
                ("/home/projects/tsconfig.json", TSCONFIG),
                ("/home/projects/a.ts", text),
            ],
        );
        let uri = lsconv::file_name_to_document_uri("/home/projects/a.ts");
        open(&client, &uri, text);
        for (line, character) in [(1, 2), (3, 0)] {
            let (msg, resp) = client.send_request(
                &lsproto::TEXT_DOCUMENT_COMPLETION_INFO,
                completion_params(&uri, line, character),
            );
            assert!(msg.error.is_none(), "{:?}", msg.error);
            let mut labels: Vec<String> = completion_items(resp)
                .into_iter()
                .filter(|item| item.kind == Some(lsproto::CompletionItemKind::KEYWORD))
                .map(|item| item.label)
                .collect();
            labels.sort();
            assert_eq!(labels, STATEMENT_KEYWORDS, "position {line}:{character}");
        }
    }
}

// ---------------------------------------------------------------------------
// Go `internal/lsp/server_completion_internal_test.go` (ts#64544, ts#64159)
// PORT: Go tests package `lsp` from inside with `&Server{}`. The port makes
// a server that never runs (`lsp::new_server`) and calls the handler, which
// is `pub`. The tests are here so that no new test module is needed.
// ---------------------------------------------------------------------------

mod completion_internal {
    use std::rc::Rc;
    use std::time::Duration;

    use ts_goport::frontend::bundled;
    use ts_goport::gostd::{GoError, context, errors};
    use ts_goport::lsp::{self, lsproto};

    use crate::support::vfstest::{MapFile, MapFs};

    /// A reader at end of input (the server never runs).
    struct NoInput;

    impl lsp::Reader for NoInput {
        fn read(&mut self) -> (Option<lsproto::Message>, Option<GoError>) {
            (None, Some(errors::EOF.clone()))
        }
    }

    /// A writer that drops every message (the server never runs).
    struct NoOutput;

    impl lsp::Writer for NoOutput {
        fn write(&mut self, _msg: &lsproto::Message) -> Result<(), GoError> {
            Ok(())
        }
    }

    /// Go `server := &Server{}`; `server.handleCompletionItemResolve(ctx,
    /// &lsproto.CompletionItem{Data: &lsproto.CompletionItemData{FileName:
    /// fileName}}, nil)` gives an error with `message`.
    pub(super) fn assert_resolve_error(file_name: &str, message: &str) {
        let server = lsp::new_server(lsp::ServerOptions {
            in_: Box::new(NoInput),
            out: Box::new(NoOutput),
            err: Box::new(std::io::sink()),
            cwd: "/".to_string(),
            fs: bundled::wrap_fs(MapFs::from_map(Vec::<(String, MapFile)>::new(), false).fs()),
            default_library_path: bundled::lib_path(),
            typings_location: String::new(),
            parse_cache: None,
            npm_install: None,
            spawn: None,
            progress_delay: Duration::ZERO,
            set_parent_process_id: None,
        });
        let item = lsproto::CompletionItem {
            data: Some(lsproto::CompletionItemData {
                file_name: file_name.to_string(),
                ..Default::default()
            }),
            ..Default::default()
        };
        let req = Rc::new(lsproto::RequestMessage::default());
        match server.handle_completion_item_resolve(&context::background(), Some(&item), &req) {
            Ok(_) => panic!("{file_name}: no error"),
            Err(err) => assert_eq!(err.error(), message, "{file_name}"),
        }
    }
}

child_test! {
    // Go: server_completion_internal_test.go:11 TestCompletionItemResolveRejectsRelativeFileName (ts#64159)
    fn completion_item_resolve_rejects_relative_file_name() {
        completion_internal::assert_resolve_error(
            "relative.ts",
            "completion item data fileName must be absolute",
        );
    }
}

child_test! {
    // Go: server_completion_internal_test.go:21 TestCompletionItemResolveRejectsMalformedDynamicFileName (ts#64159)
    fn completion_item_resolve_rejects_malformed_dynamic_file_name() {
        for file_name in ["^/invalid", "^/~ts-uri~/scheme/authority/~ts-uri-escape~zz~"] {
            completion_internal::assert_resolve_error(
                file_name,
                "completion item data fileName must be a valid dynamic path",
            );
        }
    }
}

child_test! {
    // Server skeptic probe srvskp/resolve-more-names (bump D round 2). Go N'
    // handleCompletionItemResolve passes the rooted, normalized file name to
    // ResolveCompletionItem (server.go:2178), which finds the file by it
    // (completions.go:5498), so a `data.fileName` with a trailing separator
    // or extra segments still resolves. Go answers the item, not "file not
    // found".
    fn completion_item_resolve_finds_the_file_by_its_normalized_name() {
        const A: &str = "export const alpha = 1;\nalp\n";
        let client = init_completion_client(
            "/home/projects",
            &[("/home/projects/tsconfig.json", TSCONFIG), ("/home/projects/a.ts", A)],
        );
        let a_uri = lsconv::file_name_to_document_uri("/home/projects/a.ts");
        open(&client, &a_uri, A);
        let (msg, resp) =
            client.send_request(&lsproto::TEXT_DOCUMENT_COMPLETION_INFO, completion_params(&a_uri, 1, 3));
        assert!(msg.error.is_none(), "{:?}", msg.error);
        let items = completion_items(resp);
        let item = find_completion_item(&items, "alpha").expect("alpha");

        for file_name in [
            "/home/projects/a.ts/",
            "/home/projects//a.ts",
            "/home/projects/sub//../a.ts",
        ] {
            let mut item = item.clone();
            item.data.as_mut().expect("item.Data").file_name = file_name.to_string();
            let (msg, resp) = client.send_request(&lsproto::COMPLETION_ITEM_RESOLVE_INFO, item);
            assert!(msg.error.is_none(), "{file_name}: {:?}", msg.error);
            let label = resp.flatten().map(|item| item.label);
            assert_eq!(label.as_deref(), Some("alpha"), "{file_name}");
        }
    }
}
