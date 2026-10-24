#!/usr/bin/env bash
# Blacksmith testboxes: a warm 32 vCPU Linux x64 runner (.github/workflows/testbox.yml) that runs a command
# against this worktree's files. `run` syncs this worktree to the testbox (the CLI's rsync) and runs the
# command in the synced repo root. target/, the cargo registry and the files of testbox-data.txt sit on
# sticky disks.
# usage: testbox.sh help                      this text
#        testbox.sh warmup                    start a testbox; saves its ID to target/testbox-id
#        testbox.sh run <command...>          sync this worktree, then run the command with the testbox env
#                                             (e.g. run scripts/run-cargo-capped.sh build --release -p ts_goport --bins).
#                                             Arguments keep their boundaries; use run bash -c '...' for pipes and &&
#        testbox.sh job <command...>          as run, but detached from the SSH session (testbox-remote.sh start): a
#                                             dropped SSH session does not stop it, and it keeps the testbox alive. Its
#                                             output comes back as it grows; the exit code is the command's
#        testbox.sh get <remote> [local]      copy a file or dir back with rsync (remote path absolute or relative
#                                             to the repo root; a trailing / copies a dir's contents)
#        testbox.sh push <path>...            copy zbook files or dirs to the same absolute paths (e.g. a bins dir)
#        testbox.sh seed [inputs|pin]...      copy the files of testbox-data.txt (default both disks) to their
#                                             sticky disks (30 to 35 minutes; a rerun copies only what is
#                                             missing), then release: a stopped testbox does not commit them
#        testbox.sh status | list             the saved testbox's status, list active testboxes
#        testbox.sh stop                      end the saved testbox now. stop cancels the job, and the sticky disks
#                                             do not commit after a cancelled step: the writes to them are lost
#        testbox.sh release                   forget the saved testbox: it ends at its idle timeout (TESTBOX_IDLE
#                                             minutes after the last command), and that end requests a commit of the
#                                             sticky disks. Use it after a cold build or a seed. Commits of
#                                             testboxes that lived 2 to 13 minutes landed (up to 2.1 GiB); those of two
#                                             that lived 25 and 27 minutes did not. For a commit, warm up with
#                                             TESTBOX_IDLE=1 and release after the last command. `status` shows the end
#        testbox.sh pin [KEY...]              check out each pin's Go repo at its zbook path, with npm ci node_modules
#                                             (default: the batch pin, which warmup checks out). KEY comes from this
#                                             worktree's UPSTREAM.json
#        testbox.sh oracle [KEY]              push zbook's recorded oracle of the pin when the testbox's copy differs
#        testbox.sh source <checkout>         the checkout at its own zbook path on the testbox: a git worktree at its
#                                             HEAD (with a git bundle of the commits that origin lacks), then a copy of
#                                             each modified or untracked file. Stops unless fp.py gives the same
#                                             fingerprint on both sides. candidate.sh side --host testbox uses it
#        testbox.sh tests [--pin KEY] OUT BASE
#                                             the protected test suite at HEAD: build-goport-tests.sh and
#                                             goport-tests.sh on the testbox (pushes the pin oracle first when the
#                                             testbox lacks it), fetch results.json and the logs to OUT (must not
#                                             exist), print compare-tests.py BASE OUT. After a failed build, print its
#                                             compiler errors (OUT/testbin.new/logs).
#                                             HEAD must be pushed; crates/, Cargo.toml and Cargo.lock must be clean
#        testbox.sh np-suite [--pin KEY] [--bins DIR | --expect SHA256] LABEL [BASE]
#                                             np-suite.sh run LABEL on the testbox, fetched to the main checkout's
#                                             np-suite dir (LABEL must not exist), then np-suite.sh diff BASE LABEL.
#                                             --bins DIR: push DIR/tsgo (checked against DIR/bins.sha256 when there)
#                                             and run on it, no build; a meta.txt sha256 that differs is an error.
#                                             Use it for revision evidence (the candidate's release bins).
#                                             Without --bins: a stable release tsgo built on the testbox. Its bytes
#                                             depend on the host (jemalloc's configure reads the CPU's virtual address
#                                             bits), so --expect SHA256 (12 to 64 hex) warns when it differs
#        testbox.sh perf [--mode M] [--rounds N] LABEL BIN... [-- TSGO_ARG...]
#                                             perf.sh on the testbox (default --mode both, 3 rounds): pushes the
#                                             bins, fetches the result to the main checkout's perf dir (LABEL must
#                                             not exist). Without a saved testbox it warms one up (TESTBOX_IDLE 30)
#                                             and stops it at the end. Read instructions:u and RSS; wall is not valid.
#                                             The variables of `perf.sh env` pass on to perf.sh there
#        testbox.sh release-build OUT [NAME=VALUE...]
#                                             crates/ts_goport/scripts/build-release.sh at HEAD with
#                                             RELEASE_BOLT_PROFILE=instrument (the guest has no branch sampling) and the
#                                             NAME=VALUE variables (RELEASE_*, BOLT_*, PGO_*), fetch <out>/bin (with COMMIT
#                                             and bins.sha256) and the logs to OUT (must not exist). HEAD must be pushed;
#                                             crates/, Cargo.toml and Cargo.lock must be clean
# The gate and the oracles run as on zbook: the testbox env sets HOME=/home/theo and the workflow binds the
# workspace on /home/theo/Code/sandbox/ts-rust (testbox-data.sh). Fetch a commit that the bins name before a
# gate: run git fetch --depth=1 origin <sha>.
# KEY defaults to the batch pin (docs/typechecker-state/current.json batch.upstreamPin.to).
# TESTBOX_ID overrides the saved ID. TESTBOX_ID_FILE: the file of the saved ID (default target/testbox-id of
# this worktree). TESTBOX_IDLE: idle minutes before the testbox stops (default 15).
# TESTBOX_REF: the ref the workflow runs from (default main: the workflow must be on the default branch).
# Retries: after a transient failure (a DNS, connect or TLS error or a request timeout), these steps run again,
# up to 6 tries in all: warmup, status, stop and list (blacksmith API calls), the rsync of push, get, source and
# the fetch of a job's results, and the start of run and job. Each retry prints one line on stderr. A command
# that started on the testbox never runs again (job handles a dropped SSH session), and a nonzero exit of the
# command or of a build is not a transient failure. A warmup whose answer was lost can leave one extra testbox
# that ends at its idle timeout. TESTBOX_RETRY_WAITS: the seconds before each retry (default "5 10 20 30 30";
# "" turns retries off).
# Auth: `blacksmith auth login` once per host (`blacksmith auth status` shows it).
# scripts/goport/testbox-remote.sh holds the steps that run on the testbox.
set -euo pipefail
case "${1:-help}" in help | -h | --help) sed -n '2,/^set -euo/p' "$0" | sed '$d'; exit 0 ;; esac

repo_root="$(git -C "$(dirname -- "${BASH_SOURCE[0]}")" rev-parse --show-toplevel)"
id_file="${TESTBOX_ID_FILE:-$repo_root/target/testbox-id}"
# The synced repo root on the testbox, relative to the SSH login dir (/home/runner).
remote_root=_work/ts-rust/ts-rust
cd "$repo_root"

# Transient failures of the blacksmith API, ssh and rsync (2026-10-13: "request timed out after 30s", "net/http:
# TLS handshake timeout", "Could not resolve hostname ...: Name or service not known").
transient_re='request timed out|TLS handshake timeout|i/o timeout|Client\.Timeout exceeded|context deadline exceeded'
transient_re+='|connection reset by peer|connection refused|no such host|server misbehaving|unexpected EOF'
transient_re+='|temporary failure in name resolution|could not resolve hostname|name or service not known'
transient_re+='|network is unreachable|no route to host|connection timed out|connection closed by|closed by remote host'
transient_re+='|kex_exchange_identification|broken pipe|handshake failed: EOF|502 Bad Gateway|503 Service Unavailable'
transient_re+='|504 Gateway Timeout'
read -ra retry_waits <<<"${TESTBOX_RETRY_WAITS-5 10 20 30 30}"
[[ ${retry_waits[*]} =~ ^[0-9\ ]*$ ]] ||
  { echo "TESTBOX_RETRY_WAITS: whole seconds, for example '5 10 20 30 30'" >&2; exit 2; }
# remote prints this line on the testbox's stderr just before the command. retry takes it out of the output.
start_marker="testbox.sh:start:$$:$RANDOM$RANDOM"
# Retry notices go to fd 4, this script's stderr, also from inside $(... 2>&1).
exec 4>&2

# retry KIND CMD...: run CMD, and run it again after each wait of retry_waits while it fails for a transient
# reason. CMD's stderr passes through and is checked for transient_re. Returns CMD's last exit code. KIND:
#   api  a blacksmith API call (warmup, status, stop, list): any nonzero exit.
#   net  ssh or rsync: exit 255 (ssh) or rsync's 12, 30 or 35 (the stream closed or timed out). Use it only for a
#        step that is safe to repeat.
#   run  blacksmith testbox run: only before the start marker, so a command that started never runs again.
retry() {
  local kind=$1 try=1 rc errf what line
  shift
  what=${1##*/}
  [[ $what != blacksmith ]] || what+=" $2 $3"
  errf=$(mktemp)
  while :; do
    rc=0
    # A try that prints no error must not reuse the error lines of the try before it.
    : > "$errf"
    # stdout goes straight through on fd 3. awk copies stderr to errf up to the start marker, and to our stderr
    # without the marker.
    { "$@" 2>&1 1>&3 3>&- 4>&- |
        awk -v m="$start_marker" -v f="$errf" '!s {print > f; fflush(f)} $0 == m {s = 1; close(f); next} {print; fflush()}' >&2 3>&- 4>&- ||
        rc=${PIPESTATUS[0]}; } 3>&1
    ((rc != 0 && try <= ${#retry_waits[@]})) || break
    case $kind in
      net) ((rc == 255 || rc == 12 || rc == 30 || rc == 35)) || break ;;
      run) ! grep -qxF -- "$start_marker" "$errf" || break ;;
    esac
    line=$(grep -m1 -iE -- "$transient_re" "$errf" | sed 's/\x1b\[[0-9;]*[A-Za-z]//g; s/\r//g') || break
    echo "testbox.sh: $what failed (rc $rc: ${line:0:160}); try $((try + 1)) of $((${#retry_waits[@]} + 1)) in ${retry_waits[try - 1]} s" >&4
    sleep "${retry_waits[try - 1]}"
    try=$((try + 1))
  done
  rm -f "$errf"
  return "$rc"
}

saved_id() {
  local id="${TESTBOX_ID:-$(cat "$id_file" 2>/dev/null || true)}"
  [[ -n "$id" ]] || { echo "no testbox: run 'testbox.sh warmup' or set TESTBOX_ID" >&2; exit 2; }
  echo "$id"
}
batch_pin() { jq -r '.batch.upstreamPin.to // empty' docs/typechecker-state/current.json; }

# remote <command...>: sync this worktree, run the command in the testbox's repo root with the testbox env.
# A transient failure before the command starts (the CLI's status wait, DNS, the sync) runs it again.
remote() {
  local remote_cmd id
  id="$(saved_id)"
  printf -v remote_cmd '%q ' "$@"
  # A subshell: the CLI touches ~/.testbox-last-activity after the command in this shell, and testbox.env sets
  # HOME=/home/theo. Outside the subshell that touch would miss the marker, and the idle time would count from
  # the start of a long command.
  retry run blacksmith testbox run --id "$id" "(echo $start_marker >&2; . ~/testbox.env && $remote_cmd)"
}

# ssh_route: sets host and ssh for rsync over the CLI's own SSH route (the port in connection.json and the key that warmup
# saves in ~/.blacksmith/testboxes/<id>; CLI 0.4.70, see `run --debug`). The login dir is /home/runner.
# Commands over this route do not source the testbox env, and they read stdin: give them < /dev/null.
ssh_route() {
  local id key
  id="$(saved_id)"
  # --wait: a testbox that is not ready yet has no IP, and its row has the workflow path in that column.
  host="$(retry api blacksmith testbox status --id "$id" --wait --wait-timeout 15m | awk -v id="$id" '$1 == id && $2 == "ready" {print $3}')"
  key="$HOME/.blacksmith/testboxes/$id/id_ed25519"
  # A host with a / would make rsync copy to a local path.
  [[ -n "$host" && $host != */* && -f "$key" ]] || { echo "no SSH route to $id (host '$host', key $key)" >&2; exit 1; }
  # The CLI saves the testbox's SSH port in connection.json. It is not always 64000: on 2026-10-22 some
  # testboxes listened on 64001 and 64002, and the fixed port reached another testbox or nothing. The CLI
  # writes the file only at its first `run` on the testbox, so make one when it is not there yet.
  local port conn="$HOME/.blacksmith/testboxes/$id/connection.json"
  [[ -s $conn ]] || retry net blacksmith testbox run --id "$id" true < /dev/null > /dev/null 2>&1 || true
  port="$(python3 -c 'import json, sys; print(json.load(open(sys.argv[1])).get("ssh_port") or 64000)' \
    "$conn" 2> /dev/null)" || port=64000
  [[ $port =~ ^[0-9]+$ ]] || port=64000
  ssh="ssh -p $port -i $key -o IdentitiesOnly=yes -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR -o Compression=no -o ConnectTimeout=20"
}

# push_paths <path>...: rsync each path to the same path on the testbox.
push_paths() {
  local host ssh p keep
  ssh_route
  # The idle loop of run-testbox does not see this SSH route: touch its activity marker while we copy.
  # Its stdio goes to /dev/null and fd 4 (our stderr) is closed: kill stops the loop, not its `sleep 60`,
  # and a sleep that holds the caller's stdout or stderr keeps a pipe (testbox.sh push ... 2>&1 | tail)
  # open for up to 60 s after the copy.
  while :; do $ssh "runner@$host" touch .testbox-last-activity || true; sleep 60; done < /dev/null > /dev/null 2>&1 4>&- &
  keep=$!
  # shellcheck disable=SC2064 # expand now: keep is local, and the trap runs after the function returns
  trap "kill $keep 2>/dev/null || true" EXIT
  for p in "$@"; do
    # -s: a symlink keeps its own path on the testbox; rsync copies what it points to.
    p="$(realpath -s "$p")"
    echo "push $p"
    # A file of testbox-data.txt is a bind mount there: write it in place, not by rename.
    if [[ -d "$p" ]]; then retry net rsync -aH --mkpath --compress --compress-choice=zstd -e "$ssh" "$p/" "runner@$host:$p/"
    else retry net rsync -aHL --mkpath --inplace --compress --compress-choice=zstd -e "$ssh" "$p" "runner@$host:$p"; fi
  done
  # A caller can run on for minutes (tests): stop the keepalive now.
  kill "$keep" 2>/dev/null || true
  trap - EXIT
}

# push_oracle KEY: push zbook's recorded oracle of pin KEY when the testbox's copy differs. The batch pin's
# oracle path is a bind of the pin disk (testbox-data.txt), so the push lands there; another pin's stays on
# this testbox only. A build on the runner has another sha256 (Go puts absolute paths in the binary).
push_oracle() {
  local path sha host ssh
  path=$(python3 scripts/upstream/pin.py path oracle "$1") || exit 2
  sha=$(python3 scripts/upstream/pin.py show "$1" | jq -r .oracle.sha256)
  [[ $(sha256sum < "$path" | cut -d' ' -f1) == "$sha" ]] ||
    { echo "$path does not match the sha256 in UPSTREAM.json" >&2; exit 1; }
  ssh_route
  if [[ $($ssh "runner@$host" sha256sum "$path" < /dev/null 2> /dev/null | cut -d' ' -f1) == "$sha" ]]; then
    echo "oracle $1 on the testbox"
  else
    push_paths "$path"
  fi
}

# job RUN STEP ARGS...: start testbox-remote.sh STEP ARGS... detached on the testbox, print its log as it grows
# and return its exit code. It polls over the SSH route, which does not sync this worktree.
job() {
  local run=$1 host ssh t size=0 fails=0 rc
  # exit, not return: callers run job in a || list (no set -e there), and then fetch RUN, which must not
  # take another job's run.
  # Exit 255 is the SSH session of the CLI, not the step (a side run on 2026-10-11 lost its testbox run this way
  # with the testbox up): start once more, unless the first start made RUN.log.
  remote scripts/goport/testbox-remote.sh start "$@" || {
    rc=$?
    ((rc == 255)) || exit "$rc"
    ssh_route
    sleep 5
    if $ssh "runner@$host" test -e "$remote_root/$run.log" < /dev/null; then
      echo "testbox $(saved_id): $run started (SSH rc 255)" >&2
    else
      echo "testbox $(saved_id): SSH rc 255 at the start of $run; start again" >&2
      remote scripts/goport/testbox-remote.sh start "$@" || exit
    fi
  }
  ssh_route
  t=$(mktemp -d)
  while :; do
    # RUN.rc first: once it is there, the log is complete.
    rc=$($ssh "runner@$host" cat "$remote_root/$run.rc" < /dev/null 2> /dev/null) || rc=
    if $ssh "runner@$host" tail -c "+$((size + 1))" "$remote_root/$run.log" < /dev/null > "$t/new" 2> /dev/null; then
      fails=0
      cat "$t/new"
      size=$((size + $(stat -c %s "$t/new")))
    elif ((++fails >= 10)); then
      echo "testbox $(saved_id): no $run.log after 10 tries" >&2
      rm -rf "$t"
      return 1
    fi
    [[ -z $rc ]] || break
    sleep 15
  done
  rm -rf "$t"
  return "$rc"
}

# fetch RUN DEST: copy RUN/out.tar.gz and RUN.log back, unpack them in DEST, then remove them on the testbox
# (the test binaries would fill the sticky target/). Without RUN/out.tar.gz (the job ended before it packed),
# it keeps RUN.log on the testbox and returns 1.
fetch() {
  local host ssh t
  ssh_route
  t=$(mktemp -d)
  retry net rsync -a -e "$ssh" "runner@$host:$remote_root/$1/out.tar.gz" "$t/" || {
    rm -rf "$t"
    echo "no $1/out.tar.gz on the testbox (the job log above has its error; $1.log stays there)" >&2
    return 1
  }
  retry net rsync -a -e "$ssh" "runner@$host:$remote_root/$1.log" "$t/testbox.log"
  mkdir -p "$2"
  tar -xzf "$t/out.tar.gz" -C "$2"
  mv "$t/testbox.log" "$2/"
  rm -rf "$t"
  # shellcheck disable=SC2086 # $ssh is the ssh command and its options
  retry net $ssh "runner@$host" rm -rf "$remote_root/$1" "$remote_root/$1.log" "$remote_root/$1.rc" < /dev/null
}

cmd="$1"
shift
pin_key=
if [[ $cmd == tests || $cmd == np-suite ]] && [[ ${1:-} == --pin ]]; then
  (($# >= 2)) || { echo "usage: testbox.sh $cmd --pin KEY ..." >&2; exit 2; }
  pin_key=$2
  shift 2
fi
pin_key=${pin_key:-$(batch_pin)}
case "$cmd" in
  warmup)
    rc=0
    out="$(retry api blacksmith testbox warmup testbox.yml --ref "${TESTBOX_REF:-main}" --idle-timeout "${TESTBOX_IDLE:-15}" 2>&1)" || rc=$?
    echo "$out"
    ((rc == 0)) || exit "$rc"
    # The last ID: $out also holds the errors of failed tries, and those can name a testbox.
    id="$(grep -oE 'tbx_[A-Za-z0-9_-]+' <<<"$out" | tail -1)"
    [[ -n "$id" ]] || { echo "warmup printed no testbox ID" >&2; exit 1; }
    mkdir -p "$(dirname "$id_file")"
    echo "$id" >"$id_file"
    echo "testbox $id (saved to $id_file)"
    ;;
  run)
    (($#)) || { echo "usage: testbox.sh run <command...>" >&2; exit 2; }
    remote "$@"
    ;;
  pin) remote scripts/goport/testbox-remote.sh pin "$@" ;;
  oracle)
    (($# <= 1)) || { echo "usage: testbox.sh oracle [KEY]" >&2; exit 2; }
    push_oracle "${1:-$pin_key}"
    ;;
  job)
    (($#)) || { echo "usage: testbox.sh job <command...>" >&2; exit 2; }
    run=target/testbox-runs/job-$(date -u +%Y%m%dT%H%M%S)-$$
    rc=0
    job "$run" exec "$@" || rc=$?
    # Quiet: a caller can judge the last output line (for example DONE).
    ssh_route > /dev/null 2>&1 4>/dev/null
    $ssh "runner@$host" rm -rf "$remote_root/$run.log" "$remote_root/$run.rc" < /dev/null > /dev/null 2>&1 || true
    exit "$rc"
    ;;
  source)
    (($# == 1)) || { echo "usage: testbox.sh source <checkout>" >&2; exit 2; }
    co=$(realpath -- "$1")
    [[ $co == /home/theo/Code/sandbox/ts-rust/target/* ]] ||
      { echo "$co is not under /home/theo/Code/sandbox/ts-rust/target (the testbox makes it there)" >&2; exit 2; }
    head=$(git -C "$co" rev-parse HEAD)
    t=$(mktemp -d /tmp/testbox-source.XXXXXX)
    # shellcheck disable=SC2064 # expand now
    trap "rm -rf '$t'" EXIT
    bundle=
    # The commits of HEAD that no origin branch has (as zbook last saw origin) go in a bundle; the testbox
    # fetches the bundle's prerequisites from origin.
    if [[ -n $(git -C "$co" rev-list -n 1 HEAD --not --remotes=origin) ]]; then
      bundle=$t/source.bundle
      git -C "$co" bundle create "$bundle" HEAD --not --remotes=origin
      push_paths "$bundle"
      # push_paths clears the EXIT trap.
      # shellcheck disable=SC2064
      trap "rm -rf '$t'" EXIT
    fi
    remote scripts/goport/testbox-remote.sh source "$co" "$head" $bundle
    [[ -z $bundle ]] || { ssh_route; $ssh "runner@$host" rm -f "$bundle" < /dev/null || true; }
    # Each file that differs from HEAD or is untracked (not ignored), with a new mtime (no -t: cargo judges by
    # mtime), and each deleted file.
    git -C "$co" diff -z --name-only --no-renames --diff-filter=D HEAD > "$t/deleted"
    { git -C "$co" diff -z --name-only --no-renames --diff-filter=d HEAD; git -C "$co" ls-files -z -o --exclude-standard; } > "$t/files"
    ssh_route
    if [[ -s $t/files ]]; then
      echo "copy $(tr -cd '\0' < "$t/files" | wc -c) modified or untracked files"
      retry net rsync -rlpH --from0 --files-from="$t/files" -e "$ssh" "$co/" "runner@$host:$co/"
    fi
    if [[ -s $t/deleted ]]; then
      # NUL-separated to the end: a file name can hold a newline.
      while IFS= read -r -d '' f; do printf '%s\0' "$co/$f"; done < "$t/deleted" > "$t/deleted.list"
      $ssh "runner@$host" "xargs -0 rm -f" < "$t/deleted.list"
    fi
    here_fp=$(python3 scripts/goport/fp.py "$co" | cut -d' ' -f1)
    # shellcheck disable=SC2086 # $ssh is the ssh command and its options
    there_fp=$(retry net $ssh "runner@$host" "cd $remote_root && python3 scripts/goport/fp.py $(printf %q "$co")" < /dev/null | cut -d' ' -f1)
    [[ $here_fp == "$there_fp" ]] || { echo "fp.py $co: zbook $here_fp, testbox $there_fp" >&2; exit 1; }
    echo "source $co ${head:0:12} on the testbox, fp ${here_fp:0:12}"
    ;;
  tests)
    (($# == 2)) || { echo "usage: testbox.sh tests [--pin KEY] OUT BASE" >&2; exit 2; }
    out=$(realpath -m -- "$1") base=$(realpath -- "$2")
    [[ ! -e $out ]] || { echo "$out exists (results are never replaced)" >&2; exit 2; }
    [[ -z $(git status --porcelain --untracked-files=all -- crates Cargo.toml Cargo.lock) ]] ||
      { echo "crates/, Cargo.toml or Cargo.lock has changes: commit them (build-goport-tests.sh builds HEAD)" >&2; exit 2; }
    head=$(git rev-parse HEAD)
    git branch -r --contains "$head" | grep -q . ||
      echo "warning: no remote branch here has HEAD ${head:0:12}; the testbox fetches it from origin" >&2
    push_oracle "$pin_key"
    run=target/testbox-runs/tests-$(date -u +%Y%m%dT%H%M%S)-${head:0:9}
    rc=0
    job "$run" tests "$pin_key" "$head" "$run" || rc=$?
    fetch "$run" "$out"
    if [[ ! -f $out/tests-run/results.json ]]; then
      # A failed build: its compiler errors, from the cargo logs (without the colors).
      sed 's/\x1b\[[0-9;]*m//g' "$out"/testbin.new/logs/build-*.log 2> /dev/null | grep -A 12 -m 10 '^error' | head -80 || true
      echo "no results.json (rc $rc): see $out/build.log, $out/testbin.new/logs and $out/tests.log" >&2
      exit 1
    fi
    python3 scripts/goport/compare-tests.py "$base" "$out/tests-run/results.json" --out "$out/compare.json" || rc=$?
    echo "results $out/tests-run/results.json, compare $out/compare.json"
    exit "$rc"
    ;;
  np-suite)
    np_usage="usage: testbox.sh np-suite [--pin KEY] [--bins DIR | --expect SHA256] LABEL [BASE]"
    bin='' want='' expect=''
    if [[ ${1:-} == --bins ]]; then
      (($# >= 2)) || { echo "$np_usage" >&2; exit 2; }
      bin=$(realpath -s -- "$2/tsgo")
      shift 2
      [[ -f $bin && -x $bin ]] || { echo "not an executable file: $bin" >&2; exit 2; }
      want=$(sha256sum < "$bin" | cut -d' ' -f1)
      # The bins dir of a side run or a lane: its tsgo line must name these bytes.
      [[ ! -f ${bin%/tsgo}/bins.sha256 ]] || grep -qxE "$want  (\./)?tsgo" "${bin%/tsgo}/bins.sha256" ||
        { echo "$bin: sha256 $want is not the tsgo line of ${bin%/tsgo}/bins.sha256" >&2; exit 2; }
    elif [[ ${1:-} == --expect ]]; then
      (($# >= 2)) && [[ $2 =~ ^[0-9a-f]{12,64}$ ]] || { echo "$np_usage" >&2; exit 2; }
      expect=$2
      shift 2
    fi
    (($# == 1 || $# == 2)) || { echo "$np_usage" >&2; exit 2; }
    # One dir name: the result moves to $np/LABEL, and np-suite.sh diff takes labels.
    [[ -n $1 && $1 != */* ]] || { echo "LABEL must be a dir name without /: $1" >&2; exit 2; }
    np=/home/theo/Code/sandbox/ts-rust/target/continuation-r97-goport/compat-backlog/np-suite
    [[ ! -e $np/$1 ]] || { echo "$np/$1 exists (results are never replaced)" >&2; exit 2; }
    # The same absolute path there, so meta.txt names the zbook file.
    [[ -z $bin ]] || push_paths "$bin"
    run=target/testbox-runs/np-$(date -u +%Y%m%dT%H%M%S)-$1
    rc=0
    job "$run" np-suite "$pin_key" "$1" "$run" ${bin:+"$bin"} || rc=$?
    mkdir -p "$np" target
    tmp=$(mktemp -d "$repo_root/target/testbox-np.XXXXXX")
    fetch "$run" "$tmp"
    [[ -d $tmp/$1 ]] || { echo "no np-suite result (rc $rc): the tsgo build log is $tmp/build.log" >&2; exit 1; }
    mv "$tmp/$1" "$np/$1"
    mv "$tmp/build.log" "$np/$1/testbox-build.log"
    mv "$tmp/testbox.log" "$np/$1/testbox.log"
    rmdir "$tmp"
    cat "$np/$1/summary.txt"
    [[ -z ${2:-} ]] || scripts/goport/np-suite.sh diff "$2" "$1" | tail -20
    # Last, so it is not lost above the diff: which tsgo the run used (np-suite.sh writes its sha256 in meta.txt).
    got=$(awk '$1 == "sha256" {print $2}' "$np/$1/meta.txt")
    if [[ -n $bin && $got != "$want" ]]; then
      echo "error: np-suite $1 ran a tsgo with sha256 '$got', not $bin ($want)" >&2
      rc=1
    elif [[ -n $expect && $got != "$expect"* ]]; then
      echo "warning: the tsgo built on the testbox has sha256 $got, not $expect: $1 is not a run of those bins (use --bins)" >&2
    else
      echo "np-suite $1: tsgo sha256 $got${bin:+ ($bin)}"
    fi
    exit "$rc"
    ;;
  perf)
    args=("$@")
    opts=(--mode both)
    while [[ ${1:-} == --mode || ${1:-} == --rounds ]]; do
      (($# >= 2)) || { echo "usage: testbox.sh perf [--mode M] [--rounds N] LABEL BIN... [-- TSGO_ARG...]" >&2; exit 2; }
      opts+=("$1" "$2")
      shift 2
    done
    (($# >= 2)) || { echo "usage: testbox.sh perf [--mode M] [--rounds N] LABEL BIN... [-- TSGO_ARG...]" >&2; exit 2; }
    label=$1
    shift
    [[ -n $label && $label != */* && $label != .* ]] || { echo "LABEL must be a dir name: $label" >&2; exit 2; }
    pd=/home/theo/Code/sandbox/ts-rust/target/continuation-r97-goport/perf
    [[ ! -e $pd/$label ]] || { echo "$pd/$label exists (results are never replaced)" >&2; exit 2; }
    bins=() targs=()
    while (($#)); do
      # The rest, with the --, goes to perf.sh: tsgo arguments such as --noEmit.
      [[ $1 != -- ]] || { targs=("$@"); break; }
      [[ -f $1 && -x $1 ]] || { echo "not an executable file: $1" >&2; exit 2; }
      bins+=("$(realpath -s -- "$1")")
      shift
    done
    if [[ -z ${TESTBOX_ID:-} && ! -s $id_file ]]; then
      # A new process: push_paths replaces the EXIT trap, and set -e is off on the left of ||.
      TESTBOX_IDLE=${TESTBOX_IDLE:-30} "$0" warmup
      rc=0
      "$0" perf "${args[@]}" || rc=$?
      "$0" stop
      exit "$rc"
    fi
    # The same absolute paths there, so bins.txt names the zbook files.
    push_paths "${bins[@]}"
    run=target/testbox-runs/perf-$(date -u +%Y%m%dT%H%M%S)-$label
    # The caller's GOPORT_* and allocator variables: perf.sh there passes them on to tsgo.
    mapfile -t envs < <(scripts/goport/perf.sh env)
    rc=0
    job "$run" perf "$run" "$label" "${envs[@]}" "${opts[@]}" "$label" "${bins[@]}" "${targs[@]}" || rc=$?
    mkdir -p "$pd" target
    tmp=$(mktemp -d "$repo_root/target/testbox-perf.XXXXXX")
    fetch "$run" "$tmp" || exit 1
    [[ -d $tmp/$label ]] || { echo "no perf.sh result (rc $rc): $tmp/testbox.log" >&2; exit 1; }
    mv "$tmp/$label" "$pd/$label"
    mv "$tmp/testbox.log" "$pd/$label/"
    rmdir "$tmp"
    echo "results $pd/$label (perf.sh compare <base> $label compares with another run)"
    exit "$rc"
    ;;
  release-build)
    (($# >= 1)) || { echo "usage: testbox.sh release-build OUT [NAME=VALUE...]" >&2; exit 2; }
    out=$(realpath -m -- "$1")
    shift
    [[ ! -e $out ]] || { echo "$out exists (results are never replaced)" >&2; exit 2; }
    for a in "$@"; do [[ $a =~ ^[A-Za-z_][A-Za-z0-9_]*= ]] || { echo "not NAME=VALUE: $a" >&2; exit 2; }; done
    [[ -z $(git status --porcelain --untracked-files=all -- crates Cargo.toml Cargo.lock) ]] ||
      { echo "crates/, Cargo.toml or Cargo.lock has changes: commit them (the build is of HEAD)" >&2; exit 2; }
    head=$(git rev-parse HEAD)
    git branch -r --contains "$head" | grep -q . ||
      echo "warning: no remote branch here has HEAD ${head:0:12}; the testbox fetches it from origin" >&2
    run=target/testbox-runs/release-$(date -u +%Y%m%dT%H%M%S)-${head:0:9}
    rc=0
    job "$run" release-build "$head" "$run" "$@" || rc=$?
    fetch "$run" "$out" || exit 1
    [[ -f $out/bin/BUILD.txt ]] || { echo "no release bins (rc $rc): see $out/build.log" >&2; exit 1; }
    (cd "$out/bin" && echo "$head" > COMMIT && find . -maxdepth 1 -type f -perm -u+x -printf '%f\n' | sort | xargs sha256sum > bins.sha256)
    echo "release bins $out/bin (BUILD.txt, COMMIT, bins.sha256)"
    exit "$rc"
    ;;
  get)
    # rsync, not `blacksmith testbox download`: that timed out after 5 minutes on a 124 MB results dir.
    (($# == 1 || $# == 2)) || { echo "usage: testbox.sh get <remote> [local]" >&2; exit 2; }
    ssh_route
    src="$1"
    [[ "$src" == /* ]] || src="$remote_root/$src"
    retry net rsync -aH --compress --compress-choice=zstd -e "$ssh" "runner@$host:$src" "${2:-.}"
    ;;
  push)
    (($#)) || { echo "usage: testbox.sh push <path>..." >&2; exit 2; }
    push_paths "$@"
    ;;
  seed)
    # Read the list first: a failure inside < <(...) would not stop the script.
    manifest="$(scripts/goport/testbox-data.sh paths)"
    mapfile -t paths < <(awk -v want=" ${*:-inputs pin} " 'index(want, " " $1 " ") {print $2}' <<<"$manifest")
    ((${#paths[@]})) || { echo "no seed paths for: ${*:-inputs pin}" >&2; exit 2; }
    push_paths "${paths[@]}"
    ;;
  status) retry api blacksmith testbox status --id "$(saved_id)" "$@" ;;
  stop | release)
    id="$(saved_id)"
    if [[ $cmd == stop ]]; then
      retry api blacksmith testbox stop --id "$id"
    else
      echo "testbox $id: released; it ends at its idle timeout and requests a commit of its sticky disks"
      echo "(TESTBOX_ID=$id scripts/goport/testbox.sh status shows when it completed)"
    fi
    # Keep the saved ID when TESTBOX_ID named another testbox.
    [[ "$(cat "$id_file" 2>/dev/null || true)" != "$id" ]] || rm -f "$id_file"
    ;;
  list) retry api blacksmith testbox list "$@" ;;
  *) echo "unknown command: $cmd (testbox.sh help)" >&2; exit 2 ;;
esac
