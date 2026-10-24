#!/usr/bin/env bash
# Boat sandbox helper for goport work (boat.dev; Theo, 2026-10-24: "Move everything off instabox and onto boat").
# The sandbox mirrors zbook's paths: /home/theo is a link to the sandbox HOME (/home/user), so the repo is at
# /home/theo/Code/sandbox/ts-rust and ~/.explore and ~/.local/bin resolve as on zbook. Third-party code
# (Bun, lust, outside PRs) runs here or on a testbox, never on zbook. One agent owns a sandbox at a time.
#
# usage: scripts/goport/boatbox.sh <command> ...
#   help                    this text
#   new <name> [type]       create a sandbox (type small, default, large or xlarge; default large), save its id
#                           in target/boat-box and run setup
#   box                     the sandbox id: $BOATBOX, else target/boat-box
#   setup                   make the path links and clone the repo (shallow, origin/main) if missing;
#                           safe to run again
#   run <cmd...>            run <cmd> in the repo dir on the sandbox (bash -lc); returns its exit code
#   push <path>...          copy local files or trees (absolute, or relative to this checkout) to the same
#                           absolute path on the sandbox (tar over boat ssh; about 10 MB/s)
#   get <path> [local dir]  copy a sandbox path back (gzip tar over boat ssh); default local dir: the same path
#   job <name> <cmd...>     run <cmd> detached; log /home/theo/boat-jobs/<name>.log ends with DONE or FAIL rc=<N>
#   wait <name>             block until that log ends (at most 590 s per call; call again)
#   status                  sandbox state, load, memory, disk and jobs
#   extend <hours>          set the sandbox's remaining lifetime (a new sandbox gets about 1 hour)
#   stop                    stop the sandbox (Boat snapshots it; `boat resume <id>` brings it back)
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
REPO=/home/theo/Code/sandbox/ts-rust

box() {
  if [[ -n "${BOATBOX:-}" ]]; then echo "$BOATBOX"; return; fi
  [[ -s "$ROOT/target/boat-box" ]] || { echo "boatbox: set BOATBOX or run: boatbox.sh new <name>" >&2; exit 2; }
  cat "$ROOT/target/boat-box"
}
on() { boat ssh "$(box)" -- "$@"; }
abs() { [[ "$1" == /* ]] && echo "$1" || echo "$ROOT/$1"; }

cmd=${1:-help}; shift || true
case "$cmd" in
  help|-h|--help) sed -n '2,/^set -euo/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//' ;;
  new)
    id=$(boat new --type "${2:-large}" --name "$1" --json | python3 -c 'import json,sys
for l in sys.stdin:
    d=json.loads(l)
    if d.get("event")=="ready": print(d["id"])')
    [[ -n "$id" ]] || { echo "boatbox: boat new gave no ready id" >&2; exit 1; }
    mkdir -p "$ROOT/target"; echo "$id" > "$ROOT/target/boat-box"; echo "sandbox $id"; "$0" setup ;;
  box) box ;;
  setup)
    on "set -e; [ -e /home/theo ] || sudo ln -s \$HOME /home/theo
        mkdir -p /home/theo/Code/sandbox /home/theo/.explore/repos /home/theo/.local/bin /home/theo/boat-jobs
        [ -d $REPO/.git ] || git clone -q --depth 1 https://github.com/pingdotgg/ts-rust $REPO
        cd $REPO && git log -1 --format='repo %h %s'; nproc; free -g | sed -n 2p; df -h / | tail -1" ;;
  run) on "cd $REPO && $*" ;;
  push)
    for p in "$@"; do a=$(abs "$p"); d=$(dirname "$a")
      tar -C "$d" -czf - "$(basename "$a")" | on "mkdir -p '$d' && tar -C '$d' -xzf -"; echo "pushed $a"; done ;;
  get)
    a=$1; l=${2:-$(dirname "$1")}; mkdir -p "$l"
    on "tar -C '$(dirname "$a")' -czf - '$(basename "$a")'" | tar -C "$l" -xzf -; echo "got $a -> $l" ;;
  job)
    n=$1; shift; log=/home/theo/boat-jobs/$n.log
    on "mkdir -p /home/theo/boat-jobs; cd $REPO; setsid nohup bash -lc $(printf '%q' "$*; rc=\$?; [ \$rc = 0 ] && echo DONE || echo \"FAIL rc=\$rc\"") > $log 2>&1 < /dev/null & echo started $log" ;;
  wait)
    log=/home/theo/boat-jobs/$1.log
    on "timeout 590 bash -c 'until grep -qE \"^(DONE|FAIL)\" $log 2>/dev/null; do sleep 10; done'; tail -5 $log" ;;
  status)
    boat info "$(box)" --json | python3 -c 'import json,sys; d=json.load(sys.stdin); d=d.get("sandbox",d); print(" ".join(f"{k}={d.get(k)}" for k in ["id","name","state","type","vcpu","memoryGB","health"]))' || true
    on "uptime; free -m | sed -n 2p; df -h / | tail -1; for f in /home/theo/boat-jobs/*.log; do [ -e \"\$f\" ] && echo \"\$(basename \$f): \$(tail -1 \$f | cut -c1-80)\"; done; true" ;;
  extend) boat extend "$(box)" --hours "$1" ;;
  stop) boat stop "$(box)" ;;
  *) echo "boatbox: unknown command $cmd (try help)" >&2; exit 2 ;;
esac
