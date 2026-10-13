# TS 6.0.3 test harness for a `typescript` library

Runs TypeScript 6.0.3's own tests, unmodified, against a library directory: any package directory
with `lib/typescript.js`, such as `npm/typescript-api` (tsc-rs) or a stock `typescript@6.0.3`
install (the control).

```sh
scripts/typescript-api/ts6-tests/run.sh help
scripts/typescript-api/ts6-tests/run.sh setup        # optional: run does it when needed
scripts/typescript-api/ts6-tests/run.sh run npm/typescript-api target/typescript-api/runs/pkg --suite all
scripts/typescript-api/ts6-tests/run.sh compare target/typescript-api/ts6-baselines/stock-6.0.3 target/typescript-api/runs/pkg
scripts/typescript-api/ts6-tests/run.sh triage target/typescript-api/runs/pkg   # compiler failures against TS 7's baselines
node scripts/typescript-api/ts6-tests/repro.mjs list                            # minimal repros of the triage clusters
```

## Build (cached)

`setup` (and the first `run`) clones microsoft/TypeScript `v6.0.3` (commit `050880ce59e3`, checked)
into `target/typescript-api/ts60`, runs `npm ci --ignore-scripts`, then `hereby tests --no-typecheck`,
`hereby dts` and `hereby generate-types-map` (the inputs of TS 6.0's own `runtests`), then
`build-swapped.mjs`. Stamps in `ts60/.ts6-tests/` skip finished steps; the swapped bundle is rebuilt
when `build-swapped.mjs` changes. The whole setup takes about a minute.
`TS6_TESTS_CACHE` and `TS6_TESTS_GIT` override the cache directory and the clone source.

## How the swap works

- `build-swapped.mjs` bundles `src/testRunner` with esbuild exactly like `hereby tests` (cjs,
  node14.17, no tree shaking, packages external), except that every product namespace barrel
  (`src/{compiler,services,jsTyping,server,typingsInstallerCore,deprecatedCompat}/_namespaces/ts.js`
  and `ts.server.js`) becomes `shim.cjs` (or its `.server`). The harness and test sources are
  bundled from the checkout unchanged; no compiler source is in `built/local/run-swapped.js`.
- `shim.cjs` is `require(<TS6_TESTS_LIBRARY>/lib/typescript.js)`. The library is the whole TS 6.0.3
  bundle (internals included, plus hooks in our package), so the tests read internals from the same
  object as the public API; there is no second TypeScript copy.
- Lib files stay as TS 6.0 does them: the harness mounts the checkout's `built/local/lib.*.d.ts` at
  `/.ts/` for test programs. `built/local/typescript.d.ts` is the library's own `lib/typescript.d.ts`
  (read by the Public APIs test and the APISample compiler cases); `typescript.internal.d.ts` comes
  from the TS 6.0 build (APILibCheck).
- Each mocha process runs in its own workspace `<out-dir>/ws/<name>/`: symlinks into the cache plus
  its own `test.config`, `tests/baselines/local/` (baselines that did not match land here) and a
  hard link of the bundle in `built/local/`. TS 6.0 resolves `tests/` against the working directory and
  `built/local/` against the bundle's directory, so runs and shards never share mutable files.
- `reporter.cjs` writes one JSON line per test as it finishes (a crashed process keeps its finished
  results; a missing end record marks the crash). Tests skipped by a failed `before` hook are
  recorded as failed, not left out.
- Self-check done once (2026-10-13): a fake library without `transpileModule` and with a throwing
  `createProgram` failed 1,090 of the 2,578 tier1 tests (398 with the fake error), so the tests do
  reach the library.

## Suites

| Suite | What | Tests | Wall, typescript@6.0.3 (dev host) |
|---|---|---|---|
| `apisamples` | The 9 APISample programs of `tests/cases/compiler/APISample_*.ts` (emitted JS taken from `tests/baselines/reference`), run with `node_modules/typescript` linked to the library; normalized output and written files are diffed against `apisamples/expected/` (recorded from typescript@6.0.3 with this harness). | 9 | 14 s |
| `tier1` | First-adoption selection (mocha grep in `suites.json`): Public APIs, regExpScannerRecovery, evaluation, Printer/Factory/Transform APIs, customTransforms, config parsing, programApi, moduleResolution, services Transpile/PreProcessFile/Colorization/DocumentRegistry/languageService, transpile runner. | 2,578 | 9-14 s |
| `compiler` | compiler + conformance runners, only `Correct errors for` and `Correct JS output for` (every test case and configuration), sharded over `--jobs` mocha processes by TS 6.0's own `shards`/`shardId` test config. | 29,946 | 4 jobs: 119 s (load ~10), 200-250 s (load ~20), 5.6 GiB peak; 1 job: 595 s (load ~20), 1.9 GiB |
| `all` | The three above, in that order. | 32,533 | |

The watch samples wait for output markers ("Watching for file changes", "Emitting") instead of fixed
delays: edit after the first compile, stop with SIGINT one second after the next one
(`TS6_TESTS_WATCH_TIMEOUT`, default 60 s per wait; `TS6_TESTS_WATCH_SETTLE`, 1 s;
`TS6_TESTS_SAMPLE_TIMEOUT`, 120 s for the other samples).

Which suites exercise programs, the checker and emit (and therefore tsc-rs once the hooks land):

- `compiler`: every test builds a program, checks it and emits through the public `createProgram`
  path (`compilerImpl.ts`); "Correct errors" compares diagnostics with the TS 6.0 `.errors.txt`
  baseline, "Correct JS output" the emit with the `.js` baseline. This is the main tsc-rs suite.
- `tier1`: evaluation (compile and run), Transpile and the transpile runner (`transpileModule`,
  emit), programApi, moduleResolution, customTransforms/TransformAPI (custom transformers on
  `program.emit`), languageService and DocumentRegistry (programs through the LanguageService),
  builder (builder programs), and the Public APIs `getTypeAtLocation` and `createProgram` tests.
  The rest of Public APIs (d.ts check, tokens, JSDoc, modifiers), regExpScannerRecovery, config,
  Printer/Factory and PreProcessFile/Colorization are syntax or config level and stay TS 6.0's own JS.
- `apisamples`: compile (createProgram, getPreEmitDiagnostics, emit), jsdoc (checker symbols),
  transform (`ts.transpile`, through the `transpileWorker` hook), the watch samples (watch program
  and builder, emitted files), watcher (LanguageService emit). linter is syntax level; parseConfig
  stops on the sample's own TypeError before it builds a program (TS 6.0.3 does the same).

Expected failures once tsc-rs answers: TS 7 removed settings that many TS 6.0 tests use (ES5
target, AMD/System/UMD, Classic/Node10, `outFile`, `baseUrl`, `downlevelIteration`).
`ts7-removed.json` lists them per tier1 test (see Compare).

## Outputs (`<out-dir>`)

- `results.json`: array, one entry per test: `suite`, `title` (mocha full title; APISamples are
  `APISample_<name>`), `top` (top-level describe), `state` (`pass`, `fail`, `pending`), `error`
  (first line).
- `summary.md`: counts per suite (with wall time, exit codes, crashes), per top-level describe, per
  compiler check, and the first failures.
- `meta.json`: library path and content fingerprint (`package.json`, `lib/typescript.{js,d.ts}`,
  `src/**`, `native/**`; checked again after the run), the `TSRS_*` environment, TS 6.0 commit, node,
  per-process exit code, time and local-baseline count, and the systemd unit's memory peak.
- `harness.log` (ends with `DONE` or `FAIL rc=N`), `raw/<process>.{jsonl,log}` (reporter lines and
  mocha output), `ws/<process>/tests/baselines/local/` (mismatched baselines),
  `apisamples/out/` (sample outputs) and `apisamples/work-*` (sample work dirs).

## Baselines on this host (2026-10-13, not committed)

- `target/typescript-api/ts6-baselines/stock-6.0.3`: typescript@6.0.3, `--suite all`: 32,533 of 32,533 pass.
- `target/typescript-api/ts6-baselines/pkg-nohooks`: `npm/typescript-api` without `TSRS_DEV_HOOKS`
  (no hooks installed): 32,533 of 32,533 pass; `compare` against the stock run: all retained, exit 0.

## Compare

`compare <base-out> <new-out>` prints per suite: base and new pass counts, retained, lost (base pass,
new fail or pending), gained, absent (in base, not run in new; base passes in parentheses),
still failing, and added tests, then up to `--limit` names per category with the new error line.
`--json FILE` writes every name. Exit 1 when any base pass is lost or absent. Only suites present in
both runs are compared; the others are listed as unrun. Tests that pass a setting TS 7 removed are
marked `[TS7-removed: ...]` and counted in the "Lost (TS7-removed)" column: `ts7-removed.json` holds
the 689 tier1 tests that a probe run flagged (the tier1 tests on a typescript@6.0.3 wrapper that
recorded the options each test passed in); 610 of them are services Transpile tests, whose harness
defaults to ES5.

## Triage

`triage <out-dir> [--ts7 DIR] [--limit N]` classifies every failing compiler-suite test of a run
(`results.json` and the local baselines under `ws/compiler-*/tests/baselines/local/`) against TS 7's
own baseline of the same name: the Go harness's `testdata/baselines/reference/{compiler,conformance}/`
in the `tsc/` directory DIR of a microsoft/TypeScript checkout at the tsc-rs pin (`TS6_TESTS_TS7`
overrides). The default, `target/typescript-api/ts71-baselines/tsc`, is a shallow sparse fetch of
fed0bf24149f with only the paths triage reads (`tsc/internal/testrunner` and the compiler and
conformance baselines and cases), about 350 MB, made on first use. tsc-rs is a
port of that code, so the package's answers should equal those baselines. It writes
`<out-dir>/triage.json` (every failing test with its class, reason, TS 7 case and baseline paths)
and `<out-dir>/triage.md` (counts per class and check, the D list grouped by cause, the suspicious
passing tests), reads nothing else outside the run, the TS 6.0 cache and DIR, and can be run again.

Classes, in this order of precedence:

- **A**, equals TS 7's baseline after the normalizations below: expected TS 7 behavior. Also when
  TS 7 edited only the case's directives (`directives-changed`), and when only the order of the
  emitted files differs (the Go harness lists some declaration files in another order than the tsc-rs
  CLI, the package and TS 6.0 write them; nodeNextPackageSelfNameWithOutDirDeclDirCompositeNestedDirs).
- **B**, uses a setting TS 7 removed, read from the TS 6.0 case's directives, its `tsconfig*.json` /
  `jsconfig*.json` units and the variation in the test name: target es3/es5, module
  none/amd/umd/system, `outFile`/`out`, moduleResolution classic/node10/node, `baseUrl`,
  `downlevelIteration` (any value), `esModuleInterop`/`allowSyntheticDefaultImports`/`alwaysStrict`
  false (TS 7: `compiler/program.go` "Removed in TS7"). For errors tests, `ts6Difference` says whether
  ours differs from TS 6.0's baseline only in removed/deprecated option diagnostics (TS5101, 5102,
  5107, 5108). `ts7` says what TS 7 does with the configuration (below).
- **C**, no comparable TS 7 baseline: `case-removed` (no such case in `testdata/tests/cases`),
  `case-skipped` (`skippedTests` in `internal/testrunner/compiler_runner.go`), `emit-skipped` (JS
  output of a case in `skippedEmitTests`), `variant-dropped` (the TS 7 case no longer lists this
  value or no longer varies the option), `variant-skipped` (the Go harness skips or rejects the
  configuration: `harnessutil.go` `SkipUnsupportedCompilerOptions`), `case-changed` (TS 7 edited the
  case's text, not only directives), or a directive value that ends with `;` (TS 6.0's harness reads
  `@declaration: true;` as false, the Go harness as true).
- **D**, differs from TS 7's baseline: a candidate bug in the package (or in tsc-rs). The cause
  label lists, for errors, diagnostics extra, missing, at another position or with another text (by
  code), else a related-information or file-section difference; for JS, emitted files extra, missing
  or different (by kind: js, d.ts, source map, declaration diagnostics). "output shows the real
  working directory" marks outputs that contain the repository or run path.

Passing side: a passing test equals TS 6.0's baseline; when TS 7 runs the same configuration and its
baseline differs from TS 6.0's (after normalization), the test is listed in `passingSuspicious`
with a reason (removed setting, case changed, TS 7's own known bug from `submoduleTriaged.txt`, or
"TS 7 baseline differs"). `ts7-vs-ts6` marks baselines that TS 7 lists in `submoduleAccepted.txt` or
`submoduleTriaged.txt`.

Normalizations: only what the two harnesses format differently, each found in real pairs.

- Case files are read without a byte order mark (TS 6.0's `IO.readFile` drops it; 844 cases start with one).
- TS 7 renamed promoted cases that collided with its own (`testdata/promotedTestCollisions.txt`,
  `X.ts` -> `X_promoted.ts`): the TS 7 baseline of `X_promoted` is used, with `X_promoted` read as `X`.
  The repository merge also rewrote `github.com/microsoft/typescript-go/` links in promoted cases to
  `github.com/microsoft/TypeScript/tsc/`; TS 7's case and baseline text are read with the old link.
- `==== ./a.ts (N errors) ====`: the Go harness drops a leading `./` from unit names and joins `//`
  (TS 6.0 prints the `@filename` as written). Seen in typeSatisfactionWithDefaultExport,
  jsDeclarationEmitExportedClassWithExtends.
- `@pretty` baselines: TS 6.0's `formatDiagnosticsWithColorAndContext` (JS, which the package keeps)
  against the Go diagnosticwriter. TS 7 puts a related message on its location line (TS 6.0 after the
  snippet), ends every diagnostic block with an empty line, draws one `~` for an empty span, ends the
  summary with an empty line and writes CRLF in the "Errors  Files" table (TS 6.0 LF). Both sides are
  rewritten to TS 6.0's layout with LF, and a lone `~` in a snippet is dropped on both sides (the
  plain section of the baseline still checks spans). Seen in duplicateIdentifierRelatedSpans1-7,
  manyCompilerErrorsInTheTwoFiles, prettyContextNotDebugAssertion.
- Error markers under a source line with astral characters: TS 6.0 draws them in UTF-16 units, the
  Go harness in code points (`tsbaseline/error_baseline.go`); TS 6.0-side markers are redrawn in code
  points. Seen in regularExpressionCharacterClassRangeOrder, unicodeEscapesInNames02.
- Diagnostics without a file come first in both harnesses, ordered by code in TS 6.0's
  `compareDiagnostics` but by their internal location first in TS 7's `ast.CompareDiagnostics`; both
  sides order them by code, in the summary and in the `!!! error` block before the first file.
  Seen in arrayIterationLibES5TargetDifferent (TS5053 before or after TS2318).
- Lib files need none: both harnesses print `lib.es5.d.ts:--:--`; line endings are CRLF on both sides.

## Repros

`node scripts/typescript-api/ts6-tests/repro.mjs list | <case>... | all [--pkg DIR] [--ts6 DIR] [--tsgo BIN]`
runs a tiny program per triage cluster through the package (default `npm/typescript-api`),
typescript@6.0.3 (default `npm/typescript-api/.cache/typescript-6.0.3`, which
`npm/typescript-api/scripts/build.mjs` unpacks) and the `tsgo` command line (default
`target/release/tsgo`), and prints the diagnostics and declaration files of each next to TS 7's
expected answer. Fixtures go to `target/typescript-api/ts6-repro/<case>/`. Hosts: `disk`
(`ts.createCompilerHost`), `memory` (an in-memory CompilerHost under `/ts6-repro-virtual/`, like the
TS 6.0 harness's virtual file system), `other-cwd` (relative names and `getCurrentDirectory()` point at
the fixture while the process runs elsewhere).

## Resources

`run.sh setup|run` runs the harness in its own systemd user unit (`MemoryMax=12G`, no swap;
`TS6_TESTS_MEMORY_MAX` changes the cap, `TS6_TESTS_SYSTEMD=0` runs it in place). The unit gets PATH,
HOME and the `TS6_TESTS_*` and `TSRS_*` variables only. `--jobs` is at most 4. `--detach` starts the
unit and returns; poll `<out-dir>/harness.log` for `DONE` or `FAIL`.
