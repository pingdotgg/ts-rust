#!/usr/bin/env bash
# The testbox side of testbox.sh. Each command runs on a Blacksmith testbox in the synced repo root, through
# `blacksmith testbox run`. The workflow has set up zbook's paths (testbox-data.sh mount): the workspace is
# bound on /home/theo/Code/sandbox/ts-rust, the batch pin's Go checkout (with node_modules) on its zbook path,
# the pin oracle and the other files of testbox-data.txt on theirs, and the testbox env sets HOME=/home/theo.
# usage: testbox-remote.sh pin [KEY...]            the Go checkout of each pin at its zbook path, at the pin commit,
#                                                  with npm ci node_modules (default: the batch pin, which the
#                                                  workflow checks out)
#        testbox-remote.sh start RUN STEP ARGS...  run `testbox-remote.sh STEP ARGS...` detached from the SSH session:
#                                                  output in RUN.log, then its exit code in RUN.rc. The job counts as
#                                                  testbox activity (the idle timer does not end the testbox under it)
#        testbox-remote.sh tests KEY COMMIT RUN    build-goport-tests.sh at COMMIT, goport-tests.sh at pin KEY,
#                                                  pack the results and logs in RUN/out.tar.gz (after a failed build,
#                                                  its cargo logs in testbin.new/logs)
#        testbox-remote.sh np-suite KEY LABEL RUN [BIN]
#                                                  np-suite.sh run LABEL at pin KEY on BIN (no build), else on a stable
#                                                  release tsgo built here (build.log starts with its tree, rustc and
#                                                  CPU), pack the label dir and build.log in RUN/out.tar.gz, then remove
#                                                  the label dir (after a failed build, only its build.log)
#        testbox-remote.sh exec CMD...             run CMD (the step of testbox.sh job)
#        testbox-remote.sh source DIR COMMIT [BUNDLE]
#                                                  DIR (a zbook path under target/) as a git worktree of this repo
#                                                  at COMMIT, fetched from BUNDLE (its prerequisites from origin)
#                                                  or from origin. A DIR that is not that worktree is replaced
#        testbox-remote.sh perf RUN LABEL [NAME=VALUE...] ARGS...
#                                                  perf.sh ARGS... (ARGS hold LABEL) with the NAME=VALUE variables,
#                                                  pack the label dir in RUN/out.tar.gz, then remove the label dir
#        testbox-remote.sh release-build COMMIT RUN [NAME=VALUE...]
#                                                  build-release.sh at COMMIT with RELEASE_BOLT_PROFILE=instrument
#                                                  and the NAME=VALUE variables (the guest has no branch sampling),
#                                                  pack <out>/bin and the logs in RUN/out.tar.gz, then remove <out>
# pin.py exec binds a pin's oracle and Go checkout over the default ones; bwrap makes those bind targets.
# Only microsoft/TypeScript pins (layout typescript) are set up: the older layout needs the TypeScript submodule.
set -euo pipefail
case "${1:-help}" in help | -h | --help) sed -n '2,/^set -euo/p' "$0" | sed '$d'; exit 0 ;; esac

# zbook's main checkout: the root of the absolute paths in UPSTREAM.json (pinRoot) and np-suite.sh.
ZBOOK_REPO=/home/theo/Code/sandbox/ts-rust
die() { echo "testbox-remote.sh: $*" >&2; exit 1; }
cd "$(git -C "$(dirname -- "${BASH_SOURCE[0]}")" rev-parse --show-toplevel)"
pin() { python3 scripts/upstream/pin.py "$@"; }
batch_pin() { jq -r '.batch.upstreamPin.to // empty' docs/typechecker-state/current.json; }

# on_testbox: the workspace must be bound on $ZBOOK_REPO (testbox-data.sh mount). On zbook $ZBOOK_REPO is the
# main checkout itself, and `tests` must not move its HEAD (at_commit).
on_testbox() {
  [[ $PWD != "$ZBOOK_REPO" && $ZBOOK_REPO -ef $PWD ]] ||
    die "not on a testbox: $ZBOOK_REPO is not a bind of $PWD (testbox-data.sh mount)"
}

# checkout KEY: the whole pin checkout at its zbook path, at the pin commit, with the root node_modules
# (astnav tests and np-suite run node on them).
checkout() {
  local rec repo commit go dir
  rec=$(pin show "$1") || die "unknown pin $1"
  [[ $(jq -r .layout <<< "$rec") == typescript ]] || die "pin $1: layout $(jq -r .layout <<< "$rec"), not typescript"
  repo=$(jq -r .repo <<< "$rec") commit=$(jq -r .commit <<< "$rec") go=$(jq -r .goCheckout <<< "$rec")
  dir=${go%/"$(jq -r .subdir <<< "$rec")"}
  [[ $dir == /home/theo/.explore/repos/?* ]] || die "pin $1: checkout $dir is not under /home/theo/.explore/repos"
  if [[ $(git -C "$dir" rev-parse HEAD 2> /dev/null) == "$commit" && -f $dir/node_modules/typescript/package.json ]]; then
    echo "pin $1: $dir ready"
    return
  fi
  # The workflow binds its checkout of the batch pin here: never empty it.
  ! mountpoint -q "$dir" || die "pin $1: the workflow's checkout at $dir is not at ${commit:0:12} with node_modules"
  rm -rf "$dir"
  git init -q "$dir"
  git -C "$dir" fetch -q --depth=1 "https://github.com/$repo" "$commit"
  git -C "$dir" -c advice.detachedHead=false checkout -q FETCH_HEAD
  (cd "$dir" && npm ci --ignore-scripts --no-audit --no-fund --loglevel=error)
  [[ -d $go/testdata/baselines/reference ]] || die "pin $1: no $go/testdata/baselines/reference"
  echo "pin $1: $dir at ${commit:0:12}, $(ls "$dir/node_modules" | wc -l) node_modules"
}

# at_commit COMMIT: HEAD and the index at COMMIT, the synced files kept (the sync sends files, not git state).
at_commit() {
  [[ $(git rev-parse HEAD) != "$1" ]] || return 0
  git cat-file -e "$1^{commit}" 2> /dev/null || git fetch -q --depth=1 origin "$1" ||
    die "commit $1 is not on origin: push it first"
  git reset -q "$1"
}

# release_tools: what build-release.sh needs and the runner image lacks: its rustc with llvm-tools (the
# llvm-profdata of rustc's LLVM), llvm-bolt and merge-fdata of zbook's LLVM major (apt.llvm.org), and qemu-user
# for its CPU check. Prints the PATH entry of the LLVM tools.
release_tools() {
  local llvm=23 tc
  tc=$(sed -n 's/^export RUSTUP_TOOLCHAIN="${RUSTUP_TOOLCHAIN:-\([^}]*\)}"$/\1/p' crates/ts_goport/scripts/build-release.sh)
  [[ -n $tc ]] || die "no default RUSTUP_TOOLCHAIN in build-release.sh"
  rustup toolchain install "$tc" --profile minimal -c llvm-tools > /dev/null 2>&1 || die "rustup toolchain install $tc failed"
  if [[ ! -x /usr/lib/llvm-$llvm/bin/llvm-bolt || ! -x /usr/bin/qemu-x86_64 ]]; then
    curl -fsSL https://apt.llvm.org/llvm-snapshot.gpg.key | sudo tee /etc/apt/trusted.gpg.d/apt.llvm.org.asc > /dev/null
    echo "deb http://apt.llvm.org/noble/ llvm-toolchain-noble-$llvm main" | sudo tee /etc/apt/sources.list.d/llvm-$llvm.list > /dev/null
    sudo apt-get update -qq > /dev/null
    sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq "bolt-$llvm" "llvm-$llvm" qemu-user > /dev/null ||
      die "apt-get install bolt-$llvm llvm-$llvm qemu-user failed"
  fi
  echo "/usr/lib/llvm-$llvm/bin"
}

cmd=$1
shift
case $cmd in
  start)
    (($# >= 2)) || die "usage: start RUN STEP ARGS..."
    [[ ! -e $1.log ]] || die "$1.log exists"
    mkdir -p "$(dirname "$1")"
    # A dropped SSH session must not stop a long job (one dropped after 170 s on 2026-10-10).
    setsid nohup "$0" job "$@" > /dev/null 2>&1 < /dev/null &
    echo "started $1"
    ;;
  job)
    run=$1
    shift
    # run-testbox resets its idle timer when this file is newer than the last activity. It is in the login
    # home (/home/runner), not in ~: the testbox env sets HOME=/home/theo. The keepalive ends with this
    # shell, so a killed job lets the testbox end at its idle timeout and the local poll stop.
    marker=$(getent passwd "$(id -u)" | cut -d: -f6)/.testbox-last-activity
    (while sleep 30 && kill -0 "$$" 2> /dev/null; do touch "$marker"; done) &
    keep=$!
    # RUN.rc is written on every exit. It stays 1 when the job shell itself is killed.
    rc=1
    trap 'kill "$keep" 2> /dev/null || true; echo "$rc" > "$run.rc"' EXIT
    if "$0" "$@" > "$run.log" 2>&1 < /dev/null; then rc=0; else rc=$?; fi
    ;;
  pin)
    on_testbox
    (($#)) || { k=$(batch_pin); [[ -z $k ]] || set -- "$k"; }
    (($#)) || echo "no batch pin in docs/typechecker-state/current.json: no pin checkout (testbox.sh pin KEY sets one up)"
    for k in "$@"; do checkout "$k"; done
    ;;
  tests)
    (($# == 3)) || die "usage: tests KEY COMMIT RUN"
    key=$1 commit=$2 run=$3
    [[ ! -e $run ]] || die "$run exists"
    # First: it refuses to run on zbook before at_commit moves HEAD.
    on_testbox
    at_commit "$commit"
    checkout "$key"
    mkdir -p "$run"
    rc=0
    echo "build-goport-tests.sh at ${commit:0:12} ($(date -u +%T))"
    scripts/goport/build-goport-tests.sh "$PWD" "$run/testbin" > "$run/build.log" 2>&1 || rc=$?
    # Not the bare DONE line: a driver log that ends in DONE or FAIL must not match it early.
    tail -2 "$run/build.log" | grep -vx DONE || true
    if ((rc == 0)); then
      echo "goport-tests.sh at pin $key ($(date -u +%T))"
      GOPORT_PIN=$key scripts/goport/goport-tests.sh "$run/testbin" "$run/tests-run" > "$run/tests.log" 2>&1 || rc=$?
      grep -E '^[a-z_0-9-]+: [0-9]+ names|^incomplete:' "$run/tests.log" || true
      tail -1 "$run/tests.log"
    fi
    # Not the test binaries, the release bins, the crates archive (tests-run/src) or the runner temp. A failed
    # build leaves its cargo logs (the compiler errors) in testbin.new/logs.
    (cd "$run" && for f in build.log tests.log testbin/* testbin.new/logs tests-run/*; do
      case $f in testbin/relbin | tests-run/src | tests-run/tmp) continue ;; esac
      if [[ -d $f || (-f $f && ! -x $f) ]]; then echo "$f"; fi
    done) > "$run/pack.list"
    tar -C "$run" -czf "$run/out.tar.gz" -T "$run/pack.list"
    echo "packed $run/out.tar.gz ($(du -h "$run/out.tar.gz" | cut -f1)); rc $rc ($(date -u +%T))"
    exit "$rc"
    ;;
  np-suite)
    (($# == 3 || $# == 4)) || die "usage: np-suite KEY LABEL RUN [BIN]"
    key=$1 label=$2 run=$3 bin=${4:-}
    [[ ! -e $run ]] || die "$run exists"
    on_testbox
    checkout "$key"
    mkdir -p "$run"
    if [[ -n $bin ]]; then
      [[ -f $bin && -x $bin ]] || die "not an executable file: $bin"
      echo "no build: $bin, sha256 $(sha256sum < "$bin" | cut -d' ' -f1)" > "$run/build.log"
    else
      # What the bytes depend on. jemalloc's configure takes LG_VADDR from the CPU's virtual address bits (cpuid
      # 0x80000008), so a 57-bit host builds another tsgo from the same tree (npbuild1: c428ad2d2 gives
      # 6a4e7a3a7784 with LG_VADDR 57 and 54433ccd1aa8 with 48).
      {
        echo "tree $(git rev-parse HEAD), $(git status --porcelain --untracked-files=no | wc -l) modified tracked files"
        echo "$(rustc -V); cpu family $(awk '/^cpu family/ {print $4; exit}' /proc/cpuinfo) model $(awk '/^model\t/ {print $3; exit}' /proc/cpuinfo),$(grep -m1 'address sizes' /proc/cpuinfo | cut -d: -f2)"
      } > "$run/build.log" 2>&1
      # Stable and not incremental, as candidate.sh builds release bins.
      TS_CARGO_NIGHTLY=0 TS_CARGO_INCREMENTAL=0 scripts/run-cargo-capped.sh build --release --locked -p ts_goport --bin tsgo \
        >> "$run/build.log" 2>&1 || {
        tail -20 "$run/build.log"
        tar -czf "$run/out.tar.gz" -C "$run" build.log
        die "tsgo build failed"
      }
      bin=$PWD/target/release/tsgo
    fi
    np=target/continuation-r97-goport/compat-backlog/np-suite
    rc=0
    NP_GO_DIR=$(pin path goCheckout "$key") scripts/goport/np-suite.sh run "$label" "$bin" || rc=$?
    tar -czf "$run/out.tar.gz" -C "$run" build.log -C "$PWD/$np" "$label"
    # The local copy is the record: the sticky target/ keeps no label dirs.
    rm -rf "${np:?}/${label:?}"
    exit "$rc"
    ;;
  exec)
    on_testbox
    (($#)) || die "usage: exec CMD..."
    exec "$@"
    ;;
  source)
    (($# == 2 || $# == 3)) || die "usage: source DIR COMMIT [BUNDLE]"
    dir=$1 commit=$2 bundle=${3:-}
    # First: DIR is a zbook path, and this step replaces it.
    on_testbox
    [[ $dir == "$ZBOOK_REPO"/target/?* && $dir != *..* ]] || die "source: $dir is not under $ZBOOK_REPO/target"
    if [[ -n $bundle ]]; then
      # The prerequisites: the "-<sha>" lines of the bundle header, which ends at the first empty line.
      for p in $(awk '/^$/ {exit} /^-/ {print substr($1, 2)}' "$bundle"); do
        git cat-file -e "$p^{commit}" 2> /dev/null || git fetch -q --depth=1 origin "$p" || die "prerequisite $p is not on origin"
      done
      git fetch -q "$bundle" HEAD || die "cannot fetch $bundle"
    fi
    git cat-file -e "$commit^{commit}" 2> /dev/null || git fetch -q --depth=1 origin "$commit" ||
      die "commit $commit is not on origin and not in a bundle"
    if [[ $(git -C "$dir" rev-parse HEAD 2> /dev/null) == "$commit" && $(git -C "$dir" rev-parse --git-common-dir 2> /dev/null) -ef .git ]]; then
      git -C "$dir" reset -q --hard
      git -C "$dir" clean -q -fdx -e target/
    else
      # A dir from an older testbox (target/ is a sticky disk), or another commit: a new worktree.
      rm -rf "$dir"
      git worktree prune
      git worktree add -q --detach "$dir" "$commit"
    fi
    echo "$dir at ${commit:0:12}"
    ;;
  perf)
    (($# >= 3)) || die "usage: perf RUN LABEL [NAME=VALUE...] ARGS..."
    run=$1 label=$2 envs=()
    shift 2
    while [[ ${1:-} =~ ^[A-Za-z_][A-Za-z0-9_]*= ]]; do
      envs+=("$1")
      shift
    done
    [[ ! -e $run ]] || die "$run exists"
    on_testbox
    mkdir -p "$run"
    pd=target/continuation-r97-goport/perf
    rc=0
    # PERF_WAIT: the sync and the job start can leave the load over 1.5 for a while.
    PERF_WAIT=1 env "${envs[@]}" scripts/goport/perf.sh "$@" || rc=$?
    [[ -d $pd/$label ]] || die "perf.sh wrote no $pd/$label (rc $rc)"
    tar -czf "$run/out.tar.gz" -C "$pd" "$label"
    # The local copy is the record: the sticky target/ keeps no label dirs.
    rm -rf "${pd:?}/${label:?}"
    exit "$rc"
    ;;
  release-build)
    (($# >= 2)) || die "usage: release-build COMMIT RUN [NAME=VALUE...]"
    commit=$1 run=$2
    shift 2
    [[ ! -e $run ]] || die "$run exists"
    on_testbox
    at_commit "$commit"
    mkdir -p "$run"
    llvm_bin=$(release_tools)
    cargo fetch --locked -q || die "cargo fetch failed"
    # Outside the sticky target/: each build makes two fat LTO target dirs.
    out=$(getent passwd "$(id -u)" | cut -d: -f6)/release-build/${run##*/}
    rc=0
    echo "build-release.sh at ${commit:0:12}, $("$llvm_bin/llvm-bolt" --version | grep -m1 'LLVM version' | xargs) ($(date -u +%T))"
    # From the zbook path, as the gate runs: the inputs, the corpus and BUILD.txt then have zbook's paths.
    (cd "$ZBOOK_REPO" && env PATH="$llvm_bin:$PATH" RELEASE_BOLT_PROFILE=instrument "$@" \
      crates/ts_goport/scripts/build-release.sh "$out") > "$run/build.log" 2>&1 || rc=$?
    tail -25 "$run/build.log"
    # The bins and the logs, not the target dirs, the profiles or the BOLT inputs.
    (cd "$out" 2> /dev/null && for f in bin build-*.log cpu-check-* bolt/*.log bolt/data/*.out; do
      [[ ! -e $f ]] || echo "$f"
    done) > "$run/pack.list" || true
    if [[ -s $run/pack.list ]]; then tar -czf "$run/out.tar.gz" -C "$run" build.log -C "$out" -T "$PWD/$run/pack.list"
    else tar -czf "$run/out.tar.gz" -C "$run" build.log; fi
    rm -rf "$out"
    echo "packed $run/out.tar.gz ($(du -h "$run/out.tar.gz" | cut -f1)); rc $rc ($(date -u +%T))"
    exit "$rc"
    ;;
  *) die "unknown command $cmd (testbox-remote.sh help)" ;;
esac
