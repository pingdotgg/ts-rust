//! Issue #112: with the Effect plugin, a statement that calls a possibly
//! undefined function got a TS18048 or TS2532 next to the TS2722 of `tsc`.
//! The `floatingEffect` rule asked the checker for the statement's type
//! after the check, and the quick type of a call reports them
//! (`check_non_null_expression`). Effect-TS/tsgo#860 reads the types the check
//! recorded first. Each test runs `tsgo -p` in a new scratch dir and expects
//! the output of `tsc` (`typescript@7.1.0-beta`). A passing test deletes its
//! dir.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

/// Writes `files` to a new scratch dir, runs
/// `tsgo -p tsconfig.json --pretty false` there and returns the output
/// (stdout, then stderr) and the dir.
fn tsc(name: &str, files: &[(&str, &str)]) -> (String, PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "tsgo_effect_type_query_{}_{name}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    for (file, text) in files {
        fs::write(dir.join(file), text).unwrap();
    }
    let out = Command::new(env!("CARGO_BIN_EXE_tsgo"))
        .args(["-p", "tsconfig.json", "--pretty", "false"])
        .current_dir(&dir)
        .output()
        .unwrap();
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    (text, dir)
}

fn config(plugin: &str) -> String {
    format!(
        r#"{{
  "compilerOptions": {{
    "strict": true,
    "noUncheckedIndexedAccess": true,
    "noEmit": true,
    "types": [],
    "plugins": [{plugin}]
  }},
  "files": ["a.ts"]
}}"#
    )
}

/// The issue's repro, an awaited call, and an initializer, where `tsc` itself
/// reports TS18048 next to TS2722 (the check takes the quick type there).
const A_TS: &str = r"declare const f: (() => void) | undefined;
f();

declare const fs: Array<() => void>;
fs[0]();

declare const g: (() => Promise<void>) | undefined;
export async function h() {
  await g();
}

const r = f();
void r;

export {};
";

/// `tsc` output for `A_TS`.
const A_TS_TSC: &str = "\
a.ts(2,1): error TS2722: Cannot invoke an object which is possibly 'undefined'.
a.ts(5,1): error TS2722: Cannot invoke an object which is possibly 'undefined'.
a.ts(9,9): error TS2722: Cannot invoke an object which is possibly 'undefined'.
a.ts(12,11): error TS2722: Cannot invoke an object which is possibly 'undefined'.
a.ts(12,11): error TS18048: 'f' is possibly 'undefined'.
";

fn check(name: &str, plugin: &str) {
    let (output, dir) = tsc(name, &[("tsconfig.json", &config(plugin)), ("a.ts", A_TS)]);
    assert_eq!(output, A_TS_TSC, "case dir {}", dir.display());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn floating_effect_type_query_adds_no_diagnostics() {
    check("plugin", r#"{ "name": "@effect/language-service" }"#);
}

#[test]
fn plain_output_without_plugin() {
    check("plain", "");
}
