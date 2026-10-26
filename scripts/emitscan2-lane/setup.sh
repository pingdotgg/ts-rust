#!/usr/bin/env bash
# perfscan7 (copy of perfscan6): on the testbox, the app bench in /tmp/bench.noindex: scripts/bench-apps/setup.sh (the README apps at
# their commits, npm typescript 7.1.0-dev.20261008.1, the newest Bun canary), then T3 Code at cd41c4ad without Effect
# (the fastmode1 setup). Prints the Bun revision. usage: bg.sh <log> bash setup.sh
set -euo pipefail
R=/home/theo/Code/sandbox/ts-rust
W=/tmp/bench.noindex
export COREPACK_ENABLE_DOWNLOAD_PROMPT=0 npm_config_update_notifier=false
echo "start $(date -Is)"
npm install -g pnpm@11.10.0 corepack@0.34.5 --force > /dev/null
BUN_SHA256= bash $R/scripts/bench-apps/setup.sh $W
if [[ ${SKIP_T3:-0} != 1 ]]; then # SKIP_T3=1 skips T3 Code (BENCH_APPS picks the README apps)
sha=cd41c4ada0c70cc2eec95ecd7266f3dab010c58c
t3=$W/repos/t3code
if [[ ! -d $t3 ]]; then
  git init -q "$t3" && git -C "$t3" fetch -q --depth 1 https://github.com/pingdotgg/t3code "$sha" &&
    git -C "$t3" checkout -q FETCH_HEAD && (cd "$t3" && pnpm install --frozen-lockfile --ignore-scripts) > $W/logs/install-t3code.log 2>&1
fi
(cd "$t3" && TS="$W/node_modules/ts6/lib/typescript.js" node -e '
const fs = require("fs"), ts = require(process.env.TS)
const strip = (file, to, extendsFrom, extendsTo) => {
  const json = ts.parseConfigFileTextToJson(file, fs.readFileSync(file, "utf8")).config
  if (json.compilerOptions) delete json.compilerOptions.plugins
  if (json.extends === extendsFrom) json.extends = extendsTo
  fs.writeFileSync(to, JSON.stringify(json, null, 2))
}
strip("tsconfig.base.json", "tsconfig.base.noeffect.json")
strip("apps/server/tsconfig.json", "apps/server/tsconfig.noeffect.json", "../../tsconfig.base.json", "../../tsconfig.base.noeffect.json")
')
fi
echo "bun $($W/bun-linux-x64/bun --revision)"
cat $W/logs/bun-archive.sha256
echo "tsc7 $($W/node_modules/@typescript/typescript-linux-x64/lib/tsc --version)"
for r in $W/repos/*; do echo "$(basename $r) $(git -C $r rev-parse HEAD)"; done
echo "end $(date -Is)"
