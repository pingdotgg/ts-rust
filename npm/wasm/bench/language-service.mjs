import { createLanguageService } from "../core.js";
import { loadModule } from "../node.js";
import { countFileReads } from "../test/wasm-file-reads.mjs";

const module = await loadModule(process.argv[2]);
const uri = "file:///bench/main.ts";
const capabilities = { textDocument: { semanticTokens: {
    requests: { full: true }, tokenTypes: ["variable", "property", "function"],
    tokenModifiers: ["declaration", "readonly", "local"], formats: ["relative"],
} } };

for (const rows of [1_000, 4_000]) {
    const text = "const shared = 1;\n" + Array.from({ length: rows }, (_, index) =>
        `export const item${index} = { value: shared + ${index}, label: "row" };\n`).join("");
    await countFileReads(async reads => {
        const service = await createLanguageService(module, {
            args: ["--strict", "--lib", "es2022", "/bench/main.ts"], files: { "/bench/main.ts": text }, capabilities,
        });
        try {
            await service.request("textDocument/hover", { textDocument: { uri }, position: { line: 0, character: 8 } });
            for (const method of ["textDocument/semanticTokens/full", "textDocument/documentSymbol", "textDocument/references"]) {
                reads.clear();
                const start = performance.now();
                const result = await service.request(method, { textDocument: { uri }, position: { line: 0, character: 8 }, context: { includeDeclaration: true } });
                const milliseconds = performance.now() - start;
                const results = result.data ? result.data.length / 5 : result.length;
                console.log(JSON.stringify({ rows, bytes: Buffer.byteLength(text), method, milliseconds, results, fileReads: reads.get("/bench/main.ts") ?? 0 }));
            }
        } finally { await service.dispose(); }
    });
}
