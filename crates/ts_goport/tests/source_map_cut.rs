//! A source map position inside a char: sweepN2 corpus case 11133
//! (`parserSkippedTokens16.ts`). A skipped token of a parse error ends inside
//! the 2-byte `\u{AC}` on line 3, and the source map asks for the column of
//! that byte. Go slices the bytes there and counts each byte of the cut char
//! as one UTF-16 unit (`printer/utilities.go:912` at pin N). The port
//! panicked before (exit 70, "byte index 58 is not a char boundary").
//!
//! Each case copies `fixtures/source_map_cut/case` to a new directory under
//! the system temp dir, runs `tsgo -p tsconfig.json --outDir ../out --pretty
//! false --noEmit false --<option> true` in the copy, and compares the exit
//! code, stdout and output files with `go/<option>`. A passing case deletes
//! its directory.
//!
//! The Go output is from the pin N' oracle (fed0bf24149f, sha256
//! 6768987d6299), with the same layout and args, from the repository root.
//! It differs from pin N (673a5f17d713) only in the source maps, which no
//! longer have `"sourceRoot":""` (ts#64544):
//!
//! ```sh
//! cp -r crates/ts_goport/tests/fixtures/source_map_cut/case <dir>/case && cd <dir>/case
//! GOPORT_PIN=fed0bf24149f scripts/upstream/pin.py exec -- ~/.local/bin/tsgo-oracle \
//!   -p tsconfig.json --outDir ../out --pretty false --noEmit false --<option> true > ../stdout
//! ```
//!
//! Go exits 2 (6 syntax errors) and writes the outputs.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/source_map_cut");

#[test]
fn cut_char_source_map_matches_go() {
    let fixture = Path::new(FIXTURE);
    for option in ["sourceMap", "inlineSourceMap"] {
        let root = scratch_dir();
        let case = root.join("case");
        fs::create_dir(&case).expect("create case dir");
        for name in ["parserSkippedTokens16.ts", "tsconfig.json"] {
            fs::copy(fixture.join("case").join(name), case.join(name)).expect("copy case file");
        }
        let output = Command::new(env!("CARGO_BIN_EXE_tsgo"))
            .current_dir(&case)
            .args(["-p", "tsconfig.json", "--outDir", "../out"])
            .args(["--pretty", "false", "--noEmit", "false"])
            .arg(format!("--{option}"))
            .arg("true")
            .output()
            .expect("run tsgo");
        let go = fixture.join("go").join(option);
        assert_eq!(
            output.status.code(),
            Some(2),
            "{option}: stderr {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            read(&go.join("stdout")),
            "{option}: stdout"
        );
        assert_eq!(
            files(&root.join("out")),
            files(&go.join("out")),
            "{option}: output files"
        );
        fs::remove_dir_all(&root).expect("remove scratch dir");
    }
}

/// The text of each file in `dir`, by name.
fn files(dir: &Path) -> BTreeMap<String, String> {
    fs::read_dir(dir)
        .unwrap_or_else(|error| panic!("read {}: {error}", dir.display()))
        .map(|entry| {
            let path = entry.expect("dir entry").path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            (name, read(&path))
        })
        .collect()
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

fn scratch_dir() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after 1970")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "goport-source-map-cut-{}-{nanos}",
        std::process::id()
    ));
    fs::create_dir(&dir).unwrap_or_else(|error| panic!("create {}: {error}", dir.display()));
    dir
}
