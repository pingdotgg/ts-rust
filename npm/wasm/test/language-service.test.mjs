import assert from "node:assert/strict";
import { test } from "node:test";
import { cp, mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { createLanguageService, loadModule, tsc } from "../node.js";
import { createLanguageService as createCoreLanguageService } from "../core.js";
import { verifyLanguageService } from "./language-service-sequence.mjs";
import { countFileReads } from "./wasm-file-reads.mjs";
import { fileEntries } from "../file-map.js";

test("persistent language services update, navigate, isolate and dispose", async () => {
    const result = await verifyLanguageService(createLanguageService);
    assert.equal(Object.values(result).every(Boolean), true);
});

test("Node service checks a deep expression without JSPI", async () => {
    const text = `export const deep = ${"1 + ".repeat(5_000)}1;`;
    const service = await createLanguageService({ args: ["--strict", "/deep.ts"], files: { "/deep.ts": text } });
    try {
        const result = await service.request("textDocument/diagnostic", { textDocument: { uri: "file:///deep.ts" } });
        assert.ok(Array.isArray(result.items));
        assert.ok(!result.items.some(item => item.severity === 1));
    } finally { await service.dispose(); }
});

function readonlyMap(entries) {
    const map = new Map(entries);
    return {
        get size() { return map.size; },
        get: key => map.get(key), has: key => map.has(key),
        keys: () => map.keys(), values: () => map.values(), entries: () => map.entries(),
        forEach: callback => map.forEach((value, key) => callback(value, key)),
        [Symbol.iterator]: () => map[Symbol.iterator](),
    };
}

test("structural ReadonlyMap files create and update snapshots", async () => {
    function* reusedEntries() {
        const entry = ["/a.ts", "first"];
        yield entry;
        entry[0] = "/b.ts";
        entry[1] = "second";
        yield entry;
    }
    assert.deepEqual(fileEntries(reusedEntries()), [["/a.ts", "first"], ["/b.ts", "second"]]);
    const service = await createLanguageService({ args: ["/a.ts"], files: readonlyMap([["/a.ts", "export const value = 1;"]]) });
    const params = { textDocument: { uri: "file:///a.ts" }, position: { line: 0, character: 14 } };
    try {
        assert.match(JSON.stringify(await service.request("textDocument/hover", params)), /1/);
        await service.updateFiles(readonlyMap([["/a.ts", 'export const value = "changed";']]));
        assert.match(JSON.stringify(await service.request("textDocument/hover", params)), /changed/);
        const update = service.updateFiles({ "/a.ts": "export const value = 42;" });
        const hover = service.request("textDocument/hover", params);
        const disposal = service.dispose();
        await update;
        assert.match(JSON.stringify(await hover), /42/);
        await disposal;
        await assert.rejects(service.request("textDocument/hover", params), /disposed/);
    } finally { await service.dispose(); }
});

test("setup errors retain command and configuration diagnostics", async () => {
    const cases = [
        { args: ["--unknownFlag", "/a.ts"], files: { "/a.ts": "export const n = 1;" } },
        { args: ["-p", "/p"], files: { "/p/tsconfig.json": '{"compilerOptions":{"unknownOption":true},"files":["a.ts"]}', "/p/a.ts": "export const n = 1;" } },
        { args: ["-p", "/p"], files: { "/p/tsconfig.json": '{"compilerOptions":{', "/p/a.ts": "export const n = 1;" } },
        { args: ["-p", "/missing"], files: {} },
    ];
    for (const options of cases) {
        const service = await createLanguageService(options);
        try {
            await assert.rejects(service.request("textDocument/diagnostic", { textDocument: { uri: "file:///a.ts" } }), error => {
                assert.equal(error.code, -32602);
                assert.ok(error.data.diagnostics.length > 0);
                assert.match(error.message, /TS\d+:/);
                assert.ok(error.data.diagnostics.every(diagnostic => diagnostic.text && diagnostic.code));
                return true;
            });
        } finally { await service.dispose(); }
    }
});

test("method errors and invalid parameters keep the service usable", async () => {
    const service = await createLanguageService({ args: ["/a.ts"], files: { "/a.ts": "export const value = 1;" } });
    const params = { textDocument: { uri: "file:///a.ts" }, position: { line: 0, character: 14 } };
    try {
        await assert.rejects(service.request("unknown", {}), { code: -32601 });
        await assert.rejects(service.request("textDocument/hover", { ...params, position: { line: -1, character: 0 } }), { code: -32602 });
        const end = { ...params, position: { line: 0, character: 23 } };
        assert.deepEqual(await service.request("textDocument/hover", { ...end, position: { line: 0, character: 1_000 } }), await service.request("textDocument/hover", end));
        assert.ok(await service.request("textDocument/hover", params));
    } finally { await service.dispose(); }
});

test("Node honors explicit wasm without the default asset", async () => {
    const root = await mkdtemp(join(tmpdir(), "ts-rust-service-"));
    const packageRoot = fileURLToPath(new URL("..", import.meta.url));
    try {
        await cp(packageRoot, root, { recursive: true, filter: source => !source.endsWith("ts_rust.wasm") });
        const { createLanguageService: create } = await import(pathToFileURL(join(root, "node.js")));
        const bytes = await readFile(new URL("../ts_rust.wasm", import.meta.url));
        const service = await create({ wasm: bytes, args: ["/a.ts"], files: { "/a.ts": "export const value = 1;" } });
        try { assert.ok(await service.request("textDocument/hover", { textDocument: { uri: "file:///a.ts" }, position: { line: 0, character: 14 } })); }
        finally { await service.dispose(); }
    } finally { await rm(root, { recursive: true, force: true }); }
});

function rawService(module) {
    let instance;
    const imports = {};
    for (const entry of WebAssembly.Module.imports(module)) {
        imports[entry.module] ??= {};
        imports[entry.module][entry.name] = () => { throw new Error(`unexpected import during initialization: ${entry.module}.${entry.name}`); };
    }
    instance = new WebAssembly.Instance(module, imports);
    return text => {
        const bytes = new TextEncoder().encode(text);
        const ptr = instance.exports.ts_input(bytes.length);
        new Uint8Array(instance.exports.memory.buffer).set(bytes, ptr);
        instance.exports.ts_service();
        return JSON.parse(new TextDecoder().decode(new Uint8Array(instance.exports.memory.buffer, instance.exports.ts_output(), instance.exports.ts_output_len())));
    };
}

test("raw initialization can recover and distinguishes parse errors", async () => {
    const call = rawService(await loadModule());
    assert.equal(call("{").error.code, -32700);
    assert.equal(call(JSON.stringify({ action: "initialize", cwd: "/", args: [], capabilities: 1, caseInsensitive: true })).error.code, -32602);
    assert.deepEqual(call(JSON.stringify({ action: "initialize", cwd: "/", args: [], capabilities: {}, caseInsensitive: false })), { result: null });
    assert.deepEqual(call(JSON.stringify({ action: "dispose" })), { result: null });
    assert.equal(call(JSON.stringify({ action: "initialize", cwd: "/", args: [], capabilities: {} })).error.code, -32602);
});

test("position-heavy results reuse the program's file line map", async () => {
    const module = await loadModule();
    await countFileReads(async reads => {
        const source = "const shared = 1;\n" + Array.from({ length: 250 }, (_, index) => `export const item${index} = shared;\n`).join("");
        const uri = "file:///a.ts";
        const service = await createCoreLanguageService(module, { args: ["--lib", "es2022", "/a.ts"], files: { "/a.ts": source }, capabilities: {
            textDocument: { semanticTokens: { requests: { full: true }, tokenTypes: ["variable"], tokenModifiers: ["declaration", "readonly", "local"], formats: ["relative"] } },
        } });
        try {
            await service.request("textDocument/hover", { textDocument: { uri }, position: { line: 0, character: 8 } });
            reads.clear();
            const result = await service.request("textDocument/semanticTokens/full", { textDocument: { uri } });
            assert.ok(result.data.length > 1_000);
            assert.ok((reads.get("/a.ts") ?? 0) <= 8, "file reads must be bounded independently of token count");
            await service.updateFiles({ "/a.ts": "\n" + source });
            const moved = await service.request("textDocument/hover", { textDocument: { uri }, position: { line: 1, character: 8 } });
            assert.equal(moved.range.start.line, 1);
        } finally { await service.dispose(); }
    });
});

test("the package entry isolates live editor state from ordinary compiler runs", async () => {
    const service = await createLanguageService({ args: ["--strict", "/a.ts"], files: { "/a.ts": "export const answer = 42;" } });
    try {
        const params = { textDocument: { uri: "file:///a.ts" }, position: { line: 0, character: 14 } };
        const before = await service.request("textDocument/hover", params);
        const compiled = await tsc(["--noEmit", "/a.ts"], { files: { "/a.ts": 'export const answer: number = "wrong";' }, diagnostics: "json" });
        assert.equal(compiled.diagnostics[0].code, 2322);
        assert.deepEqual(await service.request("textDocument/hover", params), before);
    } finally {
        await service.dispose();
    }
});
