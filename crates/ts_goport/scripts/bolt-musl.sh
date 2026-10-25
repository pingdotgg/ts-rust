#!/usr/bin/env bash
# BOLT of the static musl linux-x64 tsc (release.yml). BOLT orders the functions, puts the
# functions with a profile first and the hot blocks of each function together. The program does
# not change. The profile comes from a counting copy (llvm-bolt -instrument) that runs the
# training set of scripts/goport/pgo-train.sh: the release runner has no perf branch sampling.
#
# Size (size1 bolt, 2026-10-25): the new code goes into the space of the old .text
# (-use-old-text), so the BOLT tsc is about 1.1 MB bigger than the PGO tsc without its pad, not
# 24 MB (by default llvm-bolt keeps the old .text and adds the new code after it).
# -use-old-text turns off lite mode: every function is rewritten, the functions with a profile in
# .text and the others in .text.cold. llvm-bolt writes the new .eh_frame, .eh_frame_hdr and
# .gcc_except_table in place of the old ones when they fit. The 1.1 MB:
#   - The jump tables move to a new .rodata (0.7 MB). The PIC jump tables of jemalloc, libunwind
#     and musl make llvm-bolt force -jump-tables=move, and -skip-funcs does not help: with
#     -use-old-text llvm-bolt reads every function, the skipped ones too.
#   - The new segment starts after .bss (0.3 MB of file gap: llvm-bolt puts the new program
#     header table at file offset = address - first load address). Do not strip the BOLT tsc:
#     llvm-strip moves the new segment back over the gap, and Linux before 5.18 then gives the
#     tsc a wrong AT_PHDR (first load address + e_phoff), through which musl finds PT_TLS.
#   - The pad (64 KiB; `bolt-musl.sh pad`): block reordering splits call-site ranges, so the new
#     .gcc_except_table is about 2.6% bigger than the old one (R236: 25.8 KB). Without the pad it
#     does not fit and goes to the new segment (1.0 MB more). The script prints the pad left.
#   - No -split-functions: with it the code needs about 0.5 MB more than the old .text. Without
#     it the st cycles:u stay the same (query, hono, zod, VS Code, Sentry: -3.4% against the PGO
#     tsc, -3.3% with -split-functions); peak RSS of small projects is up to 4% higher.
#   - -align-functions=16 (the compiler's own; BOLT's default 64 needs more room) and -icf=safe
#     (folds about 900 functions) leave about 1% of the old .text free. The script prints it.
#   - -no-huge-pages: no 2 MiB alignment of the code.
#   When the code does not fit, llvm-bolt prints "--use-old-text failed" and writes a tsc with the
#   code in a new segment and wrong unwind tables (a caught panic segfaults): the script stops.
#
# Usage: bolt-musl.sh pad <out.o>
#   writes the pad object and prints the rustc flags that link it into the PGO tsc (release.yml
#   adds them to PGO_RUSTFLAGS). -u keeps the pad from --gc-sections.
# Usage: bolt-musl.sh <tsc> <train-dir> <out-dir>
#   tsc        the PGO tsc, linked with --emit-relocs (build-pgo.sh PGO_EMIT_RELOCS=1) and the pad
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
#   - No -split-eh (with -split-functions). It moves landing pads to the cold part and gives
#     their LSDA a start address (LPStart). The musl target unwinds with LLVM libunwind, which logs
#     "lsda at ... does not start with 0xFF" for such an LSDA. In some cases a caught panic through
#     it then aborts (relmusl1's test program: "failed to initiate panic"); in others it works.
#   - The relocation mode needs the static relocations (--emit-relocs).
# Checks of the BOLT tsc: no BOLT runtime code (no __bolt symbols), one RW PT_GNU_STACK, and
# check-unwind.py (two caught panics in the tsc with no libunwind line, and every LSDA starts with
# 0xFF: a cautious guard that can reject a tsc that would work). release.yml then compares its
# output with the plain tsc on the training set.
set -euo pipefail

case "${1:-}" in
  help | -h | --help)
    sed -n '2,/^set -euo/p' "${BASH_SOURCE[0]}" | sed -e '$d' -e 's/^# \{0,1\}//'
    exit 0
    ;;
  pad)
    (($# == 2)) || { echo "usage: bolt-musl.sh pad <out.o>" >&2; exit 2; }
    printf '\t.section .gcc_except_table.tsc_lsda_pad,"a",@progbits\n\t.globl tsc_lsda_pad\ntsc_lsda_pad:\n\t.zero 65536\n' |
      as -o "$2" -
    echo "-Clink-arg=$2 -Clink-arg=-Wl,-u,tsc_lsda_pad"
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
  -reorder-blocks=ext-tsp -reorder-functions=cdsort -use-old-text -no-huge-pages -align-functions=16 \
  -icf=safe > "$out/bolt.log" 2>&1 ||
  { tail -20 "$out/bolt.log" >&2; exit 1; }
echo "bolt: $(grep -m1 -o '[0-9]* out of [0-9]* functions in the binary ([0-9.]*%) have non-empty execution profile' "$out/bolt.log" || true)"
if [[ $(grep -c -e '--use-old-text failed' "$out/bolt.log") != 0 ]]; then
  echo "error: the new code does not fit in the old .text: $(grep -m1 -o '[0-9]* bytes needed, have [0-9]* bytes available' "$out/bolt.log")" >&2
  exit 1
fi
# sect_end FILE NAME: the end address of section NAME (0 when FILE has none). Ubuntu's awk (mawk)
# has no strtonum, so bash adds the hex address and size.
sect_end() {
  local a s
  read -r a s < <(readelf -SW "$1" | awk -v n="$2" '{ for (i = 1; i < NF; i++) if ($i == n) print $(i + 2), $(i + 4) }') || true
  echo $((${a:+16#$a} + ${s:+16#$s} + 0))
}
code_end=$(sect_end "$out/bin/tsc" .text.cold)
((code_end > 0)) || code_end=$(sect_end "$out/bin/tsc" .text)
echo "bolt: old .text free $(($(sect_end "$out/input" .text) - code_end)) bytes; BOLT tsc $(stat -c %s "$out/bin/tsc") bytes"
# The new .gcc_except_table in place of the old one: the zeros at its end are the pad left.
if [[ $(grep -c 'rewriting .gcc_except_table in-place' "$out/bolt.log") != 0 ]]; then
  "$llvm/llvm-objcopy" -O binary --only-section=.gcc_except_table "$out/bin/tsc" "$out/lsda.bin"
  echo "bolt: .gcc_except_table in place, pad left $(python3 -c 'import sys; d = open(sys.argv[1], "rb").read(); print(len(d) - len(d.rstrip(b"\0")))' "$out/lsda.bin") bytes"
else
  echo "warning: the new .gcc_except_table does not fit in the old one, so it adds about 1 MB: link a bigger pad (bolt-musl.sh pad)" >&2
fi

if [[ $(nm "$out/bin/tsc" | grep -c '__bolt') != 0 ]]; then
  echo "error: $out/bin/tsc has BOLT runtime symbols (the counting copy?)" >&2
  exit 1
fi
stack="$(readelf -lW "$out/bin/tsc" | awk '$1 == "GNU_STACK" { s = ""; for (i = 7; i < NF; i++) s = s $i; print s }')"
[[ $stack == RW ]] || { echo "error: $out/bin/tsc has PT_GNU_STACK '$stack', not one RW" >&2; exit 1; }
"$script_dir/copy-libs.sh" "$out/bin" > /dev/null
python3 "$script_dir/check-unwind.py" "$out/bin/tsc" "$llvm/llvm-dwarfdump"
echo "BOLT tsc: $out/bin/tsc"
