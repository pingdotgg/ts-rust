#!/usr/bin/env bash
# Blacksmith testboxes: a warm 32 vCPU Linux x64 runner (.github/workflows/testbox.yml) that runs a command
# against this worktree's files. `run` syncs this worktree to the testbox (the CLI's rsync) and runs the
# command in the synced repo root. target/ and the cargo registry sit on sticky disks.
# usage: testbox.sh help                      this text
#        testbox.sh warmup                    start a testbox; saves its ID to target/testbox-id
#        testbox.sh run <command...>          sync this worktree, then run the command with the testbox env
#                                             (e.g. run scripts/run-cargo-capped.sh build --release -p ts_goport --bins)
#        testbox.sh get <remote> [local]      copy a file or dir back (remote path relative to the repo root)
#        testbox.sh status | stop | list      the saved testbox's status, stop it, list active testboxes
# TESTBOX_ID overrides the saved ID. TESTBOX_IDLE: idle minutes before the testbox stops (default 15).
# TESTBOX_REF: the ref the workflow runs from (default main: the workflow must be on the default branch).
# Auth: `blacksmith auth login` once per host (`blacksmith auth status` shows it).
set -euo pipefail
case "${1:-help}" in help | -h | --help) sed -n '2,/^set -euo/p' "$0" | sed '$d'; exit 0 ;; esac

repo_root="$(git -C "$(dirname -- "${BASH_SOURCE[0]}")" rev-parse --show-toplevel)"
id_file="$repo_root/target/testbox-id"
cd "$repo_root"

saved_id() {
  local id="${TESTBOX_ID:-$(cat "$id_file" 2>/dev/null || true)}"
  [[ -n "$id" ]] || { echo "no testbox: run 'testbox.sh warmup' or set TESTBOX_ID" >&2; exit 2; }
  echo "$id"
}

cmd="$1"
shift
case "$cmd" in
  warmup)
    out="$(blacksmith testbox warmup testbox.yml --ref "${TESTBOX_REF:-main}" --idle-timeout "${TESTBOX_IDLE:-15}" 2>&1)"
    echo "$out"
    id="$(grep -oE 'tbx_[A-Za-z0-9_-]+' <<<"$out" | head -1)"
    [[ -n "$id" ]] || { echo "warmup printed no testbox ID" >&2; exit 1; }
    mkdir -p "$(dirname "$id_file")"
    echo "$id" >"$id_file"
    echo "testbox $id (saved to target/testbox-id)"
    ;;
  run)
    (($#)) || { echo "usage: testbox.sh run <command...>" >&2; exit 2; }
    blacksmith testbox run --id "$(saved_id)" ". ~/testbox.env && $*"
    ;;
  get) blacksmith testbox download --id "$(saved_id)" "$@" ;;
  status) blacksmith testbox status --id "$(saved_id)" "$@" ;;
  stop) blacksmith testbox stop --id "$(saved_id)" && rm -f "$id_file" ;;
  list) blacksmith testbox list "$@" ;;
  *) echo "unknown command: $cmd (testbox.sh help)" >&2; exit 2 ;;
esac
