# ts-rust (aka tsc-rs)

I wanted to see if LLMs could port the TypeScript compiler, checker and lsp to Rust. Turns out they can.

It [cost over $420,000](#how-did-this-go) in tokens to do it, but you could probably have done it for ~$20k (see below).

## Benchmark: T3 Code with Effect diagnostics

Full type check of [T3 Code](https://github.com/pingdotgg/t3code) with Effect diagnostics on.\*
Time is the sum of the medians for its five checked projects. Lower is faster. Bar length is
in proportion to time. See [Benchmark: T3 Code](#benchmark-t3-code) for the method and the run
without Effect diagnostics.

| Checker | Time | Relative time | Speed vs tsc 7 |
| --- | ---: | --- | ---: |
| `tsc-rs --fast` | 9.77s | █ | 4.35× |
| `tsc-rs` | 14.30s | ██ | 2.98× |
| `tsc` 7 | 42.55s | █████ | baseline |
| `bun check` | 59.24s | ███████ | 0.72× |
| `tsc` 6 | 191.00s | ████████████████████████ | 0.22× |

\* `tsc-rs` is 0.6.0 with Effect diagnostics built in. `tsc` 7 is from `@effect/tsgo`. The
`bun check` time adds a separate `effect-tsgo diagnostics` run. `tsc` 6 uses the JavaScript
Effect plugin.

## Motivations

- Test model capabilities
- Make a fast TypeScript type checker
- Make a ts checker that can work in WASM with high performance
- Memes

## Warnings

**This is an early release.** It has 100% compatibility in every real world project we have
tested. It should work as a drop in
replacement for the vast majority of apps. See [Known problems](#known-problems).

Also worth mentioning: I've never read a line of this code.

## Install

Be warned, I have no idea if this will actually work.

```sh
npm install -D tsc-rs
npx tsc-rs -p tsconfig.json
```

## How did this go?

I used a lot of OpenAI models to try and complete this port. In total I did **over $400,000 in API priced tokens with GPT-5.6 Sol and GPT 6 Astra**. They wrote over 1.3m lines of Rust over multiple months of /goal loops and never got past like 84% compat.

When I saw how little my Claude Code limits were burning, I figured it'd be fun to throw Opus 5.5 at this. It had a working v0 in 10 hours.

I assumed it kept using the code the Codex models wrote. I was wrong. **Opus 5.5 started from scratch. It got further than Astra in 1/10th the time.**

I let it keep going, and it definitely did. Total token spend was **~$24,047 of API spend over 2 weeks**. I was using my Claude accounts, and it worked out to somewhere between **925% and 983% of my $200 plan weekly limits**.

Expensive, for sure, but not that bad considering how much work has gone into typescript-go.

# "The Slop Line"

Everything below this was written by my LLMs, not me. 

## What actually is this?

ts-rust is a direct port of Microsoft's native TypeScript compiler, which is written in Go
([microsoft/TypeScript](https://github.com/microsoft/TypeScript), formerly
[typescript-go](https://github.com/microsoft/typescript-go)). It keeps Go's algorithms and
behavior and has the same command line (`tsc`), language server and API.

## Install

```sh
npm install -D tsc-rs
npx tsc-rs -p tsconfig.json
```

`tsc-rs` takes the same options as `tsc`. The npm package is `tsc-rs` so that it does not clash
with the `typescript` package. To use tsc-rs as the `typescript` package (its `tsc` bin, its JS API
and the TypeScript 7 VS Code extension), install
[@tsc-rs/typescript](npm/typescript-readme.md) under that name (from the first release after
0.7.0):

```json
{ "devDependencies": { "typescript": "npm:@tsc-rs/typescript@<version>" } }
```

Each [release](https://github.com/pingdotgg/ts-rust/releases) also has a standalone archive per
platform: the `tsc` binary with the lib files next to it.

Platforms: Linux x64 and arm64 (static, any distribution), macOS arm64, and (from the first
release after 0.2.0) macOS x64, Windows x64 and Windows arm64.

To use it in VS Code, see [@tsc-rs/typescript](npm/typescript-readme.md#vs-code).

## Effect diagnostics

`tsc-rs` has the [Effect](https://effect.website) language service diagnostics built in (codes
377xxx), so an Effect project needs no second compiler. They come from the same check as the
TypeScript diagnostics, and the language server shows them too. They run only when the tsconfig has
the plugin, as with `@effect/language-service`:

```json
{ "compilerOptions": { "plugins": [{ "name": "@effect/language-service" }] } }
```

The rules, options and `@effect-diagnostics` comments are a port of
[Effect-TS/tsgo](https://github.com/Effect-TS/tsgo) 0.51.1. The language server also has its editor
features: the Effect hover (with the Layer summary), inlay hints, the auto-import style, quick fixes,
document symbols, completions and refactors. All the editor features of 0.51.1 are ported. The lightbulb,
the quick fix menu and the editor's Refactor menu show the Effect refactors: as in 0.51.1, the server lists
`refactor.rewrite` in its code action kinds.

The API stability rules of 0.51.1 read the `@stability` JSDoc tags of Effect 4.
`unstableApiUsage` (TS377136) and `experimentalApiUsage` (TS377135) report each use of an
`unstable` or `experimental` API, as warnings by default. `allowedUnstableApis` and
`allowedExperimentalApis` in the plugin entry allow a module (`effect/cli`) or one export
(`effect/cli/Command#make`). `apiStabilityLeak` (TS377137, off by default) is for library
maintainers. So a project on `effect@4` gets TS377136 warnings for its unstable imports, as with
effect-tsgo 0.51.1. See [the Effect diagnostics doc](docs/effect-diagnostics.md#api-stability-rules).

## Status

The port is pinned to one upstream revision, microsoft/TypeScript
[`3e9236361fa9`](https://github.com/microsoft/TypeScript/commit/3e9236361fa97a9e6c685f4833ac457ae2d8c50a)
(2026-10-19, TypeScript 7.1.0-beta; [UPSTREAM.md](UPSTREAM.md)), and compared with Go at that
revision. To compare, use `typescript@7.1.0-beta` (the same commit), not 7.0.x or
`@typescript/native-preview`. A difference that this build also shows is upstream behavior, and it
goes away when the port moves to a newer pin. tsc-rs 0.4.0 to 0.8.0 used the 2026-10-08 pin
(`typescript@7.1.0-dev.20261008.1`), and 0.3.0 and earlier the 2026-09-29 pin
(`typescript@7.1.0-dev.20260929.1`).

- **Same results.** TanStack Query core and Hono check with diagnostics identical to Go's. All
  186,890 tests pass (ported Go tests and our own). The language server and API answers match Go
  on the oracle test sets. See [Conformance](#conformance).
- **Faster.** On 60 open-source projects, type checking takes about half of Go's time (geometric
  mean). The npm packages from 0.2.0 on are built in CI with PGO but without BOLT (0.1.0 has
  neither), so they are slower than that measured build.
- **Real projects.** On 120 open-source repos, the command-line output differs from Go's only in
  the problems below and where Go's own output changes from run to run.

Four switches make some projects faster or use less memory. They are off by default.

- **`--fast`** (or `GOPORT_FAST=1`) turns on the last two switches and more checkers. It runs one
  checker per core, up to 16 and at most one per 32 source files, but never fewer than Go's 4
  (`--checkers N` still sets the count, and `tsc -b` keeps 4). The cores are the ones that the
  process can use (CPU affinity and cgroup limits), so N can be different on another machine.
  Each file goes to the checker that Go's split gives it, so the output of `--fast` is the same on
  every run and equals `tsgo --checkers N` for the chosen N, except where lazy members or the never
  order change the work (see their bullets below). Where Go's output depends on N (zod, below),
  the output of `--fast` can be different on another machine. `--fast` is an option of the
  compiler command line only (as in Go, `-b` must come first). `GOPORT_FAST=1` also turns on fast
  mode in the language server (`--lsp`) and the API server (`--api`). On 16 vCPU Blacksmith
  runners (tsc-rs 0.6.0, the benchmark below), VS Code checks in 2.64 s instead of 4.81 s,
  Sentry in 2.97 s instead of 5.30 s and T3 Code `apps/server` in 2.78 s instead of 3.60 s.
  Across the six apps below, `--fast` is 2.90x the speed of `tsc` 7 against 2.10x for the
  default mode (geometric mean). The gain is small or zero on small projects. CPU time went up
  as much as 2.2x and peak memory as much as 1.9x in our 16-CPU measurements. On all 16
  benchmark cases below, the diagnostics were the same as without `--fast`. On zod, TS2344 and
  TS5115 errors in a type cycle can appear or go away, as in Go with more checkers.

  `GOPORT_FAST_STEAL=1` (with `--fast`) also turns on work stealing: a checker that has checked
  its own files takes unchecked files from the checker with the most work left. It can cut wall
  time further on large projects and can cost time on small ones. A file can then go to a
  different checker in each run. So where Go's output depends on which files share a checker,
  the output can change from run to run (on zod it does).
- **`--checkers 8`** (Go's option; the default is 4) runs 8 checkers. On an 8-core host, VS Code
  checks in 30% less time and mui in 23% less, but T3 Code server gains nothing. CPU time goes
  up 13% to 51% and peak memory 7% to 34%. The output is the same as Go's with `--checkers 8`.
  Go's own output can change from 4 to 8 checkers (on zod, a TS5115 moves).
- **`GOPORT_LAZY_MEMBERS=1`** makes a member of an instantiated class or interface only when the
  checker needs it, as microsoft/TypeScript#64475 does (open, not in the pin). Peak memory goes
  down (zod 15%, T3 Code server 6.5%, Effect 4%). The time stays about the same. The
  `--extendedDiagnostics` Symbols count is then lower than Go's. Diagnostics and emit stay the
  same as Go's, except when the members of a type resolve inside their own resolution (a cycle):
  there, as with #64475 in Go, a TS2589, TS5114 or TS5115 can move to another node or go away.
  None of the tested projects has such a case.
- **`GOPORT_FAST_NEVER_ORDER=1`** checks some intersection types in a faster order (mongodb's
  test project: 8.9 s to 1.6 s). The `--extendedDiagnostics` counts are then lower than Go's.
  Diagnostics and emit stay the same as Go's, except near the TS2589 limit (5,000,000 type
  instantiations): there, a TS2589 that Go reports can go away.

## Conformance

Results of the last accepted revision, R222 (2026-10-22), against Go at the pin `3e9236361fa9`
(`typescript@7.1.0-beta`). Every revision runs these checks on our own machines
(Blacksmith testboxes and a LAN host), not in GitHub CI. We accept it only with no lost result.

| Check | Compared with Go | Result |
| --- | --- | --- |
| Compiler test corpus | 13,728 configurations of 12,932 `compiler` and `conformance` cases | Diagnostics 13,728 of 13,728 and emit 13,654 of 13,654 match byte for byte |
| Test suite | Go's tests and baselines (ported), and our own tests | 186,890 pass, 0 fail |
| np-suite | API client tests of `packages/typescript` | 966 of 966 pass |
| Language server | 257,903 requests in 6 batteries | 243,771 match, 0 differ |
| API | 112,740 requests in 13 batteries | 112,681 match, 4 differ |
| Gate | `tsc-rs` and `tsgo` on corpus samples, real projects, build mode and editor sessions | 3,951 of 3,952 match, 1 allowed |
| Query core and Hono | Full check (2 runs) and emit | Same as Go (Hono has 2 TS7031 errors in both) |

- The corpus is all of Go's `testdata/tests/cases/compiler` and `conformance`, except what Go
  skips too (44 files, 1,720 configurations, 8 emit checks). Types, symbols and source maps match.
- The test suite skips 3,637 names: the 1,728 above, 1,906 Go baseline files that no ported test
  compares (1,763 are fourslash, see the language server row) and 3 others.
- Requests in neither column: Go's own answer changed on a second run (365 LSP, 55 API), or the
  request had no item to resolve (13,767 LSP).
- The 4 API differences are in the list of file-system callbacks that `tsc-rs` makes, not in the
  answers. The allowed gate item is a resolution trace whose order changes between threaded Go runs.

## Benchmark: T3 Code

Full type check of [T3 Code](https://github.com/pingdotgg/t3code) at
[`cd41c4ad`](https://github.com/pingdotgg/t3code/tree/cd41c4ada0c70cc2eec95ecd7266f3dab010c58c),
with and without Effect diagnostics. Each time is the **sum of the medians** for `apps/server`,
`apps/web`, `apps/mobile`, `packages/client-runtime` and `packages/shared`, not the elapsed time
of the parallel benchmark workflow. Lower is faster.

**With Effect diagnostics**

Bar length is in proportion to time.

| Checker | Time | Relative time | Speed vs tsc 7 |
| --- | ---: | --- | ---: |
| `tsc-rs --fast` | 9.77s | █ | 4.35× |
| `tsc-rs` | 14.30s | ██ | 2.98× |
| `tsc` 7 | 42.55s | █████ | baseline |
| `bun check` | 59.24s | ███████ | 0.72× |
| `tsc` 6 | 191.00s | ████████████████████████ | 0.22× |

With Effect, `tsc-rs --fast` is fastest: 4.35× the Effect-patched `tsc` 7 and 6.1× Bun
followed by the separate Effect pass. On the smallest projects (`apps/mobile`,
`packages/shared`) `--fast` gains little or nothing.

<details>
<summary><strong>Without Effect diagnostics</strong></summary>

Bar length is in proportion to time.

| Checker | Time | Relative time | Speed vs tsc 7 |
| --- | ---: | --- | ---: |
| `bun check` | 6.24s | ██ | 3.12× |
| `tsc-rs --fast` | 6.91s | ██ | 2.81× |
| `tsc-rs` | 7.95s | ██ | 2.45× |
| `tsc` 7 | 19.45s | █████ | baseline |
| `tsc` 6 | 87.87s | ████████████████████████ | 0.22× |

`tsc-rs` is 0.6.0. Bun is fastest without Effect.

</details>

Diagnostics matter when comparing these times:

- Without Effect, `tsc-rs` and `tsc-rs --fast` match the pinned `tsc` 7 diagnostics exactly.
  Both Rust modes and `tsc` 6 report the same TS2322 in the server fixture. Bun reports no
  TypeScript errors.
- With Effect, `tsc-rs`, `tsc-rs --fast` and the native Effect compiler agree on
  **4,823 Effect diagnostics**, including 4,602 unstable-API warnings. The separate Effect
  pass after Bun reports the same rules and messages, with different path and severity
  formatting.
- The native Effect compilers also report 26 TS2790 diagnostics. `tsc-rs` additionally
  reports the TS2322 above, which the pinned `tsc` 7 also reports; the Effect compiler's older
  TypeScript version does not. The JavaScript Effect plugin uses a different rule set and
  reports 305 diagnostics in total. These tools do not all perform identical checks.

Measured on 2026-10-13 UTC, on Blacksmith `blacksmith-16vcpu-ubuntu-2404` runners
(16 AMD EPYC vCPUs, about 62 GiB RAM). Each project/mode has its own runner; all five checkers
for that case run on that same runner. Every cell has 5 runs after 1 warmup, with
`--noEmit --incremental false --composite false` and default thread counts. TypeScript 6
runs on Node 24.19.0 with a 16 GiB heap.

Without Effect, the configs have no Effect plugin and `tsc` 7 is 7.1.0-dev.20261008.1 (the
pin of 0.6.0). With Effect, `@effect/tsgo` 0.51.1 supplies its TypeScript 7.0.2 executable; the
JavaScript comparison uses TypeScript 6.0.3 and `@effect/language-service` 0.87.4. Bun is
1.4.4-canary.1+c69a636b9. `tsc-rs` is 0.6.0: a release PGO build without BOLT of the tag
source ([`a8c5b7063`](https://github.com/pingdotgg/ts-rust/commit/a8c5b7063eed1ba2929ee78a7dd3508ebd43a6f8)), the same build
process as the npm packages. `tsc-rs --fast` is the same binary with `--fast`.

[Results, samples, hashes and per-case metadata](docs/benchmarks/2026-10-13-blacksmith.json)
come from [this Blacksmith run](https://github.com/pingdotgg/ts-rust/actions/runs/38461793814),
which also measured the app benchmark below.
The [workflow](.github/workflows/bench-apps.yml) caches the verified compiler and prepared
inputs before running the comparisons in parallel. The T3 Code runner is
[scripts/bench-apps/t3code.sh](scripts/bench-apps/t3code.sh).

## Benchmark: real-world apps

Full type check of six open-source apps on Blacksmith `blacksmith-16vcpu-ubuntu-2404`
runners (16 AMD EPYC vCPUs, about 62 GiB RAM). Each app has its own runner; all five checkers
for an app ran on that runner. Lower times are faster.

| App | `tsc` 6 | `tsc` 7 | `tsc-rs` 0.6.0 | `tsc-rs` 0.6.0 `--fast` | `bun check` |
| --- | ---: | ---: | ---: | ---: | ---: |
| [VS Code](https://github.com/microsoft/vscode/tree/3f07e1aba32acacb8b08ae91bfdc954b580ad1fd) | 91.75s | 9.57s | 4.81s | 2.64s | 2.32s |
| [Sentry (frontend)](https://github.com/getsentry/sentry/tree/8294650589dbd26f230c73f4ab26b62a68aede8f) | 80.27s | 13.49s | 5.30s | 2.97s | 6.93s |
| [Playwright](https://github.com/microsoft/playwright/tree/d469960fdfc461e2d5795a3fa48a58a52a91ecaf) | 6.33s | 0.82s | 0.41s | 0.33s | 0.27s |
| [Excalidraw](https://github.com/excalidraw/excalidraw/tree/53973c3a423fbd75a4ce68107786b4fcb90e4968) | 6.85s | 1.42s | 0.62s | 0.48s | 0.24s |
| [TypeORM](https://github.com/typeorm/typeorm/tree/c64a1f052fc39f6688b6b73b83d065d7147ba8bb) | 5.60s | 0.70s | 0.39s | 0.27s | 0.15s |
| [tRPC (server package)](https://github.com/trpc/trpc/tree/d756e591a5e37ef20b8d75ecd4d736c195497289) | 1.55s | 0.17s | 0.09s | 0.09s | 0.10s |
| **Geometric mean vs `tsc` 6** | **1.0×** | **7.3×** | **15.3×** | **21.2×** | **24.0×** |

`tsc-rs` 0.6.0 is **2.10× faster than `tsc` 7** across these apps (geometric mean).
The corresponding figures are 2.90× for `tsc-rs --fast` and 3.29× for Bun. Bun is fastest
on four apps; `tsc-rs --fast` is fastest on Sentry and `tsc-rs` on tRPC, where `--fast`
gains nothing (0.088 s against 0.085 s).

Diagnostics differ between compilers. `tsc-rs`, `tsc-rs --fast` and the pinned `tsc` 7
produce identical full diagnostics: 10 on VS Code, 2 on Sentry, and none on the other four
apps. `tsc` 6 reports 9 on VS Code and none on the others. Bun reports none on any app.
The time comparisons do not imply identical checking behavior for Bun or `tsc` 6.

Measured on 2026-10-13: median of 5 runs after 1 warmup, `--noEmit --incremental false`,
default thread counts, and direct native binaries. `tsc` 6 runs on Node 24.19.0 with a 16 GiB
heap. Versions: TypeScript 6.0.3 and 7.1.0-dev.20261008.1 (the pin of 0.6.0), `tsc-rs` 0.6.0,
and Bun 1.4.4-canary.1+c69a636b9. The `tsc-rs` build is release PGO without BOLT, static
musl, built from the tag source with the same training process as the release workflow.

[Results, samples, hashes and input revisions](docs/benchmarks/2026-10-13-blacksmith.json)
come from [this Blacksmith run](https://github.com/pingdotgg/ts-rust/actions/runs/38461793814),
the same run as the T3 Code benchmark above. Every job passed.

The pinned app inputs use the same preparation as the earlier benchmark:

- Excalidraw: remove `baseUrl`, which native TypeScript no longer supports.
- TypeORM: use `nodenext` module resolution.
- VS Code: install the Electron typings normally supplied by its postinstall.
- Playwright: generate the injected sources normally supplied by its build.

The scripts are in [scripts/bench-apps](scripts/bench-apps). The
[benchmark workflow](.github/workflows/bench-apps.yml) caches the verified compiler and each
prepared project, then runs each comparison on a separate runner of the same size. Each
comparison keeps all compilers on one runner and saves their full output and exit codes.

### Why `bun check` is faster

The lead of `bun check` is all in the check step. Its checkers share one type store on all cores.
`tsc` 7 runs 4 checkers, and each one makes its own copies of the types it needs: on T3 Code
`apps/server`, 57% of the types. `tsc-rs` keeps the `tsc` 7 model, because its goal is output that
is identical to `tsc` 7. More checkers or one shared store would change that output, so `tsc-rs`
does not copy them. `--checkers 8` uses more cores with output identical to `tsc` 7 at
`--checkers 8` (see [Status](#status)).

Per thread, `tsc-rs` is not slower. On T3 Code it was 1.5× to 7× faster per thread than
`bun check`, and on 28 other open-source repos the two did about the same check work per thread.
`tsc-rs` also loads and parses files as fast as `bun check` or faster.

The output is not the same. `bun check` gave output identical to `tsc` 7 on 37 of 60 open-source
repos, and `tsc-rs` on all 60. `bun check` also reports errors that `tsc` 7 does not, for example
TS2749 on T3 Code `apps/server`. These numbers are from our studies of 2026-10-07 and 2026-10-08,
with `tsc-rs` 0.1.0 and R179 on Linux.

## Known problems

- In some monorepos, the source files of a workspace package are reachable both through
  `node_modules` and through a direct import. There, `tsc-rs` can write output for more of those
  files than `tsc` does, and report TS6059 (file is not under `rootDir`) for them. `tsc` decides
  this by timing, so its own result changes between runs. `tsc-rs` gives the same result in every
  run (the result of TypeScript 6).
- In `tsc -b`, when one project imports the output of another project without a project reference,
  the two projects can build at the same time. Then `tsc` reads each output file before or after the
  other project writes it. This is timing, so the result of `tsc` changes between runs, file by
  file. `tsc-rs` makes the programs one at a time and writes the outputs of a project together when
  that project ends. So it can read old or missing output where `tsc` reads new output, or the
  reverse. Often the only difference is the file hashes in the reading project's `.tsbuildinfo`.
  Sometimes it is an error, such as TS2305, TS2307 or TS2322. Add the reference to fix it.
- In the editor, memory grows slowly during long edit sessions: 3.1 to 4.6 MiB per 1,000 edits in
  our 10,000-edit sessions, where `tsc` grows 2.6 to 2.8. It starts within 10% of `tsc`'s, and from
  about edit 20 it stays below `tsc`'s. After 10,000 edits (two sessions each) it was 232 and 249
  MiB against `tsc`'s 291 and 305 MiB on Query core, and 314 and 324 MiB against 417 and 422 MiB on
  Hono.
- `tsc-rs --version` prints the TypeScript version that it ports (7.1.0-beta), not the npm
  version. The compiler matches `typesVersions` against it.

## Development

`crates/ts_goport` is the compiler. It has three parts crates in `crates/ts_goport/parts`:
`goport_core` (everything below the language service), `goport_util` and `goport_lsproto`. Their
files stay in `crates/ts_goport/src`. It uses the lib files in `crates/ts_goport/libs`.
`tools/ts_ast_codegen` generates `crates/ts_goport/src/astdata`, and
`tools/ts_diagnostics_codegen` generates `crates/ts_goport/src/diagnostics/catalog.rs` and
`crates/ts_goport/src/diag.rs`. `crates/ts_wasm` is the WebAssembly build
([npm/wasm](npm/wasm/README.md)). `crates/ts_napi` is the Node-API addon behind the TypeScript 6.0
JavaScript API ([npm/typescript-api](npm/typescript-api/README.md)).

```sh
./scripts/run-cargo-capped.sh build --release -p ts_goport --bins
./scripts/verify.sh
```

The bins are `goport` (type check) and `tsgo` (the Go `tsgo` command line). The Go baseline tests
run with
`TS_GO_REPO=/path/to/typescript-go ./scripts/run-cargo-capped.sh test -p ts_goport --test go_baselines`.

- Port rules: [crates/ts_goport/PORTING.md](crates/ts_goport/PORTING.md)
- Measurement and gate scripts: [scripts/goport](scripts/goport/README.md)
- npm packages and releases: [npm/README.md](npm/README.md)
- Typechecker work rules: [AGENTS.md](AGENTS.md), the
  [accountability rules](docs/typechecker-accountability.md) and the
  [saved state](docs/typechecker-state/current.json)
- How the project started: [docs/history.md](docs/history.md)

### Develop on Windows

CI builds and tests `x86_64-pc-windows-msvc` (the Windows job of
[ci.yml](.github/workflows/ci.yml)).

- Install Rust with the MSVC toolchain (rustup's default on Windows) and the Visual Studio Build
  Tools (C++ workload).
- Clone with LF line endings: `git clone -c core.autocrlf=false ...`. `.gitattributes` forces LF
  for `*.rs`, `*.sh` and the bundled libs (the lib snapshot tests hash their bytes), but the test
  fixtures need LF too.
- Use `cargo` directly. `scripts/run-cargo-capped.sh` needs `systemd-run`.
- Run the tests with `RUST_TEST_THREADS=1`, as CI and `goport-tests.sh` do. The Go baseline tests
  need `TS_GO_REPO` and Node, as on Linux (see the CI job for the setup).
- Two `fswatch_watcher` tests (`test_subscribe_symlink_create` and `_delete`) make file symlinks,
  which need Developer Mode (Settings, System, For developers) or an elevated shell. Without
  either they fail with OS error 1314.
- The scripts are bash. The release scripts (`crates/ts_goport/scripts/build-pgo.sh`,
  `copy-libs.sh`, `scripts/goport/pgo-train.sh`) and `scripts/goport/win-go-compare.sh` run in
  Git Bash. The other tools in `scripts/` (the revision pipeline, the gate, `remote.sh`) are for
  the Linux hosts only.

## Releases

Push a tag `v<version>` (for example `v0.1.0`). The
[release workflow](.github/workflows/release.yml) builds, packs and tests the packages, publishes
them to npm and creates a GitHub release. A stable version goes to the dist-tag `latest`, and a
prerelease version (`v0.2.0-beta.1`) to `next` and a GitHub prerelease. See
[npm/README.md](npm/README.md#tsc-rs-releases).

## License

[MIT](LICENSE). The port keeps the licenses and notices of the code it ports: TypeScript
(Apache-2.0) and parts of the Go standard library (BSD-3-Clause). See [NOTICE.md](NOTICE.md).
