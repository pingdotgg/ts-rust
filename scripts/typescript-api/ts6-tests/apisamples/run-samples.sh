#!/usr/bin/env bash
# Runs the 9 TS 6.0 APISample programs against a `typescript` package directory and writes each
# sample's normalized output (and, for the watch samples, the files it wrote) to <out-dir>.
# usage: run-samples.sh <library-dir> <out-dir> <samples-dir with APISample_*.js>
set -u

usage="usage: run-samples.sh <library-dir> <out-dir> <samples-dir with APISample_*.js>"
if [ "${1:-}" = help ]; then
    echo "$usage"
    exit 0
fi
if [ $# -ne 3 ]; then
    echo "$usage" >&2
    exit 2
fi

LIB=$(readlink -f "$1")
OUT=$(readlink -m "$2")
SAMPLES=$(readlink -f "$3")
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
SAMPLE_TIMEOUT=${TS6_TESTS_SAMPLE_TIMEOUT:-120}
WATCH_TIMEOUT=${TS6_TESTS_WATCH_TIMEOUT:-60}
WATCH_SETTLE=${TS6_TESTS_WATCH_SETTLE:-1}

mkdir -p "$OUT"

# Drop stack frames, colors, clock times and machine paths so outputs compare across libraries.
normalize() {
    sed -E \
        -e '/^[[:space:]]+at /d' \
        -e '/^Node\.js v/d' \
        -e "s#$OUT/work-[A-Za-z_]+#<cwd>#g" \
        -e 's#\x1b\[[0-9;]*[A-Za-z]##g' \
        -e 's#[0-9]{1,2}:[0-9]{2}:[0-9]{2}( [AP]M)?#<time>#g' \
        -e "s#$LIB#<lib>#g"
}

# $1 sample, $2 fixture dir (optional): a work dir with node_modules/typescript -> the library.
setup() {
    local work="$OUT/work-$1"
    mkdir -p "$work/node_modules"
    ln -s "$LIB" "$work/node_modules/typescript"
    if [ -n "${2:-}" ]; then
        cp -r "$HERE/fixtures/$2/." "$work/"
    fi
    cp "$HERE"/drive-*.cjs "$work/"
    touch -d '-1 min' "$work"/*
    cp "$SAMPLES/APISample_$1.js" "$work/sample.js"
}

# $1 sample, rest: the command, run in the sample's work dir.
run() {
    local name=$1
    shift
    (cd "$OUT/work-$name" && timeout "$SAMPLE_TIMEOUT" "$@") > "$OUT/$name.raw" 2>&1
    echo "exit=$?" >> "$OUT/$name.raw"
    normalize < "$OUT/$name.raw" > "$OUT/$name.out"
    rm "$OUT/$name.raw"
}

# Polls <file> until <regex> matches <count> lines, the process <pid> ends or WATCH_TIMEOUT s pass.
wait_for_lines() {
    local file=$1 regex=$2 count=$3 pid=$4
    local deadline=$((SECONDS + WATCH_TIMEOUT))
    while [ "$SECONDS" -lt "$deadline" ] && kill -0 "$pid" 2>/dev/null; do
        if [ "$(grep -cE "$regex" "$file")" -ge "$count" ]; then
            return 0
        fi
        sleep 0.1
    done
    return 1
}

# SIGINT, as Ctrl-C would; SIGKILL when the process still runs 5 s later.
stop() {
    local pid=$1
    kill -INT "$pid" 2>/dev/null
    for _ in $(seq 50); do
        if ! kill -0 "$pid" 2>/dev/null; then
            return
        fi
        sleep 0.1
    done
    kill -KILL "$pid" 2>/dev/null
}

# $1 sample, $2 regex of the line that ends a compile, $3 how many such lines the first compile
# prints: start the watcher, wait for its first compile, edit src/b.ts (and b.ts), wait for one more
# compile, then stop it with SIGINT. Output markers instead of fixed delays keep this stable under load.
watch_run() {
    local name=$1 regex=$2 first=$3
    local work="$OUT/work-$name"
    local raw="$OUT/$name.raw"
    (
        cd "$work" || exit 9
        node -r "$HERE/preload.cjs" sample.js > "$raw" 2>&1 &
        local pid=$!
        if wait_for_lines "$raw" "$regex" "$first" "$pid"; then
            sleep "$WATCH_SETTLE"
            printf 'import { a } from "./a";\nexport const b: number = a;\n' > src/b.ts
            if [ -f b.ts ]; then
                printf 'export const b: number = 1;\n' > b.ts
            fi
            wait_for_lines "$raw" "$regex" "$((first + 1))" "$pid" && sleep "$WATCH_SETTLE"
        fi
        stop "$pid"
        wait "$pid"
        echo "exit=$?" >> "$raw"
    )
    normalize < "$raw" > "$OUT/$name.out"
    rm "$raw"
    (cd "$work" && find . -path ./node_modules -prune -o -type f -newer sample.js -print | sort | while read -r file; do
        echo "== $file"
        cat "$file"
    done) > "$OUT/$name.files"
}

PRELOAD=(node -r "$HERE/preload.cjs")

setup compile compile
run compile "${PRELOAD[@]}" sample.js ok.ts bad.ts
setup linter linter
run linter "${PRELOAD[@]}" sample.js lintme.ts
setup transform
run transform "${PRELOAD[@]}" sample.js
setup parseConfig compile
run parseConfig node drive-parseConfig.cjs ./sample.js ok.ts bad.ts
setup jsdoc
run jsdoc node drive-jsdoc.cjs sample.js

for name in Watch WatchWithDefaults WatchWithOwnWatchHost; do
    setup "$name" project
    watch_run "$name" "Watching for file changes" 1
done
setup watcher project
cp "$OUT/work-watcher/src/a.ts" "$OUT/work-watcher/a.ts"
printf 'export const b: string = 1;\n' > "$OUT/work-watcher/b.ts"
watch_run watcher "^Emitting " 2

echo "results in $OUT"
