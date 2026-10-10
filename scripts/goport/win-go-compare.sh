#!/usr/bin/env bash
# Windows only (Git Bash): runs a small scenario set with two tsc builds and fails when the exit
# code, stdout, stderr or any written file differs. CI runs it with the tsc of this repo and Go's tsc
# built from the pinned Go source (.github/workflows/ci.yml, the Windows test job).
#
# usage: win-go-compare.sh <tsc.exe> <go tsc.exe> <dir>
#
# The scenarios are the Windows path forms that a Linux run does not reach: backslash and absolute
# arguments, a lower-case drive letter, a \\?\ path, CRLF sources, a junction node_modules (as pnpm
# and npm workspaces make), a path over 260 characters, a non-ASCII file name, -b --verbose, emit
# with source maps, --pretty and --pretty false errors, --showConfig and --init. Each side runs in a
# new copy of the scenario tree at the same path, from the same exe path (<dir>/bin/tsc.exe, with
# the lib files when they are next to the given exe), so the paths in the output are equal. The
# clock in the -b --verbose output is masked.
set -euo pipefail
[[ $# == 3 ]] || { sed -n '2,15p' "$0" >&2; exit 2; }
command -v cygpath > /dev/null || { echo "error: Windows (Git Bash) only" >&2; exit 2; }
tsc_rs=$1 tsc_go=$2
mkdir -p "$3"
dir=$(cygpath -m "$(cd -- "$3" && pwd)")
work=$dir/work

# junction <link> <target>: a directory junction (no privilege needed, unlike a symlink).
junction() {
  MSYS_NO_PATHCONV=1 cmd /c mklink /J "$(cygpath -w "$1")" "$(cygpath -w "$2")" > /dev/null
}

# scenarios: writes the inputs into $work. The junction targets are under $dir/store, outside
# $work, so they stay when $work moves.
scenarios() {
  rm -rf "$work" "$dir/store"
  mkdir -p "$work" "$dir/store/node_modules/pkg"

  # errs: errors in an LF file and in a CRLF file.
  mkdir -p "$work/errs"
  printf '{ "compilerOptions": { "strict": true, "noEmit": true }, "include": ["*.ts"] }\n' > "$work/errs/tsconfig.json"
  printf 'export const a: number = "x";\nexport function f(x) { return x; }\n' > "$work/errs/lf.ts"
  printf 'import { a } from "./lf";\r\nlet b: string = a;\r\n\r\nexport const c = b.foo;\r\n' > "$work/errs/crlf.ts"
  printf 'export const \xc3\xbc: number = "\xc3\xa9";\n' > "$work/errs/$(printf '\xc3\xbcn\xc3\xafc\xc3\xb6d\xc3\xa9').ts"

  # emit: CRLF and LF sources, declarations and source maps.
  mkdir -p "$work/emit/src"
  printf '{ "compilerOptions": { "outDir": "out", "rootDir": "src", "declaration": true, "declarationMap": true, "sourceMap": true, "target": "es2020", "module": "nodenext" } }\n' > "$work/emit/tsconfig.json"
  printf 'export class A {\r\n  x = 1;\r\n  /** doc */\r\n  m(): number { return this.x; }\r\n}\r\n' > "$work/emit/src/a.ts"
  printf 'import { A } from "./a.js";\nexport const b = new A().m();\n' > "$work/emit/src/b.ts"

  # junc: node_modules is a junction to $dir/store/node_modules.
  mkdir -p "$work/junc"
  printf '{ "name": "pkg", "version": "1.0.0", "types": "index.d.ts" }\n' > "$dir/store/node_modules/pkg/package.json"
  printf 'export declare const v: number;\n' > "$dir/store/node_modules/pkg/index.d.ts"
  printf '{ "compilerOptions": { "noEmit": true, "module": "nodenext" }, "files": ["main.ts"] }\n' > "$work/junc/tsconfig.json"
  printf 'import { v } from "pkg";\nexport const s: string = v;\n' > "$work/junc/main.ts"
  junction "$work/junc/node_modules" "$dir/store/node_modules"

  # build: b references a, both composite.
  mkdir -p "$work/build/a" "$work/build/b"
  printf '{ "compilerOptions": { "composite": true } }\n' > "$work/build/a/tsconfig.json"
  printf 'export const a = 1;\n' > "$work/build/a/index.ts"
  printf '{ "compilerOptions": { "composite": true }, "references": [{ "path": "../a" }] }\n' > "$work/build/b/tsconfig.json"
  printf 'import { a } from "../a";\nexport const b: string = a;\n' > "$work/build/b/index.ts"

  # long: a project dir whose files have paths over 260 characters.
  long=long
  for i in 1 2 3 4 5 6 7 8 9 10 11 12; do long+=/segment-$i-abcdefghijklmnop; done
  mkdir -p "$work/$long"
  printf '{ "compilerOptions": { "outDir": "out", "declaration": true } }\n' > "$work/$long/tsconfig.json"
  printf 'export const x: number = 1;\nexport const y: string = x;\n' > "$work/$long/index.ts"
  # The path under $work, with backslashes.
  echo "${long//\//\\}" > "$dir/long-path"

  mkdir -p "$work/init"
}

# run <name> <cwd> <tsc args>...: one tsc run; its stdout, stderr and exit code go to
# $work/.runs/<name>.{out,err,rc}. MSYS_NO_PATHCONV keeps the arguments as they are.
run() {
  local name=$1 cwd=$2 rc=0
  shift 2
  mkdir -p "$work/.runs"
  (cd "$cwd" && MSYS_NO_PATHCONV=1 "$dir/bin/tsc.exe" "$@") > "$work/.runs/$name.out" 2> "$work/.runs/$name.err" || rc=$?
  echo "$rc" > "$work/.runs/$name.rc"
}

# side <tsc.exe> <out>: the scenario set with <tsc.exe>; the tree ends up in <out>.
side() {
  rm -rf "$dir/bin"
  mkdir -p "$dir/bin"
  cp "$1" "$dir/bin/tsc.exe"
  cp "$(dirname "$1")"/lib*.d.ts "$dir/bin/" 2> /dev/null || true
  scenarios
  local errs_win long_win
  errs_win=$(cygpath -w "$work/errs/tsconfig.json")
  # cygpath -w gives a long path the \\?\ prefix; this one is a plain C:\... path.
  long_win="$(cygpath -w "$work")\\$(cat "$dir/long-path")"
  run version "$work" --version
  run errs "$work" -p errs --pretty false
  run errs-pretty "$work" -p errs --pretty
  run errs-backslash "$work" -p 'errs\tsconfig.json' --pretty false
  run errs-absolute "$work/emit" -p "$errs_win" --pretty false
  run errs-lower-drive "$work/emit" -p "${errs_win,}" --pretty false
  run errs-verbatim "$work/emit" -p "\\\\?\\$errs_win" --pretty false
  run errs-file-args "$work" --noEmit --pretty false "$(cygpath -w "$work/errs/lf.ts")" 'errs\crlf.ts'
  run list-files "$work" --listFilesOnly --lib es5 "${errs_win%tsconfig.json}lf.ts"
  run show-config "$work" -p 'errs\tsconfig.json' --showConfig
  run emit "$work/emit" -p . --pretty false
  run junction "$work/junc" -p . --pretty false --listFiles --traceResolution
  run build "$work/build" -b b --verbose --pretty false
  run build-again "$work/build" -b b --verbose --pretty false
  run long "$work" -p "$long_win" --pretty false
  run init "$work/init" --init
  # The -b --verbose lines start with the local time.
  sed -i -E 's/^[0-9]{1,2}:[0-9]{2}:[0-9]{2} [AP]M - /TIME - /' "$work/.runs"/build*.out
  rm -rf "$2"
  mv "$work" "$2"
}

side "$tsc_go" "$dir/go"
side "$tsc_rs" "$dir/rs"
for f in "$dir/go/.runs"/*.rc; do
  n=$(basename "$f" .rc)
  echo "$n: exit $(cat "$f"), stdout $(wc -c < "${f%.rc}.out") bytes, stderr $(wc -c < "${f%.rc}.err") bytes"
done
if ! diff -r "$dir/go" "$dir/rs" > "$dir/compare.diff"; then
  echo "error: the tsc differs from Go's (diff -r $dir/go $dir/rs):" >&2
  head -60 "$dir/compare.diff" >&2
  exit 1
fi
echo "same as Go: $(find "$dir/go" -type f | wc -l | tr -d ' ') files ($(ls "$dir/go/.runs" | grep -c '\.rc$') runs)"
