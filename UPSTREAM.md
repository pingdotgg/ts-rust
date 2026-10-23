# Upstream provenance

The port tracks the Go code of [`microsoft/TypeScript`](https://github.com/microsoft/TypeScript)
(under `tsc/`, TypeScript 7.1.0-beta).

- Commit: [`3e9236361fa9`](https://github.com/microsoft/TypeScript/commit/3e9236361fa97a9e6c685f4833ac457ae2d8c50a)
- Commit date: 2026-10-19
- Same commit as the npm build `typescript@7.1.0-beta` (its `gitHead`): the release-7.1 merge of
  main `d7d39da5c4`. Its `tsc/` equals `d7d39da5c4` except `internal/core/version.go`.
- Oracle: `tsc/cmd/tsc` built with Go 1.27.1 (`~/.local/bin/tsgo-oracle-3e9236361fa9`)
- Since R222 (bump E), after tsc-rs 0.8.0. tsc-rs 0.4.0 to 0.8.0 used the pin `fed0bf24149f`
  below, and 0.3.0 and earlier the pin `673a5f17d713`.

`tsc-rs` output is meant to be byte-equal to `tsc` at this commit. When you compare, use the
pinned build, not `typescript@7.0.x` or `@typescript/native-preview` (its last build is from
July). A difference that the pinned build also shows is upstream behavior: it changes when the
port moves to a newer pin after upstream fixes it.

## Earlier pins

- [`fed0bf24149f`](https://github.com/microsoft/TypeScript/commit/fed0bf24149fb1ed36039212648bafdafc1ea10e)
  (2026-10-08, the `gitHead` of `typescript@7.1.0-dev.20261008.1`, oracle
  `~/.local/bin/tsgo-oracle-fed0bf24149f`): tsc-rs 0.4.0 to 0.8.0 (R191 to R221).
- [`673a5f17d713`](https://github.com/microsoft/TypeScript/commit/673a5f17d713bdc8c7185f18a9c11e3c4ac5d781)
  (2026-09-29, closest npm build `typescript@7.1.0-dev.20260929.1`): tsc-rs 0.3.0 and earlier.
  Behavior that differs between this pin and the current one is upstream change, for example the
  inference change of microsoft/TypeScript#64553 (in the nightlies from `7.1.0-dev.20261001.1`).

Before 2026-09-29 the port tracked
[`microsoft/typescript-go`](https://github.com/microsoft/typescript-go) (now archived):
`dc37b5249ab60e2bbce936f71b883e6c8136167e` (2026-06-19), then pin B `16c25522e123`. The tooling
below still names `dc37b5249ab6` as the default pin in `UPSTREAM.json`; development runs select
the current pin with `GOPORT_PIN=3e9236361fa9`.

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
- `Type::member_mask` (`checker/types.rs`, memberflags1): a new union or intersection stores the OR
  of its members' flags and of nested members' masks, and `maybe_type_of_kind`
  (`checker/checker_p30.rs`) tests that mask in place of Go's walk (`maybeTypeOfKind`,
  checker.go:28211 at `3e9236361fa9`). That is exact only while no type's flags and no member list
  change after the type can be a member. At `3e9236361fa9` Go writes `Type.flags` after
  `newType` at 3 places, each before the type can be a member: checker.go:24476 (an enum union gets
  `EnumLiteral`, port `checker_p26.rs`), 26351 (a 2-member boolean union gets `Boolean`,
  `checker_p28.rs`) and 28795 (the regular type of an object literal, `checker_p31.rs`). Only
  `newUnionType` and `newIntersectionType` (checker.go:25778, 25784) write the members of a union
  or intersection (the other `.types` writes are of tuple element lists and template literal
  types). Check: `grep -nE '\.flags (\|=|&\^=|=) ' internal/checker/*.go` for a new write
  to a `*Type`'s flags, and `grep -n '\.types = ' internal/checker/*.go` for a new write of
  members. If a new write can reach a type that is already a member, drop the mask.
- `UnionCalls` (`checker/checker_p28.rs`, unionmemo1): `get_union_type_ex` keeps the result of a
  call with no origin and no subtype reduction, and gives it to a later call with the same
  reduction, alias and types in the same order without Go's worker (`getUnionTypeWorker`,
  checker.go:26251 at `3e9236361fa9`). That is exact only while the data that the worker's sort and
  key read does not change once a type can be a member: the sort (`CompareTypes`, utilities.go:418)
  reads `Declarations()[0]` of a type's symbol, also through `compareSymbolsWorker`
  (utilities.go:366). One case is open: `mergeSymbol` (checker.go:14422) appends declarations in
  place when its target is transient (checker.go:14454), and at check time `combineSymbolTables`
  (checker.go:14372) merges. If such a merge gives a transient type symbol its first declaration
  between two equal calls, Go sorts again and can make another union, and the memo gives the old
  one. No case is known, and Go's own `unionTypes` cache (`getUnionKey`, checker.go:17940) rests on
  the same condition. Check: `grep -n 'SetDeclarations(' internal/checker/*.go` for a write that
  can reach the symbol of a type that already exists, the callers of `mergeSymbol`, and what
  `CompareTypes`, `compareSymbolsWorker` and the worker read. If such a write is found, or the
  worker gets a new effect (a counter, a cache write, a new type), drop the memo or move
  `UnionCalls::mark` at that step.

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
