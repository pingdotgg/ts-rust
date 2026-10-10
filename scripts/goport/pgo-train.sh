#!/usr/bin/env bash
# The PGO training set of the shipped tsc (bin/tsgo.rs). The release workflow
# (.github/workflows/release.yml) trains on it on each platform, through
# crates/ts_goport/scripts/build-pgo.sh PGO_TRAIN, and checks the PGO tsc with `compare`.
#
# usage: pgo-train.sh setup <dir>                  fetch the projects into <dir> (needs git and npm)
#        pgo-train.sh run <dir> <tsc>              run the training set with <tsc>
#        pgo-train.sh compare <dir> <tsc-a> <tsc-b>
#                                                  run the set with both; fail when the stdout, stderr,
#                                                  exit code or emitted files of any run differ
#   <tsc> is a tsc bin, or a dir that holds tsgo (build-pgo.sh passes its bin dir). On Windows
#   (Git Bash) the bins are tsc.exe and tsgo.exe, and node_modules is a junction.
#
# The set: the gate's 4 projects (query, hono, zod, effect) and 3 realworld4 configs (playcanvas:
# JS with JSDoc types and declaration emit; umami: a React TSX app; nestjs-cqrs: decorators and
# their metadata). Each is a git checkout at a fixed commit (only the dirs that its config reads).
# Next to it, npm installs the packages that the config reads from the project's node_modules
# (and every @types package it reads), at the versions of the project's lock file, without
# install scripts. <dir>/projects/<name>/node_modules links to them. npm --before gives every
# other package the newest version of the day the list was made, so each setup gets the same tree.
# The effect config lists the @effect/language-service plugin (its tsconfig.base.json), so the tsc
# runs the Effect rules there. effect-noplugin is the same checkout without that entry (no_plugin,
# for a name that ends in -noplugin), as most projects are: the set trains effect with and without
# the Effect rules.
#
# Each project runs `tsc -p <config> --noEmit`; the ones marked emit also run `tsc -p <config>`
# with --outDir. Output, emit and build info go to <dir>/out, so the runs only read the projects.
# Exit codes 1 and 2 are expected (some inputs have diagnostics). Any other exit code stops the
# script: a run killed by a signal writes no profile, and a release trained on part of the set
# looks fine but is slower.
#
# A noembed tsc reads the lib files next to it, so each run copies <tsc> and the lib files to
# <dir>/bin. Both sides of compare run from that path, so lib paths in the output are equal.
# Written for bash 3.2 (macOS).
set -euo pipefail

usage() { sed -n '2,/^set -euo/p' "$0" | sed -e '$d' -e 's/^# \{0,1\}//'; exit 2; }
[[ $# -ge 2 ]] || usage
cmd=$1
dir=$2
repo="$(cd -- "$(dirname -- "$0")/../.." && pwd)"
exe=""
case "$(uname -s)" in MINGW* | MSYS* | CYGWIN*) exe=.exe ;; esac
# abs <dir>: the absolute path of <dir>; on Windows a C:/... path, which the tsc also gets.
abs() {
  if [[ -n $exe ]]; then cygpath -m "$(cd -- "$1" && pwd)"; else (cd -- "$1" && pwd); fi
}
# link_dir <target> <link>: a dir symlink; on Windows a junction, which needs no privilege (Git
# Bash ln -s copies the dir).
link_dir() {
  if [[ -n $exe ]]; then
    MSYS_NO_PATHCONV=1 cmd /c mklink /J "$(cygpath -w "$2")" "$(cygpath -w "$1")" > /dev/null
  else
    ln -s "$1" "$2"
  fi
}
# The day the package lists below were taken (npm --before).
npm_before=2026-10-07

# projects: one `project` call per project. Each command defines `project` first.
# project <name> <git url> <commit> <dirs to check out, comma-separated, or .> <config> <check|emit>
#         <npm packages>...
projects() {
  project query https://github.com/TanStack/query.git 44645e9eb1dafba5f2f229adb328582075484f36 \
    packages/query-core packages/query-core/tsconfig.prod.json emit \
    @types/node@22.19.15
  project hono https://github.com/honojs/hono.git 06880c4a2b04de9dd74217f26dd831209b9c01f1 \
    src tsconfig.build.json emit \
    buffer@5.7.1 @types/node@24.3.0 undici-types@7.10.0
  project zod https://github.com/colinhacks/zod.git 43f729db4aa0cedff6d6b3261f33f8556b3c7102 \
    packages/zod,.configs packages/zod/tsconfig.json check \
    esbuild@0.25.5 recheck@4.6.0-beta.3 rollup@4.60.2 @seriousme/openapi-schema-validator@2.9.0 \
    tinybench@2.9.0 @types/benchmark@2.1.5 @types/chai@5.2.3 @types/deep-eql@4.0.2 \
    @types/estree@1.0.8 @types/node@22.13.13 vite@7.3.2 vitest@4.1.5 @web-std/file@3.0.3
  project effect https://github.com/Effect-TS/effect.git 0d083ba26b2e1afec8d3e8d83db0d05683b6602b \
    packages/effect packages/effect/tsconfig.json check \
    @types/node@26.2.0
  project effect-noplugin https://github.com/Effect-TS/effect.git 0d083ba26b2e1afec8d3e8d83db0d05683b6602b \
    packages/effect packages/effect/tsconfig.json check \
    @types/node@26.2.0
  project playcanvas https://github.com/playcanvas/engine.git 6767f256721572a34f37daba419c18da8566f5ab \
    src tsconfig.build.json emit \
    fflate@0.8.3
  project umami https://github.com/umami-software/umami.git ec0ff50388c264ed8ce46f00967e92f7e71476ae \
    . tsconfig.json check \
    bcryptjs@3.0.3 chalk@5.6.2 chart.js@4.5.1 @clickhouse/client@1.23.1 colord@2.10.0 \
    date-fns@4.4.0 date-fns-tz@3.2.0 detect-browser@5.3.0 @dicebear/collection@9.4.3 \
    @dicebear/core@9.4.3 dotenv@17.4.2 esbuild@0.28.2 immer@11.1.18 ipaddr.js@2.5.0 isbot@5.2.2 \
    is-ci@4.1.0 is-docker@4.0.0 is-localhost-ip@3.0.1 jszip@3.10.2 kafkajs@2.2.4 \
    lucide-react@1.44.0 maxmind@5.0.7 motion@12.43.0 msw@2.15.0 next@16.3.4 next-intl@4.13.4 \
    node-fetch@3.3.2 openapi-typescript@7.13.0 otplib@13.5.0 postcss@8.5.28 prisma@7.10.0 \
    @prisma/adapter-pg@7.10.0 @prisma/client@7.10.0 @prisma/extension-read-replicas@0.5.0 \
    react-error-boundary@6.1.5 react-resizable-panels@4.12.4 react-window@2.3.1 redis@5.12.1 \
    rollup@4.63.1 @rollup/plugin-commonjs@29.0.3 @rollup/plugin-node-resolve@16.0.3 \
    @rollup/plugin-replace@6.0.3 @rollup/plugin-terser@1.0.0 @rollup/plugin-typescript@12.3.0 \
    rrweb@2.0.1 rrweb-player@2.0.0-alpha.20 serialize-error@13.0.1 @tanstack/react-query@5.102.8 \
    tar@7.5.22 @testing-library/jest-dom@7.0.1 @testing-library/react@16.3.3 \
    @testing-library/user-event@14.6.7 thenby@1.4.1 tsup@8.5.1 @types/aria-query@5.0.4 \
    @types/chai@5.2.3 typescript@6.0.3 @types/deep-eql@4.0.2 @types/estree@1.0.9 @types/jest@30.0.0 \
    @types/json-schema@7.0.15 @types/node@26.5.1 @types/pg@8.20.0 @types/qrcode@1.5.6 \
    @types/react@19.3.0 @types/react-dom@19.3.0 ua-parser-js@2.0.10 @umami/api-client@0.81.0 \
    @umami/react-zen@0.254.0 uuid@14.0.2 vitest@4.1.11 zod@4.6.2 zod-openapi@6.0.2 zustand@5.0.15
  project nestjs-cqrs https://github.com/nestjs/cqrs.git 55fb4a0f8d3dbc5c0f593916576173ab10d1dc2f \
    src,test test/tsconfig.json emit \
    assertion-error@2.0.1 expect-type@1.3.0 @nestjs/common@12.1.1 @nestjs/core@12.1.1 \
    @nestjs/testing@12.1.1 reflect-metadata@0.2.2 rxjs@7.8.2 @standard-schema/spec@1.1.0 \
    tinybench@2.9.0 tinyrainbow@3.1.0 @types/chai@5.2.3 @types/deep-eql@4.0.2 @types/node@24.19.0 \
    undici-types@7.24.6 vite@8.3.1 vitest@4.1.11 @vitest/expect@4.1.11 @vitest/mocker@4.1.11 \
    @vitest/pretty-format@4.1.11 @vitest/runner@4.1.11 @vitest/snapshot@4.1.11 @vitest/spy@4.1.11 \
    @vitest/utils@4.1.11
}

# no_plugin <tsconfig>: drops the "plugins" lines of the effect tsconfig.base.json (a trailing comma
# stays; tsconfig allows it). A child config cannot remove the plugin: the Effect options merge
# across extends.
no_plugin() {
  awk '/"plugins": \[/ { skip = 1 } !skip { print } skip && /^    \}\]/ { skip = 0 }' "$1" > "$1.tmp"
  mv "$1.tmp" "$1"
  if grep -q language-service "$1" || ! grep -q '"jsx"' "$1"; then
    echo "error: cannot drop the plugin entry of $1 (its format changed?)" >&2
    exit 1
  fi
}

# setup <dir>: a project that is already complete (deps/<name>/.done) is kept.
setup() {
  project() {
    local name=$1 url=$2 commit=$3 dirs=$4 p="$dir/projects/$1" deps="$dir/deps/$1"
    shift 6
    [[ -f $deps/.done ]] && return
    echo "setup $name"
    rm -rf "$p" "$deps"
    git init -q "$p"
    git -C "$p" remote add origin "$url"
    # Cone mode: the root files and these dirs. A blob:none fetch then gets only their files.
    # shellcheck disable=SC2086 # one word per dir
    [[ $dirs == . ]] || git -C "$p" sparse-checkout set ${dirs//,/ }
    GIT_TERMINAL_PROMPT=0 git -C "$p" fetch -q --depth 1 --filter=blob:none origin "$commit"
    git -C "$p" -c advice.detachedHead=false checkout -q FETCH_HEAD
    case $name in *-noplugin) no_plugin "$p/tsconfig.base.json" ;; esac
    mkdir -p "$deps"
    echo '{ "private": true }' > "$deps/package.json"
    (cd "$deps" && npm install --ignore-scripts --no-audit --no-fund --no-package-lock --legacy-peer-deps \
      --before="$npm_before" --loglevel=error "$@")
    link_dir "$deps/node_modules" "$p/node_modules"
    touch "$deps/.done"
  }
  projects
}

# run_set <tsc>: runs the set with <tsc>, into a new <dir>/out.
run_set() {
  local tsc=$1
  [[ -d $tsc ]] && tsc=$tsc/tsgo$exe
  [[ -x $tsc ]] || { echo "error: $tsc is not an executable tsc" >&2; exit 2; }
  rm -rf "${dir:?}/bin" "${dir:?}/out"
  mkdir -p "$dir/bin" "$dir/out"
  "$repo/crates/ts_goport/scripts/copy-libs.sh" "$dir/bin"
  cp "$tsc" "$dir/bin/tsc$exe"
  project() {
    local name=$1 config=$5 runs=$6 o="$dir/out/$1" s
    [[ -f $dir/deps/$name/.done ]] || { echo "error: $name is not set up (pgo-train.sh setup $dir)" >&2; exit 2; }
    mkdir -p "$o"
    s=$SECONDS
    one "$name" check -p "$config" --noEmit --pretty false --tsBuildInfoFile "$o/check.tsbuildinfo"
    if [[ $runs == emit ]]; then
      one "$name" emit -p "$config" --pretty false --outDir "$o/emit" --tsBuildInfoFile "$o/emit.tsbuildinfo"
    fi
    echo "train $name $runs $((SECONDS - s))s"
  }
  projects
}

# one <name> <run> <tsc args>...: runs the tsc in the project dir and keeps its stdout, stderr
# and exit code in <dir>/out/<name>/<run>.{out,err,rc}.
# Under build-pgo.sh (PGO_PROFILE_DIR set), each run writes its own profile file, and llvm-profdata
# merges them. rustc's default name (default_%m_%p.profraw) has the PID, and Windows can give the
# next run the same PID. That run then merges into the file in the profile runtime, which crashed
# (access violation) on arm64 Windows: the LLVM 22 runtime merge skips the padding after the
# bitmap bytes, and that section has 3 bytes there.
one() {
  local name=$1 run=$2 o="$dir/out/$1/$2" rc=0
  shift 2
  (
    cd "$dir/projects/$name"
    [[ -z ${PGO_PROFILE_DIR:-} ]] || export LLVM_PROFILE_FILE="$PGO_PROFILE_DIR/$name-$run-%p.profraw"
    "$dir/bin/tsc$exe" "$@"
  ) > "$o.out" 2> "$o.err" || rc=$?
  echo "$rc" > "$o.rc"
  if ((rc > 2)); then
    echo "error: tsc exited $rc on $name $run: $*" >&2
    tail -5 "$o.err" >&2
    exit 1
  fi
}

case $cmd in
  setup)
    [[ $# == 2 ]] || usage
    mkdir -p "$dir"
    dir="$(abs "$dir")"
    setup
    ;;
  run)
    [[ $# == 3 ]] || usage
    dir="$(abs "$dir")"
    run_set "$3"
    ;;
  compare)
    [[ $# == 4 ]] || usage
    dir="$(abs "$dir")"
    rm -rf "$dir/out-a" "$dir/out-b"
    run_set "$3"
    mv "$dir/out" "$dir/out-a"
    run_set "$4"
    mv "$dir/out" "$dir/out-b"
    if ! diff -r "$dir/out-a" "$dir/out-b" > "$dir/compare.diff"; then
      echo "error: $3 and $4 differ (diff -r $dir/out-a $dir/out-b):" >&2
      head -40 "$dir/compare.diff" >&2
      exit 1
    fi
    echo "same output: $(find "$dir/out-a" -type f | wc -l | tr -d ' ') files (stdout, stderr, exit code, emit, build info)"
    ;;
  *) usage ;;
esac
