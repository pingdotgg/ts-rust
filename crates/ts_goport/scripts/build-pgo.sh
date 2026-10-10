#!/usr/bin/env bash
# Builds a PGO (profile-guided) release of the goport binaries on stable Rust,
# with the workspace `goport` cargo profile (fat LTO, one codegen unit).
# The release workflow (.github/workflows/release.yml) builds the shipped tsc
# with it on each platform, trained by scripts/goport/pgo-train.sh (PGO_TRAIN).
#
# Usage: build-pgo.sh [out-dir]
#   out-dir  default: <data-root>/target/goport-pgo
#
# Steps:
#   1. goport-profile build with -Cprofile-generate (instrumented).
#   2. Training runs: PGO_TRAIN, or by default goport and tsgo on query, hono,
#      zod, effect and elysia in the timed form (see below), goport on a
#      spread of corpus cases, and goport_emit on query and hono.
#   3. Merge the .profraw files with llvm-profdata.
#   4. goport-profile build with -Cprofile-use. The binaries (goport,
#      goport_emit, goport_build, goport_typesyms, tsgo; or PGO_BINS) land in
#      <out-dir>/target-use/goport.
#   5. Optional: scripts/build-bolt.sh <out-dir>/target-use/goport gives
#      BOLT copies of tsgo and goport (see its header).
#
# Link layout of the use build on Linux (perf9 round 3, r3-link). It adds two
# lld flags. The code does not change:
#   -z keep-text-section-prefix  keeps the .text.hot and .text.unlikely
#       groups that LLVM makes from the profile as their own sections, so hot
#       code sits together. Without it lld mixes them into .text. This is
#       the layout when BOLT is not used.
#   --emit-relocs  keeps the static relocations in the file (non-alloc
#       sections, not mapped at run time). BOLT needs them for relocation
#       mode, where it also reorders functions. Not with PGO_LINK=static:
#       build-bolt.sh refuses static bins, so they only make the file bigger.
# The build prints the .text and .rela.text sections of tsgo
# (readelf -S). Expect .text.hot, .text.unlikely and .rela.text. GNU ld (the
# linker of the musl target) ignores keep-text-section-prefix, but its default
# script already puts the hot and the unlikely code together inside .text.
# Other platforms (macOS) get neither flag.
#
# Environment:
#   RUSTUP_TOOLCHAIN  default 1.95.0. Its LLVM 22 matches the system
#                     llvm-profdata (LLVM 22). 1.93 has LLVM 21 and cannot read
#                     the profile that LLVM 22 writes.
#   GOPORT_DATA_ROOT  checkout that holds target/project-inputs and the corpus
#                     (default: the main checkout of this repository)
#   LLVM_PROFDATA     llvm-profdata to use. Default: the rustup llvm-tools copy
#                     (rustup component add llvm-tools) if present, else
#                     llvm-profdata on PATH. Its LLVM must not be newer than
#                     rustc's LLVM.
#   PGO_ALLOW_LLVM_MISMATCH=1  run anyway (step 4 then fails its check)
#   PGO_CORPUS_STEP   train on every Nth corpus case (default 60, about 200)
#   CARGO_FEATURES    extra cargo flags, for example "--features jemalloc".
#                     Retrain when the allocator or hot code changes.
#   PGO_TARGET        build for this target (cargo --target), for example
#                     x86_64-unknown-linux-musl. Default: the host, with
#                     --target only when PGO_LINK is set.
#   PGO_BINS          the bins to build, for example "tsgo" (default: all)
#   PGO_TRAIN         training command, in place of the default training
#                     (which needs the project inputs and the corpus of
#                     GOPORT_DATA_ROOT). The script runs `$PGO_TRAIN <dir>`,
#                     where <dir> holds the instrumented bins, with
#                     PGO_PROFILE_DIR set to the profile dir. Example:
#                     "scripts/goport/pgo-train.sh run <train-dir>".
#   PGO_CARGO         cargo command of both builds. Default:
#                     scripts/run-cargo-capped.sh when present, else cargo.
#                     CI sets cargo (the capped wrapper needs systemd-run).
#   PGO_LINK          link mode of both builds (opt-in; default: PIE, as
#                     before):
#                     nopie   -C relocation-model=static: a non-PIE
#                             executable (ELF type EXEC), still dynamic.
#                     static  nopie and -C target-feature=+crt-static: a
#                             static non-PIE executable (EXEC, no INTERP),
#                             like Go tsgo.
#                     crt-static  only -C target-feature=+crt-static. On
#                             Windows (MSVC) it links the C runtime in, so
#                             the exe needs no Visual C++ redistributable.
#                     Why: one exec of the 32 MB dynamic PIE costs about
#                     3.3 ms on cup2 (350 to 430 page faults; 146 are ld.so
#                     copy-on-write faults for 28,002 relative relocations),
#                     and a run execs twice (the malloc tunables re-exec).
#                     These builds pass --target <host>, so build scripts
#                     and proc macros do not get the flags. The bins are
#                     copied to the usual <target>/goport path. The build
#                     checks the ELF type and INTERP of tsgo.
#
# Choosing PGO_LINK (integrator, once a round): build the default and
# PGO_LINK=static. On cup2 (under its lock) time 'tsgo --version' and query
# at 4 and 16 cores, with minor faults (/usr/bin/time -f %R or getrusage
# ru_minflt; perf is blocked on cup2). Keep static only
# when it is faster at both core counts; else try nopie the same way; else
# keep the default. Check that the GLIBC_TUNABLES re-exec still happens:
#   strace -f -qq -v -s 4096 -e trace=execve -e signal=none tsgo --version
# shows two execve calls, the second with GLIBC_TUNABLES=...
# build-bolt.sh refuses static bins (BOLT breaks static glibc; see its
# header). So when BOLT is used, compare static without BOLT against the
# BOLT copy of nopie or of the default.
#
# The training runs only read project inputs: goport does not write,
# goport_emit writes to a temp --outDir and tsgo writes its .tsbuildinfo to
# a temp file.
#
# Written for bash 3.2 (macOS), except the default training. On Windows it runs
# in Git Bash: the bins are .exe files, and the paths that cargo, rustc and
# llvm-profdata get are C:/... paths (cygpath -m).
set -euo pipefail

# `help` (or -h, --help) prints this header. Without this, the word became the
# out-dir and started a build.
case "${1:-}" in
  help | -h | --help)
    sed -n '2,/^set -euo/p' "${BASH_SOURCE[0]}" | sed -e '$d' -e 's/^# \{0,1\}//'
    exit 0
    ;;
esac

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd -- "$script_dir/../../.." && pwd)"
data_root="${GOPORT_DATA_ROOT:-$(cd -- "$(git -C "$repo" rev-parse --path-format=absolute --git-common-dir)/.." && pwd)}"
out="${1:-$data_root/target/goport-pgo}"
features="${CARGO_FEATURES:-}"
corpus_step="${PGO_CORPUS_STEP:-60}"
train_cmd="${PGO_TRAIN:-}"
bin_args="--bins"
if [[ -n ${PGO_BINS:-} ]]; then
  bin_args=""
  for b in $PGO_BINS; do bin_args+=" --bin $b"; done
fi
export RUSTUP_TOOLCHAIN="${RUSTUP_TOOLCHAIN:-1.95.0}"
# The training runs start no tsgo worker (bin/tsgo.rs `launch`): when the
# launcher exits, the parent death signal can kill the worker before it has
# written its profile.
export GOPORT_LAUNCH=0
exe=""
case "$(uname -s)" in MINGW* | MSYS* | CYGWIN*) exe=.exe ;; esac
mkdir -p "$out"
out="$(cd -- "$out" && pwd)"
[[ -z $exe ]] || out="$(cygpath -m "$out")"
# Run rustc and cargo from the repository. RUSTUP_TOOLCHAIN overrides any
# toolchain file there.
cd "$repo"

# llvm-profdata must read the raw profiles of rustc's LLVM and write an
# indexed profile that rustc's LLVM can read. A newer major version reads old
# raw profiles, but its indexed output can be too new; step 4 fails then.
sysroot="$(rustc --print sysroot)"
host="$(rustc -vV | sed -n 's/^host: //p')"
rustc_llvm="$(rustc -vV | sed -n 's/^LLVM version: \([0-9]*\).*/\1/p')"
profdata="${LLVM_PROFDATA:-}"
if [[ -z "$profdata" ]]; then
  if [[ -x "$sysroot/lib/rustlib/$host/bin/llvm-profdata$exe" ]]; then
    profdata="$sysroot/lib/rustlib/$host/bin/llvm-profdata$exe"
  else
    profdata="$(command -v llvm-profdata)"
  fi
fi
profdata_llvm="$("$profdata" --version | sed -n 's/.*LLVM version \([0-9]*\).*/\1/p' | head -1)"
echo "rustc $(rustc -V | cut -d' ' -f2) LLVM $rustc_llvm, $profdata LLVM $profdata_llvm"
# Example: rustc 1.93 (LLVM 21) cannot read the indexed format 13 that LLVM 22
# writes. It only warns and builds without the profile.
if ((profdata_llvm > rustc_llvm)) && [[ "${PGO_ALLOW_LLVM_MISMATCH:-0}" != 1 ]]; then
  echo "error: llvm-profdata (LLVM $profdata_llvm) is newer than rustc's LLVM $rustc_llvm." >&2
  echo "Run 'rustup component add llvm-tools' for this toolchain, or set LLVM_PROFDATA." >&2
  exit 1
fi

# Use the repository's memory-capped cargo wrapper when it exists.
cargo_cmd=(cargo)
if [[ -n ${PGO_CARGO:-} ]]; then
  cargo_cmd=("$PGO_CARGO")
elif [[ -x "$repo/scripts/run-cargo-capped.sh" ]]; then
  cargo_cmd=("$repo/scripts/run-cargo-capped.sh")
fi

profiles="$out/profiles"
rm -rf "$profiles"
mkdir -p "$profiles"

link="${PGO_LINK:-}"
case "$link" in
  "") link_flags="" ;;
  nopie) link_flags="-Crelocation-model=static" ;;
  static) link_flags="-Crelocation-model=static -Ctarget-feature=+crt-static" ;;
  crt-static) link_flags="-Ctarget-feature=+crt-static" ;;
  *) echo "error: PGO_LINK must be empty, nopie, static or crt-static (got '$link')" >&2; exit 1 ;;
esac
# With --target, RUSTFLAGS apply only to the target crates, not to build
# scripts and proc macros (a proc macro cannot be +crt-static).
triple="${PGO_TARGET:-}"
[[ -z $triple && -n $link_flags ]] && triple="$host"

# Each step needs its own target (other RUSTFLAGS). sccache is off: it could
# reuse an object built with an older profile at the same path.
build() { # build <target-subdir> <rustflags>
  local target="$out/$1" flags="$2 $link_flags" target_args=""
  # Both builds get the same link mode, so the profile of the generate build
  # matches the code of the use build.
  [[ -n $triple ]] && target_args="--target $triple"
  echo "== build $1 ($flags)"
  # shellcheck disable=SC2086
  env CARGO_TARGET_DIR="$target" TS_CARGO_SEPARATE_TARGET=1 TS_CARGO_SCCACHE=0 RUSTFLAGS="$flags" \
    "${cargo_cmd[@]}" build --profile goport --offline --locked -p ts_goport $bin_args $target_args $features \
    > "$out/build-$1.log" 2>&1 || { tail -20 "$out/build-$1.log" >&2; exit 1; }
  # With --target cargo writes the bins to <target>/<triple>/goport. Copy them
  # to <target>/goport, where they are without --target.
  if [[ -n $triple ]]; then
    mkdir -p "$target/goport"
    find "$target/$triple/goport" -maxdepth 1 -type f -perm -u+x -exec cp -p {} "$target/goport/" \;
  fi
}

# check_link <bin>: stops when the ELF type or INTERP does not match PGO_LINK.
check_link() {
  [[ -n "$link" && $link != crt-static ]] || return 0
  local type interp=no want
  type="$(readelf -h "$1" | sed -n 's/^ *Type: *\([A-Z]*\).*/\1/p')"
  [[ "$(readelf -lW "$1")" == *" INTERP "* ]] && interp=yes
  if [[ "$link" == static ]]; then want="EXEC no"; else want="EXEC yes"; fi
  echo "link $link: $1 type $type interp $interp"
  if [[ "$type $interp" != "$want" ]]; then
    echo "error: PGO_LINK=$link wants type/interp '$want', got '$type $interp'" >&2
    exit 1
  fi
}

# 1. Instrumented build.
build target-gen "-Cprofile-generate=$profiles"
gen="$out/target-gen/goport"
[[ $(uname -s) != Linux || ! -x $gen/tsgo ]] || check_link "$gen/tsgo"

# 2. Training.
# default_train: the local training. Exit codes are ignored: some inputs have
# diagnostics on purpose.
# The binaries re-exec with their built-in malloc string only when these are
# unset (bin/goport.rs set_malloc_tunables). The timed runs have them unset,
# so train the same way.
default_train() {
  local P="$data_root/target/project-inputs" X="$data_root/target/project-inputs-extra" name s cfg cases n i dir
  local -A projects=(
    [query]="$P/query/source/packages/query-core/tsconfig.prod.json"
    [hono]="$P/hono/source/tsconfig.build.json"
    [zod]="$P/zod/source/packages/zod/tsconfig.json"
    [effect]="$P/effect/source/packages/effect/tsconfig.json"
    [elysia]="$X/elysia/src/tsconfig.json"
  )
  for name in query hono zod effect elysia; do
    s=$SECONDS
    "$gen/goport" -p "${projects[$name]}" > /dev/null 2>&1 || true
    echo "train goport $name $((SECONDS - s))s"
  done
  # tsgo in the timed form: -p <cfg> --noEmit --pretty false --tsBuildInfoFile
  # <temp>. --pretty false keeps FORCE_COLOR in the caller's env from training
  # the pretty diagnostic path. Only incremental projects (hono, effect) use the
  # build info file.
  for name in query hono zod effect elysia; do
    s=$SECONDS
    "$gen/tsgo" -p "${projects[$name]}" --noEmit --pretty false \
      --tsBuildInfoFile "$tmp/$name.tsbuildinfo" > /dev/null 2>&1 || true
    echo "train tsgo $name $((SECONDS - s))s"
  done
  for cfg in "${projects[query]}" "${projects[hono]}"; do
    "$gen/goport_emit" -p "$cfg" --outDir "$tmp/out" > /dev/null 2>&1 || true
    rm -rf "$tmp/out"
  done
  cases="$data_root/target/continuation-r97-goport/corpus-full/cases"
  n=0
  if [[ -d "$cases" ]]; then
    i=0
    for dir in "$cases"/*/; do
      if ((i++ % corpus_step == 0)) && [[ -f "$dir/tsconfig.json" ]]; then
        (cd "$dir" && timeout 60 "$gen/goport" -p tsconfig.json > /dev/null 2>&1) || true
        n=$((n + 1))
      fi
    done
  fi
  echo "train corpus $n cases"
}
unset GLIBC_TUNABLES _RJEM_MALLOC_CONF
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
[[ -z $exe ]] || tmp="$(cygpath -m "$tmp")"
if [[ -n $train_cmd ]]; then
  echo "== train: $train_cmd $gen"
  # pgo-train.sh gives each run its own profile file in PGO_PROFILE_DIR (see its `one`).
  # shellcheck disable=SC2086 # a command with its args
  PGO_PROFILE_DIR="$profiles" $train_cmd "$gen"
else
  default_train
fi
nprof=$(find "$profiles" -name '*.profraw' | wc -l | tr -d ' ')
echo "$nprof profraw files"
((nprof > 0)) || { echo "error: the training wrote no profile" >&2; exit 1; }

# 3. Merge. The file name holds the profile hash: cargo does not track the
# profile content, but it rebuilds when RUSTFLAGS change.
rm -f "$out"/goport-*.profdata
"$profdata" merge -o "$tmp/merged.profdata" "$profiles"
merged="$out/goport-$(cksum < "$tmp/merged.profdata" | cut -d' ' -f1).profdata"
mv "$tmp/merged.profdata" "$merged"
echo "merged $(du -h "$merged" | cut -f1) -> $merged"

# 4. Optimized build. rustc only warns when it cannot read the profile, so
# check the log.
# The Linux link args are the r3-link layout (see the header).
use_flags="-Cprofile-use=$merged"
if [[ $(uname -s) == Linux ]]; then
  [[ $link == static ]] || use_flags+=" -Clink-arg=-Wl,--emit-relocs"
  use_flags+=" -Clink-arg=-Wl,-z,keep-text-section-prefix"
fi
build target-use "$use_flags"
if grep -qE "profile format version|profile-use" "$out/build-target-use.log"; then
  grep -E "profile format version|profile-use" "$out/build-target-use.log" | head -3 >&2
  echo "error: rustc did not use the profile; the binaries are not PGO builds" >&2
  exit 1
fi
use="$out/target-use/goport"
if [[ $(uname -s) == Linux && -x $use/tsgo ]]; then
  check_link "$use/tsgo"
  sections="$(readelf -SW "$use/tsgo" | grep -oE ' \.(rela\.)?text[.a-z]*' | sort -u | tr -d ' ' | tr '\n' ' ')"
  echo "tsgo sections: $sections"
  if [[ $link != static ]]; then
    [[ " $sections " == *" .text.hot "* ]] || echo "warning: tsgo has no .text.hot section" >&2
    [[ " $sections " == *" .rela.text "* ]] || echo "warning: tsgo has no .rela.text; BOLT runs in non-relocation mode" >&2
  fi
fi
echo "PGO binaries in $use: ${PGO_BINS:-goport goport_emit goport_build goport_typesyms tsgo}"
