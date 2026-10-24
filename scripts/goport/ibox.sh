#!/usr/bin/env bash
# Instabox sandbox helper for goport work. Theo, 2026-10-23: "start using it instead of Boat".
# The box mirrors zbook's paths: /home/theo is a link to the box HOME (/data/home), so the repo is at
# /home/theo/Code/sandbox/ts-rust and ~/.explore and ~/.local/bin resolve as on zbook. Third-party code
# (Bun, lust, outside PRs) runs here or on a testbox, never on zbook.
#
# usage: scripts/goport/ibox.sh <command> ...
#   help                    this text
#   box                     the box name: $IBOX, else target/instabox-box, else the only box in `instabox ls`
#   setup                   make the path links and clone the repo (shallow, origin/main) if missing;
#                           safe to run again
#   run <cmd...>            run <cmd> in the repo dir on the box (bash -lc); returns its exit code
#   push <path>...          copy local files or trees (absolute, or relative to this checkout) to the same
#                           absolute path on the box (tar over instabox ssh; about 50 MB/s up)
#   get <path> [local dir]  copy a box path back (tar over instabox ssh; about 5.7 MB/s down, so compress
#                           or fetch summaries); default local dir: the same path
#   job <name> <cmd...>     run <cmd> detached on the box; log /home/theo/ibox-jobs/<name>.log ends with
#                           DONE or FAIL rc=<N>
#   wait <name>             block until that log ends (at most 590 s per call; call again)
#   status                  box state, load, memory, disk and running jobs
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
REPO=/home/theo/Code/sandbox/ts-rust

box() {
  if [[ -n "${IBOX:-}" ]]; then echo "$IBOX"; return; fi
  if [[ -s "$ROOT/target/instabox-box" ]]; then cat "$ROOT/target/instabox-box"; return; fi
  local names; names=$(instabox ls --json | python3 -c 'import json,sys; d=json.load(sys.stdin); d=d.get("sandboxes",d) if isinstance(d,dict) else d; print("\n".join(x["name"] for x in d))')
  [[ $(wc -l <<<"$names") == 1 && -n "$names" ]] || { echo "ibox: set IBOX or target/instabox-box (boxes: $names)" >&2; exit 2; }
  echo "$names"
}
on() { instabox ssh "$(box)" -- "$@"; }
abs() { [[ "$1" == /* ]] && echo "$1" || echo "$ROOT/$1"; }

cmd=${1:-help}; shift || true
case "$cmd" in
  help|-h|--help) sed -n '2,/^set -euo/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//' ;;
  box) box ;;
  setup)
    on "set -e; [ -e /home/theo ] || sudo ln -s /data/home /home/theo
        mkdir -p /home/theo/Code/sandbox /home/theo/.explore/repos /home/theo/.local/bin /home/theo/ibox-jobs
        [ -d $REPO/.git ] || git clone -q --depth 1 https://github.com/pingdotgg/ts-rust $REPO
        cd $REPO && git log -1 --format='repo %h %s'; nproc; free -g | sed -n 2p; df -h /data | tail -1" ;;
  run) on "cd $REPO && $*" ;;
  push)
    for p in "$@"; do a=$(abs "$p"); d=$(dirname "$a")
      tar -C "$d" -cf - "$(basename "$a")" | on "mkdir -p '$d' && tar -C '$d' -xf -"; echo "pushed $a"; done ;;
  get)
    a=$1; l=${2:-$(dirname "$1")}; mkdir -p "$l"
    on "tar -C '$(dirname "$a")' -cf - '$(basename "$a")'" | tar -C "$l" -xf -; echo "got $a -> $l" ;;
  job)
    n=$1; shift; log=/home/theo/ibox-jobs/$n.log
    on "mkdir -p /home/theo/ibox-jobs; cd $REPO; setsid nohup bash -lc $(printf '%q' "$*; rc=\$?; [ \$rc = 0 ] && echo DONE || echo \"FAIL rc=\$rc\"") > $log 2>&1 < /dev/null & echo started $log" ;;
  wait)
    log=/home/theo/ibox-jobs/$1.log
    on "timeout 590 bash -c 'until grep -qE \"^(DONE|FAIL)\" $log 2>/dev/null; do sleep 10; done'; tail -5 $log" ;;
  status)
    instabox get "$(box)" | sed -n '1,8p'
    on "uptime; free -m | sed -n 2p; df -h /data / | tail -2; for f in /home/theo/ibox-jobs/*.log; do [ -e \"\$f\" ] && echo \"\$(basename \$f): \$(tail -1 \$f | cut -c1-80)\"; done" ;;
  *) echo "ibox: unknown command $cmd (try help)" >&2; exit 2 ;;
esac
