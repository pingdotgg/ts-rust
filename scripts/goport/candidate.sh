#!/usr/bin/env bash
# Candidate revisions of target/worktrees/checker-port. The protected set is goport's own tests and the
# gate items (docs/typechecker-accountability.md, "Protected set"); the legacy roster drive is retired.
#
# usage: scripts/goport/candidate.sh <command> ... [--dry-run]
#   help                   this text
#   check <branch>         Scope of the <branch> diff against the allowedChangedFiles of the revision (the
#                          batch's, or a new goport batch's when the batch is accepted or with --new-batch),
#                          no change of a protected path (open_revision.py --protected) since the branch's
#                          merge base with main, rustfmt --edition 2024 on each changed .rs file, and no stale
#                          lib blob (lib-blobs.sh stale). Exit 1 on any problem.
#   open <rev> <branch> --hypothesis TEXT --change TEXT [--message TEXT] [--new-batch ID --origin TEXT]
#                          check, apply <branch> to the checkout (paths in scope, one commit), fp.py, and
#                          open_revision.py last (the only state write). A new batch has protectedSet goport.
#   side <rev|label> [--checkout DIR] [--gate-host HOST|local | --host testbox] [--name-map TSV]
#                          In systemd user unit ts-rust-side-<label>, log
#                          target/continuation-r97-goport/quality-<label>-side.log: release bins in the
#                          shared target runtime/cargo-target; goport tests (build-goport-tests.sh,
#                          goport-tests.sh, compare-tests.py against the base results; --name-map for moved
#                          or renamed tests); two bound runs; the full gate and gate-compare.py against the
#                          base gate manifest; the LSP and API oracles, each compared per request with the
#                          base results (oracle-compare.py); rustfmt and clippy on ts_goport and the kept
#                          crates (clippy target: the checkout's target/candidate-clippy; the record is reused
#                          only for the checkout's fp.py). The gate and the oracles run on --gate-host (default
#                          dbook-lan, which remote.sh keeps for this; REMOTE_USE_RESERVED=1 is set below). The base
#                          is the last accepted revision (open_revision.py --base). Each step is cached by
#                          source under evidence-cache/<key>/ and reused. The oracle compares use the answer
#                          sets of the base (protectedBase.oracleAnswers) and of the batch (batch.oracleAnswers)
#                          with --answers. With batch.oracleRebase (a pin bump, reviewer ruling 10) their base
#                          is its runs of each kind, with --parity, the API knownDiffs and --identity. When the
#                          batch is accepted, the side run is for the next batch, which open_revision.py opens
#                          without the batch's oracleRebase, gateIdMap and gateToolChanges: side drops them too.
#                          A passing compare is reused only when it used the same base runs, answer sets and
#                          rebase record; else its <kind>.json or <kind>-fail.json moves to
#                          <kind>[-fail]-key-<sha256 prefix of its key>.json with its <kind>-compare.json, and
#                          the run is compared again (the oracle runs stay in lsp-run and api-run, so no oracle
#                          runs again). A gate compare is reused only with the same base and the batch's gate id
#                          map; one of another map moves to gate-compare-idmap-<sha256 prefix>.json. Check first
#                          with --dry-run that every other step says "reuse".
#                          --host testbox runs the heavy steps on one Blacksmith testbox (testbox.sh) instead:
#                          the release bins, test bins, goport tests, bound runs, gate, LSP and API oracles and
#                          clippy, with the same commands and paths. The testbox workspace is this checkout
#                          (TOOLS); `testbox.sh source` puts the checkout at its own path there (fp.py must
#                          match). The results come back to the same zbook paths (bins and test bins with
#                          their sha256 checked); the compares, rustfmt and the records stay on zbook. The key
#                          and the cache are the same, so either host reuses the other's steps. The copies and
#                          the testbox ID go to sync.log; the unit stops the testbox at its end. After a testbox
#                          failure, run side again without --host testbox (dbook-lan) to finish.
#                          Host kinds (testbox, or ssh: zbook and the SSH hosts): with batch.oracleRebase, the
#                          rebase runs and this source's oracle runs must be of one kind (side stops before the
#                          build otherwise). A gate on another kind than the base's gate fails the side run
#                          when an editor long growth is within 0.15 MiB/edit of its cap.
#                          An integration runs `side int<N> --checkout <its worktree>` in its checks step. The
#                          revision's side run then reuses its bins, test run, gate run and oracle runs (same
#                          crates tree, so same key) and runs only the bound runs. A tests or gate compare
#                          made with an older base (the integration ran before the last accept) moves to
#                          tests-base-<sha>.json or gate-compare-base-<label>.json and runs again, as does a
#                          gate or oracle compare of another gate id map, rebase record or answer sets (above).
#   verdict-request <rev>  The request text for the auditor and the reviewer, then the accept command.
# --dry-run prints each command that writes and runs only the read-only checks.
# The state, target/ and the host commands (gate.sh, lsp_oracle.py, api_oracle.py, which remote.sh sync-scripts
# copies from the main checkout) use the main checkout ROOT. The local helpers run from the checkout of this
# script (TOOLS), so a worktree copy can be tried before its merge; root runs the main checkout's copy.
#
# A monitor of side greps only for the last log line. It is exactly one of
#   SIDE DONE | SIDE FAIL rc=<N>
# (a timeout is "FAIL rc=124"). A killed unit prints nothing, so also stop when the unit is gone:
#   until grep -qE '^SIDE (DONE|FAIL)' LOG || ! systemctl --user is-active -q UNIT; do sleep 30; done
set -euo pipefail

# Evidence builds (release bins, test bins, the evidence key and the quality step's rustfmt and
# clippy) use the shipped toolchain of build-release.sh and release.yml, not the host default. On
# alvin, 1.93.0 took about twice as long and gave the same output (scripts/goport/README.md,
# "Build toolchain"). Install it with: rustup toolchain install 1.95.0 --profile minimal -c rustfmt,clippy
export RUSTUP_TOOLCHAIN=1.95.0

ROOT=/home/theo/Code/sandbox/ts-rust
R=$ROOT/target/continuation-r97-goport
SELF=$(realpath "${BASH_SOURCE[0]}")
TOOLS=$(dirname "$(dirname "$(dirname "$SELF")")")  # the checkout of this script
G=$TOOLS/scripts/goport
EVIDENCE=${CANDIDATE_EVIDENCE:-$R/evidence-cache}  # tests point this at a copy
TARGET=$R/runtime/cargo-target                     # one warm target for candidate bins (gate.sh's default)
export REMOTE_USE_RESERVED=1                       # remote.sh keeps dbook-lan for revision evidence
BINS=(goport goport_emit goport_typesyms goport_build tsgo)
LSP_BATTERIES=b1-inline,b1-query-core,b2-query-core,b1-hono,b2-hono,fourslash
# The API oracle batteries: the 10 of pin 52168999f3dc (bumpA4 verify), and from pin 16c25522e123 (bump B)
# also API_EXT_BATTERIES (api_oracle.py build --kind ext: the API methods that bump B adds or changes). The
# oracles of the older pins have no ext goldens: API_NO_EXT_ORACLES are their sha256 prefixes (52168999f3dc:
# d2dc9ff46ff5, dc37b5249ab6: 204f15c76702). A run at any other pin runs all 13; a missing golden fails it.
API_BATTERIES=(effect hono hono-xchecker qc qc-callbacks qc-lsp qc-proto qc-xchecker tsp-lsp zod)
API_EXT_BATTERIES=(hono-ext qc-ext zod-ext)
API_NO_EXT_ORACLES=(d2dc9ff46ff5 204f15c76702)
# The kept crates (not legacy): protected unit tests, and rustfmt and clippy in the quality step.
KEPT_CRATES=(ts_scanner ts_ast ts_diagnostics ts_path ts_core ts_jsnum)
CHECKER_PORT=$ROOT/target/worktrees/checker-port
cd "$ROOT"

usage() { sed -n '2,/^set -euo/p' "$SELF" | sed '$d'; exit "${1:-2}"; }
die() { echo "candidate.sh: $*" >&2; exit 1; }
say() { echo "== $*"; }
# need <condition text>: stops, except in a dry run, where an earlier step (open) would fix it.
need() { if [[ $DRY == 1 ]]; then echo "   (dry run; a real run stops here) $*"; else die "$*"; fi; }
# run <cmd...>: prints the command, then runs it unless --dry-run.
run() { printf '+'; printf ' %q' "$@"; printf '\n'; [[ $DRY == 1 ]] || "$@"; }
# run_sh <shell text>: the same for a command with redirects or pipes. fd 8 (the target lock) stays closed.
run_sh() { printf '+ %s\n' "$1"; [[ $DRY == 1 ]] || bash -c "set -euo pipefail; $1" 8>&-; }
sha() { sha256sum < "$1" | cut -c1-64; }

# load_state: exports the saved state once; st <jq filter> reads it.
load_state() { STATE=$(mktemp /tmp/candidate-state.XXXXXX); trap 'rm -f "$STATE"' EXIT; node scripts/state.mjs export > "$STATE"; }
st() { jq -r "$1" "$STATE"; }

# scope: sets POS and NEG, the include and exclude globs of the next revision's allowedChangedFiles
# ("!" entries exclude). A new goport batch (--new-batch, or any revision after an accepted batch)
# gets open_revision.py's list; an open batch keeps its own.
scope() {
  local p
  local -a all
  if [[ -n $NEW_BATCH || $(st .batch.compilerAccepted) == true ]]; then
    mapfile -t all < <(python3 "$G/open_revision.py" --allowed)
  else
    mapfile -t all < <(st '.batch.allowedChangedFiles[]')
  fi
  POS=() NEG=()
  for p in "${all[@]}"; do if [[ $p == '!'* ]]; then NEG+=("${p#!}"); else POS+=("$p"); fi; done
}
# in_list <path> [entries...]: true when the path is one of the entries, as text.
in_list() {
  local path=$1 e
  shift
  for e in "$@"; do if [[ $path == "$e" ]]; then return 0; fi; done
  return 1
}
# in_scope <path> [globs...]: true when the path matches a glob. Globs match like fnmatch (* crosses /).
in_scope() {
  local path=$1 pat
  shift
  # shellcheck disable=SC2053 # the patterns are globs
  for pat in "$@"; do if [[ $path == $pat ]]; then return 0; fi; done
  return 1
}
# side_state: the saved state as a side run judges it (SIDE_STATE, a jq filter). An accepted batch is the base of the
# next batch, which open_revision.py --new-batch opens without the accepted batch's oracleRebase, gateIdMap and
# gateToolChanges (the records of its pin bump), so a side run after an accept drops them too (int90: accepted batch
# 103's rebase failed its oracle compares, and its gate id map made a gate compare that batch 104 refused).
SIDE_STATE='if .batch.compilerAccepted == true then .batch |= del(.oracleRebase, .gateIdMap, .gateToolChanges) else . end'
side_state() { node scripts/state.mjs export | jq -c "$SIDE_STATE"; }
# oracle_setup <base json>: the base results dirs and extra arguments of oracle-compare.py per kind (lsp, api).
# OBASE[kind]: the base dirs, protectedBase's run (the <base json> .lsp.dir or .api.dir) or the runs of
# batch.oracleRebase. OARGS[kind]: --answers for each answer set of the base (protectedBase.oracleAnswers) and of
# the batch (batch.oracleAnswers) of that kind, each once, and with oracleRebase --parity, --known-diff for each
# key of its knownDiffs and --identity; then --kind. Each answer set file must have its sha256. OKEY[kind] names
# the base dirs, the arguments and the rebase record: a passing compare is reused only with the same key.
# OREBASE[kind] is the rebase record of the kind (JSON) or empty.
declare -A OBASE=([lsp]='' [api]='') OARGS=([lsp]='' [api]='') OKEY=([lsp]='' [api]='') OREBASE=([lsp]='' [api]='')
oracle_setup() {
  local base=$1 cfg kind path hash dir
  cfg=$(side_state | jq -c --argjson base "$base" '{rebase: .batch.oracleRebase,
    sets: ([($base.oracleAnswers // [])[], (.batch.oracleAnswers // [])[]] | unique_by([.kind, .sha256]) | sort_by(.sha256))}')
  for kind in lsp api; do
    OARGS[$kind]='' OREBASE[$kind]=$(jq -c --arg k "$kind" '.rebase[$k] // empty' <<< "$cfg")
    while IFS=$'\t' read -r path hash; do
      [[ $path == /* ]] || path=$ROOT/$path
      [[ -f $path && $(sha "$path") == "$hash" ]] || die "answer set $path does not have the sha256 $hash"
      OARGS[$kind]+="--answers $path@$hash "
    done < <(jq -r --arg k "$kind" '.sets[] | select(.kind == $k) | [.path, .sha256] | @tsv' <<< "$cfg")
    if [[ -n ${OREBASE[$kind]} ]]; then
      OBASE[$kind]=''
      while IFS= read -r dir; do OBASE[$kind]+="${OBASE[$kind]:+ }$dir"; done < <(jq -r '.runs[].dir' <<< "${OREBASE[$kind]}")
      OARGS[$kind]+="--parity --identity $(jq -r '(.knownDiffs // [])[] | "--known-diff " + (.key | @sh) + " "' <<< "${OREBASE[$kind]}" | tr -d '\n')"
    else
      OBASE[$kind]=$(jq -r --arg k "$kind" '.[$k].dir // empty' <<< "$base")
    fi
    [[ -z ${OARGS[$kind]} ]] || OARGS[$kind]+="--kind $kind "
    OKEY[$kind]="${OBASE[$kind]}${OARGS[$kind]:+ ${OARGS[$kind]% }}"
    [[ -z ${OREBASE[$kind]} ]] || OKEY[$kind]+=" rebase=$(jq -cS . <<< "${OREBASE[$kind]}" | sha256sum | cut -c1-16)"
  done
}

# pathspecs: git pathspecs of POS minus NEG.
pathspecs() {
  local p
  for p in "${POS[@]}"; do printf '%s\n' ":(glob)$p"; done
  for p in "${NEG[@]}"; do printf '%s\n' ":(exclude,glob)$p"; done
}

# fmt_clean <rev:path>: true when rustfmt --edition 2024 leaves that blob unchanged. Each file is formatted
# alone from stdin, so unchanged child modules do not count. A parse error counts as a problem.
fmt_clean() {
  local blob rc=0
  blob=$(mktemp /tmp/candidate-fmt.XXXXXX)
  git cat-file blob "$1" > "$blob"
  # shellcheck disable=SC2094 # both sides only read the file
  rustfmt --edition 2024 --emit stdout < "$blob" 2> /dev/null | cmp -s - "$blob" || rc=1
  rm -f "$blob"
  return $rc
}

# check_branch <branch>: problems of <branch> as the next source of the checkout, against its HEAD. Paths
# that an exclude glob names (the saved state, target/) are skipped: open does not apply them. Prints each
# problem and returns 1 when there is one.
check_branch() {
  local branch=$1 wt base sha status path mb n=0 nrs=0 skipped=0
  local -a problems=() protected
  wt=$ROOT/$(st .batch.checkout)
  base=$(git -C "$wt" rev-parse HEAD)
  sha=$(git rev-parse --verify --quiet "$branch^{commit}") || die "unknown branch or commit: $branch"
  scope
  say "check $branch ${sha:0:9} against $(st .batch.checkout) ${base:0:9}; scope: ${POS[*]}${NEG[*]:+ except ${NEG[*]}}"
  # Reverts: a path where the checkout already has main's version (an accepted change, merged into main),
  # the branch has another version, and the branch did not change the path since its merge base with main.
  # Applying the branch would undo the accepted change (bump B wave 3 would have reverted R131's Cargo
  # files). Merge main into the branch first. Paths where the checkout is older than main do not count.
  mb=$(git merge-base main "$sha")
  local -a own
  local -A main_blob base_blob
  local mode type obj
  mapfile -t own < <(git diff --no-renames --name-only "$mb" "$sha")
  while IFS=$' \t' read -r -d '' mode type obj path; do main_blob[$path]=$obj; done < <(git ls-tree -r -z main)
  while IFS=$' \t' read -r -d '' mode type obj path; do base_blob[$path]=$obj; done < <(git ls-tree -r -z "$base")
  while IFS= read -r -d '' status && IFS= read -r -d '' path; do
    if in_scope "$path" "${NEG[@]}"; then skipped=$((skipped + 1)); continue; fi
    n=$((n + 1))
    in_scope "$path" "${POS[@]}" || problems+=("outside allowedChangedFiles: $path")
    if [[ -n ${base_blob[$path]:-} && ${main_blob[$path]:-} == "${base_blob[$path]}" ]] && ! in_list "$path" "${own[@]}"; then
      problems+=("reverts $path: the checkout has main's version and the branch did not change it (merge main into the branch first)")
    fi
    if [[ $status != D && $path == *.rs ]]; then
      nrs=$((nrs + 1))
      fmt_clean "$sha:$path" || problems+=("rustfmt --edition 2024 would change $path")
    fi
  done < <(git diff --no-renames --name-status -z "$base" "$sha")
  ((skipped == 0)) || echo "   $skipped changed path(s) under the excluded globs are not applied"
  # A stale lib blob fails the protected snapshot_matches_live_* tests: 59 agents met one from 2026-09-28 to 30.
  local line
  while IFS= read -r line; do problems+=("$line"); done < <("$G/lib-blobs.sh" stale "$sha" || true)
  # Protected paths the branch itself changed (from its merge base with main, so paths that only main changed
  # since do not count). A batch that must change one lists the exact path and no exclude glob names it.
  mapfile -t protected < <(python3 "$G/open_revision.py" --protected)
  while IFS= read -r -d '' path; do
    in_scope "$path" "${protected[@]}" || continue
    if in_list "$path" "${POS[@]}" && ! in_scope "$path" "${NEG[@]}"; then continue; fi
    problems+=("changes protected path $path (it judges the protected set; a batch that must change it lists the exact path in allowedChangedFiles, with Theo's approval)")
  done < <(git diff --no-renames --name-only -z "$mb" "$sha")
  if ((${#problems[@]})); then
    printf '   PROBLEM %s\n' "${problems[@]}"
    echo "check FAIL: ${#problems[@]} problem(s) in $n changed path(s)"
    return 1
  fi
  echo "check OK: $n changed path(s) in scope, $nrs .rs file(s) rustfmt clean"
}

# cmd_open <rev> <branch>: makes <branch> the source of R<rev> and opens R<rev>. The state is written last,
# so a failed step leaves no revision row.
cmd_open() {
  local rev=$1 branch=$2 last wt base sha patch msg commit fp dirty
  local -a specs orv
  [[ -n $HYP && -n $CHANGE ]] || die "open needs --hypothesis and --change"
  [[ -z $NEW_BATCH || -n $ORIGIN ]] || die "--new-batch needs --origin"
  last=$(st '.batch.recoveryHistory[-1].revision')
  ((rev == last + 1)) || die "R$rev is not the next revision (R$((last + 1)))"
  [[ -n $NEW_BATCH || $(st .batch.compilerAccepted) != true ]] || die "batch $(st .batch.id) is accepted; pass --new-batch ID --origin TEXT"
  wt=$ROOT/$(st .batch.checkout)
  dirty=$(git -C "$wt" status --porcelain | grep -v '^?? crates/ts_goport/CANDIDATE.md$' || true)
  [[ -z $dirty ]] || die "$wt has changes besides CANDIDATE.md:"$'\n'"$dirty"
  check_branch "$branch" || die "check failed; nothing was changed"

  sha=$(git rev-parse "$branch^{commit}") base=$(git -C "$wt" rev-parse HEAD)
  mapfile -t specs < <(pathspecs)
  if git diff --quiet "$base" "$sha" -- "${specs[@]}"; then
    say "$(st .batch.checkout) already equals $branch in scope; no commit"
    commit=$(git -C "$wt" rev-parse --short=9 HEAD)
  else
    say "apply $branch to $(st .batch.checkout):$(git diff --shortstat "$base" "$sha" -- "${specs[@]}")"
    patch=/tmp/candidate-r$rev.patch msg=$MESSAGE
    [[ -n $msg ]] || printf -v msg 'chore(goport): R%s source from %s %s\n\n%s' "$rev" "$branch" "${sha:0:9}" "$CHANGE"
    run_sh "git diff --binary $base $sha -- $(printf '%q ' "${specs[@]}")> $patch"
    run git -C "$wt" apply --index "$patch"
    run git -C "$wt" commit -q -m "$msg"
    commit='<new HEAD>'
  fi
  if [[ $commit == '<'* && $DRY == 1 ]]; then
    fp='<fp.py after the apply>'
  else
    git -C "$wt" diff --quiet "$sha" HEAD -- "${specs[@]}" || die "$wt differs from $branch in scope"
    commit=$(git -C "$wt" rev-parse --short=9 HEAD)
    fp=$(python3 "$G/fp.py" "$wt" | cut -d' ' -f1)
  fi
  say "R$rev commit $commit, source fingerprint $fp"

  orv=(python3 "$G/open_revision.py" --revision "$rev" --fingerprint "$fp" --commit "$commit"
    --hypothesis "$HYP" --change "$CHANGE")
  [[ -z $NEW_BATCH ]] || orv+=(--new-batch "$NEW_BATCH" --origin "$ORIGIN")
  if [[ $fp != '<'* ]]; then
    say "open_revision.py checks (--dry-run, no write)"
    if [[ $DRY == 1 ]]; then "${orv[@]}" --dry-run; else "${orv[@]}" --dry-run > /dev/null; fi
  fi
  run "${orv[@]}"
  say "then: $SELF side $rev; after SIDE DONE: $SELF verdict-request $rev"
  say "commit the state: git add docs && git commit"
}

# evidence_key <checkout> [pin]: cache key of the bins and their evidence. It covers the committed crates
# tree, Cargo.toml and Cargo.lock, each dirty or untracked file under them (except the CANDIDATE.md marker,
# as in bound2.sh), the Go pin (default: UPSTREAM.json current) and the build profile.
evidence_key() {
  local wt=$1 pin=${2:-} line
  [[ -n $pin ]] || pin=$(jq -r .current "$ROOT/UPSTREAM.json")
  {
    git -C "$wt" rev-parse HEAD:crates HEAD:Cargo.toml HEAD:Cargo.lock
    git -C "$wt" status --porcelain --untracked-files=all -- crates Cargo.toml Cargo.lock | while IFS= read -r line; do
      [[ $line != '?? crates/ts_goport/CANDIDATE.md' ]] || continue
      echo "$line"
      if [[ -f $wt/${line:3} ]]; then sha256sum < "$wt/${line:3}"; fi
    done
    echo "pin=$pin profile=release toolchain=$(rustc --version 2>/dev/null) incremental=0"
  } | sha256sum | cut -c1-16
}

# cmd_side <rev|label>: starts side_unit in systemd user unit ts-rust-side-<label>.
cmd_side() {
  local label=$1 wt pin unit log
  if [[ $label =~ ^[0-9]+$ ]]; then label=r$label; fi
  [[ $label =~ ^[A-Za-z0-9._-]+$ ]] || die "label must match [A-Za-z0-9._-]+"
  wt=$(realpath "${CHECKOUT:-$ROOT/$(st .batch.checkout)}")
  pin=$(st '.batch.upstreamPin.to // empty')
  unit=ts-rust-side-$label log=$R/quality-$label-side.log
  [[ ! -e $log ]] || need "$log exists; rename it first (finished steps stay cached)"
  ! systemctl --user is-active -q "$unit" || need "$unit is running"
  local -a env=(--setenv=PATH="$PATH" --setenv=HOME="$HOME")
  [[ -z ${SSH_AUTH_SOCK:-} ]] || env+=(--setenv=SSH_AUTH_SOCK="$SSH_AUTH_SOCK")
  [[ -z ${CANDIDATE_EVIDENCE:-} ]] || env+=(--setenv=CANDIDATE_EVIDENCE="$CANDIDATE_EVIDENCE")
  say "side $label in unit $unit, log $log"
  run systemd-run --user --collect --unit="$unit" --working-directory="$ROOT" "${env[@]}" \
    bash -c "exec bash $SELF _side-unit $label $wt ${pin:--} $HOST ${NAME_MAP:--} >> $log 2>&1"
  if [[ $DRY == 1 ]]; then say "the unit runs:"; side_unit "$label" "$wt" "${pin:--}" "$HOST" "${NAME_MAP:--}"; fi
  echo "monitor: until grep -qE '^SIDE (DONE|FAIL)' $log || ! systemctl --user is-active -q $unit; do sleep 30; done; tail -8 $log"
}

# free_name <dir> <name>: <name>, or <name>-2, -3, ... when <dir>/<name> exists (gate labels are never reused).
free_name() { local n=$2 i=2; while [[ -e $1/$n ]]; do n=$2-$i i=$((i + 1)); done; echo "$n"; }

# The testbox of side --host testbox (testbox.sh, the copy of TOOLS: the testbox workspace is that checkout).
# side_unit sets TBX_LOG (its sync.log) and TESTBOX_ID_FILE. tbx_up starts the testbox once: warmup, the checkout
# at its own path (testbox.sh source), the Go pin checkout and oracle and the pin cache. tbx_down stops it (never
# release: a release commits the shared warm target).
TB=$G/testbox.sh TBX_UP=0 TBX_LOG=/dev/null
tbx_up() {
  ((TBX_UP == 0)) || return 0
  TBX_UP=1
  say "$(date -u +%FT%TZ) testbox: warmup, source $1, pin ${2:-current}"
  run_sh "$TB warmup >> $TBX_LOG 2>&1"
  run_sh "$TB source $1 >> $TBX_LOG 2>&1"
  if [[ -n $2 ]]; then
    run_sh "$TB pin $2 >> $TBX_LOG 2>&1 && $TB oracle $2 >> $TBX_LOG 2>&1"
    run_sh "$TB push $(jq -r .pinRoot "$ROOT/UPSTREAM.json")/$2 >> $TBX_LOG 2>&1"
  fi
  [[ $DRY == 1 ]] || say "testbox $(cat "$TESTBOX_ID_FILE")"
}
tbx_down() { if ((TBX_UP)) && [[ $DRY == 0 ]]; then "$TB" stop >> "$TBX_LOG" 2>&1 || true; fi; }
# tbx_job <log> <command text>: runs the command on the testbox in the zbook path of the main checkout ($ROOT
# there is the TOOLS workspace), as a detached job; its output goes to the log.
tbx_job() { run_sh "$TB job bash -c $(printf %q "cd $ROOT && $2") < /dev/null > $1 2>&1"; }
# tbx_get <testbox dir> <zbook dir>: copies a dir back; the zbook dir must not exist (results are never replaced).
tbx_get() {
  [[ $DRY == 1 || ! -e $2 ]] || die "$2 exists (results are never replaced)"
  run_sh "$TB get $1/ $2/ >> $TBX_LOG 2>&1"
}
# tbx_have <dir>: copies a zbook dir (bins, test bins) to the testbox once per side run.
declare -A TBX_HAVE=()
tbx_have() { [[ -n ${TBX_HAVE[$1]:-} ]] || { run_sh "$TB push $1 >> $TBX_LOG 2>&1"; TBX_HAVE[$1]=1; }; }

# side_unit <label> <checkout> <pin|-> <host|local> <name map|->: body of the side unit. Each finished step
# is a file in the cache dir (bins/, testbin/, tests-run/, tests.json, bound.json, gate.json,
# gate-compare.json, lsp.json, api.json, quality.json) and is reused. A failed compare writes tests-fail.json,
# lsp-fail.json or api-fail.json instead. The test run (tests-run/) and the oracle runs (lsp-run, api-run: the
# label) stay, so the next run only compares them again. A failed gate compare keeps that run for good
# (repeat-run rule): gate.json becomes gate-fail-<gate label>.json and its compare gate-compare-fail-<gate
# label>.json. They are never removed or replaced; the next run runs a new gate with a new label. To compare
# the same gate again (for example after a state fix), copy gate-fail-<label>.json to gate.json. The
# verdict request lists every failed run, and accept_revision.py refuses one without a flake note. The other
# steps still run, so the verdict request has all the evidence. Last log line: SIDE DONE or SIDE FAIL rc=<N>.
side_unit() {
  local label=$1 wt=$2 pin=${3#-} host=$4 map=${5#-} key C B commit fp n r gl ll al rc oracle lock synced=0 g l fails='' base old
  local bt bts bg bgs bl ba
  local -a pinexec=()
  [[ $DRY == 1 ]] || trap 'rc=$?; tbx_down; if ((rc)); then echo "SIDE FAIL rc=$rc"; else echo "SIDE DONE"; fi' EXIT
  [[ -z $pin ]] || pinexec=(env GOPORT_PIN="$pin" python3 "$ROOT/scripts/upstream/pin.py" exec --)
  key=$(evidence_key "$wt" "$pin") C=$EVIDENCE/$key B=$EVIDENCE/$key/bins
  commit=$(git -C "$wt" rev-parse HEAD) lock=/tmp/goport-remote-${host%-lan}.lock
  local tbx=0 on=''
  if [[ $host == testbox ]]; then
    tbx=1 on=' on the testbox' TBX_LOG=$C/sync.log
    export TESTBOX_ID_FILE=$R/runtime/testbox-side-$label.id TESTBOX_IDLE=${TESTBOX_IDLE:-30}
  fi
  say "$(date -u +%FT%TZ) side $label: $wt ${commit:0:9}, crates tree $(git -C "$wt" rev-parse HEAD:crates | cut -c1-12), pin ${pin:-current}, evidence $C"
  # The base: the last accepted revision's goport test results and gate manifest, pinned by sha256.
  # The base LSP and API results are results dirs (bl, ba), compared per request.
  if base=$(python3 "$G/open_revision.py" --base); then
    bt=$(jq -r .tests.path <<< "$base") bts=$(jq -r .tests.sha256 <<< "$base")
    bg=$(jq -r .gate.path <<< "$base") bgs=$(jq -r .gate.sha256 <<< "$base")
    bl=$(jq -r '.lsp.dir // empty' <<< "$base") ba=$(jq -r '.api.dir // empty' <<< "$base")
    [[ $(sha "$bt") == "$bts" && $(sha "$bg") == "$bgs" ]] || die "the base files differ from their sha256: $base"
    say "base $(jq -r '"\(.batch) R\(.revision)"' <<< "$base"): tests $bt, gate $bg, LSP ${bl:-none}, API ${ba:-none}"
  else
    need "no protected base (open_revision.py --base)"
    bt='<base results.json>' bg='<base gate manifest>' bl='<base LSP results>' ba='<base API results>'
  fi
  # A pin rebase (batch.oracleRebase) needs "the same host setup" for its base runs and the candidate's oracle runs
  # (ruling 10 condition 1). Testbox bins and hosts differ from zbook and the SSH hosts, so both sides use one host
  # kind: testbox, or ssh (zbook and the SSH hosts). A reused oracle run counts with the host it ran on
  # (<kind>-run.host; a run without one predates --host testbox, so it is ssh). oracle-rebase.sh runs on the SSH
  # hosts. This stops before any build.
  local k rk ok
  while read -r k rk; do
    ok=$host
    [[ ! -f $C/$k-run ]] || ok=$(cat "$C/$k-run.host" 2> /dev/null || echo ssh)
    [[ $ok == testbox ]] || ok=ssh
    [[ $rk == "$ok" ]] || die "batch.oracleRebase $k runs ran on $rk hosts, and this source's $k oracle runs on $ok: ruling 10 needs one host kind for both (oracle-rebase.sh runs on the SSH hosts, so run side without --host testbox)"
  done < <(side_state | jq -r '(.batch.oracleRebase // {}) | to_entries[] | select(.key == "lsp" or .key == "api")
    | "\(.key) \([(.value.runs // [])[] | if .host == "testbox" then "testbox" else "ssh" end] | unique | join(","))"')
  run mkdir -p "$C"

  # Bins (release profile). The target lock keeps another side run from replacing the target's bins
  # between this build and the copy.
  if [[ -f $B/bins.sha256 ]]; then say "reuse bins $B (built from $(cat "$B/COMMIT"))"; else
    # The shared candidate target only for the candidate checkout: another checkout builds in its own
    # target. Cargo names a workspace crate's artifacts by its workspace-relative path and judges freshness
    # by mtime, so a build from another worktree into the shared target can look fresh to the next
    # checker-port build and be linked as stale code (R132 side try 1: goport_util from goport-legacy1).
    local target=$TARGET
    [[ $(realpath "$wt") == "$(realpath "$CHECKER_PORT")" ]] || target=$wt/target/candidate-side
    if ((tbx)); then
      # The same build on the testbox (32 jobs), in the same target path there; its purge output goes to
      # build.log. The bins come back to B.new with their sha256 checked, and the testbox's rustc must be zbook's
      # (the evidence key names zbook's).
      tbx_up "$wt" "$pin"
      say "$(date -u +%FT%TZ) build release bins in $target on the testbox"
      tbx_job "$C/build.log" "mkdir -p $C && python3 scripts/goport/purge-foreign-fingerprints.py $target $wt && cd $wt && echo \"toolchain: \$(rustc --version)\" && TS_CARGO_NIGHTLY=0 TS_CARGO_INCREMENTAL=0 TS_CARGO_LOCK_ID=candidate-side TS_CARGO_JOBS=32 TS_CARGO_SEPARATE_TARGET=1 CARGO_TARGET_DIR=$target $ROOT/scripts/run-cargo-capped.sh build --locked --release -p ts_goport ${BINS[*]/#/--bin } && rm -rf $B $B.new && mkdir $B.new && cd $target/release && cp ${BINS[*]} $B.new/ && cd $B.new && sha256sum ${BINS[*]} > bins.sha256 && echo $commit > COMMIT && mv $B.new $B"
      run_sh "rm -rf $B.new"
      tbx_get "$B" "$B.new"
      run_sh "cd $B.new && sha256sum --quiet -c bins.sha256"
      TBX_HAVE[$B]=1
      if [[ $DRY == 0 ]]; then
        [[ $(grep -m 1 '^toolchain: ' "$C/build.log" | cut -d' ' -f2-) == "$(rustc --version 2> /dev/null)" ]] ||
          die "the testbox built with $(grep -m 1 '^toolchain: ' "$C/build.log"), the evidence key names $(rustc --version 2> /dev/null)"
      fi
    else
      say "$(date -u +%FT%TZ) build release bins in $target"
      if [[ $DRY == 0 ]]; then exec 8> /tmp/ts-rust-candidate-target.lock; flock 8; fi
      # Evidence bins: the shipped toolchain (RUSTUP_TOOLCHAIN above, not the nightly edit-loop default) and
      # no incremental cache.
      # Units whose dep-info names another checkout's files (a crate with sources outside its package dir,
      # like goport_util) are removed first, so cargo builds them again from this checkout.
      run_sh "python3 $G/purge-foreign-fingerprints.py $target $wt > $C/purge.log 2>&1"
      run_sh "cd $wt && TS_CARGO_NIGHTLY=0 TS_CARGO_INCREMENTAL=0 TS_CARGO_LOCK_ID=candidate-side TS_CARGO_JOBS=12 TS_CARGO_SEPARATE_TARGET=1 CARGO_TARGET_DIR=$target $ROOT/scripts/run-cargo-capped.sh build --locked --release -p ts_goport ${BINS[*]/#/--bin } > $C/build.log 2>&1"
      run_sh "rm -rf $B.new && mkdir $B.new && cd $target/release && cp ${BINS[*]} $B.new/ && cd $B.new && sha256sum ${BINS[*]} > bins.sha256 && echo $commit > COMMIT"
    fi
    if [[ $DRY == 0 ]]; then
      exec 8>&-
      [[ $(evidence_key "$wt" "$pin") == "$key" ]] || die "the source changed during the build; $B.new is not cached"
    fi
    run mv "$B.new" "$B"
  fi

  # goport tests: the test binaries of the checkout (build-goport-tests.sh takes the target lock itself),
  # every protected suite (goport-tests.sh, on zbook) and the per-name compare with the base results.
  # The run is cached in tests-run/; only a passing compare is cached, so a rerun with --name-map only
  # compares again. A compare with an older base (an integration's side run before the last accept) is
  # not reused: it moves to tests-base-<sha256 prefix>.json, and the run is compared with the current base.
  if [[ -f $C/tests.json && $(jq -r .baseSha256 "$C/tests.json") == "$bts" ]]; then
    say "reuse goport tests $(jq -c .compare "$C/tests.json")"
  else
    if [[ -f $C/tests.json ]]; then
      old=$(jq -r .baseSha256 "$C/tests.json" | cut -c1-12)
      say "goport tests: the cached compare used base $(jq -r .base "$C/tests.json"); keep it as tests-base-$old.json and compare again"
      run mv "$C/tests.json" "$C/tests-base-$old.json"
      run mv "$C/tests-compare.json" "$C/tests-compare-base-$old.json"
    fi
    if [[ -f $C/testbin/bins.sha256 ]]; then say "reuse test bins $C/testbin (built from $(cat "$C/testbin/COMMIT"))"
    elif ((tbx)); then
      # On the testbox at the checkout's own path, so BUILD_ROOT and BUILD_TARGET are the zbook paths. The dir
      # comes back (binaries too) with its sha256 checked.
      tbx_up "$wt" "$pin"
      say "$(date -u +%FT%TZ) build test bins on the testbox"
      tbx_job "$C/testbin-build.log" "mkdir -p $C && rm -rf $C/testbin $C/testbin.new && scripts/goport/build-goport-tests.sh $wt $C/testbin" ||
        die "test bin build failed (log $C/testbin-build.log)"
      run_sh "rm -rf $C/testbin.new"
      tbx_get "$C/testbin" "$C/testbin.new"
      run_sh "cd $C/testbin.new && sha256sum --quiet -c bins.sha256 && cd .. && mv testbin.new testbin"
      TBX_HAVE[$C/testbin]=1
    else
      say "$(date -u +%FT%TZ) build test bins"
      run_sh "$G/build-goport-tests.sh $wt $C/testbin > $C/testbin-build.log 2>&1" || die "test bin build failed (log $C/testbin-build.log)"
    fi
    if [[ -f $C/tests-run/results.json ]]; then say "reuse test run $C/tests-run"; else
      say "$(date -u +%FT%TZ) goport tests$on"
      rc=0
      if ((tbx)); then
        tbx_up "$wt" "$pin"
        tbx_have "$C/testbin"
        # The test bins can come from another checkout or commit (an integration's side run). goport-tests.sh binds
        # a git archive of their COMMIT's crates over BUILD_ROOT/crates and relbin/ over BUILD_TARGET/release, so on
        # the testbox those two paths need only exist, and COMMIT must be in the testbox repo: from the bundle of
        # testbox.sh source (an ancestor of the checkout's HEAD that origin lacks) or else from origin.
        local tbc tbroot tbtarget
        tbc=$(cat "$C/testbin/COMMIT" 2> /dev/null || echo "<COMMIT>") tbroot=$(cat "$C/testbin/BUILD_ROOT" 2> /dev/null || echo "$wt")
        tbtarget=$(cat "$C/testbin/BUILD_TARGET" 2> /dev/null || echo "$wt/target/goport-tests")
        [[ $tbroot == "$ROOT"/target/?* && $tbtarget == "$ROOT"/target/?* ]] ||
          die "test bins $C/testbin: BUILD_ROOT $tbroot or BUILD_TARGET $tbtarget is not under $ROOT/target"
        tbx_job "$C/tests-run.log" "{ git cat-file -e $tbc^{commit} 2> /dev/null || git fetch -q --depth=1 origin $tbc ||
          { echo 'test bin commit $tbc (built in $tbroot) is not in the testbox repo or on origin: push it, or run side without --host testbox'; exit 3; }; } &&
          mkdir -p $tbroot/crates $tbtarget/release && rm -rf $C/tests-run.new && mkdir -p $C/tests-run.new &&
          scripts/goport/goport-tests.sh $C/testbin $C/tests-run.new ${pin:+--pin $pin}" || rc=$?
        run_sh "rm -rf $C/tests-run.new"
        tbx_get "$C/tests-run.new" "$C/tests-run.new" || ((rc)) || die "no tests-run.new on the testbox"
      else
        run_sh "rm -rf $C/tests-run.new && mkdir $C/tests-run.new && $G/goport-tests.sh $C/testbin $C/tests-run.new ${pin:+--pin $pin} > $C/tests-run.log 2>&1" || rc=$?
      fi
      if [[ $DRY == 0 ]]; then
        [[ $rc == 0 && $(tail -n 1 "$C/tests-run.log") == DONE && -f $C/tests-run.new/results.json ]] ||
          die "goport tests rc $rc, last line '$(tail -n 1 "$C/tests-run.log")' (log $C/tests-run.log)"
      fi
      run mv "$C/tests-run.new" "$C/tests-run"
    fi
    say "$(date -u +%FT%TZ) compare goport tests with the base"
    rc=0
    run_sh "python3 $G/compare-tests.py $bt $C/tests-run/results.json ${map:+--name-map $map }--out $C/tests-compare.json" || rc=$?
    if [[ $DRY == 0 ]]; then
      g=$C/tests.json; ((rc == 0)) || { g=$C/tests-fail.json; fails+="${fails:+; }goport tests: compare-tests.py rc $rc ($C/tests-compare.json)"; }
      jq --arg r "$C/tests-run/results.json" --arg s "$(sha "$C/tests-run/results.json")" --arg b "$(realpath "$bt")" --arg bs "$bts" \
        --arg tb "$C/testbin" --arg c "$(cat "$C/testbin/COMMIT")" --arg o "$C/tests-compare.json" \
        '{results: $r, sha256: $s, base: $b, baseSha256: $bs, testbin: $tb, commit: $c, compareOutput: $o, verdict,
          compare: (.total | {lost: (.lost | length), absent: (.absent | length), unrun: (.unrun | length), retained, recovered,
                              newNames, removedByMap, newFailed}),
          nameMap: (if .nameMap then {path: .nameMap.path, sha256: .nameMap.sha256} else null end),
          incomplete: .new.incomplete}' "$C/tests-compare.json" > "$g"
      ((rc)) || rm -f "$C/tests-fail.json"
    fi
  fi

  # Bound runs. bound2.sh measures only the checker-port checkout and records its commit and fingerprint.
  if [[ $wt != "$CHECKER_PORT" ]]; then say "skip bound runs: bound2.sh measures only $CHECKER_PORT"; else
    fp=$(python3 "$G/fp.py" "$wt" | cut -d' ' -f1)
    if [[ -f $C/bound.json && $(jq -r .sourceFingerprint "$C/bound.json") == "$fp" ]]; then
      say "reuse bound runs $(jq -r '.rounds | join(" ")' "$C/bound.json")"
    else
      n=0
      for r in "$R"/measure/r*; do r=${r##*/r}; if [[ $r =~ ^[0-9]+$ ]] && ((r > n)); then n=$r; fi; done
      n=r$((n + 1))
      for r in "$n" "${n}b"; do
        say "$(date -u +%FT%TZ) bound run $r$on"
        if ((tbx)); then
          # bound2.sh measures the testbox copy of the checker-port checkout (testbox.sh source: the same fp.py).
          tbx_up "$wt" "$pin"
          tbx_have "$B"
          tbx_job "$C/bound-$r.log" "GOPORT_REL=$B ${pinexec[*]} bash scripts/goport/bound2.sh $r"
          tbx_get "$R/measure/$r" "$R/measure/$r"
        else
          run_sh "GOPORT_REL=$B ${pinexec[*]} bash $G/bound2.sh $r > $C/bound-$r.log 2>&1"
        fi
      done
      if [[ $DRY == 0 ]]; then
        ! grep -HE 'DIFF|INCOMPLETE' "$R/measure/$n"/summary-*.txt "$R/measure/${n}b"/summary-*.txt || die "bound runs $n, ${n}b: not all MATCH"
        for r in query hono Q-E1 Q-E2 Q-E3 Q-E4 Q-E5 H-E1; do
          cmp -s "$R/measure/$n/$r.out" "$R/measure/${n}b/$r.out" || die "$r differs between $n and ${n}b"
        done
        jq -n --arg fp "$fp" --arg c "$commit" --arg a "$n" --arg b "${n}b" --arg m "$R/measure" \
          '{sourceFingerprint: $fp, commit: $c, rounds: [$a, $b], manifests: [$m + "/" + $a + "/manifest.json", $m + "/" + $b + "/manifest.json"]}' > "$C/bound.json"
      fi
    fi
  fi

  # Remote runs: pin, scripts and bins go to the host first, once. A testbox has the pin from tbx_up and the
  # scripts from its sync.
  remote_sync() {
    [[ $host != local && $synced == 0 ]] || return 0
    synced=1
    if ((tbx)); then tbx_up "$wt" "$pin"; tbx_have "$B"; return 0; fi
    [[ -z $pin ]] || run_sh "scripts/goport/remote.sh sync-pin $host $pin >> $C/sync.log 2>&1"
    run_sh "scripts/goport/remote.sh sync-scripts $host >> $C/sync.log 2>&1"
    run_sh "scripts/goport/remote.sh sync-bins $host $B >> $C/sync.log 2>&1"
  }
  # on_host <GOPORT_PIN value> <command text> <log>: runs the command in the repo root, on zbook, under the host
  # lock or on the testbox.
  on_host() {
    if [[ $host == local ]]; then run_sh "export GOPORT_PIN=$1; ($2) < /dev/null > $3 2>&1"
    elif ((tbx)); then tbx_job "$3" "export GOPORT_PIN=$1; ($2)"
    else run_sh "flock $lock env GOPORT_PIN=$1 scripts/goport/remote.sh run $host $(printf %q "$2") < /dev/null > $3 2>&1"; fi
  }
  # host_push <path>, host_fetch <dir>: copies to the host, and a result dir back to zbook.
  host_push() { if ((tbx)); then run_sh "$TB push $1 >> $C/sync.log 2>&1"; else run_sh "scripts/goport/remote.sh push $host $1 >> $C/sync.log 2>&1"; fi; }
  host_fetch() { if ((tbx)); then tbx_get "$1" "$1"; else run_sh "scripts/goport/remote.sh fetch $host $1 >> $C/sync.log 2>&1"; fi; }

  # Gate (full). The record is kept whatever its verdict: gate-compare.py below judges it.
  if [[ -f $C/gate.json ]]; then say "reuse gate $(jq -r '"\(.label) \(.verdict)"' "$C/gate.json")"; else
    gl=$(free_name "$R/compat/gate" "$label-full") rc=0
    remote_sync
    say "$(date -u +%FT%TZ) gate $gl on $host"
    on_host "$pin" "bash scripts/goport/gate.sh $gl --full --bins $B --commit $commit" "$C/gate-$gl.log" || rc=$?
    [[ $host == local ]] || host_fetch "$R/compat/gate/$gl"
    if [[ $DRY == 0 ]]; then
      tail -n 3 "$C/gate-$gl.log"
      [[ -f $R/compat/gate/$gl/manifest.json ]] || die "gate $gl rc $rc and no manifest (log $C/gate-$gl.log)"
      jq --arg l "$gl" --arg m "$R/compat/gate/$gl/manifest.json" --arg s "$(sha "$R/compat/gate/$gl/manifest.json")" --arg h "$host" --argjson rc "$rc" \
        '{label: $l, manifest: $m, sha256: $s, host: $h, exit: $rc, verdict, commit, upstreamPin,
          counts: ([.results[].status] | group_by(.) | map({(.[0]): length}) | add),
          failing: [.results[] | select(.status == "FAIL") | {id, detail}]}' \
        "$R/compat/gate/$gl/manifest.json" > "$C/gate.json"
    fi
  fi

  # Gate compare: each item against the base gate manifest (a MATCH stays MATCH, no new FAIL, no removed id,
  # the open editor long-growth items under their fixed caps). A failed run is kept under its label.
  # A compare with an older base moves to gate-compare-base-<base label>.json, one with another gate id map than the
  # batch's (side_state; R223 reused int90's compare with batch 103's map) to gate-compare-idmap-<sha256 prefix>.json,
  # and the run is compared again. gate-compare.py reads the batch from side_state.
  local gim cim=none
  gim=$(side_state | jq -r '.batch.gateIdMap.sha256 // "none"')
  [[ ! -f $C/gate-compare.json ]] || cim=$(jq -r '.idMap.sha256 // "none"' "$C/gate-compare.json")
  if [[ -f $C/gate-compare.json && $(jq -r .base.sha256 "$C/gate-compare.json") == "$bgs" && $cim == "$gim" ]]; then
    say "reuse gate compare $(jq -c .counts "$C/gate-compare.json")"
  else
    if [[ -f $C/gate-compare.json && $(jq -r .base.sha256 "$C/gate-compare.json") != "$bgs" ]]; then
      old=$(jq -r .base.label "$C/gate-compare.json")
      say "gate compare: the cached compare used base $old; keep it as gate-compare-base-$old.json and compare again"
      run mv "$C/gate-compare.json" "$C/gate-compare-base-$old.json"
    elif [[ -f $C/gate-compare.json ]]; then
      say "gate compare: the cached compare used gate id map ${cim:0:12}, the batch has ${gim:0:12}; keep it as gate-compare-idmap-${cim:0:12}.json and compare again"
      run mv "$C/gate-compare.json" "$C/gate-compare-idmap-${cim:0:12}.json"
    fi
    say "gate compare with the base"
    rc=0
    run_sh "node scripts/state.mjs export | jq -c '$SIDE_STATE' | python3 $G/gate-compare.py $bg \$(jq -r .manifest $C/gate.json) --state /dev/stdin --out $C/gate-compare.json > /dev/null" || rc=$?
    if [[ $DRY == 0 ]] && ((rc)); then
      # Exit 2 is bad input, not a failed run: nothing moves, and the next run compares the same gate again.
      [[ $rc == 1 && -f $C/gate-compare.json ]] || { rm -f "$C/gate-compare.json"; die "gate-compare.py rc $rc (bad input)"; }
      gl=$(jq -r .label "$C/gate.json")
      fails+="${fails:+; }gate $gl compare rc $rc (kept as gate-fail-$gl.json): $(jq -r '[.regressions[] | "\(.id) (\(.why))"] | .[:10] | join(", ")' "$C/gate-compare.json")"
      if [[ -e $C/gate-fail-$gl.json ]]; then
        # The same run compared again (a copy of gate-fail-$gl.json): that run and its first compare stay kept.
        cmp -s "$C/gate.json" "$C/gate-fail-$gl.json" || die "$C/gate.json is not the kept run gate-fail-$gl.json"
        rm -f "$C/gate.json" "$C/gate-compare.json"
      else
        mv "$C/gate-compare.json" "$C/gate-compare-fail-$gl.json"
        mv "$C/gate.json" "$C/gate-fail-$gl.json"
      fi
    fi
  fi

  # Editor items of two host kinds (tbxside1 skeptic: peak RSS 2 to 4 percent higher on a testbox). The gate judges
  # each editor item against Go in the same run on the same host, and no rule judges peak RSS. The one value that
  # gate-compare.py compares across runs is the Rust growth of editor/<project>/long against its cap, and on the R190
  # bins it was the same on a testbox and on dbook-lan (within 0.05 MiB/edit, as two runs on one kind). So a base and
  # a candidate of two host kinds count only while each long growth is at least 0.15 MiB/edit under its cap. Else
  # move gate.json and gate-compare.json aside and run the gate again on the base's kind. Checked on every run.
  if [[ $DRY == 0 && -f $C/gate.json && -f $C/gate-compare.json ]]; then
    local eh erc=0
    eh=$(python3 - "$bg" "$(jq -r .manifest "$C/gate.json")" "$C/gate-compare.json" <<'PY'
import json, os, re, sys
def run(manifest):
    try:
        return json.load(open(os.path.join(os.path.dirname(manifest), 'runs', 'editor', 'result.json')))
    except (OSError, ValueError):
        return {}
# A Blacksmith testbox has an EC2 hostname, ip-<a>-<b>-<c>-<d>; zbook and the SSH hosts have their own names.
kind = lambda r: 'testbox' if re.fullmatch(r'ip(-[0-9]+){4}', r.get('host') or '') else 'ssh'
long_growth = lambda r: {s.get('project'): (((s.get('rust') or {}).get('cand') or {}).get('summary') or {}).get('growthMiBPerEdit')
                         for s in r.get('sessions') or [] if s.get('scenario') == 'long'}
b, n = run(sys.argv[1]), run(sys.argv[2])
caps = json.load(open(sys.argv[3])).get('longCaps') or {}
near = [f"{p} {side} {g:.2f} (cap {c['cap']:.2f})" for p, c in caps.items()
        for side, g in (('base', long_growth(b).get(p)), ('new', long_growth(n).get(p))) if g is not None and g > c['cap'] - 0.15]
print(f"editor hosts: base {b.get('host')} ({kind(b)}), new {n.get('host')} ({kind(n)})"
      + (f"; long growth within 0.15 of the cap: {', '.join(near)}" if near else ''))
sys.exit(3 if near and kind(b) != kind(n) else 0)
PY
    ) || erc=$?
    echo "   $eh"
    ((erc == 0 || erc == 3)) || die "editor host check rc $erc"
    ((erc == 0)) || fails+="${fails:+; }gate: $eh, on two host kinds (run the gate again on the base's kind)"
  fi

  # oracle_json <kind> <out-root> <label> <base results dirs> <record>: compares the results of <label> with the
  # base per request (oracle-compare.py with OARGS, see oracle_setup) and writes <record>.json, or
  # <record>-fail.json on a loss, a parity problem, a rebase run that is not the base bins at the batch pin, or
  # without a base; the other one goes, as it is not the last compare (a record of another OKEY moves aside first,
  # oracle_aside). Exit 2 (bad input or a refused answer set) stops the side run. The finished run's label stays in
  # <record>-run, so a rerun only compares again.
  oracle_json() {
    local kind=$1 root=$2 lab=$3 base=$4 rec=$5 rc=0 prc=0 out rhost
    # A new run records its host next to its label; a reused run keeps the host it ran on (none before --host testbox).
    if [[ $DRY == 0 && ! -f $C/$rec-run ]]; then echo "$lab" > "$C/$rec-run"; echo "$host" > "$C/$rec-run.host"; fi
    rhost=$(cat "$C/$rec-run.host" 2> /dev/null || echo unrecorded)
    if [[ -z $base ]]; then
      # The other steps still run. The first goport batch needs the rule's apiBaseline (an API run of the R131 bins).
      echo "   no base $kind results (open_revision.py --base .$rec.dir); $lab is kept, a rerun compares it"
      fails+="${fails:+; }$kind oracle: no base results ($lab ran; a rerun compares it once the base exists)"
      return 0
    fi
    oracle_aside "$rec"
    say "$(date -u +%FT%TZ) $kind compare with the base $base${OARGS[$rec]:+ (${OARGS[$rec]% })}"
    run_sh "python3 $G/oracle-compare.py $base $root/results/$lab ${OARGS[$rec]}--out $C/$rec-compare.new.json > /dev/null" || rc=$?
    [[ $DRY == 0 ]] || return 0
    ((rc < 2)) && [[ -f $C/$rec-compare.new.json ]] || die "$kind compare rc $rc: bad input or a refused answer set (see this log)"
    mv "$C/$rec-compare.new.json" "$C/$rec-compare.json"
    out=$C/$rec.json
    ((rc == 0)) || { out=$C/$rec-fail.json; fails+="${fails:+; }$kind oracle: oracle-compare.py rc $rc ($C/$rec-compare.json)"; }
    python3 - "$kind" "$root/results/$lab" "$lab" "$rhost" "${OKEY[$rec]}" "$C/$rec-compare.json" "${OREBASE[$rec]:-null}" "$bg" > "$C/$rec.new.json" <<'PY' || prc=$?
import collections, json, sys
kind, rdir, label, host, key, cmp_path, rebase, gate = sys.argv[1:]
s, c, rebase = json.load(open(f'{rdir}/summary.json')), json.load(open(cmp_path)), json.loads(rebase)
# With batch.oracleRebase (ruling 10 condition 1): each base run has its resultsSha256, the base gate's tsgo and the
# batch pin's oracle, and the new run that oracle. The check and accept_revision.py refuse the same.
problems, heads = [], c.get('bases') or [c['base']]
if rebase:
    tsgo = ((json.load(open(gate)).get('binaries') or {}).get('tsgo') or {}).get('sha256')
    if rebase.get('binsSha256') != tsgo:
        problems.append(f"binsSha256 {rebase.get('binsSha256')} is not the base gate tsgo {tsgo}")
    for run, h in zip(rebase['runs'], heads):
        if h.get('resultsSha256') != run.get('resultsSha256'):
            problems.append(f"{run['dir']} has resultsSha256 {h.get('resultsSha256')}, batch.oracleRebase says {run.get('resultsSha256')}")
        if h.get('goportSha256') != [rebase.get('binsSha256')] or h.get('oracleSha256') != [rebase.get('oracleSha256')]:
            problems.append(f"{run['dir']} ran tsgo {h.get('goportSha256')} with the oracle {h.get('oracleSha256')}")
    if c['new'].get('oracleSha256') != [rebase.get('oracleSha256')]:
        problems.append(f"the new run used the oracle {c['new'].get('oracleSha256')}")
classes = collections.Counter()
if kind == 'LSP':
    for x in s['batteries'].values():
        classes.update(x['classes'])
    requests = sum(x['requests'] for x in s['batteries'].values())
    crash = classes['crash'] + sum(x['crashExits'] for x in s['batteries'].values())
else:
    classes.update(s['total'])
    requests, crash = sum(classes.values()), classes['crash']
print(json.dumps({'label': label, 'host': host, 'resultsDir': rdir, 'summary': f'{rdir}/summary.md', 'requests': requests,
                  'same': classes['same'], 'diff': classes['diff'], 'goportError': classes['goport_error'], 'crash': crash,
                  'timeout': classes['timeout'], 'classes': dict(classes), 'base': {'label': heads[0]['label'], 'dir': heads[0]['dir']},
                  'bases': [{'label': h['label'], 'dir': h['dir']} for h in heads], 'compareKey': key, 'compareOutput': cmp_path,
                  'compare': c['total'], 'answers': sorted(a['sha256'] for a in c.get('answers') or []),
                  **({'parity': {k: v for k, v in c['parity'].items() if k != 'badFirst'}} if c.get('parity') else {}),
                  **({'rebaseProblems': problems} if problems else {})}))
sys.exit(3 if problems else 0)
PY
    if ((prc == 3)); then out=$C/$rec-fail.json; fails+="${fails:+; }$kind oracle: the rebase runs are not the base bins at the batch pin ($C/$rec-fail.json)"
    elif ((prc)); then die "$kind oracle record rc $prc"; fi
    mv "$C/$rec.new.json" "$out"
    if [[ $out == "$C/$rec.json" ]]; then rm -f "$C/$rec-fail.json"; else rm -f "$C/$rec.json"; fi
  }
  # oracle_done <record>: true when <record>.json is a passing compare with the current OKEY (an older record without
  # compareKey was a compare with its base dir alone).
  oracle_done() {
    [[ -f $C/$1.json ]] && jq -e .compare "$C/$1.json" > /dev/null && [[ $(jq -r '.compareKey // .base.dir' "$C/$1.json") == "${OKEY[$1]}" ]]
  }
  # oracle_aside <record>: a <record>.json or <record>-fail.json of another OKEY (an older batch's rebase record or
  # answer sets: int90 under accepted batch 103) moves to <record>[-fail]-key-<sha256 prefix of its key>.json with
  # <record>-compare.json, so the next compare does not replace or remove it.
  oracle_aside() {
    local f k
    for f in "$C/$1.json" "$C/$1-fail.json"; do
      [[ -f $f ]] && k=$(jq -r '.compareKey // .base.dir' "$f") && [[ $k != "${OKEY[$1]}" ]] || continue
      k=$(printf %s "$k" | sha256sum | cut -c1-12)
      say "$1 compare: ${f##*/} used another base, answer sets or rebase record; keep it as $(basename "$f" .json)-key-$k.json and compare again"
      run mv "$f" "${f%.json}-key-$k.json"
      [[ ! -f $C/$1-compare.json ]] || run mv "$C/$1-compare.json" "$C/$1-compare-key-$k.json"
    done
  }
  if [[ -n $base ]]; then
    oracle_setup "$base"
    local la=${OARGS[lsp]% } aa=${OARGS[api]% }
    say "oracle bases: LSP ${OBASE[lsp]:-none}, API ${OBASE[api]:-none}; extra compare arguments: LSP ${la:-none}; API ${aa:-none}"
  else
    OBASE=([lsp]="$bl" [api]="$ba")
  fi

  # LSP oracle (the batteries of the R121 to R125 side scripts).
  if oracle_done lsp; then say "reuse LSP oracle $(jq -r .label "$C/lsp.json")"
  elif [[ -f $C/lsp-run ]]; then say "reuse LSP run $(cat "$C/lsp-run")"; oracle_json LSP "$R/ls-oracle/battery" "$(cat "$C/lsp-run")" "${OBASE[lsp]}" lsp
  else
    ll=$(free_name "$R/ls-oracle/battery/results" "lsp-$label") rc=0
    oracle=$(python3 scripts/upstream/pin.py path oracle ${pin:+"$pin"})
    remote_sync
    # The goldens of this oracle (golden/<oracle sha256 prefix>) are not in every host mirror (R131: cup2).
    [[ $host == local ]] || host_push "$R/ls-oracle/battery/golden/$(sha256sum "$oracle" | cut -c1-12)"
    say "$(date -u +%FT%TZ) LSP oracle $ll on $host"
    l="python3 scripts/goport/lsp_oracle.py check --out-root $R/ls-oracle/battery"
    l+=" --battery $LSP_BATTERIES --goport $B/tsgo --oracle $oracle --label $ll --jobs 12"
    on_host "" "$l" "$C/lsp-$ll.log" || rc=$?  # the oracle path carries the pin
    [[ $host == local ]] || host_fetch "$R/ls-oracle/battery/results/$ll"
    [[ $DRY == 1 || $rc == 0 ]] || die "LSP oracle $ll rc $rc (log $C/lsp-$ll.log)"
    oracle_json LSP "$R/ls-oracle/battery" "$ll" "${OBASE[lsp]}" lsp
  fi

  # API oracle (api_oracle.py, the batteries of API_BATTERIES, and API_EXT_BATTERIES unless the pin's oracle is
  # in API_NO_EXT_ORACLES): one check per battery. GOPORT_PIN selects the pin's traces (a pin cache); the goldens
  # are golden/<oracle sha256 prefix>.
  if oracle_done api; then say "reuse API oracle $(jq -r .label "$C/api.json")"
  elif [[ -f $C/api-run ]]; then say "reuse API run $(cat "$C/api-run")"; oracle_json API "$R/tests2/api" "$(cat "$C/api-run")" "${OBASE[api]}" api
  else
    al=$(free_name "$R/tests2/api/results" "api-$label") rc=0
    oracle=$(python3 scripts/upstream/pin.py path oracle ${pin:+"$pin"})
    remote_sync
    if [[ $host != local ]]; then
      host_push "$R/tests2/api/golden/$(sha256sum "$oracle" | cut -c1-12)"
      [[ -n $pin ]] || host_push "$R/tests2/api/traces"
    fi
    local -a batteries=("${API_BATTERIES[@]}")
    [[ " ${API_NO_EXT_ORACLES[*]} " == *" $(sha256sum "$oracle" | cut -c1-12) "* ]] || batteries+=("${API_EXT_BATTERIES[@]}")
    say "$(date -u +%FT%TZ) API oracle $al on $host: ${#batteries[@]} batteries"
    l="set -e; for b in ${batteries[*]}; do python3 scripts/goport/api_oracle.py check --battery \$b"
    l+=" --goport $B/tsgo --oracle $oracle --label $al --jobs 12; done"
    on_host "$pin" "$l" "$C/api-$al.log" || rc=$?
    [[ $host == local ]] || host_fetch "$R/tests2/api/results/$al"
    [[ $DRY == 1 || $rc == 0 ]] || die "API oracle $al rc $rc (log $C/api-$al.log)"
    oracle_json API "$R/tests2/api" "$al" "${OBASE[api]}" api
  fi

  # Quality on the checkout: rustfmt and clippy (R114 and R124 were refused for quality alone).
  # Fingerprint before and after. The record is reused only for the same source fingerprint (fp.py of the whole
  # checkout): an integration's record names its worktree, and the revision checkout has the same crates tree (the
  # same key) but another fingerprint, which accept_revision.py refuses (R195, state note
  # candidate-quality-reuse-defect-2026-10-11). A record of another fingerprint stays as quality.json.fp-<fp>, with
  # its logs. A record without .crates predates the kept crate check (R131 and older), so it runs again.
  # Each checkout has its own clippy target, reused by its next runs and removed with its worktree. One shared
  # dir let clippy check another checkout's goport_util (R190 side run 1, state note
  # clippy-shared-target-2026-10-11), for the reason that the release build above names.
  local clippy_target=$wt/target/candidate-clippy fpq
  fpq=$(python3 "$G/fp.py" "$wt" | cut -d' ' -f1)
  if [[ -f $C/quality.json ]] && jq -e .crates "$C/quality.json" > /dev/null &&
    [[ $(jq -r .sourceFingerprint "$C/quality.json") == "$fpq" ]]; then say "reuse quality $(jq -c . "$C/quality.json")"; else
    if [[ -f $C/quality.json ]]; then
      old=$(jq -r '.sourceFingerprint // "none"' "$C/quality.json" | cut -c1-12)
      say "quality: the cached record is of source $old, not ${fpq:0:12}; keep it as quality.json.fp-$old and run again"
      for g in quality.json clippy.log rustfmt.log; do
        [[ ! -f $C/$g ]] || run mv "$C/$g" "$C/$(free_name "$C" "$g.fp-$old")"
      done
    fi
    say "$(date -u +%FT%TZ) quality: rustfmt and clippy$on (clippy target $clippy_target)"
    if [[ $DRY == 0 ]]; then
      local fpa fpb fmt=0 cl=0 warn
      fpb=$fpq
      # After split steps 1 (R131) and 2, goport_core, goport_util and goport_lsproto are workspace crates built
      # from ts_goport files. clippy lints only the packages it names, so each one is named.
      # The kept crates that are still workspace members (stages 5 and 6 move some into goport) count too.
      local pkgs=(-p ts_goport) dirs=(crates/ts_goport) p
      for p in goport_core goport_util goport_lsproto; do grep -q "\"crates/ts_goport/parts/$p\"" "$wt/Cargo.toml" && pkgs+=(-p "$p"); done
      for p in "${KEPT_CRATES[@]}"; do grep -q "\"crates/$p\"" "$wt/Cargo.toml" && pkgs+=(-p "$p") dirs+=("crates/$p"); done
      (cd "$wt" && rustfmt --edition 2024 --check $(git ls-files "${dirs[@]/%//**/*.rs}") > "$C/rustfmt.log" 2>&1) || fmt=$?
      if ((tbx)); then
        # clippy on the testbox copy of the checkout (the same fp.py), in the same target path there (32 jobs: the
        # default off zbook is 1).
        tbx_up "$wt" "$pin"
        tbx_job "$C/clippy.log" "cd $wt && CARGO_TERM_COLOR=never TS_CARGO_LOCK_ID=candidate-side TS_CARGO_JOBS=32 TS_CARGO_SEPARATE_TARGET=1 CARGO_TARGET_DIR=$clippy_target $ROOT/scripts/run-cargo-capped.sh clippy --locked ${pkgs[*]} --all-targets" || cl=$?
      else
        (cd "$wt" && TS_CARGO_LOCK_ID=candidate-side TS_CARGO_SEPARATE_TARGET=1 CARGO_TARGET_DIR=$clippy_target \
          "$ROOT/scripts/run-cargo-capped.sh" clippy --locked "${pkgs[@]}" --all-targets > "$C/clippy.log" 2>&1) || cl=$?
      fi
      fpa=$(python3 "$G/fp.py" "$wt" | cut -d' ' -f1)
      local kept plain
      # Count on the log without ANSI color codes: the testbox sets CARGO_TERM_COLOR=always, and a colored
      # "warning" line matched neither grep, so warnings counted 0 (R197 reviewer).
      plain=$(sed -E 's/\x1b\[[0-9;]*[A-Za-z]//g' "$C/clippy.log")
      warn=$(grep -E '^(warning|error)' -A4 <<< "$plain" | grep -c -- '--> crates/ts_goport' || true)
      kept=$(grep -E '^(warning|error)' -A4 <<< "$plain" | grep -cE -- "--> crates/($(IFS='|'; echo "${KEPT_CRATES[*]}"))/" || true)
      jq -n --arg fp "$fpa" --argjson fmt "$fmt" --argjson cl "$cl" --argjson w "$warn" --argjson k "$kept" \
        --argjson same "$([[ $fpa == "$fpb" ]] && echo true || echo false)" --arg dirs "${dirs[*]}" \
        '{sourceFingerprint: $fp, rustfmtExit: $fmt, clippyExit: $cl, tsGoportWarnings: $w, keptCrateWarnings: $k,
          fingerprintUnchanged: $same, crates: ($dirs | split(" "))}' > "$C/quality.json"
      [[ $fmt == 0 && $cl == 0 && $warn == 0 && $kept == 0 && $fpa == "$fpb" ]] ||
        fails+="${fails:+; }quality: rustfmt $fmt, clippy $cl, warnings $warn ts_goport, $kept kept crates"
    fi
  fi

  say "$(date -u +%FT%TZ) evidence $C"
  for g in "$C"/{tests,tests-fail,bound,gate,gate-compare,lsp,lsp-fail,api,api-fail,quality}.json "$C"/gate-fail-*.json "$C"/gate-compare-fail-*.json; do
    if [[ -f $g ]]; then echo "   $(basename "$g" .json): $(jq -c 'del(.manifests, .failing, .knownOpen, .longCaps, .reallowed, .newAllowEntries, .regressions, .newIds, .fixed, .base, .new, .classes)' "$g")"; fi
  done
  [[ -z $fails ]] || die "$fails"
}

# cmd_verdict <rev>: prints the verdict request texts from the state and the evidence cache, then the
# accept command. Exits 1 after printing when evidence is missing or failed.
cmd_verdict() {
  local rev=$1 wt key
  [[ $(st .batch.recoveryRevision) == "$rev" ]] || die "the batch is at R$(st .batch.recoveryRevision), not R$rev"
  [[ $(st .batch.protectedSet) == goport ]] || die "batch $(st .batch.id) is not a goport batch (protectedSet goport)"
  wt=$ROOT/$(st .batch.checkout)
  key=$(evidence_key "$wt" "$(st '.batch.upstreamPin.to // empty')")
  python3 - "$STATE" "$rev" "$EVIDENCE/$key" "$G" <<'PY'
import json, os, shlex, subprocess, sys
ROOT = '/home/theo/Code/sandbox/ts-rust'
s, rev, cache = json.load(open(sys.argv[1])), int(sys.argv[2]), sys.argv[3]
sys.path.insert(0, sys.argv[4])
from accept_revision import failed_gate_runs  # the same flake-note rule that the accept step applies
b = s['batch']
row, rows = b['recoveryHistory'][-1], b['recoveryHistory'][:-1]
wt = os.path.join(ROOT, b['checkout'])
git = lambda *a: subprocess.check_output(['git', '-C', wt, *a], text=True).strip()
load = lambda k: json.load(open(f'{cache}/{k}.json')) if os.path.exists(f'{cache}/{k}.json') else None
rel = lambda p: p.replace(ROOT + '/', '')
missing, ev = [], []
base = b['protectedBase']
head_commit = git('rev-parse', 'HEAD')
other = lambda c: '' if head_commit.startswith(c[:9]) else f' (run on commit {c[:9]} with the same crates tree)'

tests, tfail = load('tests'), load('tests-fail')
t = tests or tfail
if t:
    c = t['compare']
    ev.append(f"goport tests{' FAILED' if tfail and not tests else ''}: results {rel(t['results'])} sha256 {t['sha256']} "
              f"(test bins {rel(t['testbin'])}{other(t['commit'])}) against base {base['batch']} R{base['revision']} "
              f"{base['tests']['path']} sha256 {base['tests']['sha256']}: retained {c['retained']:,}, recovered {c['recovered']}, "
              f"lost {c['lost']}, absent {c['absent']}, unrun {c['unrun']}, new names {c['newNames']} ({c['newFailed']} failed), "
              f"removed by map {c['removedByMap']}; name map {t['nameMap']['path'] + ' sha256 ' + t['nameMap']['sha256'] if t['nameMap'] else 'none'}; "
              f"incomplete suites: {', '.join(t['incomplete'] or []) or 'none'}. Per-name compare {rel(t['compareOutput'])}.")
    if not tests:
        total = json.load(open(t['compareOutput']))['total']
        ev.append('goport test losses: ' + '; '.join(f'{k} {x}' for k in ('lost', 'absent', 'unrun') for x in total[k][:20]) + '.')
        missing.append('goport tests without a loss')
else:
    missing.append('goport tests (candidate.sh side)')
bound = load('bound')
if bound:
    ev.append(f"Bound runs {' and '.join(bound['rounds'])} ({', '.join(map(rel, bound['manifests']))}): all MATCH, the two runs identical; "
              f"bins {rel(cache)}/bins (bins.sha256 there, release profile).")
else:
    missing.append('bound runs (candidate.sh side)')
gate = load('gate')
if gate:
    ev.append(f"Gate {gate['label']} ({gate['host']}{', pin ' + gate['upstreamPin'] if gate.get('upstreamPin') else ''}), manifest {rel(gate['manifest'])} "
              f"sha256 {gate['sha256']}: {gate['verdict']}, " + ', '.join(f'{gate["counts"][k]:,} {k}' for k in ('MATCH', 'ALLOWED', 'FAIL') if k in gate['counts'])
              + f"{other(gate['commit'])}." + (' FAIL items: ' + '; '.join(f"{f['id']}: {f['detail']}" for f in gate['failing']) + '.' if gate['failing'] else ''))
else:
    missing.append('gate (candidate.sh side)')
gc = load('gate-compare')
if gc:
    n = gc['counts']
    ev.append(f"Gate compare with base {base['batch']} R{base['revision']} {base['gate']['path']} ({gc['base']['label']}): "
              f"{n['regressions']} regressions, {n['knownOpen']} open-defect items under their caps, {n['fixed']} fixed; "
              f"pin changed {gc['pinChanged']}, allow list changed {gc['allowListChanged']}, new allow entries {len(gc['newAllowEntries'])}, "
              f"tool changes {[(t['key'], t['listed']) for t in gc.get('toolChanges', [])]}. "
              f"Cap rule: {gc['capRule']}. Caps now: " + ', '.join(f"{p} {c['cap']:.2f}{' (lowered)' if c['lowered'] else ''}" for p, c in gc['longCaps'].items())
              + '.' + (' Open-defect items: ' + '; '.join(f"{k['id']} growth {k['growth']:.2f} (cap {k['cap']:.2f}, base {k['baseGrowth']:.2f})" for k in gc['knownOpen']) + '.' if gc['knownOpen'] else ''))
    m = gc.get('idMap')
    if m:
        ev.append(f"Gate id map {m['path']} sha256 {m['sha256']}: {m['lines']} lines, applied {m['applied']}, {m['mapped']} ids moved, "
                  f"{len(m['broken'])} broken lines, {len(m['unused'])} unused lines. Check that each line is one case at both pins.")
    if gc['regressions']:
        ev.append('Gate regressions: ' + '; '.join(f"{r['id']} {r['base']} -> {r['new']} ({r['why']})" for r in gc['regressions'][:20]) + '.')
        missing.append('gate compare without a regression')
else:
    missing.append('gate compare (candidate.sh side)')
# Every failed gate run of this source (repeat-run rule): each regressed item needs a flake note.
fruns = failed_gate_runs(s, rev, cache, gate and gate['label'])
if fruns:
    ev.append(f"Failed gate runs of this source ({len(fruns)}; a loss in any run counts unless the reviewer accepts its flake note "
              f"flake-r{rev}-<name>, which names the item and the run): " + ' '.join(
                  f"{r['label']} (manifest {rel(r['manifest'])} sha256 {r['sha256']}, compare {rel(r['compare'])}): " + '; '.join(
                      f"{x['id']} {x['base']} -> {x['new']} ({x['why']}), flake note {x['flake'] or 'MISSING'}" for x in r['regressions'][:20]) + '.'
                  for r in fruns))
    nf = [f"{r['label']} {x['id']}" for r in fruns for x in r['regressions'] if not x['flake']]
    if nf:
        missing.append('a flake note for each item of a failed gate run: ' + ', '.join(nf[:10]))
else:
    ev.append('Failed gate runs of this source: none.')
# The oracle base (batch.oracleRebase runs at a pin bump, else protectedBase) and the answer sets of the base and the
# batch: each compare must have used them (accept_revision.py and the check refuse otherwise).
rebase = b.get('oracleRebase')
sets = {(r['kind'], r['sha256']): r for r in (base.get('oracleAnswers') or []) + (b.get('oracleAnswers') or [])}
# The answer sets that accept_revision.py keeps in batch.oracleAnswers: the base's at the batch pin and the batch's own.
pin_to = ((b.get('upstreamPin') or {}).get('to') or '').lower()
bound_sets = [r for r in base.get('oracleAnswers') or [] if pin_to and (r['pin'].startswith(pin_to) or pin_to.startswith(r['pin']))] \
    + (b.get('oracleAnswers') or [])
canon_sha = lambda v: __import__('hashlib').sha256(json.dumps(v, sort_keys=True, separators=(',', ':'), ensure_ascii=False).encode()).hexdigest()
if rebase:
    ev.append(f"Oracle rebase (reviewer ruling 10), batch.oracleRebase sha256 {canon_sha(rebase)}: the base is the base batch's bins "
              f"(tsgo {rebase['lsp']['binsSha256']}) measured again with the batch pin's oracle ({rebase['lsp']['oracleSha256'][:12]}): "
              + '; '.join(f"{k.upper()} runs " + ', '.join(f"{r['label']} ({rel(r['dir'])}, resultsSha256 {r['resultsSha256']})" for r in rebase[k]['runs'])
                          for k in ('lsp', 'api'))
              + '. API known diffs: ' + ('; '.join(f"{d['key']}: {d['reason']}" for d in rebase['api'].get('knownDiffs') or []) or 'none')
              + '. Check the runs against the base batch record and each known diff against Go at the batch pin.')
if sets:
    ev.append('Oracle answer sets (protectedBase.oracleAnswers and batch.oracleAnswers): ' + '; '.join(
        f"{r['kind']} {rel(r['path'])} sha256 {r['sha256']} (pin {r['pin']})" for r in sorted(sets.values(), key=lambda r: (r['kind'], r['path']))) + '.')
for name, k in (('LSP', 'lsp'), ('API', 'api')):
    x = load(k) or load(f'{k}-fail')
    if not x:
        missing.append(f'{name} oracle compared with the base (candidate.sh side)')
        continue
    c = x['compare']
    want = [rel(r['dir']) for r in rebase[k]['runs']] if rebase else [rel((base.get(k) or {}).get('dir') or '')]
    got = [rel(r['dir']) for r in x.get('bases') or [x['base']]]
    ev.append(f"{name} oracle {x['label']}{' FAILED' if not load(k) else ''} ({rel(x['summary'])}, {x['host']}): {x['requests']:,} requests, "
              f"{x['same']:,} same, {x['diff']} diff, {x['goportError']} goport_error, {x['crash']} crash, {x['timeout']} timeout. "
              f"Per request against base {', '.join(r['label'] for r in x.get('bases') or [x['base']])} ({', '.join(got)}): "
              f"retained {c['retained']:,}, recovered {c['recovered']}, lost {c['lost']}, unrun {c['unrun']}, absent {c['absent']}, "
              f"new {c['newRequests']}" + (f", retained by answer sets {c['retainedByAnswers']}" if 'retainedByAnswers' in c else '')
              + f" ({rel(x['compareOutput'])}).")
    par = x.get('parity')
    if par:
        ev.append(f"{name} parity at the batch pin (ruling 10 condition 3): {par['bad']} problems, {par['answerRequests']} answer set requests "
                  f"({par['allowedByAnswers']} allowed by their set), {par['knownDiffsUsed']} of {par['knownDiffs']} known diffs, "
                  f"crash exits {par['crashExits']}.")
    for problem in x.get('rebaseProblems') or []:
        ev.append(f"{name} rebase problem: {problem}.")
    if [os.path.realpath(d) for d in got] != [os.path.realpath(d) for d in want] or bool(par) != bool(rebase) \
            or x.get('answers', []) != sorted(sh for kind, sh in sets if kind == k):
        missing.append(f"an {name} compare with the base runs {', '.join(want)}, the answer sets of the base and the batch"
                       f"{' and parity' if rebase else ''} (run side again with a new label)")
    if not load(k):
        first = json.load(open(x['compareOutput']))['lostFirst'][:10]
        ev.append(f'{name} losses: ' + '; '.join(f"{f['battery']}/{f['trace']}#{f['event']} {f['method']} {f['base']} -> {f['new']}" for f in first) + '.')
        missing.append(f'{name} oracle without a loss')
quality = load('quality')
if quality:
    ev.append(f"Quality on the source ({', '.join(quality.get('crates') or ['crates/ts_goport'])}): rustfmt exit {quality['rustfmtExit']}, "
              f"clippy exit {quality['clippyExit']}, {quality['tsGoportWarnings']} ts_goport warnings, "
              f"{quality.get('keptCrateWarnings', 0)} kept crate warnings, fingerprint unchanged {quality['fingerprintUnchanged']}.")
else:
    missing.append('quality: rustfmt and clippy (candidate.sh side)')
pin = (b.get('upstreamPin') or {}).get('to')
head = (f"Batch {b['id']} (protected set goport), revision {rev}, source fingerprint {b['sourceFingerprint']} "
        f"({b['checkout']} commit {b['commit']}, crates tree {git('rev-parse', 'HEAD:crates')[:12]})."
        + (f' Go pin {pin} (GOPORT_PIN for every Go comparison).' if pin else ''))
evidence = '\n'.join(f'- {e}' for e in ev)
bind = (f"naming the batch, the source fingerprint, the goport tests sha256 {t['sha256'] if t else '<missing>'}, the gate manifest "
        f"sha256 {gate['sha256'] if gate else '<missing>'}, the name map sha256 {((t or {}).get('nameMap') or {}).get('sha256', 'none')}, "
        f"the gate id map sha256 {(b.get('gateIdMap') or {}).get('sha256', 'none')}, the oracle rebase sha256 "
        f"{canon_sha(rebase) if rebase else 'none'} and the oracle answer set sha256 values "
        f"{', '.join(sorted({r['sha256'] for r in bound_sets})) or 'none'}")
prev = next((r.get('commit') for r in reversed(rows) if r.get('commit')), None)
spec = ['--', '.', ':(exclude)docs/typechecker-state', ':(exclude)docs/typechecker-batches']
diff = git('diff', '--shortstat', prev, 'HEAD', *spec) if prev else 'unknown'
outside = git('diff', '--name-only', prev, 'HEAD', *spec, ':(exclude)crates/ts_goport').split() if prev else []
for role, key in (('audit_accepted_roster', 'auditor'), ('independent_reviewer', 'reviewer')):
    print(f"===== to {b[key]['agent']} ({role})")
    print(f'Root: verdict request for R{rev} (role {role}). {head}\n')
    if key == 'reviewer':
        print(f"Change: {(row.get('change') or '').rstrip('.')} (git diff {prev}..{b['commit']}:{diff}; outside crates/ts_goport: "
              f"{', '.join(outside[:30]) or 'none'}{' and more' if len(outside) > 30 else ''}).")
        print(f"Hypothesis: {row.get('hypothesis')}\n")
    print(f'Evidence:\n{evidence}')
    if key == 'reviewer':
        print('Please review the complete change against pinned Go (callers, caches, recursion, diagnostics, negative cases, '
              'repeat behavior), check that the protected set is complete (every base suite runs, every base name and gate item '
              f'is present or in a checked name map), check the evidence, and reply with VERDICT: PASS or STOP {bind}.\n')
    else:
        print('Please compare each base goport test name and each base gate item with the candidate, check the sources and hashes, '
              f'and reply with VERDICT: PASS or STOP {bind}.\n')
print('===== after two PASS verdicts')
print(f"python3 scripts/goport/accept_revision.py --revision {rev} --evidence {shlex.quote(cache)} --scope '<one line>' --outcome '<one line>'")
if missing:
    sys.exit('MISSING: ' + '; '.join(missing))
PY
}

DRY=0 HYP='' CHANGE='' MESSAGE='' NEW_BATCH='' ORIGIN='' CHECKOUT='' HOST=dbook-lan NAME_MAP='' GATE_HOST=''
args=()
while (($#)); do
  case $1 in
    --dry-run) DRY=1; shift ;;
    -h|--help) usage 0 ;;
    --hypothesis) HYP=${2:?--hypothesis needs a value}; shift 2 ;;
    --change) CHANGE=${2:?--change needs a value}; shift 2 ;;
    --message) MESSAGE=${2:?--message needs a value}; shift 2 ;;
    --new-batch) NEW_BATCH=${2:?--new-batch needs a value}; shift 2 ;;
    --origin) ORIGIN=${2:?--origin needs a value}; shift 2 ;;
    --checkout) CHECKOUT=${2:?--checkout needs a value}; shift 2 ;;
    --name-map) NAME_MAP=$(realpath "${2:?--name-map needs a value}"); [[ -f $NAME_MAP ]] || die "no name map $2"; shift 2 ;;
    # The last --gate-host wins, as before --host existed. --host testbox excludes any --gate-host.
    --gate-host) v=${2:?--gate-host needs a value}; [[ $v != auto ]] || die "--gate-host auto is not supported (sync-pin and fetch need one host); pick a free one with remote.sh status"
      [[ $v != testbox ]] || die "use --host testbox"; [[ $HOST != testbox ]] || die "--gate-host and --host exclude each other"; HOST=$v GATE_HOST=1; shift 2 ;;
    --host) [[ ${2:-} == testbox ]] || die "--host takes testbox (the default runs the gate and oracles on dbook-lan)"
      [[ -z $GATE_HOST ]] || die "--gate-host and --host exclude each other"; HOST=testbox; shift 2 ;;
    --*) die "unknown option $1" ;;
    *) args+=("$1"); shift ;;
  esac
done
set -- "${args[@]}"
cmd=${1:-}
(($#)) && shift
case $cmd:$# in
  help:0) usage 0 ;;
  check:1) load_state; check_branch "$1" ;;
  open:2) load_state; cmd_open "$@" ;;
  side:1) load_state; cmd_side "$1" ;;
  verdict-request:1) load_state; cmd_verdict "$1" ;;
  _side-unit:5) side_unit "$@"; exit 0 ;;
  *) usage ;;
esac
