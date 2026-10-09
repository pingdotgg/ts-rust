//! Port of Go `internal/project/session_test.go` (`TestSession`).

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;
use std::time::{Duration, Instant};

use ts_goport::flags::ScriptKind;
use ts_goport::frontend::tspath;
use ts_goport::frontend::vfs::{Replacements, wrapvfs_wrap};
use ts_goport::locale;
use ts_goport::ls::lsutil;
use ts_goport::lsp::lsproto;
use ts_goport::options::Tristate;
use ts_goport::project::{self, Session, SessionInit, SessionOptions};

use super::projecttestutil::{self, FileMap, files};
use super::util::*;
use crate::support::vfstest;

const P1: &str = "/home/projects/TS/p1";

// Go: session_test.go:28 defaultFiles
fn default_files() -> FileMap {
    files(&[
        (
            "/home/projects/TS/p1/tsconfig.json",
            r#"{
			"compilerOptions": {
				"noLib": true,
				"module": "nodenext",
				"strict": true
			},
			"include": ["src"]
		}"#,
        ),
        (
            "/home/projects/TS/p1/src/index.ts",
            r#"import { x } from "./x";"#,
        ),
        ("/home/projects/TS/p1/src/x.ts", "export const x = 1;"),
        ("/home/projects/TS/p1/config.ts", "let x = 1, y = 2;"),
    ])
}

fn default_text(name: &str) -> String {
    let files = default_files();
    String::from_utf8(files[name].data.clone()).unwrap()
}

fn p1(rel: &str) -> String {
    format!("{P1}/{rel}")
}

fn p1_uri(rel: &str) -> String {
    format!("file://{P1}/{rel}")
}

// ---------------------------------------------------------------------------
// DidOpenFile
// ---------------------------------------------------------------------------

child_test! {
    // Go: session_test.go:44 TestSession/DidOpenFile/create configured project
    fn did_open_file_create_configured_project() {
        let (session, _) = projecttestutil::setup(default_files());
        assert_eq!(projects_len(&session), 0);

        open(&session, &p1_uri("src/index.ts"), &default_text(&p1("src/index.ts")));

        assert_eq!(projects_len(&session), 1);

        let configured_project = session
            .snapshot()
            .project_collection
            .configured_project(&tspath::Path("/home/projects/ts/p1/tsconfig.json".into()));
        assert!(configured_project.is_some());

        // Get language service to access the program
        let program = program(&session, &p1_uri("src/index.ts"));
        assert!(has_file(&program, &p1("src/x.ts")));
        assert_eq!(text(&program, &p1("src/x.ts")), "export const x = 1;");
    }
}

child_test! {
    // Go: session_test.go:66 TestSession/DidOpenFile/create inferred project
    fn did_open_file_create_inferred_project() {
        let (session, _) = projecttestutil::setup(default_files());

        open(&session, &p1_uri("config.ts"), &default_text(&p1("config.ts")));

        // Find tsconfig, load, notice config.ts is not included, create inferred project
        assert_eq!(projects_len(&session), 2);

        // Should have both configured project (for tsconfig.json) and inferred project
        let snapshot = session.snapshot();
        let configured_project = snapshot
            .project_collection
            .configured_project(&tspath::Path("/home/projects/ts/p1/tsconfig.json".into()));
        let inferred_project = snapshot.project_collection.inferred_project();
        assert!(configured_project.is_some());
        assert!(inferred_project.is_some());
    }
}

child_test! {
    // Go: session_test.go:83 TestSession/DidOpenFile/inferred project for in-memory files
    fn did_open_file_inferred_project_for_in_memory_files() {
        let (session, _) = projecttestutil::setup(default_files());

        open(&session, &p1_uri("config.ts"), &default_text(&p1("config.ts")));
        open(&session, "untitled:Untitled-1", "x");
        open(&session, "untitled:Untitled-2", "y");

        assert_eq!(projects_len(&session), 1);
        assert!(session.snapshot().project_collection.inferred_project().is_some());
    }
}

child_test! {
    // Go: session_test.go:97 TestSession/DidOpenFile/inferred project JS file
    fn did_open_file_inferred_project_js_file() {
        let js_files = files(&[("/home/projects/TS/p1/index.js", r#"import { x } from "./x";"#)]);
        let (session, _) = projecttestutil::setup(js_files);

        open_kind(
            &session,
            &p1_uri("index.js"),
            r#"import { x } from "./x";"#,
            lsproto::LanguageKind::JAVA_SCRIPT,
        );

        assert_eq!(projects_len(&session), 1);

        let program = program(&session, &p1_uri("index.js"));
        assert!(has_file(&program, &p1("index.js")));
    }
}

child_test! {
    // Go: session_test.go:116 TestSession/DidOpenFile/inferred project extensionless file
    fn did_open_file_inferred_project_extensionless_file() {
        let script_files = files(&[("/home/projects/TS/p1/script", "const x = 1;")]);
        let (session, _) = projecttestutil::setup(script_files);

        open_kind(
            &session,
            &p1_uri("script"),
            "const x = 1;",
            lsproto::LanguageKind("plaintext".into()),
        );

        let snapshot = session.snapshot();
        assert_eq!(snapshot.project_collection.projects().len(), 1);
        assert!(snapshot.project_collection.inferred_project().is_some());

        let program = program(&session, &p1_uri("script"));
        let file = program.get_source_file(&p1("script"));
        assert!(file.is_some());
        assert_eq!(file.unwrap().script_kind, ScriptKind::TS);
    }
}

child_test! {
    // Go: session_test.go:116 TestSession/watchChange and didOpen in same batch rebuilds program
    fn watch_change_and_did_open_in_same_batch_rebuilds_program() {
        let files = files(&[
            (
                "/home/projects/TS/p1/tsconfig.json",
                r#"{
				"compilerOptions": {
					"noLib": true,
					"strict": true
				}
			}"#,
            ),
            ("/home/projects/TS/p1/src/a.ts", "export const a = 1;\n"),
            ("/home/projects/TS/p1/src/b.ts", "export const b = 1;\n"),
        ]);
        let (session, utils) = projecttestutil::setup(files);
        let old_content = "export const a = 1;\n";

        // Open b.ts to create the project; a.ts is included via tsconfig.
        open(&session, &p1_uri("src/b.ts"), "export const b = 1;\n");

        // Verify a.ts is in the program with the original content.
        let p = program(&session, &p1_uri("src/b.ts"));
        assert_eq!(text(&p, &p1("src/a.ts")), old_content);

        // Modify a.ts on disk (simulate a build tool or git checkout).
        let new_content = "export const a = 2;\nexport const extra = true;\n";
        utils.fs().write_file(&p1("src/a.ts"), new_content).unwrap();

        // Queue a watch event for the disk change (not flushed yet).
        watch(&session, &[(CHANGED, &p1_uri("src/a.ts"))]);

        // Open a.ts in the editor—flushes both watch event and didOpen together.
        // Before the fix, processChanges would discard the watch event,
        // leaving the project with a stale SourceFile and a mismatched line map.
        open(&session, &p1_uri("src/a.ts"), new_content);

        // The program's SourceFile must reflect the overlay (new) content.
        let p = program(&session, &p1_uri("src/a.ts"));
        assert_eq!(text(&p, &p1("src/a.ts")), new_content);
    }
}

// ---------------------------------------------------------------------------
// DidChangeFile
// ---------------------------------------------------------------------------

child_test! {
    // Go: session_test.go:162 TestSession/DidChangeFile/update file and program
    fn did_change_file_update_file_and_program() {
        let (session, _) = projecttestutil::setup(default_files());

        open(&session, &p1_uri("src/x.ts"), &default_text(&p1("src/x.ts")));

        let program_before = program(&session, &p1_uri("src/x.ts"));

        edit(&session, &p1_uri("src/x.ts"), 2, (0, 17), (0, 18), "2");

        let program_after = program(&session, &p1_uri("src/x.ts"));

        // Program should change due to the file content change
        assert!(!same_program(&program_after, &program_before));
        assert_eq!(text(&program_after, &p1("src/x.ts")), "export const x = 2;");
    }
}

child_test! {
    // Go: session_test.go:199 TestSession/DidChangeFile/update untitled file
    fn did_change_file_update_untitled_file() {
        let (session, _) = projecttestutil::setup(default_files());

        open(&session, "untitled:Untitled-1", "let x = 1;");

        let program_before = program(&session, "untitled:Untitled-1");
        let untitled_file_name = uri("untitled:Untitled-1").file_name();
        assert_eq!(text(&program_before, &untitled_file_name), "let x = 1;");

        edit(&session, "untitled:Untitled-1", 2, (0, 8), (0, 9), "2");

        let program_after = program(&session, "untitled:Untitled-1");

        assert!(!same_program(&program_after, &program_before));
        assert_eq!(text(&program_after, &untitled_file_name), "let x = 2;");
    }
}

child_test! {
    // Go: session_test.go:237 TestSession/DidChangeFile/unchanged source files are reused
    fn did_change_file_unchanged_source_files_are_reused() {
        let (session, _) = projecttestutil::setup(default_files());

        open(&session, &p1_uri("src/x.ts"), &default_text(&p1("src/x.ts")));

        let program_before = program(&session, &p1_uri("src/x.ts"));
        let index_file_before = program_before.get_source_file(&p1("src/index.ts")).unwrap();

        edit(&session, &p1_uri("src/x.ts"), 2, (0, 0), (0, 0), ";");

        let program_after = program(&session, &p1_uri("src/x.ts"));

        // Unchanged file should be reused
        let index_file_after = program_after.get_source_file(&p1("src/index.ts")).unwrap();
        assert!(Rc::ptr_eq(&index_file_after, &index_file_before));
    }
}

child_test! {
    // Go: session_test.go:274 TestSession/DidChangeFile/change can pull in new files
    fn did_change_file_change_can_pull_in_new_files() {
        let mut files = default_files();
        files.insert("/home/projects/TS/p1/y.ts".into(), "export const y = 2;".into());
        let (session, _) = projecttestutil::setup(files);

        open(&session, &p1_uri("src/index.ts"), r#"import { x } from "./x";"#);

        // Verify y.ts is not initially in the program
        let program_before = program(&session, &p1_uri("src/index.ts"));
        assert!(!has_file(&program_before, &p1("y.ts")));

        edit(
            &session,
            &p1_uri("src/index.ts"),
            2,
            (0, 0),
            (0, 0),
            "import { y } from \"../y\";\n",
        );

        let program_after = program(&session, &p1_uri("src/index.ts"));

        // y.ts should now be included in the program
        assert!(has_file(&program_after, &p1("y.ts")));
    }
}

child_test! {
    // Go: session_test.go:314 TestSession/DidChangeFile/single-file change followed by config change reloads program
    fn did_change_file_single_file_change_followed_by_config_change_reloads_program() {
        let mut files = default_files();
        files.insert(
            "/home/projects/TS/p1/tsconfig.json".into(),
            r#"{
				"compilerOptions": {
					"noLib": true,
					"module": "nodenext",
					"strict": true
				},
				"include": ["src/index.ts"]
			}"#
            .into(),
        );
        let (session, utils) = projecttestutil::setup(files);

        open(&session, &p1_uri("src/index.ts"), r#"import { x } from "./x";"#);

        let program_before = program(&session, &p1_uri("src/index.ts"));
        assert_eq!(program_before.get_source_files().len(), 2);

        edit(&session, &p1_uri("src/index.ts"), 2, (0, 0), (0, 0), "\n");

        utils
            .fs()
            .write_file(
                &p1("tsconfig.json"),
                r#"{
				"compilerOptions": {
					"noLib": true,
					"module": "nodenext",
					"strict": true
				},
				"include": ["./**/*"]
			}"#,
            )
            .unwrap();

        watch(&session, &[(CHANGED, &p1_uri("tsconfig.json"))]);

        let program_after = program(&session, &p1_uri("src/index.ts"));
        assert_eq!(program_after.get_source_files().len(), 3);
    }
}

// ---------------------------------------------------------------------------
// DidCloseFile
// ---------------------------------------------------------------------------

fn delete_close_recreate(files: FileMap) {
    let (session, utils) = projecttestutil::setup(files);

    open(&session, &p1_uri("src/x.ts"), "export const x = 1;");
    open(
        &session,
        &p1_uri("src/index.ts"),
        r#"import { x } from "./x";"#,
    );

    utils.fs().remove(&p1("src/x.ts")).unwrap();

    close(&session, &p1_uri("src/x.ts"));
    let p = program(&session, &p1_uri("src/index.ts"));
    assert!(!has_file(&p, &p1("src/x.ts")));

    utils.fs().write_file(&p1("src/x.ts"), "").unwrap();

    open(&session, &p1_uri("src/x.ts"), "");

    let p = program(&session, &p1_uri("src/x.ts"));
    assert!(has_file(&p, &p1("src/x.ts")));
    assert_eq!(text(&p, &p1("src/x.ts")), "");
}

child_test! {
    // Go: session_test.go:380 TestSession/DidCloseFile/Configured projects/delete a file, close it, recreate it
    fn did_close_file_configured_projects_delete_a_file_close_it_recreate_it() {
        delete_close_recreate(default_files());
    }
}

child_test! {
    // Go: session_test.go:411 TestSession/DidCloseFile/Inferred projects/delete a file, close it, recreate it
    fn did_close_file_inferred_projects_delete_a_file_close_it_recreate_it() {
        let mut files = default_files();
        files.remove("/home/projects/TS/p1/tsconfig.json");
        delete_close_recreate(files);
    }
}

child_test! {
    // Go: session_test.go:442 TestSession/DidCloseFile/Inferred projects/close untitled file
    fn did_close_file_inferred_projects_close_untitled_file() {
        let (session, _) = projecttestutil::setup(default_files());

        open(&session, "untitled:Untitled-1", "let x = 1;");
        close(&session, "untitled:Untitled-1");
        open(&session, "untitled:Untitled-2", "");
    }
}

// ---------------------------------------------------------------------------
// DidSaveFile
// ---------------------------------------------------------------------------

fn save_and_watch(save_first: bool) {
    let (session, _) = projecttestutil::setup(default_files());
    open(
        &session,
        &p1_uri("src/index.ts"),
        r#"import { x } from "./x";"#,
    );

    assert_eq!(session.snapshot().id(), 1);

    let index = uri(&p1_uri("src/index.ts"));
    if save_first {
        session.did_save_file(&bg(), &index);
        watch(&session, &[(CHANGED, &p1_uri("src/index.ts"))]);
    } else {
        watch(&session, &[(CHANGED, &p1_uri("src/index.ts"))]);
        session.did_save_file(&bg(), &index);
    }

    session.wait_for_background_tasks();
    // We didn't need a snapshot change, but the session overlays should be updated.
    assert_eq!(session.snapshot().id(), 1);

    // Open another file to force a snapshot update so we can see the changes.
    open(&session, &p1_uri("src/x.ts"), "export const x = 1;");
    let snapshot = session.snapshot();
    let file = snapshot
        .get_file(&p1("src/index.ts"))
        .expect("snapshot file index.ts");
    assert!(file.matches_disk_text());
}

child_test! {
    // Go: session_test.go:455 TestSession/DidSaveFile/save event first
    fn did_save_file_save_event_first() {
        save_and_watch(true);
    }
}

child_test! {
    // Go: session_test.go:482 TestSession/DidSaveFile/watch event first
    fn did_save_file_watch_event_first() {
        save_and_watch(false);
    }
}

// ---------------------------------------------------------------------------
// Source file sharing
// ---------------------------------------------------------------------------

child_test! {
    // Go: session_test.go:512 TestSession/Source file sharing/projects with similar options share source files
    fn source_file_sharing_projects_with_similar_options_share_source_files() {
        let mut files = default_files();
        files.insert(
            "/home/projects/TS/p2/tsconfig.json".into(),
            r#"{
				"compilerOptions": {
					"noLib": true,
					"module": "nodenext",
					"strict": true,
					"noCheck": true
				}
			}"#
            .into(),
        );
        files.insert(
            "/home/projects/TS/p2/src/index.ts".into(),
            r#"import { x } from "../../p1/src/x";"#.into(),
        );
        let (session, _) = projecttestutil::setup(files);

        open(&session, &p1_uri("src/index.ts"), r#"import { x } from "./x";"#);
        open(
            &session,
            "file:///home/projects/TS/p2/src/index.ts",
            r#"import { x } from "../../p1/src/x";"#,
        );

        assert_eq!(projects_len(&session), 2);

        let program1 = program(&session, &p1_uri("src/index.ts"));
        let program2 = program(&session, "file:///home/projects/TS/p2/src/index.ts");

        let x1 = program1.get_source_file(&p1("src/x.ts")).unwrap();
        let x2 = program2.get_source_file(&p1("src/x.ts")).unwrap();
        assert!(Rc::ptr_eq(&x1, &x2));
    }
}

child_test! {
    // Go: session_test.go:547 TestSession/Source file sharing/projects with different options do not share source files
    fn source_file_sharing_projects_with_different_options_do_not_share_source_files() {
        let mut files = default_files();
        files.insert(
            "/home/projects/TS/p2/tsconfig.json".into(),
            r#"{
				"compilerOptions": {
					"noLib": true,
					"module": "nodenext",
					"strict": true,
					"moduleDetection": "auto"
				},
				"include": ["src"]
			}"#
            .into(),
        );
        files.insert(
            "/home/projects/TS/p2/src/index.ts".into(),
            r#"import { x } from "../../p1/src/x";"#.into(),
        );
        let (session, _) = projecttestutil::setup(files);

        open(&session, &p1_uri("src/index.ts"), r#"import { x } from "./x";"#);
        open(
            &session,
            "file:///home/projects/TS/p2/src/index.ts",
            r#"import { x } from "../../p1/src/x";"#,
        );

        assert_eq!(projects_len(&session), 2);

        let program1 = program(&session, &p1_uri("src/index.ts"));
        let program2 = program(&session, "file:///home/projects/TS/p2/src/index.ts");

        let x1 = program1.get_source_file(&p1("src/x.ts"));
        let x2 = program2.get_source_file(&p1("src/x.ts"));
        assert!(x1.is_some() && x2.is_some());
        assert!(!Rc::ptr_eq(&x1.unwrap(), &x2.unwrap()));
    }
}

// ---------------------------------------------------------------------------
// DidChangeWatchedFiles
// ---------------------------------------------------------------------------

child_test! {
    // Go: session_test.go:586 TestSession/DidChangeWatchedFiles/change open file
    fn did_change_watched_files_change_open_file() {
        let (session, utils) = projecttestutil::setup(default_files());

        open(&session, &p1_uri("src/x.ts"), "export const x = 1;");
        open(&session, &p1_uri("src/index.ts"), r#"import { x } from "./x";"#);

        let program_before = program(&session, &p1_uri("src/index.ts"));

        utils.fs().write_file(&p1("src/x.ts"), "export const x = 2;").unwrap();

        watch(&session, &[(CHANGED, &p1_uri("src/x.ts"))]);

        let program_after = program(&session, &p1_uri("src/index.ts"));
        // Program should remain the same since the file is open and changes are handled through DidChangeTextDocument
        assert!(same_program(&program_before, &program_after));
    }
}

child_test! {
    // Go: session_test.go:614 TestSession/DidChangeWatchedFiles/change closed program file
    fn did_change_watched_files_change_closed_program_file() {
        let (session, utils) = projecttestutil::setup(default_files());

        open(&session, &p1_uri("src/index.ts"), r#"import { x } from "./x";"#);

        let program_before = program(&session, &p1_uri("src/index.ts"));

        utils.fs().write_file(&p1("src/x.ts"), "export const x = 2;").unwrap();

        watch(&session, &[(CHANGED, &p1_uri("src/x.ts"))]);

        let program_after = program(&session, &p1_uri("src/index.ts"));
        assert!(!same_program(&program_after, &program_before));
    }
}

// Go: session_test.go:640 TestSession/DidChangeWatchedFiles/change program file not in tsconfig root files
fn change_program_file_not_in_tsconfig_root_files(workspace_dir: &str) {
    let files = files(&[
        (
            "/home/projects/TS/p1/tsconfig.json",
            r#"{
							"compilerOptions": {
								"noLib": true,
								"module": "nodenext",
								"strict": true
							},
							"files": ["src/index.ts"]
						}"#,
        ),
        (
            "/home/projects/TS/p1/src/index.ts",
            r#"import { x } from "../../x";"#,
        ),
        ("/home/projects/TS/x.ts", "export const x = 1;"),
    ]);

    let (session, utils) =
        projecttestutil::setup_with_options(files, projecttestutil::session_options(workspace_dir));
    open(
        &session,
        &p1_uri("src/index.ts"),
        r#"import { x } from "../../x";"#,
    );
    let program_before = program(&session, &p1_uri("src/index.ts"));
    session.wait_for_background_tasks();

    // ts#64544: the lookup watcher keeps the spelling of the file's directory.
    assert!(utils.watches_file("/home/projects/TS/x.ts"));

    utils
        .fs()
        .write_file("/home/projects/TS/x.ts", "export const x = 2;")
        .unwrap();

    watch(&session, &[(CHANGED, "file:///home/projects/TS/x.ts")]);

    let program_after = program(&session, &p1_uri("src/index.ts"));
    assert!(!same_program(&program_after, &program_before));
}

child_test! {
    // Go: session_test.go:643 TestSession/DidChangeWatchedFiles/change program file not in tsconfig root files/workspaceDir=_
    fn did_change_watched_files_change_program_file_not_in_tsconfig_root_files_workspace_dir_root() {
        change_program_file_not_in_tsconfig_root_files("/");
    }
}

child_test! {
    // Go: session_test.go:643 TestSession/DidChangeWatchedFiles/change program file not in tsconfig root files/workspaceDir=_home_projects_TS_p1
    fn did_change_watched_files_change_program_file_not_in_tsconfig_root_files_workspace_dir_p1() {
        change_program_file_not_in_tsconfig_root_files("/home/projects/TS/p1");
    }
}

child_test! {
    // Go: session_test.go:643 TestSession/DidChangeWatchedFiles/change program file not in tsconfig root files/workspaceDir=_somewhere_else_entirely
    fn did_change_watched_files_change_program_file_not_in_tsconfig_root_files_workspace_dir_elsewhere() {
        change_program_file_not_in_tsconfig_root_files("/somewhere/else/entirely");
    }
}

child_test! {
    // Go: session_test.go:691 TestSession/DidChangeWatchedFiles/change config file
    fn did_change_watched_files_change_config_file() {
        let index_text = "\n\t\t\t\t\timport { x } from \"./x\";\n\t\t\t\t\tlet y: number = x;";
        let files = files(&[
            (
                "/home/projects/TS/p1/tsconfig.json",
                r#"{
					"compilerOptions": {
						"noLib": true,
						"strict": false
					}
				}"#,
            ),
            ("/home/projects/TS/p1/src/x.ts", "export declare const x: number | undefined;"),
            ("/home/projects/TS/p1/src/index.ts", index_text),
        ]);

        let (session, utils) = projecttestutil::setup(files);
        open(&session, &p1_uri("src/index.ts"), index_text);

        let p = program(&session, &p1_uri("src/index.ts"));
        assert_eq!(sem_diag_count(&p, &p1("src/index.ts")), 0);

        utils
            .fs()
            .write_file(
                &p1("tsconfig.json"),
                r#"{
				"compilerOptions": {
					"noLib": false,
					"strict": true
				}
			}"#,
            )
            .unwrap();

        watch(&session, &[(CHANGED, &p1_uri("tsconfig.json"))]);

        let p = program(&session, &p1_uri("src/index.ts"));
        assert_eq!(sem_diag_count(&p, &p1("src/index.ts")), 1);
    }
}

child_test! {
    // Go: session_test.go:735 TestSession/DidChangeWatchedFiles/delete explicitly included file
    fn did_change_watched_files_delete_explicitly_included_file() {
        let files = files(&[
            (
                "/home/projects/TS/p1/tsconfig.json",
                r#"{
					"compilerOptions": {
						"noLib": true
					},
					"files": ["src/index.ts", "src/x.ts"]
				}"#,
            ),
            ("/home/projects/TS/p1/src/x.ts", "export declare const x: number | undefined;"),
            ("/home/projects/TS/p1/src/index.ts", r#"import { x } from "./x";"#),
        ]);
        let (session, utils) = projecttestutil::setup(files);
        open(&session, &p1_uri("src/index.ts"), r#"import { x } from "./x";"#);

        let p = program(&session, &p1_uri("src/index.ts"));
        assert!(file_names(&p).contains(&p1("src/x.ts")));
        assert_eq!(sem_diag_count(&p, &p1("src/index.ts")), 0);

        utils.fs().remove(&p1("src/x.ts")).unwrap();

        watch(&session, &[(DELETED, &p1_uri("src/x.ts"))]);

        let p = program(&session, &p1_uri("src/index.ts"));
        // File name is still in the command line, was explicitly included
        assert!(file_names(&p).contains(&p1("src/x.ts")));
        assert_eq!(sem_diag_count(&p, &p1("src/index.ts")), 1);
        assert!(!has_file(&p, &p1("src/x.ts")));

        // Open file to trigger cleanup
        open(&session, "untitled:Untitled-1", "");
        assert!(session.snapshot().get_file(&p1("src/x.ts")).is_none());
    }
}

child_test! {
    // Go: session_test.go:780 TestSession/DidChangeWatchedFiles/delete wildcard included file
    fn did_change_watched_files_delete_wildcard_included_file() {
        let files = files(&[
            (
                "/home/projects/TS/p1/tsconfig.json",
                r#"{
					"compilerOptions": {
						"noLib": true
					},
					"include": ["src"]
				}"#,
            ),
            ("/home/projects/TS/p1/src/index.ts", "let x = 2;"),
            ("/home/projects/TS/p1/src/x.ts", "let y = x;"),
        ]);
        let (session, utils) = projecttestutil::setup(files);
        open(&session, &p1_uri("src/x.ts"), "let y = x;");

        let p = program(&session, &p1_uri("src/x.ts"));
        assert!(file_names(&p).contains(&p1("src/index.ts")));
        assert_eq!(sem_diag_count(&p, &p1("src/x.ts")), 0);

        utils.fs().remove(&p1("src/index.ts")).unwrap();

        watch(&session, &[(DELETED, &p1_uri("src/index.ts"))]);

        let p = program(&session, &p1_uri("src/x.ts"));
        // File name is gone from the command line, was originally included via wildcard
        assert!(!file_names(&p).contains(&p1("src/index.ts")));
        assert_eq!(sem_diag_count(&p, &p1("src/x.ts")), 1);

        // Open file to trigger cleanup
        open(&session, "untitled:Untitled-1", "");
        assert!(session.snapshot().get_file(&p1("src/index.ts")).is_none());
    }
}

child_test! {
    // Go: session_test.go:824 TestSession/DidChangeWatchedFiles/delete directory with wildcard included files
    fn did_change_watched_files_delete_directory_with_wildcard_included_files() {
        let files = files(&[
            (
                "/home/projects/TS/p1/tsconfig.json",
                r#"{
					"compilerOptions": {
						"noLib": true
					},
					"include": ["src"]
				}"#,
            ),
            ("/home/projects/TS/p1/src/index.ts", r#"import { x } from "./sub/x";"#),
            ("/home/projects/TS/p1/src/sub/x.ts", "export const x = 1;"),
        ]);
        let (session, utils) = projecttestutil::setup(files);
        open(&session, &p1_uri("src/index.ts"), r#"import { x } from "./sub/x";"#);

        let p = program(&session, &p1_uri("src/index.ts"));
        assert!(file_names(&p).contains(&p1("src/sub/x.ts")));
        assert_eq!(sem_diag_count(&p, &p1("src/index.ts")), 0);

        // Delete the entire subdirectory from the file system.
        utils.fs().remove(&p1("src/sub")).unwrap();

        // Simulate the single deletion event for the directory URI.
        watch(&session, &[(DELETED, &p1_uri("src/sub"))]);

        let p = program(&session, &p1_uri("src/index.ts"));
        // The directory was deleted, so the file should no longer be in the program.
        assert!(!file_names(&p).contains(&p1("src/sub/x.ts")));
        // The import should now be an error since the module is missing.
        assert_eq!(sem_diag_count(&p, &p1("src/index.ts")), 1);
    }
}

child_test! {
    // Go: session_test.go:870 TestSession/DidChangeWatchedFiles/delete directory with program-only files
    fn did_change_watched_files_delete_directory_with_program_only_files() {
        let files = files(&[
            (
                "/home/projects/TS/p1/tsconfig.json",
                r#"{
					"compilerOptions": {
						"noLib": true
					},
					"files": ["src/index.ts"]
				}"#,
            ),
            ("/home/projects/TS/p1/src/index.ts", r#"import { x } from "./sub/x";"#),
            ("/home/projects/TS/p1/src/sub/x.ts", "export const x = 1;"),
        ]);
        let (session, utils) = projecttestutil::setup(files);
        open(&session, &p1_uri("src/index.ts"), r#"import { x } from "./sub/x";"#);

        let p = program(&session, &p1_uri("src/index.ts"));
        assert!(file_names(&p).contains(&p1("src/index.ts")));
        // x.ts is not in "files" but is pulled in via the import.
        assert!(has_file(&p, &p1("src/sub/x.ts")));
        assert_eq!(sem_diag_count(&p, &p1("src/index.ts")), 0);

        // Delete the entire subdirectory from the file system.
        utils.fs().remove(&p1("src/sub")).unwrap();

        // Send a delete event for the directory URI.
        watch(&session, &[(DELETED, &p1_uri("src/sub"))]);

        let p = program(&session, &p1_uri("src/index.ts"));
        // The directory was deleted, so the file should no longer be resolvable.
        assert!(!has_file(&p, &p1("src/sub/x.ts")));
        // The import should now be an error since the module is missing.
        assert_eq!(sem_diag_count(&p, &p1("src/index.ts")), 1);
    }
}

const SIBLING_INDEX: &str =
    "import { content } from \"./f/content\";\n\nexport const value = content;";

child_test! {
    // Go: session_test.go:914 TestSession/DidChangeWatchedFiles/delete sibling folder schedules diagnostics refresh
    fn did_change_watched_files_delete_sibling_folder_schedules_diagnostics_refresh() {
        let files = files(&[
            (
                "/home/projects/TS/p1/tsconfig.json",
                r#"{
					"compilerOptions": {
						"noLib": true
					},
					"files": ["index.ts"]
				}"#,
            ),
            ("/home/projects/TS/p1/index.ts", SIBLING_INDEX),
            ("/home/projects/TS/p1/f/content.ts", "export const content = 1;"),
        ]);
        let (session, utils) = projecttestutil::setup(files);
        let content_uri = p1_uri("f/content.ts");
        open(&session, &p1_uri("index.ts"), SIBLING_INDEX);
        open(&session, &content_uri, "export const content = 1;");

        let _ = program(&session, &p1_uri("index.ts"));
        session.wait_for_background_tasks();

        let baseline_refresh_count = utils.client().refresh_diagnostics_calls();

        utils.fs().remove(&p1("f")).unwrap();

        watch(&session, &[(DELETED, &p1_uri("f"))]);
        close(&session, &content_uri);
        session.wait_for_background_tasks();

        let refresh_count = utils.client().refresh_diagnostics_calls();
        assert!(
            refresh_count > baseline_refresh_count,
            "expected RefreshDiagnostics to be called after deleting /home/projects/TS/p1/f, got {refresh_count} calls (baseline {baseline_refresh_count})"
        );
    }
}

child_test! {
    // Go: session_test.go:957 TestSession/DidChangeWatchedFiles/delete sibling folder schedules diagnostics refresh after opening third file
    fn did_change_watched_files_delete_sibling_folder_schedules_diagnostics_refresh_after_opening_third_file() {
        let files = files(&[
            (
                "/home/projects/TS/p1/tsconfig.json",
                r#"{
					"compilerOptions": {
						"noLib": true
					},
					"files": ["index.ts", "third.ts"]
				}"#,
            ),
            ("/home/projects/TS/p1/index.ts", SIBLING_INDEX),
            ("/home/projects/TS/p1/f/content.ts", "export const content = 1;"),
            ("/home/projects/TS/p1/third.ts", "export const third = 3;"),
        ]);
        let (session, utils) = projecttestutil::setup(files);
        let content_uri = p1_uri("f/content.ts");
        let third_uri = p1_uri("third.ts");
        open(&session, &p1_uri("index.ts"), SIBLING_INDEX);
        open(&session, &content_uri, "export const content = 1;");

        let _ = program(&session, &p1_uri("index.ts"));
        session.wait_for_background_tasks();

        let baseline_refresh_count = utils.client().refresh_diagnostics_calls();

        utils.fs().remove(&p1("f")).unwrap();

        watch(&session, &[(DELETED, &p1_uri("f"))]);
        open(&session, &third_uri, "export const third = 3;");
        session.wait_for_background_tasks();

        let refresh_count = utils.client().refresh_diagnostics_calls();
        assert!(
            refresh_count > baseline_refresh_count,
            "expected RefreshDiagnostics to be called after deleting /home/projects/TS/p1/f and opening /home/projects/TS/p1/third.ts, got {refresh_count} calls (baseline {baseline_refresh_count})"
        );
    }
}

/// The shared body of the "create ..." subtests: a missing file makes one
/// error; writing it and a create event fixes the error.
fn create_file_resolves_error(tsconfig: &str, index: &str, new_file: &str, new_text: &str) {
    let files = files(&[
        ("/home/projects/TS/p1/tsconfig.json", tsconfig),
        ("/home/projects/TS/p1/src/index.ts", index),
    ]);
    let (session, utils) = projecttestutil::setup(files);
    open(&session, &p1_uri("src/index.ts"), index);

    let p = program(&session, &p1_uri("src/index.ts"));
    // Initially should have an error because the file is missing
    assert_eq!(sem_diag_count(&p, &p1("src/index.ts")), 1);

    // Add the missing file
    utils.fs().write_file(&p1(new_file), new_text).unwrap();

    watch(&session, &[(CREATED, &p1_uri(new_file))]);

    // Error should be resolved
    let p = program(&session, &p1_uri("src/index.ts"));
    assert_eq!(sem_diag_count(&p, &p1("src/index.ts")), 0);
    assert!(has_file(&p, &p1(new_file)));
}

child_test! {
    // Go: session_test.go:1002 TestSession/DidChangeWatchedFiles/create explicitly included file
    fn did_change_watched_files_create_explicitly_included_file() {
        create_file_resolves_error(
            r#"{
					"compilerOptions": {
						"noLib": true
					},
					"files": ["src/index.ts", "src/y.ts"]
				}"#,
            r#"import { y } from "./y";"#,
            "src/y.ts",
            "export const y = 1;",
        );
    }
}

child_test! {
    // Go: session_test.go:1042 TestSession/DidChangeWatchedFiles/create failed lookup location
    fn did_change_watched_files_create_failed_lookup_location() {
        create_file_resolves_error(
            r#"{
					"compilerOptions": {
						"noLib": true
					},
					"files": ["src/index.ts"]
				}"#,
            r#"import { z } from "./z";"#,
            "src/z.ts",
            "export const z = 1;",
        );
    }
}

child_test! {
    // Go: session_test.go:1082 TestSession/DidChangeWatchedFiles/create wildcard included file
    fn did_change_watched_files_create_wildcard_included_file() {
        create_file_resolves_error(
            r#"{
					"compilerOptions": {
						"noLib": true
					},
					"include": ["src"]
				}"#,
            "a;",
            "src/a.ts",
            "const a = 1;",
        );
    }
}

child_test! {
    // Go: session_test.go:1122 TestSession/DidChangeWatchedFiles/irrelevant extension changes are filtered out
    fn did_change_watched_files_irrelevant_extension_changes_are_filtered_out() {
        let files = files(&[
            (
                "/home/projects/TS/p1/tsconfig.json",
                r#"{
					"compilerOptions": {
						"noLib": true
					},
					"include": ["src"]
				}"#,
            ),
            ("/home/projects/TS/p1/src/index.ts", "export const x = 1;"),
            ("/home/projects/TS/p1/src/data.txt", "some text"),
        ]);
        let (session, utils) = projecttestutil::setup(files);
        open(&session, &p1_uri("src/index.ts"), "export const x = 1;");

        let p = program(&session, &p1_uri("src/index.ts"));
        assert_eq!(sem_diag_count(&p, &p1("src/index.ts")), 0);
        let old_program = p;

        // Modify an irrelevant file and send change/create events for files with
        // extensions that are not relevant to TypeScript compilation.
        utils.fs().write_file(&p1("src/data.txt"), "updated text").unwrap();

        watch(
            &session,
            &[
                (CHANGED, &p1_uri("src/data.txt")),
                (CREATED, &p1_uri("src/styles.css")),
                (CREATED, &p1_uri("src/image.png")),
            ],
        );

        // The program should not have been rebuilt since all events had irrelevant extensions.
        let p = program(&session, &p1_uri("src/index.ts"));
        assert!(
            same_program(&p, &old_program),
            "program should not be rebuilt for irrelevant extension changes"
        );
    }
}

child_test! {
    // Go: session_test.go:1170 TestSession/DidChangeWatchedFiles/pnpm install links local package
    fn did_change_watched_files_pnpm_install_links_local_package() {
        let files = files(&[
            ("/home/projects/pnpm/pnpm-workspace.yaml", "packages:\n  - 'packages/*'"),
            (
                "/home/projects/pnpm/packages/alpha/package.json",
                r#"{ "name": "@repo/alpha", "main": "index.ts" }"#,
            ),
            (
                "/home/projects/pnpm/packages/alpha/tsconfig.json",
                r#"{
					"compilerOptions": { "noLib": true, "composite": true }
				}"#,
            ),
            ("/home/projects/pnpm/packages/alpha/index.ts", "export const alpha = 1;"),
            ("/home/projects/pnpm/packages/beta/package.json", r#"{ "name": "@repo/beta" }"#),
            (
                "/home/projects/pnpm/packages/beta/tsconfig.json",
                r#"{
					"compilerOptions": { "noLib": true }
				}"#,
            ),
            (
                "/home/projects/pnpm/packages/beta/index.ts",
                r#"import { alpha } from "@repo/alpha";"#,
            ),
        ]);
        let (session, utils) = projecttestutil::setup(files);
        let beta = "file:///home/projects/pnpm/packages/beta/index.ts";
        open(&session, beta, r#"import { alpha } from "@repo/alpha";"#);

        // Before pnpm install: the import is unresolved because node_modules/@repo/alpha doesn't exist.
        let p = program(&session, beta);
        assert_eq!(sem_diag_count(&p, "/home/projects/pnpm/packages/beta/index.ts"), 1);

        // Simulate pnpm install: create a symlink from beta's node_modules/@repo/alpha to packages/alpha.
        let map_fs = utils.fs_from_file_map();
        map_fs
            .mkdir_all("home/projects/pnpm/packages/beta/node_modules/@repo", ts_goport::frontend::vfs::FileMode::PERM)
            .unwrap();
        map_fs.add_symlink(
            "home/projects/pnpm/packages/beta/node_modules/@repo/alpha",
            "home/projects/pnpm/packages/alpha",
        );

        // Fire watch events mimicking what VS Code sends for a pnpm install.
        watch(
            &session,
            &[
                (CREATED, "file:///home/projects/pnpm/packages/beta/node_modules"),
                (CREATED, "file:///home/projects/pnpm/packages/beta/node_modules/%40repo"),
                (CREATED, "file:///home/projects/pnpm/packages/beta/node_modules/%40repo/alpha"),
                (CREATED, "file:///home/projects/pnpm/pnpm-lock.yaml"),
                (CHANGED, "file:///home/projects/pnpm/packages/beta/node_modules/.bin/tsc"),
                (CHANGED, "file:///home/projects/pnpm/packages/beta/node_modules/.bin/tsserver"),
            ],
        );

        // After pnpm install: the import should resolve.
        let p = program(&session, beta);
        assert_eq!(sem_diag_count(&p, "/home/projects/pnpm/packages/beta/index.ts"), 0);
    }
}

child_test! {
    // Go: session_test.go:1222 TestSession/DidChangeWatchedFiles/symlinked node_modules package.json change invalidates resolution
    fn did_change_watched_files_symlinked_node_modules_package_json_change_invalidates_resolution() {
        let mut files = files(&[
            (
                "/home/projects/myproject/tsconfig.json",
                r#"{
					"compilerOptions": {
						"noLib": true,
						"module": "nodenext",
						"moduleResolution": "nodenext"
					},
					"files": ["src/index.ts"]
				}"#,
            ),
            ("/home/projects/myproject/src/index.ts", r#"import { foo } from "mylib";"#),
            // The real package lives as a sibling directory
            (
                "/home/projects/mylib/package.json",
                r#"{
					"name": "mylib",
					"main": "dist/index.js"
				}"#,
            ),
            ("/home/projects/mylib/dist/index.js", "exports.foo = function() { return 1; };"),
            ("/home/projects/mylib/dist/index.d.ts", "export declare function foo(): number;"),
        ]);
        // node_modules/mylib is a symlink to the sibling
        files.insert(
            "/home/projects/myproject/node_modules/mylib".into(),
            vfstest::symlink("/home/projects/mylib"),
        );

        let (session, utils) = projecttestutil::setup_with_options(
            files,
            projecttestutil::session_options("/home/projects/myproject"),
        );
        let index = "file:///home/projects/myproject/src/index.ts";
        open(&session, index, r#"import { foo } from "mylib";"#);

        // Initial state: import resolves successfully via package.json main -> dist/index.d.ts
        let p = program(&session, index);
        session.wait_for_background_tasks();
        assert_eq!(
            sem_diag_count(&p, "/home/projects/myproject/src/index.ts"),
            0,
            "import should resolve initially"
        );

        // Assert: watched file globs cover the realpath of package.json and dist/index.d.ts.
        // With a workspace dir set, watchers use RelativePattern with a base URI.
        assert!(
            utils.watches_file("/home/projects/mylib/package.json"),
            "realpath of package.json should be watched"
        );
        assert!(
            utils.watches_file("/home/projects/mylib/dist/index.d.ts"),
            "realpath of dist/index.d.ts should be watched"
        );

        // Edit package.json to remove "main" field
        utils
            .fs()
            .write_file(
                "/home/projects/mylib/package.json",
                r#"{
				"name": "mylib"
			}"#,
            )
            .unwrap();

        // Fire watch event for the realpath of the changed package.json.
        watch(&session, &[(CHANGED, "file:///home/projects/mylib/package.json")]);

        // After removing "main" from package.json, the import should no longer resolve.
        let p = program(&session, index);
        assert!(
            sem_diag_count(&p, "/home/projects/myproject/src/index.ts") > 0,
            "import should fail after removing main from package.json"
        );
    }
}

child_test! {
    // Go: session_test.go:1296 TestSession/DidChangeWatchedFiles/create file in non-existent directory
    fn did_change_watched_files_create_file_in_non_existent_directory() {
        create_file_resolves_error(
            r#"{
					"compilerOptions": {
						"noLib": true
					},
					"files": ["src/index.ts"]
				}"#,
            r#"import { helper } from "./lib/helper";"#,
            "src/lib/helper.ts",
            "export const helper = 1;",
        );
    }
}

child_test! {
    // Go: session_test.go:1336 TestSession/DidChangeWatchedFiles/create symlink directory matching include pattern
    fn did_change_watched_files_create_symlink_directory_matching_include_pattern() {
        let files = files(&[
            (
                "/home/projects/TS/p1/tsconfig.json",
                r#"{
					"compilerOptions": {
						"noLib": true
					},
					"include": ["src"]
				}"#,
            ),
            ("/home/projects/TS/p1/src/index.ts", "export const x = 1;"),
            ("/home/projects/TS/shared/utils.ts", r#"export const util = "hello";"#),
            ("/home/projects/TS/shared/helpers.ts", "export const helper = 42;"),
        ]);
        let (session, utils) = projecttestutil::setup(files);
        open(&session, &p1_uri("src/index.ts"), "export const x = 1;");

        let p = program(&session, &p1_uri("src/index.ts"));

        // Initially, project only has the one file in src/.
        let names = file_names(&p);
        assert!(names.contains(&p1("src/index.ts")));
        assert!(!names.contains(&p1("src/linked/utils.ts")));
        assert!(!names.contains(&p1("src/linked/helpers.ts")));

        // Create a symlink directory inside src/ that points to the shared directory.
        utils
            .fs_from_file_map()
            .add_symlink("home/projects/TS/p1/src/linked", "home/projects/TS/shared");

        // Send directory creation event (what VS Code sends when a symlink directory appears).
        watch(&session, &[(CREATED, &p1_uri("src/linked"))]);

        // After the symlink directory is created, the files inside it should be
        // picked up by the wildcard include pattern.
        let p = program(&session, &p1_uri("src/index.ts"));
        let names = file_names(&p);
        assert!(names.contains(&p1("src/index.ts")));
        assert!(names.contains(&p1("src/linked/utils.ts")));
        assert!(names.contains(&p1("src/linked/helpers.ts")));
    }
}

child_test! {
    // Go: session_test.go:1405 TestSession/DidChangeWatchedFiles/skips irrelevant extensions
    fn did_change_watched_files_skips_irrelevant_extensions() {
        let files = files(&[
            (
                "/home/projects/TS/p1/tsconfig.json",
                r#"{
					"compilerOptions": {},
					"include": ["src"]
				}"#,
            ),
            ("/home/projects/TS/p1/src/index.ts", "export const x = 1;"),
        ]);
        let (session, utils) = projecttestutil::setup(files);

        open(&session, &p1_uri("src/index.ts"), "export const x = 1;");
        session.wait_for_background_tasks();

        let mut baseline_refresh_count = utils.client().refresh_diagnostics_calls();

        // Scenario A: irrelevant .svg
        watch(&session, &[(CREATED, &p1_uri("icon.svg"))]);
        session.wait_for_background_tasks();
        let mut refresh_count = utils.client().refresh_diagnostics_calls();
        assert_eq!(
            refresh_count, baseline_refresh_count,
            "irrelevant .svg should not trigger refresh"
        );

        // Scenario B: relevant .ts
        watch(&session, &[(CREATED, &p1_uri("src/new.ts"))]);
        session.wait_for_background_tasks();
        refresh_count = utils.client().refresh_diagnostics_calls();
        assert!(
            refresh_count > baseline_refresh_count,
            "relevant .ts should trigger refresh"
        );
        baseline_refresh_count = refresh_count;

        // Scenario C: tsconfig.json
        watch(&session, &[(CHANGED, &p1_uri("tsconfig.json"))]);
        session.wait_for_background_tasks();
        refresh_count = utils.client().refresh_diagnostics_calls();
        assert!(
            refresh_count > baseline_refresh_count,
            "tsconfig.json should trigger refresh"
        );
        baseline_refresh_count = refresh_count;

        // Scenario D: directory creation (no extension)
        utils
            .fs_from_file_map()
            .mkdir_all(
                "home/projects/TS/p1/node_modules/@types",
                ts_goport::frontend::vfs::FileMode::PERM,
            )
            .unwrap();
        watch(&session, &[(CREATED, &p1_uri("node_modules/@types"))]);
        session.wait_for_background_tasks();
        refresh_count = utils.client().refresh_diagnostics_calls();
        assert!(
            refresh_count > baseline_refresh_count,
            "directory change should trigger refresh"
        );
        baseline_refresh_count = refresh_count;

        // Scenario E: mixed batch
        watch(
            &session,
            &[
                (CREATED, &p1_uri("icon.png")),
                (CHANGED, &p1_uri("src/index.ts")),
            ],
        );
        session.wait_for_background_tasks();
        refresh_count = utils.client().refresh_diagnostics_calls();
        assert!(
            refresh_count > baseline_refresh_count,
            "mixed batch with relevant file should trigger refresh"
        );
        baseline_refresh_count = refresh_count;

        // Scenario F: package install noise
        watch(
            &session,
            &[
                (CREATED, &p1_uri("node_modules/pkg/LICENSE")),
                (CREATED, &p1_uri("README.md")),
                (CREATED, &p1_uri("LICENSE.txt")),
                (CREATED, &p1_uri("style.css")),
            ],
        );
        session.wait_for_background_tasks();
        refresh_count = utils.client().refresh_diagnostics_calls();
        assert_eq!(
            refresh_count, baseline_refresh_count,
            "package install noise should not trigger refresh"
        );
    }
}

// ---------------------------------------------------------------------------
// Preferences
// ---------------------------------------------------------------------------

fn src_files() -> FileMap {
    files(&[
        ("/src/tsconfig.json", "{}"),
        ("/src/index.ts", "export const x = 1;"),
    ])
}

child_test! {
    // Go: session_test.go:1384 TestSession/refreshes code lenses and inlay hints when relevant user preferences change
    fn refreshes_code_lenses_and_inlay_hints_when_relevant_user_preferences_change() {
        let (session, utils) = projecttestutil::setup(src_files());
        open(&session, "file:///src/index.ts", "export const x = 1;");
        let _ = program(&session, "file:///src/index.ts");

        session.configure(lsutil::new_default_user_preferences());
        // Change user preferences for code lens and inlay hints.
        let mut new_prefs = session.config();
        new_prefs.code_lens.references_code_lens_enabled = Tristate::True;
        new_prefs.inlay_hints.include_inlay_function_like_return_type_hints = Tristate::True;

        session.configure(new_prefs);

        assert_eq!(
            utils.client().refresh_code_lens_calls(),
            1,
            "expected one RefreshCodeLens call after code lens preference change"
        );
        assert_eq!(
            utils.client().refresh_inlay_hints_calls(),
            1,
            "expected one RefreshInlayHints call after inlay hints preference change"
        );
    }
}

child_test! {
    // Go: session_test.go:1506 TestSession/sets locale when configured
    fn sets_locale_when_configured() {
        let (session, utils) = projecttestutil::setup(files(&[]));
        let mut prefs = lsutil::new_default_user_preferences();
        prefs.locale = "fr".to_string();

        session.configure(prefs);

        let set_locale_calls = utils.client().set_locale_calls();
        assert_eq!(set_locale_calls.len(), 1);
        assert_eq!(set_locale_calls[0], "fr");
    }
}

child_test! {
    // Go: session_test.go:1519 TestSession/locale change invalidates programs (tsgo#4712)
    fn locale_change_invalidates_programs() {
        let (session, _utils) = projecttestutil::setup(files(&[
            ("/src/tsconfig.json", "{}"),
            ("/src/index.ts", "export const x = 1;"),
        ]));
        let uri = "file:///src/index.ts";
        let config_path = "/src/tsconfig.json";
        open(&session, uri, "export const x = 1;");
        let _ = language_service(&session, uri);
        let program_of = |session: &Rc<Session>| {
            configured_project(session, config_path)
                .expect("configured project")
                .borrow()
                .program
                .clone()
                .expect("program")
        };
        let initial_program = program_of(&session);

        let mut preferences = session.config();
        preferences.code_lens.references_code_lens_enabled = Tristate::True;
        session.configure(preferences.clone());
        let _ = language_service(&session, uri);
        let program_after_code_lens_change = program_of(&session);
        assert!(Rc::ptr_eq(&program_after_code_lens_change, &initial_program));

        preferences.locale = "fr".to_string();
        session.configure(preferences);
        let _ = language_service(&session, uri);
        let program_after_locale_change = program_of(&session);
        assert!(!Rc::ptr_eq(&program_after_locale_change, &initial_program));
        // Go: defer session.Close()
        session.close();
    }
}

child_test! {
    // Go: session_test.go:1551 TestSession/adds locale to background contexts
    fn adds_locale_to_background_contexts() {
        let (session, utils) = projecttestutil::setup(files(&[]));
        let (fr, ok) = locale::parse("fr");
        assert!(ok);
        let get_locale_fr = fr.clone();
        *utils.client().get_locale_func.borrow_mut() = Some(Box::new(move || get_locale_fr.clone()));
        let code_lens_fr = fr.clone();
        *utils.client().refresh_code_lens_func.borrow_mut() = Some(Box::new(move |ctx: &ts_goport::gostd::Context| {
            assert_eq!(locale::from_context(ctx), code_lens_fr);
            Ok(())
        }));
        let mut prefs = lsutil::new_default_user_preferences();
        prefs.code_lens.references_code_lens_enabled = Tristate::True;

        session.configure(prefs);

        assert_eq!(utils.client().refresh_code_lens_calls(), 1);
    }
}

/// Go `map[string]any{"js/ts": {"preferences": {...}, "unstable": {...}}}`.
fn js_ts_config(
    use_aliases: bool,
    quote_style: &str,
    ignore_case: bool,
) -> indexmap::IndexMap<String, ts_goport::frontend::json_ext::LspAny> {
    use ts_goport::frontend::json_ext::LspAny;
    let obj = |entries: Vec<(&str, LspAny)>| {
        LspAny::Object(
            entries
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect(),
        )
    };
    let config = obj(vec![
        (
            "preferences",
            obj(vec![
                ("useAliasesForRenames", LspAny::Bool(use_aliases)),
                ("quoteStyle", LspAny::String(quote_style.to_string())),
            ]),
        ),
        (
            "unstable",
            obj(vec![(
                "organizeImportsIgnoreCase",
                LspAny::Bool(ignore_case),
            )]),
        ),
    ]);
    let mut items = indexmap::IndexMap::new();
    items.insert("js/ts".to_string(), config);
    items
}

child_test! {
    // Go: session_test.go:1409 TestSession/config parsing
    fn config_parsing() {
        let (session, _) = projecttestutil::setup(src_files());
        open(&session, "file:///src/index.ts", "export const x = 1;");
        let _ = program(&session, "file:///src/index.ts");

        session.configure(lsutil::parse_user_preferences(&js_ts_config(true, "single", true)));
        let actual_config1 = session.config();
        let mut expected_prefs1 = lsutil::new_default_user_preferences();
        expected_prefs1.use_aliases_for_rename = Tristate::True;
        expected_prefs1.quote_preference = lsutil::QuotePreference::SINGLE;
        expected_prefs1.organize_imports_ignore_case = Tristate::True;

        assert_eq!(actual_config1, expected_prefs1);

        session.configure(lsutil::parse_user_preferences(&js_ts_config(false, "double", false)));
        let actual_config2 = session.config();
        let mut expected_prefs2 = lsutil::new_default_user_preferences();
        expected_prefs2.use_aliases_for_rename = Tristate::False;
        expected_prefs2.quote_preference = lsutil::QuotePreference::DOUBLE;
        expected_prefs2.organize_imports_ignore_case = Tristate::False;

        assert_eq!(actual_config2, expected_prefs2);
    }
}

// ---------------------------------------------------------------------------
// Language service for closed files
// ---------------------------------------------------------------------------

child_test! {
    // Go: session_test.go:1460 TestSession/language service for closed files/closed file in configured project not yet opened
    fn language_service_for_closed_files_closed_file_in_configured_project_not_yet_opened() {
        let files = files(&[
            (
                "/home/projects/TS/p1/tsconfig.json",
                r#"{
					"compilerOptions": {
						"noLib": true,
						"strict": true
					},
					"include": ["src"]
				}"#,
            ),
            ("/home/projects/TS/p1/src/index.ts", "export const x: number = 1;"),
        ]);
        let (session, _) = projecttestutil::setup(files);

        // Do NOT open any file. Directly request language service for a closed file
        // that belongs to the configured project.
        let p = program(&session, &p1_uri("src/index.ts"));
        assert!(has_file(&p, &p1("src/index.ts")));
        assert_eq!(text(&p, &p1("src/index.ts")), "export const x: number = 1;");
    }
}

child_test! {
    // Go: session_test.go:1489 TestSession/language service for closed files/closed file with no configured project creates inferred project
    fn language_service_for_closed_files_closed_file_with_no_configured_project_creates_inferred_project() {
        let files = files(&[(
            "/home/projects/TS/loose/index.ts",
            r#"const greeting: string = "hello";"#,
        )]);
        let (session, _) = projecttestutil::setup(files);

        let p = program(&session, "file:///home/projects/TS/loose/index.ts");
        assert!(has_file(&p, "/home/projects/TS/loose/index.ts"));
        assert_eq!(
            text(&p, "/home/projects/TS/loose/index.ts"),
            r#"const greeting: string = "hello";"#
        );
    }
}

child_test! {
    // Go: session_test.go:1511 TestSession/jsconfig.json used for JS files when tsconfig.json exists in same directory
    fn jsconfig_json_used_for_js_files_when_tsconfig_json_exists_in_same_directory() {
        let files = files(&[
            (
                "/home/projects/TS/p1/tsconfig.json",
                r#"{
				"compilerOptions": {
					"noLib": true,
					"strict": true
				}
			}"#,
            ),
            (
                "/home/projects/TS/p1/jsconfig.json",
                r#"{
				"compilerOptions": {
					"noLib": true,
					"checkJs": true
				}
			}"#,
            ),
            ("/home/projects/TS/p1/index.ts", "export const x: number = 1;"),
            ("/home/projects/TS/p1/app.js", r#"/** @type {number} */ var y = "not a number";"#),
        ]);
        let (session, _) = projecttestutil::setup(files);

        // Open the JS file - it should be assigned to the jsconfig.json project, not tsconfig.json
        open_kind(
            &session,
            &p1_uri("app.js"),
            r#"/** @type {number} */ var y = "not a number";"#,
            lsproto::LanguageKind::JAVA_SCRIPT,
        );

        let default_project = session.snapshot().get_default_project(&uri(&p1_uri("app.js")));
        let default_project = default_project.expect("JS file should have a default project");
        assert_eq!(
            default_project.borrow().config_file_name(),
            "/home/projects/TS/p1/jsconfig.json",
            "JS file should belong to jsconfig.json project, not tsconfig.json"
        );

        // Open the TS file - it should be assigned to tsconfig.json project
        open(&session, &p1_uri("index.ts"), "export const x: number = 1;");

        let default_ts_project = session.snapshot().get_default_project(&uri(&p1_uri("index.ts")));
        let default_ts_project = default_ts_project.expect("TS file should have a default project");
        assert_eq!(
            default_ts_project.borrow().config_file_name(),
            "/home/projects/TS/p1/tsconfig.json",
            "TS file should belong to tsconfig.json project"
        );
    }
}

// ---------------------------------------------------------------------------
// Auto-import warm (no Go counterpart)
// ---------------------------------------------------------------------------

/// The names of the exports in the node_modules buckets of the session's
/// auto-import registry.
fn node_modules_export_names(session: &Rc<Session>) -> BTreeSet<String> {
    let registry = session
        .snapshot()
        .auto_import_registry()
        .expect("auto import registry");
    let mut names = BTreeSet::new();
    for bucket in registry.node_modules.values() {
        if let Some(index) = &bucket.index {
            names.extend(index.borrow().entries.iter().map(|export| export.name()));
        }
    }
    names
}

child_test! {
    // PORT: no Go counterpart. The auto-import warm runs on the dispatch
    // thread, and its registry build stops at extra points when a file event
    // cancels it (`registry::DISCARD_ON_CANCEL_KEY`). Here it stops in the
    // file walk of the alias checker. The warm drops the whole clone, as Go's
    // does (session.go:1844), so the session snapshot and its registry stay
    // as they were, and a later request builds the whole index.
    fn cancelled_auto_import_warm_leaves_snapshot_and_registry_unchanged() {
        const INDEX_URI: &str = "file:///home/projects/app/index.ts";
        // Only the alias checker of the warm reads this file: foo's
        // entrypoint re-exports it, and the program does not include it.
        const OTHER: &str = "/home/projects/node_modules/foo/other.d.ts";
        let (_, map_fs) = projecttestutil::wrapped_map_fs(
            files(&[
                ("/home/projects/app/tsconfig.json", "{}"),
                ("/home/projects/app/index.ts", ""),
                ("/home/projects/node_modules/foo/package.json", r#"{ "types": "index.d.ts" }"#),
                (
                    "/home/projects/node_modules/foo/index.d.ts",
                    "export const foo = 0;\nexport * from \"./other\";",
                ),
                (OTHER, "export declare const bar: number;"),
            ]),
            false, /*useCaseSensitiveFileNames*/
        );
        // Runs once, when the file system first reads OTHER.
        let on_read_other: Rc<RefCell<Option<Box<dyn FnOnce()>>>> = Rc::default();
        let fs = {
            let inner = map_fs.clone();
            let on_read_other = on_read_other.clone();
            wrapvfs_wrap(
                map_fs,
                Replacements {
                    read_file: Some(Box::new(move |path: &str| {
                        if path == OTHER {
                            let hook = on_read_other.borrow_mut().take();
                            if let Some(hook) = hook {
                                hook();
                            }
                        }
                        inner.read_file(path)
                    })),
                    ..Default::default()
                },
            )
        };
        // The options of `bare_session`.
        let session = project::new_session(&SessionInit {
            background_ctx: bg(),
            options: Rc::new(SessionOptions {
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
        });

        open(&session, INDEX_URI, "");
        session.wait_for_background_tasks();
        // A change of one open file queues a warm (Go warmAutoImportCache).
        edit(&session, INDEX_URI, 2, (0, 0), (0, 0), "let a = 1;");
        let _ = language_service(&session, INDEX_URI);
        let before = session.snapshot();
        let registry_before = before.auto_import_registry().expect("auto import registry");
        let stats_before = format!("{:?}", registry_before.get_cache_stats());

        // A file event arrives while the warm reads OTHER (Go DidChangeFile
        // cancels the warm).
        let weak = Rc::downgrade(&session);
        *on_read_other.borrow_mut() = Some(Box::new(move || {
            if let Some(session) = weak.upgrade() {
                session.cancel_warm_auto_import_cache();
            }
        }));
        session.wait_for_background_tasks();
        assert!(on_read_other.borrow().is_none(), "the warm did not read {OTHER}");

        let after = session.snapshot();
        assert!(Rc::ptr_eq(&after, &before), "the cancelled warm changed the snapshot");
        let registry_after = after.auto_import_registry().expect("auto import registry");
        assert!(Rc::ptr_eq(&registry_after, &registry_before));
        assert_eq!(format!("{:?}", registry_after.get_cache_stats()), stats_before);

        // The warm left no partial state: a request builds the whole index.
        session
            .get_current_language_service_with_auto_imports(&bg(), &uri(INDEX_URI))
            .unwrap_or_else(|err| panic!("{}", err.error()));
        let names = node_modules_export_names(&session);
        assert!(names.contains("foo") && names.contains("bar"), "{names:?}");
        session.close();
    }
}

/// The session of the auto-import warm tests: an open index.ts whose last
/// change queued a warm that has not run yet. Only the alias checker of the
/// warm reads OTHER (foo's entrypoint re-exports it, and the program does
/// not include it). Returns the session, the hook that runs once at the next
/// read of OTHER, and the number of reads of OTHER.
fn session_with_pending_warm() -> (
    Rc<Session>,
    Rc<RefCell<Option<Box<dyn FnOnce()>>>>,
    Rc<std::cell::Cell<usize>>,
) {
    const INDEX_URI: &str = "file:///home/projects/app/index.ts";
    const OTHER: &str = "/home/projects/node_modules/foo/other.d.ts";
    let (_, map_fs) = projecttestutil::wrapped_map_fs(
        files(&[
            ("/home/projects/app/tsconfig.json", "{}"),
            ("/home/projects/app/index.ts", ""),
            (
                "/home/projects/node_modules/foo/package.json",
                r#"{ "types": "index.d.ts" }"#,
            ),
            (
                "/home/projects/node_modules/foo/index.d.ts",
                "export const foo = 0;\nexport * from \"./other\";",
            ),
            (OTHER, "export declare const bar: number;"),
        ]),
        false, /*useCaseSensitiveFileNames*/
    );
    let on_read_other: Rc<RefCell<Option<Box<dyn FnOnce()>>>> = Rc::default();
    let reads = Rc::new(std::cell::Cell::new(0));
    let fs = {
        let inner = map_fs.clone();
        let on_read_other = on_read_other.clone();
        let reads = reads.clone();
        wrapvfs_wrap(
            map_fs,
            Replacements {
                read_file: Some(Box::new(move |path: &str| {
                    if path == OTHER {
                        reads.set(reads.get() + 1);
                        let hook = on_read_other.borrow_mut().take();
                        if let Some(hook) = hook {
                            hook();
                        }
                    }
                    inner.read_file(path)
                })),
                ..Default::default()
            },
        )
    };
    let session = project::new_session(&SessionInit {
        background_ctx: bg(),
        options: Rc::new(SessionOptions {
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
    });
    open(&session, INDEX_URI, "");
    session.wait_for_background_tasks();
    // A change of one open file queues a warm (Go warmAutoImportCache).
    edit(&session, INDEX_URI, 2, (0, 0), (0, 0), "let a = 1;");
    let _ = language_service(&session, INDEX_URI);
    (session, on_read_other, reads)
}

/// A message other than a file event, as the LSP reader thread sees it
/// while the warm's clone runs: it waits for the hold, then makes the
/// attempt yield. Returns the message's wait, from the queue call.
fn yield_warm_attempt(session: &Rc<Session>) -> Duration {
    let preempt = session.warm_auto_import_preempt.clone();
    std::thread::spawn(move || {
        let logger: Option<Rc<dyn project::logging::Logger>> = None;
        let mut queued_at = None;
        preempt.on_message(false, &logger, || queued_at = Some(Instant::now()));
        queued_at.expect("message queued").elapsed()
    })
    .join()
    .expect("reader thread")
}

child_test! {
    // PORT: no Go counterpart (lswarm1). A message that does not wait for
    // the first (eager) attempt of the warm makes it yield. The warm drops
    // that clone, keeps its snapshot, and runs again later; its second clone
    // is adopted, as Go's one clone is.
    fn yielded_auto_import_warm_runs_again_and_is_adopted() {
        let (session, on_read_other, reads) = session_with_pending_warm();
        let before = session.snapshot();
        let weak = Rc::downgrade(&session);
        *on_read_other.borrow_mut() = Some(Box::new(move || {
            if let Some(session) = weak.upgrade() {
                yield_warm_attempt(&session);
            }
        }));
        session.wait_for_background_tasks();
        assert!(on_read_other.borrow().is_none(), "the warm did not read the file");
        assert_eq!(reads.get(), 2, "one read per attempt");

        let after = session.snapshot();
        assert!(!Rc::ptr_eq(&after, &before), "the warm's clone was not adopted");
        let names = node_modules_export_names(&session);
        assert!(names.contains("foo") && names.contains("bar"), "{names:?}");
        session.close();
    }
}

child_test! {
    // PORT: no Go counterpart (lswarm1). A yielded warm whose session has
    // moved past its snapshot ends without a second clone: Go's adopt
    // (session.go:1311) would discard it.
    fn yielded_auto_import_warm_ends_when_the_session_moves() {
        let (session, on_read_other, reads) = session_with_pending_warm();
        let weak = Rc::downgrade(&session);
        *on_read_other.borrow_mut() = Some(Box::new(move || {
            if let Some(session) = weak.upgrade() {
                yield_warm_attempt(&session);
            }
        }));
        // The snapshot task queues the warm, and its first attempt yields.
        ts_goport::gostd::local::run_pending();
        assert!(ts_goport::gostd::local::run_idle());
        assert_eq!(reads.get(), 1);
        assert!(session.warm_auto_import_slow.get(), "the eager attempt yielded");

        // A request with auto-imports adopts its own clone (Go
        // GetLanguageServiceWithAutoImports), so the session moves.
        let base = session.snapshot();
        session
            .get_current_language_service_with_auto_imports(&bg(), &uri("file:///home/projects/app/index.ts"))
            .unwrap_or_else(|err| panic!("{}", err.error()));
        ts_goport::gostd::local::run_pending();
        let moved = session.snapshot();
        assert!(!Rc::ptr_eq(&moved, &base), "the session did not move");

        let reads_before_retry = reads.get();
        assert!(ts_goport::gostd::local::run_idle(), "no retry was queued");
        assert_eq!(reads.get(), reads_before_retry, "the retry cloned");
        assert!(Rc::ptr_eq(&session.snapshot(), &moved));
        assert!(session.warm_auto_import_pending.borrow().is_none());
        assert!(!ts_goport::gostd::local::run_idle());
        session.close();
    }
}

child_test! {
    // PORT: no Go counterpart (lswarm1). The first attempt of a warm starts
    // as soon as no message waits, and a message that comes during its
    // clone waits up to the hold (Go's head start): the time since Go's warm
    // would have started, at most `WARM_AUTO_IMPORT_HOLD_CAP`
    // (`Session::run_pending_warm`). Here the attempt starts at least the
    // cap after that time, so the message waits the whole cap, whatever the
    // load. Only then does the attempt yield, and its retry waits for a
    // quiet period.
    fn an_eager_auto_import_warm_holds_a_message_for_the_cap() {
        use ts_goport::gostd::local::{self, IdleStart};
        let (session, on_read_other, reads) = session_with_pending_warm();
        // The snapshot task has not run, so it starts after `t0`.
        assert_eq!(local::next_idle(), None);
        let t0 = Instant::now();
        local::run_pending();
        let t1 = Instant::now();
        assert_eq!(local::next_idle(), Some(IdleStart::AtOnce), "the first attempt waits");
        // The hold counts from queued_at + (queued_at - task start), which is
        // at most t1 + (t1 - t0). The attempt starts at least the cap later.
        std::thread::sleep((t1 - t0) + project::WARM_AUTO_IMPORT_HOLD_CAP);
        let waited = Rc::new(std::cell::Cell::new(None));
        {
            let waited = waited.clone();
            let weak = Rc::downgrade(&session);
            *on_read_other.borrow_mut() = Some(Box::new(move || {
                if let Some(session) = weak.upgrade() {
                    waited.set(Some(yield_warm_attempt(&session)));
                }
            }));
        }
        assert!(local::run_idle());
        let waited = waited.get().expect("the warm did not read the file");
        assert!(waited >= project::WARM_AUTO_IMPORT_HOLD_CAP, "{waited:?}");
        assert_eq!(reads.get(), 1);
        assert!(session.warm_auto_import_slow.get(), "the eager attempt yielded");
        assert_eq!(local::next_idle(), Some(IdleStart::AfterQuiet), "the retry is eager");
        session.close();
    }
}

child_test! {
    // PORT: no Go counterpart (lswarm1). A clone that takes longer than
    // `WARM_AUTO_IMPORT_HOLD_CAP` ends, is adopted, and keeps the session
    // marked slow, so the next warm waits for a quiet period.
    fn a_long_auto_import_warm_clone_marks_the_session_slow() {
        use ts_goport::gostd::local::{self, IdleStart};
        let (session, on_read_other, reads) = session_with_pending_warm();
        *on_read_other.borrow_mut() = Some(Box::new(|| {
            std::thread::sleep(project::WARM_AUTO_IMPORT_HOLD_CAP + Duration::from_millis(1));
        }));
        let before = session.snapshot();
        local::run_pending();
        assert_eq!(local::next_idle(), Some(IdleStart::AtOnce), "the first attempt waits");
        assert!(local::run_idle());
        assert_eq!(reads.get(), 1);
        assert!(!Rc::ptr_eq(&session.snapshot(), &before), "the clone was not adopted");
        assert!(session.warm_auto_import_slow.get(), "a long clone left the session eager");
        session.close();
    }
}

child_test! {
    // PORT: no Go counterpart (lswarm1, M10). A clone that ends within
    // `WARM_AUTO_IMPORT_HOLD_CAP` clears the slow mark, so the next warm is
    // eager again. The check holds by order, not by timing: it reads the mark
    // only after an attempt that took less than the cap in all (a bound on
    // its clone), and takes a new session until an attempt does.
    fn a_short_auto_import_warm_clone_clears_the_slow_mark() {
        use ts_goport::gostd::local;
        let mut attempts = Vec::new();
        for _ in 0..20 {
            let (session, _, reads) = session_with_pending_warm();
            session.warm_auto_import_slow.set(true);
            local::run_pending();
            let before = session.snapshot();
            let start = Instant::now();
            assert!(local::run_idle());
            let attempt = start.elapsed();
            assert_eq!(reads.get(), 1);
            assert!(!Rc::ptr_eq(&session.snapshot(), &before), "the clone was not adopted");
            let slow = session.warm_auto_import_slow.get();
            session.close();
            if attempt <= project::WARM_AUTO_IMPORT_HOLD_CAP {
                assert!(!slow, "a clone within {attempt:?} kept the slow mark");
                return;
            }
            attempts.push(attempt);
        }
        panic!("no attempt took less than the cap: {attempts:?}");
    }
}

/// Ends the test process unless the returned flag is set within 60 s, for
/// a `wait_for_background_tasks` that would not return.
fn exit_unless_returned_in_60_s() -> std::sync::Arc<std::sync::atomic::AtomicBool> {
    use std::sync::atomic::{AtomicBool, Ordering};
    let returned = std::sync::Arc::new(AtomicBool::new(false));
    let watched = returned.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(60));
        if !watched.load(Ordering::SeqCst) {
            eprintln!("wait_for_background_tasks did not return in 60 s");
            std::process::exit(101);
        }
    });
    returned
}

child_test! {
    // PORT: no Go counterpart (lswarm1). While a message waits for the
    // dispatch thread, a warm attempt does not start and queues itself
    // again. `wait_for_background_tasks` then returns, where it would run
    // that attempt forever (the message cannot run while it waits). Once no
    // message waits, it runs the warm.
    fn wait_for_background_tasks_returns_while_a_message_waits() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};
        let (session, _, reads) = session_with_pending_warm();
        let busy = Arc::new(AtomicBool::new(true));
        {
            let busy = busy.clone();
            session
                .warm_auto_import_preempt
                .set_busy(Box::new(move || busy.load(Ordering::SeqCst)));
        }
        let returned = exit_unless_returned_in_60_s();
        let before = session.snapshot();
        session.wait_for_background_tasks();
        returned.store(true, Ordering::SeqCst);
        assert_eq!(reads.get(), 0, "the warm cloned while a message waited");
        assert!(session.warm_auto_import_pending.borrow().is_some(), "the warm ended");

        busy.store(false, Ordering::SeqCst);
        session.wait_for_background_tasks();
        assert_eq!(reads.get(), 1);
        assert!(!Rc::ptr_eq(&session.snapshot(), &before), "the clone was not adopted");
        session.close();
    }
}

child_test! {
    // PORT: no Go counterpart (lswarm1). A message that comes while
    // `wait_for_background_tasks` runs the idle work stops the wait too, so
    // the wait reads the busy check before each run, not only once. Here
    // the first read sees no message and every later read sees one: the warm
    // attempt does not start and queues itself again, and the wait returns.
    fn wait_for_background_tasks_returns_when_a_message_comes_during_the_wait() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};
        let (session, _, reads) = session_with_pending_warm();
        let busy_reads = Arc::new(AtomicUsize::new(0));
        {
            let busy_reads = busy_reads.clone();
            session
                .warm_auto_import_preempt
                .set_busy(Box::new(move || busy_reads.fetch_add(1, Ordering::SeqCst) > 0));
        }
        let returned = exit_unless_returned_in_60_s();
        session.wait_for_background_tasks();
        returned.store(true, Ordering::SeqCst);
        assert_eq!(reads.get(), 0, "the warm cloned while a message waited");
        assert!(session.warm_auto_import_pending.borrow().is_some(), "the warm ended");
        session.close();
    }
}

child_test! {
    // PORT: no Go counterpart (editfuzz2 P2-1). Only a released program
    // version loaded the node_modules entrypoint pk/node.d.ts; the parse
    // cache keeps its parse. The warm's auto-import extraction of package pk
    // reads the JSDoc of its export (the `@deprecated` tag). Go's
    // `SourceFile` parses lazy JSDoc from its own text (ast.go:2745
    // resolveJSDoc), so it needs no program. Before the fix the port found
    // no parser inputs for the file and exited with `unported!`.
    fn auto_import_warm_reads_jsdoc_of_a_file_that_only_a_released_program_loaded() {
        const A_URI: &str = "file:///home/projects/app/a.ts";
        const B_URI: &str = "file:///home/projects/app/b.ts";
        const NODE_DTS: &str = "/home/projects/app/node_modules/pk/node.d.ts";
        let session = bare_session(files(&[
            (
                "/home/projects/app/package.json",
                r#"{ "name": "r", "devDependencies": { "cov": "1.0.0", "pk": "1.0.0" } }"#,
            ),
            (
                "/home/projects/app/tsconfig.json",
                r#"{ "compilerOptions": { "module": "esnext", "moduleResolution": "bundler", "strict": true }, "include": ["*.ts"] }"#,
            ),
            ("/home/projects/app/a.ts", "export const a = 1;\n"),
            ("/home/projects/app/b.ts", "export const b = 1;\n"),
            (
                "/home/projects/app/node_modules/pk/package.json",
                r#"{ "name": "pk", "version": "1.0.0", "exports": { ".": { "types": "./index.d.ts" }, "./node": { "types": "./node.d.ts" } } }"#,
            ),
            (
                "/home/projects/app/node_modules/pk/index.d.ts",
                "/** The main value. */\nexport declare const pkMain: number;\n",
            ),
            (
                NODE_DTS,
                "/** @deprecated Use pkMain. */\nexport declare function pkNode(): void;\n",
            ),
            (
                "/home/projects/app/node_modules/cov/package.json",
                r#"{ "name": "cov", "version": "1.0.0", "types": "./index.d.ts" }"#,
            ),
            (
                "/home/projects/app/node_modules/cov/index.d.ts",
                "import { pkNode } from \"pk/node\";\nexport declare const cov: typeof pkNode;\n",
            ),
        ]));
        open(&session, A_URI, "export const a = 1;\n");
        open(&session, B_URI, "export const b = 1;\n");
        // A program version with cov and pk/node.d.ts, and a registry with cov.
        edit(&session, A_URI, 2, (0, 0), (0, 0), "import * as c from \"cov\";\n");
        session
            .get_current_language_service_with_auto_imports(&bg(), &uri(A_URI))
            .unwrap_or_else(|err| panic!("{}", err.error()));
        let with_node_dts = Rc::downgrade(&program(&session, A_URI));
        assert!(with_node_dts.upgrade().is_some_and(|p| has_file(&p, NODE_DTS)));

        // The disk text of a.ts has no import, so pk/node.d.ts leaves the
        // program, and the old version is released.
        close(&session, A_URI);
        // A change of one open file queues a warm.
        edit(&session, B_URI, 2, (0, 0), (0, 0), "import { pkMain } from \"pk\";\n");
        assert!(!has_file(&program(&session, B_URI), NODE_DTS));
        session.wait_for_background_tasks();
        assert!(
            with_node_dts.upgrade().is_none(),
            "the version that loaded {NODE_DTS} is alive"
        );

        // The warm extracted package pk with both entrypoints, and read the
        // JSDoc of pkNode.
        let registry = session
            .snapshot()
            .auto_import_registry()
            .expect("auto import registry");
        let pk_node = registry
            .node_modules
            .values()
            .filter_map(|bucket| bucket.index.as_ref())
            .flat_map(|index| index.borrow().entries.clone())
            .find(|export| export.name() == "pkNode")
            .expect("an export pkNode in the node_modules buckets");
        assert!(
            pk_node
                .script_element_kind_modifiers
                .contains(lsutil::ScriptElementKindModifier::DEPRECATED),
            "{:?}",
            pk_node.script_element_kind_modifiers
        );
        session.close();
    }
}

child_test! {
    // PORT: no Go counterpart (editfuzz2 P1). The parser allows any
    // expression as a module specifier or an import attribute value. Go's
    // `Node.Text` panics for a kind with no text (ast.go:308): in
    // coalesceExportsWorker (organizeimports.go:851) and in
    // getImportAttributesKey (:380). The server answers that panic as Go's
    // InternalError, so the port panics with Go's text.
    fn organize_imports_panics_like_go_on_an_expression_with_no_text() {
        const B_URI: &str = "file:///home/projects/app/b.ts";
        let cases = [
            ("export * as a.b from './a';\n", "PropertyAccessExpression"),
            ("export * from this;\n", "KeywordExpression"),
            (
                "import { A } from './a' with { type: f() };\nexport const c = A;\n",
                "CallExpression",
            ),
        ];
        for (text, data) in cases {
            let session = bare_session(files(&[
                ("/home/projects/app/a.ts", "export class A {}\n"),
                ("/home/projects/app/b.ts", text),
            ]));
            open(&session, B_URI, text);
            let ls = language_service(&session, B_URI);
            let params = lsproto::CodeActionParams {
                text_document: lsproto::TextDocumentIdentifier { uri: uri(B_URI) },
                context: Some(lsproto::CodeActionContext {
                    only: Some(vec![lsproto::CodeActionKind::SOURCE_ORGANIZE_IMPORTS]),
                    ..Default::default()
                }),
                ..Default::default()
            };
            let payload = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                ls.provide_code_actions(&bg(), &params)
            }))
            .expect_err("organize imports must panic as Go does");
            let message = payload
                .downcast_ref::<ts_goport::core::GoPanic>()
                .map(|panic| panic.message.clone());
            assert_eq!(
                message.as_deref(),
                Some(format!("Unhandled case in Node.Text: *ast.{data}").as_str()),
                "{text:?}"
            );
            session.close();
        }
    }
}
