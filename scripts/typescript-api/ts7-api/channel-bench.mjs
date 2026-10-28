// Times the tsc-rs channel in this process (crates/ts_napi) against the same channel over a
// `tsgo --api` child process (TSRS_API_CHANNEL=process): request round trips, and opening a
// project (createSnapshot) with and without filesystem callbacks. Each measurement runs in a fresh
// Node process; the modes alternate.
//
// usage: node scripts/typescript-api/ts7-api/channel-bench.mjs [--project <tsconfig.json>] [--runs N]
//        node scripts/typescript-api/ts7-api/channel-bench.mjs help
// Default project: target/typescript-api/consumers/nest/nest-cli-unit/tsconfig.json, nest-cli 12.0.8
// with its node_modules, which the nest-cli-unit scenario of consumers/nest.sh installs. Prints a
// JSON summary (milliseconds; medians unless named).

import { spawnSync } from "node:child_process";
import fs from "node:fs";
import { createRequire } from "node:module";
import path from "node:path";
import { fileURLToPath } from "node:url";

const require = createRequire(import.meta.url);
const self = fileURLToPath(import.meta.url);
const repoDir = path.resolve(path.dirname(self), "../../..");
const channelModule = path.join(repoDir, "npm/typescript-api/src/native/channel.js");
const readCallbacks = ["readFile", "fileExists", "directoryExists", "getAccessibleEntries", "realpath", "stat"];
const useOS = JSON.stringify({ kind: "useOS" });

function median(values) {
    const sorted = [...values].sort((a, b) => a - b);
    return sorted[Math.floor(sorted.length / 2)];
}

function round(value) {
    return Math.round(value * 1000) / 1000;
}

// One measurement in this process: prints its result as JSON.
function measure(kind, variant, project) {
    const { createChannel } = require(channelModule);
    const callbacks = variant === "none" ? [] : readCallbacks;
    const start = performance.now();
    const args = ["--api", "--cwd", path.dirname(project), "--useCaseSensitiveFileNames=true"];
    if (callbacks.length) args.push(`--callbacks=${callbacks.join(",")}`);
    const channel = createChannel(args);
    let calls = 0;
    for (const name of callbacks) {
        channel.registerCallback(name, (_, payload) => {
            calls++;
            return variant === "useOS" ? useOS : answerFromNode(name, JSON.parse(payload));
        });
    }
    channel.requestSync("initialize", "null");
    const opened = performance.now();
    const result = { kind, variant, openMs: round(opened - start) };
    if (variant === "echo") {
        const small = [];
        for (let i = 0; i < 21000; i++) {
            const t = performance.now();
            channel.requestSync("echo", "\"x\"");
            if (i >= 1000) small.push(performance.now() - t);
        }
        const payload = new Uint8Array(1 << 20).fill(7);
        const big = [];
        for (let i = 0; i < 220; i++) {
            const t = performance.now();
            channel.requestBinarySync("echo", payload);
            if (i >= 20) big.push(performance.now() - t);
        }
        result.echoSmallUs = round(median(small) * 1000);
        result.echoSmallMeanUs = round(small.reduce((a, b) => a + b, 0) / small.length * 1000);
        result.echo1MiBUs = round(median(big) * 1000);
    }
    else {
        const t = performance.now();
        const snapshot = JSON.parse(channel.requestSync("createSnapshot", JSON.stringify({ openProjects: [project] })));
        result.createSnapshotMs = round(performance.now() - t);
        const projectId = snapshot.projects[0].id;
        const t2 = performance.now();
        const diagnostics = JSON.parse(channel.requestSync("getSemanticDiagnostics", JSON.stringify({ snapshot: snapshot.snapshot, project: projectId })) || "[]");
        result.semanticDiagnosticsMs = round(performance.now() - t2);
        result.files = snapshot.projects[0].parsedCommandLine.fileNames.length;
        result.diagnostics = diagnostics.length;
        result.callbacks = calls;
    }
    channel.close();
    result.rssMiB = Math.round(process.memoryUsage.rss() / 2 ** 20);
    console.log(JSON.stringify(result));
}

function answerFromNode(name, file) {
    const value = v => JSON.stringify({ kind: "value", value: v });
    if (file.startsWith("bundled:///")) return useOS;
    try {
        switch (name) {
            case "readFile":
                return value(fs.readFileSync(file, "utf8"));
            case "fileExists":
                return value(fs.statSync(file, { throwIfNoEntry: false })?.isFile() ?? false);
            case "directoryExists":
                return value(fs.statSync(file, { throwIfNoEntry: false })?.isDirectory() ?? false);
            case "getAccessibleEntries": {
                const entries = fs.readdirSync(file, { withFileTypes: true });
                return value({
                    files: entries.filter(e => e.isFile()).map(e => e.name),
                    directories: entries.filter(e => e.isDirectory()).map(e => e.name),
                });
            }
            case "realpath":
                return value(fs.realpathSync(file));
            case "stat": {
                const stat = fs.statSync(file);
                return value({ mode: stat.mode, size: stat.size, mtime: stat.mtime });
            }
        }
    }
    catch {
        return name === "fileExists" || name === "directoryExists" ? value(false) : JSON.stringify({ kind: "missing" });
    }
    return useOS;
}

function run(kind, variant, project) {
    const env = { ...process.env };
    if (kind === "process") env.TSRS_API_CHANNEL = "process";
    else delete env.TSRS_API_CHANNEL;
    const child = spawnSync(process.execPath, [self, "--one", kind, variant, project], { env, encoding: "utf8", maxBuffer: 1 << 26 });
    if (child.status !== 0) throw new Error(`${kind} ${variant} failed: ${child.stderr}`);
    return JSON.parse(child.stdout.trim().split("\n").at(-1));
}

const argv = process.argv.slice(2);
if (argv[0] === "help") {
    const lines = fs.readFileSync(self, "utf8").split("\n");
    console.log(lines.slice(0, lines.findIndex(line => !line.startsWith("//"))).map(line => line.replace(/^\/\/ ?/, "")).join("\n"));
}
else if (argv[0] === "--one") {
    measure(argv[1], argv[2], argv[3]);
}
else {
    let project = path.join(repoDir, "target/typescript-api/consumers/nest/nest-cli-unit/tsconfig.json");
    let runs = 3;
    for (let i = 0; i < argv.length; i++) {
        if (argv[i] === "--project") project = path.resolve(argv[++i]);
        else if (argv[i] === "--runs") runs = Number(argv[++i]);
        else throw new Error(`unknown argument ${argv[i]} (see help)`);
    }
    if (!fs.existsSync(project)) {
        throw new Error(`no project at ${project}: pass --project, or run scripts/typescript-api/consumers/nest.sh (see help)`);
    }
    const results = {};
    for (let i = 0; i < runs; i++) {
        for (const variant of ["echo", "none", "useOS", "node"]) {
            for (const kind of ["native", "process"]) {
                (results[`${kind} ${variant}`] ??= []).push(run(kind, variant, project));
            }
        }
    }
    const summary = {};
    for (const [key, list] of Object.entries(results)) {
        const fields = {};
        for (const field of Object.keys(list[0])) {
            if (typeof list[0][field] === "number") fields[field] = median(list.map(r => r[field]));
        }
        summary[key] = fields;
    }
    console.log(JSON.stringify({ project, runs, summary }, null, 1));
}
