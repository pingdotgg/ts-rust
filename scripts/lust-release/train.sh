#!/usr/bin/env bash
# train.sh <ts-rust-checkout> <pgo-dir> <pgo-llvm.sh>: the pinned PGO profiles of the lust
# release build of tsc (release C), as .github/workflows/lust-release.yml trains them.
#
# Two steps, so that both builds copy the same fns: P0 is trained as is (copies.profdata);
# then an instrumented build that makes the hot copies P0 chooses (LUST_PGO_COPIES=P0,
# LUST_PGO_COPY_MIN=1000) trains P1 (use.profdata). The release build uses P1 with the copies
# of P0. Each instrumented build is a host (glibc) build with a clang of zig's LLVM major
# (<pgo-llvm.sh> finds one), run on scripts/goport/pgo-train.sh's training set, the same set
# release.yml trains Cargo's PGO on (needs git and npm). <pgo-dir> gets copies.profdata,
# use.profdata and llvm-profdata (a wrapper with its libs), which the release build reads.
set -euo pipefail
SRC=$1 P=$2 PGO_LLVM=$3
T=${TMPDIR:-/tmp}/lust-train; rm -rf "$T"; mkdir -p "$T"
eval "$(bash "$PGO_LLVM")"
export LUST_PGO_CC LUST_PROFDATA
cd "$SRC"
scripts/goport/pgo-train.sh setup "$T/train" > "$T/setup.log" 2>&1 || { tail -n 30 "$T/setup.log"; exit 1; }
train() { # train <out.profdata> <env...>: an instrumented build, the training set, merge
  local out=$1; shift
  env LUST_NO_DAEMON=1 LUST_RELEASE_C=1 LUST_PGO_GEN=1 LUST_CC="$LUST_PGO_CC" LUST_CACHE_DIR="$T/lc" "$@" \
    lust build crates/ts_goport/src/bin/tsgo.rs --manifest-path crates/ts_goport/Cargo.toml --bin tsgo --offline \
    --no-default-features --features noembed --profile goport --target x86_64-unknown-linux-gnu -o "$T/tsc-gen" \
    > "$T/gen.log" 2>&1 || { tail -n 30 "$T/gen.log"; exit 1; }
  rm -rf "$T/prof"
  PGO_PROFILE_DIR="$T/prof" scripts/goport/pgo-train.sh run "$T/train" "$T/tsc-gen" > "$T/run.log" 2>&1 || { tail -n 30 "$T/run.log"; exit 1; }
  "$LUST_PROFDATA" merge -o "$out" "$T/prof"
}
rm -rf "$P.new"; mkdir -p "$P.new/bin" "$P.new/lib"
train "$P.new/copies.profdata"
train "$P.new/use.profdata" LUST_PGO_COPIES="$P.new/copies.profdata" LUST_PGO_COPY_MIN=1000
# llvm-profdata and its libs, so a later run reads the profiles with no LLVM install.
cp "$(readlink -f "$(command -v "$LUST_PROFDATA")")" "$P.new/bin/llvm-profdata"
cp -L "$(dirname "$(readlink -f "$(command -v "$LUST_PROFDATA")")")"/../lib/libLLVM*.so* "$P.new/lib/" 2>/dev/null || true
# shellcheck disable=SC2016 # the wrapper expands these when it runs
printf '#!/bin/sh\nd=$(dirname "$0")\nLD_LIBRARY_PATH="$d/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" exec "$d/bin/llvm-profdata" "$@"\n' > "$P.new/llvm-profdata"
chmod +x "$P.new/llvm-profdata"
"$P.new/llvm-profdata" --version > /dev/null
rm -rf "$P" && mv "$P.new" "$P"
echo "pgo profiles trained: $(du -sh "$P" | cut -f1)"
