#!/usr/bin/env bash
set -euo pipefail
cd /home/theo/Code/sandbox/ts-rust
O=/home/theo/Code/sandbox/ts-rust/target/continuation-r97-goport/after242/emitscan2/lane
exec > "$O/bench.log" 2>&1
trap 'rc=$?; if [[ -d "$O/counts" ]]; then tar -czf "$O/counts.tar.gz" --exclude=counts/scratch --exclude=counts-noemit/scratch -C "$O" counts $(test ! -d "$O/counts-noemit" || echo counts-noemit); fi; if ((rc)); then echo "FAIL rc=$rc"; else echo DONE; fi' EXIT
hostname; date -Is; git rev-parse HEAD HEAD:crates
if [[ ! -f /tmp/bench.noindex/repos/vscode/node_modules/typescript/package.json || ! -f /tmp/bench.noindex/repos/typeorm/packages/typeorm/tsconfig.bench.json ]]; then
  bash scripts/emitscan2-lane/prep.sh
fi
if [[ ! -d /tmp/bench.noindex/repos/sentry/node_modules || ! -d /tmp/bench.noindex/repos/excalidraw/node_modules || ! -f /tmp/bench.noindex/repos/t3code/apps/server/tsconfig.noeffect.json ]]; then
  BENCH_APPS='sentry excalidraw' bash scripts/emitscan2-lane/setup.sh > "$O/setup-extra.log" 2>&1
fi
for p in vscode typeorm sentry excalidraw t3code; do git -C "/tmp/bench.noindex/repos/$p" rev-parse HEAD; done
python3 -I scripts/emitscan2-lane/counts.py "$O/counts" "$O/../measure/bin/tsgo" "$O/bin/tsgo" /home/theo/Code/sandbox/ts-rust/target/continuation-r97-goport/after241/emitscan1/measure/phases/runs.tsv vscode,typeorm,query,hono,zod,effect,sentry,excalidraw,t3server 5
python3 -I scripts/emitscan2-lane/counts.py "$O/counts-noemit" "$O/../measure/bin/tsgo" "$O/bin/tsgo" /home/theo/Code/sandbox/ts-rust/target/continuation-r97-goport/after241/emitscan1/measure/phases/runs.tsv vscode,typeorm,query,hono,zod,effect,sentry,excalidraw,t3server 5 --noEmit
tar -czf "$O/counts.tar.gz" --exclude=counts/scratch --exclude=counts-noemit/scratch -C "$O" counts $(test ! -d "$O/counts-noemit" || echo counts-noemit)
