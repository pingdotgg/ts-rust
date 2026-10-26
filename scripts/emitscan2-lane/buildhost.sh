#!/usr/bin/env bash
set -euo pipefail
cd /home/theo/Code/sandbox/ts-rust/target/worktrees/goport-emitscan2
O=/home/theo/Code/sandbox/ts-rust/target/continuation-r97-goport/after242/emitscan2/lane
exec > "$O/buildhost.log" 2>&1
trap 'rc=$?; if ((rc)); then echo "FAIL rc=$rc"; else echo DONE; fi' EXIT
scripts/goport/testbox.sh push /home/theo/Code/sandbox/ts-rust/target/continuation-r97-goport/after242/emitscan2/measure/bin
scripts/goport/testbox.sh job bash scripts/emitscan2-lane/build.sh
scripts/goport/testbox.sh get target/continuation-r97-goport/after242/emitscan2/lane/bin/ "$O/bin/"
scripts/goport/testbox.sh get target/continuation-r97-goport/after242/emitscan2/lane/build.log "$O/build.log"
