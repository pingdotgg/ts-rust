#!/usr/bin/env bash
# Builds a local linux-x64 npm package set in Go's layout at the pin, and never publishes:
#   typescript                        Go's JS launcher (bin/tsc, lib/tsc.js) and JS API (dist).
#                                     Rust: plus the postinstall npm/install.js, which rewrites
#                                     bin/tsc on POSIX to run the native tsc without Node.
#   @typescript/typescript-linux-x64  lib/tsc (the native tsc) and the lib files next to it.
#                                     Rust: plus lib/loc, the locale catalogs (copy-libs.sh).
# The layout follows the Go checkout's Herebyfile.mjs (npm/pack.mjs has the details).
#
# usage: npm-pack.sh <out-dir> <tsc>
#          Rust. <tsc> is a noembed tsgo stamped with the package version: for a quick build
#          GOPORT_BUILD_VERSION=<v> scripts/run-cargo-capped.sh build --release -p ts_goport
#          --bin tsgo --features noembed; for a shipped one RELEASE_VERSION=<v>
#          crates/ts_goport/scripts/build-release.sh. The package version is the version it reports.
#        npm-pack.sh --go <version> <out-dir>
#          Go. Builds the Go tsc at the pin as Go's release build does (Herebyfile.mjs
#          getReleaseBuildFlags and buildTsc: -trimpath, -ldflags "-s -w -X core.version=<version>",
#          tag noembed, CGO_ENABLED=0; the Go toolchain of the pin's oracle) and packs it with Go's
#          launcher only.
#        npm-pack.sh --name tsc-rs --package-version <v> [--also <os>-<arch>=<tsc>]... <out-dir> <tsc>
#          Rust, the port's own set (npm/pack.mjs --name tsc-rs) at npm version <v>: tsc-rs (bin
#          tsc-rs) and @tsc-rs/linux-x64 from <tsc>, plus @tsc-rs/<os>-<arch> for each --also, a
#          tsc built for that platform (for example darwin-arm64=<path>). It does not run here, so
#          only <tsc> is checked. <tsc> reports the TypeScript version (not stamped with <v>).
#        npm-pack.sh --name @tsc-rs/typescript --package-version <v> [--also <os>-<arch>=<tsc>]... <out-dir> <tsc>
#          Rust, the drop-in for `typescript` (npm/pack.mjs): @tsc-rs/typescript (Go's main
#          package, bin tsc; its optionalDependencies alias @typescript/typescript-<os>-<arch> to
#          npm:@tsc-rs/<os>-<arch>@<v>) and the same platform packages as --name tsc-rs, byte for
#          byte. Pin layout "typescript" only.
# The pin is GOPORT_PIN, else the current pin (scripts/upstream/pin.py path goCheckout).
# NPM_PACK_GO_DIR=<dir> names the pin's Go checkout dir when it is not at the pin.py path (CI, the
# release workflow). It needs the npm install of the repo root (node_modules/.bin/tsc). Both pin
# layouts work. "typescript" (microsoft/TypeScript, pin N on): the Go module is <repo>/tsc
# (./cmd/tsc, module github.com/microsoft/TypeScript/tsc) and the package input is
# <repo>/packages/typescript. "typescript-go": the Go module is the checkout (./cmd/tsgo, module
# github.com/microsoft/typescript-go) and the input is _packages/native-preview.
# Output: <out-dir>/typescript, <out-dir>/typescript-linux-x64 and their tarballs
# (<out-dir>/typescript-<v>.tgz, <out-dir>/typescript-typescript-linux-x64-<v>.tgz). With --name
# tsc-rs: <out-dir>/tsc-rs, <out-dir>/tsc-rs-<os>-<arch> and their tarballs. With --name
# @tsc-rs/typescript: <out-dir>/tsc-rs-typescript, the same platform dirs and their tarballs. Each
# run replaces only the dirs and tarballs of the packages it writes, so both port sets can share
# <out-dir>.
set -euo pipefail
repo="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"
usage() { sed -n '10,36p' "$0" >&2; exit 2; }

go_version="" name=typescript also=() package_version=()
while [[ ${1:-} == --name || ${1:-} == --also || ${1:-} == --package-version ]]; do
  [[ $# -ge 2 ]] || usage
  case $1 in
    --name) name=$2 ;;
    --also) also+=(--exe "$2") ;;
    --package-version) package_version=(--package-version "$2") ;;
  esac
  shift 2
done
[[ $name == typescript || $name == tsc-rs || $name == @tsc-rs/typescript ]] || usage
[[ $name == typescript || ${#package_version[@]} == 2 ]] || usage
if [[ ${1:-} == --go ]]; then
  [[ $name == typescript && ${#also[@]} == 0 ]] || usage
  [[ $# == 3 ]] || usage
  go_version=$2 out=$3
else
  [[ $# == 2 ]] || usage
  out=$1 exe=$(realpath "$2")
fi
mkdir -p "$out"
out=$(realpath "$out")
go_dir=${NPM_PACK_GO_DIR:-$("$repo/scripts/upstream/pin.py" path goCheckout)}
read -r pin layout go_toolchain < <("$repo/scripts/upstream/pin.py" show |
  node -e 'const p = JSON.parse(require("fs").readFileSync(0, "utf8")); console.log(p.key, p.layout, p.oracle.go)')
case $layout in
  typescript)
    root=$(dirname "$go_dir") && input=$root/packages/typescript input_modules=$root/node_modules
    go_cmd=./cmd/tsc go_module=github.com/microsoft/TypeScript/tsc ;;
  typescript-go)
    root=$go_dir input=$go_dir/_packages/native-preview input_modules=$go_dir/_packages/native-preview/node_modules
    go_cmd=./cmd/tsgo go_module=github.com/microsoft/typescript-go ;;
  *) echo "unknown pin layout '$layout'" >&2; exit 1 ;;
esac
build="$out/.build"
rm -rf "$build"
mkdir -p "$build"

if [[ -n $go_version ]]; then
  exe="$build/tsc"
  (cd "$go_dir" && CGO_ENABLED=0 GOTOOLCHAIN=$go_toolchain go build -trimpath \
    "-ldflags=-s -w -X $go_module/internal/core.version=$go_version" \
    -tags=noembed -o "$exe" "$go_cmd")
  libs="$go_dir/internal/bundled/libs"
  git_head=$(git -C "$go_dir" rev-parse HEAD 2>/dev/null || "$repo/scripts/upstream/pin.py" path commit)
else
  [[ -x $exe ]] || { echo "not executable: $exe" >&2; exit 2; }
  libs="$build/libs"
  "$repo/crates/ts_goport/scripts/copy-libs.sh" "$libs"
  diff -rq -x loc "$libs" "$go_dir/internal/bundled/libs" > /dev/null ||
    { echo "the lib files of $repo differ from the pin's ($go_dir): wrong pin?" >&2; exit 1; }
  # The locale catalogs (lib/loc) are the pin's internal/diagnostics/loc/*.json.gz.
  diff -q <(cd "$libs/loc" && sha256sum -- *) \
    <(cd "$go_dir/internal/diagnostics/loc" && sha256sum -- *.json.gz) > /dev/null ||
    { echo "the locale catalogs of $repo differ from the pin's ($go_dir): wrong pin?" >&2; exit 1; }
  if [[ -f $(dirname "$exe")/COMMIT ]]; then git_head=$(cat "$(dirname "$exe")/COMMIT"); else git_head=$(git -C "$repo" rev-parse HEAD); fi
fi

# A noembed tsc starts only with the lib files next to it, as in the platform package.
bin="$build/bin"
cp -r "$libs" "$bin"
cp "$exe" "$bin/tsc"
reported=$("$bin/tsc" --version)
version=${reported#Version }
[[ $reported == "Version $version" && -n $version ]] || { echo "$exe --version printed '$reported'" >&2; exit 1; }
[[ -z $go_version || $version == "$go_version" ]] || { echo "$exe reports $version, not $go_version" >&2; exit 1; }
# It must be a noembed build: it lists the lib files next to it, not bundled:/// paths.
echo > "$build/a.ts"
listed=$("$bin/tsc" --listFilesOnly --lib es5 "$build/a.ts")
grep -q "^$bin/lib.es5.d.ts$" <<< "$listed" ||
  { echo "$exe is not a noembed build: it lists $(head -1 <<< "$listed")" >&2; exit 1; }

# The JS API (dist), as Go's `npm run -w <input package> build` makes it (tsc -b), in a copy of the
# input, so the checkout stays read-only. npm finds `tsc` in the root node_modules/.bin:
# @typescript/bundled-typescript, not the typescript package (another version, whose source maps
# differ). At N the input has no node_modules of its own (npm workspaces): it uses the root's.
src="$build/dist-src"
mkdir -p "$src"
for f in "$input"/*; do
  [[ $f == */node_modules || $f == */dist ]] || cp -r "$f" "$src/"
done
ln -s "$input_modules" "$src/node_modules"
"$root/node_modules/.bin/tsc" -b "$src"

native_bin=()
[[ -n $go_version ]] || native_bin=(--native-bin)
node "$repo/npm/pack.mjs" --layout "$layout" --go-dir "$go_dir" --exe "$bin/tsc" "${also[@]}" --libs "$libs" \
  --dist "$src/dist" --version "$version" --git-head "$git_head" --out "$out/pkg" --name "$name" \
  "${package_version[@]}" "${native_bin[@]}"
# Replace each package's dir and tarballs (npm pack names @scope/name scope-name-<v>.tgz).
for d in "$out"/pkg/*; do
  b=$(basename "$d")
  t=$(node -p 'require(process.argv[1]).name.replace(/^@/, "").replace("/", "-")' "$d/package.json")
  rm -rf "${out:?}/$b" "$out/$t"-[0-9]*.tgz
  mv "$d" "$out/"
  (cd "$out/$b" && npm pack --silent --pack-destination "$out" > /dev/null)
done
rmdir "$out/pkg"
rm -rf "$build"
echo "packed ${package_version[1]:-$version} (tsc $version, $([[ -n $go_version ]] && echo Go || echo Rust), pin $pin):"
ls -1 "$out"/*.tgz
