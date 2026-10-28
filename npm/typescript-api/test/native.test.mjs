// The in-process tsc-rs channel (src/native/channel.js over crates/ts_napi) and runTsc.
// Run: cd npm/typescript-api && node --test test/native.test.mjs (after scripts/build-native.mjs).
// TSRS_API_CHANNEL=process runs the channel tests over a `tsgo --api` child process instead.

import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import { createRequire } from "node:module";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

const require = createRequire(import.meta.url);
const packageDir = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const { createChannel } = require("../src/native/channel.js");

const fsCallbacks = ["readFile", "fileExists", "directoryExists", "getAccessibleEntries", "realpath", "stat"];
const useOS = JSON.stringify({ kind: "useOS" });
const missing = JSON.stringify({ kind: "missing" });
const value = result => JSON.stringify({ kind: "value", value: result });

// A virtual project under `root`; every other path goes to the OS (the bundled libs among them).
function virtualFileSystem(root, files) {
    const all = new Map(Object.entries(files).map(([name, text]) => [`${root}/${name}`, text]));
    const inside = file => file === root || file.startsWith(`${root}/`);
    const reads = [];
    const handlers = {
        readFile: file => all.has(file) ? value(all.get(file)) : missing,
        fileExists: file => value(all.has(file)),
        directoryExists: file => value(file === root || [...all.keys()].some(name => name.startsWith(`${file}/`))),
        getAccessibleEntries: dir => {
            const names = [...all.keys()].filter(name => name.startsWith(`${dir}/`)).map(name => name.slice(dir.length + 1));
            return value({
                files: names.filter(name => !name.includes("/")),
                directories: [...new Set(names.filter(name => name.includes("/")).map(name => name.split("/")[0]))],
            });
        },
        realpath: () => JSON.stringify({ kind: "identity" }),
        stat: file => all.has(file) ? value({ mode: 0o100644, size: all.get(file).length, mtime: new Date(0) }) : missing,
    };
    return {
        reads,
        handle(name, payload) {
            const file = JSON.parse(payload);
            if (name === "readFile") reads.push(file);
            return inside(file) ? handlers[name](file) : useOS;
        },
    };
}

function openProject(root, files, { onRead } = {}) {
    const vfs = virtualFileSystem(root, files);
    const channel = createChannel([
        "--api",
        "--cwd",
        root,
        "--useCaseSensitiveFileNames=true",
        `--callbacks=${fsCallbacks.join(",")}`,
    ]);
    for (const name of fsCallbacks) {
        channel.registerCallback(name, (_, payload) => {
            if (name === "readFile") onRead?.(JSON.parse(payload));
            return vfs.handle(name, payload);
        });
    }
    return { channel, vfs };
}

function request(channel, method, params) {
    const text = channel.requestSync(method, JSON.stringify(params));
    return text === "" ? undefined : JSON.parse(text);
}

function semanticDiagnostics(channel, configFile) {
    request(channel, "initialize", null);
    const snapshot = request(channel, "createSnapshot", { openProjects: [configFile] });
    const project = snapshot.projects[0].id;
    return request(channel, "getSemanticDiagnostics", { snapshot: snapshot.snapshot, project });
}

const project = {
    "tsconfig.json": JSON.stringify({ compilerOptions: { strict: true, noEmit: true }, files: ["a.ts"] }),
    "a.ts": "export const x: number = 'no';\n",
};

test("echo returns the payload as text and as bytes", () => {
    const channel = createChannel(["--api", "--cwd", os.tmpdir()]);
    try {
        assert.equal(channel.requestSync("echo", JSON.stringify("héllo 🌍")), JSON.stringify("héllo 🌍"));
        const small = channel.requestBinarySync("echo", new Uint8Array([1, 2, 3]));
        assert.deepEqual([...small], [1, 2, 3]);
        const big = new Uint8Array(3 << 20).map((_, i) => i % 251);
        const echoed = channel.requestBinarySync("echo", big);
        assert.ok(Buffer.isBuffer(echoed));
        assert.equal(echoed.byteOffset, 0);
        assert.equal(echoed.buffer.byteLength, big.length);
        assert.ok(Buffer.from(big).equals(echoed));
        assert.equal(channel.requestSync("echo", ""), "");
    }
    finally {
        channel.close();
    }
});

test("a request reads the project through filesystem callbacks", () => {
    const { channel, vfs } = openProject("/virtual/one", project);
    try {
        const diagnostics = semanticDiagnostics(channel, "/virtual/one/tsconfig.json");
        assert.deepEqual(diagnostics.map(d => [d.fileName, d.code, d.text]), [
            ["/virtual/one/a.ts", 2322, "Type 'string' is not assignable to type 'number'."],
        ]);
        assert.ok(vfs.reads.includes("/virtual/one/a.ts"));
        assert.ok(vfs.reads.some(file => file.startsWith("bundled:///libs/")), "the bundled libs are asked for too");
    }
    finally {
        channel.close();
    }
});

test("a callback can make a nested request", () => {
    let nested;
    const { channel } = openProject("/virtual/nested", project, {
        onRead: file => {
            if (file === "/virtual/nested/a.ts") {
                nested = channel.requestSync("echo", JSON.stringify({ inside: file }));
            }
        },
    });
    try {
        assert.equal(semanticDiagnostics(channel, "/virtual/nested/tsconfig.json").length, 1);
        assert.deepEqual(JSON.parse(nested), { inside: "/virtual/nested/a.ts" });
        assert.equal(channel.requestSync("echo", "1"), "1");
    }
    finally {
        channel.close();
    }
});

test("a throwing filesystem callback fails its request and the session goes on", () => {
    const failure = new Error("disk on fire");
    let fail = true;
    const { channel } = openProject("/virtual/throws", project, {
        onRead: file => {
            if (fail && file === "/virtual/throws/a.ts") throw failure;
        },
    });
    try {
        request(channel, "initialize", null);
        assert.throws(() => request(channel, "createSnapshot", { openProjects: ["/virtual/throws/tsconfig.json"] }), error => error === failure);
        fail = false;
        assert.equal(channel.requestSync("echo", "2"), "2");
        const snapshot = request(channel, "createSnapshot", { openProjects: ["/virtual/throws/tsconfig.json"] });
        assert.equal(snapshot.projects.length, 1);
    }
    finally {
        channel.close();
    }
});

test("a serverFS.error answer or sentinel fails the request instead of ending the process", () => {
    const { channel } = openProject("/virtual/sentinel", project);
    channel.registerCallback("stat", () => JSON.stringify({ kind: "error" }));
    channel.registerCallback("readFile", (_, payload) => JSON.parse(payload).startsWith("/virtual/") ? JSON.stringify({ kind: "error" }) : useOS);
    try {
        request(channel, "initialize", null);
        assert.throws(() => request(channel, "createSnapshot", { openProjects: ["/virtual/sentinel/tsconfig.json"] }), /serverFS\.error: readFile/);
        assert.equal(channel.requestSync("echo", "3"), "3");
    }
    finally {
        channel.close();
    }
    const configured = createChannel(["--api", "--cwd", "/virtual/configured", "--callbacks=readFile:error"]);
    try {
        request(configured, "initialize", null);
        assert.throws(() => request(configured, "createSnapshot", { openProjects: ["/virtual/configured/tsconfig.json"] }), /configured with serverFS\.error: readFile/);
    }
    finally {
        configured.close();
    }
});

// A project whose files tsc-rs lists with getAccessibleEntries.
const globbedProject = {
    "tsconfig.json": JSON.stringify({ compilerOptions: { strict: true, noEmit: true }, include: ["*.ts"] }),
    "a.ts": project["a.ts"],
};

// tsc-rs panics on any answer that it does not take, and inside program loading a panic ends the
// process: each of these must fail the request instead.
const invalidAnswers = {
    readFile: [
        JSON.stringify({ kind: "error" }, null, 1),
        '{"kind":"error","value":null}',
        '{"kind":"bogus"}',
        '{"kind":"value","value":5}',
        '{"kind":"value"}',
        "not json",
        "null",
    ],
    fileExists: ['{"kind":"value","value":1}', '{"kind":"missing"}'],
    directoryExists: ['{"kind":"value","value":"yes"}'],
    getAccessibleEntries: ['{"kind":"value","value":null}', '{"kind":"value","value":{"files":"a.ts"}}', '{"kind":"value","value":[]}'],
    realpath: ['{"kind":"value","value":""}', '{"kind":"value","value":7}'],
};

test("a filesystem answer that tsc-rs does not take fails the request instead of ending the process", () => {
    const root = "/virtual/invalid";
    for (const [name, answers] of Object.entries(invalidAnswers)) {
        for (const answer of answers) {
            const { channel, vfs } = openProject(root, globbedProject);
            let given = false;
            channel.registerCallback(name, (_, payload) => {
                const file = JSON.parse(payload);
                if (given || (file !== root && !file.startsWith(`${root}/`))) return vfs.handle(name, payload);
                given = true;
                return answer;
            });
            try {
                request(channel, "initialize", null);
                const result = (() => {
                    try {
                        return semanticDiagnostics(channel, `${root}/tsconfig.json`);
                    }
                    catch (error) {
                        return error;
                    }
                })();
                assert.ok(given, `${name} was asked`);
                assert.ok(result instanceof Error, `${name} answer ${answer} fails the request`);
                assert.match(result.message, new RegExp(`filesystem callback (returned serverFS\\.error: |\`)${name}`));
                assert.equal(channel.requestSync("echo", "12"), "12");
            }
            finally {
                channel.close();
            }
        }
    }
});

// No request of a program asks stat; this calls the channel's callback entry (what the native side
// calls) directly.
test("a stat answer that tsc-rs does not take becomes a missing answer", () => {
    const channel = createChannel(["--api", "--cwd", os.tmpdir()]);
    const stat = value => JSON.stringify({ kind: "value", value });
    const invalid = [
        stat({ mode: -1, size: 0, mtime: "1970-01-01T00:00:00Z" }),
        stat({ mode: 0o100644, size: 1.5, mtime: "1970-01-01T00:00:00Z" }),
        stat({ mode: 0o100644, size: 0, mtime: "yesterday" }),
        stat({ mode: 0o100644, size: 0, mtime: "2023-02-29T00:00:00Z" }),
        stat({ mode: 0o100644, size: 0, mtime: "2024-01-02T03:04:05+24:00" }),
        stat({ mode: 0o100644, size: 0 }),
        stat(null),
        JSON.stringify({ kind: "identity" }),
    ];
    try {
        for (const answer of invalid) {
            channel.registerCallback("stat", () => answer);
            channel.callbackFailure = undefined;
            assert.equal(channel.onCall("stat", JSON.stringify("/a.ts")), JSON.stringify({ kind: "missing" }), answer);
            assert.match(channel.callbackFailure.error.message, /filesystem callback `stat` gave an invalid answer/);
        }
        const valid = stat({ mode: 0o100644, size: 3, mtime: "2024-01-02T05:04:05.123456789+02:00" });
        channel.registerCallback("stat", () => valid);
        channel.callbackFailure = undefined;
        assert.equal(channel.onCall("stat", JSON.stringify("/a.ts")), valid);
        assert.equal(channel.callbackFailure, undefined);
        channel.registerCallback("stat", () => JSON.stringify({ kind: "fakeStat", value: 1 }));
        assert.equal(channel.onCall("stat", JSON.stringify("/a.ts")), JSON.stringify({ kind: "fakeStat" }));
    }
    finally {
        channel.close();
    }
});

test("a valid filesystem answer in another form reaches tsc-rs in the form it reads", () => {
    const root = "/virtual/forms";
    const { channel, vfs } = openProject(root, globbedProject);
    const asked = new Set();
    channel.registerCallback("readFile", (_, payload) => {
        if (JSON.parse(payload) !== `${root}/a.ts`) return vfs.handle("readFile", payload);
        asked.add("readFile");
        return `{ "value": "export const x: number = 1;", "kind": "value", "extra": [1], "value": ${JSON.stringify(project["a.ts"])} }`;
    });
    channel.registerCallback("getAccessibleEntries", (_, payload) => {
        if (JSON.parse(payload) !== root) return vfs.handle("getAccessibleEntries", payload);
        asked.add("getAccessibleEntries");
        return JSON.stringify({ kind: "value", value: { files: ["tsconfig.json", "a.ts"], symlinks: null } }, null, 2);
    });
    try {
        assert.deepEqual(semanticDiagnostics(channel, `${root}/tsconfig.json`).map(d => d.code), [2322]);
        assert.deepEqual([...asked].sort(), ["getAccessibleEntries", "readFile"]);
    }
    finally {
        channel.close();
    }
});

test("an invalid writeFile answer fails the emit request", () => {
    const root = "/virtual/write";
    const writes = [];
    const vfs = virtualFileSystem(root, {
        "tsconfig.json": JSON.stringify({ compilerOptions: { outDir: "out" }, files: ["a.ts"] }),
        "a.ts": "export const x = 1;\n",
    });
    const channel = createChannel(["--api", "--cwd", root, "--useCaseSensitiveFileNames=true", `--callbacks=${[...fsCallbacks, "writeFile"].join(",")}`]);
    for (const name of fsCallbacks) channel.registerCallback(name, (_, payload) => vfs.handle(name, payload));
    let answer = '{"kind":"bogus"}';
    channel.registerCallback("writeFile", (_, payload) => {
        writes.push(JSON.parse(payload).path);
        return answer;
    });
    try {
        request(channel, "initialize", null);
        const snapshot = request(channel, "createSnapshot", { openProjects: [`${root}/tsconfig.json`] });
        const params = { snapshot: snapshot.snapshot, project: snapshot.projects[0].id };
        assert.throws(() => request(channel, "emit", params), /filesystem callback `writeFile` gave an invalid answer/);
        answer = JSON.stringify({ kind: "noop" });
        request(channel, "emit", params);
        assert.deepEqual(writes, [`${root}/out/a.js`, `${root}/out/a.js`]);
    }
    finally {
        channel.close();
    }
});

test("lone surrogates in requests and callback answers reach tsc-rs as U+FFFD", () => {
    const text = "const s = \"broken \uD83D emoji \uDE00\";\nexport const x: number = 'no';\n";
    const { channel } = openProject("/virtual/surrogate", { ...project, "a.ts": text });
    try {
        const diagnostics = semanticDiagnostics(channel, "/virtual/surrogate/tsconfig.json");
        assert.deepEqual(diagnostics.map(d => [d.code, d.pos]), [[2322, text.indexOf("x:")]], "positions stay UTF-16 offsets");
        const params = { input: "export const s = \"broken \uD83D emoji\";\n", options: { compilerOptions: {}, fileName: "module.ts" } };
        const fromText = request(channel, "transpileModule", params);
        assert.equal(fromText.outputText, "export const s = \"broken \uFFFD emoji\";\n");
        const bytes = channel.requestBinarySync("transpileModule", new TextEncoder().encode(JSON.stringify(params)));
        assert.equal(JSON.parse(Buffer.from(bytes).toString("utf8")).outputText, fromText.outputText);
        // An escaped backslash before `u` is not an escape, and an escaped pair stays.
        assert.equal(channel.requestSync("echo", '"\\\\ud83d \\ud83d\\ude00 \\uDC00"'), '"\\\\ud83d \\ud83d\\ude00 \\ufffd"');
    }
    finally {
        channel.close();
    }
});

// The channel's own callback guard needs JS to run. When JS execution terminates during a callback,
// the native side still answers a read callback harmlessly: a CALL_ERROR is a Go panic in tsc-rs,
// which inside program loading ends the whole process.
const slowReadScript = (channelPath, onRead) => `
    const fs = require("node:fs");
    const os = require("node:os");
    const path = require("node:path");
    const { createChannel } = require(${JSON.stringify(channelPath)});
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "tsrs-terminate-"));
    const file = path.join(dir, "a.ts");
    fs.writeFileSync(file, "export const a = 1;\\n");
    const channel = createChannel(["--api", "--cwd", dir, "--callbacks=readFile,fileExists"]);
    for (const name of ["readFile", "fileExists"]) {
        channel.registerCallback(name, (_, payload) => {
            if (JSON.parse(payload) === file) {
                ${onRead}
                const end = Date.now() + 2000;
                while (Date.now() < end);
            }
            return JSON.stringify({ kind: "useOS" });
        });
    }
    channel.requestSync("initialize", "null");
    const createSnapshot = () => channel.requestSync("createSnapshot", JSON.stringify({ createPrograms: [{ rootFiles: [file] }] }));`;
const channelPath = path.join(packageDir, "src", "native", "channel.js");

test("a vm timeout inside a read callback fails the request and the process goes on", { skip: process.env.TSRS_API_CHANNEL === "process" }, () => {
    const script = `${slowReadScript(channelPath, "")}
        const vm = require("node:vm");
        try {
            vm.runInNewContext("createSnapshot()", { createSnapshot }, { timeout: 200 });
            console.log("no timeout");
        }
        catch (error) {
            console.log("caught: " + error.message);
        }
        console.log("echo " + channel.requestSync("echo", JSON.stringify("after")));
        channel.close();`;
    const child = spawnSync(process.execPath, ["-e", script], { encoding: "utf8", timeout: 60_000 });
    assert.equal(child.status, 0, child.stdout + child.stderr);
    assert.match(child.stdout, /caught: Script execution timed out after 200ms/);
    assert.match(child.stdout, /echo "after"/);
});

for (const [how, onRead] of [["terminating", 'require("node:worker_threads").parentPort.postMessage("in-callback");'], ["process.exit in", "process.exit(3);"]]) {
    test(`${how} a worker inside a read callback ends only the worker`, { skip: process.env.TSRS_API_CHANNEL === "process" }, () => {
        const workerScript = `${slowReadScript(channelPath, onRead)}\ncreateSnapshot();`;
        const script = `
            const { Worker } = require("node:worker_threads");
            const worker = new Worker(${JSON.stringify(workerScript)}, { eval: true });
            worker.on("message", () => setTimeout(() => worker.terminate(), 50));
            worker.on("exit", code => {
                console.log("worker exit " + code);
                setTimeout(() => console.log("main alive"), 200);
            });
            process.on("exit", code => console.log("main exit " + code));`;
        const child = spawnSync(process.execPath, ["-e", script], { encoding: "utf8", timeout: 60_000 });
        assert.equal(child.status, 0, child.stdout + child.stderr);
        assert.match(child.stdout, how === "terminating" ? /worker exit 1/ : /worker exit 3/);
        assert.match(child.stdout, /main alive\nmain exit 0/);
    });
}

test("close inside a callback ends the session after the request", () => {
    let closeNow = true;
    const { channel } = openProject("/virtual/close", project, {
        onRead: () => {
            if (closeNow) {
                closeNow = false;
                channel.close();
            }
        },
    });
    request(channel, "initialize", null);
    const snapshot = request(channel, "createSnapshot", { openProjects: ["/virtual/close/tsconfig.json"] });
    assert.equal(snapshot.projects.length, 1);
    assert.throws(() => channel.requestSync("echo", "4"), /channel is closed/);
});

test("two channels run in one process", () => {
    const first = openProject("/virtual/first", project);
    const second = openProject("/virtual/second", {
        "tsconfig.json": JSON.stringify({ compilerOptions: { strict: true, noEmit: true }, files: ["b.ts"] }),
        "b.ts": "export const y: string = 1;\nexport const z: boolean = 2;\n",
    });
    try {
        request(first.channel, "initialize", null);
        request(second.channel, "initialize", null);
        const a = request(first.channel, "createSnapshot", { openProjects: ["/virtual/first/tsconfig.json"] });
        const b = request(second.channel, "createSnapshot", { openProjects: ["/virtual/second/tsconfig.json"] });
        const bDiagnostics = request(second.channel, "getSemanticDiagnostics", { snapshot: b.snapshot, project: b.projects[0].id });
        const aDiagnostics = request(first.channel, "getSemanticDiagnostics", { snapshot: a.snapshot, project: a.projects[0].id });
        assert.deepEqual(aDiagnostics.map(d => d.code), [2322]);
        assert.deepEqual(bDiagnostics.map(d => d.code), [2322, 2322]);
        assert.equal(bDiagnostics[0].fileName, "/virtual/second/b.ts");
    }
    finally {
        first.channel.close();
        second.channel.close();
    }
});

// All in-process sessions run on one thread, which waits in the callback; the other session runs
// inside that wait.
test("a callback of one session can make requests to another session", () => {
    const other = openProject("/virtual/other", {
        "tsconfig.json": JSON.stringify({ compilerOptions: { strict: true, noEmit: true }, files: ["b.ts"] }),
        "b.ts": "export const y: string = 1;\n",
    });
    const inner = {};
    let opened;
    const { channel } = openProject("/virtual/cross", project, {
        onRead: file => {
            if (file !== "/virtual/cross/a.ts") return;
            inner.echo = other.channel.requestSync("echo", "5");
            inner.diagnostics = semanticDiagnostics(other.channel, "/virtual/other/tsconfig.json").map(d => [d.fileName, d.code]);
            // A session that starts inside the callback.
            opened = createChannel(["--api", "--cwd", os.tmpdir()]);
            inner.opened = opened.requestSync("echo", "6");
        },
    });
    try {
        assert.deepEqual(semanticDiagnostics(channel, "/virtual/cross/tsconfig.json").map(d => [d.fileName, d.code]), [["/virtual/cross/a.ts", 2322]]);
        assert.deepEqual(inner, { echo: "5", diagnostics: [["/virtual/other/b.ts", 2322]], opened: "6" });
        assert.equal(other.channel.requestSync("echo", "7"), "7");
        assert.equal(opened.requestSync("echo", "8"), "8");
    }
    finally {
        channel.close();
        other.channel.close();
        opened?.close();
    }
});

test("a session that waits for another session's callback refuses requests from it", { skip: process.env.TSRS_API_CHANNEL === "process" }, () => {
    let outer;
    const inner = openProject("/virtual/inner", project, {
        onRead: file => {
            if (file === "/virtual/inner/a.ts") outer.channel.requestSync("echo", "9");
        },
    });
    outer = openProject("/virtual/outer", project, {
        onRead: file => {
            if (file === "/virtual/outer/a.ts") semanticDiagnostics(inner.channel, "/virtual/inner/tsconfig.json");
        },
    });
    try {
        request(outer.channel, "initialize", null);
        assert.throws(
            () => request(outer.channel, "createSnapshot", { openProjects: ["/virtual/outer/tsconfig.json"] }),
            /cannot make a request to a session that waits for that callback/,
        );
        assert.equal(outer.channel.requestSync("echo", "10"), "10");
        assert.equal(inner.channel.requestSync("echo", "11"), "11");
    }
    finally {
        outer.channel.close();
        inner.channel.close();
    }
});

// ts_goport keeps about 5.5 MiB of each program version for the life of the process (tsgo --api
// grows as much per program); a session thread per channel kept 17 MiB per cycle.
test("100 open and close cycles keep memory bounded", async t => {
    const cycle = i => {
        const root = `/virtual/cycle${i}`;
        const { channel } = openProject(root, project);
        try {
            assert.equal(semanticDiagnostics(channel, `${root}/tsconfig.json`).length, 1);
        }
        finally {
            channel.close();
        }
    };
    for (let i = 0; i < 10; i++) cycle(i);
    await new Promise(resolve => setTimeout(resolve, 500));
    const warm = process.memoryUsage.rss();
    const start = performance.now();
    for (let i = 10; i < 110; i++) cycle(i);
    const elapsed = performance.now() - start;
    await new Promise(resolve => setTimeout(resolve, 1000));
    const after = process.memoryUsage.rss();
    const perCycle = (after - warm) / 100;
    t.diagnostic(`rss after 10 warm-up cycles ${(warm / 2 ** 20).toFixed(1)} MiB, after 100 more ${(after / 2 ** 20).toFixed(1)} MiB (${(perCycle / 1024).toFixed(1)} KiB per cycle), ${(elapsed / 100).toFixed(1)} ms per cycle`);
    assert.ok(perCycle < 10 * 2 ** 20, `each cycle keeps ${(perCycle / 1024).toFixed(0)} KiB`);
});

const tscBin = path.join(packageDir, "bin", "tsc");

function tinyProject(source) {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "tsrs-tsc-"));
    fs.writeFileSync(path.join(dir, "tsconfig.json"), JSON.stringify({ compilerOptions: { strict: true, outDir: "out" }, files: ["a.ts"] }));
    fs.writeFileSync(path.join(dir, "a.ts"), source);
    return dir;
}

test("runTsc compiles a tiny project in a child process", { skip: process.env.TSRS_API_CHANNEL === "process" }, () => {
    const good = tinyProject("export const x: number = 1;\n");
    const ok = spawnSync(process.execPath, [tscBin, "-p", good], { encoding: "utf8" });
    assert.equal(ok.status, 0, ok.stdout + ok.stderr);
    assert.equal(fs.readFileSync(path.join(good, "out", "a.js"), "utf8"), "export const x = 1;\n");

    const bad = tinyProject("export const x: number = 'no';\n");
    const failed = spawnSync(process.execPath, [tscBin, "-p", bad, "--pretty", "false"], { encoding: "utf8" });
    assert.equal(failed.status, 2);
    assert.match(failed.stdout, /a\.ts\(1,14\): error TS2322: Type 'string' is not assignable to type 'number'\./);

    const version = spawnSync(process.execPath, [tscBin, "--version"], { encoding: "utf8" });
    assert.equal(version.status, 0);
    assert.match(version.stdout, /^Version 7\./);
    fs.rmSync(good, { recursive: true });
    fs.rmSync(bad, { recursive: true });
});

test("runTsc and API sessions refuse to share a process", { skip: process.env.TSRS_API_CHANNEL === "process" }, () => {
    const script = `
        const { createChannel } = require(${JSON.stringify(path.join(packageDir, "src", "native", "channel.js"))});
        const { runTsc } = require(${JSON.stringify(path.join(packageDir, "src", "tsc.js"))});
        const order = process.argv[1];
        if (order === "api-first") {
            createChannel(["--api"]).close();
            try { runTsc(["--version"]); console.log("ran"); } catch (e) { console.log("refused: " + e.message); }
        } else {
            runTsc(["--version"]);
            try { createChannel(["--api"]); console.log("opened"); } catch (e) { console.log("refused: " + e.message); }
            try { runTsc(["--version"]); console.log("ran"); } catch (e) { console.log("refused: " + e.message); }
        }`;
    const apiFirst = spawnSync(process.execPath, ["-e", script, "api-first"], { encoding: "utf8" });
    assert.match(apiFirst.stdout, /refused: this process opened a tsc-rs API session/);
    const tscFirst = spawnSync(process.execPath, ["-e", script, "tsc-first"], { encoding: "utf8" });
    assert.match(tscFirst.stdout, /refused: this process ran runTsc/);
    assert.match(tscFirst.stdout, /refused: runTsc already ran in this process/);
});
