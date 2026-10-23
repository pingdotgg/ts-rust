#!/bin/bash
# usage: perf.sh [--mode default|st|both] [--rounds N] <label> <bin>... [-- <tsgo arg>...]
#        perf.sh compare <base label or dir> <new label or dir>
#        perf.sh host                     print this host's line
#        perf.sh env                      print the caller's variables that pass on to tsgo
# Runs each binary on query, hono, zod and effect (`<bin> -p <tsconfig> [tsgo args]`) under perf stat
# (instructions:u, cycles:u, task-clock) and /usr/bin/time (max RSS), and prints the median per binary with the
# delta to bin0. Without `-- --noEmit` the emit runs too (the read-only inputs refuse the writes): it is about
# 25 percent of the effect instructions, so a checker change shows a smaller percentage.
# Modes: default = the default 4 checkers (wall and RSS); st = --singleThreaded (instructions:u for the
# 1 percent bar; tsgo only); both = st, then default, in each round.
# Rounds: PERF_ROUNDS or --rounds; default 5 (wall on mini-743d), 3 on a testbox. One untimed run of each binary
# on each project comes first (page cache). Runs are interleaved (run 1 of every binary, then run 2, ...) and
# the binary order turns each round, so drift hits every side the same way.
# Wall is perf's own elapsed time (microseconds, without the sudo and perf start). Compare wall only within
# one perf.sh run, on mini-743d. cup2 is not a wall host (A/A at median of 5: zod 3.0, query 11.5 percent),
# and alvin has no perf. On a Blacksmith testbox wall is not valid (A/A up to 7.5 percent at median of 3,
# tbxperf0) and the summary leaves it out: use `testbox.sh perf` there for instructions:u (both sides on one
# testbox) and peak RSS. cycles:u is in runs.tsv only: it is not a 1 percent metric on any host (tbxperf0).
# A/A (tbxperf1, testbox and mini-743d): st instructions:u within 0.005 percent on every project. At 4 checkers
# the counts include waits that depend on timing: query up to 0.42 percent (median of 3 on a testbox), hono
# 0.09, zod and effect 0.03, so use st for an instruction claim. RSS at median of 3: query (100 MiB) up to
# 2 percent, the others 0.6. Wall on mini-743d at median of 5: zod and effect up to 1.1 percent (tbxperf0: 1.5
# to 1.8 at the 95th percentile), hono 0.4, query (45 ms) up to 6.4 percent.
# Env: tsgo runs under `env -i` on every host, so it sees the same variables with or without sudo: HOME,
# PATH=/usr/local/bin:/usr/bin:/bin, LC_ALL=C, GOPORT_LAUNCH=0, and the caller's GOPORT_* (not GOPORT_PIN*),
# _RJEM_MALLOC_CONF, MALLOC_*, GLIBC_TUNABLES, GOMAXPROCS, GOGC, GOMEMLIMIT and GODEBUG (`perf.sh env`).
# host.txt records them (the env line), and `compare` refuses two runs with a different env.
# Host line: CPU family/model and name, microcode, kernel, perf version, THP and the virtio balloon free page
# reporting state. `compare` (bin0 of BASE against each bin of NEW, instructions:u and RSS only) refuses two
# runs with different host lines, tsgo args or env. Counts and RSS differ a little between host types.
# perf: the first of PERF_BIN, `perf`, /usr/lib/linux-tools/*/perf that runs (Ubuntu's /usr/bin/perf fails on
# a testbox kernel). perf_event_paranoid 3 or more (mini-743d: 4): perf runs under `sudo -n` and the binary
# under setpriv as this user.
# cycles:u: a probe at the start picks the mode, and host.txt records it (`cycles joint` or `cycles separate`).
# joint: one perf stat run counts instructions:u and cycles:u (dbook-lan). separate: the run counts
# instructions:u alone, and a second run of the same command right after it counts cycles:u alone, so each
# cell takes two runs. The testbox guest (Zen 5) counts only the first hardware event of a set: the second
# reads 0, and in a multi-threaded run a thread can put instructions:u on the dead counter (perfcyc1: joint
# runs lost up to 27 percent of their instructions:u in the default mode). PERF_CYCLES=joint or separate
# skips the probe. `compare` warns when the two runs used different modes.
# The script refuses to start when the 1-minute load is over PERF_MAX_LOAD (default 1.5). PERF_WAIT=1 waits up
# to 30 minutes for a quiet host instead. zbook is rarely quiet while agents build: time on mini-743d through
# remote.sh (dbook-lan is kept for revision evidence), every side on the same host.
# Output: target/continuation-r97-goport/perf/<label>/ (must not exist): summary.txt, host.txt, bins.txt,
# runs.tsv (one row per run), raw/ (perf, time and stderr files per run; .cyc.perf and .cyc.err of the
# separate cycles:u runs).
set -uo pipefail
ROOT=/home/theo/Code/sandbox/ts-rust
PERF_DIR=$ROOT/target/continuation-r97-goport/perf
# tsgo on 4 KiB pages hands the work to a worker (R137), so wait4 RSS and CPU would show only the
# launcher. Measure the process that does the work.
export GOPORT_LAUNCH=0
# The caller's variables that change what tsgo does. They pass on to tsgo on every host (env -i drops the
# rest) and go into host.txt. GOPORT_PIN* are for pin.py, not tsgo; GOPORT_LAUNCH is always 0 here.
PASS_RE='^(GOPORT_[A-Z0-9_]+|_RJEM_MALLOC_CONF|MALLOC_[A-Z0-9_]+|GLIBC_TUNABLES|GOMAXPROCS|GOGC|GOMEMLIMIT|GODEBUG)$'
# tsgo_env: the caller's variables that pass, as sorted NAME=VALUE lines.
tsgo_env() {
  local k
  for k in $(compgen -e | sort); do
    [[ $k =~ $PASS_RE && $k != GOPORT_LAUNCH && $k != GOPORT_PIN* ]] && printf '%s=%s\n' "$k" "${!k}"
  done
  return 0
}
usage() { sed -n '2,/^set -uo/p' "$0" | sed '$d'; }
die() { echo "perf.sh: $*" >&2; exit 2; }

# on_testbox: a Blacksmith testbox (the workflow writes the testbox env there). Its clock is not valid for wall.
on_testbox() { [[ -f /home/runner/testbox.env ]]; }

# find_perf: print the first perf that runs.
find_perf() {
  local p
  for p in "${PERF_BIN:-}" "$(command -v perf)" $(ls -d /usr/lib/linux-tools/*/perf 2> /dev/null | sort -rV); do
    [[ -n $p ]] && "$p" --version > /dev/null 2>&1 && { echo "$p"; return; }
  done
  return 1
}

# cycles_mode: print joint when cycles:u counts next to instructions:u in 3 probe runs, else separate (uses
# P, PREFIX and INNER). No CPU retires 64 instructions a cycle (the probe runs at about 5 on dbook-lan), so a
# lower cycles:u reading is a dead counter (the testbox reads 0, or a few thousand now and then).
cycles_mode() {
  local _ ins cyc
  for _ in 1 2 3; do
    read -r ins cyc < <("${PREFIX[@]}" "$P" stat -x, -e instructions:u,cycles:u -- "${INNER[@]}" \
      awk 'BEGIN {for (i = 0; i < 1e6; i++) s += i}' 2>&1 > /dev/null |
      awk -F, '$3 ~ /^instructions/ {i = $1} $3 ~ /^cycles/ {c = $1} END {print i + 0, c + 0}')
    ((ins > 0 && cyc * 64 >= ins)) || { echo separate; return; }
  done
  echo joint
}

# host_line PERF: the facts that two runs must share before their counts compare.
host_line() {
  local cpu thp balloon=none d
  cpu=$(awk -F'[ \t]*: ' '/^$/ {exit} {v[$1] = $2}
    END {printf "cpu=%s/%s \"%s\" ucode=%s", v["cpu family"], v["model"], v["model name"], v["microcode"]}' /proc/cpuinfo)
  thp=$(sed 's/.*\[\(.*\)\].*/\1/' /sys/kernel/mm/transparent_hugepage/enabled 2> /dev/null)
  # Virtio device 5 is the balloon; feature bit 5 is free page reporting (freed guest memory goes back to the
  # host, and the next use takes host page faults that steal and load do not show: tbxperf0 skeptic).
  for d in /sys/bus/virtio/devices/*; do
    [[ $(cat "$d/device" 2> /dev/null) == 0x0005 ]] || continue
    if [[ -e $d/driver && $(cut -c6 "$d/features") == 1 ]]; then balloon=reporting-on; else balloon=reporting-off; fi
  done
  echo "$cpu kernel=$(uname -r) perf=$("$1" --version | awk '{print $3}') thp=${thp:-none} balloon=$balloon"
}

# summary BASE_DIR NEW_DIR: medians per project, mode and bin of NEW, with the delta to bin0 of BASE. With
# BASE == NEW it is the summary of one run (wall too, when valid); otherwise a compare (no wall).
summary() {
  python3 -I - "$1" "$2" << 'PY'
import csv, os, statistics, sys

# load DIR: host.txt as a dict, the runs of runs.tsv grouped by (mode, project, bin), and bins.txt rows.
def load(d):
    info = dict(l.rstrip("\n").split(" ", 1) for l in open(os.path.join(d, "host.txt")) if " " in l)
    info.setdefault("args", "none")
    info.setdefault("env", "none")
    info.setdefault("cycles", "joint")
    cells = {}
    for r in csv.DictReader(open(os.path.join(d, "runs.tsv")), delimiter="\t"):
        cells.setdefault((r["mode"], r["project"], r["bin"]), []).append(r)
    bins = [l.rstrip("\n").split(" ", 2) for l in open(os.path.join(d, "bins.txt"))]
    return info, cells, bins

base_dir, new_dir = sys.argv[1:3]
one = os.path.realpath(base_dir) == os.path.realpath(new_dir)
bi, bcells, bbins = load(base_dir)
ni, cells, nbins = load(new_dir)
for key, what in ("line", "host lines"), ("args", "tsgo args"), ("env", "tsgo env"):
    if bi[key] != ni[key]:
        print(f"different {what}: counts do not compare\n  {base_dir}: {bi[key]}\n  {new_dir}: {ni[key]}")
        sys.exit(3)
wall = one and ni["testbox"] == "no"
order = {"query": 0, "hono": 1, "zod": 2, "effect": 3}

def med(cells, key):
    return {k: statistics.median(float(r[key]) for r in v) for k, v in cells.items()}

metrics = [("wall_us", "wall ms", 1e-3, "%.2f")] if wall else []
metrics += [("maxrss_kib", "RSS MiB", 1 / 1024, "%.1f"), ("instructions_u", "instructions:u", 1, "%d")]
bm = {m: med(bcells, m) for m, *_ in metrics}
nm = {m: med(cells, m) for m, *_ in metrics}
print(f"host {ni['name']} ({'testbox' if ni['testbox'] == 'yes' else 'host'}), mode {ni['mode']}, rounds {ni['rounds']}, medians")
print(f"host-line {ni['line']}")
print(f"tsgo args {ni['args']}")
print(f"tsgo env {ni['env']}")
if one:
    print("wall: valid (compare only within this run)" if wall else "wall: not valid on a testbox (time on dbook-lan)")
else:
    print(f"compare: bin0 of {base_dir} (base) against {new_dir}; wall compares only within one run")
    print(f"base {bbins[0][1][:12]} {bbins[0][2]}")
for k, sha, path in nbins:
    print(f"bin{k} {sha[:12]} {path}")
hdr = f"{'project':8} {'mode':8} {'bin':5}" + "".join(f" {t:>24}" for _, t, _, _ in metrics)
print(hdr)
keys = sorted({(m, p) for (m, p, _) in cells}, key=lambda k: (k[0], order.get(k[1], 9), k[1]))
warn = []
if bi["cycles"] != ni["cycles"]:
    warn.append(f"cycles:u {bi['cycles']} in the base, {ni['cycles']} here: joint runs on a testbox can lose default-mode instructions:u")
for mode, proj in keys:
    rcs = {r["rc"] for b in nbins for r in cells.get((mode, proj, b[0]), [])}
    if len(rcs) > 1:
        warn.append(f"{proj} {mode}: exit codes differ between runs ({', '.join(sorted(rcs))}): see raw/*.err")
    lines = [] if one else [("base", None)]
    lines += [(f"bin{b[0]}", b[0]) for b in nbins]
    for label, b in lines:
        out = f"{proj:8} {mode:8} {label:5}"
        for m, _, scale, fmt in metrics:
            base = bm[m].get((mode, proj, "0"))
            v = base if b is None else nm[m].get((mode, proj, b))
            if v is None:
                out += f" {'-':>24}"
                continue
            cell = fmt % (v * scale)
            if b is not None and (b != "0" or not one) and base:
                pct = 100 * (v / base - 1)
                cell += (" %+.3f%%" if m == "instructions_u" else " %+.2f%%") % pct
            out += f" {cell:>24}"
        print(out)
for w in warn:
    print("warning:", w)
PY
}

case ${1:-} in
  "" | help | -h | --help) usage; exit 0 ;;
  host)
    P=$(find_perf) || die "no perf that runs"
    host_line "$P"
    exit 0
    ;;
  env)
    tsgo_env
    exit 0
    ;;
  compare)
    (($# == 3)) || die "usage: perf.sh compare <base label or dir> <new label or dir>"
    a=$2 b=$3
    [[ -d $a ]] || a=$PERF_DIR/$a
    [[ -d $b ]] || b=$PERF_DIR/$b
    for d in "$a" "$b"; do [[ -f $d/runs.tsv && -f $d/host.txt ]] || die "no perf.sh result in $d"; done
    summary "$a" "$b"
    exit
    ;;
esac

mode=default rounds=${PERF_ROUNDS:-}
while [[ ${1:-} == --* ]]; do
  (($# >= 2)) || die "$1 needs a value"
  case $1 in
    --mode) mode=$2 ;;
    --rounds) rounds=$2 ;;
    *) die "unknown option $1 (perf.sh help)" ;;
  esac
  shift 2
done
# The old form: perf.sh <bin> <label>.
if [[ $# -eq 2 && -x $1 && ! -x $2 ]]; then set -- "$2" "$1"; fi
[[ $# -ge 2 ]] || { usage; exit 2; }
L=$1
shift
case $mode in
  default) MODES=(default) ;;
  st) MODES=(st) ;;
  both) MODES=(st default) ;;
  *) die "unknown mode $mode (default, st or both)" ;;
esac
[[ -n $L && $L != */* && $L != .* ]] || die "the label must be a dir name: $L"
rounds=${rounds:-$(on_testbox && echo 3 || echo 5)}
[[ $rounds =~ ^[1-9][0-9]*$ ]] || die "rounds must be a positive number: $rounds"
BINS=() TARGS=()
while (($#)); do
  if [[ $1 == -- ]]; then
    shift
    TARGS=("$@")
    break
  fi
  [[ -f $1 && -x $1 ]] || die "not an executable file: $1"
  BINS+=("$(realpath -s -- "$1")")
  shift
done
((${#BINS[@]})) || die "no binary"
O=$PERF_DIR/$L
[[ ! -e $O ]] || die "$O exists (results are never replaced)"

P=$(find_perf) || die "no perf that runs (perf --version fails; no /usr/lib/linux-tools/*/perf)"
mapfile -t PASS < <(tsgo_env)
# The same env for tsgo on every host: sudo's env_reset would drop the caller's variables, the plain path keeps
# all of them.
TENV=(env -i HOME="$HOME" PATH=/usr/local/bin:/usr/bin:/bin LC_ALL=C GOPORT_LAUNCH=0 "${PASS[@]}")
# C locale: no thousands separators and a decimal point in perf's output.
PREFIX=(env LC_ALL=C) INNER=("${TENV[@]}")
if (($(cat /proc/sys/kernel/perf_event_paranoid) > 2)); then
  # paranoid 3+ blocks user counters: perf runs as root and setpriv drops back to this user for the binary.
  sudo -n true 2> /dev/null || die "perf_event_paranoid is over 2 and sudo -n does not work"
  PREFIX=(sudo -n env LC_ALL=C)
  INNER=(setpriv --reuid="$(id -u)" --regid="$(id -g)" --clear-groups "${TENV[@]}")
fi
case ${PERF_CYCLES:-auto} in
  auto) CYC=$(cycles_mode) ;;
  joint | separate) CYC=$PERF_CYCLES ;;
  *) die "PERF_CYCLES must be auto, joint or separate: $PERF_CYCLES" ;;
esac
EVENTS=instructions:u,cycles:u,task-clock
[[ $CYC == joint ]] || EVENTS=instructions:u,task-clock
PAIRS=("query:target/project-inputs/query/source/packages/query-core/tsconfig.prod.json"
  "hono:target/project-inputs/hono/source/tsconfig.build.json"
  "zod:target/project-inputs/zod/source/packages/zod/tsconfig.json"
  "effect:target/project-inputs/effect/source/packages/effect/tsconfig.json")
for pair in "${PAIRS[@]}"; do
  c=$ROOT/${pair#*:}
  [[ -f $c ]] || die "no project input $c"
  # A writable input takes the emit (build info, dist/), and the next run would be another build.
  [[ ! -w ${c%/*} ]] || die "${c%/*} is writable: lock the project inputs (scripts/project-inputs.sh)"
done

exec 8> /tmp/goport-perf.lock
flock -n 8 || { echo "another perf.sh run holds /tmp/goport-perf.lock; waiting"; flock 8; }
max=${PERF_MAX_LOAD:-1.5}
load() { cut -d' ' -f1 /proc/loadavg; }
quiet() { awk -v l="$(load)" -v m="$max" 'BEGIN { exit !(l <= m) }'; }
if ! quiet; then
  [[ ${PERF_WAIT:-0} == 1 ]] || { echo "load $(load) > $max on $(hostname): timing would be noise. Use a quiet host or PERF_WAIT=1." >&2; exit 3; }
  for _ in $(seq 180); do quiet && break; sleep 10; done
  quiet || { echo "load still $(load) after 30 minutes" >&2; exit 3; }
fi

mkdir -p "$O/raw"
{
  echo "line $(host_line "$P")"
  echo "name $(hostname)"
  echo "testbox $(on_testbox && echo yes || echo no)"
  echo "mode $mode"
  echo "rounds $rounds"
  echo "args ${TARGS[*]:-none}"
  echo "env ${PASS[*]:-none}"
  echo "perf $P$([[ ${PREFIX[0]} == sudo ]] && echo " (sudo -n, setpriv)")"
  echo "cycles $CYC"
  echo "start $(date -u +%FT%TZ) load $(load)"
} > "$O/host.txt"
for k in "${!BINS[@]}"; do echo "$k $(sha256sum < "${BINS[k]}" | cut -d' ' -f1) ${BINS[k]}"; done > "$O/bins.txt"
printf 'round\tmode\tproject\tbin\tpos\twall_us\tmaxrss_kib\tinstructions_u\tcycles_u\ttask_clock_ms\trc\tload1\n' > "$O/runs.tsv"
cd "$ROOT" || die "no $ROOT"
nb=${#BINS[@]}
# One untimed run of each binary on each project: the first run reads the binary and the inputs from disk.
for pair in "${PAIRS[@]}"; do
  for b in "${BINS[@]}"; do "${TENV[@]}" "$b" -p "${pair#*:}" "${TARGS[@]}" > /dev/null 2>&1; done
done
for ((r = 1; r <= rounds; r++)); do
  for m in "${MODES[@]}"; do
    flags=()
    [[ $m == st ]] && flags=(--singleThreaded)
    for pair in "${PAIRS[@]}"; do
      n=${pair%%:*} c=${pair#*:}
      for ((i = 0; i < nb; i++)); do
        k=$(((i + r - 1) % nb))
        f=$O/raw/$n-$m-bin$k-$r
        l=$(load)
        rc=0
        /usr/bin/time -f %M -o "$f.time" "${PREFIX[@]}" "$P" stat -o "$f.perf" -e "$EVENTS" -- \
          "${INNER[@]}" "${BINS[k]}" -p "$c" "${flags[@]}" "${TARGS[@]}" > /dev/null 2> "$f.err" || rc=$?
        # perf's human output: "<value> [unit] <event>" and "<seconds> seconds time elapsed". -x drops the
        # elapsed line, and duration_time reads 0 next to other events in perf 7.2. The task-clock unit: perf 7.0
        # writes "msec", perf 6.17 writes nanoseconds with no unit column.
        read -r ins cyc task wall < <(awk '$2 ~ /^instructions/ {i = $1} $2 ~ /^cycles/ {c = $1}
          $2 ~ /^task-clock/ {t = $1 / 1e6}
          $3 ~ /^task-clock/ {t = $1; if ($2 == "nsec") t /= 1e6; else if ($2 == "usec") t /= 1e3}
          / seconds time elapsed/ {w = $1}
          END {printf "%s %s %s %d\n", i, c == "" ? "-" : c, t == "" ? "-" : t, w * 1e6}' "$f.perf")
        [[ $ins =~ ^[0-9]+$ && $wall -gt 0 ]] || die "no counters in $f.perf (rc $rc): $(head -3 "$f.err")"
        if [[ $CYC == separate ]]; then
          # The same command again, with cycles:u alone in its set. RSS, wall and task-clock stay from the first run.
          crc=0
          "${PREFIX[@]}" "$P" stat -o "$f.cyc.perf" -e cycles:u -- \
            "${INNER[@]}" "${BINS[k]}" -p "$c" "${flags[@]}" "${TARGS[@]}" > /dev/null 2> "$f.cyc.err" || crc=$?
          cyc=$(awk '$2 ~ /^cycles/ && $1 ~ /^[0-9]+$/ {c = $1} END {print c == "" ? "-" : c}' "$f.cyc.perf")
          ((crc == rc)) || echo "perf.sh: $f: exit code $rc, then $crc in the cycles:u run" >&2
        fi
        printf '%d\t%s\t%s\t%d\t%d\t%d\t%d\t%s\t%s\t%s\t%d\t%s\n' "$r" "$m" "$n" "$k" "$i" "$wall" "$(tail -1 "$f.time")" \
          "$ins" "$cyc" "$task" "$rc" "$l" >> "$O/runs.tsv"
      done
    done
  done
done
echo "end $(date -u +%FT%TZ) load $(load) max $(awk -F'\t' 'NR > 1 && $12 > m {m = $12} END {print m + 0}' "$O/runs.tsv")" >> "$O/host.txt"
summary "$O" "$O" | tee "$O/summary.txt"
echo "results $O"
