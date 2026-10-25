//! Port-only test (loadparity1): the program walk keys its casing check by
//! Go `PathKey.CaseInsensitiveKey` (tspath/pathkey.go:84), which keeps an
//! encoded dynamic name as it is. So two dynamic documents whose names
//! differ only in case get no casing diagnostic (TS1149). The expected
//! answer is Go's LSP `textDocument/diagnostic` (pin 3e9236361fa9): none.
//! The walk checks casing only on a case-sensitive file system, so the
//! session's map file system is case-sensitive (Go `projecttestutil.Setup`
//! uses a case-insensitive one).

use std::rc::Rc;

use ts_goport::frontend::bundled;
use ts_goport::lsp::lsproto;
use ts_goport::program::ls_program;
use ts_goport::project::{self, Session, SessionInit, SessionOptions};

use super::projecttestutil::{self, files};
use super::util::*;

/// `bare_session` on a case-sensitive map file system.
fn case_sensitive_session() -> Rc<Session> {
    let (_, fs) =
        projecttestutil::wrapped_map_fs(files(&[]), true /*useCaseSensitiveFileNames*/);
    project::new_session(&SessionInit {
        background_ctx: bg(),
        options: Rc::new(SessionOptions {
            default_library_path: bundled::lib_path(),
            position_encoding: lsproto::PositionEncodingKind::UTF8,
            watch_enabled: false,
            logging_enabled: false,
            ..projecttestutil::session_options("/")
        }),
        fs,
        client: None,
        logger: None,
        npm_executor: None,
        spawner: None,
        content_mapper_logger: None,
        parse_cache: None,
        content_mapped_parse_cache: None,
    })
}

const MAIN: &str =
    "import { a } from \"./Foo\";\nimport { b } from \"./foo\";\nexport const c = a + b;\n";

child_test! {
    fn dynamic_names_that_differ_only_in_case_get_no_casing_diagnostic() {
        let session = case_sensitive_session();
        let main = "vscode-vfs://github/o/r/main.ts";
        open(&session, main, MAIN);
        open(&session, "vscode-vfs://github/o/r/Foo.ts", "export const a = 1;\n");
        open(&session, "vscode-vfs://github/o/r/foo.ts", "export const b = 2;\n");
        let p = program(&session, main);
        let file = p
            .get_source_file(&uri(main).file_name())
            .expect("no source file main.ts");
        let ctx = projecttestutil::with_request_id(&bg());
        let codes: Vec<i32> = ls_program::get_semantic_diagnostics(&p, &ctx, file.root)
            .iter()
            .map(|d| d.code())
            .collect();
        assert_eq!(codes, [] as [i32; 0]);
    }
}
