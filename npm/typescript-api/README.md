# typescript-api

The TypeScript 6.0 JavaScript API (`require("typescript")`) with tsc-rs as the compiler. Tools that
use the classic `ts.*` API, such as the NestJS CLI and its swagger and graphql plugins,
typescript-eslint, ts-loader, fork-ts-checker, ts-jest and ts-node, run unchanged on it. Their
programs, type checking and emit come from tsc-rs, inside the Node process, through a Node-API addon
(`crates/ts_napi`).

The package is private and named `typescript`, so that a project can point its `typescript`
dependency at it. Nothing here publishes.

## Use

Build the addon and the package, then pack it:

```sh
node npm/typescript-api/scripts/build-native.mjs   # crates/ts_napi -> native/ts_napi.<platform>-<arch>.node
node npm/typescript-api/scripts/build.mjs          # lib/: typescript@6.0.3 with hook points, its tsserver and locale files, the tsc-rs lib files
cd npm/typescript-api && npm pack                  # typescript-6.0.3.tgz
```

`build.mjs` downloads typescript@6.0.3 from the npm registry (its sha512 is pinned) and caches it in
`.cache/`; `TS6_TARBALL=<path>` uses a local copy. It also unpacks the unchanged package into
`.cache/typescript-6.0.3`, which the tests compare with.

In a project, make every `typescript` in the tree the package. With npm:

```json
{
    "devDependencies": { "typescript": "file:../ts-rust/npm/typescript-api/typescript-6.0.3.tgz" },
    "overrides": { "typescript": "$typescript" }
}
```

With pnpm, `overrides: { typescript: "file:..." }` in `pnpm-workspace.yaml`; with yarn,
`resolutions`. A dependency such as `@nestjs/cli` that pins `typescript` as a regular dependency
needs the override, or npm installs a second copy under it.

`ts.version` is `"6.0.3"` and `ts.versionMajorMinor` `"6.0"`: tools check them (typescript-eslint
refuses 7.0). `bin/tsc` (`lib/tsc.js`) runs tsc-rs's own command line in the Node process.
`bin/tsserver` (`lib/tsserver.js`) is TypeScript 6.0's tsserver on this package, for editors that
use the workspace TypeScript.

## What runs where

- **TypeScript 6.0.3's own code** (`lib/typescript.js`): the whole API surface except what is
  listed below. The scanner, parser, node factory, visitors, printer, utilities, config parsing,
  `ts.sys`, the module resolution functions, the watch API, the solution builder, the language
  service, `ts.server`, `lib/tsserver.js`, `lib/typingsInstaller.js` and the translated messages in
  `lib/<locale>/`. Nodes, source files and diagnostics are TypeScript 6.0 objects.
- **tsc-rs**: every program (`ts.createProgram` and each TypeScript 6.0 function that calls it),
  the type checker, emit, `transpileModule` and `transpileDeclaration`, and builder programs. The
  TypeScript 6.0 type checker never runs: `createTypeChecker` throws.

So a program's answers are tsc-rs's: TypeScript 7.1 at the pin in
[UPSTREAM.md](../../UPSTREAM.md). Type ids, symbol ids and the order of union members follow
TypeScript 7 (TypeScript 6.0 with `stableTypeOrdering: true` gives the same `typeToString`). The lib
files in `lib/` are tsc-rs's (TypeScript 7.1's), the files its checker reads.

## Differences from TypeScript 6.0

Options that TypeScript 7 removed:

| Option | What happens |
|---|---|
| `baseUrl` | Rewritten into `paths` with a final `"*": ["<baseUrl>/*"]`, which keeps TypeScript 6.0's lookup order. TS5101 as in TypeScript 6.0. |
| `downlevelIteration` | Dropped (it has no effect at ES2015 and later). TS5101. |
| `moduleResolution: node`/`node10`, `esModuleInterop: false`, `allowSyntheticDefaultImports: false`, `alwaysStrict: false` | tsc-rs resolves as `bundler` (`nodenext` for Node module kinds) and keeps interop and strict mode on. TypeScript 6.0's TS5107/TS5101, silenced by `"ignoreDeprecations": "6.0"`. |
| `target: ES3`/`ES5`, `module: None`/`AMD`/`UMD`/`System`, `outFile`, `moduleResolution: classic` | tsc-rs reports TS5108/TS5102 ("has been removed"), which `ignoreDeprecations` does not silence. |
| `keyofStringsOnly` and the other options TypeScript 5.5 removed | TS5102, as in TypeScript 6.0. |

The language service is TypeScript 6.0's own services code on tsc-rs's checker. Diagnostics,
`getEmitOutput`, quick info, signature help, go to definition, find references and completion
details give TypeScript 6.0's text (names inside printed types come as plain text parts, not
`className` and the like).

Not supported, with a `tsc-rs: ... is not supported` error rather than a wrong answer:

- Language service features that need a checker member tsc-rs cannot answer.
- A few TypeChecker members tsc-rs cannot answer: `getStringLiteralType`,
  `getNumberLiteralType`, `getBigIntLiteralType`, `getSymbolOfExpando`, and some argument forms of
  others. `symbolToString`, `signatureToString` and a few more are worked out in JS.
- `context.getEmitResolver()` in a custom transformer.
- `.tsbuildinfo` files: builder programs neither read nor write them, so incremental builds start
  from scratch (they are fast). A TypeScript 6.0 build info file is not a tsc-rs one, and writing
  TypeScript 6.0's format with tsc-rs's answers would mislead a real TypeScript 6.0.
- A host's `resolveTypeReferenceDirectives` and `resolveLibrary` (its module resolver is used).
- `getSymbolCount`, `getTypeCount`, `getInstantiationCount` and `getRelationCacheSizes` return 0.

Custom transformers run with TypeScript 6.0's own transformation machinery. A `before` transformer
sees the program's source files. When it changes a file, the changed tree is printed back to
TypeScript and tsc-rs emits that text, so the output is what TypeScript 6.0 gives in the cases we
tested (the NestJS paths, swagger and graphql plugins, ts-jest), but not in every corner: a
transformer that only replaces module specifiers gets them rewritten in tsc-rs's output of the
original file. `after` and `afterDeclarations` transformers see tsc-rs's output parsed again.

Text sent to tsc-rs cannot hold a lone UTF-16 surrogate (half of a pair); it becomes U+FFFD, which
keeps every position. A host callback of one directory's session can call into another directory's
session, but not back into a session that is itself waiting for a callback.

Each program holds a snapshot in tsc-rs. It is released when the program is garbage collected,
which needs a turn of the event loop: a synchronous loop that creates thousands of programs holds
all of them until it returns.

## How it works

- `scripts/build.mjs` turns seven functions of TypeScript 6.0.3's bundle into hook points:
  `createProgram`, `createBuilderProgram`, `readBuilderProgram`, `createTypeChecker`,
  `createCompilerHostWorker`, `transpileWorker` and `createLanguageService`. Each calls
  `__tsrsHooks.<name>` when it is set. `src/index.js` sets them when the bundle loads. Because the
  hooks are inside the bundle, TypeScript 6.0's own callers (watch, the language service,
  `ts.server`, `transpileModule`) reach them too. The build also copies TypeScript 6.0.3's
  tsserver, typingsInstaller and locale files and writes `lib/tsc.js`.
- `crates/ts_napi` runs tsc-rs's API server (the server behind `tsgo --api`) on a thread of the
  Node process, with the same msgpack messages over in-memory pipes. A request blocks the JS
  thread; file system callbacks run on the JS thread during the request. `src/session.js` is the
  JS side.
- `src/program.js` is the TypeScript 6.0 `Program` over one tsc-rs snapshot. A compiler host made
  by `ts.createCompilerHost` lets tsc-rs read the disk; any other host answers tsc-rs's file system
  calls, and its `SourceFile` objects are the program's. Other source files are parsed in JS from
  the text tsc-rs compiled. A JS node maps to a tsc-rs node handle by kind and position
  (`src/nodes.js`).
- `src/checker.js` and `src/objects.js` are the TypeChecker and the Type, Symbol and Signature
  objects, made with TypeScript 6.0's own constructors so that their methods work. One object per
  tsc-rs id, so `===` and `Map` keys work.
- `src/options.js`, `src/diagnostics.js`, `src/flags.js` and `src/names.js` translate options,
  diagnostics, enum values and names between TypeScript 6.0 and tsc-rs.
- `src/emit.js`, `src/transformers.js` and `src/transpile.js` are emit; `src/builder.js` is the
  builder programs, with TypeScript 6.0's affected-file logic.

## Tests

`cd npm/typescript-api && node --test test/*.test.mjs` runs the package's own tests, after the two
builds above. Most compare with typescript@6.0.3, the copy in `.cache/typescript-6.0.3`
(`TS6_STOCK=<dir>` names another). The tests that run tools on the package (typescript-eslint's
rules, the NestJS swagger and graphql plugins, nest-cli's tsconfig-paths hook, ts-jest's hoisting)
need the pinned packages of `test/consumers`, about 180 MB from the npm registry:

```sh
cd npm/typescript-api && npm run install:consumers   # npm ci --ignore-scripts --prefix test/consumers
```

Without them those tests skip and say why. `TSRS_CONSUMERS=<node_modules>` uses other copies.

`scripts/typescript-api/` runs other projects' tests, unmodified, against the package:

- `ts7-api-tests.sh`: microsoft/TypeScript's TS7 API tests at the pin, with tsc-rs in the test
  process (the addon test).
- `ts6-tests/run.sh`: TypeScript 6.0.3's own unit tests and compiler and conformance baselines,
  with the library swapped; `triage` sorts the differences against TypeScript 7's own baselines.
- `consumers/nest.sh`: nest-cli's unit tests and NestJS apps (tsc, webpack and swc builders, watch,
  the swagger and graphql plugins, `nest new` projects with jest and ts-jest, ts-node), compared
  with typescript@6.0.3.
- `consumers/typescript-eslint.sh`: typescript-eslint's own test suites.

Results on Linux x64, tsc-rs at the pin above:

| Suite (unmodified) | Result |
|---|---|
| TypeScript's TS7 API tests (`ts7-api-tests.sh`) | 954 of 954, the sync ones with tsc-rs in the test process |
| typescript-eslint v8.71.2, all 10 packages (`typescript-eslint.sh`) | 39,387 of the 39,387 tests that pass with typescript@6.0.3 pass, 0 lost |
| NestJS (`nest.sh`): nest-cli 12.0.8's unit tests and 10 app scenarios | 1,072 of 1,072 unit tests; every scenario passes. Emitted JS and `/api-json` equal typescript@6.0.3's; source maps differ where TypeScript 7 maps differently, and so do the 4 `.d.ts` files nest-cli builds with `bin/tsc` (TypeScript 7 orders unions differently) |
| TypeScript 6.0.3's first-adoption unit tests (`ts6-tests`, tier1) | 2,142 of 2,578. Most failures pass settings TypeScript 7 removed (the services transpile tests default to ES5) |
| TypeScript 6.0.3's compiler and conformance baselines (29,946) | 24,881 equal TypeScript 6.0's. Of the other 5,065, 1,360 equal TypeScript 7's own baselines, 3,678 use settings TypeScript 7 removed, 6 have no TypeScript 7 baseline and 21 differ |

The 21: in 19, declaration emit reads `package.json` from the disk instead of the host's in-memory
files, because tsc-rs runs emit on its checker threads, whose file system is the operating
system's; the fix is in `crates/ts_goport`. In 2, a circularity error sits at another node, as it
does with tsc-rs's own command line.

Speed, the same calls through typescript@6.0.3 and through the package (one run each on a shared
10-core host, so rough):

| | typescript@6.0.3 | package |
|---|---|---|
| nest-cli (701 files): `createProgram`, `getPreEmitDiagnostics`, `emit` | 2.2 s | 0.6 s |
| typescript-eslint's eslint-plugin (613 files, `tsconfig.build.json`): the same | 8.9 s | 2.4 s |
| typescript-eslint's eslint-plugin rule tests (vitest, 4 workers) | 72–97 s | 85–105 s |

Rule tests make many small checker calls (about 55 µs each in tsc-rs), so they gain nothing.
