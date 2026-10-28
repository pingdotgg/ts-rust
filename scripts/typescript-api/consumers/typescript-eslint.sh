#!/usr/bin/env bash
# typescript-eslint v8.71.2's own test suites, unmodified, against a `typescript` package directory.
# usage: scripts/typescript-api/consumers/typescript-eslint.sh help
#        typescript-eslint.sh setup [--force]
#            Clone v8.71.2 into target/typescript-api/consumers/typescript-eslint (cached), pnpm install with the
#            pnpm of its packageManager field and build every package once with the repo's stock TypeScript.
#            `run` calls it when the clone is not set up.
#        typescript-eslint.sh run <typescript-package-dir|stock> <out-dir> [--packages p1,p2] [--shard i/n]
#                                 [--files f1,f2] [--workers N] [--timeout SECS] [--copy] [--resume]
#            Point the clone's `typescript` at the package directory, run vitest per package, restore the link,
#            and write <out-dir>/results.json (one entry per test), files.json, run.json, summary.md and logs/.
#            `stock` runs the clone's own typescript@6.0.3 without any swap (the control).
#            --packages  default: all, in this order: typescript-estree ast-spec type-utils parser project-service
#                        tsconfig-utils rule-tester typescript-eslint scope-manager eslint-plugin
#            --shard     vitest --shard=i/n for every selected package (files are split, CI uses 4 for eslint-plugin)
#            --files     vitest file filters (substrings of test file paths), e.g. rules/no-misused-spread; compare
#                        treats the other files as uncovered
#            --workers   vitest --maxWorkers (default 4)
#            --timeout   seconds per package before the run of that package is killed (default 7200); the
#                        per-file results written so far stay in results.json
#            --copy      test a copy of the package directory (<out-dir>/package) so edits during the run do not
#                        leak into it
#            --resume    keep the packages whose vitest already finished in <out-dir> (exit 0 or 1) and run the
#                        others again (killed, timed out or not started)
#        typescript-eslint.sh compare <base-out> <new-out> [--limit N] [--json FILE]
#            Lost, gained, absent, unrun and added tests per package. Exit 1 when a test that passed in the base
#            is lost, absent or unrun.
#        typescript-eslint.sh merge <out-dir> <in-dir>...
#            Combine shard runs of one package directory into one result directory.
#        typescript-eslint.sh restore
#            Put the clone's stock `typescript` link back (after a killed run).
# Heavy: run it under `systemd-run --user --collect --wait -p MemoryMax=12G -p MemorySwapMax=0
#   --setenv=PATH="$PATH" --setenv=HOME="$HOME" ...`. TSE_CLONE overrides the clone directory.
set -euo pipefail
case "${1:-}" in "" | help | -h | --help) sed -n '2,/^set -euo/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'; exit 0 ;; esac

{
here="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
tools="$here/typescript-eslint"
repo_root="$(cd -- "$here/../../.." && pwd)"

tse_tag="v8.71.2"
tse_commit="7488d681f6e0e96c2aaa1eee9c3d1b9b1ef077cb"
tse_url="https://github.com/typescript-eslint/typescript-eslint.git"
clone="${TSE_CLONE:-$repo_root/target/typescript-api/consumers/typescript-eslint}"
state="$clone.harness"
all_packages=(typescript-estree ast-spec type-utils parser project-service tsconfig-utils rule-tester typescript-eslint scope-manager eslint-plugin)

die() {
    echo "typescript-eslint.sh: $*" >&2
    exit 2
}

log() {
    echo "[$(date +%H:%M:%S)] $*"
}

take_lock() {
    mkdir -p "$state"
    exec 9>"$state/lock"
    if ! flock -n 9; then
        log "waiting for another typescript-eslint.sh in $clone"
        flock 9
    fi
}

# The pnpm of the clone's packageManager field: corepack's shim picks it in the clone, else corepack or npx.
resolve_pnpm() {
    local want
    want="$(node -p 'require(process.argv[1]).packageManager.split("@")[1].split("+")[0]' "$clone/package.json")"
    export COREPACK_ENABLE_DOWNLOAD_PROMPT=0
    if command -v pnpm >/dev/null && [ "$(cd "$clone" && pnpm --version 2>/dev/null)" = "$want" ]; then
        pnpm_cmd=(pnpm)
    elif command -v corepack >/dev/null && [ "$(cd "$clone" && corepack pnpm --version 2>/dev/null)" = "$want" ]; then
        pnpm_cmd=(corepack pnpm)
    else
        pnpm_cmd=(npx --yes "pnpm@$want")
    fi
    pnpm_version="$want"
}

# typescript-eslint overrides `typescript` with @typescript/typescript6, a shim whose lib/typescript.js and
# lib/tsserverlibrary.js are `module.exports = require("@typescript/old")`. Every `typescript` link in the
# workspace and in the store's peer links goes to that shim, so the one @typescript/old link decides which
# package all of them load. The shim stays a real directory under node_modules, so vitest keeps loading it
# with Node's own require, exactly as with the stock package.
old_link() {
    local links=("$clone"/node_modules/.pnpm/@typescript+typescript6@*/node_modules/@typescript/old)
    [ "${#links[@]}" = 1 ] && [ -L "${links[0]}" ] || die "cannot find the @typescript/old link in $clone/node_modules/.pnpm"
    printf '%s\n' "${links[0]}"
}

swap_in() {
    local pkg="$1" link
    link="$(old_link)" || exit 2
    if [ ! -f "$state/old-link.orig" ]; then
        readlink "$link" >"$state/old-link.orig"
    fi
    ln -sfn "$pkg" "$link.swap"
    mv -T "$link.swap" "$link"
}

restore_link() {
    [ -f "$state/old-link.orig" ] || return 0
    local link
    link="$(old_link)" || return 1
    ln -sfn "$(cat "$state/old-link.orig")" "$link.swap"
    mv -T "$link.swap" "$link"
    rm -f "$state/old-link.orig"
}

child=""

# timeout forwards TERM to vitest; --kill-after does not apply to a forwarded signal, so kill after 30 s here.
stop_child() {
    [ -n "$child" ] || return 0
    kill -TERM "$child" 2>/dev/null || return 0
    local waited=0
    while kill -0 "$child" 2>/dev/null && [ "$waited" -lt 30 ]; do
        sleep 1
        waited=$((waited + 1))
    done
    kill -KILL -- "-$child" 2>/dev/null || true
    log "run: stopped"
}

cmd_setup() {
    local force=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --force) force=1 ;;
            *) die "setup: unknown argument $1" ;;
        esac
        shift
    done
    mkdir -p "$(dirname "$clone")" "$state"
    if [ "$force" = 0 ] && [ -f "$state/setup.ok" ]; then
        log "setup: $clone is ready ($(head -1 "$state/setup.ok"))"
        return 0
    fi
    if [ ! -d "$clone/.git" ]; then
        log "setup: cloning $tse_tag into $clone"
        git -c advice.detachedHead=false clone --quiet --depth 1 --branch "$tse_tag" "$tse_url" "$clone"
    fi
    local head
    head="$(git -C "$clone" rev-parse HEAD)"
    [ "$head" = "$tse_commit" ] || die "setup: $clone is at $head, not $tse_tag ($tse_commit)"
    restore_link 2>/dev/null || true
    resolve_pnpm
    log "setup: pnpm $pnpm_version install (its postinstall builds every package with the stock TypeScript; log $state/setup.log)"
    local start=$SECONDS
    if ! (cd "$clone" && NX_DAEMON=false NX_NO_CLOUD=true NX_TUI=false NX_SKIP_NX_CACHE=true "${pnpm_cmd[@]}" install --frozen-lockfile) >"$state/setup.log" 2>&1; then
        tail -30 "$state/setup.log" >&2
        die "setup: pnpm install failed (log $state/setup.log)"
    fi
    local p
    for p in "${all_packages[@]}"; do
        [ -d "$clone/packages/$p" ] || die "setup: package $p is missing"
    done
    for p in typescript-estree parser type-utils utils rule-tester typescript-eslint eslint-plugin project-service tsconfig-utils scope-manager; do
        [ -f "$clone/packages/$p/dist/index.js" ] || die "setup: $p was not built (no dist/index.js); see $state/setup.log"
    done
    local orig
    orig="$(readlink "$(old_link)")"
    case "$orig" in
        */typescript@6.0.3/node_modules/typescript) ;;
        *) die "setup: @typescript/old points at $orig, not the store's typescript@6.0.3" ;;
    esac
    printf '%s %s pnpm@%s %s %ss\n' "$tse_tag" "$tse_commit" "$pnpm_version" "$(date -Is)" "$((SECONDS - start))" >"$state/setup.ok"
    log "setup: done in $((SECONDS - start))s"
}

check_package_dir() {
    local dir="$1"
    [ -f "$dir/package.json" ] || die "$dir has no package.json"
    [ -f "$dir/lib/typescript.js" ] || die "$dir has no lib/typescript.js (build npm/typescript-api with node npm/typescript-api/scripts/build.mjs)"
    [ -f "$dir/lib/tsserverlibrary.js" ] || die "$dir has no lib/tsserverlibrary.js"
}

cmd_run() {
    [ $# -ge 2 ] || die "run: need <typescript-package-dir|stock> <out-dir>"
    local pkg_arg="$1" out="$2"
    shift 2
    local packages="all" shard="" files="" workers=4 timeout_s=7200 copy=0 resume=0
    while [ $# -gt 0 ]; do
        case "$1" in
            --packages) packages="$2"; shift ;;
            --packages=*) packages="${1#*=}" ;;
            --shard) shard="$2"; shift ;;
            --shard=*) shard="${1#*=}" ;;
            --files) files="$2"; shift ;;
            --files=*) files="${1#*=}" ;;
            --workers) workers="$2"; shift ;;
            --workers=*) workers="${1#*=}" ;;
            --timeout) timeout_s="$2"; shift ;;
            --timeout=*) timeout_s="${1#*=}" ;;
            --copy) copy=1 ;;
            --resume) resume=1 ;;
            *) die "run: unknown argument $1" ;;
        esac
        shift
    done
    [[ "$workers" =~ ^[1-9][0-9]*$ ]] || die "run: --workers must be a positive integer"
    [[ "$timeout_s" =~ ^[1-9][0-9]*$ ]] || die "run: --timeout must be a positive integer"
    [ -z "$shard" ] || [[ "$shard" =~ ^[1-9][0-9]*/[1-9][0-9]*$ ]] || die "run: --shard must be i/n"

    local selected=()
    if [ "$packages" = all ]; then
        selected=("${all_packages[@]}")
    else
        local p known
        IFS=, read -r -a selected <<<"$packages"
        for p in "${selected[@]}"; do
            known=0
            for k in "${all_packages[@]}"; do [ "$k" = "$p" ] && known=1; done
            [ "$known" = 1 ] || die "run: unknown package $p (known: ${all_packages[*]})"
        done
    fi

    mkdir -p "$out"
    out="$(cd "$out" && pwd)"
    if [ "$resume" = 0 ] && [ -n "$(ls -A "$out")" ]; then
        die "run: $out is not empty (use a new directory, or --resume)"
    fi
    mkdir -p "$out/raw" "$out/logs"

    local pkg="" pkg_source=""
    if [ "$pkg_arg" != stock ]; then
        [ -d "$pkg_arg" ] || die "run: $pkg_arg is not a directory"
        pkg="$(cd "$pkg_arg" && pwd -P)"
        pkg_source="$pkg"
        check_package_dir "$pkg"
        if [ "$copy" = 1 ]; then
            if [ ! -f "$out/package/lib/typescript.js" ]; then
                log "run: copying $pkg to $out/package"
                mkdir -p "$out/package"
                tar -C "$pkg" --exclude=./node_modules --exclude=./.cache --exclude=./.git -cf - . | tar -C "$out/package" -xf -
            fi
            pkg="$out/package"
            check_package_dir "$pkg"
        fi
    fi

    take_lock
    [ -f "$state/setup.ok" ] || cmd_setup
    restore_link
    trap 'restore_link' EXIT
    trap 'stop_child; restore_link; [ ! -f "$out/run.json" ] || node "$tools/results.mjs" build "$out"; exit 130' INT TERM
    [ -z "$pkg" ] || swap_in "$pkg"

    local vitest="$clone/node_modules/.bin/vitest"
    [ -x "$vitest" ] || die "run: $vitest is missing (setup --force)"

    # Which typescript the workspace now loads, from the package that every test imports through.
    if ! (cd "$clone/packages/typescript-estree" && node "$tools/probe.cjs" "${pkg:-stock}") >"$out/probe.json" 2>"$out/logs/probe.log"; then
        cat "$out/logs/probe.log" >&2
        die "run: the swapped typescript does not load (log $out/logs/probe.log)"
    fi
    node "$tools/results.mjs" meta "$out" --clone "$clone" --tag "$tse_tag" --commit "$tse_commit" \
        --package-arg "$pkg_arg" --package-dir "${pkg:-}" --package-source "${pkg_source:-}" --shard "$shard" --files "$files" --workers "$workers" \
        --packages "$(IFS=,; echo "${selected[*]}")"
    log "run: $(node -p 'const p = require(process.argv[1]); `typescript ${p.version} from ${p.loadedFrom}`' "$out/probe.json")"

    local p start rc shard_args=() file_args=()
    [ -z "$shard" ] || shard_args=("--shard=$shard")
    [ -z "$files" ] || IFS=, read -r -a file_args <<<"$files"
    for p in "${selected[@]}"; do
        if [ "$resume" = 1 ] && grep -qE "\"package\":\"$p\",\"rc\":[01]," "$out/raw/packages.jsonl" 2>/dev/null; then
            log "run: $p already finished, skipped (--resume)"
            continue
        fi
        rm -f "$out/raw/$p.jsonl"
        if [ -f "$out/raw/packages.jsonl" ]; then
            grep -v "\"package\":\"$p\"," "$out/raw/packages.jsonl" >"$out/raw/packages.jsonl.tmp" || true
            mv "$out/raw/packages.jsonl.tmp" "$out/raw/packages.jsonl"
        fi
        log "run: $p"
        start="$(date +%s.%N)"
        rc=0
        # In the background with `wait`, so INT and TERM reach the trap at once (timeout runs vitest in its own
        # process group, out of reach of a signal to ours).
        (
            cd "$clone/packages/$p"
            export TSAPI_TSE_JSONL="$out/raw/$p.jsonl" TSAPI_TSE_PACKAGE="$p" TSAPI_TSE_PACKAGE_DIR="$clone/packages/$p" \
                NO_COLOR=1 FORCE_COLOR=0
            exec timeout --kill-after=30 "$timeout_s" "$vitest" run --config vitest.config.mts --maxWorkers="$workers" \
                --update=none --reporter="$tools/reporter.mjs" --passWithNoTests "${shard_args[@]}" "${file_args[@]}"
        ) >"$out/logs/$p.log" 2>&1 &
        child=$!
        wait "$child" || rc=$?
        child=""
        node -e 'const [p, rc, s] = process.argv.slice(1); process.stdout.write(JSON.stringify({ package: p, rc: Number(rc), wallSeconds: Math.round((Date.now() / 1000 - Number(s)) * 10) / 10, end: new Date().toISOString() }) + "\n")' \
            "$p" "$rc" "$start" >>"$out/raw/packages.jsonl"
        log "run: $p rc=$rc $(tail -1 "$out/logs/$p.log")"
    done

    restore_link
    trap - EXIT INT TERM
    if [ -n "$(git -C "$clone" status --porcelain --untracked-files=normal 2>/dev/null)" ]; then
        git -C "$clone" status --porcelain >"$out/logs/clone-dirty.txt"
        log "run: WARNING the clone has changed files after the run (see $out/logs/clone-dirty.txt)"
    fi
    node "$tools/results.mjs" build "$out"
    log "run: wrote $out/results.json and $out/summary.md"
}

case "$1" in
    setup)
        shift
        take_lock
        cmd_setup "$@"
        ;;
    run)
        shift
        cmd_run "$@"
        ;;
    compare)
        shift
        exec node "$tools/results.mjs" compare "$@"
        ;;
    merge)
        shift
        exec node "$tools/results.mjs" merge "$@"
        ;;
    restore)
        take_lock
        restore_link
        log "restore: @typescript/old -> $(readlink "$(old_link)")"
        ;;
    *) die "unknown command $1 (see help)" ;;
esac
exit
}
