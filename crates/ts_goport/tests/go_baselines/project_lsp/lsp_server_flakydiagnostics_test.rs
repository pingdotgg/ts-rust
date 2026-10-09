//! Port of Go `internal/lsp/server_flakydiagnostics_test.go` (ts#64543).
//!
//! PORT: Go `<-client.Server.InitComplete()` has no port (see
//! `lsptestutil`). Go skips the test when the bundled files are not
//! embedded; the port's bundled files are always there. Go runs the subtests
//! in parallel.

use std::sync::Arc;

use ts_goport::lsp::lsproto;

use super::lsptestutil::{self, result_response};
use super::projecttestutil::files;
use super::util::uri;

child_test! {
    // Go: server_flakydiagnostics_test.go:18 TestFlakyDiagnosticTrackingParallelEmit
    fn flaky_diagnostic_tracking_parallel_emit() {
        for no_emit_on_error in [false, true] {
            let tsconfig = format!(
                r#"{{
					"compilerOptions": {{ "strict": true, "declaration": true, "noEmitOnError": {no_emit_on_error}, "outDir": "out" }}
				}}"#
            );
            let a = "export function box<T>(value: T) { return { value }; }";
            // PORT: Go's file system is case-insensitive (`tspath.CaseInsensitive`);
            // `server_setup` makes the same.
            let on_server_request: lsptestutil::ServerRequestHandler = Arc::new(|req| {
                if req.method == lsproto::Method::CLIENT_REGISTER_CAPABILITY
                    || req.method == lsproto::Method::CLIENT_UNREGISTER_CAPABILITY
                    || req.method == lsproto::Method::WINDOW_WORK_DONE_PROGRESS_CREATE
                {
                    return Some(result_response(req, Box::new(lsproto::Null)));
                }
                None
            });
            let client = lsptestutil::new_lsp_client(
                lsptestutil::server_setup(
                    "/src",
                    files(&[
                        ("/src/tsconfig.json", tsconfig.as_str()),
                        ("/src/a.ts", a),
                        (
                            "/src/b.ts",
                            r#"import { box } from "./a"; export const b = box("b");"#,
                        ),
                        (
                            "/src/c.ts",
                            r#"import { box } from "./a"; export const c = box(1);"#,
                        ),
                    ]),
                ),
                Some(on_server_request),
                None,
            );

            let (init_msg, init_result) = client.send_request(
                &lsproto::INITIALIZE_INFO,
                lsproto::InitializeParams {
                    capabilities: Some(lsproto::ClientCapabilities::default()),
                    initialization_options: Some(lsproto::InitializationOptionsOrNull {
                        initialization_options: Some(lsproto::InitializationOptions {
                            track_flaky_diagnostics: Some(lsproto::DiagnosticFlakeLogLevel::PANIC),
                            ..Default::default()
                        }),
                    }),
                    ..Default::default()
                },
            );
            assert!(
                init_result.is_some() && init_msg.error.is_none(),
                "initialize failed"
            );
            client.send_notification(&lsproto::INITIALIZED_INFO, lsproto::InitializedParams::default());

            let u = uri("file:///src/a.ts");
            client.send_notification(
                &lsproto::TEXT_DOCUMENT_DID_OPEN_INFO,
                lsproto::DidOpenTextDocumentParams {
                    text_document: Some(lsproto::TextDocumentItem {
                        uri: u.clone(),
                        language_id: lsproto::LanguageKind::TYPE_SCRIPT,
                        text: a.to_string(),
                        ..Default::default()
                    }),
                },
            );
            let (msg, diagnostics) = client.send_request(
                &lsproto::TEXT_DOCUMENT_DIAGNOSTIC_INFO,
                lsproto::DocumentDiagnosticParams {
                    text_document: lsproto::TextDocumentIdentifier { uri: u },
                    ..Default::default()
                },
            );
            assert!(
                diagnostics.is_some() && msg.error.is_none(),
                "diagnostics request failed (noEmitOnError={no_emit_on_error})"
            );
            let report = diagnostics
                .and_then(|diagnostics| diagnostics.full_document_diagnostic_report)
                .expect("expected a full document diagnostic report");
            assert_eq!(report.items.len(), 0);
        }
    }
}
