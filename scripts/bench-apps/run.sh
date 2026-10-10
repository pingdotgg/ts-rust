#!/usr/bin/env bash
# Times a full type check of each app with tsc 6, tsc 7, tsc-rs and bun check (hyperfine, 1 warmup,
# RUNS runs, default 5). Before the timing, one run per tool writes its exit code and errors.
# usage: scripts/bench-apps/run.sh <work-dir> [app...]   (after setup.sh; default: every app)
# Output: <work-dir>/results/<app>.json (hyperfine), <app>.errors and <app>.<tool>.diag.
# TOOLS limits a run, for example TOOLS=tsc-rs to time a new tsc-rs only. The hyperfine and errors
# files then get the tools in their name (<app>.tsc-rs.json), and summary.py puts them over the
# full run's times.
# Time on a quiet machine: the numbers are noise while something else uses the CPU.
set -uo pipefail
[[ $# -ge 1 ]] || { sed -n '4p' "$0" >&2; exit 2; }
work=$(cd "$1" && pwd); shift
# app, repo dir, tsconfig
APPS='vscode vscode src/tsconfig.json
sentry sentry tsconfig.json
playwright playwright tsconfig.json
typeorm typeorm packages/typeorm/tsconfig.bench.json
excalidraw excalidraw tsconfig.bench.json
trpc-server trpc packages/server/tsconfig.json'

# Native binaries, without the npm launchers. tsc 6 runs out of memory on VS Code and Sentry with
# Node's default heap.
RS=$work/node_modules/@tsc-rs/darwin-arm64/lib/tsc
GO=$work/node_modules/@typescript/typescript-darwin-arm64/lib/tsc
TS6="node --max-old-space-size=16384 $work/node_modules/ts6/lib/tsc.js"
BUN=$work/bun-darwin-aarch64/bun
names=(tsc6 tsc7 tsc-rs bun)
sel=()
for i in "${!names[@]}"; do [[ " ${TOOLS:-${names[*]}} " == *" ${names[$i]} "* ]] && sel+=("$i"); done
tag=${TOOLS:+.${TOOLS// /+}}
mkdir -p "$work/results"

while read -r app dir cfg; do
  [[ $# == 0 || " $* " == *" $app "* ]] || continue
  flags="-p $cfg --noEmit --incremental false"
  cmds=("$TS6 $flags --pretty false" "$GO $flags --pretty false" "$RS $flags --pretty false"
        "$BUN check $flags --no-pretty --all")
  cd "$work/repos/$dir"
  : >"$work/results/$app$tag.errors"
  for i in "${sel[@]}"; do
    out=$(${cmds[$i]} 2>&1); rc=$?
    grep 'error TS' <<<"$out" | sort >"$work/results/$app.${names[$i]}.diag"
    echo "${names[$i]} rc=$rc errors=$(wc -l <"$work/results/$app.${names[$i]}.diag" | tr -d ' ')" >>"$work/results/$app$tag.errors"
  done
  echo "== $app: $(tr '\n' ' ' <"$work/results/$app$tag.errors")"
  args=()
  for i in "${sel[@]}"; do args+=(-n "${names[$i]}" "${cmds[$i]}"); done
  hyperfine -N -i --warmup 1 --runs "${RUNS:-5}" --export-json "$work/results/$app$tag.json" "${args[@]}"
done <<<"$APPS"
