# Classic TypeScript language-service compatibility

Status: Approved. Implementation pending. The current WebAssembly editor API does not satisfy this contract.

## Required calling code

Consumers can replace the package import while keeping their language-service code:

```ts
const service = ts.createLanguageService(host, documentRegistry);
const info = service.getQuickInfoAtPosition(fileName, utf16Offset);
const diagnostics = service.getSemanticDiagnostics(fileName);
const program = service.getProgram();
const file = program?.getSourceFile(fileName);
const checker = program?.getTypeChecker();
service.dispose();
```

Creation and methods keep their TypeScript signatures and synchronous return values. The supplied
`LanguageServiceHost` controls files, snapshots, versions, options, resolution and cancellation.
Existing setup helpers, including `ScriptSnapshot.fromString`, `createDocumentRegistry`, enums
and display-part helpers, remain available where callers need them.

TypeScript 6.0.3's published declarations are the initial classic API contract. This is a calling
contract, separate from the compiler semantics of the TypeScript-Go revision this port targets.
Independent package-wide `createProgram` support is outside this request. Objects reachable from
`LanguageService.getProgram`, diagnostics and completion symbols are inside it.

## Current gaps

The current factory accepts file/configuration options and returns a promise. Its service has
`request`, `updateFiles`, `deleteFiles` and asynchronous `dispose`. The reference interface has
65 unique members, including overloads and optional members. Its host and return types differ.

The Rust port's native API also has a different contract. It creates snapshots, projects and
compiler objects. Its completion response has numeric LSP kinds and omits several classic
completion fields. Existing hover responses contain rendered Markdown. A JavaScript adapter
cannot recover original display parts, modifiers and documentation tags from those responses.

`getProgram`, `getCompletionEntrySymbol` and `Diagnostic.file` expose object graphs with methods,
cycles and identity. Source files, nodes, symbols, types and signatures need faithful JavaScript
objects backed by retained Rust data. Plain JSON results do not fulfill that requirement.

Custom resolution and emit-transformer callbacks operate on those objects. Filesystem callbacks
alone do not cover the host contract. Refactors, paste edits and the remaining classic methods
need an explicit producer and behavior check; matching an LSP feature name is insufficient.

## Implementation shape

Use a synchronous facade on the caller's thread. Rust imports invoke the supplied host directly.
Build classic result records before the LSP conversions discard information. Reuse the native
snapshot leases, object registries and AST encoder where they preserve the required semantics.
Keep transport handles private and preserve the identities and lifetime of returned objects.

The native API transport is useful infrastructure, but its existing schemas do not implement
the classic interface. The installed native TypeScript decoder also uses a different AST protocol
version from the port. Generate or use a decoder from the same pinned source as the encoder.

A browser's synchronous methods cannot depend on blocking worker RPC. Asset readiness must be
handled by package loading without adding an initialization call to consumer code. Synchronous
execution also needs new stack evidence; the existing async worker-stack checks do not prove it.

Do not return promises, fabricate missing fields, install unsupported-method stubs, parse hover
Markdown into guessed TypeScript structures, or run the JavaScript TypeScript service as a hidden
fallback. None meets this contract.

## Execution and checks

- [x] Read authoritative factory, host and language-service declarations.
- [x] Prove the current declarations fail factory and service assignability.
- [x] Compare direct Rust bindings and native-session transport designs.
- [ ] Prove synchronous loading, host callbacks, reentrancy and adequate stack in Node and browsers.
- [ ] Bind versions, snapshots, options, cancellation, registry sharing and custom host callbacks.
- [ ] Preserve program/source-file/node/symbol/type/signature identity and lifetime across edits.
- [ ] Implement every declared method and overload with complete classic result records.
- [ ] Compare unchanged callers against the reference for results, edits, callbacks and disposal.
- [ ] Pass compile-time compatibility and runtime comparisons before describing the API as drop-in.

The first runtime gate creates a service from an ordinary host, returns a diagnostic with a real
source-file object, observes a version change, and checks the retained old source file. Include
the existing deep-expression fixture on this synchronous path. The first result-record gate
checks quick info and completions with documentation, tags, modifiers and symbol identities.

To run the declaration check, install the reference into a temporary directory and pass its
package directory to `npm run check:typescript-api --prefix npm/wasm -- <reference-directory>`.
Use `typescript@6.0.3` for the initial contract. The command derives the member list from that
package and checks both the factory and returned service against its declarations. It currently
exits with status 1 because compatibility is incomplete. It is separate from the scoped WASM
tests and is not a claim of runtime equivalence.

Pass the reference package's `lib/typescript.js` as a second argument for a positive control.
The reference passes its own declarations. This checks that a failed replacement result comes
from the replacement contract rather than a broken declaration-check harness.
