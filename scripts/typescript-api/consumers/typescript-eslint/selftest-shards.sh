#!/usr/bin/env bash
# Harness self-test: eslint-plugin in 4 shards with one package directory, merged, then compared with a full run.
# usage: selftest-shards.sh <typescript-package-dir|stock> <runs-dir> <full-run-dir>
#        selftest-shards.sh help
# Writes <runs-dir>/<label>-plugin-shard{1..4}, <label>-plugin-merged, <label>-plugin-merged.compare.md and
# <runs-dir>/<label>-shards.log ending with DONE or FAIL rc=N. The compare must show 0 lost, absent and unrun.
set -euo pipefail
case "${1:-}" in "" | help | -h | --help) sed -n '2,/^set -euo/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'; exit 0 ;; esac
{
pkg="$1"
runs="$2"
full="$3"
label="$(basename "$pkg")"
[ "$pkg" = stock ] && label=stock
harness="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)/typescript-eslint.sh"
log="$runs/$label-shards.log"
mkdir -p "$runs"
rc=0
{
    for i in 1 2 3 4; do
        "$harness" run "$pkg" "$runs/$label-plugin-shard$i" --packages eslint-plugin --shard "$i/4"
    done
    "$harness" merge "$runs/$label-plugin-merged" "$runs/$label-plugin-shard1" "$runs/$label-plugin-shard2" \
        "$runs/$label-plugin-shard3" "$runs/$label-plugin-shard4"
    "$harness" compare "$full" "$runs/$label-plugin-merged" >"$runs/$label-plugin-merged.compare.md"
} >"$log" 2>&1 || rc=$?
if [ "$rc" = 0 ]; then echo DONE; else echo "FAIL rc=$rc"; fi >>"$log"
exit "$rc"
}
