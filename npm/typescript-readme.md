# @tsc-rs/typescript

[tsc-rs](https://www.npmjs.com/package/tsc-rs) as the `typescript` package. tsc-rs is a Rust port
of the TypeScript 7 compiler, which Microsoft writes in Go. This package has the layout of the
TypeScript 7 `typescript` package: the `tsc` bin, the JS API (`typescript/unstable/*`) and the
platform packages that VS Code finds. Install it under the name `typescript`, and the tools that
use `typescript` run tsc-rs.

This is an early release. Report problems at https://github.com/pingdotgg/ts-rust/issues.

## Use

In `package.json`:

```json
{ "devDependencies": { "typescript": "npm:@tsc-rs/typescript@<version>" } }
```

Or in a pnpm catalog (`pnpm-workspace.yaml`), with `"typescript": "catalog:"` in each
`package.json`:

```yaml
catalog:
  typescript: npm:@tsc-rs/typescript@<version>
```

To also replace the `typescript` that other packages depend on, use `overrides` (npm, in
`package.json`, or pnpm, in `pnpm-workspace.yaml`) with the same value.

Then run `npx tsc -p tsconfig.json`. `tsc` takes the same options as TypeScript 7's `tsc`.

`tsc --version` prints the TypeScript version that tsc-rs ports (7.1.0-dev), and so does
`require("typescript").version`. The compiler matches `typesVersions` against that version. The
npm version of this package is the tsc-rs version.

The postinstall makes `tsc` start without Node on Linux and macOS, about 20 ms faster per run.
pnpm runs it only when you allow it, for example with `allowBuilds` in `pnpm-workspace.yaml`:

```yaml
allowBuilds:
  "@tsc-rs/typescript": true
```

Without it, `tsc` still works through Node.

## Peer dependencies

A package with a peer dependency on `typescript` sees the npm version of this package (for example
0.8.0), not 7.1. npm then stops the install with ERESOLVE, also for an optional peer. pnpm warns.
To accept it, add to `package.json` for npm:

```json
{ "overrides": { "typescript": "$typescript" } }
```

and to `pnpm-workspace.yaml` for pnpm:

```yaml
peerDependencyRules:
  allowAny:
    - typescript
```

## VS Code

Install the TypeScript 7 extension
([TypeScriptTeam.native-preview](https://marketplace.visualstudio.com/items?itemName=TypeScriptTeam.native-preview)),
and point it at the package in `.vscode/settings.json`:

```json
{ "js/ts.tsdk.path": "node_modules/typescript/lib" }
```

Allow it when VS Code asks. The extension starts the tsc-rs language server and shows the npm
version of this package.

## Effect diagnostics

tsc-rs has the [Effect](https://effect.website) language service diagnostics built in (codes
377xxx), so an Effect project needs no second compiler and no patch. They come from the same check
as the TypeScript diagnostics, and the language server shows them too. They run only when the
tsconfig has the plugin, as with `@effect/language-service`:

```json
{ "compilerOptions": { "plugins": [{ "name": "@effect/language-service" }] } }
```

The rules, options and `@effect-diagnostics` comments are a port of
[Effect-TS/tsgo](https://github.com/Effect-TS/tsgo) 0.51.1. See the
[tsc-rs README](https://www.npmjs.com/package/tsc-rs#effect-diagnostics).

## Platforms

- Linux x64 and arm64 (static, any distribution)
- macOS arm64 (Apple silicon) and x64 (Intel)
- Windows x64 and arm64

npm and pnpm install only the platform package of your platform: `@tsc-rs/<os>-<arch>`, under
the name `@typescript/typescript-<os>-<arch>`. tsc-rs and @tsc-rs/typescript use the same platform
packages.

## License

MIT. The port keeps the licenses and notices of TypeScript (Apache-2.0, Copyright (c) Microsoft
Corporation) and Go (BSD-3-Clause). See NOTICE.txt.
