//! Ports of internal/execute/build/graph_test.go and
//! internal/execute/tsc/extendedconfigcache_test.go.
//!
//! PORT: `TestBuildScheduling` (ts#64615) builds programs, and the compiler
//! runs in this process with the OS override for the test system, which is
//! for the whole process. So each of its subtests is a test that runs in a
//! child process of its own (`run_test_in_child`), as in watcher_race.rs.

use super::Subtests;
use crate::support::child::{new_in_process_test_sys, run_test_in_child};
use crate::support::runner::{FileMap, TscInput};
use crate::support::test_sys::{TestSys, new_tsc_system};
use crate::support::vfstest;
use rustc_hash::FxHashMap;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use ts_goport::contentmapper::ProcessExitState;
use ts_goport::execute::build::command_line::parse_build_command_line;
use ts_goport::execute::build::host::TscExtendedConfigCache;
use ts_goport::execute::build::orchestrator::{Options, new_orchestrator};
use ts_goport::execute::tsc::compile::{
    ErrorWriter, ExitStatus, System, SystemParseConfigHost, Writer,
};
use ts_goport::frontend::tsoptions::{
    ParseConfigHost, ParsedCommandLine, get_parsed_command_line_of_config_file,
};
use ts_goport::frontend::vfs::Fs;
use ts_goport::frontend::vfs::wrapvfs::{Replacements, wrapvfs_wrap};
use ts_goport::fswatch::EventKind;
use ts_goport::gostd::{GoError, context};

// ---------------------------------------------------------------------------
// execute/build/graph_test.go
// ---------------------------------------------------------------------------

/// Go `buildOrderTestCase`.
struct BuildOrderTestCase {
    name: &'static str,
    projects: &'static [&'static str],
    expected: &'static [&'static str],
    // ts#64220
    expected_schedule: &'static [&'static str],
    circular: bool,
}

// Go: execute/build/graph_test.go:21 TestBuildOrderGenerator
#[test]
fn test_build_order_generator() {
    #[rustfmt::skip]
    let test_cases = [
        BuildOrderTestCase { name: "specify two roots", projects: &["A", "G"], expected: &["D", "E", "C", "B", "A", "G"], expected_schedule: &["D", "E", "G", "C", "B", "A"], circular: false },
        BuildOrderTestCase { name: "multiple parts of the same graph in various orders", projects: &["A"], expected: &["D", "E", "C", "B", "A"], expected_schedule: &["D", "E", "C", "B", "A"], circular: false },
        BuildOrderTestCase { name: "multiple parts of the same graph in various orders", projects: &["A", "C", "D"], expected: &["D", "E", "C", "B", "A"], expected_schedule: &["D", "E", "C", "B", "A"], circular: false },
        BuildOrderTestCase { name: "multiple parts of the same graph in various orders", projects: &["D", "C", "A"], expected: &["D", "E", "C", "B", "A"], expected_schedule: &["D", "E", "C", "B", "A"], circular: false },
        BuildOrderTestCase { name: "other orderings", projects: &["F"], expected: &["E", "F"], expected_schedule: &["E", "F"], circular: false },
        BuildOrderTestCase { name: "other orderings", projects: &["E"], expected: &["E"], expected_schedule: &["E"], circular: false },
        BuildOrderTestCase { name: "other orderings", projects: &["F", "C", "A"], expected: &["E", "F", "D", "C", "B", "A"], expected_schedule: &["E", "D", "F", "C", "B", "A"], circular: false },
        BuildOrderTestCase { name: "returns circular order", projects: &["H"], expected: &["E", "J", "I", "H"], expected_schedule: &["E", "J", "I", "H"], circular: true },
        BuildOrderTestCase { name: "returns circular order", projects: &["A", "H"], expected: &["D", "E", "C", "B", "A", "J", "I", "H"], expected_schedule: &["D", "E", "C", "J", "B", "I", "A", "H"], circular: true },
    ];
    let mut t = Subtests::new("TestBuildOrderGenerator");
    for testcase in &test_cases {
        testcase.run(&mut t);
    }
    t.finish();
}

impl BuildOrderTestCase {
    // Go: execute/build/graph_test.go:164 configName
    fn config_name(project: &str) -> String {
        format!("/home/src/workspaces/project/{project}/tsconfig.json")
    }

    // Go: execute/build/graph_test.go:168 projectName
    fn project_name(config: &str) -> String {
        let s = config
            .strip_prefix("/home/src/workspaces/project/")
            .unwrap_or(config);
        s.strip_suffix("/tsconfig.json").unwrap_or(s).to_string()
    }

    // Go: execute/build/graph_test.go:174 run
    fn run(&self, t: &mut Subtests) {
        t.run(
            &format!("{} - {}", self.name, self.projects.join(",")),
            || self.run_case(),
        );
    }

    fn run_case(&self) -> Result<(), String> {
        // PORT: Go ranges over the `deps` map; the order does not matter.
        let deps: &[(&str, &[&str])] = &[
            ("A", &["B", "C"]),
            ("B", &["C", "D"]),
            ("C", &["D", "E"]),
            ("F", &["E"]),
            ("H", &["I"]),
            ("I", &["J"]),
            ("J", &["H", "E"]),
        ];
        let deps_of = |project: &str| -> Vec<String> {
            deps.iter()
                .find(|(p, _)| *p == project)
                .map(|(_, d)| d.iter().map(|s| s.to_string()).collect())
                .unwrap_or_default()
        };
        let reverse_deps_of = |project: &str| -> Vec<String> {
            let mut out = Vec::new();
            for (p, ds) in deps {
                if ds.contains(&project) {
                    out.push(p.to_string());
                }
            }
            out
        };
        let verify_deps = |orchestrator: &ts_goport::execute::build::orchestrator::Orchestrator,
                           build_order: &[String],
                           has_down_stream: bool|
         -> Result<(), String> {
            for (index, project) in build_order.iter().enumerate() {
                let upstream: Vec<String> = orchestrator
                    .upstream(&Self::config_name(project))
                    .iter()
                    .map(|c| Self::project_name(c))
                    .collect();
                let expected_upstream = deps_of(project);
                if upstream.len() > expected_upstream.len() {
                    return Err(format!(
                        "Expected upstream for {project} to be at most {}, got {}",
                        expected_upstream.len(),
                        upstream.len()
                    ));
                }
                for expected in &expected_upstream {
                    if build_order[..index].contains(expected) {
                        if !upstream.contains(expected) {
                            return Err(format!(
                                "Expected upstream for {project} to contain {expected}"
                            ));
                        }
                    } else if upstream.contains(expected) {
                        return Err(format!(
                            "Expected upstream for {project} to not contain {expected}"
                        ));
                    }
                }

                let downstream: Vec<String> = orchestrator
                    .downstream(&Self::config_name(project))
                    .iter()
                    .map(|c| Self::project_name(c))
                    .collect();
                let expected_downstream = if has_down_stream {
                    reverse_deps_of(project)
                } else {
                    Vec::new()
                };
                if downstream.len() > expected_downstream.len() {
                    return Err(format!(
                        "Expected downstream for {project} to be at most {}, got {}",
                        expected_downstream.len(),
                        downstream.len()
                    ));
                }
                for expected in &expected_downstream {
                    if build_order[index + 1..].contains(expected) {
                        if !downstream.contains(expected) {
                            return Err(format!(
                                "Expected downstream for {project} to contain {expected}"
                            ));
                        }
                    } else if downstream.contains(expected) {
                        return Err(format!(
                            "Expected downstream for {project} to not contain {expected}"
                        ));
                    }
                }
            }
            Ok(())
        };

        let mut files = FileMap::new();
        for project in ["A", "B", "C", "D", "E", "F", "G", "H", "I", "J"] {
            files.insert(
                format!("/home/src/workspaces/project/{project}/{project}.ts"),
                "export {}".into(),
            );
            let mut references_str = String::new();
            let project_deps = deps_of(project);
            if !project_deps.is_empty() {
                references_str = format!(
                    r#", "references": [{}]"#,
                    project_deps
                        .iter()
                        .map(|dep| format!(r#"{{ "path": "../{dep}" }}"#))
                        .collect::<Vec<_>>()
                        .join(",")
                );
            }
            files.insert(
                Self::config_name(project),
                format!(
                    r#"{{
                "compilerOptions": {{ "composite": true }},
                "files": ["./{project}.ts"],
                {references_str}
            }}"#
                )
                .into(),
            );
        }

        let sys: Rc<dyn System> =
            Rc::new(new_tsc_system(files, true, "/home/src/workspaces/project"));
        let build = |flag: &str| -> ts_goport::execute::build::orchestrator::Orchestrator {
            let mut args: Vec<String> = vec!["--build".to_string(), flag.to_string()];
            args.extend(self.projects.iter().map(|p| p.to_string()));
            let build_command = parse_build_command_line(&args, &SystemParseConfigHost(&*sys));
            new_orchestrator(Options {
                sys: Rc::clone(&sys),
                command: Rc::new(build_command),
                testing: None,
            })
        };
        let mut orchestrator = build("--dry");
        orchestrator.generate_graph(None);
        let build_order: Vec<String> = orchestrator
            .order()
            .iter()
            .map(|c| Self::project_name(c))
            .collect();
        if build_order != self.expected {
            return Err(format!(
                "assert.DeepEqual(buildOrder, b.expected) failed: got {build_order:?}, want {:?}",
                self.expected
            ));
        }
        verify_deps(&orchestrator, &build_order, false)?;
        // Go: graph_test.go:245-247 (ts#64220)
        let schedule_order: Vec<String> = orchestrator
            .schedule_order()
            .iter()
            .map(|c| Self::project_name(c))
            .collect();
        if schedule_order != self.expected_schedule {
            return Err(format!(
                "assert.DeepEqual(scheduleOrder, b.expectedSchedule) failed: got {schedule_order:?}, want {:?}",
                self.expected_schedule
            ));
        }
        verify_deps(&orchestrator, &schedule_order, false)?;

        if !self.circular {
            for (project, project_deps) in deps {
                let child = Self::config_name(project);
                // PORT: Go looks up config names in the project-name order,
                // so the index is always -1 and the loop always continues.
                let Some(child_index) = build_order.iter().position(|p| *p == child) else {
                    continue;
                };
                for dep in *project_deps {
                    let parent = Self::config_name(dep);
                    let parent_index = build_order
                        .iter()
                        .position(|p| *p == parent)
                        .map_or(-1, |i| i as i64);
                    if child_index as i64 <= parent_index {
                        return Err(format!(
                            "Expecting child {project} to be built after parent {dep}"
                        ));
                    }
                }
            }
        }

        orchestrator.generate_graph_reusing_old_tasks();
        let build_order2: Vec<String> = orchestrator
            .order()
            .iter()
            .map(|c| Self::project_name(c))
            .collect();
        if build_order2 != self.expected {
            return Err(format!(
                "assert.DeepEqual(buildOrder2, b.expected) failed: got {build_order2:?}, want {:?}",
                self.expected
            ));
        }

        let mut orchestrator = build("--watch");
        orchestrator.generate_graph(None);
        let build_order3: Vec<String> = orchestrator
            .order()
            .iter()
            .map(|c| Self::project_name(c))
            .collect();
        verify_deps(&orchestrator, &build_order3, true)
    }
}

// Go: execute/build/graph_test.go:39 TestBuildScheduling (ts#64615)
// PORT: Go blocks the read of `Leaf/index.ts` until `Independent` starts,
// with a 30 s timeout. Here the builders are one thread: a task starts when
// it reads its files, and a started task writes its outputs when it is
// built (orchestrator.rs). So the read of `Independent/index.ts` checks that
// `Leaf/dist/index.js` is not written yet: `Independent` started while
// `Leaf` still held a builder. With the build order as the take order (Go
// before ts#64615), `Independent` waits behind `Middle`, which waits on
// `Leaf`, so `Leaf` is built first and the check fails.
fn test_build_scheduling(
    test: &str,
    project: &'static str,
    refs_only: bool,
    order: &'static [&'static str],
) {
    run_test_in_child(&format!("units_emit::execute_tests::{test}"), move || {
        let sys = new_scheduling_test_system();
        let command = parse_build_command_line(
            &["--build", "--verbose", "--builders", "2", "Root", "Other"].map(String::from),
            &SystemParseConfigHost(&*sys),
        );
        let mut orchestrator = new_orchestrator(Options {
            sys: sys.clone() as Rc<dyn System>,
            command: Rc::new(command),
            testing: None,
        });
        let ctx = context::background();
        let result = if refs_only {
            orchestrator.build_references(&ctx, project)
        } else {
            orchestrator.build(&ctx, project)
        };
        let output = String::from_utf8_lossy(&sys.output.borrow()).into_owned();
        assert_eq!(result.result.status, ExitStatus::Success, "{output}");
        assert_eq!(result.statistics.projects, order.len() as i32);
        assert_eq!(result.statistics.projects_built, order.len() as i32);
        let mut reported = Vec::new();
        for line in output.split('\n') {
            if line.contains("Building project") {
                for project in order {
                    if line.contains(&format!("'{project}/tsconfig.json'")) {
                        reported.push(*project);
                    }
                }
            }
        }
        assert_eq!(reported, order, "{output}");
        for project in ["Leaf", "Middle", "Independent", "Root", "Other"] {
            assert_eq!(
                sys.fs
                    .file_exists(&format!("/project/{project}/dist/index.js")),
                order.contains(&project),
                "{project}: {output}"
            );
        }
        if order.contains(&"Independent") {
            assert_eq!(
                sys.independent_started_while_leaf_built.get(),
                Some(true),
                "Independent was queued behind a builder waiting on Leaf"
            );
        }
    });
}

#[test]
fn test_build_scheduling_full_build() {
    test_build_scheduling(
        "test_build_scheduling_full_build",
        "",
        false,
        &["Leaf", "Middle", "Independent", "Root", "Other"],
    );
}

#[test]
fn test_build_scheduling_selected_project() {
    test_build_scheduling(
        "test_build_scheduling_selected_project",
        "Root",
        false,
        &["Leaf", "Middle", "Independent", "Root"],
    );
}

#[test]
fn test_build_scheduling_selected_references() {
    test_build_scheduling(
        "test_build_scheduling_selected_references",
        "Root",
        true,
        &["Leaf", "Middle", "Independent"],
    );
}

// Go: execute/build/graph_test.go:111 schedulingTestSystem, with
// graph_test.go:101 schedulingTestFS
// PORT: Go embeds `*tsctests.TestSys`; every method but `FS` and `Writer`
// calls the test system. The `schedulingTestFS` is a `wrapvfs` file system
// whose `ReadFile` calls the hook first (see `test_build_scheduling`). The
// hook stores whether `Leaf/dist/index.js` was missing when
// `Independent/index.ts` was read (`None` until then).
struct SchedulingTestSystem {
    test_sys: Rc<TestSys>,
    fs: Rc<dyn Fs>,
    output: Rc<RefCell<Vec<u8>>>,
    independent_started_while_leaf_built: Rc<Cell<Option<bool>>>,
}

impl System for SchedulingTestSystem {
    // Go: graph_test.go:118 schedulingTestSystem.Writer
    fn writer(&self) -> Writer {
        self.output.clone()
    }
    fn error_writer(&self) -> ErrorWriter {
        System::error_writer(&*self.test_sys)
    }
    // Go: graph_test.go:117 schedulingTestSystem.FS
    fn fs(&self) -> Rc<dyn Fs> {
        self.fs.clone()
    }
    fn default_library_path(&self) -> String {
        System::default_library_path(&*self.test_sys)
    }
    fn get_current_directory(&self) -> String {
        System::get_current_directory(&*self.test_sys)
    }
    fn write_output_is_tty(&self) -> bool {
        System::write_output_is_tty(&*self.test_sys)
    }
    fn get_width_of_terminal(&self) -> i32 {
        System::get_width_of_terminal(&*self.test_sys)
    }
    fn get_environment_variable(&self, name: &str) -> (String, bool) {
        System::get_environment_variable(&*self.test_sys, name)
    }
    fn spawn(
        &self,
        command: &[String],
        dir: &str,
        stderr: Option<Box<dyn std::io::Write + Send>>,
    ) -> Result<Arc<dyn ProcessExitState>, GoError> {
        System::spawn(&*self.test_sys, command, dir, stderr)
    }
    fn now(&self) -> SystemTime {
        System::now(&*self.test_sys)
    }
    fn since_start(&self) -> Duration {
        System::since_start(&*self.test_sys)
    }
}

// Go: execute/build/graph_test.go:120 newSchedulingTestSystem
// PORT: Go `tsctests.NewTscSystem(files, tspath.CaseSensitive, "/project")`
// is the in-process test system of `newTestSys` with the cwd `/project`
// (`new_in_process_test_sys`), which also adds the test default libraries;
// these projects set `noLib`. Go ranges over the `deps` map; the order does
// not matter.
fn new_scheduling_test_system() -> Rc<SchedulingTestSystem> {
    let mut files: FileMap = [(
        "/project/lib.d.ts".to_string(),
        r#"
interface Array<T> {}
interface Boolean {}
interface CallableFunction {}
interface Function {}
interface IArguments {}
interface NewableFunction {}
interface Number {}
interface Object {}
interface RegExp {}
interface String {}
"#
        .into(),
    )]
    .into_iter()
    .collect();
    let deps: &[(&str, &[&str])] = &[
        ("Root", &["Middle", "Independent"]),
        ("Middle", &["Leaf"]),
        ("Leaf", &[]),
        ("Independent", &[]),
        ("Other", &[]),
    ];
    for (project, references) in deps {
        files.insert(
            format!("/project/{project}/index.ts"),
            "export const value = 1;".into(),
        );
        let references: Vec<String> = references
            .iter()
            .map(|reference| format!(r#"{{ "path": "../{reference}" }}"#))
            .collect();
        files.insert(
            format!("/project/{project}/tsconfig.json"),
            format!(
                r#"{{
			"compilerOptions": {{ "composite": true, "noLib": true, "outDir": "dist" }},
			"files": ["../lib.d.ts", "index.ts"],
			"references": [{}]
		}}"#,
                references.join(",")
            )
            .into(),
        );
    }
    let test_sys = new_in_process_test_sys(&TscInput {
        files,
        cwd: "/project".into(),
        ..Default::default()
    });
    let inner = System::fs(&*test_sys);
    let independent_started_while_leaf_built = Rc::new(Cell::new(None));
    let fs = wrapvfs_wrap(
        inner.clone(),
        Replacements {
            read_file: Some(Box::new({
                let inner = inner.clone();
                let started = independent_started_while_leaf_built.clone();
                move |path: &str| {
                    if path == "/project/Independent/index.ts" && started.get().is_none() {
                        started.set(Some(!inner.file_exists("/project/Leaf/dist/index.js")));
                    }
                    inner.read_file(path)
                }
            })),
            ..Default::default()
        },
    );
    Rc::new(SchedulingTestSystem {
        test_sys,
        fs,
        output: Rc::new(RefCell::new(Vec::new())),
        independent_started_while_leaf_built,
    })
}

// followups76r2: Go packageJsonLookupChanged tests a deleted path with a
// path key test (`PathKey.ContainsPath`, orchestrator.go:676). Go's
// case-insensitive key folds case with ToLower, which keeps 'ſ' and 'ς'
// (EqualFold would match them to 's' and 'σ'): deleting "/p/ſ" does not
// change "/p/s/package.json"; deleting "/p/S" does. The answers come from a
// probe of Go at 3e9236361fa9 (`TestF76R2PackageJsonProbe`).
#[test]
fn package_json_lookup_changed_tests_path_keys_like_go() {
    let sys: Rc<dyn System> = Rc::new(new_tsc_system(FileMap::new(), false, "/p"));
    let build_command = parse_build_command_line(&[], &SystemParseConfigHost(&*sys));
    let orchestrator = new_orchestrator(Options {
        sys: Rc::clone(&sys),
        command: Rc::new(build_command),
        testing: None,
    });
    let cases: &[(&str, EventKind, &str, bool)] = &[
        ("/p/\u{17f}", EventKind::Delete, "/p/s/package.json", false),
        (
            "/p/\u{17f}",
            EventKind::Delete,
            "/p/\u{3c3}/package.json",
            false,
        ),
        ("/p/S", EventKind::Delete, "/p/s/package.json", true),
        ("/p/S", EventKind::Delete, "/p/\u{3c3}/package.json", false),
        ("/p/\u{3c2}", EventKind::Delete, "/p/s/package.json", false),
        (
            "/p/\u{3c2}",
            EventKind::Delete,
            "/p/\u{3c3}/package.json",
            false,
        ),
        ("/p/S", EventKind::Update, "/p/s/package.json", false),
        ("/p/S", EventKind::Update, "/p/\u{3c3}/package.json", false),
        (
            "/p/s/package.json",
            EventKind::Update,
            "/p/s/package.json",
            true,
        ),
        (
            "/p/s/package.json",
            EventKind::Update,
            "/p/\u{3c3}/package.json",
            false,
        ),
    ];
    for &(changed, kind, package_json, expected) in cases {
        let changed_paths: FxHashMap<_, _> = [(orchestrator.to_path(changed), kind)]
            .into_iter()
            .collect();
        assert_eq!(
            orchestrator.package_json_lookup_changed(package_json, &changed_paths),
            expected,
            "{changed} {kind:?} {package_json}"
        );
    }
}

// ---------------------------------------------------------------------------
// execute/tsc/extendedconfigcache_test.go
// ---------------------------------------------------------------------------

// Go: execute/tsc/extendedconfigcache_test.go:12 testParseConfigHost
struct TestParseConfigHost {
    fs: Rc<dyn Fs>,
    cwd: String,
}

impl ParseConfigHost for TestParseConfigHost {
    fn fs(&self) -> Rc<dyn Fs> {
        Rc::clone(&self.fs)
    }
    fn get_current_directory(&self) -> String {
        self.cwd.clone()
    }
}

/// Go `tsoptions.GetParsedCommandLineOfConfigFile("/project/tsconfig.json",
/// nil, nil, host, &tsc.ExtendedConfigCache{})` over a map file system with
/// `files`.
// PORT: Go `tsc.ExtendedConfigCache` is `TscExtendedConfigCache` in
// execute/build/host.rs.
fn parse_with_cache(files: &[(&str, &str)]) -> Option<ParsedCommandLine> {
    let fs = vfstest::from_map(
        files.iter().copied(),
        false, /*useCaseSensitiveFileNames*/
    );
    let host = TestParseConfigHost {
        fs,
        cwd: "/project".to_string(),
    };
    let cache = TscExtendedConfigCache::default();

    let (cmd, _) = get_parsed_command_line_of_config_file(
        "/project/tsconfig.json",
        None,
        None,
        &host,
        Some(&cache),
    );
    cmd
}

// Go: execute/tsc/extendedconfigcache_test.go:110 assertHasCircularityDiagnostic
fn assert_has_circularity_diagnostic(cmd: &ParsedCommandLine) -> Result<(), String> {
    if cmd.errors.iter().any(|d| d.code == 18000) {
        return Ok(());
    }
    Err(format!(
        "expected circularity diagnostic (code 18000), but none was found; errors: {:?}",
        cmd.errors.iter().map(|d| d.code).collect::<Vec<_>>()
    ))
}

// Go: execute/tsc/extendedconfigcache_test.go:20 TestExtendedConfigCacheExtendsCircularity
#[test]
fn test_extended_config_cache_extends_circularity() {
    let mut t = Subtests::new("TestExtendedConfigCacheExtendsCircularity");

    t.run("self-referencing extends", || {
        // Regression test: a tsconfig extends cycle should produce an error,
        // not a deadlock when using the tsc ExtendedConfigCache.
        let cmd = parse_with_cache(&[
            ("/project/tsconfig.json", r#"{"extends": "./base.json"}"#),
            ("/project/base.json", r#"{"extends": "./base.json"}"#),
            ("/project/main.ts", "// Hello World!"),
        ])
        .ok_or("expected non-nil ParsedCommandLine")?;
        assert_has_circularity_diagnostic(&cmd)
    });

    t.run("mutual extends cycle", || {
        // Two config files that extend each other.
        let cmd = parse_with_cache(&[
            ("/project/tsconfig.json", r#"{"extends": "./other.json"}"#),
            ("/project/other.json", r#"{"extends": "./tsconfig.json"}"#),
            ("/project/main.ts", "// Hello World!"),
        ])
        .ok_or("expected non-nil ParsedCommandLine")?;
        assert_has_circularity_diagnostic(&cmd)
    });

    t.run("case-insensitive self-referencing extends", || {
        // On a case-insensitive FS, ./Base.json and ./base.json resolve to the same
        // cache entry. The cycle check must use canonical paths to avoid deadlock.
        let cmd = parse_with_cache(&[
            ("/project/tsconfig.json", r#"{"extends": "./Base.json"}"#),
            ("/project/base.json", r#"{"extends": "./base.json"}"#),
            ("/project/main.ts", "// Hello World!"),
        ])
        .ok_or("expected non-nil ParsedCommandLine")?;
        assert_has_circularity_diagnostic(&cmd)
    });

    t.finish();
}

// Go: execute/tsc/extendedconfigcache_test.go:89 TestExtendedConfigCacheNullExtendsDoesNotPanic
#[test]
fn test_extended_config_cache_null_extends_does_not_panic() {
    let cmd = parse_with_cache(&[
        ("/project/tsconfig.json", r#"{"extends": null}"#),
        ("/project/main.ts", "// Hello World!"),
    ])
    .expect("expected non-nil ParsedCommandLine");
    assert!(
        !cmd.errors.is_empty(),
        "expected diagnostics for invalid null extends"
    );
}
