# lust: a fast dev check loop

[lust](https://github.com/t3dotgg/lust-lang) is a compiler for a subset of Rust.
It checks and builds much faster than rustc, and keeps a warm daemon so that
an edit is checked again in milliseconds. Every program lust accepts is valid
Rust with the same behavior. When lust does not support some code, it rejects
it with an `L####` error code; it does not guess.

Use lust for the edit and check loop. Published releases stay with Cargo
(`cargo build --release`, `crates/ts_goport/scripts/build-release.sh`); CI also builds a
lust release tsc for linux-x64 on each push to main (see
[Release builds in CI](#release-builds-in-ci-linux-x64)).

## Install

lust runs on Linux x86_64 with glibc 2.38 or newer (Ubuntu 24.04, Debian 13,
Fedora 39 and newer). Use lust 0.2.0 or newer. Get
`lust-0.2.0-x86_64-linux.tar.gz` from the
[lust releases](https://github.com/t3dotgg/lust-lang/releases/tag/v0.2.0), then:

```sh
tar -xzf lust-0.2.0-x86_64-linux.tar.gz -C ~/.local/opt
export PATH="$HOME/.local/opt/lust-0.2.0-x86_64-linux/bin:$PATH"
lust --version
```

The daemon needs about 7 GB of memory for `ts_goport`. A 16 GB machine is
enough.

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

The first `lust check` in a directory is cold: about 12 s for the `ts_goport`
lib (cold `cargo check` with its dependencies: about 24 s). It also starts a
lust daemon in the background for that directory (log:
`.lust-cache/daemon.log`), which loads the target once more (about 15 s)
before its first warm answer. Later
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
   check after a one-line body edit takes 20 to 30 ms. Incremental
   `cargo check -p ts_goport --lib` takes about 1.4 s for an edit in the
   `ts_goport` crate's own files and about 5.7 s for an edit in
   `goport_core`, where most of the code is. An edit to a fn signature is
   usually fast too (median 54 to 109 ms), but some take 4 to 10 s.
3. Before a commit or a PR, run the normal Cargo checks and tests. The CI
   and the release build do not change.

## Current state

With lust 0.2.0 on this tree (16 vCPU, 32 GB Linux box):

- `lust check`: all 18 lib and bin targets pass.
- `lust build`: all 11 bins build. The lust-built
  bins give the same output as the Cargo-built bins on every fixture project
  of `crates/ts_goport/tests` (540 of 540 runs: stdout, stderr, exit code and
  every written file), the 7 integration test binaries pass with them, and
  the codegen tools give the same files (12 of 12 runs). 683 of 683 unit
  tests give the same result.
- Speed (12 one-line body edits in `goport_core` files, checked through
  `ts_goport`): warm `lust check` median 24 ms, incremental `cargo check`
  median 5.7 s. Cold: `lust check` 9.1 s, `cargo check` 20.8 s.

## Release builds in CI (linux-x64)

`.github/workflows/lust-release.yml` builds the linux-x64 release tsc
(`x86_64-unknown-linux-musl`, static) with lust on each push to main, and on
demand (Actions, "lust-release", Run workflow; tick "retrain" to retrain the
PGO profiles on that commit). It publishes nothing: the artifact
`tsc-linux-x64-lust.zst` is the tsc as one zstd file (not zipped; `zstd -d`
checks its stored checksum). `release.yml` and Cargo stay the source of truth
for releases.

The build is the shipped config of lust's release driver: release C (lust
emits C, zig cc compiles it at `-O2`), PGO with pinned profiles, and hot copies
chosen by the profile. It runs on `release.yml`'s linux-x64 runner
(`blacksmith-16vcpu-ubuntu-2404`). A Blacksmith sticky disk keeps lust and
zig, the crate registry, a clone of this repo, the profiles and lust's state:
the front image (the front end of the last build) and the object cache. So a
push recompiles only the C units whose text changed:

- Warm (the usual push): about 11 s of job time for a typical merge into main
  (median of 18 jobs on a 12-merge replay of main's history; the cargo
  linux-x64 job of `release.yml` takes about 17 min).
- Cold (a new disk, or a new `LUST_REF`): about 15 min of job time. lust is
  built from source (about 6 min), the profiles are trained on
  `scripts/goport/pgo-train.sh`'s set (about 6.5 min), then a cold build
  (about 1.5 min).

The release job is one step after the sticky disk: it builds, runs
`release.yml`'s "Check the tsc" smoke, and uploads the artifact while the smoke
runs (`scripts/lust-release/upload.sh`, the calls `upload-artifact@v7` makes
for one unzipped file; it creates the artifact during the build and finalizes
it only when the smoke passes). This takes about 0.6 s off the job against a
separate upload step (TABLE 26 in lust's notes). Those calls are not a
documented API: if `upload.sh` fails for any reason, the job uploads the same
file with `upload-artifact@v7` in a later step, stays green, and logs a
warning.

Checks: the release job runs the smoke. The verify job starts the tsc on Alpine (static) and runs the 216 tsgo fixture
runs against a Cargo tsc of the same commit (`scripts/lust-release/goldens.py`,
built in the `goldens` job). Its run speed is within 1.2x of the Cargo PGO
tsc (wall time on the PGO training set and on a TypeORM check).

`LUST_REF` pins a lust-lang commit. To update lust, change `LUST_REF`: the
next run builds that lust and retrains the profiles. The scripts:
`scripts/lust-release/build.sh` (the build), `scripts/lust-release/train.sh`
(the profiles) and `scripts/lust-release/upload.sh` (the artifact).

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
