#!/usr/bin/env bash
# Builds the release goport binaries: PGO, then BOLT. Only build options
# change, not the source.
#
# Default (the shipped build): dynamic glibc with jemalloc as the allocator
# (default cargo feature `jemalloc`; jemalloc has
# narenas:4,thp:always,metadata_thp:disabled,cache_oblivious:false built in: this script sets
# JEMALLOC_SYS_WITH_MALLOC_CONF to the JEMALLOC_CONF line of bin/tsgo.rs, see bin/goport.rs
# `set_malloc_tunables`; step 4 checks it). The bins link against glibc 2.28, so
# they start on any x86-64 Linux with glibc 2.28 or later (Debian 10, RHEL 8,
# Ubuntu 20.04 and later).
# RELEASE_STATIC=1: static glibc with glibc malloc, for comparison.
# RELEASE_LIBC=musl: static musl with jemalloc (no shared libraries, like Go's
# tsgo). RELEASE_PIE=0: a non-PIE bin (like Go's tsgo). See the static1 note
# below for what each one costs.
#
# Lib files: a shipped release (RELEASE_VERSION set) is Go's noembed build, as
# Go's release builds and npm packages are. The lib files are not in the bins;
# they are next to them. A dev build (no RELEASE_VERSION) embeds them, as the
# pinned oracle does.
#
# Usage: build-release.sh [out-dir]
#   out-dir  default: <data-root>/target/goport-release
#
# Steps:
#   0. Dynamic build: make the glibc 2.28 sysroot once (floor_sysroot).
#   1. Instrumented build (-Cprofile-generate) of goport, tsgo and goport_emit.
#   2. PGO training: the same runs as build-pgo.sh.
#   3. Merge the raw profiles with llvm-profdata.
#   4. PGO use build of the shipped bins (tsgo, goport, goport_emit,
#      goport_build, goport_typesyms), linked with --emit-relocs. BOLT needs
#      the relocations. They do not change the code.
#      Dynamic build: check that no bin needs a GLIBC_ symbol version above
#      the floor (objdump -T).
#   5. BOLT: record each bin with perf branch sampling on training runs
#      (for tsgo, also editor sessions: RELEASE_LSP_SESSIONS), convert with
#      perf2bolt, merge with merge-fdata, rewrite with llvm-bolt.
#      Then check the program headers of the shipped bins (check_headers).
#   6. Run tsgo in qemu on a CPU without AVX. A dynamic tsgo runs there on the
#      glibc 2.28 of the sysroot.
#   The binaries land in <out-dir>/bin, with BUILD.txt (and the lib files
#   in a noembed build).
#
# Measured on R121 source, against the plain goport profile and build-pgo.sh:
#   - zbook (glibc 2.44), paired perf stat: 14 to 15% fewer cycles than plain
#     and 1.3 to 2.6% fewer than PGO (BOLT; svelte, not trained: 1.7%).
#     Static glibc changes nothing on zbook.
#   - cup2 (glibc 2.41), wall time: 24 to 36% less than the PGO build. A
#     dynamic glibc malloc build uses the host glibc malloc, which is slow in
#     glibc 2.41 with the tunables that tsgo sets.
# Measured on R122 source (target/continuation-r97-goport/release2/measure.md),
# geometric mean against the static glibc build: the dynamic jemalloc build
# with narenas:4 only was 12 to 14% slower on dbook (THP madvise: jemalloc got
# no huge pages) and 3 to 7% slower on cup2 (THP always). With
# thp:always,metadata_thp:always set by hand it was 0.5 to 6% faster on dbook
# and 2 to 4% slower on cup2. So the bins now set thp:always (rss1 then set
# metadata_thp:disabled and cache_oblivious:false for less RSS, see
# bin/goport.rs `set_malloc_tunables`). A dynamic build has
# no LGPL relink duties (glibc stays a shared library). Linked against the
# host glibc it needed glibc 2.39 (pidfd_spawnp in Rust std), hence the floor.
# Not used: -C target-cpu=x86-64-v3 (no measurable gain, and no AVX2 means
# SIGILL), and panic=abort (the port catches panics for Go recover parity).
# Measured on R145 source (static1 lane, target/continuation-r97-goport/static1):
# PGO + BOLT builds against the default, paired rounds of tsgo check and emit on
# query, hono, zod and effect, on dbook, mini-abf9 and mini-743d (mean of the 8
# cells per run). Output is byte-equal.
#   - RELEASE_PIE=0: 1.0 to 1.3% faster, peak RSS the same. Same glibc floor.
#     The bin loses ASLR for its own code and data, as Go's tsgo (non-PIE) does.
#   - RELEASE_LIBC=musl RELEASE_PIE=0: starts on any x86-64 Linux (glibc 2.27,
#     Alpine), no shared libraries, 2 to 5 MiB less RSS on query, 0.6 to 1.2 ms
#     less to start. But 0.5 to 1.9% slower (hono up to 3.8%): musl's
#     memcpy, memcmp and memset take 5 to 7% of the time, glibc's about 3.5%.
#     Without PGO and BOLT, a musl static-pie bin is 4% slower than the
#     default.
#   - RELEASE_STATIC=1 RELEASE_JEMALLOC=1 with the plain x86-64 Arch glibc 2.44
#     (RELEASE_SYSROOT): starts on any x86-64 Linux with kernel 4.4 or later,
#     Alpine too; 0.8 to 1.3% faster, 0 to 7 MiB less RSS. The best of these,
#     but static glibc brings the LGPL duties (see RELEASE_STATIC), which need
#     a decision.
#   - -Wl,-z,pack-relative-relocs with glibc: the bin needs GLIBC_ABI_DT_RELR
#     (glibc 2.36), so it does not start on the floor glibc. Not used.
# So the default stays dynamic glibc and PIE.
# Measured on R148 source (pgolsp1, target/continuation-r97-goport/pgolsp1):
# editor sessions in the training against none, every side in one job on a
# mini (the first two rounds on dbook-lan). Output is byte-equal, and the LSP
# oracle answers are the same.
#   - Sessions in BOLT only at 2500 Hz (the default), on mini-abf9: editor
#     edit medians (ls_edit_bench long) query-core -3.7%, effect -3.5%, hono
#     -1.1% (another build of the same script on mini-743d: -4.9%, -4.9%,
#     -1.3%). The CLI cells (-p, --singleThreaded, tsc -b, watch) moved
#     -4.2 to +0.8% (hono watch api +1.4%, and -0.6% in a rerun with more
#     reps). Peak RSS moved 0.3% at most (the other build: query +1.6%).
#   - Sessions in PGO and BOLT at 1/8 of the CLI weight: about 1 point more
#     on effect and hono, but the bin text that a CLI run maps grew by 2 to
#     7 MiB (query check peak RSS +7.7% on mini-743d), most of it in the PGO
#     layout.
#   - Sessions in PGO and BOLT at full weight or 1/4: zod and effect check
#     lost 1.1 to 2.2%.
# Measured on R179 source (pgotrain2, target/continuation-r97-goport/pgotrain2):
# the effect project trained with the plugin (as before), without it, and both
# ways (as now), two release builds each, every side in one run: tsgo -p on the
# gate projects (effect with and without the plugin), T3 Code (5 workspaces,
# with and without the plugin), eslint-plugin-svelte and huggingface.js; 2 runs
# on mini-743d, 2 on alvin. Output is byte-equal.
#   - Both ways, against the plugin only: runs without the plugin 0.2 to 1.0%
#     faster (geometric mean of the cells), runs with it 0.4 to 0.8% faster
#     (one alvin run: 1.1% slower, within its noise), peak RSS the same
#     (query +0.5 to +0.7 MiB in 2 of 3 runs). BOLT hot text +6% (3.8 to
#     4.0 MiB).
#   - Without the plugin only: runs with the plugin 1.1 to 2.2% slower (effect
#     check 2 to 4%, T3 Code 1 to 3%), runs without it -0.6 to +2.0%, peak
#     RSS 0.3% less (query -0.5 to -1.1 MiB).
#
# Environment:
#   RUSTUP_TOOLCHAIN  default 1.95.0. Its LLVM 22 matches the system
#                     llvm-profdata (LLVM 22).
#   GOPORT_DATA_ROOT  checkout that holds target/project-inputs and the corpus
#                     (default: the main checkout of this repository)
#   LLVM_PROFDATA     llvm-profdata to use (same rule as build-pgo.sh)
#   RELEASE_STATIC    0 (default): dynamic glibc, linked against the floor
#                     sysroot. 1: static-pie, glibc linked in. Static glibc
#                     brings LGPL relink duties when shipped, and the glibc
#                     must support the oldest kernel we ship to.
#   RELEASE_LIBC      gnu (default): glibc, as RELEASE_STATIC says. musl: the
#                     target x86_64-unknown-linux-musl, fully static. rustc
#                     links its own musl (1.2.5) and libunwind; the jemalloc C
#                     code builds with musl-gcc (package musl). Needs
#                     `rustup target add x86_64-unknown-linux-musl` for
#                     RUSTUP_TOOLCHAIN. musl is MIT licensed (no LGPL duties).
#   RELEASE_PIE       1 (default): a position-independent bin (PIE or
#                     static-pie). 0: -C relocation-model=static, a bin at a
#                     fixed address with no relative relocations, as Go's.
#   RELEASE_JEMALLOC  1: jemalloc (default, except with static glibc). 0:
#                     glibc malloc (default with static glibc; the build
#                     passes --no-default-features).
#   RELEASE_FEATURES  more cargo features of ts_goport for both builds
#                     (default none). "jemalloc" there sets RELEASE_JEMALLOC=1.
#                     "noembed" builds Go's noembed mode, as Go's release builds
#                     and npm packages: the lib files are not in the bins, which
#                     read them from their own dir and print their real paths.
#                     The script copies them (scripts/copy-libs.sh) next to the
#                     bins in each dir that runs them and in <out-dir>/bin.
#                     RELEASE_VERSION adds "noembed".
#   RELEASE_VERSION   the version the bins report (tsc -v, .tsbuildinfo,
#                     typesVersions matching, ATA), for example 7.0.2. It is
#                     passed to the build as the build-time env var
#                     GOPORT_BUILD_VERSION (core.rs VERSION), as Go's release
#                     build sets core.version with -ldflags -X. Default:
#                     $GOPORT_BUILD_VERSION, else none, and then the bins report
#                     the source default 7.1.0-dev, as a Go build without -X.
#                     RELEASE_VERSION also adds the feature "noembed" (a
#                     shipped release). For a version with embedded libs, set
#                     GOPORT_BUILD_VERSION and not RELEASE_VERSION.
#   RELEASE_GLIBC_FLOOR  dynamic build: the newest GLIBC_ symbol version a
#                     bin may need (default 2.28). Change it together with
#                     RELEASE_SYSROOT.
#   RELEASE_SYSROOT   Dynamic build: the glibc to link against, with
#                     usr/include, usr/lib (libc.so.6, crt files, libgcc_s.so)
#                     and lib64/ld-linux-x86-64.so.2 for the qemu run.
#                     Default: <data-root>/target/goport-release-sysroot/glibc-2.28,
#                     made by floor_sysroot on first use.
#                     Static build: dir with the static glibc and libgcc to
#                     link: usr/lib/{libc.a,rcrt1.o,...} and libgcc.a,
#                     libgcc_eh.a and crtbeginS.o at the host cc's path under
#                     it. Default: the build host's. They must be built for
#                     plain x86-64. CachyOS builds them for its CPU level
#                     (zbook: AVX-512), so there use the Arch core glibc and
#                     gcc packages of the same versions, extracted.
#   RELEASE_BOLT      1 (default). 0: skip step 5, for hosts without llvm-bolt
#                     or perf branch sampling (Intel LBR, AMD LBR v2 or BRS).
#   PGO_CORPUS_STEP   train on every Nth corpus case (default 60, about 200)
#   BOLT_PERF_FREQ    perf sample frequency for BOLT (default 20000)
#   RELEASE_LSP_SESSIONS  editor sessions of the tsgo BOLT training, as
#                     <project>:<edits> (default "query-core:300 hono:200
#                     effect:300"; empty: none). Each is the long session of
#                     scripts/goport/ls_edit_bench.py (typing, errfix, imports
#                     and mix edits with VS Code-like request bursts), run by
#                     scripts/lsp-train.py with the same messages each time.
#                     BUILD.txt records the plan digests and the sha256 of
#                     ls_edit_bench.py. Without them, the editor's own code
#                     (the LSP server, snapshot updates, node reads of the
#                     edited file) runs in BOLT .cold code: 6.6% of an effect
#                     edit (studies/lspeffect1).
#   BOLT_LSP_PERF_FREQ  perf sample frequency of the editor sessions (default
#                     2500, 1/8 of the CLI runs' rate; see the pgolsp1 note
#                     above). The kernel cap (kernel.perf_event_max_sample_rate)
#                     applies to both rates: at 3000 (zbook, 2026-10-04) the
#                     sessions sample at about 2500 Hz and the CLI runs at
#                     3000, so the sessions are 42 to 43% of the tsgo BOLT
#                     samples there (pgo1, R170 builds), not about 12%.
#                     BUILD.txt records the cap.
#
# Rules this script keeps:
#   - Both cargo builds pass --target. The flags then reach only the shipped
#     code, not build scripts or proc macros (a proc macro cannot link static
#     code). --target also changes every mangled name, so a profile from a
#     build without --target (build-pgo.sh) does not match. Train here.
#   - Each run trains PGO and BOLT again. Do not reuse a profile after a
#     source change: a stale profile only makes rustc or BOLT warn.
#   - The BOLT runs of a jemalloc build set _RJEM_MALLOC_CONF to the
#     JEMALLOC_CONF line of the bin source, so the bin does not exec itself
#     even when its jemalloc lacks the built-in value.
#     The glibc tunables depend on the core count (ThreadBudget in
#     program.rs), so glibc bins exec themselves under perf. The perf2bolt
#     check below fails when the samples do not map to the bin.
#   - The stack stays non-executable. Without a PT_GNU_STACK header, or with
#     one that has E, glibc maps every thread stack PROT_EXEC, and glibc 2.41
#     and later with glibc.rtld.execstack=0 refuses to start the bin. So BOLT
#     runs without -use-gnu-stack (that option turns PT_GNU_STACK into the
#     new text segment), and check_headers fails on a bin without a RW
#     PT_GNU_STACK. BOLT then writes a new program header table at file
#     offset = address, which adds 4 MB of zeros to each file (sparse on
#     disk; they compress to nothing). Kernels before 5.18 find the table
#     only there, so do not strip or objcopy the BOLT output (that moves the
#     table). check_headers fails when it moved.
#   - The bins must run on any x86-64 CPU. Step 4 scans the libc and jemalloc
#     code for AVX and BMI, and step 6 runs tsgo in qemu-x86_64 with a CPU
#     model without AVX (package qemu-user).
#   - A dynamic build links against glibc 2.28 (the sysroot), not the host
#     glibc: a symbol version binds to the glibc it is linked against. The C
#     code of jemalloc builds against the same sysroot (CFLAGS_<target>), so
#     its headers do not ask for newer symbols.
#
# The training runs only read project inputs: emit writes to a temp --outDir,
# the editor sessions send the edits as overlays (didOpen, didChange),
# tsgo writes .tsbuildinfo to a temp file, goport_build runs on a temp copy.
set -euo pipefail

# `help` (or -h, --help) prints the header. Without this, the word became the
# out-dir and started a real build, outside the build lock.
case "${1:-}" in
  help | -h | --help)
    sed -n '2,/^set -euo/{/^set -euo/d;s/^# \{0,1\}//;p}' "${BASH_SOURCE[0]}"
    exit 0
    ;;
esac

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd -- "$script_dir/../../.." && pwd)"
data_root="${GOPORT_DATA_ROOT:-$(cd -- "$(git -C "$repo" rev-parse --path-format=absolute --git-common-dir)/.." && pwd)}"
out="${1:-$data_root/target/goport-release}"
libc="${RELEASE_LIBC:-gnu}"
pie="${RELEASE_PIE:-1}"
static="${RELEASE_STATIC:-0}"
case $libc in
  gnu) jemalloc="${RELEASE_JEMALLOC:-$((static == 1 ? 0 : 1))}" ;;
  musl) static=1 jemalloc="${RELEASE_JEMALLOC:-1}" ;;
  *) echo "error: RELEASE_LIBC is gnu or musl, not $libc" >&2; exit 1 ;;
esac
features="${RELEASE_FEATURES:-}"
# A shipped release is the noembed build (see the header).
if [[ -n ${RELEASE_VERSION:-} && ",${features// /,}," != *,noembed,* ]]; then
  features="${features:+$features,}noembed"
fi
[[ ",${features// /,}," == *,jemalloc,* ]] && jemalloc=1
noembed=0
[[ ",${features// /,}," == *,noembed,* ]] && noembed=1
version="${RELEASE_VERSION:-${GOPORT_BUILD_VERSION:-}}"
if [[ -n $version ]]; then export GOPORT_BUILD_VERSION="$version"; else unset GOPORT_BUILD_VERSION; fi
glibc_floor="${RELEASE_GLIBC_FLOOR:-2.28}"
bolt="${RELEASE_BOLT:-1}"
corpus_step="${PGO_CORPUS_STEP:-60}"
lsp_sessions="${RELEASE_LSP_SESSIONS-query-core:300 hono:200 effect:300}"
export RUSTUP_TOOLCHAIN="${RUSTUP_TOOLCHAIN:-1.95.0}"
# The training runs start no tsgo worker (bin/tsgo.rs `launch`): when the
# launcher exits, the parent death signal can kill the worker before it has
# written its profile.
export GOPORT_LAUNCH=0
shipped=(tsgo goport goport_emit goport_build goport_typesyms)
mkdir -p "$out"
out="$(cd -- "$out" && pwd)"
cd "$repo"

# llvm-profdata must not be newer than rustc's LLVM (see build-pgo.sh).
sysroot="$(rustc --print sysroot)"
host="$(rustc -vV | sed -n 's/^host: //p')"
target="$host"
if [[ $libc == musl ]]; then
  target="${host%-gnu}-musl"
  # -Cprofile-generate needs the profiler runtime of the target's std.
  compgen -G "$sysroot/lib/rustlib/$target/lib/libprofiler_builtins-*.rlib" > /dev/null \
    || { echo "error: no std for $target in $sysroot (rustup target add $target)" >&2; exit 1; }
  command -v musl-gcc > /dev/null || { echo "error: musl-gcc not found (package musl); jemalloc needs it" >&2; exit 1; }
fi
rustc_llvm="$(rustc -vV | sed -n 's/^LLVM version: \([0-9]*\).*/\1/p')"
profdata="${LLVM_PROFDATA:-}"
if [[ -z "$profdata" ]]; then
  if [[ -x "$sysroot/lib/rustlib/$host/bin/llvm-profdata" ]]; then
    profdata="$sysroot/lib/rustlib/$host/bin/llvm-profdata"
  else
    profdata="$(command -v llvm-profdata)"
  fi
fi
profdata_llvm="$("$profdata" --version | sed -n 's/.*LLVM version \([0-9]*\).*/\1/p' | head -1)"
echo "rustc $(rustc -V | cut -d' ' -f2) LLVM $rustc_llvm, $profdata LLVM $profdata_llvm"
# A rustc that is not a rustup proxy (for example /usr/bin/rustc first on
# PATH) ignores RUSTUP_TOOLCHAIN.
if [[ $RUSTUP_TOOLCHAIN =~ ^[0-9]+\.[0-9]+\.[0-9]+$ && $(rustc -V) != "rustc $RUSTUP_TOOLCHAIN "* ]]; then
  echo "error: rustc on PATH is $(rustc -V), not $RUSTUP_TOOLCHAIN (put the rustup proxies first on PATH)" >&2
  exit 1
fi
if ((profdata_llvm > rustc_llvm)); then
  echo "error: llvm-profdata (LLVM $profdata_llvm) is newer than rustc's LLVM $rustc_llvm" >&2
  exit 1
fi
if [[ $bolt == 1 ]]; then
  for tool in llvm-bolt perf2bolt merge-fdata llvm-objcopy perf; do
    command -v "$tool" > /dev/null || { echo "error: $tool not found (or set RELEASE_BOLT=0)" >&2; exit 1; }
  done
  perf record -q -e cycles:u -j any,u -o /dev/null -- true 2> /dev/null \
    || { echo "error: perf branch sampling (-j any,u) does not work here (or set RELEASE_BOLT=0)" >&2; exit 1; }
fi

command -v qemu-x86_64 > /dev/null || { echo "error: qemu-x86_64 not found (package qemu-user); step 6 needs it" >&2; exit 1; }
command -v python3 > /dev/null || { echo "error: python3 not found; check_np_config and the editor sessions need it" >&2; exit 1; }

# floor_sysroot <dir>: makes the glibc 2.28 sysroot of the dynamic build in
# <dir>, from Arch Linux packages of April 2019 (built for plain x86-64):
# glibc 2.28 (headers, libc.so.6, crt files), linux-api-headers 5.0.7 (kernel
# headers for the jemalloc C code) and gcc-libs 8.3.0 (libgcc_s, which rustc
# links). The sha256 pins fix the files. Their signatures were checked with
# pacman-key when the pins were written (2026-09-27). lib and lib64 point at
# usr/lib, so `qemu-x86_64 -L <dir>` loads this glibc (step 6).
floor_sysroot() {
  local dir=$1 sum pkg
  [[ -f $dir/.complete ]] && return
  rm -rf "$dir"
  mkdir -p "$dir/pkgs"
  while read -r sum pkg; do
    curl -sfL -o "$dir/pkgs/${pkg##*/}" "https://archive.archlinux.org/packages/$pkg" \
      || { echo "error: cannot download $pkg" >&2; exit 1; }
    echo "$sum  $dir/pkgs/${pkg##*/}" | sha256sum -c --quiet - || { echo "error: $pkg does not match its sha256" >&2; exit 1; }
  done << 'PKGS'
34fa06bac690f62f7167ef68c04978cad90d0fcf505abbac2bd563f20ee0803b g/glibc/glibc-2.28-6-x86_64.pkg.tar.xz
5c891731216b2752ddd15cd5851216d85e9a17dd5807ba02b88ff6597909e805 l/linux-api-headers/linux-api-headers-5.0.7-1-any.pkg.tar.xz
400e2ecb1b2dfb40e09cdb6805f0075cbc88e6fcef9b73f23c64a6e709dcd61b g/gcc-libs/gcc-libs-8.3.0-1-x86_64.pkg.tar.xz
PKGS
  tar -C "$dir" -xf "$dir/pkgs/glibc-2.28-6-x86_64.pkg.tar.xz" --exclude=usr/lib/getconf usr/include usr/lib
  tar -C "$dir" -xf "$dir/pkgs/linux-api-headers-5.0.7-1-any.pkg.tar.xz" usr/include
  tar -C "$dir" -xf "$dir/pkgs/gcc-libs-8.3.0-1-x86_64.pkg.tar.xz" --wildcards 'usr/lib/libgcc_s.so*'
  ln -s usr/lib "$dir/lib"
  ln -s usr/lib "$dir/lib64"
  touch "$dir/.complete"
}

link_flags=""
# CFLAGS_<target> for the C code (jemalloc), in both builds.
c_env=()
glibc_root=""
if [[ $libc == musl ]]; then
  # Static by default. rustc links its own musl libc.a, crt files and
  # libunwind (self-contained), so only the jemalloc C code needs musl-gcc.
  c_env=("CC_${target//-/_}=musl-gcc")
elif [[ $static == 1 ]]; then
  link_flags="-C target-feature=+crt-static"
  if [[ -n ${RELEASE_SYSROOT:-} ]]; then
    RELEASE_SYSROOT="$(cd -- "$RELEASE_SYSROOT" && pwd)"
    # -B and -L come before gcc's own paths, so the linker takes the startup
    # files, libc.a and libgcc from here. --sysroot makes the absolute paths
    # in linker scripts (libm.a is one) point into it too.
    # The same libgcc dir as the host cc uses (not a multilib dir like 32/).
    libgcc_dir="$RELEASE_SYSROOT$(dirname "$(cc -print-libgcc-file-name)")"
    [[ -f $RELEASE_SYSROOT/usr/lib/libc.a && -f $libgcc_dir/libgcc_eh.a ]] \
      || { echo "error: RELEASE_SYSROOT needs usr/lib/libc.a and $libgcc_dir/libgcc_eh.a" >&2; exit 1; }
    link_flags+=" -C link-arg=--sysroot=$RELEASE_SYSROOT"
    for d in "$RELEASE_SYSROOT/usr/lib" "$libgcc_dir"; do
      link_flags+=" -C link-arg=-B$d -C link-arg=-L$d"
    done
  fi
else
  glibc_root="${RELEASE_SYSROOT:-$data_root/target/goport-release-sysroot/glibc-2.28}"
  [[ -n ${RELEASE_SYSROOT:-} ]] || floor_sysroot "$glibc_root"
  glibc_root="$(cd -- "$glibc_root" && pwd)"
  [[ -f $glibc_root/usr/lib/libc.so.6 && -f $glibc_root/usr/include/stdio.h && -e $glibc_root/lib64/ld-linux-x86-64.so.2 ]] \
    || { echo "error: $glibc_root needs usr/lib/libc.so.6, usr/include/stdio.h and lib64/ld-linux-x86-64.so.2" >&2; exit 1; }
  # -B and -L come before gcc's own paths (gcc otherwise also searches the
  # host /usr/lib), so crt files, libc.so, libgcc_s.so come from here.
  # --sysroot sends the absolute paths in libc.so (a linker script) and the
  # C headers here too.
  floor_flags="--sysroot=$glibc_root -B$glibc_root/usr/lib -L$glibc_root/usr/lib"
  for f in $floor_flags; do link_flags+=" -C link-arg=$f"; done
  c_env=("CFLAGS_${host//-/_}=$floor_flags")
fi
# No PIE: the code uses fixed addresses, so the bin has almost no relocations
# to apply at start and no .data.rel.ro pages to write.
[[ $pie == 1 ]] || link_flags+=" -C relocation-model=static"

cargo_cmd=(cargo)
[[ -x "$repo/scripts/run-cargo-capped.sh" ]] && cargo_cmd=("$repo/scripts/run-cargo-capped.sh")
# Both builds need the same features, or the profile does not match the code.
feature_args=()
[[ $jemalloc == 1 ]] || feature_args+=(--no-default-features)
[[ -n $features ]] && feature_args+=(--features "$features")

# malloc_env <bin>: with jemalloc, the _RJEM_MALLOC_CONF=value that <bin>
# sets before it execs itself (its JEMALLOC_CONF line), or nothing for a bin
# that does not exec itself. With glibc malloc, nothing (see the rules above).
malloc_env() {
  local src="$repo/crates/ts_goport/src/bin/$1.rs"
  if [[ $jemalloc == 1 ]]; then
    sed -n 's/^const JEMALLOC_CONF: &str = "\([^"]*\)";$/_RJEM_MALLOC_CONF=\1/p' "$src" 2> /dev/null | head -1 || true
  fi
}

# jemalloc: build JEMALLOC_CONF into jemalloc (tikv-jemalloc-sys --with-malloc-conf) for the
# shipped bins only, so they do not exec themselves at start. Dev and evidence builds leave it
# unset and keep the exec (the same allocator settings, one more execve).
if [[ $jemalloc == 1 ]]; then
  JEMALLOC_SYS_WITH_MALLOC_CONF="$(malloc_env tsgo)"
  export JEMALLOC_SYS_WITH_MALLOC_CONF="${JEMALLOC_SYS_WITH_MALLOC_CONF#_RJEM_MALLOC_CONF=}"
  [[ -n $JEMALLOC_SYS_WITH_MALLOC_CONF ]] || { echo "error: no JEMALLOC_CONF line in bin/tsgo.rs" >&2; exit 1; }
fi

# libs_to <dir>: a noembed bin reads the lib files next to it, so each dir
# that runs one gets them.
libs_to() {
  if [[ $noembed == 1 ]]; then "$script_dir/copy-libs.sh" "$1"; fi
}

# build <target-subdir> <rustflags> <bin>...: goport profile, own target dir.
# sccache is off: it could reuse an object built with an older profile.
build() {
  local name=$1 flags=$2 bin_args=() b
  shift 2
  for b in "$@"; do bin_args+=(--bin "$b"); done
  echo "== build $name ($flags) ${feature_args[*]}"
  env "${c_env[@]}" CARGO_TARGET_DIR="$out/$name" TS_CARGO_SEPARATE_TARGET=1 TS_CARGO_SCCACHE=0 RUSTFLAGS="$flags" \
    "${cargo_cmd[@]}" build --profile goport --offline --locked -p ts_goport --target "$target" "${feature_args[@]}" "${bin_args[@]}" \
    > "$out/build-$name.log" 2>&1 || { tail -20 "$out/build-$name.log" >&2; exit 1; }
}

P="$data_root/target/project-inputs"
declare -A projects=(
  [query]="$P/query/source/packages/query-core/tsconfig.prod.json"
  [hono]="$P/hono/source/tsconfig.build.json"
  [zod]="$P/zod/source/packages/zod/tsconfig.json"
  [effect]="$P/effect/source/packages/effect/tsconfig.json"
  [elysia]="$data_root/target/project-inputs-extra/elysia/src/tsconfig.json"
)
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

# The effect project trains twice, in PGO and BOLT (CLI runs and editor
# sessions): as the input, whose tsconfig.base.json lists the
# @effect/language-service plugin, so the Effect rules run; and as effectnp,
# without the plugin entry, as most projects are (pgotrain2, see the header).
# effect_np <dir> makes <dir> an inputs dir (like $P, with only the effect
# project) for effectnp. A child config cannot remove the plugin (the Effect
# options merge across extends), so <dir>/effect/source/tsconfig.base.json is
# a copy of the input's without the "plugins" lines (a trailing comma stays;
# tsconfig allows it). Every other entry links to the input, and the paths stay
# under <dir>, so the config finds the same files (${configDir}, type roots,
# node_modules) and the editor sessions open them there. The awk edit fits the
# input's format only, so check_np_config checks the copy.
effect_np() {
  local np=$1/effect/source src=$P/effect/source e
  mkdir -p "$np/packages/effect"
  for e in "$src"/* "$src"/.[!.]*; do
    [[ -e $e ]] || continue
    case ${e##*/} in packages | tsconfig.base.json) ;; *) ln -s "$e" "$np/" ;; esac
  done
  for e in "$src"/packages/*; do
    [[ ${e##*/} == effect ]] || ln -s "$e" "$np/packages/"
  done
  for e in "$src"/packages/effect/* "$src"/packages/effect/.[!.]*; do
    [[ -e $e ]] || continue
    ln -s "$e" "$np/packages/effect/"
  done
  awk '/"plugins": \[/ { skip = 1 } !skip { print } skip && /^    \}\]/ { skip = 0 }' \
    "$src/tsconfig.base.json" > "$np/tsconfig.base.json"
  check_np_config "$src/tsconfig.base.json" "$np/tsconfig.base.json" || exit 1
}

# check_np_config <input> <copy> fails unless both parse as tsconfig JSON
# (comments and trailing commas allowed), the input lists the
# @effect/language-service plugin, and the copy equals the input without
# compilerOptions.plugins. Else effectnp would train on a broken config (tsgo
# reports it and checks less), or on the config of effect, with no error.
check_np_config() {
  python3 - "$1" "$2" << 'PY'
import json, re, sys

STRING = r'"(?:\\.|[^"\\])*"'


def parse(path):
    # Drop the comments, then the trailing commas, outside strings.
    keep = lambda m: m.group(0) if m.group(0).startswith('"') else ""
    text = open(path, encoding="utf-8").read()
    text = re.sub(STRING + r"|//[^\n]*|/\*.*?\*/", keep, text, flags=re.S)
    text = re.sub(STRING + r"|,(?=\s*[}\]])", keep, text)
    try:
        return json.loads(text)
    except ValueError as err:
        sys.exit(f"error: {path} does not parse as tsconfig JSON: {err}")


src, copy = sys.argv[1], sys.argv[2]
want, got = parse(src), parse(copy)
plugins = want.get("compilerOptions", {}).pop("plugins", None) or []
if not any(isinstance(p, dict) and p.get("name") == "@effect/language-service" for p in plugins):
    sys.exit(f"error: {src} lists no @effect/language-service plugin")
if got != want:
    sys.exit(f"error: {copy} is not {src} without compilerOptions.plugins (its format changed?)")
PY
}
effect_np "$tmp/np"
projects[effectnp]="$tmp/np/effect/source/packages/effect/tsconfig.json"

# 1. Instrumented build. Training runs only these three bins; the other two
# share their code, so they use the same profile.
profiles="$out/profiles"
rm -rf "$profiles"
mkdir -p "$profiles"
build target-gen "-Cprofile-generate=$profiles $link_flags" goport tsgo goport_emit
gen="$out/target-gen/$target/goport"
libs_to "$gen"

# 2. PGO training. Exit codes 1 and 2 are expected: some inputs have diagnostics on
# purpose. A run killed by a signal (a crash, a stack overflow) writes no profile, so
# it stops the build: a release trained on part of the runs is slower and looks fine.
killed=0
train() {
  local rc=0
  "$@" > /dev/null 2>&1 || rc=$?
  if ((rc > 128 && rc != 124)); then
    killed=$((killed + 1))
    echo "training run killed (exit $rc): $*" >&2
  fi
}
for name in query hono zod effect effectnp elysia; do
  train "$gen/goport" -p "${projects[$name]}"
  train "$gen/tsgo" -p "${projects[$name]}" --noEmit --tsBuildInfoFile "$tmp/$name.tsbuildinfo"
done
for name in query hono; do
  train "$gen/goport_emit" -p "${projects[$name]}" --outDir "$tmp/out"
  rm -rf "$tmp/out"
done
cases="$data_root/target/continuation-r97-goport/corpus-full/cases"
n=0 i=0
for dir in "$cases"/*/; do
  if ((i++ % corpus_step == 0)) && [[ -f "$dir/tsconfig.json" ]]; then
    train env -C "$dir" timeout 60 "$gen/goport" -p tsconfig.json
    n=$((n + 1))
  fi
done
nprof=$(find "$profiles" -name '*.profraw' | wc -l)
echo "trained on 5 projects (effect with and without the plugin) and $n corpus cases, $nprof profraw files"
if ((killed > 0)); then
  echo "error: $killed PGO training runs were killed by a signal (see above)" >&2
  exit 1
fi
if ((nprof < 3)); then
  echo "error: $nprof profraw files, expected one per trained bin (goport, tsgo, goport_emit)" >&2
  exit 1
fi

# 3. Merge. The file name holds the profile hash: cargo does not track the
# profile content, but it rebuilds when RUSTFLAGS change.
rm -f "$out"/goport-*.profdata
"$profdata" merge -o "$tmp/merged.profdata" "$profiles"
merged="$out/goport-$(sha256sum "$tmp/merged.profdata" | cut -c1-12).profdata"
mv "$tmp/merged.profdata" "$merged"

# 4. Optimized build. rustc only warns when it cannot read the profile.
build target-use "-Cprofile-use=$merged -C link-arg=-Wl,--emit-relocs $link_flags" "${shipped[@]}"
if grep -q "profile format version\|profile-use" "$out/build-target-use.log"; then
  grep "profile format version\|profile-use" "$out/build-target-use.log" | head -3 >&2
  echo "error: rustc did not use the profile" >&2
  exit 1
fi
use="$out/target-use/$target/goport"
libs_to "$use"
if [[ -n $version && $("$use/tsgo" -v) != "Version $version" ]]; then
  echo "error: $use/tsgo reports $("$use/tsgo" -v), not Version $version (RELEASE_VERSION)" >&2
  exit 1
fi
# jemalloc: the bins do not exec themselves at start when jemalloc has their
# JEMALLOC_CONF built in (set above). jemalloc prints the built-in
# value (config.malloc_conf) with its exit stats.
if [[ $jemalloc == 1 ]]; then
  conf="$(malloc_env tsgo)"
  conf="${conf#_RJEM_MALLOC_CONF=}"
  stats="$(_RJEM_MALLOC_CONF=stats_print:true,stats_print_opts:mdablxe "$use/tsgo" --version 2>&1 || true)"
  if [[ -z $conf || $stats != *"config.malloc_conf: \"$conf\""* ]]; then
    echo "error: the jemalloc of $use/tsgo does not have \"$conf\" built in; JEMALLOC_SYS_WITH_MALLOC_CONF (set from bin/tsgo.rs above) did not reach tikv-jemalloc-sys" >&2
    exit 1
  fi
fi
if [[ $static == 1 ]] && readelf -d "$use/tsgo" | grep -q NEEDED; then
  echo "error: $use/tsgo still loads shared libraries" >&2
  exit 1
fi
# glibc floor: no bin may need a GLIBC_ symbol version above the floor. BOLT
# does not change the dynamic symbols, so the check runs here, before it.
if [[ $static != 1 ]]; then
  for b in "${shipped[@]}"; do
    need="$(objdump -T "$use/$b" | grep -o 'GLIBC_[0-9][0-9.]*' | sort -uV | tail -1)"
    [[ -n $need ]] || { echo "error: objdump -T shows no GLIBC_ version in $use/$b" >&2; exit 1; }
    if [[ $(printf '%s\n' "$need" "GLIBC_$glibc_floor" | sort -V | tail -1) != "GLIBC_$glibc_floor" ]]; then
      echo "error: $b needs $need, above the glibc floor $glibc_floor: $(objdump -T "$use/$b" | grep "($need)" | awk '{print $NF}' | head -5 | xargs)" >&2
      exit 1
    fi
    # -z pack-relative-relocs adds GLIBC_ABI_DT_RELR (glibc 2.36 and later),
    # which objdump -T does not list.
    if readelf -VW "$use/$b" | grep -q GLIBC_ABI_DT_RELR; then
      echo "error: $b needs GLIBC_ABI_DT_RELR (glibc 2.36), above the glibc floor $glibc_floor" >&2
      exit 1
    fi
    echo "glibc floor: $b needs $need at most"
  done
fi
# CPU check, part a: a static bin carries its own libc code, and a jemalloc
# bin its own C allocator. That code must run on any x86-64 CPU. No AVX or BMI
# instruction may appear in non-Rust code, except in the variants that glibc
# picks at run time (names with avx, evex, fma or xsave). Rust code is built
# for plain x86-64 and checks the CPU itself.
if [[ $static == 1 || $jemalloc == 1 ]]; then
  for b in "${shipped[@]}"; do
    objdump -d --no-show-raw-insn "$use/$b" | awk '
      /^[0-9a-f]+ <.*>:$/ { fn = substr($2, 2, length($2) - 3); next }
      fn !~ /^_(ZN|R)/ && fn !~ /avx|evex|fma|xsave/ &&
        /\t(v[a-z0-9]+|andn|bextr|blsi|blsmsk|blsr|bzhi|pdep|pext|mulx|rorx|sarx|shlx|shrx|k[a-z]+[bwdq]) / { bad[fn]++ }
      END { for (f in bad) print f }' > "$out/cpu-check-$b.txt"
    if [[ -s $out/cpu-check-$b.txt ]]; then
      echo "error: $b has AVX or BMI code in $(wc -l < "$out/cpu-check-$b.txt") non-Rust functions, for example $(sort "$out/cpu-check-$b.txt" | head -3 | xargs)." >&2
      echo "The static libc or libgcc (see RELEASE_SYSROOT) or jemalloc (see CFLAGS) is not built for plain x86-64." >&2
      exit 1
    fi
  done
fi
rm -rf "${out:?}/bin"
mkdir -p "$out/bin"
libs_to "$out/bin"
if [[ $bolt != 1 ]]; then
  for b in "${shipped[@]}"; do cp "$use/$b" "$out/bin/$b"; done
fi

# 5. BOLT. No -use-gnu-stack: it drops PT_GNU_STACK (see the rules above).
bolt_opts=(-reorder-blocks=ext-tsp -reorder-functions=cdsort -split-functions -split-all-cold -split-eh)
p2b_opts=()
if [[ $static == 1 && $libc == gnu ]]; then
  # The static libgcc unwinder has jump tables that point into ".cold" parts
  # that BOLT cannot tie to their parent (3 local copies of
  # read_encoded_value_with_base). perf2bolt is strict by default and stops on
  # them. Keep these functions where they are. They only run on a panic.
  p2b_opts=(--strict=0)
  bolt_opts+=("-skip-funcs=read_encoded_value_with_base.*,linear_search_fdes.*,fde_single_encoding_extract.*,fde_mixed_encoding_extract.*")
fi
bolt_dir="$out/bolt"
declare -A reps=([query]=12 [hono]=6 [zod]=4 [effect]=3 [effectnp]=3) # about 5 s of work per recording

# rec <name> <reps> <cmd...>: one perf recording of <reps> runs, each without
# old .tsbuildinfo or emit output in $tmp. freq (default BOLT_PERF_FREQ) sets
# the sample frequency.
rec() {
  local name=$1 count=$2
  shift 2
  perf record -q -e cycles:u -j any,u -F "${freq:-${BOLT_PERF_FREQ:-20000}}" -o "$bolt_dir/data/$name.data" -- bash -c \
    'n=$1; t=$2; shift 2; for ((i = 0; i < n; i++)); do rm -rf "$t"/*.tsbuildinfo "$t"/out-*; "$@"; done; true' \
    _ "$count" "$tmp" "$@" > "$bolt_dir/data/$name.out" 2>&1 || true
  [[ -s "$bolt_dir/data/$name.data" ]] || { echo "error: no perf data for $name" >&2; exit 1; }
}

# lsp_rec <name> <bin> <inputs-dir> <session>...: one recording of editor
# sessions (lsp-train.py) at the lower sample frequency.
lsp_rec() {
  local name=$1 b=$2
  shift 2
  freq=${BOLT_LSP_PERF_FREQ:-2500} rec "$name" 1 python3 "$script_dir/lsp-train.py" "$b" "$@"
  grep -qx 'lsp-train: ok' "$bolt_dir/data/$name.out" \
    || { tail -5 "$bolt_dir/data/$name.out" >&2; echo "error: the editor sessions of the BOLT training failed" >&2; exit 1; }
}

# bolt_train <bin> <path>: the training runs of one bin.
bolt_train() {
  local b=$2 name src dst e
  case $1 in
    tsgo)
      for name in query hono zod effect effectnp; do
        rec "tsgo-$name" "${reps[$name]}" "$b" -p "${projects[$name]}" --noEmit --pretty false --tsBuildInfoFile "$tmp/$name.tsbuildinfo"
      done
      for name in query hono; do
        rec "tsgo-emit-$name" "${reps[$name]}" "$b" -p "${projects[$name]}" --pretty false --outDir "$tmp/out-$name" --tsBuildInfoFile "$tmp/emit-$name.tsbuildinfo"
      done
      # The editor sessions, at a lower sample frequency. The effect sessions
      # run again without the plugin, in a second recording (tsgo-lsp-np).
      if [[ -n $lsp_sessions ]]; then
        local lsp_np=() s
        for s in $lsp_sessions; do [[ ${s%%:*} != effect ]] || lsp_np+=("$s"); done
        # shellcheck disable=SC2086 # one argument per session
        lsp_rec tsgo-lsp "$b" "$P" $lsp_sessions
        if ((${#lsp_np[@]})); then lsp_rec tsgo-lsp-np "$b" "$tmp/np" "${lsp_np[@]}"; fi
      fi ;;
    goport)
      for name in query hono zod effect effectnp; do rec "goport-$name" "${reps[$name]}" "$b" -p "${projects[$name]}"; done ;;
    goport_emit)
      for name in query hono; do rec "goport_emit-$name" "${reps[$name]}" "$b" -p "${projects[$name]}" --outDir "$tmp/out-$name"; done ;;
    goport_typesyms)
      for name in query hono; do rec "goport_typesyms-$name" "${reps[$name]}" "$b" -p "${projects[$name]}" -o "$tmp/out-$name"; done ;;
    goport_build)
      # Writable copy of the query monorepo; the top-level node_modules is a link.
      src="$P/query/source" dst="$tmp/query-build"
      mkdir -p "$dst"
      for e in "$src"/* "$src"/.[!.]*; do
        [[ -e $e || -L $e ]] || continue
        if [[ $(basename "$e") == node_modules && -d $e && ! -L $e ]]; then ln -s "$e" "$dst/node_modules"; else cp -a "$e" "$dst/"; fi
      done
      chmod -R u+w "$dst"
      find "$dst" -path '*/node_modules' -prune -o -name '*.tsbuildinfo' -print0 | xargs -0 -r rm -f
      (cd "$dst" && rec goport_build-query-chain 1 "$b" -b packages/query-sync-storage-persister/tsconfig.json)
      rm -rf "$dst" ;;
  esac
}

if [[ $bolt == 1 ]]; then
  rm -rf "$bolt_dir"
  mkdir -p "$bolt_dir/input" "$bolt_dir/data"
  libs_to "$bolt_dir/input"
  for b in "${shipped[@]}"; do
    # BOLT reads a symbol with ".cold" or ".warm" in it as a split fragment and
    # stops ("parent function not found"). Rust paths make such names
    # ("Session..warm_auto_import_cache"). Rename them in a copy: "..warm" to
    # "..Warm" (same length, only .symtab changes).
    nm "$use/$b" | awk '$2 ~ /^[tT]$/ && $3 ~ /\.\.(cold|warm)/ {r = $3; gsub(/\.\.cold/, "..Cold", r); gsub(/\.\.warm/, "..Warm", r); print $3, r}' > "$bolt_dir/$b.rename.txt"
    llvm-objcopy --redefine-syms="$bolt_dir/$b.rename.txt" "$use/$b" "$bolt_dir/input/$b"
    # The bin sets its malloc variable and execs itself unless the caller set it.
    unset GLIBC_TUNABLES _RJEM_MALLOC_CONF
    tunables="$(malloc_env "$b")"
    [[ -n $tunables ]] && export "${tunables?}"
    bolt_train "$b" "$bolt_dir/input/$b"
    for d in "$bolt_dir/data/$b"-*.data; do
      perf2bolt "${p2b_opts[@]}" -p "$d" -o "${d%.data}.fdata" "$bolt_dir/input/$b" > "${d%.data}.p2b.log" 2>&1 \
        || { tail -5 "${d%.data}.p2b.log" >&2; exit 1; }
      # Samples in the kernel (and, when dynamic, in libc) count as unknown:
      # 10 to 25%. Most of them unknown means the samples did not map to the bin.
      pct="$(sed -n 's/.*involving unknown regions: [0-9]* (\([0-9]*\)\..*/\1/p' "${d%.data}.p2b.log")"
      [[ -n $pct ]] && ((pct < 50)) || { echo "error: the samples in $d do not map to $b" >&2; exit 1; }
    done
    merge-fdata "$bolt_dir/data/$b"-*.fdata > "$bolt_dir/$b.fdata" 2> "$bolt_dir/$b.merge.log"
    llvm-bolt "$bolt_dir/input/$b" -o "$out/bin/$b" -data="$bolt_dir/$b.fdata" "${bolt_opts[@]}" > "$bolt_dir/$b.bolt.log" 2>&1 \
      || { tail -20 "$bolt_dir/$b.bolt.log" >&2; exit 1; }
    echo "bolt $b: $(grep -m1 -o '[0-9]* out of [0-9]* functions in the binary ([0-9.]*%) have non-empty execution profile' "$bolt_dir/$b.bolt.log" || true)"
  done
  unset GLIBC_TUNABLES _RJEM_MALLOC_CONF
fi

# check_headers <bin>: fails unless the bin has one PT_GNU_STACK and it is RW
# (no E), and unless the program header table is where both old and new
# kernels look for it: at the first LOAD's address + e_phoff (kernels before
# 5.18) and at the PT_PHDR address (5.18 and later).
check_headers() {
  local f=$1 hdrs stack phoff phdr load
  hdrs="$(readelf -lW "$f")"
  stack="$(awk '$1 == "GNU_STACK" { s = ""; for (i = 7; i < NF; i++) s = s $i; print s }' <<< "$hdrs")"
  if [[ $stack != RW ]]; then
    [[ -n $stack ]] && stack="PT_GNU_STACK flags ${stack//$'\n'/ and }" || stack="no PT_GNU_STACK"
    echo "error: $f has $stack, not one RW PT_GNU_STACK. glibc then maps thread stacks executable." >&2
    exit 1
  fi
  phoff="$(sed -n 's/.*starting at offset \([0-9]*\)$/\1/p' <<< "$hdrs")"
  read -r -a phdr <<< "$(awk '$1 == "PHDR" { print $2, $3; exit }' <<< "$hdrs")"
  read -r -a load <<< "$(awk '$1 == "LOAD" { print $2, $3; exit }' <<< "$hdrs")"
  if ((${#phdr[@]} == 2 && phoff + load[1] - load[0] != phdr[1])); then
    echo "error: $f has its program header table at file offset $(printf '%#x' "$phoff"), but PT_PHDR says address $(printf '%#x' "${phdr[1]}"). Kernels before 5.18 would read the wrong table (was the bin stripped?)." >&2
    exit 1
  fi
}
for b in "${shipped[@]}"; do check_headers "$out/bin/$b"; done
echo "headers: all bins have a RW PT_GNU_STACK and a program header table that old kernels find"

# 6. CPU check, part b: tsgo on query and hono in qemu with a CPU model
# without AVX (qemu64). A dynamic tsgo loads the glibc 2.28 of the sysroot
# there (qemu -L; its libs are built for plain x86-64), so this also checks
# that it starts on the floor glibc. The run must give the same output and
# exit code as a native run: a loader error ("version `GLIBC_2.xx' not
# found") exits 1, which is also a tsgo exit code. The malloc variable is set
# so the run stays in qemu: an exec would run the new image on the host CPU.
# Any GLIBC_TUNABLES value stops the glibc re-exec.
tunables="$(malloc_env tsgo)"
[[ $jemalloc == 1 ]] || tunables="GLIBC_TUNABLES=glibc.malloc.hugetlb=1"
qemu=(qemu-x86_64 -cpu qemu64)
[[ $static == 1 ]] || qemu+=(-L "$glibc_root")
# check_run <name> <log> <cmd...>: runs tsgo on project <name>, prints the
# exit code.
check_run() {
  local name=$1 log=$2 code=0
  shift 2
  rm -f "$tmp/cpu-check.tsbuildinfo"
  # No core file: qemu would write it to the current dir on a crash.
  (ulimit -c 0 && exec env ${tunables:+"$tunables"} "$@" -p "${projects[$name]}" \
    --noEmit --pretty false --tsBuildInfoFile "$tmp/cpu-check.tsbuildinfo") > "$log" 2>&1 || code=$?
  echo "$code"
}
for name in query hono; do
  log="$out/cpu-check-$name"
  native="$(check_run "$name" "$log-native.log" "$out/bin/tsgo")"
  emulated="$(check_run "$name" "$log-qemu64.log" "${qemu[@]}" "$out/bin/tsgo")"
  if ((emulated >= 128)); then
    echo "error: tsgo on $name stops with signal $((emulated - 128)) on a plain x86-64 CPU (4 = illegal instruction); see RELEASE_SYSROOT and $log-qemu64.log" >&2
    exit 1
  fi
  if [[ $emulated != "$native" ]] || ! cmp -s "$log-native.log" "$log-qemu64.log"; then
    echo "error: tsgo on $name in qemu64${glibc_root:+ on glibc $glibc_floor} exits $emulated (native $native), or its output differs; see $log-*.log" >&2
    exit 1
  fi
  # jemalloc prints "<jemalloc>: ..." to stderr for a setting that it does not
  # support (thp:always needs madvise(MADV_HUGEPAGE) at build time).
  if grep -q '<jemalloc>' "$log-native.log"; then
    echo "error: jemalloc warns on $name: $(grep -m1 '<jemalloc>' "$log-native.log")" >&2
    exit 1
  fi
  echo "cpu check: tsgo on $name in qemu64${glibc_root:+ on glibc $glibc_floor} exit $emulated, same output as native"
done

{
  echo "source: $(git -C "$repo" rev-parse HEAD)$(git -C "$repo" diff --quiet HEAD -- crates Cargo.toml Cargo.lock ':(exclude,glob)crates/*/scripts/**' || echo ' (dirty)')"
  echo "rustc: $(rustc -V), target $target, cargo profile goport"
  echo "pgo: $merged, trained on 5 projects (effect with and without the plugin) and $n corpus cases"
  echo "pie: $pie"
  if [[ $libc == musl ]]; then
    echo "libc: static musl (rustc self-contained)"
  elif [[ $static == 1 ]]; then
    echo "static glibc: 1${RELEASE_SYSROOT:+, sysroot $RELEASE_SYSROOT}"
  else
    echo "static glibc: 0, glibc floor $glibc_floor, sysroot $glibc_root"
  fi
  echo "allocator: $([[ $jemalloc == 1 ]] && echo "jemalloc, built-in $(malloc_env tsgo)" || echo "glibc malloc")"
  echo "cargo feature args: ${feature_args[*]:-none}"
  echo "version: $("$out/bin/tsgo" -v) (RELEASE_VERSION ${version:-unset})"
  echo "lib files: $([[ $noembed == 1 ]] && echo "next to the bins (noembed)" || echo "embedded")"
  if [[ $bolt == 1 ]]; then
    echo "bolt: $(llvm-bolt --version | grep -m1 'LLVM version' | xargs), ${bolt_opts[*]}"
    echo "bolt perf: -F ${BOLT_PERF_FREQ:-20000}, kernel.perf_event_max_sample_rate $(cat /proc/sys/kernel/perf_event_max_sample_rate 2> /dev/null || echo unknown) at the end"
    if [[ -n $lsp_sessions ]]; then
      echo "bolt editor sessions (tsgo, ${BOLT_LSP_PERF_FREQ:-2500} Hz): $({ sed -n 's/^lsp-train: \(.*\): [0-9]* of .*/\1/p' "$bolt_dir/data/tsgo-lsp.out"; sed -n 's/^lsp-train: \(.*\): [0-9]* of .*/\1 (no plugin)/p' "$bolt_dir/data/tsgo-lsp-np.out"; } 2> /dev/null | paste -sd';' | sed 's/;/; /g'); ls_edit_bench.py sha256 $(sha256sum "$repo/scripts/goport/ls_edit_bench.py" | cut -c1-12)"
    fi
  else
    echo "bolt: off"
  fi
} > "$out/bin/BUILD.txt"
echo "release binaries: $out/bin/{$(IFS=,; echo "${shipped[*]}")}"
