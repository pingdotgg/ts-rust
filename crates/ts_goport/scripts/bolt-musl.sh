#!/usr/bin/env bash
# BOLT of the static musl linux-x64 tsc (release.yml). BOLT puts the hot blocks of each function
# together, moves the cold blocks out and orders the functions. The program does not change.
# The profile comes from a counting copy (llvm-bolt -instrument) that runs the training set of
# scripts/goport/pgo-train.sh: the release runner has no perf branch sampling.
#
# Usage: bolt-musl.sh <tsc> <train-dir> <out-dir>
#   tsc        the PGO tsc, linked with --emit-relocs (build-pgo.sh PGO_EMIT_RELOCS=1)
#   train-dir  a pgo-train.sh setup dir
#   out-dir    new dir; the BOLT tsc is <out-dir>/bin/tsc (with the lib files of a noembed tsc),
#              the logs and counts stay in <out-dir>
#   LLVM_BIN   dir of llvm-bolt, merge-fdata, llvm-objcopy and llvm-dwarfdump
#              (default /usr/lib/llvm-23/bin)
#
# Static bins differ from build-release.sh's dynamic ones in three ways (relmusl1):
#   - A static bin runs no BOLT code at exit, so the counting copy writes its counts from a
#     watcher process every second (-instrumentation-sleep-time=1), each time all of them
#     (-instrumentation-no-counters-clear). Some watchers spin after their last write and block
#     every signal but SIGKILL: the script kills them.
#   - No -split-eh. It moves landing pads to the cold part and gives their LSDA a start address
#     (LPStart). The musl target unwinds with LLVM libunwind, which aborts on such an LSDA
#     ("lsda at ... does not start with 0xFF"), so a caught panic would end the tsc.
#   - The relocation mode needs the static relocations (--emit-relocs).
# Checks of the BOLT tsc: no BOLT runtime code (no __bolt symbols), one RW PT_GNU_STACK, and
# check-unwind.py (a caught panic in the tsc, and every LSDA starts with 0xFF). release.yml then
# compares its output with the plain tsc on the training set.
set -euo pipefail

case "${1:-}" in
  help | -h | --help)
    sed -n '2,/^set -euo/p' "${BASH_SOURCE[0]}" | sed -e '$d' -e 's/^# \{0,1\}//'
    exit 0
    ;;
esac
(($# == 3)) || { echo "usage: bolt-musl.sh <tsc> <train-dir> <out-dir>" >&2; exit 2; }
script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd -- "$script_dir/../../.." && pwd)"
tsc=$1
train="$(cd -- "$2" && pwd)"
out=$3
llvm="${LLVM_BIN:-/usr/lib/llvm-23/bin}"
[[ ! -e $out ]] || { echo "error: $out exists" >&2; exit 2; }
mkdir -p "$out/counts" "$out/bin"
out="$(cd -- "$out" && pwd)"
# grep -c, not -q: with pipefail, a grep that stops early can fail the pipe (SIGPIPE).
[[ $(readelf -SW "$tsc" | grep -c ' \.rela\.text ') != 0 ]] ||
  { echo "error: $tsc has no .rela.text (link it with --emit-relocs: build-pgo.sh PGO_EMIT_RELOCS=1)" >&2; exit 1; }

# BOLT reads a symbol with ".cold" or ".warm" in it as a split part of another function and stops
# ("parent function not found"). Rust paths make such names ("Session..warm_auto_import_cache").
# Rename them in a copy: "..warm" to "..Warm" (same length, only .symtab changes).
nm "$tsc" | awk '$2 ~ /^[tT]$/ && $3 ~ /\.\.(cold|warm)/ {r = $3; gsub(/\.\.cold/, "..Cold", r); gsub(/\.\.warm/, "..Warm", r); print $3, r}' > "$out/rename.txt"
"$llvm/llvm-objcopy" --redefine-syms="$out/rename.txt" "$tsc" "$out/input"

# The counting copy. pgo-train.sh runs its copy at <train-dir>/bin/tsc.
"$llvm/llvm-bolt" "$out/input" -o "$out/instr" -instrument -instrumentation-file="$out/counts/tsc" \
  -instrumentation-file-append-pid -instrumentation-binpath="$train/bin/tsc" \
  -instrumentation-sleep-time=1 -instrumentation-no-counters-clear > "$out/instr.log" 2>&1 ||
  { tail -20 "$out/instr.log" >&2; exit 1; }
# One process per run: tsgo on 4 KiB pages would start a worker (bin/tsgo.rs `launch`).
GOPORT_LAUNCH=0 "$repo/scripts/goport/pgo-train.sh" run "$train" "$out/instr" > "$out/train.log" 2>&1 ||
  { tail -20 "$out/train.log" >&2; exit 1; }
# The watchers write up to about 1 s after their tsc ends.
sleep 5
left=$(pgrep -fc "^$train/bin/tsc" || true)
pkill -9 -f "^$train/bin/tsc" || true
echo "bolt training: $(find "$out/counts" -name 'tsc.*.fdata' | wc -l) count files, $left watchers killed"
"$llvm/merge-fdata" "$out"/counts/tsc.*.fdata > "$out/tsc.fdata" 2> "$out/merge.log" ||
  { tail -5 "$out/merge.log" >&2; exit 1; }

"$llvm/llvm-bolt" "$out/input" -o "$out/bin/tsc" -data="$out/tsc.fdata" \
  -reorder-blocks=ext-tsp -reorder-functions=cdsort -split-functions -split-all-cold > "$out/bolt.log" 2>&1 ||
  { tail -20 "$out/bolt.log" >&2; exit 1; }
echo "bolt: $(grep -m1 -o '[0-9]* out of [0-9]* functions in the binary ([0-9.]*%) have non-empty execution profile' "$out/bolt.log" || true)"

if [[ $(nm "$out/bin/tsc" | grep -c '__bolt') != 0 ]]; then
  echo "error: $out/bin/tsc has BOLT runtime symbols (the counting copy?)" >&2
  exit 1
fi
stack="$(readelf -lW "$out/bin/tsc" | awk '$1 == "GNU_STACK" { s = ""; for (i = 7; i < NF; i++) s = s $i; print s }')"
[[ $stack == RW ]] || { echo "error: $out/bin/tsc has PT_GNU_STACK '$stack', not one RW" >&2; exit 1; }
"$script_dir/copy-libs.sh" "$out/bin" > /dev/null
python3 "$script_dir/check-unwind.py" "$out/bin/tsc" "$llvm/llvm-dwarfdump"
echo "BOLT tsc: $out/bin/tsc"
