// Upstream sources at pinned commits (cached), app copies, and installing the typescript package
// under test into them.

import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";

import { cacheDir, copyTree, harnessDir, hashTree, manifestHash, readJson, removePath, writeJson } from "./util.mjs";

export const pins = readJson(path.join(harnessDir, "pins.json"));

function git(cwd, args) {
    return execFileSync("git", args, { cwd, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] }).trim();
}

// A checkout of pins.sources[name] at its commit, cloned once into the cache.
export function ensureSource(name) {
    const pin = pins.sources[name];
    const dir = path.join(cacheDir, "src", `${name}@${pin.commit.slice(0, 12)}`);
    if (fs.existsSync(path.join(dir, ".git")) && git(dir, ["rev-parse", "HEAD"]) === pin.commit) {
        return dir;
    }
    const tmp = `${dir}.tmp`;
    removePath(tmp);
    fs.mkdirSync(tmp, { recursive: true });
    git(tmp, ["init", "-q"]);
    git(tmp, ["remote", "add", "origin", pin.repo]);
    if (pin.sparse !== undefined) {
        git(tmp, ["sparse-checkout", "set", "--cone", ...pin.sparse]);
    }
    git(tmp, ["fetch", "-q", "--depth", "1", "--filter=blob:none", "origin", pin.commit]);
    git(tmp, ["checkout", "-q", "FETCH_HEAD"]);
    const head = git(tmp, ["rev-parse", "HEAD"]);
    if (head !== pin.commit) {
        throw new Error(`${name}: checked out ${head}, expected ${pin.commit}`);
    }
    removePath(dir);
    fs.renameSync(tmp, dir);
    return dir;
}

// A fresh copy of `from` at `appDir`; node_modules and build outputs of an earlier run go too.
export function prepareAppDir(from, appDir) {
    removePath(appDir);
    copyTree(from, appDir, (name) => name === ".git" || name === "node_modules" || name === "dist");
}

// TS 6.0 parsed package.json shape: the typescript dependency becomes the packed tarball and an
// override makes every other copy in the tree the same package (@nestjs/cli pins its own typescript).
export function useTarballInPackageJson(appDir, tgz, { extraDependencies = {}, extraDevDependencies = {} } = {}) {
    const file = path.join(appDir, "package.json");
    const pkg = readJson(file);
    const section = pkg.dependencies?.typescript !== undefined ? "dependencies" : "devDependencies";
    pkg[section] = { ...pkg[section], typescript: `file:${tgz}` };
    pkg.dependencies = { ...pkg.dependencies, ...extraDependencies };
    pkg.devDependencies = { ...pkg.devDependencies, ...extraDevDependencies };
    pkg.overrides = { ...pkg.overrides, typescript: "$typescript" };
    fs.writeFileSync(file, `${JSON.stringify(pkg, null, 2)}\n`);
    return pkg;
}

// Drops the typescript entries of a package-lock.json so that npm resolves typescript from the
// package.json spec (the tarball) and never from a lock entry of an earlier tarball.
export function dropTypescriptFromLock(lockFile) {
    if (!fs.existsSync(lockFile)) {
        return false;
    }
    const lock = readJson(lockFile);
    for (const key of Object.keys(lock.packages ?? {})) {
        if (key === "node_modules/typescript" || key.endsWith("/node_modules/typescript")) {
            delete lock.packages[key];
        }
    }
    fs.writeFileSync(lockFile, `${JSON.stringify(lock, null, 2)}\n`);
    return true;
}

export function cachedLockFile(key) {
    return path.join(cacheDir, "locks", `${key}.package-lock.json`);
}

// Restores a lockfile saved by an earlier run, so that transitive versions do not drift between
// the two runs of a comparison.
export function restoreCachedLock(key, appDir) {
    const cached = cachedLockFile(key);
    if (!fs.existsSync(cached)) {
        return false;
    }
    fs.copyFileSync(cached, path.join(appDir, "package-lock.json"));
    return true;
}

export function saveCachedLock(key, appDir) {
    const cached = cachedLockFile(key);
    const lockFile = path.join(appDir, "package-lock.json");
    if (fs.existsSync(cached) || !fs.existsSync(lockFile)) {
        return;
    }
    fs.mkdirSync(path.dirname(cached), { recursive: true });
    fs.copyFileSync(lockFile, cached);
    dropTypescriptFromLock(cached);
}

export function npmEnv(baseEnv) {
    return {
        ...baseEnv,
        npm_config_cache: path.join(cacheDir, "npm"),
        npm_config_prefer_offline: "true",
        npm_config_audit: "false",
        npm_config_fund: "false",
        npm_config_update_notifier: "false",
        npm_config_progress: "false",
        npm_config_loglevel: "warn",
    };
}

// Every node_modules/typescript directory of the app (nested copies included).
function findTypescriptCopies(appDir) {
    const found = [];
    walk(path.join(appDir, "node_modules"), "node_modules");
    return found;

    function walk(dir, rel) {
        let entries;
        try {
            entries = fs.readdirSync(dir, { withFileTypes: true });
        } catch {
            return;
        }
        for (const entry of entries) {
            if (!entry.isDirectory() && !entry.isSymbolicLink()) {
                continue;
            }
            const abs = path.join(dir, entry.name);
            const relPath = `${rel}/${entry.name}`;
            if (entry.name === "typescript") {
                found.push(relPath);
                walkNested(abs, relPath);
            } else if (entry.name.startsWith("@")) {
                walk(abs, relPath);
            } else if (!entry.name.startsWith(".")) {
                walkNested(abs, relPath);
            }
        }
    }

    function walkNested(pkgDir, rel) {
        const nested = path.join(pkgDir, "node_modules");
        if (fs.existsSync(nested)) {
            walk(nested, `${rel}/node_modules`);
        }
    }
}

// Checks that every typescript copy in the app is byte-identical to the packed package.
export function verifyInstall(appDir, pack) {
    const copies = findTypescriptCopies(appDir).map((rel) => {
        const dir = path.join(appDir, rel);
        const manifest = hashTree(dir, (rel, entry) => entry.name === "node_modules");
        const pkg = fs.existsSync(path.join(dir, "package.json")) ? readJson(path.join(dir, "package.json")) : {};
        const treeHash = manifestHash(manifest);
        return {
            path: rel,
            name: pkg.name,
            version: pkg.version,
            symlink: fs.lstatSync(dir).isSymbolicLink(),
            treeHash,
            matches: treeHash === pack.treeHash,
        };
    });
    const binTsc = path.join(appDir, "node_modules/.bin/tsc");
    return {
        ok: copies.length > 0 && copies.every((copy) => copy.matches),
        copies,
        binTsc: fs.existsSync(binTsc) ? path.relative(appDir, fs.realpathSync(binTsc)) : null,
    };
}

export function writeInstallReport(file, report) {
    writeJson(file, report);
}
