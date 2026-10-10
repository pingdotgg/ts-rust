//! Port of internal/testutil/jstest/node.go.
//!
//! PORT: Go `testing.TB` is not passed. Go `t.Fatal` in `getNodeExe` is a
//! panic, and Go `t.Skip` is `skip_if_no_node_js` returning true. Go
//! `t.TempDir()` is `TempDir`, a directory under `TSCTEST_TMP` (default
//! `DEFAULT_TMP_ROOT`) that is removed on drop.

use super::repo;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};
use ts_goport::frontend::json::{UnmarshalerFrom, json_unmarshal};
use ts_goport::frontend::tspath;

/// Root of the temp directories when `TSCTEST_TMP` is not set.
#[cfg(not(target_os = "macos"))]
pub(crate) const DEFAULT_TMP_ROOT: &str =
    "/home/theo/Code/sandbox/ts-rust/target/continuation-r97-goport/go-baseline-tests/tmp";
/// macOS: `/home` is an autofs mount there, so the default is under /tmp.
#[cfg(target_os = "macos")]
pub(crate) const DEFAULT_TMP_ROOT: &str = "/tmp/ts-rust-go-baseline-tests/tmp";

// Go: jstest/node.go:16 loaderScript
const LOADER_SCRIPT: &str = r#"import script from "./script.mjs";
process.stdout.write(JSON.stringify(await script(...process.argv.slice(2))));"#;

// Go: jstest/node.go:19 getNodeExeOnce
// PORT: Go `exec.LookPath("node")` is a search of PATH for an executable
// file. Go returns "" when node is not found; the port returns `None`.
fn get_node_exe_once() -> Option<&'static Path> {
    static NODE_EXE: OnceLock<Option<PathBuf>> = OnceLock::new();
    NODE_EXE
        .get_or_init(|| {
            // Go `exec.LookPath("node")` adds the PATHEXT extensions on Windows.
            const EXE_NAME: &str = if cfg!(windows) { "node.exe" } else { "node" };
            let path = std::env::var_os("PATH")?;
            std::env::split_paths(&path)
                .map(|dir| dir.join(EXE_NAME))
                .find(|candidate| is_executable(candidate))
        })
        .as_deref()
}

// Go: os/exec/lp_windows.go:22 findExecutable (a file that is not a directory)
#[cfg(windows)]
fn is_executable(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.is_file())
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

// Go: jstest/node.go:31 EvalNodeScript
/// EvalNodeScript imports a Node.js script that default-exports a single function,
/// calls it with the provided arguments, and unmarshals the JSON-stringified
/// awaited return value into T.
pub(crate) fn eval_node_script<T: UnmarshalerFrom + Default>(
    script: &str,
    dir: &Path,
    args: &[&str],
) -> Result<T, String> {
    eval_node_script_with_loader(script, LOADER_SCRIPT, dir, args)
}

// Go: jstest/node.go:37 EvalNodeScriptWithTS
/// EvalNodeScriptWithTS is like EvalNodeScript, but provides the TypeScript
/// library to the script as the first argument.
// PORT: Go `dir == ""` is `None`.
pub(crate) fn eval_node_script_with_ts<T: UnmarshalerFrom + Default>(
    script: &str,
    dir: Option<&Path>,
    args: &[&str],
) -> Result<T, String> {
    let temp_dir;
    let dir = match dir {
        Some(dir) => dir,
        None => {
            temp_dir = TempDir::new();
            temp_dir.path()
        }
    };
    // Go N (microsoft/TypeScript layout): `node_modules` is in the repo root,
    // the parent of `tsc/`.
    let ts_src_path = repo::root_path().join("../node_modules/typescript/lib/typescript.js");
    let mut ts_src = tspath::normalize_path(&ts_src_path.to_string_lossy());
    if ts_src.starts_with('/') {
        ts_src = format!("file://{ts_src}");
    } else {
        ts_src = format!("file:///{ts_src}");
    }
    let ts_loader_script = format!(
        r#"import script from "./script.mjs";
import * as ts from "{ts_src}";
process.stdout.write(JSON.stringify(await script(ts, ...process.argv.slice(2))));"#
    );
    eval_node_script_with_loader(script, &ts_loader_script, dir, args)
}

// Go: jstest/node.go:53 SkipIfNoNodeJS
/// True (after it prints the skip reason) when the test must return.
pub(crate) fn skip_if_no_node_js(test: &str) -> bool {
    if get_node_exe_once().is_none() {
        println!("SKIP {test}: Node.js not found");
        return true;
    }
    false
}

// Go: jstest/node.go:60 evalNodeScript
fn eval_node_script_with_loader<T: UnmarshalerFrom + Default>(
    script: &str,
    loader: &str,
    dir: &Path,
    args: &[&str],
) -> Result<T, String> {
    let exe = get_node_exe();
    let script_path = dir.join("script.mjs");
    std::fs::write(&script_path, script).map_err(|err| err.to_string())?;
    let loader_path = dir.join("loader.mjs");
    std::fs::write(&loader_path, loader).map_err(|err| err.to_string())?;

    let mut exec_cmd = Command::new(exe);
    exec_cmd.arg(&loader_path).args(args).current_dir(dir);
    let output = match combined_output(exec_cmd) {
        Ok(output) => output,
        Err((err, output)) => {
            return Err(format!(
                "failed to run node: {err}\n{}",
                String::from_utf8_lossy(&output)
            ));
        }
    };

    let mut result = T::default();
    json_unmarshal(&output, &mut result, &[])
        .map_err(|err| format!("failed to unmarshal JSON output: {err}"))?;
    Ok(result)
}

/// Go `(*exec.Cmd).CombinedOutput`: stdout and stderr share one pipe. The
/// error text of a failed exit is the Go `*exec.ExitError` text.
fn combined_output(mut cmd: Command) -> Result<Vec<u8>, (String, Vec<u8>)> {
    let fail = |err: std::io::Error| (err.to_string(), Vec::new());
    let (mut reader, writer) = std::io::pipe().map_err(fail)?;
    cmd.stdout(writer.try_clone().map_err(fail)?).stderr(writer);
    let mut child = cmd.spawn().map_err(fail)?;
    // The command holds the write ends; drop it so the read sees EOF.
    drop(cmd);
    let mut output = Vec::new();
    let read = reader.read_to_end(&mut output);
    let status = child.wait().map_err(fail)?;
    if let Err(err) = read {
        return Err((err.to_string(), output));
    }
    if !status.success() {
        let err = match status.code() {
            Some(code) => format!("exit status {code}"),
            None => status.to_string(),
        };
        return Err((err, output));
    }
    Ok(output)
}

// Go: jstest/node.go:89 getNodeExe
fn get_node_exe() -> &'static Path {
    if let Some(exe) = get_node_exe_once() {
        return exe;
    }
    panic!("Node.js not found");
}

/// Go `t.TempDir()`: a new empty directory, removed when this value drops.
pub(crate) struct TempDir(PathBuf);

impl TempDir {
    pub(crate) fn new() -> TempDir {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::var_os("TSCTEST_TMP")
            .map_or_else(|| PathBuf::from(DEFAULT_TMP_ROOT), PathBuf::from);
        let dir = root.join(format!(
            "jstest-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        // A directory left by a killed run with the same pid starts empty.
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)
            .unwrap_or_else(|err| panic!("TempDir {}: {err}", dir.display()));
        TempDir(dir)
    }

    pub(crate) fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
