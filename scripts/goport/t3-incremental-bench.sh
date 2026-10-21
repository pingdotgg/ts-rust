#!/usr/bin/env bash
# t3-incremental-bench.sh: cold vs incremental tsc times on a T3 Code checkout (t3inc1, 2026-10-21).
#
# Usage:
#   t3-incremental-bench.sh [-n RUNS] [-c CHANGE] [-p PROJECTS] [-t] [-x FLAGS] <t3code-dir> <tsc> [<tsc>...]
#   t3-incremental-bench.sh help
#
#   -n RUNS     runs per cell (default 3; the table shows the median and min-max)
#   -c CHANGE   which edit to make (default server-config):
#                 server-config     apps/server/src/config.ts: add an optional field to the
#                                   ServerConfig service shape (about 200 server files import it)
#                 contracts-literal packages/contracts/src/baseSchemas.ts: add a DpopFailureReason literal
#                 contracts-brand   packages/contracts/src/baseSchemas.ts: ThreadId brand "ThreadId" -> "ThreadIdX"
#                 contracts-field   packages/contracts/src/orchestrationV2.ts: optional field in OrchestrationV2AppThread
#   -p PROJECTS project dirs to check, comma separated (default: apps/server for server-config,
#               packages/contracts for the contracts-* edits). The contracts files are external library
#               files (found through node_modules) in every other project, so only packages/contracts
#               signs them.
#   -t          T3's own setup: add "incremental": true to tsconfig.base.json and run
#               `<tsc> --noEmit --extendedDiagnostics` in the project dir, as T3's typecheck script does.
#               The build info is then <project>/tsconfig.tsbuildinfo; the bench removes it before each
#               cold run and at exit. Without -t, each run is
#               `<tsc> -p <project> --noEmit --incremental --tsBuildInfoFile <tmp> --extendedDiagnostics`
#               in the checkout root, and the checkout's tsconfig files are not changed.
#   -x FLAGS    more tsc flags for every run, for example -x --isolatedModules
#   <tsc>       a tsc binary (tsc-rs lib/tsc, a Go lib/tsc or a ts-rust target/release/tsgo).
#               Use LABEL=PATH to name a column.
#
# One run of one tsc on one project is these cells, in this order:
#   cold      no build info
#   nochange  a run right after cold, no edit
#   incr      the edit, then a run (Theo's case)
#   revert    the edit undone, then a run
# Each round runs every tsc on every project once, so the tsc sides take turns.
# It prints wall seconds, the incr/cold ratio, the "Emit time" of incr (the nested d.ts signature
# emits) and whether incr printed the same diagnostics as a cold run with the edit.
# The edited files are restored at exit (also on error or Ctrl-C).
#
# Go's d.ts walk against the tsc-rs noEmit fallback (affected_files.rs skips_dts_walk)
# on one tsgo build: GOPORT_DTS_WALK_FALLBACK=0 turns the fallback off. Pass a wrapper script as one
# <tsc>, e.g. walk=/path/walk.sh with `GOPORT_DTS_WALK_FALLBACK=0 exec /path/tsgo "$@"`.
# Run it on a quiet machine (a boat sandbox or dbook-lan), not on zbook; compare only cells of one run.
set -euo pipefail

usage() { sed -n '2,/^set -euo/p' "$0" | sed '$d; s/^# \{0,1\}//'; }
[ "${1:-}" = help ] && { usage; exit 0; }

RUNS=3 CHANGE=server-config PROJECTS='' FLAGS='' T3MODE=''
while getopts n:c:p:x:th o; do
	case $o in
	n) RUNS=$OPTARG ;;
	c) CHANGE=$OPTARG ;;
	p) PROJECTS=$OPTARG ;;
	x) FLAGS=$OPTARG ;;
	t) T3MODE=1 ;;
	*) usage; exit 2 ;;
	esac
done
shift $((OPTIND - 1))
[ $# -ge 2 ] || { usage; exit 2; }
T3=$(cd "$1" && pwd)
shift

case $CHANGE in
server-config)
	FILE=apps/server/src/config.ts
	FROM='    readonly traceTimingEnabled: boolean;'
	TO='    readonly traceTimingEnabled: boolean;\n    readonly xBenchField?: string | undefined;'
	DEFPROJ=apps/server ;;
contracts-literal)
	FILE=packages/contracts/src/baseSchemas.ts
	FROM='  "time_window",'
	TO='  "time_window",\n  "x_bench_reason",'
	DEFPROJ=packages/contracts ;;
contracts-brand)
	FILE=packages/contracts/src/baseSchemas.ts
	FROM='makeEntityId("ThreadId")'
	TO='makeEntityId("ThreadIdX")'
	DEFPROJ=packages/contracts ;;
contracts-field)
	FILE=packages/contracts/src/orchestrationV2.ts
	FROM='export const OrchestrationV2AppThread = Schema.Struct({'
	TO='export const OrchestrationV2AppThread = Schema.Struct({\n  xBenchField: Schema.optional(Schema.String),'
	DEFPROJ=packages/contracts ;;
*) echo "unknown change: $CHANGE" >&2; exit 2 ;;
esac
IFS=, read -ra PROJ <<<"${PROJECTS:-$DEFPROJ}"
for p in "${PROJ[@]}"; do
	[ -f "$T3/$p/tsconfig.json" ] || { echo "no $T3/$p/tsconfig.json" >&2; exit 2; }
done
[ -f "$T3/$FILE" ] || { echo "no $T3/$FILE" >&2; exit 2; }
grep -qF -- "$FROM" "$T3/$FILE" || { echo "$FILE has no line: $FROM" >&2; exit 2; }

# replace <file> <from> <to>: replace the first <from> with <to> (\n in <to> is a newline).
replace() {
	python3 -I -c 'import sys; p, a, b = sys.argv[1:4]; s = open(p).read(); assert a in s, a; open(p, "w").write(s.replace(a, b.replace("\\n", "\n"), 1))' "$@"
}
# bi <project>: the build info file of a project.
bi() {
	if [ -n "$T3MODE" ]; then echo "$T3/$1/tsconfig.tsbuildinfo"; else echo "$WORK/${1//\//_}.tsbuildinfo"; fi
}

WORK=$(mktemp -d "${TMPDIR:-/tmp}/t3-incr-bench.XXXXXX")
cp "$T3/$FILE" "$WORK/orig"
cp "$T3/tsconfig.base.json" "$WORK/base.orig"
restore() {
	cp "$WORK/orig" "$T3/$FILE"
	cp "$WORK/base.orig" "$T3/tsconfig.base.json"
	if [ -n "$T3MODE" ]; then for p in "${PROJ[@]}"; do rm -f "$(bi "$p")"; done; fi
	rm -rf "$WORK"
}
trap restore EXIT
trap 'exit 130' INT TERM
[ -z "$T3MODE" ] || replace "$T3/tsconfig.base.json" '"noEmit": true,' '"noEmit": true,\n    "incremental": true,'

edit() { replace "$T3/$FILE" "$FROM" "$TO"; }
unedit() { cp "$WORK/orig" "$T3/$FILE"; }

# run <tsc> <project> <out-file>: one tsc run on a project; prints wall seconds.
run() {
	local s e
	s=$(date +%s.%N)
	if [ -n "$T3MODE" ]; then
		(cd "$T3/$2" && "$1" --noEmit --extendedDiagnostics $FLAGS >"$3" 2>&1) || true
	else
		(cd "$T3" && "$1" -p "$2" --noEmit --incremental --tsBuildInfoFile "$(bi "$2")" --extendedDiagnostics $FLAGS >"$3" 2>&1) || true
	fi
	e=$(date +%s.%N)
	awk -v s="$s" -v e="$e" 'BEGIN { printf "%.3f\n", e - s }'
}
# diags <out-file>: the diagnostic lines of a run, sorted.
diags() { grep -E '\): (error|warning|suggestion|message) TS' "$1" | sort; }
cell() { sort -n | awk '{ v[NR] = $1 } END { printf "%.2f (%.2f-%.2f)", v[int((NR + 1) / 2)], v[1], v[NR] }'; }
median() { sort -n | awk '{ v[NR] = $1 } END { printf "%.2f", v[int((NR + 1) / 2)] }'; }

for _ in $(seq "$RUNS"); do
	for arg in "$@"; do
		label=${arg%%=*} tsc=${arg#*=}
		[ "$label" = "$arg" ] && label=${tsc: -14}
		for p in "${PROJ[@]}"; do
			k="$WORK/$label.${p//\//_}"
			unedit; rm -f "$(bi "$p")"
			run "$tsc" "$p" "$WORK/o.cold" >>"$k.cold"
			run "$tsc" "$p" "$WORK/o.nochange" >>"$k.nochange"
			edit
			run "$tsc" "$p" "$k.o.incr" >>"$k.incr"
			grep -E '^Emit time' "$k.o.incr" | awk '{ sub("s", "", $3); print $3 }' >>"$k.emit" || true
			unedit
			run "$tsc" "$p" "$WORK/o.revert" >>"$k.revert"
		done
	done
done

printf '# %s, projects %s, change %s (%s), setup %s, flags [%s], %s runs, %s\n' "$T3" "${PROJ[*]}" "$CHANGE" "$FILE" \
	"$([ -n "$T3MODE" ] && echo 't3 (incremental in tsconfig.base.json, run in the project dir)' || echo cli)" \
	"$FLAGS" "$RUNS" "$(date -u +%FT%TZ)"
printf '%-14s %-24s %-20s %-20s %-20s %-20s %-6s %-10s %s\n' tsc project cold incr revert nochange ratio incr-emit same-diags
for arg in "$@"; do
	label=${arg%%=*} tsc=${arg#*=}
	[ "$label" = "$arg" ] && label=${tsc: -14}
	for p in "${PROJ[@]}"; do
		k="$WORK/$label.${p//\//_}"
		# Same diagnostics as a cold run with the edit?
		edit; rm -f "$(bi "$p")"
		run "$tsc" "$p" "$WORK/o.coldedit" >/dev/null
		same=yes
		cmp -s <(diags "$k.o.incr") <(diags "$WORK/o.coldedit") || same=no
		unedit; rm -f "$(bi "$p")"
		touch "$k.emit"
		c=$(median <"$k.cold") i=$(median <"$k.incr")
		printf '%-14s %-24s %-20s %-20s %-20s %-20s %-6s %-10s %s\n' "$label" "$p" \
			"$(cell <"$k.cold")" "$(cell <"$k.incr")" "$(cell <"$k.revert")" "$(cell <"$k.nochange")" \
			"$(awk -v i="$i" -v c="$c" 'BEGIN { printf "%.2f", i / c }')" "$(median <"$k.emit")s" "$same"
	done
done
