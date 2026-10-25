//! Port-only test (loadparity1): the include processor diagnostics of a
//! project read the bound CommonJS indicator. Go binds a project program
//! right after `NewProgram` (project.go:598) and builds those diagnostics at
//! their first read, after the bind. The expected text is Go's LSP
//! `textDocument/diagnostic` answer (pin 3e9236361fa9).

use ts_goport::locale::Locale;
use ts_goport::program::ls_program;

use super::projecttestutil::{self, files};
use super::util::*;

const A: &str = "import c = require(\"../c.js\");\nimport f = require(\"../f.js\");\nexport const a = [c, f];\n";

child_test! {
    fn root_dir_diagnostic_explains_a_common_js_only_file() {
        let (session, _) = projecttestutil::setup(files(&[
            (
                "/src/tsconfig.json",
                r#"{"compilerOptions":{"allowJs":true,"module":"nodenext","moduleDetection":"legacy","rootDir":"sub","outDir":"out","noLib":true,"types":[]},"include":["sub"]}"#,
            ),
            ("/src/sub/a.ts", A),
            ("/src/c.js", "module.exports = 1;\n"),
            ("/src/f.js", "var f = 1;\n"),
        ]));
        open(&session, "file:///src/sub/a.ts", A);
        let p = program(&session, "file:///src/sub/a.ts");
        let file = p.get_source_file("/src/sub/a.ts").expect("no source file /src/sub/a.ts");
        let ctx = projecttestutil::with_request_id(&bg());
        let locale = Locale::default();
        let mut messages: Vec<(i32, String)> = ls_program::get_semantic_diagnostics(&p, &ctx, file.root)
            .iter()
            .map(|d| {
                let mut text = d.localize(&locale);
                for chained in d.message_chain() {
                    text.push_str("\n  ");
                    text.push_str(&chained.localize(&locale));
                }
                (d.code(), text)
            })
            .collect();
        messages.sort();
        let want = [
            (2306, "File '/src/f.js' is not a module."),
            (
                6059,
                "File '/src/c.js' is not under 'rootDir' '/src/sub'. 'rootDir' is expected to contain all source files.\n  File is CommonJS module because 'package.json' was not found",
            ),
            (
                6059,
                "File '/src/f.js' is not under 'rootDir' '/src/sub'. 'rootDir' is expected to contain all source files.",
            ),
        ]
        .map(|(code, text)| (code, text.to_string()));
        assert_eq!(messages, want);
    }
}
