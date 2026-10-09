//! Port of Go `internal/project/ata/ata_test.go` (`TestATA`).

use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::ThreadId;

use ts_goport::frontend::json_ext::LspAny;
use ts_goport::gostd::Context;
use ts_goport::ls::lsutil;
use ts_goport::lsp::lsproto;
use ts_goport::project::Session;

use super::projecttestutil::{
    self, FileMap, NpmInstallCall, SessionUtils, TEST_TYPINGS_LOCATION, TypingsInstallerOptions,
    files,
};
use super::util::*;

const APP: &str = "/user/username/projects/project/app.js";
const APP_URI: &str = "file:///user/username/projects/project/app.js";

fn ti(types_registry: &[&str], package_to_file: &[(&str, &str)]) -> TypingsInstallerOptions {
    TypingsInstallerOptions {
        types_registry: types_registry.iter().map(|s| s.to_string()).collect(),
        package_to_file: package_to_file
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect::<BTreeMap<_, _>>(),
    }
}

fn file_text(files: &FileMap, name: &str) -> String {
    String::from_utf8(files[name].data.clone()).unwrap()
}

/// Setup with the typings installer, open `APP` as JavaScript and wait for
/// the background tasks (the common start of the Go subtests).
fn open_app(files: FileMap, ti_options: TypingsInstallerOptions) -> (Rc<Session>, SessionUtils) {
    let text = file_text(&files, APP);
    let (session, utils) = projecttestutil::setup_with_typings_installer(files, ti_options);
    open_kind(&session, APP_URI, &text, lsproto::LanguageKind::JAVA_SCRIPT);
    session.wait_for_background_tasks();
    (session, utils)
}

fn calls(utils: &SessionUtils) -> Vec<NpmInstallCall> {
    utils.npm_executor().npm_install_calls()
}

fn types_registry_call(call: &NpmInstallCall) {
    assert_eq!(call.cwd, TEST_TYPINGS_LOCATION);
    assert_eq!(
        call.args,
        ["install", "--ignore-scripts", "types-registry@latest"]
    );
}

fn typings_file(name: &str) -> String {
    format!("{TEST_TYPINGS_LOCATION}/node_modules/@types/{name}/index.d.ts")
}

child_test! {
    // Go: ata_test.go:22 TestATA/local module should not be picked up
    fn local_module_should_not_be_picked_up() {
        let files = files(&[
            (APP, "const c = require('./config');"),
            ("/user/username/projects/project/config.js", "export let x = 1"),
            (
                "/user/username/projects/project/jsconfig.json",
                r#"{
					"compilerOptions": { "moduleResolution": "commonjs" },
					"typeAcquisition": { "enable": true }
			}"#,
            ),
        ]);
        let (session, utils) = open_app(files, ti(&["config"], &[]));
        let p = program(&session, APP_URI);
        // Verify the local config.js file is included in the program
        assert!(
            has_file(&p, "/user/username/projects/project/config.js"),
            "local config.js should be included"
        );

        // Verify that only types-registry was installed (no @types/config since it's a local module)
        let npm_calls = calls(&utils);
        assert_eq!(npm_calls.len(), 1);
        assert_eq!(npm_calls[0].args[2], "types-registry@latest");
    }
}

const PACKAGE_JSON_JQUERY: &str = r#"{
				"name": "test",
				"dependencies": {
					"jquery": "^3.1.0"
				}
			}"#;

child_test! {
    // Go: ata_test.go:58 TestATA/configured projects
    fn configured_projects() {
        let files = files(&[
            (APP, ""),
            (
                "/user/username/projects/project/tsconfig.json",
                r#"{
				"compilerOptions": { "allowJs": true },
				"typeAcquisition": { "enable": true },
			}"#,
            ),
            ("/user/username/projects/project/package.json", PACKAGE_JSON_JQUERY),
        ]);
        let (_session, utils) = open_app(files, ti(&[], &[("jquery", "declare const $: { x: number }")]));
        let npm_calls = calls(&utils);
        assert_eq!(npm_calls.len(), 2);
        assert_eq!(npm_calls[0].cwd, TEST_TYPINGS_LOCATION);
        assert_eq!(npm_calls[0].args[2], "types-registry@latest");
        assert_eq!(npm_calls[1].cwd, TEST_TYPINGS_LOCATION);
        assert!(npm_calls[1].args.iter().any(|a| a == "@types/jquery@latest"));
        assert_eq!(utils.client().refresh_diagnostics_calls(), 1);
    }
}

child_test! {
    // Go: ata_test.go:92 TestATA/inferred projects
    fn inferred_projects() {
        let files = files(&[(APP, ""), ("/user/username/projects/project/package.json", PACKAGE_JSON_JQUERY)]);
        let (session, utils) = open_app(files, ti(&[], &[("jquery", "declare const $: { x: number }")]));
        // Check that npm install was called twice
        let calls = calls(&utils);
        assert_eq!(2, calls.len(), "Expected exactly 2 npm install calls");
        types_registry_call(&calls[0]);
        assert_eq!(calls[1].cwd, TEST_TYPINGS_LOCATION);
        assert_eq!(calls[1].args[2], "@types/jquery@latest");

        // Verify the types file was installed
        let p = program(&session, APP_URI);
        assert!(has_file(&p, &typings_file("jquery")), "jquery types should be installed");
    }
}

child_test! {
    // followups4 (R153 reviewer): `inferred_projects` with an executor that
    // gives `npm_install_func`, as the LSP server's does. ATA runs each npm
    // call on a helper thread, and the post of its result wakes the request
    // on the session thread (`TypingsInstaller::npm_install`).
    fn inferred_projects_with_npm_on_a_helper_thread() {
        let files = files(&[(APP, ""), ("/user/username/projects/project/package.json", PACKAGE_JSON_JQUERY)]);
        let ti_options = ti(&[], &[("jquery", "declare const $: { x: number }")]);
        let registry = projecttestutil::create_types_registry_file_content(&ti_options);
        let (session, utils) = projecttestutil::setup_with_typings_installer(files, ti_options);
        let map = projecttestutil::current_map_fs_for_test();
        let thread_calls: Arc<Mutex<Vec<(ThreadId, Vec<String>)>>> = Arc::default();
        let log = Arc::clone(&thread_calls);
        *utils.npm_executor().npm_install_on_thread.borrow_mut() =
            Some(Arc::new(move |_ctx: &Context, cwd: &str, args: &[String]| {
                log.lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push((std::thread::current().id(), args.to_vec()));
                let (path, text) = if args[2] == "types-registry@latest" {
                    (format!("{cwd}/node_modules/types-registry/index.json"), registry.as_str())
                } else {
                    (typings_file("jquery"), "declare const $: { x: number }")
                };
                map.fs().write_file(&path, text).expect("write the npm output");
                (Vec::new(), None)
            }));

        open_kind(&session, APP_URI, "", lsproto::LanguageKind::JAVA_SCRIPT);
        session.wait_for_background_tasks();

        assert!(calls(&utils).is_empty(), "npm ran on the session thread");
        let thread_calls = thread_calls.lock().unwrap_or_else(PoisonError::into_inner).clone();
        let args: Vec<&str> = thread_calls.iter().map(|(_, args)| args[2].as_str()).collect();
        assert_eq!(args, ["types-registry@latest", "@types/jquery@latest"]);
        let session_thread = std::thread::current().id();
        assert!(thread_calls.iter().all(|(thread, _)| *thread != session_thread));
        let p = program(&session, APP_URI);
        assert!(has_file(&p, &typings_file("jquery")), "jquery types should be installed");
    }
}

child_test! {
    // Go: ata_test.go:129 TestATA/type acquisition with disableFilenameBasedTypeAcquisition:true
    fn type_acquisition_with_disable_filename_based_type_acquisition_true() {
        let files = files(&[
            ("/user/username/projects/project/jquery.js", ""),
            (
                "/user/username/projects/project/tsconfig.json",
                r#"{
				"compilerOptions": { "allowJs": true },
				"typeAcquisition": { "enable": true, "disableFilenameBasedTypeAcquisition": true }
			}"#,
            ),
        ]);

        let (session, utils) = projecttestutil::setup_with_typings_installer(files, ti(&["jquery"], &[]));

        // Should only get types-registry install, no jquery install since filename-based acquisition is disabled
        open_kind(
            &session,
            "file:///user/username/projects/project/jquery.js",
            "",
            lsproto::LanguageKind::JAVA_SCRIPT,
        );
        session.wait_for_background_tasks();

        // Check that npm install was called once (only types-registry)
        let calls = calls(&utils);
        assert_eq!(1, calls.len(), "Expected exactly 1 npm install call");
        types_registry_call(&calls[0]);
    }
}

/// The files of the "discover from node_modules" subtests with this jsconfig and app.js.
fn node_modules_files(app: &str, package_json: &str, jsconfig: &str) -> FileMap {
    files(&[
        (APP, app),
        ("/user/username/projects/project/package.json", package_json),
        ("/user/username/projects/project/jsconfig.json", jsconfig),
        (
            "/user/username/projects/project/node_modules/commander/index.js",
            "",
        ),
        (
            "/user/username/projects/project/node_modules/commander/package.json",
            r#"{ "name": "commander" }"#,
        ),
        (
            "/user/username/projects/project/node_modules/jquery/index.js",
            "",
        ),
        (
            "/user/username/projects/project/node_modules/jquery/package.json",
            r#"{ "name": "jquery" }"#,
        ),
        (
            "/user/username/projects/project/node_modules/jquery/nested/package.json",
            r#"{ "name": "nested" }"#,
        ),
    ])
}

fn node_modules_ti() -> TypingsInstallerOptions {
    ti(
        &["nested", "commander"],
        &[("jquery", "declare const jquery: { x: number }")],
    )
}

child_test! {
    // Go: ata_test.go:155 TestATA/discover from node_modules
    fn discover_from_node_modules() {
        let files = node_modules_files(
            "",
            r#"{
			    "dependencies": {
					"jquery": "1.0.0"
				}
			}"#,
            "{}",
        );
        let (_session, utils) = open_app(files, node_modules_ti());

        // Check that npm install was called twice
        let calls = calls(&utils);
        assert_eq!(2, calls.len(), "Expected exactly 2 npm install calls");
        types_registry_call(&calls[0]);
        assert_eq!(calls[1].cwd, TEST_TYPINGS_LOCATION);
        assert_eq!(calls[1].args[2], "@types/jquery@latest");
    }
}

child_test! {
    // Go: ata_test.go:192 TestATA/discover from node_modules empty types
    fn discover_from_node_modules_empty_types() {
        let files = node_modules_files(
            "",
            r#"{"dependencies": {"jquery": "1.0.0"}}"#,
            r#"{"compilerOptions": {"types": []}}"#,
        );
        let (_session, utils) = open_app(files, node_modules_ti());

        // Only types-registry should be installed
        let calls = calls(&utils);
        assert_eq!(1, calls.len());
        types_registry_call(&calls[0]);
    }
}

child_test! {
    // Go: ata_test.go:222 TestATA/discover from node_modules explicit types
    fn discover_from_node_modules_explicit_types() {
        let files = node_modules_files(
            "",
            r#"{"dependencies": {"jquery": "1.0.0"}}"#,
            r#"{"compilerOptions": {"types": ["jquery"]}}"#,
        );
        let (_session, utils) = open_app(files, node_modules_ti());

        // Only types-registry should be installed
        let calls = calls(&utils);
        assert_eq!(1, calls.len());
        types_registry_call(&calls[0]);
    }
}

child_test! {
    // Go: ata_test.go:252 TestATA/discover from node_modules empty types has import
    fn discover_from_node_modules_empty_types_has_import() {
        let files = node_modules_files(
            r#"import "jquery";"#,
            r#"{"dependencies": {"jquery": "1.0.0"}}"#,
            r#"{"compilerOptions": {"types": []}}"#,
        );
        let (_session, utils) = open_app(files, node_modules_ti());

        // types-registry + jquery types
        let calls = calls(&utils);
        assert_eq!(2, calls.len());
        types_registry_call(&calls[0]);
        assert!(calls[1].args.iter().any(|a| a == "@types/jquery@latest"));
    }
}

/// The checks of "discover from bower_components" and "discover from bower.json".
fn two_calls_and_jquery_installed(session: &Rc<Session>, utils: &SessionUtils) {
    // Check that npm install was called twice
    let calls = calls(utils);
    assert_eq!(2, calls.len(), "Expected exactly 2 npm install calls");
    types_registry_call(&calls[0]);
    assert_eq!(calls[1].cwd, TEST_TYPINGS_LOCATION);
    assert_eq!(calls[1].args[2], "@types/jquery@latest");

    // Verify the types file was installed
    let p = program(session, APP_URI);
    assert!(
        has_file(&p, &typings_file("jquery")),
        "jquery types should be installed"
    );
}

child_test! {
    // Go: ata_test.go:283 TestATA/discover from bower_components
    fn discover_from_bower_components() {
        let files = files(&[
            (APP, ""),
            ("/user/username/projects/project/jsconfig.json", "{}"),
            ("/user/username/projects/project/bower_components/jquery/index.js", ""),
            ("/user/username/projects/project/bower_components/jquery/bower.json", r#"{ "name": "jquery" }"#),
        ]);
        let (session, utils) = open_app(files, ti(&[], &[("jquery", "declare const jquery: { x: number }")]));
        two_calls_and_jquery_installed(&session, &utils);
    }
}

child_test! {
    // Go: ata_test.go:317 TestATA/discover from bower.json
    fn discover_from_bower_json() {
        let files = files(&[
            (APP, ""),
            ("/user/username/projects/project/jsconfig.json", "{}"),
            (
                "/user/username/projects/project/bower.json",
                r#"{
				"dependencies": {
                    "jquery": "^3.1.0"
                }
			}"#,
            ),
        ]);
        let (session, utils) = open_app(files, ti(&[], &[("jquery", "declare const jquery: { x: number }")]));
        two_calls_and_jquery_installed(&session, &utils);
    }
}

child_test! {
    // Go: ata_test.go:354 TestATA/Malformed package.json should be watched
    fn malformed_package_json_should_be_watched() {
        let files = files(&[
            (APP, ""),
            ("/user/username/projects/project/package.json", r#"{"dependencies": { "co } }"#),
        ]);
        let (session, utils) = open_app(files, ti(&[], &[("commander", "export let x: number")]));

        // Initially only types-registry update attempted
        let first = calls(&utils);
        assert_eq!(1, first.len());
        types_registry_call(&first[0]);

        // Fix package.json and notify watcher
        utils
            .fs()
            .write_file(
                "/user/username/projects/project/package.json",
                r#"{ "dependencies": { "commander": "0.0.2" } }"#,
            )
            .unwrap();
        watch(&session, &[(CHANGED, "file:///user/username/projects/project/package.json")]);
        // diagnostics refresh triggered - simulate by getting the language service
        let _ = session.get_language_service(&bg(), &uri(APP_URI));
        session.wait_for_background_tasks();

        let second = calls(&utils);
        assert_eq!(2, second.len());
        assert!(second[1].args.iter().any(|a| a == "@types/commander@latest"));

        // Verify types file present
        let p = program(&session, APP_URI);
        assert!(has_file(&p, &typings_file("commander")));
    }
}

child_test! {
    // Go: ata_test.go:401 TestATA/should redo resolution that resolved to '.js' file after typings are installed
    fn should_redo_resolution_that_resolved_to_js_file_after_typings_are_installed() {
        let files = files(&[
            // PORT: the Go literal is a raw string with `\n` escapes (not newlines).
            (APP, r#"\n                import * as commander from "commander";\n            "#),
            ("/user/username/projects/node_modules/commander/index.js", "module.exports = 0"),
        ]);
        let (session, utils) = open_app(files, ti(&[], &[("commander", "export let commander: number")]));

        let calls = calls(&utils);
        assert_eq!(2, calls.len());
        assert!(calls[1].args.iter().any(|a| a == "@types/commander@latest"));

        let p = program(&session, APP_URI);
        // Types file present
        assert!(has_file(&p, &typings_file("commander")));
        // JS resolution should be dropped
        assert!(!has_file(&p, "/user/username/projects/node_modules/commander/index.js"));
    }
}

/// The files of the cache entry subtests.
fn cache_files(dev_version: &str, lock: &str) -> FileMap {
    let typings = typings_file("jquery");
    let package_json = format!("{TEST_TYPINGS_LOCATION}/package.json");
    let package_lock = format!("{TEST_TYPINGS_LOCATION}/package-lock.json");
    let package_json_text = format!(
        r#"{{"dependencies":{{"types-registry":"^0.1.317"}},"devDependencies":{{"@types/jquery":"^{dev_version}"}}}}"#
    );
    files(&[
        (APP, ""),
        (
            "/user/username/projects/project/package.json",
            r#"{"name":"test","dependencies":{"jquery":"^3.1.0"}}"#,
        ),
        (typings.as_str(), "export const x = 10;"),
        (package_json.as_str(), package_json_text.as_str()),
        (package_lock.as_str(), lock),
    ])
}

fn jquery_typings_text(session: &Rc<Session>) -> String {
    let p = program(session, APP_URI);
    text(&p, &typings_file("jquery"))
}

child_test! {
    // Go: ata_test.go:432 TestATA/expired cache entry (inferred project, should install typings)
    fn expired_cache_entry_inferred_project_should_install_typings() {
        let files = cache_files("1.0.0", r#"{"dependencies":{"@types/jquery":{"version":"1.0.0"}}}"#);
        let (session, _) = open_app(files, ti(&[], &[("jquery", "export const y = 10")]));
        // Expect updated content from installed typings
        assert_eq!(jquery_typings_text(&session), "export const y = 10");
    }
}

child_test! {
    // Go: ata_test.go:460 TestATA/non-expired cache entry (inferred project, should not install typings)
    fn non_expired_cache_entry_inferred_project_should_not_install_typings() {
        let files = cache_files("1.3.0", r#"{"dependencies":{"@types/jquery":{"version":"1.3.0"}}}"#);
        let (session, _) = open_app(files, ti(&["jquery"], &[]));
        // Expect existing content unchanged
        assert_eq!(jquery_typings_text(&session), "export const x = 10;");
    }
}

// Go: ata_test.go:486 TestATA/deduplicate from local @types packages
// PORT: Go skips this subtest ("Todo - implement removing local @types from
// include list"); it is not ported.

child_test! {
    // Go: ata_test.go:519 TestATA/expired cache entry (inferred project, should install typings) lockfile3
    fn expired_cache_entry_inferred_project_should_install_typings_lockfile3() {
        let files = cache_files(
            "1.0.0",
            r#"{"packages":{"node_modules/@types/jquery":{"version":"1.0.0"}}}"#,
        );
        let (session, _) = open_app(files, ti(&[], &[("jquery", "export const y = 10")]));
        // Expect updated content from installed typings
        assert_eq!(jquery_typings_text(&session), "export const y = 10");
    }
}

child_test! {
    // Go: ata_test.go:547 TestATA/non-expired cache entry (inferred project, should not install typings) lockfile3
    fn non_expired_cache_entry_inferred_project_should_not_install_typings_lockfile3() {
        let files = cache_files(
            "1.3.0",
            r#"{"packages":{"node_modules/@types/jquery":{"version":"1.3.0"}}}"#,
        );
        let (session, _) = open_app(files, ti(&["jquery"], &[]));
        // Expect existing content unchanged
        assert_eq!(jquery_typings_text(&session), "export const x = 10;");
    }
}

child_test! {
    // Go: ata_test.go:573 TestATA/should install typings for unresolved imports
    fn should_install_typings_for_unresolved_imports() {
        let files = files(&[(
            APP,
            "
				import * as fs from \"fs\";
                import * as commander from \"commander\";
                import * as component from \"@ember/component\";
			",
        )]);
        let (session, utils) = open_app(
            files,
            ti(
                &[],
                &[
                    ("node", "export let node: number"),
                    ("commander", "export let commander: number"),
                    ("ember__component", "export let ember__component: number"),
                ],
            ),
        );

        // Check that npm install was called twice
        let calls = calls(&utils);
        assert_eq!(2, calls.len(), "Expected exactly 2 npm install calls");
        types_registry_call(&calls[0]);

        // The second call should install all three packages at once
        assert_eq!(calls[1].cwd, TEST_TYPINGS_LOCATION);
        assert_eq!(calls[1].args[0], "install");
        assert_eq!(calls[1].args[1], "--ignore-scripts");
        // Check that all three packages are in the install command
        let install_args = &calls[1].args;
        assert!(install_args.iter().any(|a| a == "@types/ember__component@latest"));
        assert!(install_args.iter().any(|a| a == "@types/commander@latest"));
        assert!(install_args.iter().any(|a| a == "@types/node@latest"));

        // Verify the types files were installed
        let p = program(&session, APP_URI);
        assert!(has_file(&p, &typings_file("node")), "node types should be installed");
        assert!(has_file(&p, &typings_file("commander")), "commander types should be installed");
        assert!(
            has_file(&p, &typings_file("ember__component")),
            "ember__component types should be installed"
        );
    }
}

child_test! {
    // Go: ata_test.go:626 TestATA/ATA with WatchEnabled false should not panic
    fn ata_with_watch_enabled_false_should_not_panic() {
        let files = files(&[(APP, ""), ("/user/username/projects/project/package.json", PACKAGE_JSON_JQUERY)]);

        let (session, utils) = projecttestutil::setup_with_options_and_typings_installer(
            files,
            Some(ts_goport::project::SessionOptions {
                watch_enabled: false,
                ..projecttestutil::session_options("/")
            }),
            ti(&[], &[("jquery", "declare const $: { x: number }")]),
        );

        // Open a file to trigger project creation and ATA.
        open_kind(&session, APP_URI, "", lsproto::LanguageKind::JAVA_SCRIPT);
        session.wait_for_background_tasks();

        // ATA should have run
        assert_eq!(2, calls(&utils).len(), "Expected exactly 2 npm install calls");

        // Getting the language service should not panic after
        // applying ATA changes and grabbing the latest snapshot.
        let _ = language_service(&session, APP_URI);
    }
}

/// Go `lsutil.ParseUserPreferences(config)`.
fn parse_prefs(config: Vec<(&str, LspAny)>) -> lsutil::UserPreferences {
    let items: indexmap::IndexMap<String, LspAny> = config
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
    lsutil::parse_user_preferences(&items)
}

/// Go `{"js/ts": {"tsserver": {"automaticTypeAcquisition": {"enabled": false}}}}`.
fn unified_ata_disabled() -> Vec<(&'static str, LspAny)> {
    vec![(
        "js/ts",
        lsp_object(vec![(
            "tsserver",
            lsp_object(vec![(
                "automaticTypeAcquisition",
                lsp_object(vec![("enabled", LspAny::Bool(false))]),
            )]),
        )]),
    )]
}

/// Go `{"typescript": {"disableAutomaticTypeAcquisition": true}}`.
fn deprecated_ata_disabled() -> Vec<(&'static str, LspAny)> {
    vec![(
        "typescript",
        lsp_object(vec![(
            "disableAutomaticTypeAcquisition",
            LspAny::Bool(true),
        )]),
    )]
}

// Go: ata_test.go:694 TestATA/ATA disabled via <name>
fn ata_disabled(config: Vec<(&str, LspAny)>, name: &str) {
    let files = files(&[
        (APP, ""),
        (
            "/user/username/projects/project/package.json",
            PACKAGE_JSON_JQUERY,
        ),
    ]);
    let (session, utils) = projecttestutil::setup_with_typings_installer(
        files,
        ti(&[], &[("jquery", "declare const $: { x: number }")]),
    );

    session.configure(parse_prefs(config));
    open_kind(&session, APP_URI, "", lsproto::LanguageKind::JAVA_SCRIPT);
    session.wait_for_background_tasks();

    assert_eq!(
        0,
        calls(&utils).len(),
        "Expected no npm install calls when ATA is disabled via {name}"
    );
}

child_test! {
    // Go: ata_test.go:694 TestATA/ATA disabled via unified setting
    fn ata_disabled_via_unified_setting() {
        ata_disabled(unified_ata_disabled(), "unified setting");
    }
}

child_test! {
    // Go: ata_test.go:694 TestATA/ATA disabled via deprecated setting
    fn ata_disabled_via_deprecated_setting() {
        ata_disabled(deprecated_ata_disabled(), "deprecated setting");
    }
}

child_test! {
    // Go: ata_test.go:722 TestATA/ATA re-enabled after being disabled triggers diagnostics refresh
    fn ata_re_enabled_after_being_disabled_triggers_diagnostics_refresh() {
        let files = files(&[(APP, ""), ("/user/username/projects/project/package.json", PACKAGE_JSON_JQUERY)]);
        let (session, utils) = projecttestutil::setup_with_typings_installer(
            files,
            ti(&[], &[("jquery", "declare const $: { x: number }")]),
        );

        // Disable ATA
        session.configure(parse_prefs(unified_ata_disabled()));

        open_kind(&session, APP_URI, "", lsproto::LanguageKind::JAVA_SCRIPT);
        session.wait_for_background_tasks();

        assert_eq!(0, calls(&utils).len(), "Expected no npm install calls when ATA is disabled");

        let baseline_refresh_count = utils.client().refresh_diagnostics_calls();

        // Re-enable ATA
        session.configure(parse_prefs(Vec::new()));
        session.wait_for_background_tasks();

        let refresh_count = utils.client().refresh_diagnostics_calls();
        assert!(
            refresh_count > baseline_refresh_count,
            "Expected RefreshDiagnostics call after ATA re-enabled"
        );
    }
}
