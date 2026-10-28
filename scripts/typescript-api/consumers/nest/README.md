# NestJS end-to-end harness

Runs NestJS tooling (`@nestjs/cli` 12.0.8, `@nestjs/swagger` 12.0.2, `@nestjs/graphql` 14.0.3)
and the other tools that load `typescript` in NestJS projects (webpack with ts-loader and
fork-ts-checker-webpack-plugin, jest with ts-jest, ts-node) against a `typescript` package
directory or tarball, and diffs two runs. Entry point:
`scripts/typescript-api/consumers/nest.sh` (`nest.sh help`).

```sh
scripts/typescript-api/consumers/nest.sh run npm/typescript-api/.cache/typescript-6.0.3.tgz target/typescript-api/consumers/nest/runs/stock
scripts/typescript-api/consumers/nest.sh run npm/typescript-api target/typescript-api/consumers/nest/runs/pkg
scripts/typescript-api/consumers/nest.sh compare target/typescript-api/consumers/nest/runs/stock target/typescript-api/consumers/nest/runs/pkg --out target/typescript-api/consumers/nest/runs/stock-vs-pkg
```

## How the package is installed

`run` packs the package with `npm pack --ignore-scripts` (a `.tgz` argument is used as is) into
`<out>/package/`, unpacks it and records a manifest (sha256 per file, `treeHash`). Each app copy
gets `"typescript": "file:<tgz>"` in the section that held typescript and
`"overrides": {"typescript": "$typescript"}`, and its lockfile loses its typescript entries. After
`npm install`, every `node_modules/**/typescript` directory must have the package's `treeHash`
(check "every installed typescript copy is the package under test", `<scenario>/install.json`).
nest-cli (a) is the exception: `npm ci`, then `npm install --no-save typescript@file:<tgz>`, so that
its own `test/package-json.spec.ts` keeps passing.

Upstream sources are cloned at the commits in `pins.json` into
`target/typescript-api/consumers/nest/_cache/src/`. Sample installs use `--before` (`pins.json`
`npmBefore`) and reuse the lockfile of their first install (`_cache/locks/`), so transitive
versions do not drift between the two runs of a comparison. npm uses its own cache
(`_cache/npm`). App copies live in `target/typescript-api/consumers/nest/<scenario>/` and are
recreated from scratch (node_modules included) on every run. One run at a time (`.lock`).

## Scenarios

| | id | what runs | kept outputs |
|---|---|---|---|
| a | nest-cli-unit | nestjs/nest-cli 12.0.8 as its CI does: `npm ci`, `npm run build` (the package's `bin/tsc`), `npm run clean`, `npm test` (vitest, 4 workers) | `build/` (files the build created), per-test results |
| b | swagger-sample | nestjs/nest `sample/11-swagger` (ESM, nodenext): `npm run build` (tsc builder, swagger plugin with introspectComments), its unit and e2e vitest suites, `node dist/main.js`, `GET /api-json` | `dist/`, tsbuildinfo, `api-json.json`, per-test results |
| c | cjs-paths | `../fixtures/nest-cjs-paths`: CommonJS, `baseUrl` + `paths` (`@app/*`, `@common`), `ignoreDeprecations: "6.0"`, swagger plugin; `npm run build`, run the app, six HTTP requests, `GET /api-json`; checks that no alias is left in `dist` | `dist/`, tsbuildinfo, `http/*.json`, `api-json.json` |
| d | cjs-paths-watch | the same fixture with `nest start --watch`: first build, `GET /`, edit `src/app.service.ts`, wait for "File change detected" and the second "Found 0 errors", check the restarted app answers the edit, stop | `dist-initial/`, `dist-after-edit/`, `http/`, watch event order |
| e | swagger-swc | `sample/11-swagger` plus `@swc/cli`/`@swc/core` (`pins.json`): `nest build --builder swc --type-check` | `dist/`, `metadata.ts` (TS program + printer) |
| f | graphql-sample | `sample/23-graphql-code-first` with the graphql plugin: `--legacy-peer-deps`, `strict: false` (TS 6.0 defaults to strict and the sample has 4 errors), plus `@as-integrations/express5` (missing from the sample; the app does not start without it); `npm run build`, run the app, one GraphQL query | `dist/`, tsbuildinfo, the generated `schema.gql`, `http/` |
| g1 | cjs-paths-webpack | the CJS fixture plus the webpack packages in `pins.json` `webpack` (nest-cli's own dev versions), `nest-cli.json` without plugins: `nest build --webpack` (ts-loader `transpileOnly` + fork-ts-checker-webpack-plugin), run the bundle with the six requests of c, then a type error appended to `src/app.service.ts` must fail the build with TS2322 (fork-ts-checker) | `dist/main.js`, `build-output.txt`, `build-type-error-output.txt`, `http/`, `api-json.json` |
| g2 | cjs-paths-webpack-swagger | the same with the swagger plugin: ts-loader in full mode (language service `getEmitOutput`, `getCustomTransformers(program)`, declarations as webpack assets), no fork-ts-checker; c's schema checks; the type error must fail the build through ts-loader's own diagnostics | as g1, plus the `.d.ts` files |
| h1 | nest-new-esm | `../fixtures/nest-new-app`, the default `nest new` project (ESM, vitest, oxlint): `npm test`, `npm run test:e2e`, `npm run build`, `node dist/main.js`, `GET /` | `dist/`, tsbuildinfo, per-test results, `http/root.json` |
| h2 | nest-new-cjs | `../fixtures/nest-new-cjs`, the CommonJS `nest new` project (jest + ts-jest; jest reads `jest.config.ts`, which imports `typescript`, through ts-node; ts-jest compiles each file with its own one-file `ts.createProgram` because the module kind is nodenext): `npm test` and `npm run test:e2e` with `--no-cache`, `npm run build`, run, `GET /` | as h1 |
| i | ts-node | `../fixtures/ts-node-script` with ts-node 10.9.2: `node -r ts-node/register` in type-check mode (language service) and with `TS_NODE_TRANSPILE_ONLY=true` (`transpileModule`), each on `src/main.ts` (imports TS modules, an enum, a generic class, a decorator, `import()`) and on `src/type-error.ts` (TS2322 in type-check mode, runs in transpile-only mode); plus transpile-only with `tsconfig.no-rootdir.json`, where TS 6.0 reports TS5011 (rootDir must be set when ts-node's outDir is); that step is not required, compare still counts it | `out/<run>.txt` (output without stack frames) |

A bare letter selects its numbered scenarios (`--scenario g` runs g1 and g2).

Both `nest new` templates lint with `oxlint --type-aware` (oxlint-tsgolint, its own Go TypeScript),
which never loads the `typescript` package, so h does not run `npm run lint`.

Fixtures (committed, with their `package-lock.json`):

- `../fixtures/nest-cjs-paths`: written by hand for c, d and g.
- `../fixtures/nest-new-app`, `../fixtures/nest-new-cjs`: `nest.sh make-nest-new-fixture` (versions
  in `pins.json` `nestNew`). `nest-new-app` is `nest new nest-new-app --skip-git --skip-install
  --package-manager npm --no-observe --language ts` (without a terminal the template is ESM with
  vitest). nest-cli 12.0.8 has no flag for the module system; in a terminal `nest new` asks for it,
  so `nest-new-cjs` is the same schematic call nest-cli makes with the CommonJS answer
  (`--type=cjs`). Every dependency range is then pinned to the version npm resolves at `npmBefore`.
- `../fixtures/ts-node-script`: written by hand for i.

Every scenario also keeps `build-output.txt` (build output with paths, times and pids replaced)
where it builds. Samples listen on port 3000; `preload-port.cjs` (`node -r`) moves them to a free
port without editing them. The fixture reads `PORT`.

## Output

`<out>/results.json` (package, pins, host, every scenario record) and `<out>/summary.md`;
per scenario `<out>/<id>/status.json` (status `pass`/`fail`/`error`, steps with exit codes, times
and logs, checks, tests, artifacts, metrics), `logs/NN-<step>.log`, the artifacts above,
`tests/<suite>.json` (`{counts, tests: {"<file> > <describe> > <test>": status}}`) and the raw
vitest or jest report. A scenario fails when a required step ends with another exit code than it
expects (0, or non-zero for the type-error builds) or a required check fails; `info` checks and
steps marked not required never fail it. `run` exits 0 when every scenario passes, 1 otherwise.

## Compare

`compare <base> <new>` prints a markdown report: status per scenario, step and check changes,
per suite the retained, lost, recovered, absent and added tests, and per artifact the files that
differ byte by byte (unified diff excerpts; JSON files are diffed with sorted keys and labelled
"bytes differ, JSON equal" when only order or spacing changed), plus the watch event order.
Times are shown, never counted. A `.tsbuildinfo` (at the app root or inside a tree such as
`dist/`) is diffed and listed but never counted: it records the compiler's own lib file hashes and
format. webpack's asset sizes are replaced in `build-output.txt` (the bundle itself is compared). Exit 0 when nothing differs, 1 otherwise. `--out <dir>` writes
`compare.md` (longer excerpts), `compare.json` and every full diff under `diffs/`.

## Environment

- `--env NAME=VALUE` (repeatable) reaches builds, tests and apps, never npm, so a `NODE_OPTIONS`
  preload does not run inside npm.
- `TSRS_*` variables of the caller reach every command (for example `TSRS_API_CHANNEL`,
  `TSRS_NAPI_PATH`).
- jest runs with `--no-cache`: its transform cache lives in the OS temp dir and ts-jest keys it by
  the typescript version string, "6.0.3" for stock and the package alike.
- `nest.sh run` re-runs itself under `systemd-run --user --collect --wait --pipe` with
  `MemoryMax=12G` (`NEST_HARNESS_NO_UNIT=1` skips that). `NEST_HARNESS_TIMEOUT_SCALE=N`
  multiplies every timeout.
- Needs network access for the first clone and npm installs; later runs work from the caches as
  long as the package set does not change.
