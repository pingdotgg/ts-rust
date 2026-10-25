#!/usr/bin/env bash
# Copies the bundled lib files into <dir>. A noembed build (cargo feature
# `noembed`) reads them there: next to its binaries, as Go's `hereby lib`
# puts them next to a noembed tsgo, and as the npm packages ship them.
#
# The set is the one that the default build embeds
# (src/frontend/bundled/embed.rs): crates/ts_goport/libs. Its texts equal the
# pin's internal/bundled/libs, so the lib parse and bind snapshots load them.
#
# It also copies the localized message catalogs (crates/ts_goport/data/loc,
# the pin's internal/diagnostics/loc/*.json.gz) into <dir>/loc. A noembed
# build reads them there for a locale (src/diagnostics_loc.rs); Go embeds
# them in its noembed build too.
#
# Usage: copy-libs.sh <dir>
set -euo pipefail

[[ $# == 1 ]] || { echo "usage: $0 <dir>" >&2; exit 2; }
script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
crates="$(cd -- "$script_dir/../.." && pwd)"
mkdir -p "$1/loc"
cp "$crates"/ts_goport/libs/lib*.d.ts "$1"/
cp "$crates"/ts_goport/data/loc/*.json.gz "$1"/loc/
