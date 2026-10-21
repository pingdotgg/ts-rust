#!/usr/bin/env bash
# Installs the drop-in @tsc-rs/typescript from a local npm registry (verdaccio) under the name
# `typescript`, as a project does with "typescript": "npm:@tsc-rs/typescript@<v>", and checks each
# install: npm, pnpm (isolated, hoisted, enableGlobalVirtualStore) and bun. Linux x64 only, with
# Node 24, npm 11, pnpm 12 ($PNPM, default pnpm) and verdaccio 6 on PATH (bun when on PATH). Run
# it on a boat sandbox, not on zbook: it installs from npmjs and starts many tsc processes.
#
# usage: npm-alias-test.sh <pkg-dir> <work-dir>
#          <pkg-dir>: npm-pack.sh --name @tsc-rs/typescript output (tsc-rs-typescript-<v>.tgz and
#          the platform .tgz files; pack with --also darwin-arm64=<any tsc> for the platform
#          mismatch checks). A tsc-rs-<v>.tgz there adds the check of "typescript": "npm:tsc-rs@<v>".
#          <work-dir> is made fresh: <work-dir>/registry (verdaccio config, storage, npmrc), the
#          caches and one project per install. The registry keeps running after the run.
#        npm-alias-test.sh serve <work-dir>   (re)starts the registry on the saved storage
#        npm-alias-test.sh stop <work-dir>    stops it
#        npm-alias-test.sh url                prints the registry URL
# To install from the registry by hand: `npm install --registry "$(npm-alias-test.sh url)" ...`, or
# in a pnpm project `@tsc-rs:registry=<url>` in .npmrc (the other packages still come from npmjs).
# NPM_ALIAS_PORT is the registry port (default 4873, on 127.0.0.1). The registry serves @tsc-rs/*
# and tsc-rs from its storage only, and proxies every other package to registry.npmjs.org.
# Checks, per install:
#   - node_modules/.bin/tsc --version, and require("typescript").version: the tsc version.
#   - bin/tsc is the sh and JS polyglot when the postinstall ran (npm/install.js).
#   - import("typescript/unstable/sync") gives the API class.
#   - tsc -p on a tiny project: exit 2 with TS2322 for bad.ts, and the emit of ok.ts.
#   - VS Code's resolver (resolveTsdkPathToExe, the JS copy below) on node_modules/typescript/lib
#     finds lib/tsc of @tsc-rs/linux-x64, with the npm version as the version it shows.
#   - that tsc, started as VS Code starts it (--lsp --stdio), answers an LSP initialize request and
#     stops after shutdown and exit. (Its exit code is 1, with "context canceled" on stderr, as
#     Go's tsc at the pin does.)
# Platform mismatch: npm install --os=darwin --cpu=arm64, and pnpm with supportedArchitectures
# darwin arm64, succeed, install no @tsc-rs/linux-x64, and install @tsc-rs/darwin-arm64 when it is
# in <pkg-dir>.
# Peer dependencies: installs next to valibot 1.2.0 (an optional peer typescript >=5) and next to
# ts-api-utils 2.1.0 (a peer typescript >=4.8.4), with npm, pnpm and bun. Their errors and warnings
# are printed as "info" lines, not checked. Checked: npm with "overrides": {"typescript":
# "$typescript"} and pnpm with peerDependencyRules.allowAny [typescript] install ts-api-utils with
# no peer error or warning.
# Prints one line per check and ends with "npm-alias-test: PASS" or "npm-alias-test: FAIL (<n>)".
set -uo pipefail
usage() { sed -n '8,20p' "$0" >&2; exit 2; }
port=${NPM_ALIAS_PORT:-4873}
url=http://127.0.0.1:$port/
PNPM=${PNPM:-pnpm}

registry_up() { curl -sf "${url}-/ping" > /dev/null; }
registry_start() { # registry_start <registry dir>
  local reg=$1
  ! registry_up || { echo "a registry already answers at $url" >&2; return 1; }
  # setsid: the registry outlives this shell (a boat exec ends its process group).
  setsid nohup verdaccio --config "$reg/config.yaml" --listen "127.0.0.1:$port" > "$reg/verdaccio.log" 2>&1 < /dev/null &
  echo $! > "$reg/pid"
  for _ in $(seq 150); do registry_up && return 0; sleep 0.2; done
  echo "verdaccio did not start: $(tail -3 "$reg/verdaccio.log")" >&2
  return 1
}
registry_stop() { # registry_stop <registry dir>
  local reg=$1
  if [[ -f $reg/pid ]]; then kill "$(cat "$reg/pid")" 2> /dev/null; rm -f "$reg/pid"; fi
  for _ in $(seq 50); do registry_up || return 0; sleep 0.2; done
}

case ${1:-} in
  url) echo "$url"; exit 0 ;;
  serve) [[ $# == 2 ]] || usage; reg=$(realpath "$2")/registry; registry_stop "$reg"; registry_start "$reg" || exit 1
    echo "registry $url (storage $reg/storage)"; exit 0 ;;
  stop) [[ $# == 2 ]] || usage; registry_stop "$(realpath "$2")/registry"; exit 0 ;;
  help | -h | --help) usage ;;
esac
[[ $# == 2 ]] || usage
pkg=$(realpath "$1") work=$(realpath -m "$2")
main_tgz=("$pkg"/tsc-rs-typescript-[0-9]*.tgz)
[[ ${#main_tgz[@]} == 1 && -f ${main_tgz[0]} ]] || { echo "no single tsc-rs-typescript-<v>.tgz in $pkg" >&2; exit 2; }
for tool in node npm curl verdaccio "$PNPM"; do
  command -v "$tool" > /dev/null || { echo "missing $tool" >&2; exit 2; }
done
[[ $("$PNPM" --version) == 12.* ]] || { echo "PNPM=$PNPM is pnpm $("$PNPM" --version), not 12" >&2; exit 2; }
meta=$(tar -xOzf "${main_tgz[0]}" package/package.json)
field() { node -p 'const p = JSON.parse(process.argv[1]); process.argv[2].split(".").reduce((v, k) => v?.[k], p) ?? ""' "$meta" "$1"; }
version=$(field version) tsc_version=$(field tscVersion)
want="Version $tsc_version"
# The platform tarballs, from the aliases: "@typescript/typescript-linux-x64": "npm:@tsc-rs/linux-x64@<v>".
mapfile -t platforms < <(node -e '
  for (const spec of Object.values(JSON.parse(process.argv[1]).optionalDependencies ?? {})) {
    const m = spec.match(/^npm:@tsc-rs\/([a-z0-9]+-[a-z0-9]+)@/);
    if (m) console.log(m[1]);
  }' "$meta")
publish=()
for p in "${platforms[@]}"; do
  [[ -f $pkg/tsc-rs-$p-$version.tgz ]] && publish+=("$pkg/tsc-rs-$p-$version.tgz")
done
[[ -f $pkg/tsc-rs-linux-x64-$version.tgz ]] || { echo "no tsc-rs-linux-x64-$version.tgz in $pkg" >&2; exit 2; }
tsc_rs_tgz=$pkg/tsc-rs-$version.tgz
[[ -f $tsc_rs_tgz ]] && publish+=("$tsc_rs_tgz")
publish+=("${main_tgz[0]}")

reg=$work/registry
[[ ! -d $reg ]] || registry_stop "$reg"
rm -rf "$work"
mkdir -p "$reg" "$work/tools"
cat > "$reg/config.yaml" << 'EOF'
storage: ./storage
auth:
  htpasswd:
    file: ./htpasswd
    max_users: 100
uplinks:
  npmjs:
    url: https://registry.npmjs.org/
packages:
  "@tsc-rs/*":
    access: $all
    publish: $authenticated
  tsc-rs:
    access: $all
    publish: $authenticated
  "**":
    access: $all
    publish: $authenticated
    proxy: npmjs
max_body_size: 1gb
log: { type: stdout, format: pretty, level: warn }
EOF
registry_start "$reg" || exit 1
# A local user; its token stays in <work-dir>/registry/npmrc.
token=$(curl -sf -X PUT -H 'content-type: application/json' -d '{"name":"t3pkg1","password":"t3pkg1-local"}' \
  "${url}-/user/org.couchdb.user:t3pkg1" | node -p 'JSON.parse(require("fs").readFileSync(0, "utf8")).token')
[[ -n $token && $token != undefined ]] || { echo "no registry token" >&2; exit 1; }
printf 'registry=%s\n//127.0.0.1:%s/:_authToken=%s\n' "$url" "$port" "$token" > "$reg/npmrc"
tag=latest
[[ $version != *-* ]] || tag=next
for f in "${publish[@]}"; do
  npm publish "$f" --userconfig "$reg/npmrc" --tag "$tag" --access public > "$reg/publish.log" 2>&1 ||
    { echo "npm publish $f failed: $(tail -3 "$reg/publish.log")" >&2; exit 1; }
done
echo "registry $url: published $(printf '%s ' "${publish[@]##*/}")(dist-tag $tag)"
echo "npm $(npm --version), pnpm $("$PNPM" --version), node $(node --version)$(command -v bun > /dev/null && echo ", bun $(bun --version)")"

fails=0
# Take each status in r before the check: a $(...) in the label would reset $?.
check() { # check <name> <ok 0|1> [detail]
  if [[ $2 == 0 ]]; then echo "ok   $1"; else echo "FAIL $1${3:+: $3}"; fails=$((fails + 1)); fi
}
info() { sed 's/^/info /'; }

# VS Code's resolver: a copy of resolveTsdkPathToExe and its helpers in microsoft/TypeScript
# packages/vscode-typescript (the same lines at the pin fed0bf24149f and at main 538671d3c of
# 2026-10-21): src/util.ts lines 305-341 (resolveTsdkPathToExe), 343-353 (realpathUri) and 48
# (packagedExeBaseNames), src/tsdkPackage.ts lines 50-54 (resolvePackageExecutable). node:fs replaces
# vscode.workspace.fs; the tsdk path is absolute, so workspaceResolve is path.resolve. Prints the
# ExeInfo as JSON.
cat > "$work/tools/resolve-tsdk.mjs" << 'EOF'
import fs from "node:fs";
import module from "node:module";
import path from "node:path";

const packagedExeBaseNames = ["tsc", "tsgo"];

function resolvePackageExecutable(packageJsonPath, platformPackage, exeName) {
    const require = module.createRequire(packageJsonPath);
    const platformPackageJson = require.resolve(`@typescript/${platformPackage}/package.json`);
    return path.join(path.dirname(platformPackageJson), "lib", exeName);
}

function realpath(p) {
    try {
        return fs.realpathSync(p);
    }
    catch {
        return p;
    }
}

function resolveTsdkPathToExe(tsdkPath) {
    const resolved = realpath(path.resolve(tsdkPath));
    for (const packagePath of [resolved, path.join(resolved, "..")]) {
        try {
            const packageJsonPath = path.join(packagePath, "package.json");
            const packageJson = JSON.parse(fs.readFileSync(packageJsonPath, "utf8"));
            const name = packageJson.name;
            const bin = packageJson.bin;
            if (typeof name !== "string" || !bin || typeof bin !== "object") continue;
            const baseName = name.startsWith("@") ? name.split("/")[1] : name;
            if (!baseName) continue;
            const expectedBinName = baseName === "typescript" ? "tsc" : "tsgo";
            if (!Object.prototype.hasOwnProperty.call(bin, expectedBinName)) continue;
            const exeName = `${expectedBinName}${process.platform === "win32" ? ".exe" : ""}`;
            const platformPackage = `${baseName}-${process.platform}-${process.arch}`;
            const exePath = resolvePackageExecutable(packageJsonPath, platformPackage, exeName);
            fs.statSync(exePath);
            return { path: exePath, version: typeof packageJson.version === "string" ? packageJson.version : "unknown" };
        }
        catch {}
    }
    for (const baseName of packagedExeBaseNames) {
        try {
            const exePath = path.join(resolved, `${baseName}${process.platform === "win32" ? ".exe" : ""}`);
            fs.statSync(exePath);
            return { path: exePath, version: "(local)", isLocal: true };
        }
        catch {}
    }
}

console.log(JSON.stringify(resolveTsdkPathToExe(process.argv[2]) ?? null));
EOF

# An LSP client as VS Code starts the server: <exe> --lsp --stdio (client.ts, vscode-languageclient
# TransportKind.stdio adds --stdio). It sends initialize, initialized, shutdown and exit, answers
# server requests with null, and prints the serverInfo, the number of capabilities and the exit
# code. It passes when the server answers initialize and stops after exit.
cat > "$work/tools/lsp-init.mjs" << 'EOF'
import { spawn } from "node:child_process";
import { pathToFileURL } from "node:url";

const [exe, root] = process.argv.slice(2);
const child = spawn(exe, ["--lsp", "--stdio"], { cwd: root, stdio: ["pipe", "pipe", "inherit"] });
const send = message => {
    const body = Buffer.from(JSON.stringify({ jsonrpc: "2.0", ...message }));
    child.stdin.write(`Content-Length: ${body.length}\r\n\r\n`);
    child.stdin.write(body);
};
let initialized = false;
const timer = setTimeout(() => {
    console.log("no answer in 20 s");
    child.kill();
    process.exit(1);
}, 20000);
let buffer = Buffer.alloc(0);
child.stdout.on("data", data => {
    buffer = Buffer.concat([buffer, data]);
    for (;;) {
        const end = buffer.indexOf("\r\n\r\n");
        if (end < 0) return;
        const length = Number(/Content-Length: (\d+)/i.exec(buffer.subarray(0, end).toString())?.[1]);
        if (buffer.length < end + 4 + length) return;
        const message = JSON.parse(buffer.subarray(end + 4, end + 4 + length).toString());
        buffer = buffer.subarray(end + 4 + length);
        if (message.id === 1 && message.result) {
            initialized = true;
            const { serverInfo, capabilities } = message.result;
            console.log(JSON.stringify({ serverInfo, capabilities: Object.keys(capabilities ?? {}).length }));
            send({ method: "initialized", params: {} });
            send({ id: 2, method: "shutdown" });
        }
        else if (message.id === 2) {
            send({ method: "exit" });
        }
        else if (message.method && message.id !== undefined) {
            send({ id: message.id, result: null });
        }
    }
});
child.on("exit", code => {
    clearTimeout(timer);
    console.log(`exit ${code}`);
    process.exit(initialized ? 0 : 1);
});
send({
    id: 1,
    method: "initialize",
    params: { processId: process.pid, rootUri: pathToFileURL(root).href, capabilities: {}, workspaceFolders: null },
});
EOF

spec="npm:@tsc-rs/typescript@$version"
# project <dir> [extra dependencies JSON]: a package.json with typescript as the alias.
project() {
  mkdir -p "$1"
  echo "{\"name\":\"alias-test\",\"private\":true,\"devDependencies\":{\"typescript\":\"$spec\"${2:+,$2}}}" > "$1/package.json"
}
pnpm_config() { # pnpm_config <dir> <yaml lines...>: a fresh store and cache per project
  local d=$1
  shift
  printf '%s\n' "storeDir: $d.store" "cacheDir: $d.cache" 'allowBuilds:' '  "@tsc-rs/typescript": true' "$@" > "$d/pnpm-workspace.yaml"
}
pnpm_install() { # pnpm_install <dir> <log>
  (cd "$1" && "$PNPM" install --registry="$url" > "$2" 2>&1)
}

# checks <tag> <dir> <postinstall 0|1>: the checks of one install.
checks() {
  local tag=$1 d=$2 native=$3 v b r exe lib_pkg
  local nm=$d/node_modules
  v=$("$nm/.bin/tsc" --version 2>&1)
  [[ $v == "$want" ]]
  r=$?
  check "$tag .bin/tsc --version: $want" $r "$v"
  b=$nm/typescript/bin/tsc
  if ((native)); then
    [[ $(sed -n 1p "$b") == "#!/bin/sh" ]]
    r=$?
    check "$tag bin/tsc is the sh and JS polyglot (the postinstall ran)" $r "$(head -1 "$b")"
  else
    head -1 "$b" | grep -q node
    r=$?
    check "$tag bin/tsc is Go's JS launcher (no postinstall)" $r "$(head -1 "$b")"
  fi
  v=$(cd "$d" && node -e 'console.log(require("typescript").version)' 2>&1)
  [[ $v == "$tsc_version" ]]
  r=$?
  check "$tag require(\"typescript\").version: $tsc_version" $r "$v"
  v=$(cd "$d" && node --input-type=module -e 'const m = await import("typescript/unstable/sync"); console.log(typeof m.API)' 2>&1)
  [[ $v == function ]]
  r=$?
  check "$tag import(\"typescript/unstable/sync\") gives the API class" $r "$v"
  mkdir -p "$d/tiny"
  echo 'export const x: number = 1;' > "$d/tiny/ok.ts"
  echo 'const y: number = "a";' > "$d/tiny/bad.ts"
  echo '{"compilerOptions":{"strict":true,"declaration":true,"outDir":"out"},"files":["ok.ts","bad.ts"]}' > "$d/tiny/tsconfig.json"
  v=$(cd "$d" && node_modules/.bin/tsc -p tiny --pretty false 2>&1)
  r=$?
  [[ $r == 2 && $v == *"bad.ts(1,7): error TS2322"* ]] && grep -qx 'export const x = 1;' "$d/tiny/out/ok.js" &&
    [[ -f $d/tiny/out/ok.d.ts ]]
  r=$?
  check "$tag tsc -p tiny: exit 2, TS2322 in bad.ts, ok.js and ok.d.ts" $r "exit $r: $v"
  v=$(node "$work/tools/resolve-tsdk.mjs" "$nm/typescript/lib" 2>&1)
  exe=$(node -p 'JSON.parse(process.argv[1])?.path ?? ""' "$v" 2> /dev/null)
  lib_pkg=$(node -p 'const p = require(process.argv[1]); p.name + "@" + p.version' "$(realpath -m "$exe/../../package.json")" 2> /dev/null)
  [[ $(node -p 'JSON.parse(process.argv[1]).version' "$v" 2> /dev/null) == "$version" && $lib_pkg == "@tsc-rs/linux-x64@$version" &&
    $("$exe" --version 2>&1) == "$want" ]]
  r=$?
  check "$tag VS Code resolveTsdkPathToExe(node_modules/typescript/lib): lib/tsc of @tsc-rs/linux-x64, version $version" $r "$v ($lib_pkg)"
  echo "info $tag   resolved ${exe#"$d"/}"
  v=$(node "$work/tools/lsp-init.mjs" "$exe" "$d/tiny" 2>&1)
  r=$?
  [[ $r == 0 ]]
  check "$tag tsc --lsp --stdio answers initialize: $(head -1 <<< "$v")" $r "$v"
}

# npm
d=$work/npm
project "$d"
if (cd "$d" && npm install --no-audit --no-fund --registry "$url" --cache "$work/npm-cache" > "$work/npm.log" 2>&1); then
  check "npm install typescript@$spec" 0
  grep -i -E 'warn' "$work/npm.log" | info
  checks npm "$d" 1
else
  check "npm install typescript@$spec" 1 "$(tail -5 "$work/npm.log")"
fi

# pnpm: isolated, hoisted and the global virtual store.
declare -A pnpm_modes=([isolated]="nodeLinker: isolated" [hoisted]="nodeLinker: hoisted"
  [gvs]="enableGlobalVirtualStore: true")
for mode in isolated hoisted gvs; do
  d=$work/pnpm-$mode
  project "$d"
  pnpm_config "$d" "${pnpm_modes[$mode]}"
  if pnpm_install "$d" "$work/pnpm-$mode.log"; then
    check "pnpm $mode install typescript@$spec" 0
    grep -i -E 'warn|ignored|skipped' "$work/pnpm-$mode.log" | info
    checks "pnpm-$mode" "$d" 1
  else
    check "pnpm $mode install typescript@$spec" 1 "$(tail -5 "$work/pnpm-$mode.log")"
  fi
done
# The global virtual store is in the store dir, not in the project.
real=$(realpath "$work/pnpm-gvs/node_modules/typescript")
[[ $real == "$work/pnpm-gvs.store/"* ]]
r=$?
check "pnpm gvs: node_modules/typescript is in the global virtual store" $r "$real"

# bun, with the postinstall trusted. bun names a trusted dependency by its install name.
if command -v bun > /dev/null; then
  d=$work/bun
  project "$d"
  node -e 'const f = process.argv[1]; const p = require(f); p.trustedDependencies = ["typescript"];
    require("fs").writeFileSync(f, JSON.stringify(p))' "$d/package.json"
  printf '[install]\nregistry = "%s"\n[install.cache]\ndir = "%s"\n' "$url" "$work/bun-cache" > "$d/bunfig.toml"
  if (cd "$d" && bun install > "$work/bun.log" 2>&1); then
    check "bun install typescript@$spec" 0
    checks bun "$d" 1
  else
    check "bun install typescript@$spec" 1 "$(tail -5 "$work/bun.log")"
  fi
else
  echo "skip bun (not on PATH)"
fi

# "typescript": "npm:tsc-rs@<v>": its getExePath.js finds @tsc-rs/linux-x64 by the fixed name.
if [[ -f $tsc_rs_tgz ]]; then
  d=$work/npm-tsc-rs
  mkdir -p "$d"
  echo "{\"name\":\"alias-test\",\"private\":true,\"devDependencies\":{\"typescript\":\"npm:tsc-rs@$version\"}}" > "$d/package.json"
  if (cd "$d" && npm install --no-audit --no-fund --registry "$url" --cache "$work/npm-cache" > "$work/npm-tsc-rs.log" 2>&1); then
    v=$("$d/node_modules/.bin/tsc-rs" --version 2>&1)
    r=$(node "$d/node_modules/typescript/lib/tsc.js" --version 2>&1)
    [[ $v == "$want" && $r == "$want" ]]
    r=$?
    check "npm typescript@npm:tsc-rs@$version: .bin/tsc-rs and lib/tsc.js --version: $want" $r "$v / $r"
  else
    check "npm install typescript@npm:tsc-rs@$version" 1 "$(tail -5 "$work/npm-tsc-rs.log")"
  fi
fi

# Platform mismatch.
darwin=0
[[ -f $pkg/tsc-rs-darwin-arm64-$version.tgz ]] && darwin=1
d=$work/npm-darwin-arm64
project "$d"
if (cd "$d" && npm install --os=darwin --cpu=arm64 --no-audit --no-fund --registry "$url" --cache "$work/npm-cache" > "$work/npm-darwin-arm64.log" 2>&1); then
  plat=$(node -p 'require(process.argv[1]).name' "$d/node_modules/@typescript/typescript-darwin-arm64/package.json" 2> /dev/null)
  [[ ! -e $d/node_modules/@typescript/typescript-linux-x64 ]] && { ((!darwin)) || [[ $plat == @tsc-rs/darwin-arm64 ]]; }
  r=$?
  check "npm --os=darwin --cpu=arm64: no linux-x64$( ((darwin)) && echo ", @typescript/typescript-darwin-arm64 is @tsc-rs/darwin-arm64")" $r \
    "$(ls "$d/node_modules/@typescript" 2>&1)"
  node -e 'const l = require(process.argv[1]).packages;
    for (const [k, v] of Object.entries(l)) if (k.includes("@typescript/")) console.log(k, v.resolved?.replace(/^.*\//, ""), v.optional ? "optional" : "")' \
    "$d/package-lock.json" | info
else
  check "npm --os=darwin --cpu=arm64 install" 1 "$(tail -5 "$work/npm-darwin-arm64.log")"
fi
d=$work/pnpm-darwin-arm64
project "$d"
pnpm_config "$d" "supportedArchitectures:" "  os: [darwin]" "  cpu: [arm64]"
if pnpm_install "$d" "$work/pnpm-darwin-arm64.log"; then
  v=$(ls "$d/node_modules/.pnpm")
  ! grep -q '^@tsc-rs+linux-x64@' <<< "$v" && { ((!darwin)) || grep -q '^@tsc-rs+darwin-arm64@' <<< "$v"; }
  r=$?
  check "pnpm supportedArchitectures darwin arm64: no linux-x64$( ((darwin)) && echo ", @tsc-rs/darwin-arm64")" $r "$v"
else
  check "pnpm supportedArchitectures darwin arm64 install" 1 "$(tail -5 "$work/pnpm-darwin-arm64.log")"
fi

# Peer dependencies on typescript see the npm version of the alias.
for peer in valibot:1.2.0 ts-api-utils:2.1.0; do
  dep="\"${peer%%:*}\":\"${peer#*:}\""
  d=$work/peer-npm-${peer%%:*}
  project "$d" "$dep"
  (cd "$d" && npm install --no-audit --no-fund --registry "$url" --cache "$work/npm-cache" > "$d.log" 2>&1)
  echo "info peer npm + $peer: exit $?"
  grep -E 'ERESOLVE|peer|Conflicting|Found:|Could not resolve|overridden|invalid' "$d.log" | head -8 | info
  d=$work/peer-pnpm-${peer%%:*}
  project "$d" "$dep"
  pnpm_config "$d"
  pnpm_install "$d" "$d.log"
  echo "info peer pnpm + $peer: exit $?"
  grep -B1 -A6 -i 'peer dep' "$d.log" | head -10 | info
  if command -v bun > /dev/null; then
    d=$work/peer-bun-${peer%%:*}
    project "$d" "$dep"
    printf '[install]\nregistry = "%s"\n[install.cache]\ndir = "%s"\n' "$url" "$work/bun-cache" > "$d/bunfig.toml"
    (cd "$d" && bun install > "$d.log" 2>&1)
    echo "info peer bun + $peer: exit $?"
    grep -i -E 'peer|warn|error' "$d.log" | head -4 | info
  fi
done
# The fixes: npm overrides the peer spec with the root dependency, pnpm allows any typescript.
d=$work/peer-npm-overrides
project "$d" '"ts-api-utils":"2.1.0"'
node -e 'const f = process.argv[1]; const p = require(f); p.overrides = { typescript: "$typescript" };
  require("fs").writeFileSync(f, JSON.stringify(p))' "$d/package.json"
(cd "$d" && npm install --no-audit --no-fund --registry "$url" --cache "$work/npm-cache" > "$d.log" 2>&1)
r=$?
[[ $r == 0 ]] && ! grep -qE 'ERESOLVE|invalid' "$d.log" && [[ $(cd "$d" && node_modules/.bin/tsc --version) == "$want" ]]
r=$?
check "npm overrides {\"typescript\": \"\$typescript\"}: ts-api-utils installs with no peer error" $r "exit $r: $(grep -m3 -E 'ERESOLVE|error' "$d.log")"
d=$work/peer-pnpm-allow
project "$d" '"ts-api-utils":"2.1.0"'
pnpm_config "$d" "peerDependencyRules:" "  allowAny: [typescript]"
pnpm_install "$d" "$d.log"
r=$?
[[ $r == 0 ]] && ! grep -qi 'peer dep' "$d.log"
r=$?
check "pnpm peerDependencyRules.allowAny [typescript]: ts-api-utils installs with no peer warning" $r "exit $r: $(tail -5 "$d.log")"

echo "registry still up at $url (storage $reg/storage); stop it with: $0 stop $work"
if ((fails)); then echo "npm-alias-test: FAIL ($fails)"; exit 1; fi
echo "npm-alias-test: PASS"
