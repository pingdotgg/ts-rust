# Bundled lib provenance

The 115 `lib*.d.ts` files here are exact copies of `tsc/internal/bundled/libs`
in microsoft/TypeScript at `fed0bf24149fb1ed36039212648bafdafc1ea10e`
(2026-10-08, the bump D pin). Since #63993 that directory is the lib source:
the TypeScript submodule is gone, and the files are edited in place.

This is the only lib dir. `src/frontend/bundled/embed.rs` embeds every file
here (`bundled_lib!`), `scripts/copy-libs.sh` copies them next to the binaries
for a noembed build, and `scripts/gen-lib-names.py` writes
`src/core/lib_names.rs` from them.

The upstream contents are licensed under Apache-2.0 and retain Microsoft's
copyright notice in every generated declaration file.

## At a pin bump

1. Replace the files with `internal/bundled/libs` of the new pin (`<go>` is the
   `tsc` directory of the pin's checkout):
   `rm crates/ts_goport/libs/lib*.d.ts && cp <go>/internal/bundled/libs/lib*.d.ts crates/ts_goport/libs/`.
2. Update the pin above.
3. When a file is added or removed, change the `bundled/embed.rs` entries and
   the `LIB_NAMES` list in `bundled.rs` (Go `embed_generated.go` and
   `libs_generated.go`), and check the `LibMap` in `tsoptions/enum_maps.rs`
   (Go `tsoptions/enummaps.go`): a lib file refers to other libs by those
   names. Then run `scripts/gen-lib-names.py`.
4. Regenerate the lib parse and bind snapshots (`lib_parse.bin`, then
   `lib_bind.bin`).
