# lust: a fast dev check loop

[lust](https://github.com/t3dotgg/lust-lang) is a compiler for a subset of Rust.
It checks and builds much faster than rustc, and keeps a warm daemon so that
an edit is checked again in milliseconds. Every program lust accepts is valid
Rust with the same behavior. When lust does not support some code, it rejects
it with an `L####` error code; it does not guess.

Use lust for the edit and check loop. Release builds stay with Cargo
(`cargo build --release`, `crates/ts_goport/scripts/build-release.sh`).

## Install

lust runs on Linux x86_64. Get `lust-<version>-x86_64-linux.tar.gz` from the
[lust releases](https://github.com/t3dotgg/lust-lang/releases), then:

```sh
tar -xzf lust-<version>-x86_64-linux.tar.gz -C ~/.local/opt
export PATH="$HOME/.local/opt/lust-<version>-x86_64-linux/bin:$PATH"
lust capabilities   # prints what the subset supports
```

Keep the `bin/` and `lib/lust/` directories together: lust finds its tcc
backend and std sources relative to its own executable. Cargo must be on
`PATH` too, and the crates must be in the Cargo registry (run `cargo fetch`
once): lust reads `Cargo.toml`, `Cargo.lock` and the crate sources.

## Check one target

Name the crate root, the manifest and the target, as with Cargo:

```sh
lust check crates/ts_goport/src/lib.rs --manifest-path crates/ts_goport/Cargo.toml --lib
lust check crates/ts_goport/src/bin/tsgo.rs --manifest-path crates/ts_goport/Cargo.toml --bin tsgo
```

`scripts/lust-check.sh` checks every lib, bin and cdylib target of the
workspace, or only the ones you name:

```sh
scripts/lust-check.sh                    # all targets
scripts/lust-check.sh ts_goport ts_goport:tsgo
```

`--json` (or `LUST_FLAGS=--json` for the script) prints one diagnostic per
line. `lust explain L0820` prints the long text for one error code.

## The warm daemon

The first `lust check` in a directory is cold: about 70 s for `ts_goport`
(cold `cargo check` is faster). It also starts a lust daemon in the
background for that directory (log: `.lust-cache/daemon.log`), which
loads the target once more before its first warm answer. Later
checks go to the daemon and are warm: it keeps each target's parsed and
checked program and checks again only what an edit changed. The daemon exits
after 30 idle minutes. `LUST_NO_DAEMON=1` turns it off. Do not commit or
share `.lust-cache/`.

## Dev features

`.lust.toml` at the repo root sets the features of lust dev builds:

```toml
dev-exclude-features = ["jemalloc", "lib-snapshot", "backtrace"]
```

- `jemalloc`: lust has no jemalloc, so dev builds use the system allocator.
- `lib-snapshot`: without the embedded lib snapshots every lib file parses
  and binds live, with the same output. Their compile-time source hashes
  would make each edit to the parser or binder change a const.
- `backtrace`: lust cannot give the same backtrace text as rustc.

Cargo does not read `.lust.toml`, so Cargo builds keep their defaults.

## Dev loop

1. Edit.
2. `scripts/lust-check.sh ts_goport` (or the target you work on). Warm, a
   check after a one-line body edit in `ts_goport` takes about 15 ms;
   `cargo check -p ts_goport --lib` takes about 6 s for the same edit.
3. Before a commit or a PR, run the normal Cargo checks and tests. The CI
   and the release build do not change.

## Current state

With lust <LUST_VERSION> on this tree:

- `lust check`: <CHECK> targets pass.
- `lust build`: <BUILD> bins build.
- Parity: the lust-built bins give the same output as the Cargo-built bins
  on every fixture project of `crates/ts_goport/tests` (<RUNS> runs), the
  integration test binaries pass with them (<TESTS>), and the codegen tools
  give the same files (<TOOLS>).
- Speed (16 vCPU, 12 one-line body edits in `ts_goport`): warm `lust check`
  median 14 ms, `cargo check` median 5,982 ms.

## When lust rejects code

lust rejects code it does not support, never with a wrong answer. If
`cargo check` passes and lust gives an error:

1. Check `lust explain <code>`. Some codes are a known gap of the subset.
2. Open an issue on [lust-lang](https://github.com/t3dotgg/lust-lang/issues)
   with the error line (`--json` output is best), the lust version, the
   ts-rust commit and the target.
3. Until it is fixed, use `cargo check` for that target.

Do not change ts-rust code only to make lust accept it, unless the change is
small and keeps the same behavior.
