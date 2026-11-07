//! Issue #52: with the Effect plugin and `exactOptionalPropertyTypes`, `delete`
//! of a property from an index signature got a false TS2790. Effect-TS/tsgo#828
//! fixes it in `TypeParser.GetSymbolAtLocation`. Each test checks a fixture of
//! `tests/effect_fixtures`, which `scripts/effect/focused-fixtures.mjs` also
//! runs against the reference. These two need no packages, so they run here too.

use std::path::Path;
use std::process::Command;

/// Runs `tsgo -p tsconfig.json --pretty false` in the fixture dir (each fixture
/// has `noEmit`, so nothing is written) and returns success and the stdout lines.
fn tsc(fixture: &str) -> (bool, Vec<String>) {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/effect_fixtures")
        .join(fixture);
    let out = Command::new(env!("CARGO_BIN_EXE_tsgo"))
        .args(["-p", "tsconfig.json", "--pretty", "false"])
        .current_dir(dir)
        .output()
        .unwrap();
    let lines = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::to_string)
        .collect();
    (out.status.success(), lines)
}

#[test]
fn index_signature_delete_is_clean() {
    let (success, lines) = tsc("delete-index-signature");
    assert_eq!(lines, Vec::<String>::new());
    assert!(success);
}

#[test]
fn required_and_readonly_delete_still_report() {
    let optional = "error TS2790: The operand of a 'delete' operator must be optional.";
    let readonly =
        "error TS2704: The operand of a 'delete' operator cannot be a read-only property.";
    let (success, lines) = tsc("delete-required-property");
    assert_eq!(
        lines,
        [
            format!("src/index.ts(2,8): {optional}"),
            format!("src/index.ts(3,9): {optional}"),
            format!("src/index.ts(4,8): {optional}"),
            format!("src/index.ts(8,8): {readonly}"),
        ]
    );
    assert!(!success);
}
