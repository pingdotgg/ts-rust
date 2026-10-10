import assert from "node:assert/strict";
import { test } from "node:test";
import { loadModule } from "../node.js";
import { memoryFileSystem } from "../core.js";
import { createServiceChannel } from "../service-channel.js";
import { createHostServiceChannel } from "../classic-host.js";

import { fixture, argumentsForSettings, verifyClassicRuntime } from "./classic-runtime-sequence.mjs";

const diagnostics = service => service.request("textDocument/diagnostic", { textDocument: { uri: "file:///a.ts" } });

test("shared private classic runtime sequence", async () => {
    assert.ok(Object.values(verifyClassicRuntime(await loadModule())).every(Boolean));
});

test("private classic host bridge reads snapshots and synchronizes versions without promises", async () => {
    const project = fixture();
    const service = createHostServiceChannel(await loadModule(), project.host, argumentsForSettings);
    assert.equal(service.then, undefined);
    try {
        const first = diagnostics(service);
        assert.equal(first.then, undefined);
        assert.ok(!first.items.some(item => item.code === 2322));
        const reads = project.reads.get("/b.ts");
        const oldSnapshot = project.retained.find(snapshot => snapshot.getText(0, snapshot.getLength()) === "export const value = 1;");
        assert.ok(oldSnapshot);
        diagnostics(service);
        assert.equal(project.reads.get("/b.ts"), reads);
        project.text.set("/b.ts", 'export const value = "changed";');
        project.versions.set("/b.ts", "2");
        assert.ok(diagnostics(service).items.some(item => item.code === 2322));
        assert.equal(oldSnapshot.getText(0, oldSnapshot.getLength()), "export const value = 1;");
        project.text.set("/b.ts", "export const value = 1;");
        project.setProjectVersion("2");
        assert.ok(!diagnostics(service).items.some(item => item.code === 2322));
        assert.equal(service.dispose(), undefined);
        assert.equal(service.dispose(), undefined);
        assert.throws(() => diagnostics(service), /disposed/);
    } finally { service.dispose(); }
});

test("private classic host bridge refreshes options and roots and preserves snapshot failures", async () => {
    const project = fixture();
    project.text.set("/a.ts", "export const result: number = undefined;");
    const service = createHostServiceChannel(await loadModule(), project.host, argumentsForSettings);
    try {
        assert.ok(diagnostics(service).items.some(item => item.code === 2322));
        project.setSettings({ strict: false });
        assert.ok(!diagnostics(service).items.some(item => item.code === 2322));
        project.versions.set("/a.ts", "2");
        project.failSnapshot(true);
        assert.throws(() => diagnostics(service), /snapshot failure/);
        project.failSnapshot(false);
        assert.ok(Array.isArray(diagnostics(service).items));
        project.setRoots(["/b.ts"]);
        assert.throws(() => diagnostics(service), /document is not part of the program/);
        project.setRoots(["/a.ts"]);
        assert.ok(Array.isArray(diagnostics(service).items));
        project.text.set("/a.ts", 'import { value } from "./b"; export const result = value;');
        project.versions.set("/a.ts", "3");
        project.text.delete("/b.ts");
        project.setProjectVersion("3");
        assert.ok(diagnostics(service).items.some(item => item.code === 2307));
        project.text.set("/b.ts", "export const value = 1;");
        project.setProjectVersion("4");
        assert.ok(!diagnostics(service).items.some(item => item.code === 2307));
        const unicode = 'const emoji = "🌊"; export const result: number = "wrong";';
        project.text.set("/a.ts", unicode);
        project.versions.set("/a.ts", "4");
        const error = diagnostics(service).items.find(item => item.code === 2322);
        assert.equal(error.range.start.character, unicode.indexOf("result"));
    } finally { service.dispose(); }
});

test("synchronous channel guards reentry and validates configuration before invalidation", async () => {
    const memory = memoryFileSystem({ "/a.ts": "export const value = 1;" });
    let channel;
    let reentered = false;
    const fs = { ...memory, readFile(path) {
        if (channel && path === "/a.ts") {
            assert.throws(() => channel.call({ action: "invalidate" }), /reenter/);
            assert.throws(() => channel.dispose(), /during a host callback/);
            reentered = true;
        }
        return memory.readFile(path);
    } };
    channel = createServiceChannel(await loadModule(), { fs, args: ["--noLib", "/a.ts"] });
    const request = { action: "request", method: "textDocument/diagnostic", params: { textDocument: { uri: "file:///a.ts" } } };
    try {
        assert.ok(Array.isArray(channel.call(request).items));
        assert.ok(reentered);
        assert.throws(() => channel.call({ action: "configure", args: [1] }), { code: -32602 });
        assert.ok(Array.isArray(channel.call(request).items));
        assert.equal(channel.call({ action: "configure", args: ["--noLib", "/a.ts"] }), null);
        assert.ok(Array.isArray(channel.call(request).items));
    } finally { assert.equal(channel.dispose(), undefined); }
});

test("synchronous host channels isolate snapshots and an uncaught callback failure", async () => {
    const module = await loadModule();
    const first = fixture();
    const second = fixture();
    second.text.set("/b.ts", 'export const value = "other";');
    const firstService = createHostServiceChannel(module, first.host, argumentsForSettings);
    const secondService = createHostServiceChannel(module, second.host, argumentsForSettings);
    try {
        assert.ok(!diagnostics(firstService).items.some(item => item.code === 2322));
        assert.ok(diagnostics(secondService).items.some(item => item.code === 2322));
        const memory = memoryFileSystem({ "/a.ts": "export const value = 1;" });
        const failure = new Error("host read failure");
        const broken = createServiceChannel(module, { args: ["--noLib", "/a.ts"], fs: {
            ...memory, readFile() { throw failure; },
        } });
        const request = { action: "request", method: "textDocument/diagnostic", params: { textDocument: { uri: "file:///a.ts" } } };
        assert.throws(() => broken.call(request), error => error === failure);
        assert.throws(() => broken.call(request), error => error === failure);
        assert.equal(broken.dispose(), undefined);
        assert.ok(Array.isArray(diagnostics(firstService).items));
        assert.ok(Array.isArray(diagnostics(secondService).items));
    } finally {
        firstService.dispose();
        secondService.dispose();
    }
});
