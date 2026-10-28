#!/usr/bin/env bash
# Compiles memfast.c (lust's memcpy, memmove, memcmp, bcmp and memchr; MIT, see its header) for
# the static musl linux-x64 tsc, checks the object, and prints the rustc flags that link it in
# place of musl's functions. release.yml passes them to build-pgo.sh (PGO_RUSTFLAGS).
#
# Usage: memfast.sh <out.o>
#   CC  the C compiler (default musl-gcc)
#
# Why: musl's memcmp and bcmp are byte loops and its memcpy and memmove use rep movs. They took
# 4.4% of the cycles of a VS Code check and 9.2% on TypeORM; with memfast 2.0% and 3.3% (relmusl1,
# cycles:u --singleThreaded). Results are libc's: the tsc output does not change.
# The functions get the names __wrap_<name>, and the link wraps the five symbols
# (-Wl,--wrap=<name>). Plain names would clash with musl's in libc.a, because rustc puts
# -C link-arg after it. --wrap sends every call to these, musl's own calls too. memset stays
# musl's (asm).
# Checks: no undefined symbol (a loop that the compiler turns back into a memcpy call would call
# itself through the wrap), and no AVX or BMI code (the tsc runs on any x86-64).
set -euo pipefail

case "${1:-}" in
  "" | help | -h | --help)
    sed -n '2,/^set -euo/p' "${BASH_SOURCE[0]}" | sed -e '$d' -e 's/^# \{0,1\}//'
    exit 2
    ;;
esac
out=$1
src="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)/memfast.c"
names=(memcpy memmove memcmp bcmp memchr)
defs=() wraps=""
for n in "${names[@]}"; do
  defs+=("-D$n=__wrap_$n")
  wraps+="${wraps:+,}--wrap=$n"
done
# lust builds the unit with -fno-strict-aliasing (see the comment in memfast.c).
"${CC:-musl-gcc}" -O2 -fno-strict-aliasing "${defs[@]}" -c "$src" -o "$out"
undefined="$(nm -u "$out")"
if [[ -n $undefined ]]; then
  echo "error: $out has undefined symbols: $undefined" >&2
  exit 1
fi
# grep -c, not -q: with pipefail, a grep that stops early can fail the pipe (SIGPIPE).
if [[ $(objdump -d --no-show-raw-insn "$out" |
  grep -cE $'\t(v[a-z0-9]+|andn|bextr|blsi|blsmsk|blsr|bzhi|pdep|pext|mulx|rorx|sarx|shlx|shrx) ') != 0 ]]; then
  echo "error: $out has AVX or BMI code" >&2
  exit 1
fi
for n in "${names[@]}"; do
  [[ $(nm "$out" | grep -cE " T __wrap_$n\$") == 1 ]] || { echo "error: $out has no __wrap_$n" >&2; exit 1; }
done
echo "-Clink-arg=-Wl,$wraps -Clink-arg=$out"
