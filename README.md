# ts-rust (aka tsc-rs)

I wanted to see if LLMs could port the TypeScript compiler, checker and lsp to Rust. Turns out they can.

It [cost over $420,000](#how-did-this-go) in tokens to do it, but you could probably have done it for ~$20k (see below)

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

Expensive, for sure, but not that bad considering how much work has went into typescript-go.

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
with the `typescript` package. Each [release](https://github.com/pingdotgg/ts-rust/releases) also
has a standalone archive per platform: the `tsc` binary with the lib files next to it.

Platforms: Linux x64 and arm64 (static, any distribution), macOS arm64, and (from the first
release after 0.2.0) macOS x64, Windows x64 and Windows arm64.

To use it in VS Code, see the [npm package README](npm/tsc-rs-readme.md#vs-code).

## Effect diagnostics

`tsc-rs` has the [Effect](https://effect.website) language service diagnostics built in (codes
377xxx), so an Effect project needs no second compiler. They come from the same check as the
TypeScript diagnostics, and the language server shows them too. They run only when the tsconfig has
the plugin, as with `@effect/language-service`:

```json
{ "compilerOptions": { "plugins": [{ "name": "@effect/language-service" }] } }
```

The rules, options and `@effect-diagnostics` comments are a port of
[Effect-TS/tsgo](https://github.com/Effect-TS/tsgo) 0.46.1. The editor features of the language
service (quick fixes, refactors, hover, completions) are not ported.

## Status

The port is pinned to one upstream revision, microsoft/TypeScript
[`673a5f17d713`](https://github.com/microsoft/TypeScript/commit/673a5f17d713bdc8c7185f18a9c11e3c4ac5d781)
(2026-09-29, TypeScript 7.1.0-dev; [UPSTREAM.md](UPSTREAM.md)), and compared with Go at that
revision. To compare, use `typescript@7.1.0-dev.20260929.1`, not 7.0.x or
`@typescript/native-preview`. A difference that this build also shows is upstream behavior, and it
goes away when the port moves to a newer pin.

- **Same results.** TanStack Query core and Hono check with diagnostics identical to Go's. All
  181,711 ported Go tests pass. The language server and API answers match Go on the oracle test
  sets.
- **Faster.** On 60 open-source projects, type checking takes about half of Go's time (geometric
  mean). The npm packages from 0.2.0 on are built in CI with PGO but without BOLT (0.1.0 has
  neither), so they are slower than that measured build.
- **Real projects.** On 120 open-source repos, the command-line output differs from Go's only in
  the problems below and where Go's own output changes from run to run.

## Benchmark: T3 Code

Full type check of [T3 Code](https://github.com/pingdotgg/t3code), compared with `tsc` 6, `tsc` 7
and the new `bun check` in Bun. T3 Code uses Effect, so there are two cases: without the Effect
diagnostics and with them. Each time is the sum for the five T3 Code projects. Lower is faster.

**Without Effect diagnostics**

| Checker     |   Time | vs `tsc` 6 | vs `tsc` 7   |                                            |
| ----------- | -----: | ---------: | ------------ | ------------------------------------------ |
| `bun check` |  4.07s |      15.4× | 3.95× faster | `█`                                        |
| `tsc-rs`    |  5.27s |      11.9× | 3.05× faster | `██`                                       |
| `tsc` 7     | 16.10s |       3.9× | baseline     | `█████`                                    |
| `tsc` 6     | 62.63s |   baseline | 3.89× slower | `██████████████████`                       |

**With Effect diagnostics**

| Checker                                         |    Time | vs `tsc` 6 | vs `tsc` 7 + Effect |                                            |
| ----------------------------------------------- | ------: | ---------: | ------------------- | ------------------------------------------ |
| `tsc-rs` (Effect built in)                      |   8.35s |      16.6× | 2.52× faster        | `██`                                       |
| `tsc` 7 + `@effect/tsgo`                        |  21.07s |       6.6× | baseline            | `██████`                                   |
| `bun check`, then `effect-tsgo diagnostics`     |  37.60s |       3.7× | 1.78× slower        | `███████████`                              |
| `tsc` 6 + `@effect/language-service`            | 138.63s |   baseline | 6.58× slower        | `████████████████████████████████████████` |

`bun check` is the fastest when you do not need the Effect diagnostics: 1.29× faster than
`tsc-rs` in total. On `packages/client-runtime`, `tsc-rs` is a little faster. `bun check` does not
have the Effect diagnostics, so an Effect project needs a second pass. `tsc-rs` gets them from its
one check, which makes it 4.5× faster than `bun check` plus `effect-tsgo diagnostics`. See
[Why `bun check` is faster](#why-bun-check-is-faster).

Errors. `tsc-rs`, `tsc` 7 + `@effect/tsgo` and the `effect-tsgo diagnostics` pass report the same
221 Effect diagnostics. `tsc` 6 uses the JavaScript Effect plugin
(`@effect/language-service` 0.87.4), which has a different rule set: it reports 287 on
`apps/server` where the others report 177. `tsc-rs` and `tsc` 6 report one more error, TS2322 in
`apps/server/scripts/record-pi-rpc-replay-fixture.ts`. TypeScript 7.1.0-dev reports it too, and
[pingdotgg/t3code#16704](https://github.com/pingdotgg/t3code/pull/16704) fixes it.

How it was measured: the same machine and method as the real-world apps below, with
`--composite false` added (`apps/web` is composite). T3 Code at
[`cd41c4ad`](https://github.com/pingdotgg/t3code/tree/cd41c4ada0c70cc2eec95ecd7266f3dab010c58c),
projects `apps/server`, `apps/web`, `apps/mobile`, `packages/client-runtime` and
`packages/shared`. Without Effect, the configs have no Effect plugin. With Effect, `tsc` 7 is the
Effect-patched 7.0.2 from `@effect/tsgo` 0.46.1, and `tsc` 6 is 6.0.3 patched with
`@effect/language-service`. The script is
[scripts/bench-apps/t3code.sh](scripts/bench-apps/t3code.sh). The `tsc-rs` switch in T3 Code is
[pingdotgg/t3code#16704](https://github.com/pingdotgg/t3code/pull/16704).

## Benchmark: real-world apps

Full type check of six open-source apps with `tsc` 6 (the JavaScript compiler), `tsc` 7 (the Go
compiler), `tsc-rs` and `bun check`. The multiplier is the speedup over `tsc` 6. Lower times are
faster.

| App | Lines checked | `tsc` 6 | `tsc` 7 | `tsc-rs` | `bun check` |
| --- | ---: | ---: | ---: | ---: | ---: |
| [VS Code](https://github.com/microsoft/vscode/tree/3f07e1aba32acacb8b08ae91bfdc954b580ad1fd) | 3.75M | 54.56s | 6.84s (8.0×) | 3.49s (15.6×) | 1.62s (33.7×) |
| [Sentry](https://github.com/getsentry/sentry/tree/8294650589dbd26f230c73f4ab26b62a68aede8f) (frontend) | 2.11M | 58.76s | 7.90s (7.4×) | 3.76s (15.6×) | 3.14s (18.7×)\* |
| [Playwright](https://github.com/microsoft/playwright/tree/d469960fdfc461e2d5795a3fa48a58a52a91ecaf) | 585k | 4.48s | 0.66s (6.8×) | 0.29s (15.3×) | 0.18s (25.0×) |
| [Excalidraw](https://github.com/excalidraw/excalidraw/tree/53973c3a423fbd75a4ce68107786b4fcb90e4968) | 449k | 5.32s | 0.80s (6.7×) | 0.58s (9.2×) | 0.18s (29.0×) |
| [TypeORM](https://github.com/typeorm/typeorm/tree/c64a1f052fc39f6688b6b73b83d065d7147ba8bb) | 386k | 3.86s | 0.55s (7.0×) | 0.31s (12.4×) | 0.19s (20.0×) |
| [tRPC](https://github.com/trpc/trpc/tree/d756e591a5e37ef20b8d75ecd4d736c195497289) (server package) | 209k | 1.10s | 0.16s (6.8×) | 0.08s (13.6×) | 0.12s (9.1×)\* |
| **Geometric mean** | | | **7.1×** | **13.4×** | **20.9×** |

\* `bun check` reports errors that no other checker reports: 3 on Sentry and 2 on tRPC.

Compared with `tsc` 7, `tsc-rs` is 1.89× faster and `bun check` is 2.95× faster (geometric
means). `bun check` is the fastest on every app except tRPC. Compared with `tsc-rs` 0.2.0,
`bun check` is 1.56× faster (geometric mean; it was 1.83× with 0.1.0):

| App | `bun check` vs `tsc-rs` |
| --- | --- |
| VS Code | 2.16× faster |
| Sentry | 1.20× faster |
| Playwright | 1.63× faster |
| TypeORM | 1.61× faster |
| Excalidraw | 3.16× faster |
| tRPC | 1.48× slower |

Each config checks with 0 errors under `tsc` 7.0.2. The other differences:

- `tsc-rs` reports 10 errors on VS Code and 2 on Sentry. TypeScript 7.1.0-dev (`typescript@next`)
  reports the same errors, line for line. `tsc-rs` ports a 7.1 dev revision, which has checks that
  7.0.2 does not have.
- `tsc` 6 reports 9 errors on VS Code.

How it was measured: Apple M4 Pro (12 cores, 48 GB), macOS 26.5.1. [hyperfine](https://github.com/sharkdp/hyperfine),
median of 5 runs after 1 warmup run, with `--noEmit --incremental false`. Each checker uses its
default thread count. `tsc` 7 and `tsc-rs` run as native binaries, without the npm launcher.
`tsc` 6 runs on Node 24.19 with a 16 GB heap, because it runs out of memory on VS Code and Sentry
with the default heap. Versions: `tsc-rs` 0.2.0, TypeScript 7.0.2 and 6.0.3, Bun canary
`bd599f5af`. The `tsc-rs` times are from 2026-10-09 and the others from 2026-10-07, on the same
machine. `tsc-rs` 0.2.0 reports the same errors as 0.1.0. Lines checked is the `tsc` 7
`--extendedDiagnostics` count, with the `.d.ts` files. The T3 Code benchmark above uses the same
machine and method.

Four apps needed changes to check with 0 errors under `tsc` 7. Nothing else changed:

- Excalidraw: no `baseUrl`, because TS 7 removed it.
- TypeORM: `moduleResolution` changed from `node` to `nodenext`, because TS 7 removed `node`.
- VS Code: the `electron` typings that its postinstall adds.
- Playwright: the sources that its build generates.

Two apps are not in the table:

- rxjs main needs its workspace packages built first.
- date-fns uses project references. There, `tsc -p` and `bun check` do different work.

The scripts are in [scripts/bench-apps](scripts/bench-apps): `setup.sh <dir>`, then
`run.sh <dir>` and `summary.py <dir>`, and `t3code.sh <dir>` for T3 Code. To time a new `tsc-rs`
only, run `run.sh` and `t3code.sh` with `TOOLS=tsc-rs`.

### Why `bun check` is faster

The lead of `bun check` is all in the check step. Its checkers share one type store on all cores.
`tsc` 7 runs 4 checkers, and each one makes its own copies of the types it needs: on T3 Code
`apps/server`, 57% of the types. `tsc-rs` keeps the `tsc` 7 model, because its goal is output that
is identical to `tsc` 7. More checkers or one shared store would change that output, so `tsc-rs`
does not copy them.

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
  `tsc-rs` can still read the old or missing output (TS2305 or TS2307) where `tsc` reads the new
  one, in a few cases: `noEmitOnError` projects, non-incremental `noCheck` projects, several large
  projects that only need to write their outputs with the default builders, or when the reading
  project references another project that builds before the writer. Add the reference to fix it.
- In the editor, memory grows slowly during long edit sessions (about 20 MiB per 1,000 edits). It
  starts 12 to 24% above `tsc`'s, and from about edit 20 it stays below `tsc`'s in the sessions we
  measured (up to 2,190 edits).
- `tsc-rs --version` prints the TypeScript version that it ports (7.1.0-dev), not the npm
  version. The compiler matches `typesVersions` against it.

## Development

`crates/ts_goport` is the compiler. It has two parts crates, `goport_util` and `goport_lsproto`,
in `crates/ts_goport/parts`, and uses the lib files in `crates/ts_goport/libs`.
`tools/ts_ast_codegen` generates `crates/ts_goport/src/astdata`, and
`tools/ts_diagnostics_codegen` generates `crates/ts_goport/src/diagnostics/catalog.rs` and
`crates/ts_goport/src/diag.rs`. `crates/ts_wasm` is the WebAssembly build
([npm/wasm](npm/wasm/README.md)).

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
