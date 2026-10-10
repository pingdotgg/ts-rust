import assert from "node:assert/strict";
import { test } from "node:test";
import { createLanguageService, loadModule, tsc } from "../node.js";
import { createLanguageService as createCoreLanguageService } from "../core.js";
import { verifyLanguageService } from "./language-service-sequence.mjs";

test("persistent language services update, navigate, isolate and dispose", async () => {
    const module = await loadModule();
    const result = await verifyLanguageService(options => createCoreLanguageService(module, options));
    assert.equal(Object.values(result).every(Boolean), true);
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
