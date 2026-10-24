#!/bin/bash
# thpguard3: wall and sys time of valibot and the 4 gate projects (--noEmit), THP guard as it is against
# GOPORT_THP_GUARD=force, back to back and after idle gaps. Read-only on inputs.
# usage: measure.sh <outdir> [gap s] [rounds] [idle s] [configs]
#   configs (default "unset force forcel go"):
#     unset  ours, guard as it is, GOPORT_LAUNCH=0 (wait4 sees the work)
#     force  ours, GOPORT_THP_GUARD=force, GOPORT_LAUNCH=0
#     forcel ours, GOPORT_THP_GUARD=force, default launch (the worker of a 4 KiB run; wall is what a caller
#            sees; user and sys are the launcher's only)
#     go     tsgo-oracle at pin 3e9236361fa9
# Phases: b2b (per project and config 6 runs back to back; drop run 1 in the tables), gap (rounds x projects
# x configs, each run after a sleep of gap s), idle (valibot and effect, unset and force, each after idle s).
# Env: PHASES (default "warm b2b gap idle"), IDLE_PROJECTS (default "valibot effect"), IDLE_CFGS (default
# "unset force"), IDLE_ROUNDS (default 1; each round runs every idle project and config once).
# runs.tsv: one row per run. The columns thp_alloc, thp_fallback and compact_stall hold, in this order, the
# deltas of pgfault (system wide), compact_stall and thp_fault_alloc (/proc/vmstat order; table.py maps them). err/: stderr of every run (guard decisions). out/: stdout md5 per run.
set -u
. "$(dirname "$0")/projects.sh"
out=$1; gap=${2:-10}; rounds=${3:-5}; idle=${4:-90}; cfgs=${5:-"unset force forcel go"}
mkdir -p "$out/err"
"$(dirname "$0")/sysinfo.sh" > "$out/sysinfo.txt" 2>&1
vm() { awk '$1 ~ /^(thp_fault_alloc|thp_fault_fallback|compact_stall|pgfault)$/ {printf "%s ", $2}' /proc/vmstat; }
free2m() { awk '{ for (i = 14; i <= NF; i++) t += $i * (2 ^ (i - 5)) * 4 } END { printf "%.0f", t / 1024 }' /proc/buddyinfo; }
printf 'phase\tround\tproject\tconfig\tgap\twall\tuser\tsys\tminflt\trss_kb\trc\tfree2m_mib\tmemfree_mib\tthp_alloc\tthp_fallback\tcompact_stall\tmd5\tguard\n' > "$out/runs.tsv"
n=0
run() { # phase round project config gap
  local phase=$1 rnd=$2 pname=$3 cfg=$4 g=$5 dir args bin envs p
  for p in "${PROJECTS[@]}"; do IFS='|' read -r name dir args <<< "$p"; [[ $name == "$pname" ]] && break; done
  bin=$OURS; envs=(GOPORT_LAUNCH=0 GOPORT_THP_GUARD_DEBUG=1)
  case $cfg in
    force) envs+=(GOPORT_THP_GUARD=force) ;;
    forcel) envs=(GOPORT_THP_GUARD=force GOPORT_THP_GUARD_DEBUG=1) ;;
    go) bin=$GO; envs=() ;;
  esac
  [[ $g != 0 ]] && sleep "$g"
  n=$((n + 1))
  local f2 mf pre post tag=$out/err/$n.$phase.$pname.$cfg
  f2=$(free2m); mf=$(awk '/MemFree/ {print int($2/1024)}' /proc/meminfo); read -r -a pre <<< "$(vm)"
  (cd "$dir" && env -u GOPORT_THP_GUARD -u GOPORT_LAUNCH "${envs[@]}" /usr/bin/time -f "%e %U %S %R %M" -o "$tag.time" \
     "$bin" $args > "$tag.out" 2> "$tag.err"; echo $? > "$tag.rc")
  read -r -a post <<< "$(vm)"
  local t md5 guard
  t=$(tail -1 "$tag.time"); md5=$(md5sum < "$tag.out" | cut -c1-12); rm -f "$tag.out"
  guard=$(grep -h 'thp_guard:' "$tag.err" | sed 's/goport thp_guard: //' | tr '\n' ';' | tr '\t' ' ')
  read -r wall user sys minflt rss <<< "$t"
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$phase" "$rnd" "$pname" "$cfg" "$g" \
    "$wall" "$user" "$sys" "$minflt" "$rss" "$(cat "$tag.rc")" "$f2" "$mf" $((post[0] - pre[0])) $((post[1] - pre[1])) \
    $((post[2] - pre[2])) "$md5" "$guard" >> "$out/runs.tsv"
}
names=(); for p in "${PROJECTS[@]}"; do names+=("${p%%|*}"); done
phases=" ${PHASES:-warm b2b gap idle} "
# One untimed read of each project (page cache).
[[ $phases == *" warm "* ]] && for pn in "${names[@]}"; do run warm 0 "$pn" force 0; done
[[ $phases == *" b2b "* ]] && for pn in "${names[@]}"; do for c in $cfgs; do for i in 1 2 3 4 5 6; do run b2b "$i" "$pn" "$c" 0; done; done; done
[[ $phases == *" gap "* ]] && for r in $(seq 1 "$rounds"); do for pn in "${names[@]}"; do for c in $cfgs; do run gap "$r" "$pn" "$c" "$gap"; done; done; done
if [[ $idle != 0 && $phases == *" idle "* ]]; then
  for r in $(seq 1 "${IDLE_ROUNDS:-1}"); do
    for pn in ${IDLE_PROJECTS:-valibot effect}; do for c in ${IDLE_CFGS:-unset force}; do run idle "$r" "$pn" "$c" "$idle"; done; done
  done
fi
"$(dirname "$0")/sysinfo.sh" > "$out/sysinfo-end.txt" 2>&1
echo "runs $n"
