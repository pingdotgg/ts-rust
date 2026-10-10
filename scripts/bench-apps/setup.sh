#!/usr/bin/env bash
# Prepares the real-world app benchmark in the README (macOS arm64): the compilers, Bun canary and
# the apps at their measured commits, with dependencies installed without install scripts.
# usage: scripts/bench-apps/setup.sh <work-dir>
# Use a work dir whose name ends in .noindex, so Spotlight does not index the repos.
set -euo pipefail
[[ $# == 1 ]] || { sed -n '4p' "$0" >&2; exit 2; }
mkdir -p "$1" && cd "$1"
mkdir -p repos logs

# Compilers. TypeScript 6 and 7 both name their bin tsc, so run.sh calls each one by path.
[[ -f package.json ]] || echo '{ "private": true }' >package.json
npm i -q --no-audit --no-fund tsc-rs@0.2.0 typescript@7.0.2 ts6@npm:typescript@6.0.3
# bun check is in Bun canary. The README numbers are from bd599f5af; the canary URL always has the latest.
curl -fsSL -o bun.zip https://github.com/oven-sh/bun/releases/download/canary/bun-darwin-aarch64.zip
unzip -oq bun.zip && rm bun.zip
echo "bun $(bun-darwin-aarch64/bun --revision)"

checkout() { # name url sha
  [[ -d repos/$1 ]] && return
  git init -q "repos/$1" && git -C "repos/$1" fetch -q --depth 1 "$2" "$3" && git -C "repos/$1" checkout -q FETCH_HEAD
}
install() { # name
  cd "repos/$1"
  if [[ -f pnpm-lock.yaml ]]; then pnpm install --frozen-lockfile --ignore-scripts
  elif [[ -f yarn.lock ]]; then corepack yarn install --frozen-lockfile --ignore-scripts
  else npm ci --ignore-scripts --no-audit --no-fund
  fi
}
while read -r name url sha; do
  checkout "$name" "$url" "$sha"
  (install "$name") >"logs/install-$name.log" 2>&1 || { echo "install failed: $name (logs/install-$name.log)" >&2; exit 1; }
done <<'EOF'
vscode https://github.com/microsoft/vscode 3f07e1aba32acacb8b08ae91bfdc954b580ad1fd
sentry https://github.com/getsentry/sentry 8294650589dbd26f230c73f4ab26b62a68aede8f
playwright https://github.com/microsoft/playwright d469960fdfc461e2d5795a3fa48a58a52a91ecaf
typeorm https://github.com/typeorm/typeorm c64a1f052fc39f6688b6b73b83d065d7147ba8bb
excalidraw https://github.com/excalidraw/excalidraw 53973c3a423fbd75a4ce68107786b4fcb90e4968
trpc https://github.com/trpc/trpc d756e591a5e37ef20b8d75ecd4d736c195497289
EOF

# The fixes that make each config check with 0 errors under tsc 7. Nothing else changes.
# VS Code: its postinstall puts the electron typings in node_modules.
if [[ ! -d repos/vscode/node_modules/electron ]]; then
  mkdir -p tmp-electron && (cd tmp-electron && echo '{ "private": true }' >package.json &&
    npm i -q --ignore-scripts --no-audit --no-fund electron@43.7.7)
  cp -R tmp-electron/node_modules/electron repos/vscode/node_modules/electron && rm -rf tmp-electron
fi
# Playwright: its build generates the injected sources that the checked files import.
(cd repos/playwright && node utils/generate_injected.js)
# Excalidraw: TS 7 removed baseUrl. The paths are already relative to the config.
grep -v '"baseUrl"' repos/excalidraw/tsconfig.json >repos/excalidraw/tsconfig.bench.json
# TypeORM: TS 7 removed moduleResolution node (node10). The package is CommonJS, so nodenext keeps
# its extensionless imports.
echo '{ "extends": "./tsconfig.json", "compilerOptions": { "module": "nodenext", "moduleResolution": "nodenext" } }' \
  >repos/typeorm/packages/typeorm/tsconfig.bench.json
echo ready
