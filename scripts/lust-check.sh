#!/usr/bin/env bash
# scripts/lust-check.sh [target...]: `lust check` of every lib, bin and cdylib target of the
# workspace (docs/lust.md). A target is a package name (its lib) or <package>:<bin>.
# With no targets it checks all of them. Exit 0 when every check passes.
#
# The first run is cold and starts a lust daemon for this directory; later runs are warm.
# LUST overrides the lust binary (default: lust on PATH). Extra lust flags go in LUST_FLAGS
# (for example LUST_FLAGS=--json).
set -uo pipefail
cd "$(dirname "$0")/.." || exit 2
lust=${LUST:-lust}
command -v "$lust" >/dev/null || { echo "lust-check: no lust on PATH (see docs/lust.md)" >&2; exit 2; }

# One line per target: <name> <src> <manifest> <--lib | --bin NAME>
targets=$(cargo metadata --no-deps --format-version 1 --offline | python3 -c '
import json, sys
for p in json.load(sys.stdin)["packages"]:
    for t in p["targets"]:
        kind = t["kind"][0]
        if kind == "bin":
            print(p["name"] + ":" + t["name"], t["src_path"], p["manifest_path"], "--bin", t["name"])
        elif kind in ("lib", "cdylib"):
            print(p["name"], t["src_path"], p["manifest_path"], "--lib")
') || exit 2

want=" $* "
fail=0
while read -r name src manifest sel; do
  [ $# -eq 0 ] || [[ $want == *" $name "* ]] || continue
  # shellcheck disable=SC2086 # sel and LUST_FLAGS are word lists
  if "$lust" check "$src" --manifest-path "$manifest" $sel ${LUST_FLAGS:-}; then
    echo "lust-check: $name ok"
  else
    echo "lust-check: $name FAILED"
    fail=1
  fi
done <<< "$targets"
exit $fail
