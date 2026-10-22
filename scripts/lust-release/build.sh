#!/usr/bin/env bash
# build.sh <ts-rust-checkout> <out-dir>: the lust release build of tsc for linux-x64
# (x86_64-unknown-linux-musl, static, release C through zig cc), as .github/workflows/lust-release.yml
# runs it. Writes <out-dir>/tsc and <out-dir>/build.log.
#
# Environment:
#   STATE_DIR   lust's front image (LUST_FRONT_STATE): a warm build restores the front end of
#               the last build from it, and saves the new one in the background
#   CACHE_DIR   lust's object cache (LUST_CACHE_DIR): a warm build compiles only the C units
#               whose text changed
#   LUST_PGO_USE, LUST_PGO_COPIES, LUST_PGO_COPY_MIN, LUST_PROFDATA
#               the pinned PGO profiles (see the workflow's training step); without them the
#               build is release C without PGO
#   LUST        the lust command (default: lust)
# Prints one line: the build time and the units lust compiled.
set -uo pipefail
SRC=$1 OUT=$2
LUST=${LUST:-lust}
: "${STATE_DIR:?}" "${CACHE_DIR:?}"
mkdir -p "$OUT" "$STATE_DIR" "$CACHE_DIR"
t0=$(date +%s.%N)
LUST_NO_DAEMON=1 LUST_RELEASE_C=1 LUST_FRONT_STATE="$STATE_DIR" LUST_FRONT_SAVE=background LUST_CACHE_DIR="$CACHE_DIR" \
  "$LUST" build "$SRC/crates/ts_goport/src/bin/tsgo.rs" \
  --manifest-path "$SRC/crates/ts_goport/Cargo.toml" --bin tsgo --offline \
  --no-default-features --features noembed --profile goport \
  --target x86_64-unknown-linux-musl -o "$OUT/tsc" --timings > "$OUT/build.log" 2>&1
rc=$?
t1=$(date +%s.%N)
[ $rc = 0 ] || { tail -n 40 "$OUT/build.log"; echo "lust build failed (rc=$rc)"; exit $rc; }
units=$(grep -oE "[0-9]+ units \([0-9]+ from cache" "$OUT/build.log" | head -1 | awk '{gsub(/\(/, "", $3); printf "%d of %d units compiled", $1 - $3, $1}')
echo "lust build $(awk -v a="$t0" -v b="$t1" 'BEGIN{printf "%.1f", b-a}') s, ${units:-? units}"
