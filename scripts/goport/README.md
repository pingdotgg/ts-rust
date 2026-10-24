# goport measurement scripts

Moved from `/tmp/port`, where a reboot would erase them. They measure the `ts_goport`
candidate in `target/worktrees/checker-port` against the pinned `tsgo-oracle`.

- `gate.sh <label>`: full regression gate. Run it before any goport merge.
- `bound2.sh <round>`: bound parity run with a manifest (Query, Hono, error copies, sweeps, emit).
- `measure.sh`, `measure-extra.sh`, `sweep.sh`: the project comparisons that `bound2.sh` and `gate.sh` use.
- `perf.sh [--mode default|st|both] <label> <bin>... [-- <tsgo arg>...]`: medians of wall, peak RSS and
  instructions:u on Query, Hono, zod and effect, runs interleaved across the binaries; `perf.sh compare`
  compares two runs with the same host line, tsgo args and env. Wall on dbook-lan, instructions and RSS on a
  testbox (`testbox.sh perf`). See "Timing".
- `facts [section...]`: in one call, the paths and versions agents look up before their first edit: main
  against origin, the accepted revision (commit, bins checked against the gate manifest, gate, evidence),
  the Go pins with checkouts and oracles, the project tsconfigs, the newest lane bins, active `goport-*`
  worktrees and the last gates. Local reads only, about 2 s.
- `wfstatus [--all]`: one line per workflow of the newest ts-rust session (state, agents done,
  running labels, age). `wfstatus <run-id>` prints that run's agent results. Use it instead of
  parsing `journal.jsonl` by hand.
- `prune-worktrees.py`: lists stale worktrees and merged branches. `--apply` removes them.
- `clean-builds.py`: lists the Cargo build dirs and private Go caches of worktrees idle for 3 days, and the
  Go build cache when it is over 40 GB. `--apply` removes them (logs, reports and evidence stay). On zbook the
  `ts-rust-clean-builds` systemd user timer runs `--apply` daily (`clean-builds.py timer` installs it).
- `fp.py <checkout>`: the source fingerprint recorded in the saved state. `fpcommit.py <checkout> <commit>`
  gives the same value for the checkout as it was at a commit.
- `gate.sh` also runs an editor stage (memory and answers limits of `ls_edit_bench.py` on Query
  core and Hono), so `--bins` must hold `tsgo` too. Build the bins with `--release --bins`.
- `ls_edit_bench.py --rust BIN`: editor sessions (typing, error then fix, VS Code request mix, imports,
  200-edit session, and typing and error then fix with a 150 ms pause between edits) on Query core,
  Hono and effect against Go. RSS, edit latency and answers; exit 1 when Rust is over the limits in
  its docstring.

- `buildbench.sh`, `buildbench-plan.sh`, `buildbench-remote.sh`: timed `ts_goport` builds on a quiet
  host under its lock (see "Build toolchain" below). `bin-identity.sh <out> <bins-a> <bins-b>`:
  byte-for-byte output of two bin dirs (`tsgo -p` with emit and `goport -p`) on Query core, Hono,
  zod and effect.

Revision bindings made before this move pin `/tmp/port/fp.py`. That copy is identical.
`tmp-port.sh restore` puts back the legacy `/tmp/port` files after a reboot or a tmpfiles cleanup
(a login does it too). It stays after the legacy roster: the sweep step of `scripts/upstream/rerecord.sh`
runs it first (the oracle sweeps write their build info under `/tmp/port`), and the runners in
`compat/p5-corpus` and `typesyms/scale` call `/tmp/port/treehash.py`. Put new tools here, never in `/tmp`.

## Revision pipeline

The protected set is goport's own tests and the gate items (`docs/typechecker-accountability.md`,
"Protected set"). The base of a candidate is the last accepted revision: its goport test results
(after a legacy batch, `docs/goport-protected/tests-r131.json.gz`), its gate manifest and its LSP
and API oracle results (after a legacy batch, its LSP run and the rule's `apiBaseline` `api-r131`,
an API run of the R131 bins). `open_revision.py --base` prints it. The legacy roster drive is
retired. R132 is the last legacy revision.

1. `candidate.sh check <branch>`: scope and rustfmt of the branch against the checkout
   `target/worktrees/checker-port`. A goport batch may change every path except
   `docs/typechecker-state`, `docs/typechecker-batches`, `target/` and the protected paths
   (`open_revision.py --protected`: the tools, runners, oracles, baseline and rules that judge the
   protected set, `remote.sh`, `UPSTREAM.json` (the oracle and Go checkout of each pin),
   `scripts/run-cargo-capped.sh` (it builds the test and release bins and runs clippy), and every
   script that `gate.sh` and `bound2.sh` run from the repository, which it reads from their text).
   The check fails when the branch changes a protected path since its merge base with `main`,
   unless the batch lists that exact path (with Theo's approval).
2. `candidate.sh open <rev> <branch> --hypothesis TEXT --change TEXT --new-batch <id> --origin TEXT`:
   applies the branch to the checkout in one commit and records the revision (`open_revision.py`,
   the only state write). Leave out `--new-batch` for a later revision of an open batch. Then commit
   the state.
3. `candidate.sh side <rev> [--gate-host HOST | --host testbox] [--name-map TSV]`: in a systemd unit, the release
   bins, the goport tests (`build-goport-tests.sh`, `goport-tests.sh`, then `compare-tests.py`
   against the base results), two bound runs, the full gate and `gate-compare.py` against the base
   gate manifest, the LSP oracle (`lsp_oracle.py`) and the API oracle (`api_oracle.py`, 10
   batteries) each compared per request with the base results (`oracle-compare.py`), and rustfmt
   and clippy on `ts_goport` and the kept crates (in the checkout's own `target/candidate-clippy`; the
   quality record is reused only for the same `fp.py` fingerprint). Wait for `SIDE DONE`. A pin bump that
   renames Go tests, or a moved test, needs `--name-map` (old suite, old name, new suite, new name,
   then the evidence). A gate run that fails its compare stays in the evidence cache for good, as
   `gate-fail-<gate label>.json` and `gate-compare-fail-<gate label>.json`, and the next `side` runs a
   new gate with a new label (repeat-run rule). Before the verdicts, root records a flake note
   `flake-r<rev>-<name>` for each item of a failed run, naming the item id and the run label, with
   the evidence that the flake rule asks for.
4. `candidate.sh verdict-request <rev>`: the texts for the auditor and the reviewer, with every
   failed gate run of the source and its flake notes, then the accept command. The texts ask for a
   verdict that names the goport tests, gate manifest, name map and gate id map sha256.
5. After two PASS verdicts: `accept_revision.py --revision <rev> --evidence <cache dir> --scope TEXT
   --outcome TEXT`. It refuses a failed gate run that has no flake note for an item. It records the
   evidence and the verdicts (the history row and both verdicts carry `goportTestsSha256`,
   `gateSha256`, `nameMapSha256` and `gateIdMapSha256`), and every gate run of the source in `gateRuns` (each failed
   run with its regressions and flake notes, then the batch gate). It runs
   `check-typechecker-batch.mjs`, and records the acceptance only when the check passes. The check
   also finds each kept `gate-compare-fail-<label>.json`, runs `gate-compare.py` on that run again
   and needs a flake note for each regressed item, so skipping the refusal does not pass.

`gate-compare.py <base manifest> <new manifest>` compares the gate item by item: a base MATCH stays
MATCH, or becomes ALLOWED only by an allow entry (same id, condition and case path) that the base
allow list has too (the single-threaded-equal items change between MATCH and ALLOWED on the same bins).
An ALLOWED item needs a verified allow condition, and a removed id or a new FAIL is a regression. A
corpus id names another case at another Go pin, so each `corpus-diag`, `corpus-emit` and `f1` entry of
`gate-allow.txt` names its case path, and the gate applies it only to the item of that id with that
case path. The open editor
long-growth items (`editor/*/long`) may FAIL only while the batch has the open defect
`editor-long-growth`, only on growth, and only up to a fixed cap per project: query-core 1.58 and
hono 1.28 MiB/edit (`LONG_CAP` in `gate-compare.py`, the one place of the caps; the output lists them
in `longCaps`). Each cap is the highest growth of a good build + 0.15, and it does not follow the base,
so growth cannot add up over revisions. Caps only go down. A batch that fixes some growth can lower
`LONG_CAP` (a protected path the batch lists; the reviewer checks the value). A cap also goes down by
itself to 1.00, the lowest value of the gate's own limit (2 x Go + 1), once the base Rust growth of that
project is at or under 1.00: from then on its item must be MATCH with growth at or under 1.00. A MATCH
at a higher growth does not lower it, because the gate's limit follows Go's slope and the same bins
can then FAIL. A pin bump that renumbers the corpus cases names a gate id map in `batch.gateIdMap`
(`path`, `sha256`; lines of old id, new id and case path). It applies only when the two manifests are
at different upstream pins: a mapped id is the same item, and a base allow entry moves only with its
own case. A line's case path must equal the case path of the base item and of the new item. The map
cannot remove a case. An unmapped base id of a mapped family, or a line that names another case, is a
removed id (format and rules in the `gate-compare.py` docstring). The corpus-emit stage runs the case
paths of `gate-emit-sample.txt`, so a pin that adds cases keeps the same sample under new ids.

`oracle-compare.py <base results dir>... <new results dir>` compares LSP or API oracle results per
request: a base request that was `same` or `oracle_error_same` must stay so. It exits 1 on a lost,
unrun or absent request, and 2 on bad input. Options (formats in its docstring):

- Several base dirs: every run counts. A request is protected when it is protected in any base run.
  `batch.oracleRebase` gives these runs at a pin bump: the base batch's bins run again at the new pin.
- `--answers FILE@SHA256` (repeatable): an answer set, the recorded Go answers of flaky requests at one
  pin (`batch.oracleAnswers`). A base `flaky_oracle` request in a set counts as `retainedByAnswers` only
  when goport's new answer is in the set. At another pin it is protected like a `same` request.
- `--parity` with `--known-diff KEY` (API only): the new run must match Go at its pin. The output lists
  each problem, and a problem gives exit 1.
- `--identity`: each head also gets `resultsSha256` (the `sha256sum` listing of every file in the dir),
  and the tsgo and oracle sha256 values that the run records.

Without options and with one base dir, the output is byte-equal to the output before the options.
`candidate.sh side` passes the answer sets of the base and of the batch. With `batch.oracleRebase` it
also passes its runs, `--parity`, the API known diffs and `--identity`. It reuses a passing compare only
when the base runs, answer sets and rebase record have not changed.

`candidate.sh` runs its local helpers from its own checkout, so a worktree copy can be tried with
`--dry-run` before its merge. The state, `target/` and the host commands (`gate.sh` and the oracles,
which `remote.sh sync-scripts` copies from the main checkout) always use the main checkout.

## Build toolchain

`scripts/run-cargo-capped.sh` runs edit-loop commands (`build`, `check`, `test`, `run`, `bench`)
with the pinned `nightly-2026-06-17` and `-Zthreads=8` (the job count, at most 8), and builds
`ts_goport` incrementally. Release `-p ts_goport --bins` on dbook-lan, 16 jobs, sccache off
(`target/continuation-r97-goport/buildspeed/bench.md`):

| build | 1.93.0 (before) | nightly -Zthreads=8 | + incremental ts_goport (default) |
|---|---|---|---|
| clean | 215 s | 84 s | 84 s |
| touch one file | 75 s | 43 s | 7 s |
| one-line edit | 75 s | 43 s | 32 s |

- Incremental is the edit-loop gain: a one-line edit also takes 32 s on 1.93.0 with it. The nightly
  gains are in clean builds, large edits (merges, rebases, branch switches), `check` and `test`.
- Output does not change. `tsgo -p` with emit and `goport -p` on Query core, Hono, zod and effect
  are byte-equal to 1.93.0 bins, the quick gate is equal item for item, and `perf.sh` run time is
  equal within 1%.
- 1.95.0 is the shipped toolchain: `build-release.sh` and `build-pgo.sh` (shipped and timing bins),
  `release.yml`, CI (`fmt`, `clippy`, tests) and the candidate evidence builds (see below) set it.
  `--profile goport` through `run-cargo-capped.sh` and `fmt` or `clippy` there use the default
  toolchain, as does any command with `RUSTUP_TOOLCHAIN` or a `+toolchain` argument. For timing,
  build every side with the same toolchain.
- `TS_CARGO_NIGHTLY=0` uses the default toolchain. `TS_CARGO_INCREMENTAL=0` turns incremental off;
  `1` turns it on for every workspace crate. After an internal compiler error, build again with both
  set to 0 and report the error.
- On a host without the toolchain, edit-loop builds stop with the install command (a quiet fallback
  to the default toolchain made them slower with no error). Install it with
  `rustup toolchain install nightly-2026-06-17 --profile minimal`.
- The first build in each target dir after the switch rebuilds every crate once. The incremental
  cache of ts_goport takes about 1.7 GB per target dir. sccache does not cache an incremental crate, so only
  `ts_goport` is incremental: sccache still caches every other crate.
- To measure a build change, run the `buildbench-plan.sh` sessions `defaults` (release) and `devloop`
  (dev: `check`, `--bin tsgo`, `--bins`, `test --no-run`) before and after it on one host:
  `buildbench-remote.sh dbook-lan defaults` from the worktree under test (the `setup` session copies
  its source to the host first), or `buildbench-plan.sh <session> <out-dir>` on the host you are on.
  `buildbench-report.py <runs-dir>...` makes the table, with the peak memory of each build.

Edit loop on alvin (24 cores, 16 jobs, sccache off, 2026-10-10, `ts_goport` 566k lines; two runs,
`buildbench-plan.sh` sessions `devloop` and `defaults`):

| command | clean | touch | one-line edit |
|---|---|---|---|
| `check -p ts_goport` | 67-70 s | 9-10 s | 12 s |
| `build -p ts_goport --bin tsgo` | 80-88 s | 14-16 s | 20-21 s |
| `build -p ts_goport --bins` | 84-87 s | 14 s | 18-19 s |
| `test -p ts_goport --no-run` | 83-91 s | 16-17 s | 21-22 s |
| `build --release -p ts_goport --bins` | 107-112 s | 14-15 s | 69-71 s |
| `build --release -p ts_goport --bin tsgo` | 119-130 s | 16-19 s | 71-78 s |

- Use `check` while you edit, and build bins only to run them. `--bin tsgo` saves no wall time: the 9
  links run in parallel (dev `--bins` uses about 10 s more CPU per edit).
- A touch with no change costs 9-10 s in `check`: macro expansion, name resolution and the
  incremental dep graph run on the whole crate each time (`-Ztime-passes`). So an edit in
  `ls/hover.rs` costs the same as one in the checker.
- Peak memory of a whole build is at most 6.1 GB, except `test --no-run` (10 to 11 GB clean).
- The nightly links with rust-lld. mold (`-C link-arg=-fuse-ld=mold`) used about 25% less CPU per
  edit, but dev `--bins` and `test --no-run` edits took the same wall time (21 to 31 s, two runs
  each), so the script keeps rust-lld.
- With 1 job (the default on every host but zbook until 2026-10-10), `check` took 228 s clean and
  17 s for an edit, and a dev `--bin tsgo` build 301 s clean and 29 s for an edit.

Revision evidence is the exception: `candidate.sh side` builds its release bins and its test bins with `TS_CARGO_NIGHTLY=0 TS_CARGO_INCREMENTAL=0` and `RUSTUP_TOOLCHAIN=1.95.0`, so the goport tests, gate, bound runs and oracles test bins from the shipped toolchain with no incremental cache (the evidence key records the toolchain). Until 2026-10-10 it took the host's default toolchain (1.93.0 on zbook and in CI). With these settings on alvin (`buildbench-plan.sh` session `toolchains`, one run each):

| toolchain | clean | touch | one-line edit |
|---|---|---|---|
| 1.93.0 | 284 s | 340 s | 263-303 s |
| 1.95.0 | 163 s | 139 s | 130-135 s |
| 1.99.0 | 144 s | 115 s | 134 s |

The time goes to the compiler frontend: 192 to 272 s of each 1.93.0 build, 64 to 72 s on the newer ones. `bin-identity.sh` gave equal output for 1.95.0 and 1.99.0 against 1.93.0 on Query core, Hono, zod and effect.

## Editor sessions: `ls_edit_bench.py`

Our other checks measure startup and watch mode. This one measures a language server during edits,
against Go, with the same messages on both sides.

Run it on zbook:

- for every integration candidate, before it merges
- for every change to `crates/ts_goport/src/{ls,lsp,project,api}/`, `program.rs` or the allocator
  (jemalloc features, `set_malloc_tunables` in `src/bin/tsgo.rs`)

```
flock /tmp/goport-lsguard.lock scripts/goport/ls_edit_bench.py \
    --rust cand=/abs/path/to/candidate/tsgo --rust base=/abs/path/to/main/tsgo \
    --out target/continuation-r97-goport/lsguard/<label>
```

- Use absolute binary paths. Each server starts in its project directory.
- Put the main build in the same run (`--rust base=...`). Then load from other agents affects both
  Rust sides equally.
- Other upstream pin: `GOPORT_PIN=<key> scripts/upstream/pin.py exec -- scripts/goport/ls_edit_bench.py ...`.
- One pass is 3 projects (Query core, Hono, effect) x 5 scenarios (typing, errfix, mix, imports, and a
  200-edit long session), plus typing-paced and errfix-paced on Query core and Hono. With Go and two
  Rust builds it takes about 4 to 5 minutes, plus the wait for the lock.
- One session again: `--projects hono --scenarios long`. New limits on an old run: `--recheck --out DIR`.
- Paced scenarios on effect: `--projects effect --scenarios typing-paced,errfix-paced` (not in the
  default pass, to keep it short).

Think time (pace): the pause after each edit's answers, before the next edit. The client sends
nothing then, except answers to server requests. The Rust server runs idle work (the auto-import
warm) only after 50 ms with no message (`IDLE_QUIET_PERIOD` in `lsp/server.rs`). Editors pause
between keystrokes, so an edit can arrive while that work runs.

- typing-paced and errfix-paced: typing and errfix with a 150 ms pause. They catch the leak that
  editors hit: R123 on Query core goes to 6.4 GiB after 40 edits, with a 185 ms median edit (Go 266
  MiB, 17 ms).
- `--pace-ms N`: the same pause for the other scenarios (default 0).
- `--sweep [MS,...]`: for diagnosis. It runs each scenario once per pause (default 0, 30, 80, 150 and
  300 ms). Defaults: Query core, typing and errfix, 20 edits. `--projects`, `--scenarios` and
  `--edits` change them. R123: 0 and 30 ms pass, 80 and 150 ms add about 155 MiB per edit, 300 ms
  passes (the warm finishes before the next edit).

How to read the result:

- Exit 0: every limit passes. Exit 1: a limit failed. Exit 2: usage error, or a Go session failed.
- `report.md` has one row per project, scenario, pace and side, then "Failed limits" with the value
  and the limit for each failure. `result.json` has every edit.
- Limits (Rust against the Go of the same run):

  | limit | Rust must be at or below |
  |---|---|
  | rss | RSS added by the edits: 2 x Go + 256 MiB |
  | growth | RSS slope over the second half, long session only: 2 x Go + 1 MiB per edit |
  | editMedian | median didChange-to-diagnostics time: 1.5 x Go + 5 ms |
  | editP95 | p95 of the same: 2 x Go + 20 ms |
  | roundMedian | median time until the last answer of the burst: 2 x Go + 10 ms |
  | answers | no error, crash, timeout or RSS-cap kill where Go answered |

- rss and growth do not change much with load. The latency limits do. If only a latency limit fails,
  run that session again before you trust it.
- Answer differences are listed but not judged. Hono completion `autoImport/moduleSpecifier`
  (Go `"."`, Rust `"./adapter/bun"`) also differs between two Go runs. Look at any other difference.
- Known state on 2026-09-27: R122, R123 and the branches on R123 fail 5 unpaced sessions on memory:
  imports on Query core and effect (rss), and long on all three projects (growth and rss).
  Go against Go passes all 15. R123 also fails typing-paced and errfix-paced on Query core (rss and
  all latency limits); it passes them on Hono. Go against Go passes all 4 paced sessions.

## Timing

Root decision of 2026-10-11 (state note `tbxperf0-result-2026-10-11`; data in `tbxperf0/report.md` and
`tbxperf1/summary.md` under `target/continuation-r97-goport/`):

| metric | bar | where | how | A/A |
|---|---|---|---|---|
| instructions:u, `--singleThreaded` | 1% | a testbox, both sides on one testbox | `testbox.sh perf <label> <bin>...` | 0.005% or less |
| peak RSS, default 4 checkers | 5% | a testbox | the same call, median of 3 | query up to 2%, the others 0.6% |
| wall, default 4 checkers | 2% | dbook-lan, every side in one run (mini-743d until 2026-10-12) | `perf.sh <label> <bin>...` through `remote.sh` (5 rounds) | zod and effect up to 1.1% (tbxperf0: 1.5 to 1.8% at the 95th percentile), hono 0.4%; query up to 6.4% |

- `perf.sh` runs `perf stat -e instructions:u,cycles:u,task-clock` and `/usr/bin/time` (max RSS) on each run. Wall
  is perf's own elapsed time in microseconds. `/usr/bin/time %e` had 10 ms steps (about a quarter of a Query run), and the
  sudo and perf start add 24 ms on mini-743d. `--mode st` runs `--singleThreaded`, `both` runs st and the
  default 4 checkers. `PERF_ROUNDS` or `--rounds` sets the rounds: 5 by default, 3 on a testbox.
- cycles:u on a testbox: the Zen 5 guest counts only the first hardware event of a perf stat set, and the
  second reads 0. A probe at the start of `perf.sh` finds this and counts cycles:u in a second run of each
  cell (host.txt `cycles separate`), so a testbox run takes twice as long. dbook-lan counts both in one run
  (`cycles joint`). Before this fix (perfcyc1, 2026-10-23), testbox runs had cycles:u 0, and a default-mode
  run sometimes lost instructions:u (one thread counted them on the dead counter: -0.18% on effect, up to -27%
  in `perf/gap1-cells`). st runs could lose them too (t3web-ne -8.85% in one `perf/int90-tbx-cells` round), so
  older testbox instruction counts are not clean. Lane copies of the tool (`perf-cells.sh`) need the same fix.
- Query runs for 45 ms, and its wall moved about 5% from run to run on mini-743d: use zod and effect for a wall
  claim.
- dbook-lan is the only wall host since Theo retired alvin, cup2 and mini-743d (2026-10-12). Its A/A is not
  measured yet: run one before a wall claim near the 2% bar. The A/A column above is from mini-743d. cup2 was not a wall host: an A/A at median of 5 gave zod 3.0%, effect 1.8% and query
  11.5% (`perf/tbxperf1-r2-cup2-aa`; its virtio balloon has free page reporting on). alvin has no perf.
- At 4 checkers the instruction counts include waits that depend on timing (A/A: query up to 0.42% at median of
  3 on a testbox, hono 0.09%, zod and effect 0.03%). Use the st counts for an instruction claim. Do not use cycles:u for a 1% claim on any
  host (it is in `runs.tsv` only), and do not use minor faults on a testbox.
- Emit: `perf.sh` runs `tsgo -p` with emit, and the read-only inputs refuse the writes. The emit is about 25% of
  the effect instructions. For checker work add `-- --noEmit`: R190 against R189 on effect, st, is +9.39% with
  emit and +12.80% with `--noEmit` (+4.17 G instructions both ways).
- Hono configuration: a timing report names the Hono configuration it ran (bin, tsconfig, args, emit or
  `--noEmit`, exit code and error count), so its counts are not read as the gate result. `perf.sh` and the
  timing output checks run `tsgo -p target/project-inputs/hono/source/tsconfig.build.json` on the read-only
  input. The input refuses the tsbuildinfo write (TS5033, also with `--noEmit`). With `--noEmit` they give 3
  errors and exit 1. With emit the run ends in a nil-dereference panic, exit 2 and no diagnostics, as Go at the
  pin does: the TS5033 has no file and `diagnosticToStringBuilder` (Go snapshot.go:405, port
  `execute/incremental/snapshot.rs`) reads its file name. So a Hono timing with emit times a run that panics in
  the emit. The gate item `measure/hono` and the bound runs run `goport -p` on the same tsconfig: exit 2 with the
  2 TS7031 errors, Go's answer at the pin (honorec1).
- Host line: CPU family/model and name, microcode, kernel, perf version, THP and the virtio balloon free page
  reporting state (`perf.sh host`). `perf.sh compare <base> <new>` (bin0 of base against each bin of new;
  instructions and RSS, no wall) refuses two runs with different host lines, tsgo args or env. Fresh testboxes gave
  the same R190 counts within 0.002%. Do not compare a testbox number with a mini-743d number.
- Testbox wall is not valid: the guest clock moves about 1.8% per run, and free page reporting gives host page
  faults that steal and load do not show (tbxperf0 skeptic). A testbox summary leaves wall out.
- perf: `perf.sh` takes the first perf that runs (`PERF_BIN`, `perf`, `/usr/lib/linux-tools/*/perf`: the
  testbox's `/usr/bin/perf` wants tools for its kernel). At perf_event_paranoid 3 or more (mini-743d, dbook-lan:
  4; cup2: 3) perf runs under `sudo -n` and the binary under `setpriv` as the user.
- Env: tsgo runs under `env -i` on every host, so sudo's env reset does not change what it sees. It gets HOME,
  `PATH=/usr/local/bin:/usr/bin:/bin`, `LC_ALL=C`, `GOPORT_LAUNCH=0` and the caller's `GOPORT_*` (not
  `GOPORT_PIN*`), `_RJEM_MALLOC_CONF`, `MALLOC_*`, `GLIBC_TUNABLES`, `GOMAXPROCS`, `GOGC`, `GOMEMLIMIT` and
  `GODEBUG`. `perf.sh env` prints the ones set. host.txt records them on its `env` line (`none` when no one is
  set), and `compare` refuses two runs with a different env. `testbox.sh perf` passes them on to the testbox.
  For an A/B of a variable (for example `GOPORT_LIB_SNAPSHOT=0`), put both sides in one run: one side is a
  wrapper script that sets the variable and execs the bin (the start of bash counts on that side).
- Time like against like: every Rust side is a stable build. On a testbox, build with
  `TS_CARGO_NIGHTLY=0 TS_CARGO_INCREMENTAL=0 TS_CARGO_JOBS=32 scripts/run-cargo-capped.sh build --release ...`.
  Release builds (PGO and BOLT) of a revision timing run on a testbox too: `testbox.sh release-build` (see
  "Blacksmith testboxes"). Both sides of a timing are the same kind of build: testbox builds against testbox
  builds, never against zbook LBR builds.

## Remote runners

`remote.sh` copies binaries and scripts to a remote host, runs a command there in the repo root and
fetches the results. zbook builds. Each host keeps a mirror at zbook's absolute paths (manifests and
setup notes in `target/continuation-r97-goport/remote/`). The header of `remote.sh` has the usage.

| command | use | lock |
|---|---|---|
| `status [host...]` | per host: load, free RAM, busiest process, and the zbook lock (holder, age, waiters) | none |
| `look <host> <cmd>` | a quick look: logs, files, hashes, tools. Same route and paths as `run`, 120 s limit | none |
| `run <host\|auto> <cmd>` | a job in the repo root. stdin passes through: `run <host> bash -s <<'EOF'` | holds it |
| `job <host\|auto> <cmd>` | a zbook script that does several steps on `$REMOTE_HOST` (sync, run, fetch) | holds it for the whole script |
| `sync-bins`, `sync-scripts`, `sync-pin`, `push` | copy to the host (`all`: every host at once) | none |
| `fetch <host> <dir>` | copy results back; never replaces a file | none |

Locks:

- `run` and `job` take the zbook lock `/tmp/goport-remote-<host>.lock` (dbook-lan: `goport-remote-dbook.lock`)
  and wait for it. While one waits, it prints the holder.
- A caller that holds the lock passes the open lock file on to its children. `run` finds it in
  `/proc/$$/fd` and does not lock again. So `flock /tmp/goport-remote-<host>.lock remote.sh run <host> ...`
  and scripts that do `exec 9>LOCK; flock 9` still work. The flock wrapper is optional now.
- A script under `job` must not take the lock itself: it would wait for its own lock (`status` then shows
  the job as holder with 1 waiting).
- The lock does not pass through `systemd-run`. Take it inside the unit, not around `systemd-run`.
- `auto` takes the first host in order whose lock is free, whose load is under half its cores and whose
  mirror paths are right. When no host is free, it checks again every 30 s.
- Until 2026-09-28, `run auto` released its lock when the command started (ssh closes inherited file
  descriptors), and `run <host>` took no lock. `run` now keeps its shell alive as the lock holder.

Do not use raw `ssh` or `rsync`. `look` reaches the host over the right route (the minis by their LAN
names) with zbook's paths (alvin through its `zbook-paths` wrapper, dbook with `HOME=/home/theo`).
`pin.py sync <host>` uses the ssh config route, which is Tailscale for the minis: use `remote.sh sync-pin`.

Root: name a host in an agent prompt only for timing. Other jobs use `auto`. dbook-lan is kept for
revision evidence (`remote.sh help`, "Reserved"): timing that needs zbook-class hardware runs there with
`REMOTE_USE_RESERVED=1` only when root names it and no revision run waits for it. Record each package install or other host change in
`target/continuation-r97-goport/remote/<host>-setup.md`.

- alvin, cup2: cloud hosts for gates, corpus suites, sweeps and oracle checks. alvin has no sudo. cup2
  has `sudo -n` and `/usr/local/sbin/fleet-pkg-install`.
- dbook (ssh host `dbook-lan`): LAN host with zbook's CPU (Ryzen AI Max+ 395, 32 threads) but
  only 26 GB RAM. It is kept for revision evidence: `candidate.sh side` runs the gate and the
  oracles there, `auto` skips it, and `run` and `job` refuse it without `REMOTE_USE_RESERVED=1`.
  Every dbook job takes the same lock, timing included, so the two never overlap.
  dbook is on the same LAN as zbook: always use `dbook-lan` (dbook.local), never the Tailscale
  name `dbook`. `remote.sh` maps `dbook` to `dbook-lan`.
- mini-743d (Ryzen 7 8845HS): LAN mini with 16 threads and 28 GB RAM. It runs gates and checks.
  It is wired since 2026-09-28 (about 215 MB/s over ssh), so it is also good for quiet timing and
  the editor benchmark (`ls_edit_bench.py`, every side on the same host). Every job takes the
  host's own lock, timing included. `remote.sh` always reaches it by the LAN name
  mini-743d.local, never Tailscale. It maps the `-ts` name to `mini-743d`.
- mini-abf9 is Theo's own machine again since 2026-10-04. `remote.sh` refuses every command on it.
- dbook-lan and mini-743d: `sudo -n`, perf_event_paranoid 4 (use `sudo perf`), THP madvise, and
  perf, bpftrace and strace installed.

## Blacksmith testboxes

`testbox.sh` runs builds, the protected suites, the gate and the oracle batteries on a warm Blacksmith
runner (32 vCPU, 124 GB RAM), not on zbook or the SSH hosts. `.github/workflows/testbox.yml` sets up the
testbox, `testbox-data.sh` makes zbook's paths there, and `testbox-remote.sh` holds the steps of `tests`,
`np-suite` and `pin`. `testbox.sh help` gives the usage.

```sh
scripts/goport/testbox.sh warmup                                # about 1 min; TESTBOX_REF=<branch> for a workflow change
scripts/goport/testbox.sh tests <out-dir> <base results.json>   # build-goport-tests.sh, goport-tests.sh, compare-tests.py
scripts/goport/testbox.sh np-suite --bins <bins dir> <label> [<base label>]
                                                                 # np-suite.sh run on <bins dir>/tsgo, then np-suite.sh diff
scripts/goport/testbox.sh perf <label> <bin>... [-- --noEmit]   # perf.sh: instructions:u and RSS (see "Timing")
scripts/goport/testbox.sh release-build <out-dir>               # build-release.sh at HEAD, BOLT from instrumentation counts
scripts/goport/testbox.sh push <bins dir>                       # then run gate.sh, lsp_oracle.py or api_oracle.py as on zbook,
                                                                 # from the zbook path, so the manifest names the same scripts:
                                                                 # run bash -c 'cd /home/theo/Code/sandbox/ts-rust && GOPORT_PIN=<pin> bash scripts/goport/gate.sh ...'
scripts/goport/testbox.sh get <remote> [<local>]                # rsync a result back
scripts/goport/testbox.sh job <command...>                     # as run, detached: survives a dropped SSH session
scripts/goport/testbox.sh source <checkout>                     # the checkout at its own zbook path there (fp.py checked)
scripts/goport/testbox.sh stop                                  # or release (see "Sticky disks")
```

- Warmup: bubblewrap, psutil, Node 24, Go, Rust 1.93.0 and the nightly, and zbook's paths
  (`testbox-data.sh mount`):
  - The workspace is bound on `/home/theo/Code/sandbox/ts-rust`, and the testbox env sets
    `HOME=/home/theo`.
  - The whole checkout of the batch pin, with `npm ci` node_modules, is bound on its zbook path.
  - Each path of `testbox-data.txt` is bound from its sticky disk.
  - `pin.py exec` binds the pin over the default oracle and goCheckout paths. bwrap makes those bind targets.
- Data: `testbox-data.txt` lists the zbook files that the gate and the LSP and API oracles read and git does
  not hold. Disk `ts-rust-testbox-inputs` holds the files that are the same for every pin: the project inputs,
  the runner trees, the corpus and the Effect e1 data (below). Disk `ts-rust-testbox-pin-<key>` holds the pin
  cache, the LSP and API goldens and the pin oracle. `testbox.sh seed [inputs|pin]` copies them from zbook (30
  to 35 minutes; a rerun copies only what is missing). Then `release`. A testbox reads the list of the ref
  that it warmed up from: to seed a new path, warm up with `TESTBOX_REF=<your pushed branch>`. With main's
  list, the copy goes to the target disk, not the inputs disk.
- Effect e1 (`lsp_battery_effect.py`): the inputs disk holds its traces, the E0 goldens and flaky files, the
  E0 copies, the T3 Code copy of e1-t3code and the effect-tsgo 0.51.1 repo (3.4 GiB). A lane pushes only its
  bins: `push <bins dir>`, then
  `run bash -c 'cd /home/theo/Code/sandbox/ts-rust && GOPORT_LAUNCH=0 python3 scripts/goport/lsp_battery_effect.py check --server <bins>/tsgo --label <label> --jobs 16'`
  and `get` `ls-oracle/effect/results/<label>/` (the results are on the target disk). New e1 goldens (a new
  part or a new E0) reach the disk with another `seed inputs` and a release. `--parts` limits a run to some
  parts: e1-effect and e1-t3code, and e1-effect-ce and e1-t3code-ce (completion at `// @` and after
  `extends Data.`). On a fresh testbox (2026-10-11) the R190 check took 20 s for the first two parts and
  33 s for all four; the push of tsgo took 4 s.
- Oracles: `pin.py` checks the sha256 of the pin oracle, and a build on the runner has another sha256 (Go
  puts absolute paths in the binary). So the testbox uses zbook's recorded binary from the pin disk.
  `tests` compares the testbox's copy with UPSTREAM.json and pushes zbook's binary when they differ.
- Git: the sync sends files, not git state. `tests` needs HEAD on origin: the testbox fetches it and resets
  its index to it, so `build-goport-tests.sh` sees a clean tree at HEAD. Before a gate, fetch the commit
  that the bins name (`run git fetch --depth=1 origin <sha>`).
- Other pins: `testbox.sh pin KEY` checks out a pin of this worktree's UPSTREAM.json (microsoft/TypeScript
  pins only), for example bump D fed0bf24149f from a bump D worktree. Then
  `testbox.sh tests --pin KEY <out-dir> <base results.json>` or `testbox.sh np-suite --pin KEY <label>`.
  The pin disk holds only the batch pin's files. `tests --pin` pushes the other pin's oracle to that
  testbox only.
- Output: `tests` writes `<out-dir>/tests-run` (results.json, logs, tsv, track), `<out-dir>/testbin` (the
  metadata, no binaries) and `<out-dir>/compare.json`. After a failed build it writes the cargo logs to
  `<out-dir>/testbin.new/logs` and prints the compiler errors. `np-suite` writes the label dir in the main
  checkout's np-suite dir, and `perf` in the main checkout's perf dir. None replaces an existing dir.
- `np-suite --bins <dir>` pushes `<dir>/tsgo` (checked against `<dir>/bins.sha256`) and runs on it, so
  meta.txt names its zbook path and sha256; another sha256 there is an error. Revision evidence uses it with
  the candidate's release bins. Without `--bins` the step builds a stable release tsgo on the testbox, and
  that build is not always byte-equal to another build of the same commit: jemalloc's configure takes
  `LG_VADDR` from the build CPU's virtual address bits (cpuid). The perf host records show two testbox CPU
  models: cpu family 26 model 68 (48 bits) and model 2 (EPYC Turin). On 2026-10-24 the npint98w build of
  c428ad2d2 gave tsgo `6a4e7a3a7784`, the side build `54433ccd1aa8`; a rebuild with
  `JEMALLOC_SYS_WITH_LG_VADDR=57` gives `6a4e7a3a7784` (npbuild1). `testbox-build.log` starts with the tree,
  rustc and CPU, and `--expect <sha256>` warns when the built tsgo differs.
- `perf`: pushes the bins to their zbook paths, runs `perf.sh --mode both` (3 rounds; `--mode` and `--rounds`
  pass through, and so do `-- <tsgo arg>...` and the variables of `perf.sh env`) and fetches the label dir. Without a saved testbox it warms one up
  (`TESTBOX_IDLE` 30) and stops it at the end. Read the instructions and the RSS: wall is not valid there.
- Activity: the idle timer reads the marker `/home/runner/.testbox-last-activity`. The CLI touches it at
  the start and the end of each `run`. `testbox.sh` runs the command in a subshell, so the
  `HOME=/home/theo` of the testbox env does not move the end touch. The timer does not see the SSH route
  of `push`, `get` and the `tests` poll. So `push` and `seed` touch the marker every minute, and the
  `tests` and `np-suite` jobs every 30 s.
- Retries: a DNS, connect or TLS error or a request timeout of the blacksmith API, ssh or rsync makes the step
  run again, up to 6 tries after 5, 10, 20, 30 and 30 s (`TESTBOX_RETRY_WAITS`), with one line on stderr per
  retry. This covers warmup, status, stop, list, the rsync of push, get, source and fetch, and the start of
  `run` and `job`. A command that started on the testbox never runs again: `run` prints a start marker on the
  testbox's stderr, and retries only before it. A nonzero exit of the command (a failed build too) is not
  retried. `scripts/goport/tests/testbox-retry.sh` checks this with shims and no testbox.
- Sticky disks: `target/` and the cargo registry, and the inputs and pin disks. The inputs and pin disks
  commit only when a file changed (`commit: on-change`), so a testbox that only reads them does not commit an
  older copy. A sticky disk commits only when the job ends with no failed or cancelled step. `stop` cancels the
  job, so its writes are lost. `release` forgets the testbox, which ends at its idle timeout
  (`TESTBOX_IDLE`, default 15 minutes) and requests a commit.
  - Not every requested commit lands. On 2026-10-10, five testboxes that lived 2 to 13 minutes committed,
    up to 2.1 GiB (a warm target after `tests` and `np-suite`). One that lived 27 minutes (10 to 15 idle
    minutes at the end, no `stop`) requested a 2 GiB commit that never landed. The cause is not known: a
    39-minute testbox of another session probably did commit.
  - `TESTBOX_IDLE=1` with a release right after the last command gave commits that landed. Only `tests`,
    `np-suite`, `push` and `seed` keep a testbox alive: with a short idle timeout, a `run` or `pin` command
    longer than about 6 minutes is cut off (the VM waits at most 290 s for SSH sessions, then shuts down).
    `TESTBOX_ID=<id> testbox.sh status` shows the end.
  - The last commit wins, across branches too: every branch uses the same disk keys.
  - `on-change` compares the disk usage, not the files: a change of 4096 bytes or less (for example a file
    written again at the same size) does not commit.
  - The idle loop keeps the time of the last activity in memory and only moves it forward, so an older
    marker time does not end a testbox sooner. A released testbox with `TESTBOX_IDLE=60` lives 60 more
    minutes. `seed` keeps the testbox alive itself, so a short `TESTBOX_IDLE` is safe for a seed. On
    2026-10-11 both seed commits landed: `TESTBOX_IDLE=60` (78 minutes, 3.4 GiB) and `TESTBOX_IDLE=5`
    (21 minutes, 2.6 MB). The first took 18 minutes; the second 15, almost all of it the scan of the files
    that were there.
- Release builds: `release-build` runs `crates/ts_goport/scripts/build-release.sh` at HEAD (pushed; crates/,
  Cargo.toml and Cargo.lock clean) with `RELEASE_BOLT_PROFILE=instrument` and fetches `<out>/bin` (BUILD.txt,
  COMMIT, bins.sha256) and the logs. `NAME=VALUE` arguments pass on to the script (`RELEASE_*`, `BOLT_*`,
  `PGO_*`). The guest has no branch stack sampling, so `llvm-bolt -instrument` makes a counting copy of each bin,
  which runs the same training; the five copies train at once. The step installs what the runner image lacks:
  the script's rustc with llvm-tools, BOLT 23 from apt.llvm.org (zbook has 23.1.1) and qemu-user. The build dirs
  are outside the sticky `target/`. On R195 source (tbxrel1, 2026-10-11) a build took 16 minutes with the
  fetch (zbook: 17). Against the two zbook LBR builds: st instructions:u +0.04 to +0.30%, peak RSS the same or
  up to 11% lower, mini-743d wall -4.3 to +0.5% (7 of 9 cells lower in both builds), same output. The RSS and wall come from the profile (every block that
  ran is in the hot text, not in `.text.cold`), the instructions from the PGO build on the testbox.
- Times (2026-10-10, cold target): `tests` 502 s (test bins 3.6 min, suite 4.7 min), `np-suite` 112 s. With
  the R189 bins: the full gate 8.4 min, the LSP batteries 3 min, the API batteries 1 min. `perf` with two bins
  (2026-10-11): 2 minutes with the warmup (perf.sh itself 47 s at 3 rounds, 76 s at 5).
- `candidate.sh side --host testbox` runs the release bins, test bins, goport tests, bound runs, gate, LSP and
  API oracles and clippy on one testbox (`TESTBOX_ID_FILE` keeps its ID out of `target/testbox-id`). The results
  come back to the same zbook paths; the compares and records stay on zbook, and the evidence key and cache are
  the same. The default host stays dbook-lan until a batch lists the change; it is also the fallback after a
  testbox failure (finished steps stay cached). R190 source, 2026-10-11: 27.9 min against 29.1 min for zbook and
  dbook-lan, with the same results (target/continuation-r97-goport/tbxside1/summary.md). R195 source: 28.8 min
  against 33.2 min (the int62 side run and R195's bound runs and quality), with the same results
  (target/continuation-r97-goport/tbxside2).
  - Test bins of another checkout (an integration's side run) also run there: the testbox gets their commit
    (from origin, or from the checkout's bundle) and their two bind paths.
  - Host kinds: testbox, or ssh (zbook and the SSH hosts). With `batch.oracleRebase`, the rebase runs and the
    candidate's oracle runs must be one kind (ruling 10 "the same host setup"); `oracle-rebase.sh` runs on the
    SSH hosts, so a pin bump runs `side` without `--host testbox`. Each oracle run records its host in
    `<kind>-run.host`.
  - Editor items: the gate judges each one against Go in the same run, and no rule judges peak RSS (2 to 4
    percent higher on a testbox). `gate-compare.py` compares only the Rust growth of `editor/<project>/long`
    with its cap across runs, and that growth was the same on both kinds. `side` fails when the base and the
    candidate gates are of two kinds and a long growth is within 0.15 MiB/edit of its cap.
- Not on testboxes: wall timing. Wall runs on dbook-lan (alvin, cup2 and mini-743d are retired).
