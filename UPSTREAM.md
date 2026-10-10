# Upstream provenance

The port tracks the Go code of [`microsoft/TypeScript`](https://github.com/microsoft/TypeScript)
(under `tsc/`, TypeScript 7.1.0-dev).

- Commit: [`673a5f17d713`](https://github.com/microsoft/TypeScript/commit/673a5f17d713bdc8c7185f18a9c11e3c4ac5d781)
- Commit date: 2026-09-29
- Closest npm build: `typescript@7.1.0-dev.20260929.1`
- Oracle: `tsc/cmd/tsc` built with Go 1.27.1 (`~/.local/bin/tsgo-oracle-673a5f17d713`)

`tsc-rs` output is meant to be byte-equal to `tsc` at this commit. When you compare, use the
pinned build, not `typescript@7.0.x` or `@typescript/native-preview` (its last build is from
July). A difference that the pinned build also shows is upstream behavior: it changes when the
port moves to a newer pin after upstream fixes it.

## Earlier pins

Before 2026-09-29 the port tracked
[`microsoft/typescript-go`](https://github.com/microsoft/typescript-go) (now archived):
`dc37b5249ab60e2bbce936f71b883e6c8136167e` (2026-06-19), then pin B `16c25522e123`. The tooling
below still names `dc37b5249ab6` as the default pin in `UPSTREAM.json`; development runs select
the current pin with `GOPORT_PIN=673a5f17d713`.

Generated Rust inputs under `spec/` record their own source paths and must be
updated intentionally. Normal Cargo builds must not depend on a Go or Node
installation. Differential and baseline maintenance commands may use an
external upstream checkout and compiler oracle.

The default development oracle (`dc37b5249ab6`) is built from that checkout with Go 1.26.4:

```sh
CGO_ENABLED=0 go build -o ~/.local/bin/tsgo-oracle ./cmd/tsgo
```

## Pins and the pin selector

`UPSTREAM.json` is the machine-readable pin file: the current pin, every known pin (commit,
Go checkout, oracle path and sha256) and the oracle caches that belong to a pin. The default
paths above hold the current pin and do not change when a pin is added.

`GOPORT_PIN=<key>` (the first 12 hex digits of a pin commit) runs one command against another
pin. `scripts/goport/gate.sh`, `sweep-wide.sh`, `compare-build.sh`, `scripts/tsgo-oracle.sh`
and `scripts/upstream/corpus.py` honor it directly; `remote.sh run` passes it on. Any other
command honors it through `scripts/upstream/pin.py exec -- <command>`. The run gets a private
mount namespace where the default oracle, Go checkout and caches show the pin's files
(`target/continuation-r97-goport/pins/<key>/...`). With the variable unset nothing changes.

- `scripts/upstream/pin.py`: show, path, exec, binds, add (checkout and oracle), sync (to a host).
- `scripts/upstream/drift.py O N --out DIR`: upstream commits from O to N as lane work lists,
  with the Rust functions each Go change reaches.
- `scripts/upstream/rerecord.sh <key> [host]`: re-records the Go-side evidence for a pin into
  its pin-keyed dirs; `record.py` holds the recorders it adds.

The `tests/go_baselines` harness reads `TS_GO_REPO`; under `pin.py exec` its default path is
the pin checkout.

## Pin bump checks

Some port-only shortcuts are exact only while some Go code stays as it is. At each pin bump, check
each one against Go at the new pin (`drift.py` lists the Go changes), and fix or drop the shortcut
when the condition no longer holds.

- `crates/ts_goport/src/flags.rs` holds Go's integer flag and enum consts, kept in step by hand.
  Compare it with `internal/{ast,binder,checker,core}` at the new pin.
- `Checker::is_distribution_dependent` (`checker/relater_p5.rs`) keeps the answer of its first walk
  on the conditional root (chkperf3). That is exact only while the walk
  (`isTypeParameterPossiblyReferenced`, checker.go:22891 at `fed0bf24149f`) reads only state that
  its first read fixes: the AST; `resolvedSymbol` of TypeReference nodes, whose one writer is
  `getSymbolFromTypeReference` (checker.go:23606; TypeScript's JS `getTypeFromTypeReference` also
  writes it); the write-once `getResolvedSymbol` links (checker.go:14169); and the declarations of a
  resolved value symbol (check-time `mergeSymbol` clones a non-transient target before it appends,
  checker.go:14424-14449). A transient target is not cloned: `mergeSymbol` appends to its
  declarations in place (checker.go:14449). The merges of `initializeChecker` (checker.go:1309-1398:
  globals, pattern ambient modules, module augmentations) do that before any walk. At check time
  only `combineSymbolTables` (checker.go:16360) merges, when a late-bound member or export of a
  late-binding container has the name of an early one. The early symbol is transient only when an
  init merge made it, and it is a value that name resolution returns only in a corner (a function
  merged with a namespace across files, with a late-bound expando of the same name). That corner
  is not checked. Check: `grep -n 'resolvedSymbol = ' internal/checker/*.go`, the callers of
  `mergeSymbol` and `mergeSymbolTable`, and the walk itself. If a new writer of `resolvedSymbol` can reach a TypeReference node (or the first
  identifier of a `typeof` query), or the walk reads other state, drop the memo or clear it there.

## Pins in microsoft/TypeScript

`microsoft/typescript-go` is archived. From 2026-08-19 the Go code is in
[`microsoft/TypeScript`](https://github.com/microsoft/TypeScript) under `tsc/`, with Go 1.27 and
the module path `github.com/microsoft/TypeScript/tsc`. Pin B (`16c25522e123`) is tree-identical to
`microsoft/TypeScript` `51c91ec1d:tsc`; the "equivalent" field of each old pin names its commit there.

- `pin.py add <full sha or main> --repo microsoft/TypeScript` fetches that one commit into
  `~/.explore/repos/microsoft__TypeScript@<key>` and builds `tsgo-oracle-<key>` from `tsc/cmd/tsc`
  with Go 1.27.1. The pin record has `"layout": "typescript"`, `"subdir": "tsc"` and
  `goCheckout` `<checkout>/tsc`. So under `pin.py exec` the default Go checkout path shows `tsc/`,
  and tools find `internal/`, `cmd/` and `testdata/` where they always did.
- The TypeScript submodule is gone. Its test cases are in `testdata/tests/cases` (a case that
  collided with a Go case has the name in `testdata/promotedTestCollisions.txt`), the
  `submodule*` baseline dirs are merged into the plain dirs, and the `.diff` baselines are gone.
  `record.py corpus` maps the old sample paths. `cmd/tsgo` is now `cmd/tsc`.
- `record.py typesyms` rewrites the module path in its overlay main. `drift.py` reads the new repo
  with `--repo <clone> --subdir tsc`.
- Not adapted yet: the fourslash trace recorder (goport-ls `scripts/goport/fourslash-record`),
  `np-suite.sh` (the client moved to `packages/typescript` at the repo root) and the
  `tests/go_baselines` submodule runner. Bump C ports them (see
  `target/continuation-r97-goport/upstream/bumpC/plan.md`).

Some pin caches are inputs, not oracle outputs, because they come from the pin's Go tests or
API: the API traces (`tests2/api/traces`, built by `scripts/goport/api_oracle.py build` under
`GOPORT_PIN`; pins after `dc37b5249ab6` speak API protocol 2), the fourslash LS traces
(`ls-oracle/fourslash`, recorded from the pin's fourslash tests) and the f1 sample case files
(`record.py corpus`). Build them at each pin before its oracle recording.
