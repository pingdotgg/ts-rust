//! Port of Go `internal/project/project_test.go` (`TestProjectProgramUpdateKind`,
//! `TestProject`, `TestPushDiagnostics`, `TestDisplayName`,
//! `TestProgressNotifications`).

use std::collections::BTreeMap;
use std::rc::Rc;

use ts_goport::diag;
use ts_goport::frontend::core_context::{self, CheckerLifetime};
use ts_goport::lsp::lsproto;
use ts_goport::options::{CompilerOptions, Tristate};
use ts_goport::program::ls_program;
use ts_goport::project::{ProgramUpdateKind, Session};

use super::projecttestutil::{self, ProgressCall, TypingsInstallerOptions, files};
use super::util::*;
use crate::support::baseline;

// Go: project_test.go:24 filterDiagnosticsByURI
// filterDiagnosticsByURI returns all PublishDiagnostics calls matching the given URI,
// starting from the given index.
fn filter_diagnostics_by_uri(
    calls: &[lsproto::PublishDiagnosticsParams],
    u: &str,
    from: usize,
) -> Vec<lsproto::PublishDiagnosticsParams> {
    calls[from..]
        .iter()
        .filter(|c| c.uri.0 == u)
        .cloned()
        .collect()
}

fn configured_update_kind(session: &Rc<Session>, config: &str) -> ProgramUpdateKind {
    session
        .snapshot()
        .project_collection
        .configured_project(&path(config))
        .expect("configured project")
        .borrow()
        .program_update_kind
}

const SRC_INDEX: &str = "file:///src/index.ts";

child_test! {
    // Go: project_test.go:43 TestProjectProgramUpdateKind/NewFiles on initial build
    fn program_update_kind_new_files_on_initial_build() {
        let (session, _) = projecttestutil::setup(files(&[
            ("/src/tsconfig.json", "{}"),
            ("/src/index.ts", "export const x = 1;"),
        ]));
        open(&session, SRC_INDEX, "export const x = 1;");
        let _ = language_service(&session, SRC_INDEX);
        assert_eq!(configured_update_kind(&session, "/src/tsconfig.json"), ProgramUpdateKind::NEW_FILES);
    }
}

child_test! {
    // Go: project_test.go:59 TestProjectProgramUpdateKind/Cloned on single-file change
    fn program_update_kind_cloned_on_single_file_change() {
        let (session, _) = projecttestutil::setup(files(&[
            ("/src/tsconfig.json", "{}"),
            ("/src/index.ts", "console.log('Hello');"),
        ]));
        open(&session, SRC_INDEX, "console.log('Hello');");
        let _ = language_service(&session, SRC_INDEX);
        edit(&session, SRC_INDEX, 2, (0, 20), (0, 20), "\n");
        let _ = language_service(&session, SRC_INDEX);
        assert_eq!(configured_update_kind(&session, "/src/tsconfig.json"), ProgramUpdateKind::CLONED);
    }
}

child_test! {
    // Go: project_test.go:83 TestProjectProgramUpdateKind/compiler options update inferred project (ts#63950)
    fn program_update_kind_compiler_options_update_inferred_project() {
        const FILE_NAME: &str = "/src/index.ts";
        let (session, _) = projecttestutil::setup(files(&[(FILE_NAME, "export const x = 1;")]));
        let u = format!("file://{FILE_NAME}");
        open(&session, &u, "export const x = 1;");
        let old_project = session
            .snapshot()
            .project_collection
            .inferred_project()
            .expect("inferred project");
        let old_program = old_project.borrow().program.clone().expect("program");
        assert_eq!(old_program.options().strict, Tristate::Unknown);

        session.did_change_compiler_options_for_inferred_projects(
            &bg(),
            Some(Rc::new(CompilerOptions {
                no_lib: Tristate::True,
                strict: Tristate::True,
                ..Default::default()
            })),
        );
        session
            .get_language_service(&bg(), &uri(&u))
            .unwrap_or_else(|err| panic!("GetLanguageService: {}", err.error()));

        let updated_project = session
            .snapshot()
            .project_collection
            .inferred_project()
            .expect("inferred project");
        let updated_project = updated_project.borrow();
        assert_eq!(
            updated_project
                .command_line
                .as_ref()
                .expect("command line")
                .compiler_options()
                .strict,
            Tristate::True
        );
        let updated_program = updated_project.program.clone().expect("program");
        assert!(!Rc::ptr_eq(&updated_program, &old_program));
        assert_eq!(updated_program.options().strict, Tristate::True);
        assert_eq!(old_program.options().strict, Tristate::Unknown);
    }
}

child_test! {
    // Server skeptic problem 3 (bump D round 2). Go N' compares the inferred
    // project's options with the generated Equals (ts#64457,
    // projectcollectionbuilder.go:1424), which checks the order of the
    // `paths` keys (options_generated.go:510, OrderedMap.EqualFunc). So new
    // options that differ only in that order replace the command line.
    fn compiler_options_with_reordered_paths_update_inferred_project() {
        const FILE_NAME: &str = "/src/index.ts";
        let (session, _) = projecttestutil::setup(files(&[(FILE_NAME, "export const x = 1;")]));
        let u = format!("file://{FILE_NAME}");
        open(&session, &u, "export const x = 1;");
        let paths_keys = |keys: &[&str]| -> Vec<String> {
            let options = Rc::new(CompilerOptions {
                no_lib: Tristate::True,
                paths: Some(
                    keys.iter()
                        .map(|key| (key.to_string(), Some(vec![format!("./{key}")])))
                        .collect(),
                ),
                ..Default::default()
            });
            session.did_change_compiler_options_for_inferred_projects(&bg(), Some(options));
            session
                .get_language_service(&bg(), &uri(&u))
                .unwrap_or_else(|err| panic!("GetLanguageService: {}", err.error()));
            let project = session
                .snapshot()
                .project_collection
                .inferred_project()
                .expect("inferred project");
            let project = project.borrow();
            let options = project.command_line.as_ref().expect("command line").compiler_options();
            options.paths.as_ref().expect("paths").keys().cloned().collect()
        };

        assert_eq!(paths_keys(&["a/*", "b/*"]), ["a/*", "b/*"]);
        assert_eq!(paths_keys(&["b/*", "a/*"]), ["b/*", "a/*"]);
    }
}

child_test! {
    // Go: project_test.go:111 TestProjectProgramUpdateKind/NewFiles when import resolution mode changes
    // #4792
    fn program_update_kind_new_files_when_import_resolution_mode_changes() {
        let index = r#"import type { Value } from "pkg" with { "resolution-mode": "require" };
const value: Value = { mode: "require" };"#;
        let (session, _) = projecttestutil::setup(files(&[
            (
                "/src/tsconfig.json",
                r#"{
				"compilerOptions":{"module":"preserve","moduleResolution":"bundler","noEmit":true},
				"files":["index.ts"]
			}"#,
            ),
            ("/src/index.ts", index),
            (
                "/src/node_modules/pkg/package.json",
                r#"{
				"name": "pkg",
				"version": "1.0.0",
				"exports": {
					".": {
						"import": "./index.mjs",
						"require": "./index.js"
					}
				}
			}"#,
            ),
            ("/src/node_modules/pkg/index.d.mts", r#"export interface Value { mode: "import" }"#),
            ("/src/node_modules/pkg/index.d.ts", r#"export interface Value { mode: "require" }"#),
        ]));
        open(&session, SRC_INDEX, index);
        let p = program(&session, SRC_INDEX);
        assert_eq!(sem_diag_count(&p, "/src/index.ts"), 0);

        session.did_change_file(
            &bg(),
            &uri(SRC_INDEX),
            2,
            &[lsproto::TextDocumentContentChangePartialOrWholeDocument {
                partial: None,
                whole_document: Some(lsproto::TextDocumentContentChangeWholeDocument {
                    text: r#"import type { Value } from "pkg" with { "resolution-mode": "import" };
const value: Value = { mode: "require" };"#
                        .to_string(),
                }),
            }],
        );
        let p = program(&session, SRC_INDEX);
        let file = p
            .get_source_file("/src/index.ts")
            .expect("no source file /src/index.ts");
        let diags = ls_program::get_semantic_diagnostics(
            &p,
            &projecttestutil::with_request_id(&bg()),
            file.root,
        );
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].code(), diag::Type_0_is_not_assignable_to_type_1.code() as i32);

        assert_eq!(configured_update_kind(&session, "/src/tsconfig.json"), ProgramUpdateKind::NEW_FILES);
    }
}

child_test! {
    // Go: project_test.go:80 TestProjectProgramUpdateKind/SameFileNames on config change without root changes
    fn program_update_kind_same_file_names_on_config_change_without_root_changes() {
        let (session, utils) = projecttestutil::setup(files(&[
            ("/src/tsconfig.json", r#"{"compilerOptions": {"strict": true}}"#),
            ("/src/index.ts", "export const x = 1;"),
        ]));
        open(&session, SRC_INDEX, "export const x = 1;");
        let _ = language_service(&session, SRC_INDEX);
        utils
            .fs()
            .write_file("/src/tsconfig.json", r#"{"compilerOptions": {"strict": false}}"#)
            .unwrap();
        watch(&session, &[(CHANGED, "file:///src/tsconfig.json")]);
        let _ = language_service(&session, SRC_INDEX);
        assert_eq!(
            configured_update_kind(&session, "/src/tsconfig.json"),
            ProgramUpdateKind::SAME_FILE_NAMES
        );
    }
}

child_test! {
    // Go: project_test.go:101 TestProjectProgramUpdateKind/NewFiles on root addition
    fn program_update_kind_new_files_on_root_addition() {
        let (session, utils) = projecttestutil::setup(files(&[
            ("/src/tsconfig.json", "{}"),
            ("/src/index.ts", "export {}"),
        ]));
        open(&session, SRC_INDEX, "export {}");
        let _ = language_service(&session, SRC_INDEX);
        let content = "export const y = 2;";
        utils.fs().write_file("/src/newfile.ts", content).unwrap();
        watch(&session, &[(CREATED, "file:///src/newfile.ts")]);
        open(&session, "file:///src/newfile.ts", content);
        let _ = language_service(&session, "file:///src/newfile.ts");
        assert_eq!(configured_update_kind(&session, "/src/tsconfig.json"), ProgramUpdateKind::NEW_FILES);
    }
}

child_test! {
    // Go: project_test.go:124 TestProjectProgramUpdateKind/SameFileNames when adding an unresolvable import with multi-file change
    fn program_update_kind_same_file_names_when_adding_an_unresolvable_import_with_multi_file_change() {
        let (session, _) = projecttestutil::setup(files(&[
            ("/src/tsconfig.json", "{}"),
            ("/src/index.ts", "export const x = 1;"),
            ("/src/other.ts", "export const z = 3;"),
        ]));
        open(&session, SRC_INDEX, "export const x = 1;");
        let _ = language_service(&session, SRC_INDEX);
        // Change index.ts to add an unresolvable import
        edit(&session, SRC_INDEX, 2, (0, 0), (0, 0), "\nimport \"./does-not-exist\";\n");
        let _ = language_service(&session, SRC_INDEX);
        assert_eq!(
            configured_update_kind(&session, "/src/tsconfig.json"),
            ProgramUpdateKind::SAME_FILE_NAMES
        );
    }
}

fn jquery_typings() -> TypingsInstallerOptions {
    let mut package_to_file = BTreeMap::new();
    // Provide typings content to be installed for jquery so ATA actually installs something
    package_to_file.insert(
        "jquery".to_string(),
        "declare const $: { x: number }".to_string(),
    );
    TypingsInstallerOptions {
        types_registry: Vec::new(),
        package_to_file,
    }
}

child_test! {
    // Go: project_test.go:154 TestProject/commandLineWithTypingsFiles is reset on CommandLine change
    fn command_line_with_typings_files_is_reset_on_command_line_change() {
        let (session, utils) = projecttestutil::setup_with_typings_installer(
            files(&[
                ("/user/username/projects/project1/app.js", ""),
                (
                    "/user/username/projects/project1/package.json",
                    r#"{"name":"p1","dependencies":{"jquery":"^3.1.0"}}"#,
                ),
                ("/user/username/projects/project2/app.js", ""),
            ]),
            jquery_typings(),
        );

        // 1) Open an inferred project file that triggers ATA
        let uri1 = "file:///user/username/projects/project1/app.js";
        open_kind(&session, uri1, "", lsproto::LanguageKind::JAVA_SCRIPT);

        // 2) Wait for ATA/background tasks to finish, then get a language service for the first file
        session.wait_for_background_tasks();
        // Sanity check: ensure ATA performed at least one install
        let npm_calls = utils.npm_executor().npm_install_calls();
        assert!(!npm_calls.is_empty(), "expected at least one npm install call from ATA");
        let _ = language_service(&session, uri1);

        // 3) Open another inferred project file
        let uri2 = "file:///user/username/projects/project2/app.js";
        open_kind(&session, uri2, "", lsproto::LanguageKind::JAVA_SCRIPT);

        // 4) Get a language service for the second file
        let _ = language_service(&session, uri2);
    }
}

child_test! {
    // Go: project_test.go:192 TestProject/inferred project rebuilt twice in one snapshot after typings install does not crash
    fn inferred_project_rebuilt_twice_in_one_snapshot_after_typings_install_does_not_crash() {
        let (session, utils) = projecttestutil::setup_with_typings_installer(
            files(&[
                ("/user/username/projects/project1/a.js", ""),
                (
                    "/user/username/projects/project1/package.json",
                    r#"{"name":"p1","dependencies":{"jquery":"^3.1.0"}}"#,
                ),
                ("/user/username/projects/project1/b.ts", "export const y = 1;"),
            ]),
            jquery_typings(),
        );

        let a_uri = "file:///user/username/projects/project1/a.js";
        let b_uri = "file:///user/username/projects/project1/b.ts";

        // 1) Open the file that triggers ATA, plus another file that joins the inferred project.
        open_kind(&session, a_uri, "", lsproto::LanguageKind::JAVA_SCRIPT);
        open(&session, b_uri, "export const y = 1;");

        // 2) Let ATA install jquery typings, then build the inferred program so its
        //    typings files are populated.
        session.wait_for_background_tasks();
        let npm_calls = utils.npm_executor().npm_install_calls();
        assert!(!npm_calls.is_empty(), "expected at least one npm install call from ATA");
        let _ = language_service(&session, a_uri);

        // 3) Queue two pending changes that will be flushed together in the next snapshot.
        session.did_change_file(
            &bg(),
            &uri(a_uri),
            2,
            &[lsproto::TextDocumentContentChangePartialOrWholeDocument {
                partial: None,
                whole_document: Some(lsproto::TextDocumentContentChangeWholeDocument {
                    text: "// changed".to_string(),
                }),
            }],
        );
        utils
            .fs()
            .write_file(
                "/user/username/projects/project1/tsconfig.json",
                r#"{"compilerOptions":{},"files":["b.ts"]}"#,
            )
            .unwrap();
        watch(&session, &[(CREATED, "file:///user/username/projects/project1/tsconfig.json")]);

        // 4) Flush the pending changes and the request in a single snapshot.
        let _ = language_service(&session, a_uri);
    }
}

const BASE_URL_TSCONFIG: &str = r#"{"compilerOptions": {"baseUrl": "."}}"#;

child_test! {
    // Go: project_test.go:272 TestPushDiagnostics/publishes program diagnostics on initial program creation
    fn push_diagnostics_publishes_program_diagnostics_on_initial_program_creation() {
        let (session, utils) = projecttestutil::setup(files(&[
            ("/src/tsconfig.json", BASE_URL_TSCONFIG),
            ("/src/index.ts", "export const x = 1;"),
        ]));
        open(&session, SRC_INDEX, "export const x = 1;");
        let _ = language_service(&session, SRC_INDEX);

        session.wait_for_background_tasks();

        let calls = utils.client().publish_diagnostics_calls();
        assert!(!calls.is_empty(), "expected at least one PublishDiagnostics call");

        // Find the call for tsconfig.json
        let tsconfig_call = calls.iter().find(|c| c.uri.0 == "file:///src/tsconfig.json");
        let tsconfig_call = tsconfig_call.expect("expected PublishDiagnostics call for tsconfig.json");
        assert!(!tsconfig_call.diagnostics.is_empty(), "expected at least one diagnostic");
    }
}

child_test! {
    // Go: project_test.go:303 TestPushDiagnostics/clears diagnostics when project is removed
    fn push_diagnostics_clears_diagnostics_when_project_is_removed() {
        let (session, utils) = projecttestutil::setup(files(&[
            ("/src/tsconfig.json", BASE_URL_TSCONFIG),
            ("/src/index.ts", "export const x = 1;"),
            ("/src2/tsconfig.json", r#"{"compilerOptions": {}}"#),
            ("/src2/index.ts", "export const y = 2;"),
        ]));
        open(&session, SRC_INDEX, "export const x = 1;");
        let _ = language_service(&session, SRC_INDEX);
        session.wait_for_background_tasks();

        // Open a file in a different project to trigger cleanup of the first
        close(&session, SRC_INDEX);
        open(&session, "file:///src2/index.ts", "export const y = 2;");
        let _ = language_service(&session, "file:///src2/index.ts");
        session.wait_for_background_tasks();

        let calls = utils.client().publish_diagnostics_calls();
        // Should have at least one call for the first project with diagnostics,
        // and one clearing it after switching projects
        let first_project_calls = filter_diagnostics_by_uri(&calls, "file:///src/tsconfig.json", 0);
        assert!(
            first_project_calls.len() >= 2,
            "expected at least 2 PublishDiagnostics calls for first project"
        );
        // Last call should clear diagnostics
        let last_call = first_project_calls.last().unwrap();
        assert_eq!(last_call.diagnostics.len(), 0, "expected empty diagnostics after project cleanup");
    }
}

child_test! {
    // Go: project_test.go:342 TestPushDiagnostics/updates diagnostics when program changes
    fn push_diagnostics_updates_diagnostics_when_program_changes() {
        let (session, utils) = projecttestutil::setup(files(&[
            ("/src/tsconfig.json", BASE_URL_TSCONFIG),
            ("/src/index.ts", "export const x = 1;"),
        ]));
        open(&session, SRC_INDEX, "export const x = 1;");
        let _ = language_service(&session, SRC_INDEX);
        session.wait_for_background_tasks();

        let initial_call_count = utils.client().publish_diagnostics_calls().len();

        // Change the tsconfig to remove baseUrl
        utils.fs().write_file("/src/tsconfig.json", r#"{"compilerOptions": {}}"#).unwrap();
        watch(&session, &[(CHANGED, "file:///src/tsconfig.json")]);
        let _ = language_service(&session, SRC_INDEX);
        session.wait_for_background_tasks();

        let calls = utils.client().publish_diagnostics_calls();
        assert!(
            calls.len() > initial_call_count,
            "expected additional PublishDiagnostics call after change"
        );

        // Find the last call for tsconfig.json
        let last_tsconfig_call = calls.iter().rev().find(|c| c.uri.0 == "file:///src/tsconfig.json");
        let last_tsconfig_call =
            last_tsconfig_call.expect("expected PublishDiagnostics call for tsconfig.json");
        // After fixing the error, there should be no program diagnostics
        assert_eq!(
            last_tsconfig_call.diagnostics.len(),
            0,
            "expected no diagnostics after removing baseUrl option"
        );
    }
}

child_test! {
    // Go: project_test.go:462 TestPushDiagnostics/updates diagnostics when a config file changes on disk with no follow-up request
    fn push_diagnostics_updates_diagnostics_when_a_config_file_changes_on_disk_with_no_follow_up_request() {
        let (session, utils) = projecttestutil::setup(files(&[
            ("/src/tsconfig.json", r#"{"compilerOptions": {}}"#),
            ("/src/index.ts", "export const x = 1;"),
        ]));
        open(&session, SRC_INDEX, "export const x = 1;");
        let _ = language_service(&session, SRC_INDEX);
        session.wait_for_background_tasks();

        let calls_before_change = utils.client().publish_diagnostics_calls().len();

        // Editors do not attach the language server to JSON documents, so a config file
        // edit only reaches the session through the file watcher. Config file diagnostics
        // are pushed, so they must be republished without waiting for a client request.
        utils
            .fs()
            .write_file("/src/tsconfig.json", r#"{"compilerOptions": {"target": "nope"}}"#)
            .unwrap();
        watch(&session, &[(CHANGED, "file:///src/tsconfig.json")]);
        session.wait_for_background_tasks();

        let calls = utils.client().publish_diagnostics_calls();
        let tsconfig_calls =
            filter_diagnostics_by_uri(&calls, "file:///src/tsconfig.json", calls_before_change);
        assert!(
            !tsconfig_calls.is_empty(),
            "expected PublishDiagnostics call for tsconfig.json after watched file change"
        );
        let last_tsconfig_call = &tsconfig_calls[tsconfig_calls.len() - 1];

        let expected_message = "Argument for '--target' option must be:";
        assert!(
            last_tsconfig_call
                .diagnostics
                .iter()
                .any(|diag| diag.message.as_string().contains(expected_message)),
            "expected invalid target diagnostic on tsconfig.json, got: {:?}",
            last_tsconfig_call.diagnostics
        );
    }
}

child_test! {
    // Go: project_test.go:383 TestPushDiagnostics/does not publish for inferred projects
    fn push_diagnostics_does_not_publish_for_inferred_projects() {
        let (session, utils) =
            projecttestutil::setup(files(&[("/src/index.ts", "let x: number = 'not a number';")]));
        open(&session, SRC_INDEX, "let x: number = 'not a number';");
        let _ = language_service(&session, SRC_INDEX);
        session.wait_for_background_tasks();

        let calls = utils.client().publish_diagnostics_calls();
        // Should not have any calls since inferred projects don't have tsconfig.json
        assert_eq!(calls.len(), 0, "expected no PublishDiagnostics calls for inferred projects");
    }
}

child_test! {
    // Go: project_test.go:605 TestPushDiagnostics/publishes global diagnostics after checking (ts#64452)
    fn push_diagnostics_publishes_global_diagnostics_after_checking() {
        let index = "export function f() {\n\t\t\t\tusing x = { [Symbol.dispose]() {} };\n\t\t\t}";
        let (session, utils) = projecttestutil::setup(files(&[
            (
                "/src/tsconfig.json",
                r#"{
				"compilerOptions": {
					"target": "es2020"
				}
			}"#,
            ),
            ("/src/index.ts", index),
        ]));
        open(&session, SRC_INDEX, index);
        // Request semantic diagnostics to trigger checking, which triggers the global type resolvers.
        let ctx = projecttestutil::with_request_id(&bg());
        let ls = session
            .get_language_service(&ctx, &uri(SRC_INDEX))
            .unwrap_or_else(|err| panic!("{}", err.error()));
        // Drain background tasks from DidOpenFile (publishProgramDiagnostics, etc.)
        // before triggering global diagnostics, to avoid racing with publishGlobalDiagnostics.
        session.wait_for_background_tasks();

        // ts#64452
        let diagnostics_ctx = || {
            core_context::with_checker_lifetime(
                &projecttestutil::with_request_id(&bg()),
                CheckerLifetime::DIAGNOSTICS,
            )
        };
        for _ in 0..2 {
            let program = ls.get_program();
            let file = program.get_source_file("/src/index.ts").expect("source file").root;
            let diags = ls_program::get_semantic_diagnostics(program, &diagnostics_ctx(), file);
            assert!(!diags.is_empty());
            for diag in &diags {
                assert_eq!(diag.file(), file);
            }

            let report = ls
                .provide_diagnostics(&diagnostics_ctx(), &uri(SRC_INDEX))
                .unwrap_or_else(|err| panic!("{}", err.error()));
            let report = report
                .full_document_diagnostic_report
                .expect("full document diagnostic report");
            let mut has_source_diag = false;
            for diag in &report.items {
                assert!(
                    !diag.message.as_string().contains("Cannot find global"),
                    "global diagnostic should only be published on tsconfig.json"
                );
                if diag.code.as_ref().and_then(|code| code.integer) == Some(2550) {
                    has_source_diag = true;
                }
            }
            assert!(has_source_diag, "expected the source diagnostic about Symbol.dispose");
        }
        // Enqueue global diagnostics publishing (normally done by the LSP server after each request).
        session.enqueue_publish_global_diagnostics();
        session.wait_for_background_tasks();

        let calls = utils.client().publish_diagnostics_calls();
        // Find the last call for tsconfig.json
        let last_tsconfig_call = calls.iter().rev().find(|c| c.uri.0 == "file:///src/tsconfig.json");
        let last_tsconfig_call =
            last_tsconfig_call.expect("expected PublishDiagnostics call for tsconfig.json");
        // Should have global diagnostics (e.g., Cannot find global type 'Disposable')
        let has_global_diag = last_tsconfig_call
            .diagnostics
            .iter()
            .any(|d| d.message.as_string().contains("Cannot find global"));
        assert!(
            has_global_diag,
            "expected a 'Cannot find global' diagnostic on tsconfig.json, got: {:?}",
            last_tsconfig_call.diagnostics
        );
    }
}

/// Go `TestPushDiagnostics/query globals {before,after} semantic checking`
/// (project_test.go:678, ts#64452): the loop body for one `checkFirst`.
fn push_diagnostics_query_globals(check_first: bool) {
    let name = if check_first {
        "query globals after semantic checking"
    } else {
        "query globals before semantic checking"
    };
    const URI: &str = "file:///src/repro.ts";
    const SOURCE: &str = "type Json = string | Json[];
type Parsed<T> = T extends object ? { [K in keyof T]: Parsed<T[K]> } : T;
declare function wrap<T>(value: T): Parsed<T>;
export const value = wrap({ items: [] as Json[] });";
    let ctx = bg();
    let (session, utils) = projecttestutil::setup(files(&[
        (
            "/src/tsconfig.json",
            r#"{"compilerOptions":{"strict":true,"noEmit":true}}"#,
        ),
        ("/src/repro.ts", SOURCE),
    ]));
    open(&session, URI, SOURCE);
    let service = session
        .get_language_service(&projecttestutil::with_request_id(&ctx), &uri(URI))
        .unwrap_or_else(|err| panic!("{}", err.error()));
    session.wait_for_background_tasks();

    let mut output = String::new();
    let mut record = |caption: &str, data: String| {
        output.push_str(&format!("// {caption}\n{data}\n\n"));
    };
    let marshal = |value: &dyn ts_goport::frontend::json::MarshalerTo| -> String {
        ts_goport::frontend::json::json_marshal_indent(value, "", "  ")
            .unwrap_or_else(|err| panic!("{err:?}"))
    };
    let check = |record: &mut dyn FnMut(&str, String)| {
        let report = service
            .provide_diagnostics(
                &core_context::with_checker_lifetime(
                    &projecttestutil::with_request_id(&ctx),
                    CheckerLifetime::DIAGNOSTICS,
                ),
                &uri(URI),
            )
            .unwrap_or_else(|err| panic!("{}", err.error()));
        record(
            "Document diagnostics",
            marshal(&report.full_document_diagnostic_report),
        );
    };
    if check_first {
        check(&mut record);
    }
    for _ in 0..2 {
        let before = utils.client().publish_diagnostics_calls().len();
        let hover = service
            .provide_hover(
                &projecttestutil::with_request_id(&ctx),
                &lsproto::HoverParams {
                    text_document: lsproto::TextDocumentIdentifier { uri: uri(URI) },
                    position: lsproto::Position {
                        line: 3,
                        character: 14,
                    },
                    ..Default::default()
                },
            )
            .unwrap_or_else(|err| panic!("{}", err.error()));
        record("Hover on value", marshal(&hover.hover));
        session.enqueue_publish_global_diagnostics();
        session.wait_for_background_tasks();
        let published: Vec<lsproto::PublishDiagnosticsParams> =
            utils.client().publish_diagnostics_calls()[before..].to_vec();
        record("Published after hover", marshal(&published));
    }
    check(&mut record);
    baseline::run(
        &format!("{}.jsonc", name.replace(' ', "-")),
        &output,
        &baseline::Options {
            subfolder: "project".into(),
            ..Default::default()
        },
    )
    .unwrap_or_else(|err| panic!("{err}"));
}

child_test! {
    // Go: project_test.go:680 TestPushDiagnostics/query globals before semantic checking (ts#64452)
    fn push_diagnostics_query_globals_before_semantic_checking() {
        push_diagnostics_query_globals(false);
    }
}

child_test! {
    // Go: project_test.go:680 TestPushDiagnostics/query globals after semantic checking (ts#64452)
    fn push_diagnostics_query_globals_after_semantic_checking() {
        push_diagnostics_query_globals(true);
    }
}

child_test! {
    // Go: project_test.go:453 TestPushDiagnostics/cleans tsconfig diagnostics after TS files close and restores them after TS file is reopened
    fn push_diagnostics_cleans_tsconfig_diagnostics_after_ts_files_close_and_restores_them_after_ts_file_is_reopened() {
        let (session, utils) = projecttestutil::setup(files(&[
            ("/src/tsconfig.json", BASE_URL_TSCONFIG),
            ("/src/index.ts", "export const x = 1;"),
        ]));
        open(&session, SRC_INDEX, "export const x = 1;");
        let _ = language_service(&session, SRC_INDEX);
        session.wait_for_background_tasks();

        let calls = utils.client().publish_diagnostics_calls();
        let tsconfig_calls = filter_diagnostics_by_uri(&calls, "file:///src/tsconfig.json", 0);
        assert!(
            !tsconfig_calls.is_empty(),
            "expected PublishDiagnostics call for tsconfig.json after opening file"
        );
        assert_eq!(
            tsconfig_calls[0].diagnostics.len(),
            1,
            "expected one diagnostic on tsconfig.json after opening file"
        );

        let calls_before_close = calls.len();

        close(&session, SRC_INDEX);
        session.wait_for_background_tasks();

        // Cleans up diagnostics after close
        let calls = utils.client().publish_diagnostics_calls();
        let clear_calls = filter_diagnostics_by_uri(&calls, "file:///src/tsconfig.json", calls_before_close);
        assert!(
            !clear_calls.is_empty(),
            "expected PublishDiagnostics call for tsconfig.json after project close"
        );
        assert_eq!(
            clear_calls.last().unwrap().diagnostics.len(),
            0,
            "expected empty diagnostics after project close"
        );

        let calls_before_reopen = calls.len();

        session.did_open_file(
            &bg(),
            &uri(SRC_INDEX),
            2,
            "export const x = 1;",
            &lsproto::LanguageKind::TYPE_SCRIPT,
        );
        let _ = language_service(&session, SRC_INDEX);
        session.wait_for_background_tasks();

        // Restores diagnostics after reopen
        let calls = utils.client().publish_diagnostics_calls();
        let reopened_calls = filter_diagnostics_by_uri(&calls, "file:///src/tsconfig.json", calls_before_reopen);
        assert!(
            !reopened_calls.is_empty(),
            "expected PublishDiagnostics call for tsconfig.json after reopening file"
        );
        assert_eq!(
            reopened_calls.last().unwrap().diagnostics.len(),
            1,
            "expected one diagnostic on tsconfig.json after reopening file"
        );
    }
}

child_test! {
    // Go: project_test.go:505 TestDisplayName/configured project returns relative config path
    fn display_name_configured_project_returns_relative_config_path() {
        let (session, _) = projecttestutil::setup(files(&[
            ("/home/projects/tsconfig.json", "{}"),
            ("/home/projects/index.ts", "export const x = 1;"),
        ]));
        open(&session, "file:///home/projects/index.ts", "export const x = 1;");
        let _ = language_service(&session, "file:///home/projects/index.ts");

        let configured = session
            .snapshot()
            .project_collection
            .configured_project(&path("/home/projects/tsconfig.json"))
            .expect("configured project");
        assert_eq!(configured.borrow().display_name("/home/projects"), "tsconfig.json");
    }
}

child_test! {
    // Go: project_test.go:522 TestDisplayName/configured project with nested config
    fn display_name_configured_project_with_nested_config() {
        let (session, _) = projecttestutil::setup(files(&[
            ("/home/projects/sub/tsconfig.json", "{}"),
            ("/home/projects/sub/index.ts", "export const x = 1;"),
        ]));
        open(&session, "file:///home/projects/sub/index.ts", "export const x = 1;");
        let _ = language_service(&session, "file:///home/projects/sub/index.ts");

        let configured = session
            .snapshot()
            .project_collection
            .configured_project(&path("/home/projects/sub/tsconfig.json"))
            .expect("configured project");
        assert_eq!(configured.borrow().display_name("/home/projects"), "sub/tsconfig.json");
    }
}

child_test! {
    // Go: project_test.go:819 TestDisplayName/configured project preserves config path casing (ts#64319)
    fn display_name_configured_project_preserves_config_path_casing() {
        let (session, _) = projecttestutil::setup(files(&[
            ("/home/projects/Project/tsconfig.json", "{}"),
            ("/home/projects/Project/index.ts", "export const x = 1;"),
        ]));
        open(&session, "file:///home/projects/Project/index.ts", "export const x = 1;");
        let _ = language_service(&session, "file:///home/projects/Project/index.ts");

        let configured = session
            .snapshot()
            .project_collection
            .configured_project(&path("/home/projects/project/tsconfig.json"))
            .expect("configured project");
        assert_eq!(configured.borrow().display_name("/home/projects"), "Project/tsconfig.json");
    }
}

child_test! {
    // Go: project_test.go:539 TestDisplayName/inferred project returns directory base name
    fn display_name_inferred_project_returns_directory_base_name() {
        let (session, _) = projecttestutil::setup_with_options(
            files(&[("/home/projects/index.ts", "export const x = 1;")]),
            ts_goport::project::SessionOptions {
                typings_location: String::new(),
                push_diagnostics_enabled: true,
                ..projecttestutil::session_options("/home/projects")
            },
        );
        open(&session, "file:///home/projects/index.ts", "export const x = 1;");
        let _ = language_service(&session, "file:///home/projects/index.ts");

        let inferred = session
            .snapshot()
            .project_collection
            .inferred_project()
            .expect("inferred project");
        let name = inferred.borrow().display_name("/home");
        assert_eq!(name, "projects");
    }
}

fn is_project_0(call: &ProgressCall) -> bool {
    std::ptr::eq(call.message, diag::Project_0)
}

child_test! {
    // Go: project_test.go:570 TestProgressNotifications/emits progress for configured project loading
    fn progress_emits_progress_for_configured_project_loading() {
        let (session, utils) = projecttestutil::setup(files(&[
            ("/home/projects/tsconfig.json", "{}"),
            ("/home/projects/index.ts", "export const x = 1;"),
        ]));
        open(&session, "file:///home/projects/index.ts", "export const x = 1;");
        let _ = language_service(&session, "file:///home/projects/index.ts");

        let start_calls = utils.client().progress_start_calls();
        let finish_calls = utils.client().progress_finish_calls();

        assert!(!start_calls.is_empty(), "expected at least one ProgressStart call");
        assert!(!finish_calls.is_empty(), "expected at least one ProgressFinish call");

        assert!(
            start_calls.iter().any(is_project_0),
            "expected ProgressStart with Project_0 message"
        );
        assert!(
            finish_calls.iter().any(is_project_0),
            "expected ProgressFinish with Project_0 message"
        );
    }
}

child_test! {
    // Go: project_test.go:606 TestProgressNotifications/emits progress for inferred project loading
    fn progress_emits_progress_for_inferred_project_loading() {
        let (session, utils) =
            projecttestutil::setup(files(&[("/home/projects/index.ts", "export const x = 1;")]));
        open(&session, "file:///home/projects/index.ts", "export const x = 1;");
        let _ = language_service(&session, "file:///home/projects/index.ts");

        let start_calls = utils.client().progress_start_calls();
        let finish_calls = utils.client().progress_finish_calls();

        assert!(!start_calls.is_empty(), "expected at least one ProgressStart call");
        assert!(!finish_calls.is_empty(), "expected at least one ProgressFinish call");

        assert!(
            start_calls.iter().any(is_project_0),
            "expected ProgressStart with Project_0 message"
        );
    }
}

child_test! {
    // Go: project_test.go:632 TestProgressNotifications/each start has a matching finish
    fn progress_each_start_has_a_matching_finish() {
        let (session, utils) = projecttestutil::setup(files(&[
            ("/home/projects/tsconfig.json", "{}"),
            ("/home/projects/a.ts", "export const a = 1;"),
            ("/home/projects/b.ts", "export const b = 2;"),
        ]));
        open(&session, "file:///home/projects/a.ts", "export const a = 1;");
        let _ = language_service(&session, "file:///home/projects/a.ts");

        let starts = utils.client().progress_start_calls().iter().filter(|c| is_project_0(c)).count();
        let finishes = utils.client().progress_finish_calls().iter().filter(|c| is_project_0(c)).count();
        assert_eq!(
            starts, finishes,
            "ProgressStart and ProgressFinish calls for Project_0 should be balanced"
        );
    }
}
