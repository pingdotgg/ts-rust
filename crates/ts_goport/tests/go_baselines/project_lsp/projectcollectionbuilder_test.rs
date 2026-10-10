//! Port of Go `internal/project/projectcollectionbuilder_test.go` (`TestProjectCollectionBuilder`).

use ts_goport::lsp::lsproto;
use ts_goport::project::Kind;

use super::projecttestutil::{self, FileMap, files};
use super::util::*;

const MAIN: &str = "/user/username/projects/myproject/src/main.ts";
const MAIN_URI: &str = "file:///user/username/projects/myproject/src/main.ts";
const DUMMY_URI: &str = "file:///user/username/workspaces/dummy/dummy.ts";
const MP: &str = "/user/username/projects/myproject";

fn file_text(files: &FileMap, name: &str) -> String {
    String::from_utf8(files[name].data.clone()).unwrap()
}

fn cfg(name: &str) -> String {
    format!("{MP}/{name}")
}

/// Close `u`, open the dummy file, and check that only the inferred project remains.
fn close_and_open_dummy(session: &std::rc::Rc<ts_goport::project::Session>, u: &str) {
    close(session, u);
    open(session, DUMMY_URI, "const x = 1;");
    assert_eq!(projects_len(session), 1);
    assert!(has_inferred_project(session));
}

child_test! {
    // Go: projectcollectionbuilder_test.go:26 TestProjectCollectionBuilder/when project found is solution referencing default project directly
    fn when_project_found_is_solution_referencing_default_project_directly() {
        let files = files_for_solution_config_file(&["./tsconfig-src.json"], "", &[]);
        let (session, _) = projecttestutil::setup(files.clone());
        let content = file_text(&files, MAIN);

        // Ensure configured project is found for open file
        open(&session, MAIN_URI, &content);
        let snapshot = session.snapshot();
        assert_eq!(projects_len(&session), 1);
        assert!(has_configured_project(&session, &cfg("tsconfig-src.json")));

        // Ensure request can use existing snapshot
        let _ = language_service(&session, MAIN_URI);
        let request_snapshot = session.snapshot();
        assert!(std::rc::Rc::ptr_eq(&request_snapshot, &snapshot));

        // Searched configs should be present while file is open
        assert!(has_config(&session, &cfg("tsconfig.json")), "solution config should be present");
        assert!(has_config(&session, &cfg("tsconfig-src.json")), "direct reference should be present");

        // Close the file and open one in an inferred project
        close_and_open_dummy(&session, MAIN_URI);

        // Config files should have been released
        assert!(!has_config(&session, &cfg("tsconfig.json")));
        assert!(!has_config(&session, &cfg("tsconfig-src.json")));
    }
}

child_test! {
    // Go: projectcollectionbuilder_test.go:62 TestProjectCollectionBuilder/when project found is solution referencing default project indirectly
    fn when_project_found_is_solution_referencing_default_project_indirectly() {
        let mut files = files_for_solution_config_file(
            &["./tsconfig-indirect1.json", "./tsconfig-indirect2.json"],
            "",
            &[],
        );
        apply_indirect_project_files(&mut files, 1, "");
        apply_indirect_project_files(&mut files, 2, "");
        let (session, _) = projecttestutil::setup(files.clone());
        let content = file_text(&files, MAIN);

        // Ensure configured project is found for open file
        open(&session, MAIN_URI, &content);
        assert_eq!(projects_len(&session), 1);
        let src_project = configured_project(&session, &cfg("tsconfig-src.json")).expect("src project");

        // Verify the default project is the source project
        assert!(default_project_is(&session, MAIN_URI, &src_project));

        // Searched configs should be present while file is open
        assert!(has_config(&session, &cfg("tsconfig.json")), "solution config should be present");
        assert!(has_config(&session, &cfg("tsconfig-indirect1.json")), "direct reference should be present");
        assert!(has_config(&session, &cfg("tsconfig-src.json")), "indirect reference should be present");

        // Close the file and open one in an inferred project
        close_and_open_dummy(&session, MAIN_URI);

        // Config files should be released
        assert!(!has_config(&session, &cfg("tsconfig.json")));
        assert!(!has_config(&session, &cfg("tsconfig-src.json")));
        assert!(!has_config(&session, &cfg("tsconfig-indirect1.json")));
        assert!(!has_config(&session, &cfg("tsconfig-indirect2.json")));
    }
}

child_test! {
    // Go: projectcollectionbuilder_test.go:102 TestProjectCollectionBuilder/when project found is solution with disableReferencedProjectLoad referencing default project directly
    fn when_project_found_is_solution_with_disable_referenced_project_load_referencing_default_project_directly() {
        let files = files_for_solution_config_file(
            &["./tsconfig-src.json"],
            r#""disableReferencedProjectLoad": true"#,
            &[],
        );
        let (session, _) = projecttestutil::setup(files.clone());
        let content = file_text(&files, MAIN);

        // Ensure no configured project is created due to disableReferencedProjectLoad
        open(&session, MAIN_URI, &content);
        assert_eq!(projects_len(&session), 1);
        assert!(!has_configured_project(&session, &cfg("tsconfig-src.json")));

        // Should use inferred project instead
        assert_eq!(default_project_kind(&session, MAIN_URI), Kind::INFERRED);

        // Searched configs should be present while file is open
        assert!(has_config(&session, &cfg("tsconfig.json")), "solution config should be present");
        assert!(!has_config(&session, &cfg("tsconfig-src.json")), "direct reference should not be present");

        // Close the file and open another one in the inferred project
        close_and_open_dummy(&session, MAIN_URI);

        // Config files should be released
        assert!(!has_config(&session, &cfg("tsconfig.json")));
        assert!(!has_config(&session, &cfg("tsconfig-src.json")));
    }
}

child_test! {
    // Go: projectcollectionbuilder_test.go:137 TestProjectCollectionBuilder/when project found is solution referencing default project indirectly through disableReferencedProjectLoad
    fn when_project_found_is_solution_referencing_default_project_indirectly_through_disable_referenced_project_load() {
        let mut files = files_for_solution_config_file(&["./tsconfig-indirect1.json"], "", &[]);
        apply_indirect_project_files(&mut files, 1, r#""disableReferencedProjectLoad": true"#);
        let (session, _) = projecttestutil::setup(files.clone());
        let content = file_text(&files, MAIN);

        // Ensure no configured project is created due to disableReferencedProjectLoad in indirect project
        open(&session, MAIN_URI, &content);
        assert_eq!(projects_len(&session), 1);
        assert!(!has_configured_project(&session, &cfg("tsconfig-src.json")));

        // Should use inferred project instead
        assert_eq!(default_project_kind(&session, MAIN_URI), Kind::INFERRED);

        // Searched configs should be present while file is open
        assert!(has_config(&session, &cfg("tsconfig.json")), "solution config should be present");
        assert!(
            has_config(&session, &cfg("tsconfig-indirect1.json")),
            "solution direct reference should be present"
        );
        assert!(!has_config(&session, &cfg("tsconfig-src.json")), "indirect reference should not be present");

        // Close the file and open another one in the inferred project
        close_and_open_dummy(&session, MAIN_URI);

        // Config files should be released
        assert!(!has_config(&session, &cfg("tsconfig.json")));
        assert!(!has_config(&session, &cfg("tsconfig-src.json")));
        assert!(!has_config(&session, &cfg("tsconfig-indirect1.json")));
    }
}

child_test! {
    // Go: projectcollectionbuilder_test.go:175 TestProjectCollectionBuilder/when project found is solution referencing default project indirectly through disableReferencedProjectLoad in one but without it in another
    fn when_project_found_is_solution_referencing_default_project_indirectly_through_disable_referenced_project_load_in_one_but_without_it_in_another() {
        let mut files = files_for_solution_config_file(
            &["./tsconfig-indirect1.json", "./tsconfig-indirect2.json"],
            "",
            &[],
        );
        apply_indirect_project_files(&mut files, 1, r#""disableReferencedProjectLoad": true"#);
        apply_indirect_project_files(&mut files, 2, "");
        let (session, _) = projecttestutil::setup(files.clone());
        let content = file_text(&files, MAIN);

        // Ensure configured project is found through the indirect project without disableReferencedProjectLoad
        open(&session, MAIN_URI, &content);
        assert_eq!(projects_len(&session), 1);
        let src_project = configured_project(&session, &cfg("tsconfig-src.json")).expect("src project");

        // Verify the default project is the source project (found through indirect2, not indirect1)
        assert!(default_project_is(&session, MAIN_URI, &src_project));

        // Searched configs should be present while file is open
        assert!(has_config(&session, &cfg("tsconfig.json")), "solution config should be present");
        assert!(has_config(&session, &cfg("tsconfig-indirect1.json")), "direct reference 1 should be present");
        assert!(has_config(&session, &cfg("tsconfig-indirect2.json")), "direct reference 2 should be present");
        assert!(has_config(&session, &cfg("tsconfig-src.json")), "indirect reference should be present");

        // Close the file and open another one in the inferred project
        close_and_open_dummy(&session, MAIN_URI);

        // Config files should be released
        assert!(!has_config(&session, &cfg("tsconfig.json")));
        assert!(!has_config(&session, &cfg("tsconfig-src.json")));
        assert!(!has_config(&session, &cfg("tsconfig-indirect1.json")));
        assert!(!has_config(&session, &cfg("tsconfig-indirect2.json")));
    }
}

child_test! {
    // Go: projectcollectionbuilder_test.go:216 TestProjectCollectionBuilder/when project found is project with own files referencing the file from referenced project
    fn when_project_found_is_project_with_own_files_referencing_the_file_from_referenced_project() {
        let mut files = files_for_solution_config_file(&["./tsconfig-src.json"], "", &[r#""./own/main.ts""#]);
        files.insert(
            cfg("own/main.ts"),
            "
			import { foo } from '../src/main';
			foo;
			export function bar() {}
		"
            .into(),
        );
        let (session, _) = projecttestutil::setup(files.clone());
        let content = file_text(&files, MAIN);

        // Ensure configured project is found for open file - should load both projects
        open(&session, MAIN_URI, &content);
        assert_eq!(projects_len(&session), 2);
        let src_project = configured_project(&session, &cfg("tsconfig-src.json")).expect("src project");
        assert!(has_configured_project(&session, &cfg("tsconfig.json")));

        // Verify the default project is the source project
        assert!(default_project_is(&session, MAIN_URI, &src_project));

        // Searched configs should be present while file is open
        assert!(has_config(&session, &cfg("tsconfig.json")), "solution config should be present");
        assert!(has_config(&session, &cfg("tsconfig-src.json")), "direct reference should be present");

        // Close the file and open another one in the inferred project
        close_and_open_dummy(&session, MAIN_URI);

        // Config files should be released
        assert!(!has_config(&session, &cfg("tsconfig.json")));
        assert!(!has_config(&session, &cfg("tsconfig-src.json")));
    }
}

child_test! {
    // Go: projectcollectionbuilder_test.go:258 TestProjectCollectionBuilder/when file is not part of first config tree found, looks into ancestor folder and its references to find default project
    fn when_file_is_not_part_of_first_config_tree_found_looks_into_ancestor_folder_and_its_references_to_find_default_project() {
        let demos = "
                import * as helpers from 'demos/helpers';
                export const demo = () => {
                    helpers;
                }
            ";
        let files = files(&[
            ("/home/src/projects/project/app/Component-demos.ts", demos),
            ("/home/src/projects/project/app/Component.ts", "export const Component = () => {}"),
            (
                "/home/src/projects/project/app/tsconfig.json",
                r#"{
				"compilerOptions": {
					"composite": true,
					"outDir": "../app-dist/",
				},
				"include": ["**/*"],
				"exclude": ["**/*-demos.*"],
			}"#,
            ),
            ("/home/src/projects/project/demos/helpers.ts", "export const foo = 1;"),
            (
                "/home/src/projects/project/demos/tsconfig.json",
                r#"{
				"compilerOptions": {
					"composite": true,
					"rootDir": "../",
					"outDir": "../demos-dist/",
					"paths": {
						"demos/*": ["./*"],
					},
				},
				"include": [
					"**/*",
					"../app/**/*-demos.*",
				],
			}"#,
            ),
            (
                "/home/src/projects/project/tsconfig.json",
                r#"{
				"compilerOptions": {
					"outDir": "./dist/",
				},
				"references": [
					{ "path": "./demos/tsconfig.json" },
					{ "path": "./app/tsconfig.json" },
				],
				"files": []
			}"#,
            ),
        ]);
        let (session, _) = projecttestutil::setup(files);
        let u = "file:///home/src/projects/project/app/Component-demos.ts";

        // Ensure configured project is found for open file
        open(&session, u, demos);
        assert_eq!(projects_len(&session), 2);
        let demo_project = configured_project(&session, "/home/src/projects/project/demos/tsconfig.json")
            .expect("demo project");
        assert!(has_configured_project(&session, "/home/src/projects/project/tsconfig.json"));

        // Verify the default project is the demos project (not the app project that excludes demos files)
        assert!(default_project_is(&session, u, &demo_project));

        // Searched configs should be present while file is open
        assert!(has_config(&session, "/home/src/projects/project/app/tsconfig.json"), "app config should be present");
        assert!(has_config(&session, "/home/src/projects/project/demos/tsconfig.json"), "demos config should be present");
        assert!(has_config(&session, "/home/src/projects/project/tsconfig.json"), "solution config should be present");

        // Close the file and open another one in the inferred project
        close_and_open_dummy(&session, u);

        // Config files should be released
        assert!(!has_config(&session, "/home/src/projects/project/app/tsconfig.json"));
        assert!(!has_config(&session, "/home/src/projects/project/demos/tsconfig.json"));
        assert!(!has_config(&session, "/home/src/projects/project/tsconfig.json"));
    }
}

child_test! {
    // Go: projectcollectionbuilder_test.go:338 TestProjectCollectionBuilder/when dts file is next to ts file and included as root in referenced project
    fn when_dts_file_is_next_to_ts_file_and_included_as_root_in_referenced_project() {
        let index_dts = "
                 declare global {
                    interface Window {
                        electron: ElectronAPI
                        api: unknown
                    }
                }
            ";
        let files = files(&[
            ("/home/src/projects/project/src/index.d.ts", index_dts),
            ("/home/src/projects/project/src/index.ts", "const api = {}"),
            (
                "/home/src/projects/project/tsconfig.json",
                r#"{
				"include": [
					"src/*.d.ts",
				],
				"references": [{ "path": "./tsconfig.node.json" }],
			}"#,
            ),
            (
                "/home/src/projects/project/tsconfig.node.json",
                r#"{
				"include": ["src/**/*"],
                "compilerOptions": {
                    "composite": true,
                },
			}"#,
            ),
        ]);
        let (session, _) = projecttestutil::setup(files);
        let u = "file:///home/src/projects/project/src/index.d.ts";

        // Ensure configured projects are found for open file
        open(&session, u, index_dts);
        assert_eq!(projects_len(&session), 2);
        assert!(has_configured_project(&session, "/home/src/projects/project/tsconfig.json"));

        // Verify the default project is inferred
        assert_eq!(default_project_kind(&session, u), Kind::INFERRED);

        // Searched configs should be present while file is open
        assert!(has_config(&session, "/home/src/projects/project/tsconfig.json"), "root config should be present");
        assert!(has_config(&session, "/home/src/projects/project/tsconfig.node.json"), "node config should be present");

        // Close the file and open another one in the inferred project
        close_and_open_dummy(&session, u);

        // Config files should be released
        assert!(!has_config(&session, "/home/src/projects/project/tsconfig.json"));
        assert!(!has_config(&session, "/home/src/projects/project/tsconfig.node.json"));
    }
}

child_test! {
    // Go: projectcollectionbuilder_test.go:397 TestProjectCollectionBuilder/#1630
    fn issue_1630() {
        let files = files(&[
            (
                "/project/lib/tsconfig.json",
                r#"{
				"files": ["a.ts"]
			}"#,
            ),
            ("/project/lib/a.ts", "export const a = 1;"),
            ("/project/lib/b.ts", "export const b = 1;"),
            (
                "/project/tsconfig.json",
                r#"{
				"files": [],
				"references": [{ "path": "./lib" }],
				"compilerOptions": {
					"disableReferencedProjectLoad": true
				}
			}"#,
            ),
            ("/project/index.ts", ""),
        ]);

        let (session, _) = projecttestutil::setup(files);

        // opening b.ts puts /project/lib/tsconfig.json in the config file registry and creates the project,
        // but the project is ultimately not a match
        open(&session, "file:///project/lib/b.ts", "export const b = 1;");
        // opening an unrelated file triggers cleanup of /project/lib/tsconfig.json since no open file is part of that project,
        // but will keep the config file in the registry since lib/b.ts is still open
        open(&session, "untitled:Untitled-1", "");
        // Opening index.ts searches /project/tsconfig.json and then checks /project/lib/tsconfig.json without opening it.
        open(&session, "file:///project/index.ts", "");
    }
}

child_test! {
    // Go: projectcollectionbuilder_test.go:428 TestProjectCollectionBuilder/inferred project root files are in stable order
    fn inferred_project_root_files_are_in_stable_order() {
        let (session, _) = projecttestutil::setup(files(&[
            ("/project/a.ts", "export const a = 1;"),
            ("/project/b.ts", "export const b = 1;"),
            ("/project/c.ts", "export const c = 1;"),
        ]));

        // b, c, a
        open(&session, "file:///project/b.ts", "export const b = 1;");
        open(&session, "file:///project/c.ts", "export const c = 1;");
        open(&session, "file:///project/a.ts", "export const a = 1;");

        assert_eq!(projects_len(&session), 1);
        let inferred_project = session
            .snapshot()
            .project_collection
            .inferred_project()
            .expect("inferred project");
        let program = inferred_project.borrow().program.clone().expect("program");
        assert_eq!(
            program.command_line().file_names(),
            ["/project/a.ts", "/project/b.ts", "/project/c.ts"]
        );
    }
}

child_test! {
    // Go: projectcollectionbuilder_test.go:457 TestProjectCollectionBuilder/project lookup terminates
    fn project_lookup_terminates() {
        let (session, _) = projecttestutil::setup(files(&[
            (
                "/tsconfig.json",
                r#"{
				"files": [],
				"references": [
					{
						"path": "./packages/pkg1"
					},
					{
						"path": "./packages/pkg2"
					},
				]
			}"#,
            ),
            (
                "/packages/pkg1/tsconfig.json",
                r#"{
				"include": ["src/**/*.ts"],
				"compilerOptions": {
					"composite": true,
				},
				"references": [
					{
						"path": "../pkg2"
					},
				]
			}"#,
            ),
            (
                "/packages/pkg2/tsconfig.json",
                r#"{
				"include": ["src/**/*.ts"],
				"compilerOptions": {
					"composite": true,
				},
				"references": [
					{
						"path": "../pkg1"
					},
				]
			}"#,
            ),
            ("/script.ts", "export const a = 1;"),
        ]));
        open(&session, "file:///script.ts", "export const a = 1;");
        // Test should terminate
    }
}

child_test! {
    // Go: projectcollectionbuilder_test.go:500 TestProjectCollectionBuilder/file moves to inferred project after import is deleted
    fn file_moves_to_inferred_project_after_import_is_deleted() {
        let (session, _) = projecttestutil::setup(files(&[
            ("/project/tsconfig.json", r#"{"compilerOptions": {"strict": true}}"#),
            ("/project/index.ts", r#"import { helper } from "./node_modules/dep/index";"#),
            ("/project/node_modules/dep/index.d.ts", "export declare function helper(): void;"),
        ]));

        // Step 1: Open the project root file
        let root_uri = "file:///project/index.ts";
        open(&session, root_uri, r#"import { helper } from "./node_modules/dep/index";"#);
        let _ = language_service(&session, root_uri);

        // Step 2: Open the node_modules dependency file - should be in the configured project
        let dep_uri = "file:///project/node_modules/dep/index.d.ts";
        open(&session, dep_uri, "export declare function helper(): void;");

        let configured = configured_project(&session, "/project/tsconfig.json")
            .expect("configured project should exist");
        assert!(
            default_project_is(&session, dep_uri, &configured),
            "dependency should be in the configured project initially"
        );

        // Step 3: Delete the import from the root file
        session.did_change_file(
            &bg(),
            &uri(root_uri),
            2,
            &[lsproto::TextDocumentContentChangePartialOrWholeDocument {
                partial: None,
                whole_document: Some(lsproto::TextDocumentContentChangeWholeDocument {
                    text: "// import removed".to_string(),
                }),
            }],
        );

        // Step 4: Request language service for the dependency - it should now be in an inferred project
        let _ = language_service(&session, dep_uri);

        assert_eq!(
            default_project_kind(&session, dep_uri),
            Kind::INFERRED,
            "dependency should be in an inferred project after import is deleted"
        );
    }
}

child_test! {
    // Go: projectcollectionbuilder_test.go:544 TestProjectCollectionBuilder/should update project on package.json change
    fn should_update_project_on_package_json_change() {
        let index = r##"import { add } from "#utils";"##;
        let (session, utils) = projecttestutil::setup(files(&[
            (
                "/home/projects/myproject/tsconfig.json",
                r#"{
				"compilerOptions": {
					"module": "nodenext",
					"moduleResolution": "nodenext",
					"noLib": true,
					"noEmit": true
				}
			}"#,
            ),
            (
                "/home/projects/myproject/package.json",
                r##"{
				"name": "myproject",
				"type": "module",
				"imports": {
					"#utils": "./src/utils.ts"
				}
			}"##,
            ),
            ("/home/projects/myproject/src/index.ts", index),
            (
                "/home/projects/myproject/src/utils.ts",
                "export function add(a: number, b: number) { return a + b; }",
            ),
        ]));
        let index_uri = "file:///home/projects/myproject/src/index.ts";
        open(&session, index_uri, index);

        // Verify initial state: #utils resolves to utils.ts, so utils.ts is in the program
        let p = program(&session, index_uri);
        assert_eq!(all_sem_diag_count(&p), 0, "should have no diagnostics with correct package.json");

        // Now change the package.json to point #utils at a non-existent file
        utils
            .fs()
            .write_file(
                "/home/projects/myproject/package.json",
                r##"{
			"name": "myproject",
			"type": "module",
			"imports": {
				"#utils": "./src/nonexistent.ts"
			}
		}"##,
            )
            .unwrap();
        watch(&session, &[(CHANGED, "file:///home/projects/myproject/package.json")]);

        let updated_program = program(&session, index_uri);
        assert_eq!(
            all_sem_diag_count(&updated_program),
            1,
            "should have diagnostics after package.json change"
        );
    }
}

child_test! {
    // PORT: Go's project search starts every reference of a BFS level on its
    // own goroutine (`core/bfs.go:102`), so after the build project finds a
    // file, Go still creates the spec project that also includes it (hono's
    // tsconfig.build.json and tsconfig.spec.json). After an open, the cleanup
    // deletes it again. A project tree request for a closed file
    // (willRenameFiles, `lsp/server.go` handleWillRenameFilesWorker) has no
    // cleanup, so the spec project stays and the answer has the test files.
    fn solution_search_creates_every_project_of_the_level() {
        const HELPER_URI: &str = "file:///user/username/projects/myproject/src/helper.ts";
        let files = files(&[
            (
                "/user/username/projects/myproject/tsconfig.json",
                r#"{
			"files": [],
			"references": [{ "path": "./tsconfig.build.json" }, { "path": "./tsconfig.spec.json" }]
		}"#,
            ),
            (
                "/user/username/projects/myproject/tsconfig.build.json",
                r#"{ "include": ["src/**/*.ts"], "exclude": ["src/**/*.test.ts"] }"#,
            ),
            (
                "/user/username/projects/myproject/tsconfig.spec.json",
                r#"{ "include": ["src/**/*.ts"] }"#,
            ),
            (MAIN, "export const foo = 1;"),
            (
                "/user/username/projects/myproject/src/helper.ts",
                "export const bar = 2;",
            ),
            (
                "/user/username/projects/myproject/src/main.test.ts",
                "import { foo } from './main';\nfoo;",
            ),
        ]);
        let (session, _) = projecttestutil::setup(files.clone());
        let content = file_text(&files, MAIN);

        // The open creates both projects; the cleanup keeps only the default one.
        open(&session, MAIN_URI, &content);
        let build_project =
            configured_project(&session, &cfg("tsconfig.build.json")).expect("build project");
        assert!(default_project_is(&session, MAIN_URI, &build_project));
        assert!(!has_configured_project(&session, &cfg("tsconfig.spec.json")));
        assert_eq!(projects_len(&session), 1);

        // The search for a closed file visits every project of the level.
        let services =
            session.get_language_services_for_documents_loading_project_tree(&bg(), &[uri(HELPER_URI)]);
        assert_eq!(services.len(), 2);
        assert!(
            has_configured_project(&session, &cfg("tsconfig.spec.json")),
            "spec project should be created by the same search level"
        );
        assert_eq!(projects_len(&session), 2);
    }
}

child_test! {
    // PORT: not in Go. The open in the layout above makes a program for the
    // spec project, and the cleanup of the same snapshot clone deletes the
    // project. No snapshot holds that program, so `dispose` never frees it.
    // Go's GC frees it; the port frees it at the end of the clone
    // (`Snapshot::clone`, `ProjectCollectionBuilder::made_programs`). Else
    // each open in hono keeps a whole spec program.
    fn deleted_project_of_a_clone_releases_its_program() {
        let files = files(&[
            (
                "/user/username/projects/myproject/tsconfig.json",
                r#"{
			"files": [],
			"references": [{ "path": "./tsconfig.build.json" }, { "path": "./tsconfig.spec.json" }]
		}"#,
            ),
            (
                "/user/username/projects/myproject/tsconfig.build.json",
                r#"{ "include": ["src/**/*.ts"], "exclude": ["src/**/*.test.ts"] }"#,
            ),
            (
                "/user/username/projects/myproject/tsconfig.spec.json",
                r#"{ "include": ["src/**/*.ts"] }"#,
            ),
            (MAIN, "export const foo = 1;"),
            (
                "/user/username/projects/myproject/src/main.test.ts",
                "import { foo } from './main';\nfoo;",
            ),
        ]);
        let (session, _) = projecttestutil::setup(files.clone());
        let content = file_text(&files, MAIN);

        open(&session, MAIN_URI, &content);
        assert!(!has_configured_project(&session, &cfg("tsconfig.spec.json")));
        assert_eq!(projects_len(&session), 1);
        assert_eq!(
            ts_goport::program::ls_program::registered_programs(),
            1,
            "only the build project's program should stay registered"
        );
        // The clone froze the spec project's host too, so the host holds
        // neither the project nor the builder.
        assert_eq!(
            ts_goport::project::unfrozen_compiler_hosts(),
            0,
            "every host that the clone made should be frozen"
        );
    }
}

child_test! {
    // PORT: not in Go. One clone updates the inferred project twice: the
    // edit of a.ts makes the request for a.ts update its program
    // (projectcollectionbuilder.go:658), and the request for the closed b.ts,
    // which no config holds, adds it as a root and updates it again
    // (:626 ensureInferredProjectIncludesClosedFile). Go's GC frees the
    // first program and its host; the port releases the first program at
    // the end of the clone (`Snapshot::clone`,
    // `ProjectCollectionBuilder::made_programs`), and the second program,
    // which the project keeps, still answers. With push diagnostics on (the
    // `projecttestutil::setup` default), the queued diagnostics task of a
    // snapshot holds that snapshot's programs until it runs, as Go's task
    // keeps its pointer: before the wait for the background tasks the
    // program of the first snapshot is registered too, after it only the
    // last program is.
    fn project_updated_twice_in_a_clone_releases_its_first_program() {
        const A: &str = "/home/projects/loose/a.ts";
        const A_URI: &str = "file:///home/projects/loose/a.ts";
        const B: &str = "/home/projects/loose/b.ts";
        const B_URI: &str = "file:///home/projects/loose/b.ts";
        let a = "export const a = 1;";
        let (session, _) =
            projecttestutil::setup(files(&[(A, a), (B, "export const b: number = 2;")]));
        open(&session, A_URI, a);
        assert_eq!(default_project_kind(&session, A_URI), Kind::INFERRED);
        assert_eq!(ts_goport::program::ls_program::registered_programs(), 1);

        edit(&session, A_URI, 2, (0, 0), (0, 0), "// edited\n");
        // ts#64642: a request that also loads the project trees ends with
        // the inferred project's roots, the open files not in a configured
        // project (projectcollectionbuilder.go:763 cleanupInferredProject),
        // which drops the closed b.ts again. So the clone asks for the two
        // documents only (Go `ResourceRequest{Documents: uris}`).
        session.get_snapshot(
            &bg(),
            ts_goport::project::ResourceRequest {
                documents: vec![uri(A_URI), uri(B_URI)],
                ..Default::default()
            },
            false, /*callerRef*/
        );
        let program = program(&session, A_URI);
        assert!(has_file(&program, B), "b.ts should be a root of the inferred project");
        assert_eq!(sem_diag_count(&program, B), 0);
        // The background tasks run on this thread, and only the wait below
        // runs them (`background::Queue::wait`), so this count does not
        // depend on timing: the program of the first snapshot waits for
        // its queued diagnostics task, the project keeps the last program,
        // and the first program of the clone is released already.
        assert_eq!(
            ts_goport::program::ls_program::registered_programs(),
            2,
            "the first snapshot's program and the last program should be registered before the wait"
        );
        session.wait_for_background_tasks();
        assert_eq!(
            ts_goport::program::ls_program::registered_programs(),
            1,
            "only the inferred project's last program should stay registered"
        );
        assert_eq!(
            ts_goport::project::unfrozen_compiler_hosts(),
            0,
            "no host of the clone should keep its project and the builder"
        );
    }
}

child_test! {
    // PORT: Go builds the symlink cache on first use (compiler/program.go:2300),
    // after the project host is frozen (project/compilerhost.go:57), so the
    // resolution of a package.json dependency that is not installed is not
    // tracked. A Created event for node_modules/<dependency> then does not
    // mark the project dirty (projectcollectionbuilder.go:1574, editfuzz3 H3).
    fn created_dependency_directory_keeps_program() {
        let index = "export const x = 1;";
        let (session, _) = projecttestutil::setup(files(&[
            ("/home/projects/myproject/tsconfig.json", "{}"),
            (
                "/home/projects/myproject/package.json",
                r#"{ "name": "myproject", "dependencies": { "zlibx": "^1.0.0" } }"#,
            ),
            (
                "/home/projects/myproject/node_modules/other/package.json",
                r#"{ "name": "other" }"#,
            ),
            ("/home/projects/myproject/src/index.ts", index),
        ]));
        let index_uri = "file:///home/projects/myproject/src/index.ts";
        open(&session, index_uri, index);
        let before = program(&session, index_uri);

        watch(
            &session,
            &[(CREATED, "file:///home/projects/myproject/node_modules/zlibx")],
        );
        let after = program(&session, index_uri);
        assert!(
            same_program(&before, &after),
            "a Created event for an uninstalled dependency should not rebuild the program"
        );
    }
}

child_test! {
    env &[("GOPORT_BIND_THREADS", "2")];
    // PORT: when one clone makes the program and builds its auto-import
    // bucket, Go's registry makes the first use of the symlink cache
    // (ls/autoimport/registry.go:1256) before the host is frozen
    // (project/snapshot.go:662, then :711). That build tracks the missing
    // node_modules/<dependency> directory, so a Created event for it marks
    // the project dirty (projectcollectionbuilder.go:1574). The parallel
    // bind (2 threads) first builds the checkers' copy with the tracking
    // paused (H3); the registry's build of the program's own value then
    // reads the directory again (module/resolver.go:1061, no cache in
    // `resolvePackageDirectoryOnly`), with the tracking on.
    fn created_dependency_directory_rebuilds_after_auto_imports_before_freeze() {
        let index = "export const x = 1;";
        let (session, _) = projecttestutil::setup(files(&[
            ("/home/projects/myproject/tsconfig.json", "{}"),
            (
                "/home/projects/myproject/package.json",
                r#"{ "name": "myproject", "dependencies": { "zlibx": "^1.0.0" } }"#,
            ),
            (
                "/home/projects/myproject/node_modules/other/package.json",
                r#"{ "name": "other" }"#,
            ),
            ("/home/projects/myproject/src/index.ts", index),
            ("/home/projects/myproject/src/other.ts", "export const y = 2;"),
        ]));
        let index_uri = "file:///home/projects/myproject/src/index.ts";
        open(&session, index_uri, index);
        let opened = program(&session, index_uri);

        // A new import: the next clone makes a new program, and the
        // auto-import request builds the project's bucket in that clone.
        edit(&session, index_uri, 2, (0, 0), (0, 0), "import { y } from \"./other\";\n");
        session
            .get_current_language_service_with_auto_imports(&bg(), &uri(index_uri))
            .unwrap_or_else(|err| panic!("{}", err.error()));
        let before = program(&session, index_uri);
        assert!(!same_program(&opened, &before));

        watch(
            &session,
            &[(CREATED, "file:///home/projects/myproject/node_modules/zlibx")],
        );
        let after = program(&session, index_uri);
        assert!(
            !same_program(&before, &after),
            "the registry's symlink cache build tracked node_modules/zlibx, so its Created event rebuilds"
        );
    }
}

// Go: projectcollectionbuilder_test.go:602 filesForSolutionConfigFile
fn files_for_solution_config_file(
    solution_refs: &[&str],
    compiler_options: &str,
    own_files: &[&str],
) -> FileMap {
    let compiler_options_str = if compiler_options.is_empty() {
        String::new()
    } else {
        format!(
            r#""compilerOptions": {{
			{compiler_options}
		}},"#
        )
    };
    let own_files_str = own_files.join(",");
    let refs: Vec<String> = solution_refs
        .iter()
        .map(|r| format!(r#"{{ "path": "{r}" }}"#))
        .collect();
    let tsconfig = format!(
        r#"{{
			{compiler_options_str}
			"files": [{own_files_str}],
			"references": [
				{}
			]
		}}"#,
        refs.join(",")
    );
    files(&[
        (
            "/user/username/projects/myproject/tsconfig.json",
            tsconfig.as_str(),
        ),
        (
            "/user/username/projects/myproject/tsconfig-src.json",
            r#"{
			"compilerOptions": {
				"composite": true,
				"outDir": "./target",
			},
			"include": ["./src/**/*"]
		}"#,
        ),
        (
            MAIN,
            "
			import { foo } from './helpers/functions';
			export { foo };",
        ),
        (
            "/user/username/projects/myproject/src/helpers/functions.ts",
            "export const foo = 1;",
        ),
    ])
}

// Go: projectcollectionbuilder_test.go:638 applyIndirectProjectFiles
fn apply_indirect_project_files(files: &mut FileMap, project_index: i32, compiler_options: &str) {
    files.extend(files_for_indirect_project(project_index, compiler_options));
}

// Go: projectcollectionbuilder_test.go:642 filesForIndirectProject
fn files_for_indirect_project(project_index: i32, compiler_options: &str) -> FileMap {
    let tsconfig = format!(
        r#"{{
			"compilerOptions": {{
				"composite": true,
				"outDir": "./target/",
				{compiler_options}
			}},
			"files": [
				"./indirect{project_index}/main.ts"
			],
			"references": [
				{{
				"path": "./tsconfig-src.json"
				}}
			]
		}}"#
    );
    let config_name =
        format!("/user/username/projects/myproject/tsconfig-indirect{project_index}.json");
    let main_name = format!("/user/username/projects/myproject/indirect{project_index}/main.ts");
    files(&[
        (config_name.as_str(), tsconfig.as_str()),
        (main_name.as_str(), "export const indirect = 1;"),
    ])
}

child_test! {
    // Go: fourslash/tests/workspaceSymbolNewInferredProject_test.go:15 TestWorkspaceSymbolNewInferredProject (ts#64642)
    // PORT: the session part of the fourslash test (the LSP oracle's
    // fourslash battery runs the test itself). workspace/symbol loads the
    // project trees and reads the program of every language service project.
    // After e.ts leaves the tsconfig project, it is in a new inferred
    // project, which needs a program too.
    fn workspace_symbol_new_inferred_project() {
        const A_URI: &str = "file:///home/src/projects/p/a.ts";
        const E_URI: &str = "file:///home/src/projects/p/e.ts";
        let session = bare_session(files(&[
            ("/home/src/projects/p/tsconfig.json", r#"{ "files": ["a.ts", "b.ts"] }"#),
            ("/home/src/projects/p/a.ts", "import \"./e\";\n"),
            ("/home/src/projects/p/b.ts", "export const b = 1;\n"),
            ("/home/src/projects/p/e.ts", "export const e = 1;\n"),
        ]));
        open(&session, A_URI, "import \"./e\";\n");
        open(&session, E_URI, "export const e = 1;\n");
        // e.ts isn't listed in tsconfig.json, it only gets pulled in by the import in a.ts.
        // Once that import is gone, e.ts should move to the inferred project.
        edit(&session, A_URI, 2, (0, 0), (0, 13), "");

        let mut projects: Vec<(Kind, bool)> = Vec::new();
        session.with_snapshot_loading_project_tree(&bg(), None, &mut |snapshot| {
            for project in snapshot.project_collection.language_service_projects() {
                let project = project.borrow();
                let program = project
                    .get_program()
                    .expect("every language service project has a program");
                let has_e = program.get_source_file("/home/src/projects/p/e.ts").is_some();
                projects.push((project.kind, has_e));
            }
        });
        assert!(
            projects.contains(&(Kind::INFERRED, true)),
            "e.ts is in the inferred project's program: {projects:?}"
        );
        session.close();
    }
}
