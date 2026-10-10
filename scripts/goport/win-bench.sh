#!/usr/bin/env bash
# TEMPORARY (lane win1): times tsc builds on Windows (Git Bash) on projects of the pgo-train.sh set.
# usage: win-bench.sh <train-dir> <runs> <name>=<tsc.exe>...
# Each cell (project, check or emit) runs every tsc <runs> times, the order of the tscs alternating
# each round, after one warm-up run each. It prints real, user and sys seconds of each run and the
# median real time of each tsc per cell. Build info and emit go to a new temp dir for every run.
set -euo pipefail
train=$1 runs=$2
shift 2
names=() exes=()
for a in "$@"; do names+=("${a%%=*}"); exes+=("${a#*=}"); done
tmp=$(cygpath -m "$(mktemp -d)")
TIMEFORMAT='%R %U %S'
declare -A cfg=(
  [query]=packages/query-core/tsconfig.prod.json
  [hono]=tsconfig.build.json
  [zod]=packages/zod/tsconfig.json
  [effect-noplugin]=packages/effect/tsconfig.json
)
one() { # one <exe> <project> <mode> -> "real user sys rc"
  local exe=$1 p=$2 mode=$3 args
  rm -rf "$tmp/o"
  mkdir -p "$tmp/o"
  args=(-p "${cfg[$p]}" --pretty false --tsBuildInfoFile "$tmp/o/b.tsbuildinfo")
  if [[ $mode == check ]]; then args+=(--noEmit); else args+=(--outDir "$tmp/o/emit"); fi
  { time (cd "$train/projects/$p" && MSYS_NO_PATHCONV=1 "$exe" "${args[@]}" > /dev/null 2>&1; echo $? > "$tmp/rc"); } 2> "$tmp/time"
  echo "$(cat "$tmp/time") rc=$(cat "$tmp/rc")"
}
for p in query hono zod effect-noplugin; do
  for mode in check emit; do
    for i in "${!exes[@]}"; do one "${exes[$i]}" "$p" "$mode" > /dev/null || true; done
    declare -A reals=()
    for ((r = 0; r < runs; r++)); do
      order=("${!exes[@]}")
      ((r % 2 == 0)) || order=($(printf '%s\n' "${!exes[@]}" | sort -rn))
      for i in "${order[@]}"; do
        res=$(one "${exes[$i]}" "$p" "$mode")
        echo "run $p $mode ${names[$i]} $r: $res"
        reals[$i]+="${res%% *} "
      done
    done
    line="median $p $mode:"
    for i in "${!exes[@]}"; do
      m=$(printf '%s\n' ${reals[$i]} | sort -n | sed -n "$(((runs + 1) / 2))p")
      line+=" ${names[$i]}=${m}s"
    done
    echo "$line"
    unset reals
  done
done
