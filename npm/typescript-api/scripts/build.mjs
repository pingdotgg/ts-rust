// Builds lib/ from the typescript@6.0.3 npm package and the tsc-rs lib files.
//
// lib/typescript.js is TypeScript 6.0.3's own bundle with a few functions turned into hook points
// (see HOOKS). src/index.js fills the hooks, so programs, checkers and emit come from tsc-rs while
// every other export stays TypeScript 6.0.3's. tsserver.js and typingsInstaller.js are TypeScript
// 6.0.3's, on that bundle; lib/tsc.js runs tsc-rs's command line, as bin/tsc does. The locale
// directories hold TypeScript 6.0.3's translated messages. The lib.*.d.ts files are the ones tsc-rs
// embeds (crates/ts_goport/libs), so the files that tools read from disk are the files the compiler
// checks. The unchanged package goes to .cache/typescript-6.0.3: the tests compare with it.
//
// usage: node scripts/build.mjs
// TS6_TARBALL=<path> uses that typescript-6.0.3.tgz instead of the cache or the registry.

import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import zlib from "node:zlib";

const packageDir = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const repoDir = path.resolve(packageDir, "../..");
const libDir = path.join(packageDir, "lib");
const cacheDir = path.join(packageDir, ".cache");

const tarball = {
    url: "https://registry.npmjs.org/typescript/-/typescript-6.0.3.tgz",
    integrity: "sha512-y2TvuxSZPDyQakkFRPZHKFm+KKVqIisdg9/CZwm9ftvKXLP8NRWj38/ODjNbr43SsoXqNuAisEf1GdCxqWcdBw==",
    cache: path.join(cacheDir, "typescript-6.0.3.tgz"),
};
const stockDir = path.join(cacheDir, "typescript-6.0.3");

// Functions of the TypeScript 6.0.3 bundle that src/index.js can replace. Each becomes
// `function name(...args)` that calls `__tsrsHooks.name` when it is set and the original
// (`name__ts6`, also in `__tsrsOriginals`) when it is not.
const HOOKS = [
    "createProgram",
    "createBuilderProgram",
    "readBuilderProgram",
    "createTypeChecker",
    "createCompilerHostWorker",
    "transpileWorker",
    "createLanguageService",
];

// tsserver.js, typingsInstaller.js and their _*.js modules require ./typescript.js, so they run
// on the bundle with the hooks.
const copiedFiles = {
    "package/lib/typescript.d.ts": "typescript.d.ts",
    "package/lib/tsserverlibrary.js": "tsserverlibrary.js",
    "package/lib/tsserverlibrary.d.ts": "tsserverlibrary.d.ts",
    "package/lib/tsserver.js": "tsserver.js",
    "package/lib/_tsserver.js": "_tsserver.js",
    "package/lib/typingsInstaller.js": "typingsInstaller.js",
    "package/lib/_typingsInstaller.js": "_typingsInstaller.js",
    "package/lib/watchGuard.js": "watchGuard.js",
    "package/lib/typesMap.json": "typesMap.json",
    "package/LICENSE.txt": "TypeScript-LICENSE.txt",
    "package/ThirdPartyNoticeText.txt": "TypeScript-ThirdPartyNoticeText.txt",
};

// TypeScript 6.0's lib/tsc.js runs its command line when it is loaded (tools run it with node or
// require it); this one runs tsc-rs's.
const tscShim = `"use strict";

// Changed by tsc-rs (npm/typescript-api/scripts/build.mjs): the tsc command line of tsc-rs, as bin/tsc runs it.
require("../src/tsc.js").main(process.argv.slice(2));
`;

const files = readTar(zlib.gunzipSync(await readTarball()));
// The new lib/ is written next to the cache and takes the place of the old one when it is complete,
// so that processes that load the package meanwhile find whole files.
const partialDir = path.join(cacheDir, "lib.partial");
fs.rmSync(partialDir, { recursive: true, force: true });
fs.mkdirSync(partialDir, { recursive: true });

fs.writeFileSync(path.join(partialDir, "typescript.js"), patchTypeScript(entry(files, "package/lib/typescript.js").toString("utf8")));
for (const [from, to] of Object.entries(copiedFiles)) {
    fs.writeFileSync(path.join(partialDir, to), entry(files, from));
}
fs.writeFileSync(path.join(partialDir, "tsc.js"), tscShim);
for (const [name, { data }] of files) {
    const locale = /^package\/lib\/([a-z]{2}(?:-[a-z]{2})?)\/diagnosticMessages\.generated\.json$/.exec(name);
    if (locale !== null) {
        fs.mkdirSync(path.join(partialDir, locale[1]), { recursive: true });
        fs.writeFileSync(path.join(partialDir, locale[1], "diagnosticMessages.generated.json"), data);
    }
}

const goLibDir = path.join(repoDir, "crates/ts_goport/libs");
for (const name of fs.readdirSync(goLibDir)) {
    if (name.startsWith("lib.") && name.endsWith(".d.ts")) {
        fs.copyFileSync(path.join(goLibDir, name), path.join(partialDir, name));
    }
}
replaceDirectory(libDir, partialDir);

unpackStock(files);

console.log(`wrote ${path.relative(process.cwd(), libDir) || "."} and ${path.relative(process.cwd(), stockDir)}`);

async function readTarball() {
    const fromEnv = process.env.TS6_TARBALL;
    if (fromEnv) {
        return verified(fs.readFileSync(fromEnv), fromEnv);
    }
    if (fs.existsSync(tarball.cache)) {
        return verified(fs.readFileSync(tarball.cache), tarball.cache);
    }
    const response = await fetch(tarball.url);
    if (!response.ok) {
        throw new Error(`GET ${tarball.url}: ${response.status}`);
    }
    const bytes = verified(Buffer.from(await response.arrayBuffer()), tarball.url);
    fs.mkdirSync(path.dirname(tarball.cache), { recursive: true });
    fs.writeFileSync(tarball.cache, bytes);
    return bytes;
}

function verified(bytes, source) {
    const integrity = `sha512-${createHash("sha512").update(bytes).digest("base64")}`;
    if (integrity !== tarball.integrity) {
        throw new Error(`${source}: integrity ${integrity}, expected ${tarball.integrity}`);
    }
    return bytes;
}

// The ustar subset that npm tarballs use: regular files, with the name split over prefix and name.
// Each entry is { data, mode }.
function readTar(bytes) {
    const result = new Map();
    for (let offset = 0; offset + 512 <= bytes.length;) {
        const header = bytes.subarray(offset, offset + 512);
        if (header.every(byte => byte === 0)) {
            break;
        }
        const field = (start, length) => header.subarray(start, start + length).toString("utf8").replace(/\0.*$/s, "");
        const name = field(345, 155) ? `${field(345, 155)}/${field(0, 100)}` : field(0, 100);
        const size = parseInt(field(124, 12).trim() || "0", 8);
        const type = field(156, 1);
        if (type === "0" || type === "") {
            result.set(name, { data: bytes.subarray(offset + 512, offset + 512 + size), mode: parseInt(field(100, 8).trim() || "644", 8) });
        }
        offset += 512 + Math.ceil(size / 512) * 512;
    }
    return result;
}

function entry(files, name) {
    const file = files.get(name);
    if (file === undefined) {
        throw new Error(`typescript-6.0.3.tgz has no ${name}`);
    }
    return file.data;
}

// The unchanged package, written next to the tarball once. A new directory takes the place of the
// old one only when it is complete.
function unpackStock(files) {
    const marker = path.join(stockDir, ".integrity");
    if (fs.existsSync(marker) && fs.readFileSync(marker, "utf8") === tarball.integrity) {
        return;
    }
    const partial = `${stockDir}.partial`;
    fs.rmSync(partial, { recursive: true, force: true });
    for (const [name, { data, mode }] of files) {
        if (!name.startsWith("package/")) {
            continue;
        }
        const file = path.join(partial, name.slice("package/".length));
        fs.mkdirSync(path.dirname(file), { recursive: true });
        fs.writeFileSync(file, data, { mode: mode & 0o777 });
    }
    fs.writeFileSync(path.join(partial, ".integrity"), tarball.integrity);
    replaceDirectory(stockDir, partial);
}

function replaceDirectory(directory, replacement) {
    const old = `${replacement}.old`;
    fs.rmSync(old, { recursive: true, force: true });
    if (fs.existsSync(directory)) {
        fs.renameSync(directory, old);
    }
    fs.renameSync(replacement, directory);
    fs.rmSync(old, { recursive: true, force: true });
}

function patchTypeScript(text) {
    text = replaceOnce(
        text,
        "\nvar ts = {}; ((module) => {\n",
        "\nvar __tsrsHooks = {}, __tsrsOriginals;\nvar ts = {}; ((module) => {\n",
    );
    for (const name of HOOKS) {
        text = replaceOnce(
            text,
            `\nfunction ${name}(`,
            `\nfunction ${name}(...args) {\n  const hook = __tsrsHooks.${name};\n  return hook === undefined ? ${name}__ts6(...args) : hook(...args);\n}\nfunction ${name}__ts6(`,
        );
    }
    const originals = HOOKS.map(name => `${name}: ${name}__ts6`).join(", ");
    text = replaceOnce(
        text,
        "\nmodule.exports = __toCommonJS(typescript_exports);\n",
        `\nmodule.exports = __toCommonJS(typescript_exports);\n__tsrsOriginals = { ${originals} };\n`,
    );
    text = replaceOnce(
        text,
        "\n//# sourceMappingURL=typescript.js.map",
        `\nrequire("../src/index.js").install(ts, __tsrsHooks, __tsrsOriginals);\n`,
    );
    const header = "***************************************************************************** */\n";
    return replaceOnce(
        text,
        header,
        `${header}\n/*! Changed by tsc-rs (npm/typescript-api/scripts/build.mjs): hook points in ${HOOKS.join(", ")}, and src/index.js installs the hooks. */\n`,
    );
}

function replaceOnce(text, search, replacement) {
    const first = text.indexOf(search);
    if (first === -1 || text.indexOf(search, first + 1) !== -1) {
        throw new Error(`typescript.js: expected exactly one ${JSON.stringify(search)}`);
    }
    return text.slice(0, first) + replacement + text.slice(first + search.length);
}
