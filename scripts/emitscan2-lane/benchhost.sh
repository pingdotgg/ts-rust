#!/usr/bin/env bash
set -euo pipefail
cd /home/theo/Code/sandbox/ts-rust/target/worktrees/goport-emitscan2
O=/home/theo/Code/sandbox/ts-rust/target/continuation-r97-goport/after242/emitscan2/lane
exec > "$O/benchhost.log" 2>&1
trap 'rc=$?; if ((rc)); then echo "FAIL rc=$rc"; else echo DONE; fi' EXIT
scripts/goport/testbox.sh push /home/theo/Code/sandbox/ts-rust/target/continuation-r97-goport/after241/emitscan1/measure/phases/runs.tsv /home/theo/Code/sandbox/ts-rust/target/continuation-r97-goport/perfscan7/cfg/effnoplug
rc=0
scripts/goport/testbox.sh job bash scripts/emitscan2-lane/bench.sh || rc=$?
scripts/goport/testbox.sh get target/continuation-r97-goport/after242/emitscan2/lane/bench.log "$O/bench.log"
scripts/goport/testbox.sh get target/continuation-r97-goport/after242/emitscan2/lane/counts.tar.gz "$O/counts.tar.gz" || true
if [[ -f "$O/counts.tar.gz" ]]; then
  mkdir "$O/counts-result"
  tar -xzf "$O/counts.tar.gz" -C "$O/counts-result"
fi
((rc == 0)) || exit "$rc"
scripts/goport/testbox.sh perf --mode st --rounds 5 emitscan2-st-noemit "$O/../measure/bin/tsgo" "$O/bin/tsgo" -- --noEmit
