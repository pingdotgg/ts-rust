#!/usr/bin/env bash
# Times the T3 Code projects without and with the Effect diagnostics (hyperfine, 1 warmup, RUNS
# runs, default 5). Before the timing, one run per tool writes its exit code and diagnostic count.
# usage: scripts/bench-apps/t3code.sh <work-dir> [sha]   (after setup.sh; default sha: the README's)
# T3_PROJECTS and T3_MODES (noeffect, effect) limit a run, for example T3_MODES=noeffect. TOOLS
# limits the tools as in run.sh (TOOLS=tsc-rs).
# Output: <work-dir>/results/t3-<project>-<mode>.json (hyperfine) and .diags, mode noeffect or effect.
# With TOOLS, the files get the tools in their name (t3-<project>-<mode>.tsc-rs.json).
#
# Without Effect: each project config without its plugins, extending a copy of tsconfig.base.json
# without plugins. A child "plugins": [] is not enough: tsc-rs 0.1.0 keeps the base plugins.
# apps/mobile extends the Expo base and has no plugin. With Effect: the project configs as they are, and
#   tsc 6     patched with @effect/language-service 0.87.4 (the JS plugin; its rule set differs)
#   tsc 7     the Effect-patched tsc 7.0.2 from @effect/tsgo (effect-tsgo get-exe-path)
#   tsc-rs    Effect diagnostics built in
#   bun       bun check, then effect-tsgo diagnostics for the Effect diagnostics
set -uo pipefail
[[ $# -ge 1 ]] || { sed -n '4p' "$0" >&2; exit 2; }
work=$(cd "$1" && pwd)
sha=${2:-cd41c4ada0c70cc2eec95ecd7266f3dab010c58c}
t3=$work/repos/t3code
PROJECTS=${T3_PROJECTS:-apps/server apps/web apps/mobile packages/client-runtime packages/shared}
MODES=${T3_MODES:-noeffect effect}

if [[ ! -d $t3 ]]; then
  git init -q "$t3" && git -C "$t3" fetch -q --depth 1 https://github.com/pingdotgg/t3code "$sha" &&
    git -C "$t3" checkout -q FETCH_HEAD && (cd "$t3" && pnpm install --frozen-lockfile --ignore-scripts) >/dev/null
fi
if [[ ! -d $work/ts6effect ]]; then
  mkdir -p "$work/ts6effect" && (cd "$work/ts6effect" && echo '{ "private": true }' >package.json &&
    npm i -q --no-audit --no-fund typescript@6.0.3 @effect/language-service@0.87.4 && npx effect-language-service patch)
fi
# The configs have comments, so TypeScript's own parser reads them.
(cd "$t3" && TS="$work/node_modules/ts6/lib/typescript.js" node -e '
  const fs = require("fs"), ts = require(process.env.TS)
  const strip = (file, to, extendsFrom, extendsTo) => {
    const json = ts.parseConfigFileTextToJson(file, fs.readFileSync(file, "utf8")).config
    if (json.compilerOptions) delete json.compilerOptions.plugins
    if (json.extends === extendsFrom) json.extends = extendsTo
    fs.writeFileSync(to, JSON.stringify(json, null, 2))
  }
  strip("tsconfig.base.json", "tsconfig.base.noeffect.json")
  for (const p of ["apps/server", "apps/web", "apps/mobile", "packages/client-runtime", "packages/shared"])
    strip(`${p}/tsconfig.json`, `${p}/tsconfig.noeffect.json`, "../../tsconfig.base.json", "../../tsconfig.base.noeffect.json")
')

RS=$work/node_modules/@tsc-rs/darwin-arm64/lib/tsc
GO=$work/node_modules/@typescript/typescript-darwin-arm64/lib/tsc
GOE=$(cd "$t3" && node_modules/.bin/effect-tsgo get-exe-path | tail -1)
[[ -x $GOE ]] || { echo "no Effect-patched tsc: $GOE" >&2; exit 1; }
TS6="node --max-old-space-size=16384 $work/node_modules/ts6/lib/tsc.js"
TS6E="node --max-old-space-size=16384 $work/ts6effect/node_modules/typescript/lib/tsc.js"
BUN=$work/bun-darwin-aarch64/bun
ETSGO=$t3/node_modules/.bin/effect-tsgo
# --composite false: apps/web is composite, which needs incremental.
F="--noEmit --incremental false --composite false"
names=(tsc6 tsc7 tsc-rs bun)
sel=()
for i in "${!names[@]}"; do [[ " ${TOOLS:-${names[*]}} " == *" ${names[$i]} "* ]] && sel+=("$i"); done
tag=${TOOLS:+.${TOOLS// /+}}
mkdir -p "$work/results"

for p in $PROJECTS; do
  cd "$t3/$p"
  slug=${p//\//-}
  for mode in $MODES; do
    if [[ $mode == noeffect ]]; then
      c=tsconfig.noeffect.json
      cmds=("$TS6 -p $c $F --pretty false" "$GO -p $c $F --pretty false" "$RS -p $c $F --pretty false"
            "$BUN check -p $c $F --no-pretty --all")
    else
      c=tsconfig.json
      cmds=("$TS6E -p $c $F --pretty false" "$GOE -p $c $F --pretty false" "$RS -p $c $F --pretty false"
            "sh -c '$BUN check -p $c $F --no-pretty --all; $ETSGO diagnostics --project $PWD/$c --format text'")
    fi
    out=$work/results/t3-$slug-$mode$tag
    : >"$out.diags"
    for i in "${sel[@]}"; do
      o=$(eval "${cmds[$i]}" 2>&1); rc=$?
      n=$(grep -cE '(error|warning|message|suggestion) (TS[0-9]+|effect\()' <<<"$o")
      echo "${names[$i]} rc=$rc diags=$n" >>"$out.diags"
    done
    echo "== $p $mode: $(tr '\n' ' ' <"$out.diags")"
    args=()
    for i in "${sel[@]}"; do args+=(-n "${names[$i]}" "${cmds[$i]}"); done
    hyperfine -N -i --warmup 1 --runs "${RUNS:-5}" --export-json "$out.json" "${args[@]}" >/dev/null
  done
done
echo DONE
