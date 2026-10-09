//! Port of Go `internal/ls/autoimport/registry_test.go` (`TestRegistryLifecycle`,
//! `TestContentMappedNodeModulesFileUsesProjectBucket`, `TestHiddenDirectoriesInNodeModules`,
//! `TestAutoImportEntrypointDirectorySearch`, `TestUpdateIndexesConcurrentMapSafety`).

use std::rc::Rc;

use ts_goport::frontend::tspath;
use ts_goport::ls::autoimport::registry::{BucketStats, CacheStats, ProjectID, Registry};
use ts_goport::ls::lsconv;
use ts_goport::ls::lsutil;
use ts_goport::lsp::lsproto;
use ts_goport::options::Tristate;
use ts_goport::project::{self, Session, SessionOptions};

use super::autoimporttestutil::{
    self, MonorepoPackageConfig, MonorepoPackageTemplate, MonorepoSetupConfig, TextFileSpec,
};
use super::projecttestutil::{self, FileMap, TypingsInstallerOptions, files};
use super::util::*;
use crate::support::{contentmappertest, vfstest};

// Go: registry_test.go:1234 lifecycleProjectRoot, monorepoProjectRoot
const LIFECYCLE_PROJECT_ROOT: &str = "/home/src/autoimport-lifecycle";
const MONOREPO_PROJECT_ROOT: &str = "/home/src/autoimport-monorepo";

// Go: registry_test.go:1274 autoImportStats
fn auto_import_stats(session: &Rc<Session>) -> CacheStats {
    let snapshot = session.snapshot();
    let registry = snapshot
        .auto_import_registry()
        .expect("auto import registry not initialized");
    registry.get_cache_stats()
}

// Go: registry_test.go:1284 singleBucket
fn single_bucket(buckets: &[BucketStats]) -> BucketStats {
    assert_eq!(buckets.len(), 1, "expected 1 bucket, got {}", buckets.len());
    buckets[0].clone()
}

fn with_auto_imports(session: &Rc<Session>, u: &lsproto::DocumentUri) {
    session
        .get_current_language_service_with_auto_imports(&bg(), u)
        .unwrap_or_else(|err| panic!("GetCurrentLanguageServiceWithAutoImports: {}", err.error()));
}

fn ls(session: &Rc<Session>, u: &lsproto::DocumentUri) {
    session
        .get_language_service(&bg(), u)
        .unwrap_or_else(|err| panic!("GetLanguageService: {}", err.error()));
}

fn open_uri(
    session: &Rc<Session>,
    u: &lsproto::DocumentUri,
    content: &str,
    kind: lsproto::LanguageKind,
) {
    session.did_open_file(&bg(), u, 1, content, &kind);
}

fn whole_change(session: &Rc<Session>, u: &lsproto::DocumentUri, version: i32, text: &str) {
    session.did_change_file(
        &bg(),
        u,
        version,
        &[lsproto::TextDocumentContentChangePartialOrWholeDocument {
            partial: None,
            whole_document: Some(lsproto::TextDocumentContentChangeWholeDocument {
                text: text.to_string(),
            }),
        }],
    );
}

fn has_dependency(bucket: &BucketStats, name: &str) -> bool {
    bucket
        .dependency_names
        .as_ref()
        .is_some_and(|d| d.contains(name))
}

fn dependency_set(bucket: &BucketStats) -> std::collections::BTreeSet<String> {
    bucket
        .dependency_names
        .as_ref()
        .map(|d| d.iter().cloned().collect())
        .unwrap_or_default()
}

fn set_of(items: &[&str]) -> std::collections::BTreeSet<String> {
    items.iter().map(|s| s.to_string()).collect()
}

/// The preferences of the "prepared for importing" checks.
fn import_preferences() -> lsutil::UserPreferences {
    let mut preferences = lsutil::new_default_user_preferences();
    preferences.include_completions_for_module_exports = Tristate::True;
    preferences.include_completions_for_import_statements = Tristate::True;
    preferences
}

/// Go `snapshot.GetDefaultProject(uri).ID()` as an `autoimport.ProjectID`.
fn default_project_id(session: &Rc<Session>, u: &lsproto::DocumentUri) -> ProjectID {
    let project = session
        .snapshot()
        .get_default_project(u)
        .expect("default project");
    let id = project.borrow().id().to_string();
    ProjectID(id)
}

/// Go `snapshot.AutoImportRegistry().IsPreparedForImportingFile(fileName, projectID, preferences)`.
fn is_prepared(
    session: &Rc<Session>,
    file_name: &str,
    project_id: &ProjectID,
    preferences: &lsutil::UserPreferences,
) -> bool {
    let snapshot = session.snapshot();
    let registry = snapshot.auto_import_registry();
    Registry::is_prepared_for_importing_file(
        registry.as_deref(),
        file_name,
        project_id,
        preferences,
    )
}

fn template(
    name: &str,
    node_module_names: &[&str],
    dependency_names: &[&str],
) -> MonorepoPackageTemplate {
    MonorepoPackageTemplate {
        name: name.to_string(),
        node_module_names: node_module_names.iter().map(|s| s.to_string()).collect(),
        dependency_names: dependency_names.iter().map(|s| s.to_string()).collect(),
    }
}

child_test! {
    // Go: registry_test.go:27 TestRegistryLifecycle/builds project and node_modules buckets
    fn builds_project_and_node_modules_buckets() {
        let fixture = autoimporttestutil::setup_lifecycle_session(LIFECYCLE_PROJECT_ROOT, 1);
        let session = fixture.session();
        let main_file = fixture.single_project().file(0);
        open_uri(session, &main_file.uri(), main_file.content(), lsproto::LanguageKind::TYPE_SCRIPT);

        let stats = auto_import_stats(session);
        let project_bucket = single_bucket(&stats.project_buckets);
        let node_modules_bucket = single_bucket(&stats.node_modules_buckets);
        assert!(project_bucket.state.dirty());
        assert_eq!(0, project_bucket.file_count);
        assert!(node_modules_bucket.state.dirty());
        assert_eq!(0, node_modules_bucket.file_count);

        with_auto_imports(session, &main_file.uri());

        let stats = auto_import_stats(session);
        let project_bucket = single_bucket(&stats.project_buckets);
        let node_modules_bucket = single_bucket(&stats.node_modules_buckets);
        assert!(!project_bucket.state.dirty());
        assert!(project_bucket.export_count > 0);
        assert!(!node_modules_bucket.state.dirty());
        assert!(node_modules_bucket.export_count > 0);
    }
}

child_test! {
    // Go: registry_test.go:55 TestRegistryLifecycle/bucket does not rebuild on same-file change
    fn bucket_does_not_rebuild_on_same_file_change() {
        let fixture = autoimporttestutil::setup_lifecycle_session(LIFECYCLE_PROJECT_ROOT, 2);
        let session = fixture.session();
        let utils = fixture.utils();
        let project = fixture.single_project();
        let main_file = project.file(0);
        let secondary_file = project.file(1);
        open_uri(session, &main_file.uri(), main_file.content(), lsproto::LanguageKind::TYPE_SCRIPT);
        open_uri(session, &secondary_file.uri(), secondary_file.content(), lsproto::LanguageKind::TYPE_SCRIPT);
        with_auto_imports(session, &main_file.uri());

        let updated_content = format!("{}// change\n", main_file.content());
        whole_change(session, &main_file.uri(), 2, &updated_content);

        ls(session, &main_file.uri());

        let stats = auto_import_stats(session);
        let project_bucket = single_bucket(&stats.project_buckets);
        let node_modules_bucket = single_bucket(&stats.node_modules_buckets);
        assert!(project_bucket.state.dirty());
        assert_eq!(project_bucket.state.dirty_file_exported(), utils.to_path(main_file.file_name()));
        assert!(!node_modules_bucket.state.dirty());
        assert_eq!(node_modules_bucket.state.dirty_file_exported(), tspath::Path(String::new()));

        // Bucket should not recompute when requesting same file changed
        with_auto_imports(session, &main_file.uri());
        let stats = auto_import_stats(session);
        let project_bucket = single_bucket(&stats.project_buckets);
        assert!(project_bucket.state.dirty());
        assert_eq!(project_bucket.state.dirty_file_exported(), utils.to_path(main_file.file_name()));

        // Bucket should recompute when other file has changed
        whole_change(session, &secondary_file.uri(), 1, "// new content");
        with_auto_imports(session, &main_file.uri());
        let stats = auto_import_stats(session);
        let project_bucket = single_bucket(&stats.project_buckets);
        assert!(!project_bucket.state.dirty());
    }
}

child_test! {
    // Go: registry_test.go:103 TestRegistryLifecycle/bucket updates on same-file change when new files added to the program
    fn bucket_updates_on_same_file_change_when_new_files_added_to_the_program() {
        let project_root = "/home/src/explicit-files-project";
        let tsconfig = format!("{project_root}/tsconfig.json");
        let index = format!("{project_root}/index.ts");
        let utils_ts = format!("{project_root}/utils.ts");
        let files = files(&[
            (
                tsconfig.as_str(),
                r#"{
				"compilerOptions": {
					"module": "esnext",
					"target": "esnext",
					"strict": true
				},
				"files": ["index.ts"]
			}"#,
            ),
            (index.as_str(), ""),
            (utils_ts.as_str(), "export const foo = 1;\nexport const bar = 2;"),
        ]);
        let (session, _) = projecttestutil::setup(files);

        let index_uri = uri(&format!("file://{project_root}/index.ts"));

        // Open the index.ts file
        open_uri(&session, &index_uri, "", lsproto::LanguageKind::TYPE_SCRIPT);
        with_auto_imports(&session, &index_uri);
        let stats = auto_import_stats(&session);
        let project_bucket = single_bucket(&stats.project_buckets);
        assert_eq!(1, project_bucket.file_count);

        // Edit index.ts to import foo from utils.ts
        whole_change(&session, &index_uri, 2, r#"import { foo } from "./utils";"#);

        // Bucket should be rebuilt because new files were added
        with_auto_imports(&session, &index_uri);
        let stats = auto_import_stats(&session);
        let project_bucket = single_bucket(&stats.project_buckets);
        assert_eq!(2, project_bucket.file_count);
    }
}

child_test! {
    // Go: registry_test.go:147 TestRegistryLifecycle/package.json dependency changes invalidate node_modules buckets
    fn package_json_dependency_changes_invalidate_node_modules_buckets() {
        let fixture = autoimporttestutil::setup_lifecycle_session(LIFECYCLE_PROJECT_ROOT, 1);
        let session = fixture.session();
        let session_utils = fixture.utils();
        let project = fixture.single_project();
        let main_file = project.file(0);
        let node_package = project.node_modules()[0].clone();
        let package_json = project.package_json_file().clone();

        open_uri(session, &main_file.uri(), main_file.content(), lsproto::LanguageKind::TYPE_SCRIPT);
        with_auto_imports(session, &main_file.uri());
        let stats = auto_import_stats(session);
        assert!(!single_bucket(&stats.node_modules_buckets).state.dirty());

        let update_package_json = |content: &str| {
            session_utils.fs().write_file(package_json.file_name(), content).unwrap();
            session.did_change_watched_files(
                &bg(),
                &[Some(lsproto::FileEvent {
                    type_: CHANGED,
                    uri: package_json.uri(),
                })],
            );
        };

        let same_deps_content = format!(
            "{{\n  \"name\": \"local-project-stable\",\n  \"dependencies\": {{\n    \"{}\": \"*\"\n  }}\n}}\n",
            node_package.name
        );
        update_package_json(&same_deps_content);
        ls(session, &main_file.uri());
        let stats = auto_import_stats(session);
        assert!(!single_bucket(&stats.node_modules_buckets).state.dirty());

        let different_deps_content = format!(
            "{{\n  \"name\": \"local-project-stable\",\n  \"dependencies\": {{\n    \"{}\": \"*\",\n    \"newpkg\": \"*\"\n  }}\n}}\n",
            node_package.name
        );
        update_package_json(&different_deps_content);
        with_auto_imports(session, &main_file.uri());
        let stats = auto_import_stats(session);
        assert!(has_dependency(&single_bucket(&stats.node_modules_buckets), "newpkg"));
    }
}

child_test! {
    // Go: registry_test.go:189 TestRegistryLifecycle/node_modules buckets get deleted when no open files can reference them
    fn node_modules_buckets_get_deleted_when_no_open_files_can_reference_them() {
        let fixture = autoimporttestutil::setup_monorepo_lifecycle_session(MonorepoSetupConfig {
            root: MONOREPO_PROJECT_ROOT.to_string(),
            template: template("monorepo", &["pkg-root"], &[]),
            packages: vec![
                MonorepoPackageConfig {
                    file_count: 1,
                    template: template("package-a", &["pkg-a"], &[]),
                },
                MonorepoPackageConfig {
                    file_count: 1,
                    template: template("package-b", &["pkg-b"], &[]),
                },
            ],
            ..Default::default()
        });
        let session = fixture.session();
        let monorepo = fixture.monorepo();
        let file_a = monorepo.package(0).file(0);
        let file_b = monorepo.package(1).file(0);

        // Open file in package-a, should create buckets for root and package-a node_modules
        open_uri(session, &file_a.uri(), file_a.content(), lsproto::LanguageKind::TYPE_SCRIPT);
        with_auto_imports(session, &file_a.uri());

        // Open file in package-b, should also create buckets for package-b
        open_uri(session, &file_b.uri(), file_b.content(), lsproto::LanguageKind::TYPE_SCRIPT);
        with_auto_imports(session, &file_b.uri());
        let stats = auto_import_stats(session);
        assert_eq!(stats.node_modules_buckets.len(), 3);
        assert_eq!(stats.project_buckets.len(), 2);

        // Close file in package-a, package-a's node_modules bucket and project bucket should be removed
        session.did_close_file(&bg(), &file_a.uri());
        with_auto_imports(session, &file_b.uri());
        let stats = auto_import_stats(session);
        assert_eq!(stats.node_modules_buckets.len(), 2);
        assert_eq!(stats.project_buckets.len(), 1);
    }
}

child_test! {
    // Go: registry_test.go:232 TestRegistryLifecycle/deleting node_modules leaves the registry prepared for importing
    fn deleting_node_modules_leaves_the_registry_prepared_for_importing() {
        let fixture = autoimporttestutil::setup_lifecycle_session(LIFECYCLE_PROJECT_ROOT, 1);
        let session = fixture.session();
        let session_utils = fixture.utils();
        let project = fixture.single_project();
        let main_file = project.file(0);

        let preferences = import_preferences();

        // Build auto-imports once so both buckets are clean and prepared.
        open_uri(session, &main_file.uri(), main_file.content(), lsproto::LanguageKind::TYPE_SCRIPT);
        with_auto_imports(session, &main_file.uri());

        let project_id = default_project_id(session, &main_file.uri());
        assert!(is_prepared(session, main_file.file_name(), &project_id, &preferences));
        assert_eq!(auto_import_stats(session).node_modules_buckets.len(), 1);

        // Simulate the user deleting node_modules.
        let node_modules_dir = tspath::combine_paths(project.root(), &["node_modules"]);
        session_utils.fs().remove(&node_modules_dir).unwrap();
        session.did_change_watched_files(
            &bg(),
            &[Some(lsproto::FileEvent {
                type_: DELETED,
                uri: lsconv::file_name_to_document_uri(&node_modules_dir),
            })],
        );

        // Re-preparing auto-imports must succeed and leave the registry prepared.
        with_auto_imports(session, &main_file.uri());

        assert!(
            is_prepared(session, main_file.file_name(), &project_id, &preferences),
            "registry should be prepared after node_modules is deleted"
        );
        // The node_modules bucket should be removed entirely, not left behind as an
        // empty bucket.
        assert_eq!(auto_import_stats(session).node_modules_buckets.len(), 0);
    }
}

child_test! {
    // Go: registry_test.go:281 TestRegistryLifecycle/deleting node_modules alongside a package.json change removes the bucket
    fn deleting_node_modules_alongside_a_package_json_change_removes_the_bucket() {
        let fixture = autoimporttestutil::setup_lifecycle_session(LIFECYCLE_PROJECT_ROOT, 1);
        let session = fixture.session();
        let session_utils = fixture.utils();
        let project = fixture.single_project();
        let main_file = project.file(0);
        let package_json = project.package_json_file();

        let preferences = import_preferences();

        open_uri(session, &main_file.uri(), main_file.content(), lsproto::LanguageKind::TYPE_SCRIPT);
        with_auto_imports(session, &main_file.uri());

        let project_id = default_project_id(session, &main_file.uri());
        assert_eq!(auto_import_stats(session).node_modules_buckets.len(), 1);

        // In a single changeset, edit package.json AND delete node_modules.
        session_utils
            .fs()
            .write_file(package_json.file_name(), r#"{"name": "app", "dependencies": {}}"#)
            .unwrap();
        let node_modules_dir = tspath::combine_paths(project.root(), &["node_modules"]);
        session_utils.fs().remove(&node_modules_dir).unwrap();
        session.did_change_watched_files(
            &bg(),
            &[
                Some(lsproto::FileEvent {
                    type_: CHANGED,
                    uri: package_json.uri(),
                }),
                Some(lsproto::FileEvent {
                    type_: DELETED,
                    uri: lsconv::file_name_to_document_uri(&node_modules_dir),
                }),
            ],
        );

        with_auto_imports(session, &main_file.uri());

        assert!(is_prepared(session, main_file.file_name(), &project_id, &preferences));
        assert_eq!(auto_import_stats(session).node_modules_buckets.len(), 0);
    }
}

child_test! {
    // Go: registry_test.go:324 TestRegistryLifecycle/deleting a package directory inside node_modules invalidates the bucket
    fn deleting_a_package_directory_inside_node_modules_invalidates_the_bucket() {
        let fixture = autoimporttestutil::setup_lifecycle_session(LIFECYCLE_PROJECT_ROOT, 1);
        let session = fixture.session();
        let session_utils = fixture.utils();
        let project = fixture.single_project();
        let main_file = project.file(0);
        let node_package = project.node_modules()[0].clone();

        open_uri(session, &main_file.uri(), main_file.content(), lsproto::LanguageKind::TYPE_SCRIPT);
        with_auto_imports(session, &main_file.uri());
        assert!(single_bucket(&auto_import_stats(session).node_modules_buckets).export_count > 0);

        // Delete just the package directory, leaving node_modules itself in place.
        session_utils.fs().remove(&node_package.directory).unwrap();
        session.did_change_watched_files(
            &bg(),
            &[Some(lsproto::FileEvent {
                type_: DELETED,
                uri: lsconv::file_name_to_document_uri(&node_package.directory),
            })],
        );

        with_auto_imports(session, &main_file.uri());
        assert_eq!(single_bucket(&auto_import_stats(session).node_modules_buckets).export_count, 0);
    }
}

child_test! {
    // Go: registry_test.go:353 TestRegistryLifecycle/node_modules bucket dependency selection changes with open files
    fn node_modules_bucket_dependency_selection_changes_with_open_files() {
        let monorepo_root = "/home/src/monorepo";
        let package_a_dir = tspath::combine_paths(monorepo_root, &["packages", "a"]);
        let monorepo_index = tspath::combine_paths(monorepo_root, &["index.js"]);
        let package_a_index = tspath::combine_paths(&package_a_dir, &["index.js"]);

        let fixture = autoimporttestutil::setup_monorepo_lifecycle_session(MonorepoSetupConfig {
            root: monorepo_root.to_string(),
            template: template("monorepo", &["pkg1", "pkg2", "pkg3"], &["pkg1"]),
            packages: vec![MonorepoPackageConfig {
                file_count: 0,
                template: template("a", &[], &["pkg1", "pkg2"]),
            }],
            extra_files: vec![
                TextFileSpec {
                    path: monorepo_index.clone(),
                    content: "export const monorepoIndex = 1;\n".to_string(),
                },
                TextFileSpec {
                    path: package_a_index.clone(),
                    content: "export const pkgA = 2;\n".to_string(),
                },
            ],
            ..Default::default()
        });
        let session = fixture.session();
        let monorepo_handle = fixture.extra_file(&monorepo_index).clone();
        let package_a_handle = fixture.extra_file(&package_a_index).clone();

        // Open monorepo root file: expect dependencies restricted to pkg1
        open_uri(session, &monorepo_handle.uri(), monorepo_handle.content(), lsproto::LanguageKind::JAVA_SCRIPT);
        with_auto_imports(session, &monorepo_handle.uri());
        let stats = auto_import_stats(session);
        assert_eq!(dependency_set(&single_bucket(&stats.node_modules_buckets)), set_of(&["pkg1"]));

        // Open package-a file: pkg2 should be added to existing bucket
        open_uri(session, &package_a_handle.uri(), package_a_handle.content(), lsproto::LanguageKind::JAVA_SCRIPT);
        with_auto_imports(session, &package_a_handle.uri());
        let stats = auto_import_stats(session);
        assert_eq!(
            dependency_set(&single_bucket(&stats.node_modules_buckets)),
            set_of(&["pkg1", "pkg2"])
        );

        // Close package-a file; only monorepo bucket should remain
        session.did_close_file(&bg(), &package_a_handle.uri());
        with_auto_imports(session, &monorepo_handle.uri());
        let stats = auto_import_stats(session);
        assert_eq!(dependency_set(&single_bucket(&stats.node_modules_buckets)), set_of(&["pkg1"]));

        // Close monorepo file; no node_modules buckets should remain
        session.did_close_file(&bg(), &monorepo_handle.uri());
        session.did_open_file(&bg(), &uri("untitled:Untitled-1"), 0, "", &lsproto::LanguageKind::TYPE_SCRIPT);
        ls(session, &uri("untitled:Untitled-1"));
        let stats = auto_import_stats(session);
        assert_eq!(stats.node_modules_buckets.len(), 0);
    }
}

child_test! {
    // Go: registry_test.go:417 TestRegistryLifecycle/node_modules bucket includes resolved packages from all projects
    fn node_modules_bucket_includes_resolved_packages_from_all_projects() {
        let monorepo_root = "/home/src/cross-project-deps";
        let package_a_dir = tspath::combine_paths(monorepo_root, &["packages", "a"]);
        let package_b_dir = tspath::combine_paths(monorepo_root, &["packages", "b"]);
        let package_a_index = tspath::combine_paths(&package_a_dir, &["index.ts"]);
        let package_b_index = tspath::combine_paths(&package_b_dir, &["index.ts"]);

        let fixture = autoimporttestutil::setup_monorepo_lifecycle_session(MonorepoSetupConfig {
            root: monorepo_root.to_string(),
            // Both pkg-listed and pkg-unlisted exist in node_modules,
            // but only pkg-listed is in the root package.json dependencies
            template: template("monorepo", &["pkg-listed", "pkg-unlisted"], &["pkg-listed"]),
            packages: vec![
                MonorepoPackageConfig {
                    file_count: 0,
                    template: template("a", &[], &["pkg-listed"]),
                },
                MonorepoPackageConfig {
                    file_count: 0,
                    template: template("b", &[], &["pkg-listed"]),
                },
            ],
            extra_files: vec![
                // project-a directly imports pkg-unlisted (not in package.json)
                TextFileSpec {
                    path: package_a_index.clone(),
                    content: "import { pkg_unlisted_value } from \"pkg-unlisted\";\nexport const a = pkg_unlisted_value;\n".to_string(),
                },
                // project-b does not import pkg-unlisted
                TextFileSpec {
                    path: package_b_index.clone(),
                    content: "export const b = 1;\n".to_string(),
                },
            ],
            ..Default::default()
        });
        let session = fixture.session();
        let package_a_handle = fixture.extra_file(&package_a_index).clone();
        let package_b_handle = fixture.extra_file(&package_b_index).clone();

        // Open file in project-a (which imports pkg-unlisted)
        open_uri(session, &package_a_handle.uri(), package_a_handle.content(), lsproto::LanguageKind::TYPE_SCRIPT);
        with_auto_imports(session, &package_a_handle.uri());

        // Open file in project-b (which does not import pkg-unlisted)
        open_uri(session, &package_b_handle.uri(), package_b_handle.content(), lsproto::LanguageKind::TYPE_SCRIPT);
        // Request auto-imports for project-b
        with_auto_imports(session, &package_b_handle.uri());

        let stats = auto_import_stats(session);
        let node_modules_bucket = single_bucket(&stats.node_modules_buckets);
        assert!(has_dependency(&node_modules_bucket, "pkg-listed"), "pkg-listed should be in dependencies");
        assert!(
            has_dependency(&node_modules_bucket, "pkg-unlisted"),
            "pkg-unlisted should be in dependencies because project-a imports it"
        );
    }
}

/// The files of the two symlinked monorepo subtests (Go literals).
fn symlinked_monorepo_files(
    project_a_dir: &str,
    project_b_dir: &str,
    other_pkg_dir: &str,
    project_a_index: &str,
    project_b_src_index: &str,
    project_b_dist_index: &str,
    other_pkg_index: &str,
) -> FileMap {
    let mut files = FileMap::new();
    let mut add = |path: String, text: &str| {
        files.insert(path, text.into());
    };
    // project-b: the library package
    add(
        tspath::combine_paths(project_b_dir, &["tsconfig.json"]),
        r#"{
				"compilerOptions": {
					"composite": true,
					"outDir": "./dist",
					"rootDir": "./src",
					"declaration": true,
					"module": "esnext",
					"strict": true
				},
				"include": ["src"]
			}"#,
    );
    add(
        tspath::combine_paths(project_b_dir, &["package.json"]),
        r#"{
				"name": "project-b",
				"version": "1.0.0",
				"main": "dist/index.js",
				"types": "dist/index.d.ts"
			}"#,
    );
    add(
        project_b_src_index.to_string(),
        "export function projectBFunction(): string { return \"hello\"; }\nexport const projectBValue: number = 42;",
    );
    add(
        project_b_dist_index.to_string(),
        "export declare function projectBFunction(): string;\nexport declare const projectBValue: number;",
    );
    // other-pkg
    add(
        tspath::combine_paths(other_pkg_dir, &["package.json"]),
        r#"{
				"name": "other-pkg",
				"version": "1.0.0",
				"main": "index.js",
				"types": "index.d.ts"
			}"#,
    );
    add(
        other_pkg_index.to_string(),
        "export declare function otherFunction(): void;\nexport declare const otherValue: string;",
    );
    // project-a: the consumer package
    add(
        tspath::combine_paths(project_a_dir, &["tsconfig.json"]),
        r#"{
				"compilerOptions": {
					"module": "esnext",
					"strict": true,
					"outDir": "./dist",
					"rootDir": "./src"
				},
				"include": ["src"],
				"references": [{ "path": "../project-b" }]
			}"#,
    );
    add(
        tspath::combine_paths(project_a_dir, &["package.json"]),
        r#"{
				"name": "project-a",
				"dependencies": { "project-b": "*", "other-pkg": "*" }
			}"#,
    );
    add(project_a_index.to_string(), "console.log(\"hello\");\n");
    // Symlink: project-b is accessible via node_modules
    files.insert(
        tspath::combine_paths(project_a_dir, &["node_modules", "project-b"]),
        vfstest::symlink(project_b_dir),
    );
    files
}

fn file_text(files: &FileMap, name: &str) -> String {
    String::from_utf8(files[name].data.clone()).unwrap()
}

child_test! {
    // Go: registry_test.go:496 TestRegistryLifecycle/symlinked monorepo invalidates on source file change
    fn symlinked_monorepo_invalidates_on_source_file_change() {
        let monorepo_root = "/home/src/symlinked-monorepo-invalidation";
        let project_a_dir = tspath::combine_paths(monorepo_root, &["packages", "project-a"]);
        let project_b_dir = tspath::combine_paths(monorepo_root, &["packages", "project-b"]);
        let project_a_index = tspath::combine_paths(&project_a_dir, &["src", "index.ts"]);
        let project_b_src_index = tspath::combine_paths(&project_b_dir, &["src", "index.ts"]);
        let project_b_dist_index = tspath::combine_paths(&project_b_dir, &["dist", "index.d.ts"]);
        let other_pkg_dir = tspath::combine_paths(&project_a_dir, &["node_modules", "other-pkg"]);
        let other_pkg_index = tspath::combine_paths(&other_pkg_dir, &["index.d.ts"]);

        let files = symlinked_monorepo_files(
            &project_a_dir,
            &project_b_dir,
            &other_pkg_dir,
            &project_a_index,
            &project_b_src_index,
            &project_b_dist_index,
            &other_pkg_index,
        );

        let (session, _) = projecttestutil::setup(files.clone());

        // Open project-a's index file and get initial auto-imports
        let project_a_uri = lsconv::file_name_to_document_uri(&project_a_index);
        open_uri(&session, &project_a_uri, &file_text(&files, &project_a_index), lsproto::LanguageKind::TYPE_SCRIPT);
        with_auto_imports(&session, &project_a_uri);

        // Verify initial state: bucket is clean with files
        let stats = auto_import_stats(&session);
        let node_modules_bucket = single_bucket(&stats.node_modules_buckets);
        let initial_file_count = node_modules_bucket.file_count;
        assert!(!node_modules_bucket.state.dirty(), "bucket should be clean initially");
        assert!(initial_file_count > 0, "bucket should have files initially");

        // Open project-b's source file
        let project_b_uri = lsconv::file_name_to_document_uri(&project_b_src_index);
        open_uri(&session, &project_b_uri, &file_text(&files, &project_b_src_index), lsproto::LanguageKind::TYPE_SCRIPT);

        // Modify the file (delete one export)
        whole_change(&session, &project_b_uri, 2, "export const projectBValue: number = 42;");

        // Check that the node_modules bucket is now dirty
        ls(&session, &project_a_uri);
        let stats = auto_import_stats(&session);
        let node_modules_bucket = single_bucket(&stats.node_modules_buckets);
        assert!(node_modules_bucket.state.dirty(), "bucket should be dirty after source file change");

        // Verify that only project-b is marked for update, not other-pkg.
        let dirty_packages = node_modules_bucket
            .state
            .dirty_packages_exported()
            .expect("dirty packages should be tracked");
        assert!(dirty_packages.contains("project-b"), "project-b should be in dirty packages");
        assert!(!dirty_packages.contains("other-pkg"), "other-pkg should NOT be in dirty packages");
        assert_eq!(dirty_packages.len(), 1, "only one package should be dirty");

        // Rebuild by requesting auto-imports again.
        with_auto_imports(&session, &project_a_uri);

        // Verify bucket is clean again after rebuild
        let stats = auto_import_stats(&session);
        let node_modules_bucket = single_bucket(&stats.node_modules_buckets);
        assert!(!node_modules_bucket.state.dirty(), "bucket should be clean after rebuild");
    }
}

child_test! {
    // Go: registry_test.go:627 TestRegistryLifecycle/pnpm-style symlinks only grant granular updates to workspace packages
    fn pnpm_style_symlinks_only_grant_granular_updates_to_workspace_packages() {
        let monorepo_root = "/home/src/pnpm-monorepo";
        let project_a_dir = tspath::combine_paths(monorepo_root, &["packages", "project-a"]);
        let project_b_dir = tspath::combine_paths(monorepo_root, &["packages", "project-b"]);
        let project_a_index = tspath::combine_paths(&project_a_dir, &["src", "index.ts"]);
        let project_b_src_index = tspath::combine_paths(&project_b_dir, &["src", "index.ts"]);
        let project_b_dist_index = tspath::combine_paths(&project_b_dir, &["dist", "index.d.ts"]);

        // Simulated pnpm virtual store for a registry package (inside project-a's node_modules).
        let pnpm_store_dir =
            tspath::combine_paths(&project_a_dir, &["node_modules", ".pnpm-store", "other-pkg@1.0.0"]);
        let other_pkg_index = tspath::combine_paths(&pnpm_store_dir, &["index.d.ts"]);

        let mut files = symlinked_monorepo_files(
            &project_a_dir,
            &project_b_dir,
            &pnpm_store_dir,
            &project_a_index,
            &project_b_src_index,
            &project_b_dist_index,
            &other_pkg_index,
        );
        // Symlink: pnpm-style registry package (realpath inside node_modules/.pnpm)
        files.insert(
            tspath::combine_paths(&project_a_dir, &["node_modules", "other-pkg"]),
            vfstest::symlink(&pnpm_store_dir),
        );

        let (session, _) = projecttestutil::setup_with_options(
            files.clone(),
            ts_goport::project::SessionOptions {
                typings_location: String::new(),
                push_diagnostics_enabled: true,
                ..projecttestutil::session_options(monorepo_root)
            },
        );

        // Open project-a's index file and build auto-imports
        let project_a_uri = lsconv::file_name_to_document_uri(&project_a_index);
        open_uri(&session, &project_a_uri, &file_text(&files, &project_a_index), lsproto::LanguageKind::TYPE_SCRIPT);
        with_auto_imports(&session, &project_a_uri);

        // Verify initial state: bucket is clean
        let stats = auto_import_stats(&session);
        assert!(
            !single_bucket(&stats.node_modules_buckets).state.dirty(),
            "bucket should be clean initially"
        );

        // Modify project-b's source file (local workspace package)
        let project_b_uri = lsconv::file_name_to_document_uri(&project_b_src_index);
        open_uri(&session, &project_b_uri, &file_text(&files, &project_b_src_index), lsproto::LanguageKind::TYPE_SCRIPT);
        whole_change(&session, &project_b_uri, 2, "export const projectBValue: number = 42;");

        // project-b should get a granular update (tracked in dirtyPackages)
        ls(&session, &project_a_uri);
        let stats = auto_import_stats(&session);
        let node_modules_bucket = single_bucket(&stats.node_modules_buckets);
        assert!(
            node_modules_bucket.state.dirty(),
            "bucket should be dirty after workspace package change"
        );
        let dirty_packages = node_modules_bucket
            .state
            .dirty_packages_exported()
            .expect("dirty packages should be tracked for workspace package");
        assert!(dirty_packages.contains("project-b"), "project-b should be in dirty packages");
        assert_eq!(dirty_packages.len(), 1, "only project-b should be dirty");

        // Rebuild to clear dirty state
        with_auto_imports(&session, &project_a_uri);
        let stats = auto_import_stats(&session);
        assert!(
            !single_bucket(&stats.node_modules_buckets).state.dirty(),
            "bucket should be clean after rebuild"
        );

        // Now modify other-pkg (pnpm registry package, realpath inside node_modules/.pnpm)
        let other_pkg_uri = lsconv::file_name_to_document_uri(&other_pkg_index);
        open_uri(&session, &other_pkg_uri, &file_text(&files, &other_pkg_index), lsproto::LanguageKind::TYPE_SCRIPT);
        whole_change(&session, &other_pkg_uri, 2, "export declare function otherFunction(): void;");

        // other-pkg should trigger a full rebuild (multipleFilesDirty), not a granular update.
        let stats_cell: std::cell::RefCell<Option<CacheStats>> = Default::default();
        session
            .with_language_service_and_snapshot(&bg(), &project_a_uri, |_ls, snapshot| {
                *stats_cell.borrow_mut() = Some(
                    snapshot
                        .auto_import_registry()
                        .expect("auto import registry")
                        .get_cache_stats(),
                );
                Ok(None)
            })
            .unwrap_or_else(|err| panic!("{}", err.error()));
        let stats = stats_cell.into_inner().expect("stats");
        let node_modules_bucket = single_bucket(&stats.node_modules_buckets);
        assert!(
            node_modules_bucket.state.dirty(),
            "bucket should be dirty after registry package change"
        );
        // A full rebuild means dirtyPackages is nil (multipleFilesDirty takes precedence)
        // or dirtyPackages doesn't contain "other-pkg" as a granular entry
        if let Some(dirty_packages) = node_modules_bucket.state.dirty_packages_exported() {
            assert!(
                !dirty_packages.contains("other-pkg"),
                "other-pkg should NOT be in dirty packages (should trigger full rebuild)"
            );
        }
    }
}

child_test! {
    // Go: registry_test.go:784 TestRegistryLifecycle/circular workspace symlinks do not exclude local project files
    fn circular_workspace_symlinks_do_not_exclude_local_project_files() {
        let monorepo_root = "/home/src/circular-workspaces";
        let package_a_dir = tspath::combine_paths(monorepo_root, &["packages", "pkg-a"]);
        let package_b_dir = tspath::combine_paths(monorepo_root, &["packages", "pkg-b"]);
        let consumer_a = tspath::combine_paths(&package_a_dir, &["consumer.ts"]);
        let helper_a = tspath::combine_paths(&package_a_dir, &["helper.ts"]);

        let mut files = FileMap::new();
        let mut add = |path: String, text: &str| {
            files.insert(path, text.into());
        };
        add(
            tspath::combine_paths(&package_a_dir, &["tsconfig.json"]),
            r#"{
				"compilerOptions": {
					"module": "esnext",
					"strict": true
				}
			}"#,
        );
        add(
            tspath::combine_paths(&package_a_dir, &["package.json"]),
            r#"{
				"name": "pkg-a",
				"dependencies": { "pkg-b": "*" }
			}"#,
        );
        add(
            tspath::combine_paths(&package_a_dir, &["index.ts"]),
            "import { b } from \"pkg-b\";\nexport const a = b;\n",
        );
        add(
            consumer_a.clone(),
            "export const usesHelper = uniqueHelperValueFromHelperA;\n",
        );
        add(helper_a, "export const uniqueHelperValueFromHelperA = 1;\n");
        add(
            tspath::combine_paths(&package_b_dir, &["tsconfig.json"]),
            r#"{
				"compilerOptions": {
					"module": "esnext",
					"strict": true
				}
			}"#,
        );
        add(
            tspath::combine_paths(&package_b_dir, &["package.json"]),
            r#"{
				"name": "pkg-b",
				"dependencies": { "pkg-a": "*" }
			}"#,
        );
        add(
            tspath::combine_paths(&package_b_dir, &["index.ts"]),
            "import { a } from \"pkg-a\";\nexport const b = a;\n",
        );
        // Circular workspace links
        files.insert(
            tspath::combine_paths(&package_a_dir, &["node_modules", "pkg-b"]),
            vfstest::symlink(&package_b_dir),
        );
        files.insert(
            tspath::combine_paths(&package_b_dir, &["node_modules", "pkg-a"]),
            vfstest::symlink(&package_a_dir),
        );

        let (session, _) = projecttestutil::setup(files.clone());
        let consumer_a_uri = lsconv::file_name_to_document_uri(&consumer_a);
        open_uri(&session, &consumer_a_uri, &file_text(&files, &consumer_a), lsproto::LanguageKind::TYPE_SCRIPT);

        with_auto_imports(&session, &consumer_a_uri);

        let stats = auto_import_stats(&session);
        let project_bucket = single_bucket(&stats.project_buckets);
        assert_eq!(
            3, project_bucket.file_count,
            "expected all pkg-a project files despite circular workspace symlinks"
        );
    }
}

child_test! {
    // Go: registry_test.go:843 TestRegistryLifecycle/changed fileExcludePatterns triggers bucket rebuild
    fn changed_file_exclude_patterns_triggers_bucket_rebuild() {
        let fixture = autoimporttestutil::setup_lifecycle_session(LIFECYCLE_PROJECT_ROOT, 1);
        let session = fixture.session();
        let main_file = fixture.single_project().file(0);

        // Open file and build auto-imports initially
        open_uri(session, &main_file.uri(), main_file.content(), lsproto::LanguageKind::TYPE_SCRIPT);
        with_auto_imports(session, &main_file.uri());

        // Verify buckets are clean after initial build
        let stats = auto_import_stats(session);
        assert!(!single_bucket(&stats.project_buckets).state.dirty());
        assert!(!single_bucket(&stats.node_modules_buckets).state.dirty());

        // IsPreparedForImportingFile should return true with no exclude patterns
        let project_id = default_project_id(session, &main_file.uri());
        let preferences = import_preferences();
        assert!(is_prepared(session, main_file.file_name(), &project_id, &preferences));

        // Change the file exclude patterns preference
        let mut new_preferences = import_preferences();
        new_preferences.auto_import_file_exclude_patterns = vec!["**/node_modules/**/*.d.ts".to_string()];
        session.configure(new_preferences.clone());

        // IsPreparedForImportingFile should return false since exclude patterns changed
        assert!(!is_prepared(session, main_file.file_name(), &project_id, &new_preferences));

        // After GetCurrentLanguageServiceWithAutoImports, buckets should be rebuilt
        with_auto_imports(session, &main_file.uri());

        // IsPreparedForImportingFile should return true now that buckets are rebuilt
        assert!(
            is_prepared(session, main_file.file_name(), &project_id, &new_preferences),
            "IsPreparedForImportingFile should return true after bucket rebuild with new fileExcludePatterns"
        );
    }
}

child_test! {
    // Go: registry_test.go:897 TestRegistryLifecycle/dedupes packages that resolve to same realpath across ancestor node_modules buckets
    fn dedupes_packages_that_resolve_to_same_realpath_across_ancestor_node_modules_buckets() {
        let repo_root = "/home/src/autoimport-realpath-dedupe";
        let app_dir = tspath::combine_paths(repo_root, &["apps", "web"]);
        let shared_pkg_dir = tspath::combine_paths(repo_root, &["node_modules", "shared"]);
        let app_index = tspath::combine_paths(&app_dir, &["src", "index.ts"]);

        let mut files = FileMap::new();
        let mut add = |path: String, text: &str| {
            files.insert(path, text.into());
        };
        add(
            tspath::combine_paths(repo_root, &["package.json"]),
            r#"{
				"name": "repo-root",
				"private": true,
				"dependencies": { "shared": "*" }
			}"#,
        );
        add(
            tspath::combine_paths(repo_root, &["tsconfig.json"]),
            r#"{
				"compilerOptions": {
					"module": "esnext",
					"target": "esnext",
					"strict": true
				},
				"include": ["apps/**/*"]
			}"#,
        );
        add(
            tspath::combine_paths(&app_dir, &["package.json"]),
            r#"{
				"name": "web",
				"private": true,
				"dependencies": { "shared": "*" }
			}"#,
        );
        add(
            tspath::combine_paths(&app_dir, &["tsconfig.json"]),
            r#"{
				"compilerOptions": {
					"module": "esnext",
					"target": "esnext",
					"strict": true
				},
				"include": ["src"]
			}"#,
        );
        add(app_index.clone(), "export const app = 1;\n");
        add(
            tspath::combine_paths(&shared_pkg_dir, &["package.json"]),
            r#"{
				"name": "shared",
				"version": "1.0.0",
				"types": "index.d.ts"
			}"#,
        );
        add(
            tspath::combine_paths(&shared_pkg_dir, &["index.d.ts"]),
            "export declare const sharedValue: 1;\n",
        );
        files.insert(
            tspath::combine_paths(&app_dir, &["node_modules", "shared"]),
            vfstest::symlink(&shared_pkg_dir),
        );

        let (session, _) = projecttestutil::setup(files);

        let app_uri = lsconv::file_name_to_document_uri(&app_index);
        open_uri(&session, &app_uri, "export const app = 1;\n", lsproto::LanguageKind::TYPE_SCRIPT);

        with_auto_imports(&session, &app_uri);

        let stats = auto_import_stats(&session);
        assert_eq!(
            stats.node_modules_buckets.len(),
            2,
            "expected both app and repo node_modules buckets"
        );
        assert_eq!(stats.unique_package_count, 1, "expected one unique package after realpath dedup");
    }
}

child_test! {
    // Go: registry_test.go:958 TestContentMappedNodeModulesFileUsesProjectBucket
    // PORT: Go `bundled.Embedded` is always true in the port, so the skip is dropped.
    fn content_mapped_node_modules_file_uses_project_bucket() {
        const MAIN_TEXT: &str = "profileTitle;";
        let mapper_package_json = contentmappertest::package_json(contentmappertest::COMPONENT_MAPPER);
        let files = files(&[
            (
                "/home/project/tsconfig.json",
                r#"{
			"compilerOptions": { "module": "esnext", "moduleResolution": "bundler", "strict": true, "skipLibCheck": true },
			"contentMappers": [ { "package": "mapper", "extensions": [".vue"] } ]
		}"#,
            ),
            ("/home/project/node_modules/mapper/package.json", mapper_package_json.as_str()),
            (
                "/home/project/node_modules/profile-package/ProfileCard.vue",
                r#"<component name="ProfileCard">
<script lang="ts">
export const profileTitle = "Profile";
</script>"#,
            ),
            (
                "/home/project/node_modules/profile-package/HiddenCard.vue",
                r#"<component name="HiddenCard">
<script lang="ts">
export const hiddenTitle = "Hidden";
</script>"#,
            ),
            ("/home/project/node_modules/profile-package/ordinary.ts", "export const ordinary = true;"),
            (
                "/home/project/load.ts",
                r#"import "profile-package/ProfileCard.vue";
import "profile-package/ordinary";"#,
            ),
            ("/home/project/main.ts", MAIN_TEXT),
        ]);
        // PORT: the Go literal names 5 fields; the others are Go zero values
        // (`watch_enabled` and `logging_enabled` false). Go nil `tiOptions`
        // is the default `TypingsInstallerOptions`.
        let (mut init, _) = projecttestutil::get_session_init_options(
            files,
            Some(SessionOptions {
                run_external_code: true,
                watch_enabled: false,
                logging_enabled: false,
                ..projecttestutil::session_options("/home/project")
            }),
            TypingsInstallerOptions::default(),
        );
        init.spawner = Some(contentmappertest::new_spawner());
        let session = project::new_session(&init);

        let main_uri = uri("file:///home/project/main.ts");
        open_uri(&session, &main_uri, MAIN_TEXT, lsproto::LanguageKind::TYPE_SCRIPT);
        with_auto_imports(&session, &main_uri);
        session.wait_for_background_tasks();

        let project_bucket = single_bucket(&auto_import_stats(&session).project_buckets);
        assert_eq!(
            project_bucket.file_count, 3,
            "expected the two project roots and referenced mapped package file"
        );
        // Go: defer session.Close()
        session.close();
    }
}

child_test! {
    // Go: registry_test.go:1007 TestHiddenDirectoriesInNodeModules/deep import through subdirectory package.json in hidden store
    fn deep_import_through_subdirectory_package_json_in_hidden_store() {
        let project_root = "/home/src/fuse-project";
        let store_dir = format!("{project_root}/node_modules/.yarn-store");
        let pkg_store_dir = format!("{store_dir}/some-pkg-npm-1.0.0-abc123/package");

        let mut files = FileMap::new();
        let mut add = |path: String, text: &str| {
            files.insert(path, text.into());
        };
        add(
            format!("{project_root}/tsconfig.json"),
            r#"{
				"compilerOptions": {
					"module": "commonjs",
					"target": "es2020",
					"strict": true
				}
			}"#,
        );
        add(
            format!("{project_root}/package.json"),
            r#"{
				"name": "test-project",
				"dependencies": {
					"some-pkg": "*",
					"real-package": "*"
				}
			}"#,
        );
        // Deep import: "some-pkg/debug" — resolves through the subdirectory package.json
        add(format!("{project_root}/index.ts"), r#"import { debug } from "some-pkg/debug";"#);
        // Real package that should be indexed normally
        add(
            format!("{project_root}/node_modules/real-package/package.json"),
            r#"{"name":"real-package","version":"1.0.0","types":"index.d.ts"}"#,
        );
        add(
            format!("{project_root}/node_modules/real-package/index.d.ts"),
            "export declare const realExport: number;\n",
        );
        add(
            format!("{pkg_store_dir}/package.json"),
            r#"{"name":"some-pkg","version":"1.0.0","types":"index.d.ts"}"#,
        );
        add(format!("{pkg_store_dir}/index.d.ts"), "export declare const something: number;\n");
        add(
            format!("{pkg_store_dir}/debug/package.json"),
            r#"{"main":"./debug.js","types":"./debug.d.ts"}"#,
        );
        add(
            format!("{pkg_store_dir}/debug/debug.d.ts"),
            "export declare function debug(msg: string): void;\n",
        );
        add(
            format!("{pkg_store_dir}/debug/debug.js"),
            "exports.debug = function(msg) { console.log(msg); };\n",
        );
        // Other content in the hidden store that should never be crawled
        add(
            format!("{store_dir}/other-pkg-npm-2.0.0-def456/package/package.json"),
            r#"{"name":"other-pkg","version":"1.0.0","types":"index.d.ts"}"#,
        );
        add(
            format!("{store_dir}/other-pkg-npm-2.0.0-def456/package/index.d.ts"),
            "export declare const other: string;\n",
        );
        // Symlink: node_modules/some-pkg -> .yarn-store/.../package/
        files.insert(
            format!("{project_root}/node_modules/some-pkg"),
            vfstest::symlink(&pkg_store_dir),
        );

        let (session, _) = projecttestutil::setup(files);

        let index_uri = uri(&format!("file://{project_root}/index.ts"));
        open_uri(
            &session,
            &index_uri,
            r#"import { debug } from "some-pkg/debug";"#,
            lsproto::LanguageKind::TYPE_SCRIPT,
        );

        with_auto_imports(&session, &index_uri);

        let stats = auto_import_stats(&session);
        let node_modules_bucket = single_bucket(&stats.node_modules_buckets);

        // .yarn-store must not appear as a dependency name.
        let dependency_names = node_modules_bucket
            .dependency_names
            .as_ref()
            .expect("DependencyNames should not be nil");
        for name in dependency_names.iter() {
            assert!(
                !name.starts_with('.'),
                "hidden directory {name:?} should not appear as a dependency name"
            );
        }
    }
}

// Go: registry_test.go:1090 TestAutoImportEntrypointDirectorySearch (files)
const ENTRYPOINT_ROOT: &str = "/home/src/entrypoint-search";

fn entrypoint_files() -> FileMap {
    let pkg_dir = format!("{ENTRYPOINT_ROOT}/node_modules/my-pkg");
    let entries = [
        (
            format!("{ENTRYPOINT_ROOT}/tsconfig.json"),
            r#"{
			"compilerOptions": {
				"module": "commonjs",
				"target": "es2020"
			}
		}"#,
        ),
        (
            format!("{ENTRYPOINT_ROOT}/package.json"),
            r#"{
			"name": "test-project",
			"dependencies": { "my-pkg": "*" }
		}"#,
        ),
        (
            format!("{ENTRYPOINT_ROOT}/index.ts"),
            r#"import { main } from "my-pkg";"#,
        ),
        (
            format!("{pkg_dir}/package.json"),
            r#"{"name":"my-pkg","version":"1.0.0","types":"index.d.ts"}"#,
        ),
        (
            format!("{pkg_dir}/index.d.ts"),
            "export declare const main: number;\n",
        ),
        (
            format!("{pkg_dir}/extra.d.ts"),
            "export declare const extra: string;\n",
        ),
        (
            format!("{pkg_dir}/nested/deep.d.ts"),
            "export declare const deep: boolean;\n",
        ),
        (
            format!("{pkg_dir}/nested/deeper.d.ts"),
            "export declare const deeper: boolean;\n",
        ),
    ];
    let entries: Vec<(&str, &str)> = entries.iter().map(|(p, t)| (p.as_str(), *t)).collect();
    files(&entries)
}

fn entrypoint_index_uri() -> lsproto::DocumentUri {
    uri(&format!("file://{ENTRYPOINT_ROOT}/index.ts"))
}

fn directory_search_prefs() -> lsutil::UserPreferences {
    let mut prefs = lsutil::new_default_user_preferences();
    prefs.auto_import_entrypoint_directory_search = Tristate::True;
    prefs
}

child_test! {
    // Go: registry_test.go:1119 TestAutoImportEntrypointDirectorySearch/default limits to main entrypoint
    fn entrypoint_default_limits_to_main_entrypoint() {
        let (session, _) = projecttestutil::setup(entrypoint_files());
        let index_uri = entrypoint_index_uri();
        open_uri(&session, &index_uri, r#"import { main } from "my-pkg";"#, lsproto::LanguageKind::TYPE_SCRIPT);

        with_auto_imports(&session, &index_uri);

        let stats = auto_import_stats(&session);
        // Without the preference, only the main entrypoint (index.d.ts) should be found
        assert_eq!(
            1,
            single_bucket(&stats.node_modules_buckets).file_count,
            "expected only 1 file (main entrypoint) by default"
        );
    }
}

child_test! {
    // Go: registry_test.go:1137 TestAutoImportEntrypointDirectorySearch/autoImportEntrypointDirectorySearch enables all files
    fn entrypoint_auto_import_entrypoint_directory_search_enables_all_files() {
        let (session, _) = projecttestutil::setup(entrypoint_files());
        session.configure(directory_search_prefs());

        let index_uri = entrypoint_index_uri();
        open_uri(&session, &index_uri, r#"import { main } from "my-pkg";"#, lsproto::LanguageKind::TYPE_SCRIPT);

        with_auto_imports(&session, &index_uri);

        let stats = auto_import_stats(&session);
        let file_count = single_bucket(&stats.node_modules_buckets).file_count;
        // With the preference, all 4 .d.ts files should be found via directory search
        assert!(file_count >= 4, "expected at least 4 files from directory search, got {file_count}");
    }
}

child_test! {
    // Go: registry_test.go:1159 TestAutoImportEntrypointDirectorySearch/changing preference triggers rebuild
    fn entrypoint_changing_preference_triggers_rebuild() {
        let (session, _) = projecttestutil::setup(entrypoint_files());
        let index_uri = entrypoint_index_uri();
        open_uri(&session, &index_uri, r#"import { main } from "my-pkg";"#, lsproto::LanguageKind::TYPE_SCRIPT);

        // Build auto-imports with default preferences (directory search disabled)
        with_auto_imports(&session, &index_uri);

        let stats = auto_import_stats(&session);
        assert_eq!(
            1,
            single_bucket(&stats.node_modules_buckets).file_count,
            "expected only 1 file initially"
        );

        // Now enable directory search
        let prefs = directory_search_prefs();
        session.configure(prefs.clone());

        // Registry should report not prepared (preference changed)
        let project_id = default_project_id(&session, &index_uri);
        assert!(
            !is_prepared(&session, &format!("{ENTRYPOINT_ROOT}/index.ts"), &project_id, &prefs),
            "registry should not be prepared after preference change"
        );

        // Rebuild
        with_auto_imports(&session, &index_uri);

        let stats = auto_import_stats(&session);
        let file_count = single_bucket(&stats.node_modules_buckets).file_count;
        assert!(
            file_count >= 4,
            "expected at least 4 files after rebuild with directory search enabled, got {file_count}"
        );
    }
}

child_test! {
    // Go: registry_test.go:1200 TestAutoImportEntrypointDirectorySearch/deep import from program update enables recursive search for that package
    fn entrypoint_deep_import_from_program_update_enables_recursive_search_for_that_package() {
        let (session, _) = projecttestutil::setup(entrypoint_files());
        let index_uri = entrypoint_index_uri();
        open_uri(&session, &index_uri, r#"import { main } from "my-pkg";"#, lsproto::LanguageKind::TYPE_SCRIPT);

        // Initial build with top-level import only ("my-pkg", not a deep import)
        with_auto_imports(&session, &index_uri);

        let stats = auto_import_stats(&session);
        assert_eq!(
            1,
            single_bucket(&stats.node_modules_buckets).file_count,
            "expected only 1 file (main entrypoint) before deep import"
        );

        // Now update the program to add a deep import from the same package
        whole_change(
            &session,
            &index_uri,
            2,
            "import { main } from \"my-pkg\";\nimport { deep } from \"my-pkg/nested/deep\";\n",
        );

        // After the program update, auto-imports should detect the deep import and
        // enable recursive directory search for my-pkg, finding all .d.ts files.
        with_auto_imports(&session, &index_uri);

        let stats = auto_import_stats(&session);
        let file_count = single_bucket(&stats.node_modules_buckets).file_count;
        assert!(
            file_count >= 4,
            "expected at least 4 files after deep import triggers recursive search, got {file_count}"
        );
    }
}

child_test! {
    // Go: registry_test.go:1239 TestUpdateIndexesConcurrentMapSafety
    fn update_indexes_concurrent_map_safety() {
        const PROJECT_ROOT: &str = "/home/src/autoimport-fallback-race";
        const PACKAGE_COUNT: i32 = 40;

        let mut files = FileMap::new();
        files.insert(
            format!("{PROJECT_ROOT}/tsconfig.json"),
            r#"{
			"compilerOptions": { "module": "esnext", "target": "esnext", "strict": true }
		}"#.into(),
        );
        files.insert(format!("{PROJECT_ROOT}/index.ts"), "export {};\n".into());
        for i in 0..PACKAGE_COUNT {
            let pkg_dir = format!("{PROJECT_ROOT}/node_modules/pkg{i}");
            files.insert(
                format!("{pkg_dir}/package.json"),
                format!(r#"{{"name":"pkg{i}","version":"1.0.0","main":"index.js"}}"#).into(),
            );
            files.insert(format!("{pkg_dir}/index.js"), "module.exports = {};\n".into());
            let types_dir = format!("{PROJECT_ROOT}/node_modules/@types/pkg{i}");
            files.insert(
                format!("{types_dir}/package.json"),
                format!(r#"{{"name":"@types/pkg{i}","version":"1.0.0","types":"index.d.ts"}}"#).into(),
            );
            files.insert(
                format!("{types_dir}/index.d.ts"),
                format!("export declare const foo{i}: number;\n").into(),
            );
        }

        let (session, _) = projecttestutil::setup(files);

        let index_uri = uri(&format!("file://{PROJECT_ROOT}/index.ts"));
        open_uri(&session, &index_uri, "export {};\n", lsproto::LanguageKind::TYPE_SCRIPT);

        with_auto_imports(&session, &index_uri);

        let stats = auto_import_stats(&session);
        let node_modules_bucket = single_bucket(&stats.node_modules_buckets);
        assert_eq!(node_modules_bucket.export_count, PACKAGE_COUNT);
    }
}

child_test! {
    // followups4 (R153 reviewer): the registry extracts a package first with
    // the narrow walk of lspreg1 (`AliasResolver::new_narrow_checker`), which
    // does not read the imports of a .ts source. This entrypoint exports a
    // binding that it imports, so the checker asks for other.ts, which the
    // narrow walk did not read. That is a miss: the narrow results are
    // dropped, and the full walk extracts the package again. The export `y`
    // then has its target in other.ts, as Go gives (Go reads the files that
    // the checker asks for).
    fn narrow_walk_miss_extracts_the_package_with_the_full_walk() {
        let root = "/home/src/narrow-miss";
        let pkg = format!("{root}/node_modules/src-pkg");
        let index_text = r#"import { y } from "src-pkg";"#;
        let mut files = FileMap::new();
        let mut add = |path: String, text: &str| {
            files.insert(path, text.into());
        };
        add(
            format!("{root}/tsconfig.json"),
            r#"{"compilerOptions":{"module":"esnext","moduleResolution":"bundler"}}"#,
        );
        add(
            format!("{root}/package.json"),
            r#"{"name":"narrow-miss","dependencies":{"src-pkg":"*"}}"#,
        );
        add(format!("{root}/index.ts"), index_text);
        add(
            format!("{pkg}/package.json"),
            r#"{"name":"src-pkg","version":"1.0.0","types":"index.ts"}"#,
        );
        add(format!("{pkg}/index.ts"), "import { y } from \"./other\";\nexport { y };\n");
        add(format!("{pkg}/other.ts"), "export const y = 1;\n");
        let (session, _) = projecttestutil::setup(files);
        let index_uri = uri(&format!("file://{root}/index.ts"));
        open_uri(&session, &index_uri, index_text, lsproto::LanguageKind::TYPE_SCRIPT);

        with_auto_imports(&session, &index_uri);

        let registry = session
            .snapshot()
            .auto_import_registry()
            .expect("auto import registry not initialized");
        assert_eq!(registry.node_modules.len(), 1);
        let bucket = registry.node_modules.values().next().unwrap();
        let index = bucket.index.as_ref().expect("an indexed bucket").borrow();
        let exports = index.find("y", true);
        let from_index: Vec<_> = exports
            .iter()
            .filter(|export| export.module_file_name == format!("{pkg}/index.ts"))
            .collect();
        assert_eq!(from_index.len(), 1, "{exports:?}");
        assert_eq!(
            from_index[0].target.module_id.as_string(),
            format!("{pkg}/other.ts")
        );
        assert_eq!(from_index[0].target.export_name, "y");
    }
}
