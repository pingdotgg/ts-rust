# Notices

ts-rust is licensed under the [MIT License](LICENSE).

ts-rust is a port of other projects. Those parts keep their original licenses and notices, so the
crates and npm packages say `MIT AND Apache-2.0`.

## TypeScript

Copyright (c) Microsoft Corporation. Licensed under the Apache License, Version 2.0.

- The compiler source in `crates/` is a Rust port of the native TypeScript compiler (Go), from
  [microsoft/TypeScript](https://github.com/microsoft/TypeScript) and
  [microsoft/typescript-go](https://github.com/microsoft/typescript-go). We changed it: it is
  rewritten in Rust. [UPSTREAM.md](UPSTREAM.md) records the upstream revision.
- The lib files (`crates/ts_goport/libs`), the localized diagnostic messages
  (`crates/ts_goport/data/loc`) and the JS launcher and JS API in the npm packages are copied from
  TypeScript without change.

- The AST schema (`tools/ts_ast_codegen/spec`) is copied from TypeScript, with edits for the Rust
  code generator ([PROVENANCE.md](tools/ts_ast_codegen/spec/PROVENANCE.md)).
- `npm/typescript-api` builds its `lib/` from the typescript@6.0.3 npm package
  (`npm/typescript-api/scripts/build.mjs`): `typescript.d.ts`, `tsserverlibrary.*`,
  `watchGuard.js` and `typesMap.json` without change, and `typescript.js` with hook points added
  (the build marks the file as changed). Some functions in `npm/typescript-api/src` port parts of
  TypeScript 6.0.3 (`src/compiler/builder.ts`, `program.ts`, `emitter.ts`); each names its source.

License: [licenses/TypeScript-LICENSE.txt](licenses/TypeScript-LICENSE.txt). Third-party notices
of TypeScript: [licenses/TypeScript-NOTICE.txt](licenses/TypeScript-NOTICE.txt).

## Language Server Protocol meta model

Copyright (c) Microsoft Corporation. Licensed under the MIT License.

`crates/ts_goport/src/lsp/lsproto/_generate/metaModelSchema.mts` is from
[microsoft/vscode-languageserver-node](https://github.com/microsoft/vscode-languageserver-node),
as TypeScript uses it.

License: [licenses/vscode-languageserver-node-LICENSE.txt](licenses/vscode-languageserver-node-LICENSE.txt).

## NestJS CLI

Copyright (c) 2017-2025 Kamil Mysliwiec, Thomas Ricart. Licensed under the MIT License.

`npm/typescript-api/test/fixtures/nest-cli-hooks` holds spec fixtures of
[nestjs/nest-cli](https://github.com/nestjs/nest-cli), copied without change
([PROVENANCE.md](npm/typescript-api/test/fixtures/nest-cli-hooks/PROVENANCE.md)).

License: [licenses/nest-cli-LICENSE.txt](licenses/nest-cli-LICENSE.txt).

## Go

Copyright (c) 2009 The Go Authors. Licensed under the BSD 3-Clause License.

`crates/ts_goport/src/gostd` ports parts of the Go standard library and `golang.org/x/text` that
the compiler uses.

License: [licenses/Go-LICENSE.txt](licenses/Go-LICENSE.txt).

## Unicode data

Copyright (c) Unicode, Inc. Licensed under the Unicode License v3.

The collation and normalization tables in `crates/ts_goport/src/gostd/data` and
`crates/ts_goport/src/gostd/unicode_tables.rs` come from Unicode and CLDR data, through the Go
packages above.

License: [licenses/Unicode-LICENSE.txt](licenses/Unicode-LICENSE.txt).
