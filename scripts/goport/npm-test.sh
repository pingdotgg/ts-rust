#!/usr/bin/env bash
# Installs an npm-pack.sh package set into a fresh project and checks that tsc through npm gives
# the same output as the native tsc run directly.
#
# usage: npm-test.sh [--name <tsc-rs|@tsc-rs/typescript>] <pkg-dir> <work-dir>
#   --name      the package set of npm-pack.sh --name (default typescript). tsc-rs: the main package
#               tsc-rs, bin tsc-rs, platform package @tsc-rs/linux-x64. @tsc-rs/typescript: bin tsc,
#               the platform package @tsc-rs/linux-x64 installed under Go's name
#               @typescript/typescript-linux-x64, as the main package's alias installs it.
#   <pkg-dir>   npm-pack.sh output (the main and the linux-x64 .tgz files)
#   <work-dir>  made fresh: <work-dir>/proj (the npm project) and <work-dir>/out (tsc output)
# NPM_TEST_INPUTS=<dir> names the project inputs (default <repo>/target/project-inputs).
# The install from a registry, under the name typescript, is scripts/goport/npm-alias-test.sh.
# Checks:
#   - bin/tsc: the sh and JS polyglot of npm/install.js when the package has the postinstall
#     (Rust), else Go's JS launcher. bin/tsc and node_modules/.bin/tsc run the platform tsc without
#     Node (NODE_OPTIONS that make Node fail to start do not stop them).
#   - --version, and --listFilesOnly lists the lib files of the platform package's lib dir.
#   - --locale de gives German messages: the platform package's lib/loc holds the catalogs.
#   - @tsc-rs/typescript: require("@tsc-rs/typescript").version is the version the tsc reports.
#   - query and hono, tsc -p <cfg> --outDir <out>: stdout, exit code and the emitted files are the
#     same for the direct run (platform lib/tsc), node_modules/.bin/tsc, node bin/tsc (a tool that
#     starts the bin with Node) and the JS launcher (node node_modules/typescript/lib/tsc.js, the
#     fallback when the postinstall did not run).
#   - the postinstall with a failing chmod leaves no temp file and does not change bin/tsc.
#   - a bin/tsc whose native tsc path names no file runs Go's launcher with Node.
#   - the project moved to another dir: .bin/tsc and node bin/tsc still run, with its lib files.
#   - pnpm (when on PATH), with the build approved: the same bin checks in a pnpm project.
# Prints one line per check and ends with "npm-test: PASS" or "npm-test: FAIL (<n>)".
set -uo pipefail
repo=/home/theo/Code/sandbox/ts-rust
usage() { sed -n '5,13p' "$0" >&2; exit 2; }
name=typescript
if [[ ${1:-} == --name ]]; then [[ $# -ge 2 ]] || usage; name=$2; shift 2; fi
# plat is the install name of the platform package, plat_tgz the name of its tarball.
case $name in
  typescript) plat=@typescript/typescript-linux-x64 plat_tgz=typescript-typescript-linux-x64 bin=tsc ;;
  tsc-rs) plat=@tsc-rs/linux-x64 plat_tgz=tsc-rs-linux-x64 bin=tsc-rs ;;
  @tsc-rs/typescript) plat=@typescript/typescript-linux-x64 plat_tgz=tsc-rs-linux-x64 bin=tsc ;;
  *) usage ;;
esac
[[ $# == 2 ]] || usage
pkg=$(realpath "$1") work=$(realpath -m "$2")
proj="$work/proj" out="$work/out"
rm -rf "$work"
mkdir -p "$proj" "$out"
main_tgz=${name#@}
tgz=("$pkg/$plat_tgz"-[0-9]*.tgz "$pkg/${main_tgz/\//-}"-[0-9]*.tgz)
[[ ${#tgz[@]} == 2 && -f ${tgz[0]} && -f ${tgz[1]} ]] || { echo "no package set in $pkg" >&2; exit 2; }
echo '{"name":"npm-test","private":true}' > "$proj/package.json"
# The platform package under its install name (an alias for @tsc-rs/typescript).
(cd "$proj" && npm install --offline --no-audit --no-fund --silent "$plat@file:${tgz[0]}" "${tgz[1]}") ||
  { echo "npm install failed" >&2; exit 1; }

fails=0
check() { # check <name> <ok 0|1> [detail]
  if [[ $2 == 0 ]]; then echo "ok   $1"; else echo "FAIL $1${3:+: $3}"; fails=$((fails + 1)); fi
}
nm="$proj/node_modules"
direct="$nm/$plat/lib/tsc"
declare -A ways=([direct]="$direct" [npm]="$nm/.bin/$bin" [node]="node $nm/$name/bin/$bin"
  [js]="node $nm/$name/lib/tsc.js")
want=$("$direct" --version)
native=0
grep -q '"postinstall"' "$nm/$name/package.json" && native=1
echo > "$out/a.ts"

# bins <tag> <node_modules>: the bin checks, for npm-test's npm project and the pnpm project.
bins() {
  local tag=$1 nm=$2 main lib_dir listed v env=() how=""
  local b=$nm/$name/bin/$bin
  if ((native)); then
    [[ -f $b && ! -L $b && -x $b && $(sed -n '1p;3p' "$b") == $'#!/bin/sh\nimport "../lib/tsc.js";' ]]
    check "$tag bin/$bin is the sh and JS polyglot" $? "$(ls -l "$b")"
    # Node fails to start with this NODE_OPTIONS; the native tsc ignores it.
    env=(NODE_OPTIONS=--require=/npm-test/no-node) how=" without Node"
  else
    [[ ! -L $b ]] && head -1 "$b" | grep -q node
    check "$tag bin/$bin is Go's JS launcher" $?
  fi
  for x in "$b" "$nm/.bin/$bin"; do
    [[ $(env "${env[@]}" "$x" --version 2>&1) == "$want" ]]
    check "$tag ${x#"$nm"/} --version$how: $want" $?
  done
  v=$(node "$b" --version 2>&1)
  [[ $v == "$want" ]]
  check "$tag node $name/bin/$bin --version: $want" $? "$(grep -m1 Error <<< "$v" || head -1 <<< "$v")"
  # The platform package is in the node_modules dir of the main package (npm hoists both; pnpm
  # links both into the main package's .pnpm/<pkg>/node_modules). $name can be scoped.
  main=$(realpath "$nm/$name")
  lib_dir=$(realpath "${main%/"$name"}/$plat/lib")
  for x in "$nm/.bin/$bin" "node $b"; do
    listed=$(cd "$out" && $x --listFilesOnly --lib es5 a.ts 2>&1)
    grep -q "^$lib_dir/lib.es5.d.ts$" <<< "$listed"
    check "$tag ${x//"$nm/"/} lists $lib_dir/lib.es5.d.ts" $? "$(head -1 <<< "$listed")"
  done
}

bins npm "$nm"
# A failed chmod in the postinstall leaves no bin/<bin>.<pid>.tmp and does not change the bin.
if ((native)); then
  b=$nm/$name/bin/$bin before=$(sha256sum < "$nm/$name/bin/$bin")
  printf '%s\n' 'const fs = require("node:fs");' \
    'fs.chmodSync = () => { throw Object.assign(new Error("EPERM: chmod"), { code: "EPERM" }); };' \
    > "$work/chmod-fails.cjs"
  warn=$(node --require "$work/chmod-fails.cjs" "$nm/$name/lib/install.js" 2>&1)
  tmps=$(find "$nm/$name/bin" -name "$bin.*.tmp")
  [[ -z $tmps && $(sha256sum < "$b") == "$before" && $warn == *EPERM* ]]
  check "a failed chmod in the postinstall leaves no temp file and keeps bin/$bin" $? "${tmps:-$warn}"
  # A native path that names no file (pnpm's side-effects cache in another layout): the bin runs
  # Go's launcher with Node.
  cp -p "$b" "$work/bin.keep"
  sed -i '2s|t="${p%/\*}/[^"]*"|t="${p%/*}/../no-such-dir/tsc"|' "$b"
  v=$("$nm/.bin/$bin" --version 2>&1)
  [[ $v == "$want" ]] && grep -q no-such-dir "$b"
  check "a bin whose native path names no file runs Go's launcher: $want" $? "$v"
  cp -p "$work/bin.keep" "$b"
fi
[[ $(${ways[js]} --version) == "$want" ]]
check "js --version: $want" $?
if [[ $name == @tsc-rs/typescript ]]; then
  v=$(cd "$proj" && node -e 'console.log(require(process.argv[1]).version)' "$name" 2>&1)
  [[ "Version $v" == "$want" ]]
  check "require(\"$name\").version: ${want#Version }" $? "$v"
fi
lib_dir=$(realpath "$(dirname "$direct")")
for w in direct js; do
  listed=$(cd "$out" && ${ways[$w]} --listFilesOnly --lib es5 a.ts)
  grep -q "^$lib_dir/lib.es5.d.ts$" <<< "$listed"
  check "$w lists $lib_dir/lib.es5.d.ts" $? "$(head -1 <<< "$listed")"
done
# The locale catalogs: the native tsc reads them from the platform package's lib/loc.
echo 'const x: number = "a";' > "$out/de.ts"
v=$(cd "$out" && "$direct" --noEmit --locale de de.ts 2>&1)
grep -q 'error TS2322: Der Typ "string"' <<< "$v"
check "direct --locale de reads lib/loc/de-DE.json.gz" $? "$(head -1 <<< "$v")"

P=${NPM_TEST_INPUTS:-$repo/target/project-inputs}
declare -A cfgs=([query]=$P/query/source/packages/query-core/tsconfig.prod.json [hono]=$P/hono/source/tsconfig.build.json)
for p in query hono; do
  for w in direct npm node js; do
    (cd "$out" && ${ways[$w]} -p "${cfgs[$p]}" --outDir "$out/$p-$w" --pretty false > "$out/$p-$w.stdout" 2>&1)
    echo "exit $?" >> "$out/$p-$w.stdout"
  done
  # Take each status before the label: a $(...) in the label would reset $?.
  for w in npm node js; do
    cmp -s "$out/$p-direct.stdout" "$out/$p-$w.stdout"
    r=$?
    check "$p $w stdout and exit code equal direct ($(tail -1 "$out/$p-direct.stdout"), $(($(wc -l < "$out/$p-direct.stdout") - 1)) lines)" $r
    diff -rq "$out/$p-direct" "$out/$p-$w" > /dev/null 2>&1
    r=$?
    check "$p $w emit equals direct ($(find "$out/$p-direct" -type f | wc -l) files)" $r
  done
done

# The bin finds the platform tsc by a relative path, so the project can move.
mv "$proj" "$work/moved"
bins moved "$work/moved/node_modules"
mv "$work/moved" "$proj"

if command -v pnpm > /dev/null; then
  pp="$work/pnpm"
  mkdir -p "$pp"
  echo "{\"name\":\"npm-test-pnpm\",\"private\":true,\"dependencies\":{\"$name\":\"file:${tgz[1]}\"}}" > "$pp/package.json"
  printf 'overrides:\n  "%s": "file:%s"\ndangerouslyAllowAllBuilds: true\n' "$plat" "${tgz[0]}" > "$pp/pnpm-workspace.yaml"
  if (cd "$pp" && pnpm install --offline --silent --store-dir "$work/pnpm-store" > "$work/pnpm.log" 2>&1); then
    bins pnpm "$pp/node_modules"
  else
    check "pnpm install" 1 "$(tail -3 "$work/pnpm.log")"
  fi
else
  echo "skip pnpm (not on PATH)"
fi

if ((fails)); then echo "npm-test: FAIL ($fails)"; exit 1; fi
echo "npm-test: PASS"
