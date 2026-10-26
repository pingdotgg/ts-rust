#!/usr/bin/env bash
set -euo pipefail
R=/home/theo/Code/sandbox/ts-rust
cd "$R"
O=$R/target/continuation-r97-goport/after242/emitscan2/lane/prep
mkdir -p "$O"
exec > "$O/prep.log" 2>&1
trap 'rc=$?; if ((rc)); then echo "FAIL rc=$rc"; else echo DONE; fi' EXIT
SKIP_T3=1 BENCH_APPS='vscode typeorm playwright' bash scripts/emitscan2-lane/setup.sh
for p in vscode typeorm playwright; do git -C /tmp/bench.noindex/repos/$p rev-parse HEAD; done
sha256sum /home/theo/.local/bin/tsgo-oracle-3e9236361fa9
