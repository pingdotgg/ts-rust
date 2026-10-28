#!/usr/bin/env bash
# NestJS end-to-end harness for a `typescript` package (TS 6.0 API). See nest/README.md.
set -euo pipefail

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

usage() {
    cat <<'EOF'
usage:
  nest.sh help
  nest.sh run <typescript-package-dir|tgz> <out-dir> [--scenario a,b,...] [--env NAME=VALUE]... [--force]
  nest.sh compare <base-out> <new-out> [--out <dir>] [--max-diff-lines N]
  nest.sh make-nest-new-fixture [name...]

run      npm-packs the package (or takes a .tgz), installs it into each scenario's app copy under
         target/typescript-api/consumers/nest/<scenario> with "typescript": "file:<tgz>" plus
         "overrides": {"typescript": "$typescript"}, runs the scenario and writes
         <out-dir>/<scenario>/{status.json,logs/,dist/,...}, <out-dir>/results.json and summary.md.
         Exit 0 when every scenario passes, 1 when one fails, 2 on a usage or setup error.
         Scenarios (letters or ids; g = g1,g2): a=nest-cli-unit b=swagger-sample c=cjs-paths
                    d=cjs-paths-watch e=swagger-swc f=graphql-sample g1=cjs-paths-webpack
                    g2=cjs-paths-webpack-swagger h1=nest-new-esm h2=nest-new-cjs i=ts-node
         --env applies to builds, tests and apps, not to npm (e.g. --env TSRS_API_CHANNEL=process).
         TSRS_* variables of the caller pass through to every command.
         Runs inside `systemd-run --user` with MemoryMax=12G unless NEST_HARNESS_NO_UNIT=1.
         NEST_HARNESS_TIMEOUT_SCALE=N multiplies every timeout.
compare  diffs two run outputs: scenario status, step and check changes, per-test pass/fail
         changes, dist and other artifacts byte by byte (unified diff excerpts), api-json as JSON.
         .tsbuildinfo files (also inside dist trees) are diffed but never counted.
         Exit 0 when nothing differs, 1 when something does, 2 on a usage error.
         --out <dir> also writes compare.json, compare.md and the full diffs there.
make-nest-new-fixture [name...]
         regenerates fixtures/nest-new-app and nest-new-cjs (pins.json nestNew): the schematics app
         template via `nest new --skip-git --skip-install` (cjs: the same schematic with --type=cjs),
         dependencies pinned to the versions npm resolves at pins.json npmBefore, and its
         package-lock.json. Needs network access.
EOF
}

run_in_unit() {
    local -a setenv=(--setenv=PATH="$PATH" --setenv=HOME="$HOME" --setenv=NEST_HARNESS_IN_UNIT=1)
    local name
    while IFS= read -r name; do
        setenv+=(--setenv="$name=${!name}")
    done < <(compgen -e | grep -E '^(TSRS_|NEST_HARNESS_|npm_config_|NPM_CONFIG_)' || true)
    exec systemd-run --user --collect --wait --pipe --quiet \
        -p MemoryMax=12G -p MemorySwapMax=0 \
        --unit="nest-harness-$$-$(date +%s)" \
        --working-directory="$PWD" \
        "${setenv[@]}" \
        node "$DIR/nest/run.mjs" "$@"
}

cmd="${1:-help}"
case "$cmd" in
    help | -h | --help)
        usage
        ;;
    run)
        shift
        if [[ "${NEST_HARNESS_NO_UNIT:-0}" != "1" && "${NEST_HARNESS_IN_UNIT:-0}" != "1" ]] && command -v systemd-run >/dev/null; then
            run_in_unit "$@"
        fi
        exec node "$DIR/nest/run.mjs" "$@"
        ;;
    compare)
        shift
        exec node "$DIR/nest/compare.mjs" "$@"
        ;;
    make-nest-new-fixture)
        shift
        exec node "$DIR/nest/make-nest-new-fixture.mjs" "$@"
        ;;
    *)
        usage >&2
        exit 2
        ;;
esac
