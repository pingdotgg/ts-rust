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

License: [licenses/TypeScript-LICENSE.txt](licenses/TypeScript-LICENSE.txt). Third-party notices
of TypeScript: [licenses/TypeScript-NOTICE.txt](licenses/TypeScript-NOTICE.txt).

## Language Server Protocol meta model

Copyright (c) Microsoft Corporation. Licensed under the MIT License.

`crates/ts_goport/src/lsp/lsproto/_generate/metaModelSchema.mts` is from
[microsoft/vscode-languageserver-node](https://github.com/microsoft/vscode-languageserver-node),
as TypeScript uses it.

License: [licenses/vscode-languageserver-node-LICENSE.txt](licenses/vscode-languageserver-node-LICENSE.txt).

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

## memfast

Copyright (c) 2026 Theo Browne. Licensed under the MIT License.

`crates/ts_goport/scripts/memfast.c` is `src/runtime/memfast.c` of
[t3dotgg/lust-lang](https://github.com/t3dotgg/lust-lang) (commit 6ced2b86fd), without change
below its header. The linux-x64 tsc links it in place of musl's memcpy, memmove, memcmp, bcmp and
memchr.

License: [licenses/memfast-LICENSE.txt](licenses/memfast-LICENSE.txt).
