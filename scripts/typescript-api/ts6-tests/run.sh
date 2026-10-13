#!/usr/bin/env bash
# Runs TypeScript 6.0.3's own tests, unmodified, against a `typescript` library directory.
# usage: run.sh help | setup | run <library-dir> <out-dir> [--suite S] [--jobs N] [--force] [--detach]
#        | compare <base-out> <new-out> [--limit N] [--json FILE] | triage <out-dir> [--ts7 DIR] [--limit N]
set -euo pipefail

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)

command=${1:-help}
case "$command" in
setup | run)
    # Setup installs a large repo and the suites run big test loads: keep them under a memory cap
    # in their own systemd unit. TS6_TESTS_SYSTEMD=0 skips this (for a caller that made its own unit).
    if [ "${TS6_TESTS_SYSTEMD:-1}" = 1 ] && command -v systemd-run >/dev/null; then
        args=()
        detach=0
        for arg in "$@"; do
            if [ "$arg" = --detach ]; then
                detach=1
            else
                args+=("$arg")
            fi
        done
        unit=(--user --collect --quiet --same-dir -p MemoryMax="${TS6_TESTS_MEMORY_MAX:-12G}" -p MemorySwapMax=0
            --setenv=PATH="$PATH" --setenv=HOME="$HOME" --setenv=TS6_TESTS_SYSTEMD=0)
        while IFS='=' read -r name _; do
            unit+=(--setenv="$name=${!name}")
        done < <(env | grep -E '^(TS6_TESTS_[A-Z_]+|TSRS_[A-Z_]+)=' | grep -v '^TS6_TESTS_SYSTEMD=' || true)
        if [ "$detach" = 1 ]; then
            name="ts6-tests-$(date +%Y%m%d-%H%M%S)-$$"
            systemd-run "${unit[@]}" --unit="$name" node "$HERE/harness.mjs" "${args[@]}"
            echo "started unit $name (journalctl --user -u $name); <out-dir>/harness.log ends with DONE or FAIL rc=N"
            exit 0
        fi
        exec systemd-run "${unit[@]}" --wait --pipe node "$HERE/harness.mjs" "${args[@]}"
    fi
    exec node "$HERE/harness.mjs" "$@"
    ;;
*)
    exec node "$HERE/harness.mjs" "$@"
    ;;
esac
