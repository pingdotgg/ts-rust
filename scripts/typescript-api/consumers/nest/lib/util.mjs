// File system, hashing and text helpers shared by run.mjs and compare.mjs.

import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

export const harnessDir = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
export const repoDir = path.resolve(harnessDir, "../../../..");
export const fixturesDir = path.resolve(harnessDir, "../fixtures");
export const workDir = path.join(repoDir, "target/typescript-api/consumers/nest");
export const cacheDir = path.join(workDir, "_cache");

export function readJson(file) {
    return JSON.parse(fs.readFileSync(file, "utf8"));
}

export function readJsonIfExists(file) {
    if (!fs.existsSync(file)) {
        return undefined;
    }
    return readJson(file);
}

export function writeJson(file, value) {
    fs.mkdirSync(path.dirname(file), { recursive: true });
    fs.writeFileSync(file, `${JSON.stringify(value, null, 2)}\n`);
}

export function sha256(data) {
    return createHash("sha256").update(data).digest("hex");
}

export function sha256File(file) {
    return sha256(fs.readFileSync(file));
}

// Relative paths (with "/") of every regular file under dir, sorted, symlinks followed for files.
export function listFiles(dir, skip = () => false) {
    const files = [];
    walk(dir, "");
    return files.sort();

    function walk(abs, rel) {
        for (const entry of fs.readdirSync(abs, { withFileTypes: true })) {
            const relPath = rel === "" ? entry.name : `${rel}/${entry.name}`;
            if (skip(relPath, entry)) {
                continue;
            }
            const absPath = path.join(abs, entry.name);
            if (entry.isDirectory()) {
                walk(absPath, relPath);
            } else if (entry.isFile() || (entry.isSymbolicLink() && fs.statSync(absPath).isFile())) {
                files.push(relPath);
            }
        }
    }
}

// { relPath: sha256 } of every file under dir.
export function hashTree(dir, skip) {
    const manifest = {};
    if (!fs.existsSync(dir)) {
        return manifest;
    }
    for (const rel of listFiles(dir, skip)) {
        manifest[rel] = sha256File(path.join(dir, rel));
    }
    return manifest;
}

export function manifestHash(manifest) {
    return sha256(Object.entries(manifest).map(([file, hash]) => `${hash}  ${file}\n`).join(""));
}

export function copyTree(from, to, skip = () => false) {
    fs.mkdirSync(to, { recursive: true });
    for (const entry of fs.readdirSync(from, { withFileTypes: true })) {
        if (skip(entry.name, entry)) {
            continue;
        }
        fs.cpSync(path.join(from, entry.name), path.join(to, entry.name), {
            recursive: true,
            dereference: false,
            verbatimSymlinks: true,
        });
    }
}

export function removePath(target) {
    fs.rmSync(target, { recursive: true, force: true, maxRetries: 3 });
}

export function stripAnsi(text) {
    // eslint-disable-next-line no-control-regex
    return text.replace(/\u001b\[[0-9;?]*[A-Za-z]/g, "");
}

// Output text with the parts that change from run to run (paths, times, pids, durations) replaced.
export function normalizeOutput(text, replacements = []) {
    let out = stripAnsi(text).replace(/\r\n/g, "\n");
    for (const [from, to] of replacements) {
        out = out.split(from).join(to);
    }
    return out
        .replace(/\[Nest\] \d+\s+- [^\n]*?\s{2,}(LOG|WARN|ERROR|DEBUG|VERBOSE)\b/g, "[Nest] PID - TIME $1")
        .replace(/\+\d+ms\b/g, "+Nms")
        .replace(/\(\d+(\.\d+)?m?s\)/g, "(TIME)")
        .replace(/\b\d{1,2}:\d{2}:\d{2}( [AP]M)?\b/g, "TIME")
        .replace(/\b\d+(\.\d+)?\s?(ms|s)\b/g, "N$2");
}

export function formatMs(ms) {
    if (ms === undefined || ms === null) {
        return "-";
    }
    if (ms < 1000) {
        return `${Math.round(ms)} ms`;
    }
    return `${(ms / 1000).toFixed(1)} s`;
}

export function isoNow() {
    return new Date().toISOString();
}
