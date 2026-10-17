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

# One record per target, 4 NUL-terminated fields: <name> <src> <manifest> <bin name, empty for
# a lib>. NUL keeps paths with spaces whole. A requested name that is no target is an error.
list=$(mktemp) || exit 2
trap 'rm -f "$list"' EXIT
cargo metadata --no-deps --format-version 1 --offline | python3 -c '
import json, sys
want = sys.argv[1:]
found = set()
out = sys.stdout.buffer
for p in json.load(sys.stdin)["packages"]:
    for t in p["targets"]:
        kind = t["kind"][0]
        if kind == "bin":
            name, bin_name = p["name"] + ":" + t["name"], t["name"]
        elif kind in ("lib", "cdylib"):
            name, bin_name = p["name"], ""
        else:
            continue
        if want and name not in want:
            continue
        found.add(name)
        for field in (name, t["src_path"], p["manifest_path"], bin_name):
            out.write(field.encode() + b"\0")
missing = [w for w in want if w not in found]
if missing:
    sys.exit("lust-check: no such target: " + " ".join(missing) + " (use <package> or <package>:<bin>)")
' "$@" > "$list" || exit 2

fail=0
while IFS= read -r -d '' name && IFS= read -r -d '' src && IFS= read -r -d '' manifest &&
  IFS= read -r -d '' bin; do
  if [ -n "$bin" ]; then sel=(--bin "$bin"); else sel=(--lib); fi
  # shellcheck disable=SC2086 # LUST_FLAGS is a word list
  if "$lust" check "$src" --manifest-path "$manifest" "${sel[@]}" ${LUST_FLAGS:-} </dev/null; then
    echo "lust-check: $name ok"
  else
    echo "lust-check: $name FAILED"
    fail=1
  fi
done < "$list"
exit $fail
