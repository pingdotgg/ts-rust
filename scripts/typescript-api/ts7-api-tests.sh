#!/usr/bin/env bash
# Runs microsoft/TypeScript's own TS7 API tests (packages/typescript/test/**/*.test.ts at the tsc-rs
# pin, unmodified) against tsc-rs. The sync API's channel (src/api/syncChannel.ts) becomes the tsc-rs
# channel (npm/typescript-api/src/native/channel.js) through a Node module hook loaded with
# NODE_OPTIONS=--import (ts7-api/register.mjs), so the sync tests run tsc-rs in the test process
# (crates/ts_napi). The async tests keep spawning tsgo (built/local/tsc -> --tsgo).
#
# usage: scripts/typescript-api/ts7-api-tests.sh [options] [label]
#        scripts/typescript-api/ts7-api-tests.sh help
#   label              output directory name under target/typescript-api/ts7-api/ (default: the channel)
#   --channel KIND     native (default): the in-process addon; process: the same channel code over a
#                      `tsgo --api` child process (TSRS_API_CHANNEL=process); ts7: no hook, TS7's own
#                      SyncRpcChannel over tsgo (the reference)
#   --ts-repo DIR      a microsoft/TypeScript checkout at the pin (default: a sparse shallow fetch of
#                      fed0bf24149fb1ed36039212648bafdafc1ea10e into target/typescript-api/ts71)
#   --tsgo PATH        the tsgo binary for the async tests and version.test.ts (default target/release/tsgo)
#   --napi PATH        the addon to load (TSRS_NAPI_PATH; default npm/typescript-api/native/...)
#   --concurrency N    test files at a time (default 4)
#   --name PATTERN     only tests whose name matches (node --test-name-pattern)
#   --files GLOB       test files, relative to packages/typescript (default './test/**/*.test.ts')
#   --no-systemd       run in this scope, not under systemd-run with MemoryMax=12G
# Output in target/typescript-api/ts7-api/<label>/: results.jsonl (one line per test), spec.log,
# shim.log (one line per channel the hook opened), summary.json (counts for test/sync, test/async and
# the rest, and the failures). The last line of run.log is DONE or FAIL rc=<N>.
set -euo pipefail
case "${1:-}" in help | -h | --help) sed -n '2,/^set -euo/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'; exit 0 ;; esac

repo="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"
pin=fed0bf24149fb1ed36039212648bafdafc1ea10e
channel=native
ts_repo=""
tsgo="$repo/target/release/tsgo"
napi=""
concurrency=4
name_pattern=""
files='./test/**/*.test.ts'
use_systemd=1
label=""
while (($#)); do
  case "$1" in
    --channel) channel="$2"; shift 2 ;;
    --ts-repo) ts_repo="$2"; shift 2 ;;
    --tsgo) tsgo="$2"; shift 2 ;;
    --napi) napi="$2"; shift 2 ;;
    --concurrency) concurrency="$2"; shift 2 ;;
    --name) name_pattern="$2"; shift 2 ;;
    --files) files="$2"; shift 2 ;;
    --no-systemd) use_systemd=0; shift ;;
    -*) echo "unknown option: $1 (see help)" >&2; exit 2 ;;
    *) label="$1"; shift ;;
  esac
done
case "$channel" in native | process | ts7) ;; *) echo "--channel must be native, process or ts7" >&2; exit 2 ;; esac
label="${label:-$channel}"
tsgo="$(realpath -- "$tsgo")"
[[ -x "$tsgo" ]] || { echo "tsgo not found: $tsgo (build it, or pass --tsgo)" >&2; exit 2; }

work="$repo/target/typescript-api"
out="$work/ts7-api/$label"
tree="$work/ts7-tree"
hooks="$repo/scripts/typescript-api/ts7-api"
mkdir -p "$out"

# The TypeScript source at the pin: only the package, the fixtures and the astnav baselines.
if [[ -z "$ts_repo" ]]; then
  ts_repo="$work/ts71"
  if [[ "$(git -C "$ts_repo" rev-parse HEAD 2>/dev/null || true)" != "$pin" ]]; then
    rm -rf "$work/ts71"
    git init -q "$ts_repo"
    git -C "$ts_repo" remote add origin https://github.com/microsoft/TypeScript.git
    git -C "$ts_repo" sparse-checkout set packages/typescript tsc/testdata/fixtures tsc/testdata/baselines/reference/astnav
    git -C "$ts_repo" fetch -q --depth 1 --filter=blob:none origin "$pin"
    git -C "$ts_repo" checkout -q FETCH_HEAD
  fi
fi
ts_head="$(git -C "$ts_repo" rev-parse HEAD 2>/dev/null || echo unknown)"
[[ "$ts_head" == "$pin" ]] || echo "warning: $ts_repo is at $ts_head, not the pin $pin" >&2

# A copy, not a link: getExePath resolves built/local/tsc from the package's real path.
rm -rf "$tree/packages" "$tree/tsc" "$tree/built"
mkdir -p "$tree/packages" "$tree/built/local" "$tree/tsc/testdata/baselines/reference"
cp -a "$ts_repo/packages/typescript" "$tree/packages/"
cp -a "$ts_repo/tsc/testdata/fixtures" "$tree/tsc/testdata/"
cp -a "$ts_repo/tsc/testdata/baselines/reference/astnav" "$tree/tsc/testdata/baselines/reference/"
ln -sf "$tsgo" "$tree/built/local/tsc"
if [[ ! -d "$tree/node_modules/tinybench" ]]; then
  # sync/ast.test.ts imports ast.bench.ts, which imports tinybench.
  (cd "$tree" && { [[ -f package.json ]] || echo '{"private":true}' > package.json; } &&
    npm install --no-audit --no-fund --silent tinybench@6)
fi

node_options="${NODE_OPTIONS:-}"
env_args=(TSRS_SHIM_LOG="$out/shim.log")
if [[ "$channel" != ts7 ]]; then
  node_options="--import=$hooks/register.mjs${node_options:+ $node_options}"
fi
[[ "$channel" == process ]] && env_args+=(TSRS_API_CHANNEL=process TSRS_TSGO="$tsgo")
[[ -n "$napi" ]] && env_args+=(TSRS_NAPI_PATH="$(realpath -- "$napi")")
rm -f "$out/shim.log" "$out/results.jsonl" "$out/spec.log" "$out/summary.json"

test_cmd=(node --experimental-strip-types --no-warnings --conditions @typescript/source --test
  --test-concurrency="$concurrency" --test-timeout=300000
  --test-reporter=spec --test-reporter-destination="$out/spec.log"
  --test-reporter="$hooks/reporter.mjs" --test-reporter-destination="$out/results.jsonl")
[[ -n "$name_pattern" ]] && test_cmd+=(--test-name-pattern="$name_pattern")
test_cmd+=("$files")

{
  echo "channel=$channel ts=$ts_head tsgo=$tsgo napi=${napi:-default} concurrency=$concurrency"
  start=$(date +%s%N)
  set +e
  if ((use_systemd)) && command -v systemd-run > /dev/null; then
    setenv=(--setenv=PATH="$PATH" --setenv=HOME="$HOME" --setenv=NODE_OPTIONS="$node_options")
    for pair in "${env_args[@]}"; do setenv+=(--setenv="$pair"); done
    systemd-run --user --collect --wait --quiet -p MemoryMax=12G -p MemorySwapMax=0 \
      --working-directory="$tree/packages/typescript" "${setenv[@]}" \
      -p "Description=ts7-api-tests $label" "${test_cmd[@]}"
  else
    (cd "$tree/packages/typescript" && env NODE_OPTIONS="$node_options" "${env_args[@]}" "${test_cmd[@]}")
  fi
  rc=$?
  set -e
  echo "wall $(( ($(date +%s%N) - start) / 1000000 )) ms"
  node "$hooks/summarize.mjs" "$out/results.jsonl" "$out/shim.log" > "$out/summary.json" || true
  head -c 2000 "$out/summary.json"; echo
  if ((rc == 0)); then echo DONE; else echo "FAIL rc=$rc"; fi
} 2>&1 | tee "$out/run.log"
