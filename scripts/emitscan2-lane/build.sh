#!/usr/bin/env bash
set -euo pipefail
cd /home/theo/Code/sandbox/ts-rust
O=target/continuation-r97-goport/after242/emitscan2/lane
mkdir -p "$O/bin"
exec > "$O/build.log" 2>&1
trap 'rc=$?; if ((rc)); then echo "FAIL rc=$rc"; else echo DONE; fi' EXIT
hostname; date -Is; lscpu | grep -E 'Model name|Address sizes'; git rev-parse HEAD HEAD:crates; rustc --version
TS_CARGO_JOBS=32 TS_CARGO_NIGHTLY=0 TS_CARGO_INCREMENTAL=0 RUSTFLAGS='-C force-frame-pointers=yes' scripts/run-cargo-capped.sh build --profile goport -p ts_goport --bin tsgo
cp target/goport/tsgo "$O/bin/tsgo"
git rev-parse HEAD > "$O/bin/COMMIT"
sha256sum "$O/bin/tsgo" > "$O/bin/bins.sha256"
