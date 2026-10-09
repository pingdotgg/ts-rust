//! Go: internal/execute/tsctests/runner.go: the tsc scenario runner
//! (`tscInput`, `tscEdit`, `run`, `getDiffForIncremental`,
//! `getBaselineSubFolder`).
//!
//! PORT: Go runs each input as a parallel subtest. `run_tsc_inputs` runs
//! the inputs of one Go test function on `TSCTEST_JOBS` threads (default
//! 4), prints one line per baseline (`ok <baseline>` or `FAIL <baseline>:
//! <reason>`) and panics once at the end with every failure. Each
//! `execute.CommandLine` runs in a child process (child.rs).
//! `TSCTEST_FILTER=<substring>` runs only the inputs whose
//! `<subfolder>/<subScenario>` contains it.
//!
//! PORT: a Go test function with watch inputs (`-w`) is two `#[test]`s
//! here: one runs the other inputs (`WatchFilter::NonWatch`) and one the
//! watch inputs (`WatchFilter::WatchOnly`). A watch command's child keeps
//! its watcher across the edits (child.rs `WatchChild`).

use std::collections::{BTreeMap, VecDeque};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex};

use ts_goport::execute::tsc::compile::{ExitStatus, System};
use ts_goport::frontend::tspath::{EXTENSION_TS_BUILD_INFO, file_extension_is};

use crate::support::baseline;
use crate::support::child::{ChildRun, run_command_in_child};
use crate::support::test_sys::{TestSys, lock, new_test_sys};
use crate::support::vfstest::MapFile;

const FILTER_ENV: &str = "TSCTEST_FILTER";
const JOBS_ENV: &str = "TSCTEST_JOBS";
const DEFAULT_JOBS: usize = 4;
/// Failures that the final panic lists.
const MAX_LISTED_FAILURES: usize = 50;

// Go: sys.go:34 FileMap
// PORT: Go `map[string]any`; each value is a `MapFile` (Go string, byte
// slice or `*fstest.MapFile`).
pub type FileMap = BTreeMap<String, MapFile>;

/// Go `FileMap{"path": value, ...}`. Each value goes through `MapFile::from`
/// (`&str`, `String`, `Vec<u8>` or a `MapFile` such as `symlink(...)`).
macro_rules! file_map {
    ($($path:expr => $value:expr),* $(,)?) => {{
        #[allow(unused_mut)]
        let mut files = $crate::support::runner::FileMap::new();
        $(
            files.insert(
                ::std::string::String::from($path),
                $crate::support::vfstest::MapFile::from($value),
            );
        )*
        files
    }};
}
pub(crate) use file_map;

/// Go `[]string{"-b", ...}` for a command line.
macro_rules! args {
    ($($arg:expr),* $(,)?) => {
        ::std::vec![$(::std::string::String::from($arg)),*]
    };
}
pub(crate) use args;

/// Go `func(*TestSys)` of a `tscEdit`.
pub type EditFn = Arc<dyn Fn(&TestSys) + Send + Sync>;

/// Go `edit: func(sys *TestSys) { ... }`.
pub fn edit(f: impl Fn(&TestSys) + Send + Sync + 'static) -> Option<EditFn> {
    Some(Arc::new(f))
}

// Go: runner.go:18 tscEdit
// PORT: Go nil `commandLineArgs` is `None`.
#[derive(Clone, Default)]
pub struct TscEdit {
    pub caption: String,
    pub command_line_args: Option<Vec<String>>,
    pub edit: Option<EditFn>,
    pub expected_diff: String,
}

// Go: runner.go:25 noChange
pub fn no_change() -> TscEdit {
    TscEdit {
        caption: "no change".to_string(),
        ..TscEdit::default()
    }
}

// Go: runner.go:29 noChangeOnlyEdit
pub fn no_change_only_edit() -> Vec<TscEdit> {
    vec![no_change()]
}

// Go: runner.go:33 tscInput
// PORT: Go nil `commandLineArgs` is an empty `Vec`.
#[derive(Clone, Default)]
pub struct TscInput {
    pub sub_scenario: String,
    pub command_line_args: Vec<String>,
    pub files: FileMap,
    pub cwd: String,
    pub edits: Vec<TscEdit>,
    pub env: BTreeMap<String, String>,
    /// Go `outputIsTTY *bool` (ts#63941): `None` is nil.
    pub output_is_tty: Option<bool>,
    pub ignore_case: bool,
    pub windows_style_root: String,
}

/// Which inputs a test runs (see the module comment).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WatchFilter {
    /// Inputs without a watch flag.
    NonWatch,
    /// Inputs with a watch flag.
    WatchOnly,
}

/// Go `tsc.ExitStatus` names of `executeCommand` (runner.go:47).
fn exit_status_text(status: ExitStatus) -> &'static str {
    match status {
        ExitStatus::Success => "ExitStatus:: Success",
        ExitStatus::DiagnosticsPresentOutputsSkipped => {
            "ExitStatus:: DiagnosticsPresent_OutputsSkipped"
        }
        ExitStatus::DiagnosticsPresentOutputsGenerated => {
            "ExitStatus:: DiagnosticsPresent_OutputsGenerated"
        }
        ExitStatus::InvalidProjectOutputsSkipped => "ExitStatus:: InvalidProject_OutputsSkipped",
        ExitStatus::ProjectReferenceCycleOutputsSkipped => {
            "ExitStatus:: ProjectReferenceCycle_OutputsSkipped"
        }
        ExitStatus::NotImplemented => "ExitStatus:: NotImplemented",
    }
}

/// The problems of one run that are not baseline differences.
#[derive(Default)]
struct RunNotes {
    /// Unported Go code that a command reached.
    unported: Vec<String>,
}

impl TscInput {
    // Go: runner.go:45 executeCommand
    // PORT: the command runs in a child process. An `Err` is a child that
    // ended without a result (see child.rs). Go passes the subtest context
    // (`t.Context()`, tsgo#4712), whose end closes a content mapper host
    // that the command made. Here the child ends with its process (a watch
    // child when the input ends), and its mapper connections with it.
    fn execute_command(
        &self,
        sys: &TestSys,
        baseline_builder: &mut String,
        command_line_args: &[String],
        notes: &mut RunNotes,
    ) -> Result<ChildRun, String> {
        baseline_builder.push_str("tsgo ");
        baseline_builder.push_str(&command_line_args.join(" "));
        baseline_builder.push('\n');
        let result = run_command_in_child(sys, command_line_args)?;
        baseline_builder.push_str(exit_status_text(result.status));
        if let Some(unported) = &result.unported {
            notes
                .unported
                .push(format!("tsgo {}: {unported}", command_line_args.join(" ")));
        }
        Ok(result)
    }

    /// Go `strings.ReplaceAll(test.subScenario, " ", "-") + ".js"`.
    pub fn baseline_name(&self) -> String {
        format!("{}.js", self.sub_scenario.replace(' ', "-"))
    }

    // Go: runner.go:67 run
    // PORT: returns the failures in place of `t.Errorf`. A child without a
    // result stops the input; the baseline so far is still written when
    // local baselines are on, to show where it stopped.
    fn run(&self, scenario: &str) -> Result<(), String> {
        let subfolder = format!("{}/{scenario}", self.get_baseline_sub_folder());
        let baseline_options = baseline::Options {
            subfolder,
            ..baseline::Options::default()
        };
        let mut notes = RunNotes::default();
        let mut baseline_builder = String::new();
        let mut unexpected_diff = String::new();
        let result = self.run_steps(&mut baseline_builder, &mut unexpected_diff, &mut notes);

        let mut failures: Vec<String> = Vec::new();
        if let Err(child_failure) = result {
            baseline_builder.push_str(&format!("\n\n!!! tsctest: {child_failure}\n"));
            failures.push(child_failure);
        }
        if let Err(diff) =
            baseline::run(&self.baseline_name(), &baseline_builder, &baseline_options)
        {
            failures.push(diff);
        }
        if !unexpected_diff.is_empty() {
            failures.push(format!(
                "Test {} has unexpected diff {} with incremental build, please review the baseline file",
                self.sub_scenario,
                truncate(&unexpected_diff)
            ));
        }
        for unported in notes.unported {
            failures.push(format!("unported: {unported}"));
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(failures.join("\n"))
        }
    }

    /// The body of Go `run` up to `baseline.Run`.
    fn run_steps(
        &self,
        baseline_builder: &mut String,
        unexpected_diff: &mut String,
        notes: &mut RunNotes,
    ) -> Result<(), String> {
        // initial test tsc compile
        let sys = new_test_sys(self, false);
        baseline_builder.push_str(&format!(
            "currentDirectory::{}\nuseCaseSensitiveFileNames::{}\nInput::\n",
            sys.get_current_directory(),
            sys.fs().use_case_sensitive_file_names(),
        ));
        sys.baseline_fs_with_diff(baseline_builder);
        let result =
            self.execute_command(&sys, baseline_builder, &self.command_line_args, notes)?;
        sys.serialize_state(baseline_builder);
        // Go: `if result.Watcher != nil && sys.mockWatchBackend.HasWatches()`
        // (the child sends the watch state only then).
        if let Some(watch_state) = &result.watch_state {
            baseline_builder.push_str(watch_state);
        }
        let mut watcher = result.watcher;
        unexpected_diff.push_str(&sys.baseline_programs(baseline_builder, "Initial build"));

        for (index, edit) in self.edits.iter().enumerate() {
            sys.clear_output();
            let command_line_args = edit
                .command_line_args
                .as_deref()
                .unwrap_or(&self.command_line_args);
            // PORT: Go runs the two parts below in parallel (one
            // `core.WorkGroup`); they use separate systems, so this runs
            // them one after the other.
            {
                baseline_builder.push_str(&format!("\n\nEdit [{index}]:: {}\n", edit.caption));
                if let Some(edit_fn) = &edit.edit {
                    edit_fn(&sys);
                }
                let changed_paths = sys.changed_paths();
                sys.baseline_fs_with_diff(baseline_builder);

                let mut watch_state = None;
                match &mut watcher {
                    // PORT: Go discards this result, so its watcher (a
                    // watch command line in an edit) ends here.
                    None => {
                        self.execute_command(&sys, baseline_builder, command_line_args, notes)?;
                    }
                    Some(watcher) => {
                        let cycle = watcher
                            .do_cycle(&sys, &changed_paths)
                            .map_err(|err| format!("watch cycle of edit [{index}]: {err}"))?;
                        if let Some(unported) = cycle.unported {
                            notes
                                .unported
                                .push(format!("watch cycle of edit [{index}]: {unported}"));
                        }
                        watch_state = cycle.watch_state;
                    }
                }
                sys.serialize_state(baseline_builder);
                if let Some(watch_state) = &watch_state {
                    baseline_builder.push_str(watch_state);
                }
                unexpected_diff.push_str(&sys.baseline_programs(
                    baseline_builder,
                    &format!("Edit [{index}]:: {}\n", edit.caption),
                ));
            }
            // Compute build with all the edits
            let non_incremental_sys = new_test_sys(self, true);
            for previous in &self.edits[..=index] {
                if let Some(edit_fn) = &previous.edit {
                    edit_fn(&non_incremental_sys);
                }
            }
            let non_incremental = run_command_in_child(&non_incremental_sys, command_line_args)
                .map_err(|err| format!("non-incremental build of edit [{index}]: {err}"))?;
            if let Some(unported) = non_incremental.unported {
                notes.unported.push(format!(
                    "non-incremental tsgo {}: {unported}",
                    command_line_args.join(" ")
                ));
            }

            let diff = get_diff_for_incremental(&sys, &non_incremental_sys);
            if !diff.is_empty() {
                let explanation = if edit.expected_diff.is_empty() {
                    "!!! Unexpected diff, please review and either fix or write explanation as expectedDiff !!!"
                } else {
                    edit.expected_diff.as_str()
                };
                baseline_builder.push_str(&format!("\n\nDiff:: {explanation}\n"));
                baseline_builder.push_str(&diff);
                if edit.expected_diff.is_empty() {
                    unexpected_diff.push_str(&format!(
                        "Edit [{index}]:: {}\n!!! Unexpected diff, please review and either fix or write explanation as expectedDiff !!!\n{diff}\n",
                        edit.caption
                    ));
                }
            } else if !edit.expected_diff.is_empty() {
                baseline_builder.push_str(&format!(
                    "\n\nDiff:: {} !!! Diff not found but explanation present, please review and remove the explanation !!!\n",
                    edit.expected_diff
                ));
                unexpected_diff.push_str(&format!(
                    "Edit [{index}]:: {}\n!!! Diff not found but explanation present, please review and remove the explanation !!!\n",
                    edit.caption
                ));
            }
        }
        Ok(())
    }

    // Go: runner.go:184 getBaselineSubFolder
    pub fn get_baseline_sub_folder(&self) -> String {
        let command_name = if self
            .command_line_args
            .iter()
            .any(|arg| matches!(arg.as_str(), "-b" | "--b" | "-build" | "--build"))
        {
            "tsbuild"
        } else {
            "tsc"
        };
        let w = if self
            .command_line_args
            .iter()
            .any(|arg| matches!(arg.as_str(), "-w" | "--w" | "-watch" | "--watch"))
        {
            "Watch"
        } else {
            ""
        };
        format!("{command_name}{w}")
    }

    /// Whether the input runs in watch mode (`tscWatch`, `tsbuildWatch`).
    pub fn is_watch(&self) -> bool {
        self.get_baseline_sub_folder().ends_with("Watch")
    }
}

// Go: runner.go:149 getDiffForIncremental
fn get_diff_for_incremental(incremental_sys: &TestSys, non_incremental_sys: &TestSys) -> String {
    let mut diff_builder = String::new();

    let mut non_incremental_outputs: Vec<String> =
        lock(&non_incremental_sys.shared().written_files)
            .iter()
            .cloned()
            .collect();
    non_incremental_outputs.sort();
    for non_incremental_output in &non_incremental_outputs {
        if file_extension_is(non_incremental_output, EXTENSION_TS_BUILD_INFO)
            || non_incremental_output.ends_with(".readable.baseline.txt")
        {
            // Just check existence
            if !incremental_sys
                .fs_from_file_map()
                .file_exists(non_incremental_output)
            {
                diff_builder.push_str(&baseline::diff_text(
                    &format!("nonIncremental {non_incremental_output}"),
                    &format!("incremental {non_incremental_output}"),
                    "Exists",
                    "",
                ));
                diff_builder.push('\n');
            }
        } else {
            let (non_incremental_text, ok) = non_incremental_sys
                .fs_from_file_map()
                .read_file(non_incremental_output);
            assert!(ok, "Written file not found {non_incremental_output}");
            let (incremental_text, ok) = incremental_sys
                .fs_from_file_map()
                .read_file(non_incremental_output);
            if !ok || incremental_text != non_incremental_text {
                diff_builder.push_str(&baseline::diff_text(
                    &format!("nonIncremental {non_incremental_output}"),
                    &format!("incremental {non_incremental_output}"),
                    &non_incremental_text,
                    &incremental_text,
                ));
                diff_builder.push('\n');
            }
        }
    }

    let incremental_output = incremental_sys.get_output(true);
    let non_incremental_output = non_incremental_sys.get_output(true);
    if incremental_output != non_incremental_output {
        diff_builder.push_str(&baseline::diff_text(
            "nonIncremental.output.txt",
            "incremental.output.txt",
            &non_incremental_output,
            &incremental_output,
        ));
    }
    diff_builder
}

/// Failure text longer than this is cut (the baseline shows all of it).
const MAX_REASON_CHARS: usize = 2000;

/// `text`, cut to `MAX_REASON_CHARS` characters.
fn truncate(text: &str) -> String {
    match text.char_indices().nth(MAX_REASON_CHARS) {
        Some((end, _)) => format!("{}... ({} more bytes)", &text[..end], text.len() - end),
        None => text.to_string(),
    }
}

/// The text of a panic payload.
fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(text) = payload.downcast_ref::<&str>() {
        (*text).to_string()
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else {
        "panic".to_string()
    }
}

/// Runs the inputs of one Go test function that `filter` and
/// `TSCTEST_FILTER` select, and returns `(baseline, result)` per input in
/// input order. It prints one line per baseline.
pub fn run_tsc_input_results(
    scenario: &str,
    inputs: Vec<TscInput>,
    filter: WatchFilter,
) -> Vec<(String, Result<(), String>)> {
    let name_filter = std::env::var(FILTER_ENV).ok().filter(|f| !f.is_empty());
    let selected: Vec<(usize, TscInput)> = inputs
        .into_iter()
        .filter(|input| match filter {
            WatchFilter::NonWatch => !input.is_watch(),
            WatchFilter::WatchOnly => input.is_watch(),
        })
        .filter(|input| {
            name_filter.as_ref().is_none_or(|name_filter| {
                format!("{}/{}", input.get_baseline_sub_folder(), input.sub_scenario)
                    .contains(name_filter.as_str())
            })
        })
        .enumerate()
        .collect();
    let jobs = std::env::var(JOBS_ENV)
        .ok()
        .and_then(|jobs| jobs.parse::<usize>().ok())
        .unwrap_or(DEFAULT_JOBS)
        .clamp(1, selected.len().max(1));

    let queue = Mutex::new(selected.into_iter().collect::<VecDeque<_>>());
    // (input index, name, result)
    type IndexedResult = (usize, String, Result<(), String>);
    let results: Mutex<Vec<IndexedResult>> = Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for _ in 0..jobs {
            scope.spawn(|| {
                loop {
                    let Some((index, input)) = lock(&queue).pop_front() else {
                        break;
                    };
                    let name = format!(
                        "{}/{scenario}/{}",
                        input.get_baseline_sub_folder(),
                        input.baseline_name()
                    );
                    let result = catch_unwind(AssertUnwindSafe(|| input.run(scenario)))
                        .unwrap_or_else(|payload| {
                            Err(format!("panic: {}", panic_text(payload.as_ref())))
                        });
                    match &result {
                        Ok(()) => println!("ok {name}"),
                        Err(reason) => println!("FAIL {name}: {reason}"),
                    }
                    lock(&results).push((index, name, result));
                }
            });
        }
    });
    let mut results = results
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    results.sort_by_key(|(index, _, _)| *index);
    results
        .into_iter()
        .map(|(_, name, result)| (name, result))
        .collect()
}

/// Panics with the failures of `results`, if any (see `run_tsc_inputs`).
pub fn check_results(label: &str, results: &[(String, Result<(), String>)]) {
    let failures: Vec<String> = results
        .iter()
        .filter_map(|(name, result)| {
            result
                .as_ref()
                .err()
                .map(|reason| format!("FAIL {name}: {reason}"))
        })
        .collect();
    if failures.is_empty() {
        return;
    }
    let listed = failures
        .iter()
        .take(MAX_LISTED_FAILURES)
        .cloned()
        .collect::<Vec<_>>()
        .join("\n");
    panic!(
        "{label}: {} of {} baselines failed\n{listed}",
        failures.len(),
        results.len()
    );
}

/// Go `for _, test := range testCases { test.run(t, scenario) }` for the
/// inputs that `filter` selects. Panics once at the end with the number of
/// failures and the first 50.
pub fn run_tsc_inputs(scenario: &str, inputs: Vec<TscInput>, filter: WatchFilter) {
    let results = run_tsc_input_results(scenario, inputs, filter);
    check_results(scenario, &results);
}
