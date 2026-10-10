# Diagnostic catalog provenance

The files crates/ts_goport/src/diagnostics/catalog.rs and
crates/ts_goport/src/diag.rs are generated from
tsc/internal/diagnostics/diagnostics_generated.go in the cached
microsoft/TypeScript checkout at commit
fed0bf24149fb1ed36039212648bafdafc1ea10e (2026-10-08, the bump D pin).
catalog.rs holds exactly the pin's 2,222 messages. There is no overlay.

diag.rs has one static per Go message, and after them the messages of
`spec/removed.txt`: messages that Go removed at the pin while unported Rust
code still uses them. They are not in `CATALOG`. Each comment in that file
names the Rust user and the upstream PR that removes it. Remove a line with its
last Rust user and generate again. With no message lines left, diag.rs is the
plain Go catalog.

Go builds the catalog from tsc/internal/diagnostics/diagnosticMessages.json
(before the move to microsoft/TypeScript: the TypeScript submodule's
diagnosticMessages.json plus extraDiagnosticMessages.json). The code
generator also accepts JSON inputs directly with repeated --input arguments.
`--names-output` (diag.rs) needs `--go-generated`, because the Go variable
names are only in the Go file. The generator reads both Go file shapes: the
old one with a generated `keyToMessage` switch and the one since #64402 with
the `allMessages` list.

To generate both files again (for example at a pin bump), from the repository
root:

```
cargo run --release -p ts_diagnostics_codegen -- \
  --go-generated <go>/internal/diagnostics/diagnostics_generated.go \
  --output crates/ts_goport/src/diagnostics/catalog.rs \
  --names-output crates/ts_goport/src/diag.rs \
  --removed tools/ts_diagnostics_codegen/spec/removed.txt \
  --provenance "microsoft/TypeScript@<commit> tsc/internal/diagnostics/diagnostics_generated.go"
rustfmt --edition 2024 crates/ts_goport/src/diag.rs
```

Then update the commit above and the catalog size in
`generated_catalog_is_complete_and_sorted` (diagnostics/mod.rs). A Rust user of
a message that Go removed stops the build: port the Go change that removed it,
or, when another lane owns that change, add the message's line from the
previous pin to `spec/removed.txt`.
