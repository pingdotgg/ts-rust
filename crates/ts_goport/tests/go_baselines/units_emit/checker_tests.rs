//! Ports of internal/checker/checker_test.go tests that bump D adds
//! (TestGetTypeAtLocationOfTypeOnlyImportClause, ts#64468). The older
//! TestGetSymbolAtLocation is in `program_tests.rs`.
//!
//! Each test builds a program, so it runs in a child process of its own
//! (see `childprog`).

use super::childprog::{in_child, install_map_fs, new_program_with_config, source_file};
use crate::support::vfstest::MapFs;
use ts_goport::frontend::bundled;
use ts_goport::frontend::compiler::{CompilerHost, new_compiler_host};
use ts_goport::frontend::tsoptions::{ParseConfigHost, get_parsed_command_line_of_config_file};
use ts_goport::frontend::vfs::Fs;
use ts_goport::gostd::context;
use ts_goport::prelude::*;
use ts_goport::program::ls_program;

/// Go passes the compiler host, which is also a `tsoptions.ParseConfigHost`.
/// This forwards the two methods (as `program_tests.rs` does).
struct HostAsParseConfigHost(Rc<dyn CompilerHost>);

impl ParseConfigHost for HostAsParseConfigHost {
    fn fs(&self) -> Rc<dyn Fs> {
        self.0.fs()
    }
    fn get_current_directory(&self) -> String {
        self.0.get_current_directory()
    }
}

// Go: checker/checker_test.go:63 TestGetTypeAtLocationOfTypeOnlyImportClause
#[test]
fn test_get_type_at_location_of_type_only_import_clause() {
    in_child(
        module_path!(),
        "test_get_type_at_location_of_type_only_import_clause",
        || {
            let map_fs = MapFs::from_map(
                [
                    (
                        "/types.ts",
                        "export type U = number;
export default interface D { x: number }",
                    ),
                    (
                        "/main.ts",
                        r#"import type { U } from "./types";
import type * as types from "./types";
import { U as V } from "./types";
import type D from "./types";
export const u: U = 1;
export const v: V = 1;
export type W = types.U;
export type E = D;"#,
                    ),
                    (
                        "/tsconfig.json",
                        r#"
				{
					"compilerOptions": {},
					"files": ["types.ts", "main.ts"]
				}
			"#,
                    ),
                ],
                false, /*useCaseSensitiveFileNames*/
            );
            install_map_fs(&map_fs, "/");
            let fs = bundled::wrap_fs(map_fs.fs());

            let cd = "/";
            let host = new_compiler_host(cd, fs, &bundled::lib_path(), None, None, None);

            let (parsed, errors) = get_parsed_command_line_of_config_file(
                "/tsconfig.json",
                Some(&CompilerOptions::default()),
                None,
                &HostAsParseConfigHost(host),
                None,
            );
            assert_eq!(errors.len(), 0, "Expected no errors in parsed command line");

            let p =
                new_program_with_config(map_fs.fs(), cd, Rc::new(parsed.expect("parsed config")));
            let _current = ls_program::enter(&p);
            ls_program::bind_source_files(&p);
            let (c, done) = ls_program::get_type_checker(&p, &context::background());
            let file = source_file(&p, "/main.ts").root;
            let import_clause_at = |index: usize| file.statements().get(index).import_clause();
            // An import clause without a default binding has no symbol of its own. A type-only one
            // should get the same type as the equivalent regular import instead of crashing.
            let regular = c.borrow_mut().get_type_at_location(import_clause_at(2));
            for index in [0, 1] {
                let typ = c.borrow_mut().get_type_at_location(import_clause_at(index));
                assert!(
                    typ.is_some(),
                    "Expected type of import clause {index} to be non-nil"
                );
                assert_eq!(typ, regular);
            }

            let default_clause = c.borrow_mut().get_type_at_location(import_clause_at(3));
            let default_reference = c
                .borrow_mut()
                .get_type_at_location(file.statements().get(7).type_());
            assert_eq!(default_clause, default_reference);
            done.call();
        },
    );
}
