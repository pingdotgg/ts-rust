# typescript-eslint harness

`scripts/typescript-api/consumers/typescript-eslint.sh` runs typescript-eslint v8.71.2's own vitest suites,
unmodified, against a `typescript` package directory (normally `npm/typescript-api`), and compares runs.
`typescript-eslint.sh help` prints the usage.

```sh
S=scripts/typescript-api/consumers/typescript-eslint.sh
R=target/typescript-api/consumers/typescript-eslint-runs
node npm/typescript-api/scripts/build.mjs
systemd-run --user --collect --wait -p MemoryMax=12G -p MemorySwapMax=0 \
    --setenv=PATH="$PATH" --setenv=HOME="$HOME" \
    $PWD/$S run $PWD/npm/typescript-api $PWD/$R/mine --copy
$S compare $R/stock-6.0.3 $R/mine            # exit 1 when a test that passed in the base is lost, absent or unrun
$S run npm/typescript-api $R/plugin-1 --packages eslint-plugin --shard 1/4
$S merge $R/plugin-all $R/plugin-1 $R/plugin-2 $R/plugin-3 $R/plugin-4
$S run npm/typescript-api $R/one-rule --packages eslint-plugin --files rules/no-misused-spread
```

## What it does

1. `setup` (also run by the first `run`): shallow clone of tag v8.71.2 (commit 7488d681f6e0) into
   `target/typescript-api/consumers/typescript-eslint`, then `pnpm install --frozen-lockfile` with the pnpm of
   the repo's `packageManager` field (12.9.0; corepack's `pnpm` shim, else `corepack pnpm`, else `npx pnpm@12.9.0`).
   The repo's postinstall builds every package with its stock TypeScript (`tsc6`). A warm pnpm store makes this
   about 30 s; harness state is in `typescript-eslint.harness/` next to the clone (`setup.ok`, `setup.log`, lock).
2. `run` swaps the package in, runs `vitest run --config vitest.config.mts --maxWorkers=4 --update=none` in each
   selected package directory (not through nx, so nothing rebuilds; `--update=none` so a run never writes
   snapshots into the clone), and puts the stock link back on exit, Ctrl-C and TERM (vitest stops within a
   second and the partial results are written); `restore` repairs the link after a `kill -9`. One run at a time
   per clone (flock). The run warns when the clone has changed files afterwards.
3. Results come from `reporter.mjs`, a vitest reporter that appends one JSON line per finished test file, so a
   killed or timed-out package keeps every file that finished.

## How the swap works

typescript-eslint overrides `typescript` with `@typescript/typescript6@6.0.2`, a shim whose `lib/typescript.js`
and `lib/tsserverlibrary.js` are `module.exports = require("@typescript/old")`, and `@typescript/old` is the store's
`typescript@6.0.3`. All 15 workspace `node_modules/typescript` links and the store's peer links (ts-api-utils,
...) go to that shim. The harness repoints the one link
`node_modules/.pnpm/@typescript+typescript6@6.0.2/node_modules/@typescript/old` at the package under test.

This is the research's method A at one level deeper. The shim stays a real directory under `node_modules`, so
vitest keeps loading `typescript` with Node's own `require` (one module instance per worker, as with the stock
package), and `require("typescript/package.json")` stays the shim's, as in the stock run. `probe.json` records what
the workspace then loads; the run stops when it is not the package under test. A marker package showed that every
vitest fork worker loads the swapped package.

`stock` as the package directory runs with no swap: the control.

## Output (`<out-dir>/`)

- `results.json`: one entry per test: `{package, file, name, state, error}`; `name` is vitest's full name
  (`suite > test`), `state` is passed, failed, skipped or todo, `error` the first line of the first error.
- `files.json`: one entry per planned test file with its state (`unrun` when it did not finish) and counts.
- `summary.md`: per package files run/planned, tests, passed, failed, skipped, file errors, unhandled errors,
  vitest exit code and wall time, then the failing files with their first error.
- `run.json`: package under test (path, name, version, sha256 of `lib/typescript.js` and of `src/`, git head),
  probe, clone, shard, workers, host and load.
- `raw/<package>.jsonl`: reporter records (planned files after `--shard`, one record per file with up to 5 full
  error texts, the end record with unhandled errors); `raw/packages.jsonl`: exit code and wall time per package.
- `logs/<package>.log`: one line per file and the vitest output; `package/` with `--copy`.

## compare

Tests are keyed by package, file and full name (repeated names in one file pair up in order). Per package:
retained, lost (passed in base, failed or skipped in new), gained, absent (passed in base, missing from a file the
new run ran), unrun (passed in base, its file did not finish), added (only in new) and uncovered (base tests in
files or packages the new run did not plan, for example other shards). `--limit N` lists more names, `--json FILE`
writes every list. The exit code is 1 when any test is lost, absent or unrun.

## Baselines (2026-10-13, host "dev": 10 cores, load 3 to 24 from other agents, so wall times are noisy)

Runs in `target/typescript-api/consumers/typescript-eslint-runs/`, all packages, `--maxWorkers=4`:

| Package | Tests | stock-6.0.3 pass/fail/skip | tsapi-nohooks pass/fail/skip | Wall s (stock, nohooks) |
|---|---|---|---|---|
| typescript-estree | 410 | 409/0/1 | 409/0/1 | 17.1, 36.4 |
| ast-spec | 12,720 | 4,568/0/8,152 | 4,568/0/8,152 | 9.3, 24.6 |
| type-utils | 365 | 364/0/1 | 354/10/1 | 5.4, 7.3 |
| parser | 34 | 34/0/0 | 34/0/0 | 4.2, 4.9 |
| project-service | 37 | 37/0/0 | 37/0/0 | 1.3, 1.2 |
| tsconfig-utils | 7 | 7/0/0 | 7/0/0 | 1.4, 1.3 |
| rule-tester | 120 | 120/0/0 | 120/0/0 | 2.7, 3.0 |
| typescript-eslint | 96 | 95/0/1 | 95/0/1 | 3.3, 3.2 |
| scope-manager | 543 | 543/0/0 | 543/0/0 | 5.9, 5.1 |
| eslint-plugin | 33,382 | 33,210/0/172 | 33,209/1/172 | 151.6, 153.2 |
| total | 47,714 | 39,387/0/8,327 | 39,376/11/8,327 | 202, 240 |

- `stock-6.0.3` is the control (no swap). Two more stock eslint-plugin runs (4 shards merged, and a resumed run)
  gave the same 33,210/0/172 per test.
- `tsapi-nohooks` is `npm/typescript-api` at 00:32 (`--copy`; `src/index.js` installs no hooks unless
  `TSRS_DEV_HOOKS=1`). Its 11 differences are the package's, not the harness's: see below.
- A back-to-back rerun of typescript-estree and ast-spec showed no wall difference between the two packages
  (40.9/35.6 s stock against 45.9/33.0 s, then 30.4/8.5 s against 15.3/7.5 s): the wall column is host load.
- `checks/` keeps the runs behind the explanations below (`run-marker*`, `run-ts6libs`, `run-tsrslibs`).

## Known differences that are not the harness

- **Package name (10 type-utils tests).** `npm/typescript-api/package.json` is named `@tsc-rs/typescript`.
  TypeScript takes a declaration file's package name from its `package.json` `name`
  (`program.sourceFileToPackageName`), so `TypeOrValueSpecifier > typeMatchesSpecifier > matches a matching
  package specifier` (`{ from: 'package', package: 'typescript' }`, 10 tests) fails; a copy named `typescript`
  passes all 365 (`checks/run-marker-named`). An npm alias or `file:` override installs the package under
  `node_modules/typescript` with the same name, so users of type-utils' package specifiers see the same thing.
- **lib files (1 eslint-plugin test).** `npm/typescript-api/lib/lib.*.d.ts` are tsc-rs's (TS 7.1) lib files;
  15 of them differ from TS 6.0.3's (es5, es2015.core, es2015.collection, es2015.symbol, dom, ...).
  `no-misused-spread > invalid > const o = { ...ref }` (WeakRef) reports nothing with them and passes with TS
  6.0.3's lib files in the same package (`checks/run-tsrslibs`, `checks/run-ts6libs`).

## Files

- `../typescript-eslint.sh`: setup, run, compare, merge, restore.
- `reporter.mjs`: the vitest reporter (per-file JSON lines; planned files after `--shard`, computed with the
  configured sequencer because vitest reports the files before it shards them and `vitest list` ignores `--shard`).
- `results.mjs`: run.json, results.json, files.json, summary.md; compare; merge.
- `probe.cjs`: which `typescript` the workspace loads.
- `selftest-shards.sh <pkg|stock> <runs-dir> <full-run-dir>`: eslint-plugin in 4 shards, merged, compared with a
  full run (must show 0 lost, absent and unrun).
