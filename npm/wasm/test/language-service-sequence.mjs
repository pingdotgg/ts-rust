// Shared by the Node tests and the browser worker example.
const source = 'export const person = { name: "Ada" };\n';
const main = 'import { person } from "./a";\nexport const age: number = person.name;\nperson.name;\n';
const uri = "file:///p/b.ts";

function check(value, message) {
    if (!value) throw new Error(message);
}

function position(text, marker, offset = 0) {
    const at = text.lastIndexOf(marker) + offset;
    const prefix = text.slice(0, at);
    return { line: prefix.split("\n").length - 1, character: at - prefix.lastIndexOf("\n") - 1 };
}

function params(text, marker, offset = 0, documentUri = uri) {
    return { textDocument: { uri: documentUri }, position: position(text, marker, offset) };
}

function project(text = source) {
    return {
        cwd: "/p",
        args: ["-p", "/p"],
        capabilities: {
            textDocument: {
                semanticTokens: {
                    requests: { full: true, range: true },
                    tokenTypes: ["namespace", "type", "class", "enum", "interface", "typeParameter", "parameter", "variable", "property", "enumMember", "function", "method"],
                    tokenModifiers: ["declaration", "static", "readonly", "async", "defaultLibrary", "local"],
                    formats: ["relative"],
                },
            },
        },
        files: {
            "/p/tsconfig.json": JSON.stringify({ compilerOptions: { strict: true, target: "es2022", module: "esnext", types: [] }, files: ["a.ts", "b.ts"] }),
            "/p/a.ts": text,
            "/p/b.ts": main,
        },
    };
}

function applyEdits(text, edits) {
    const offset = point => text.split("\n").slice(0, point.line).reduce((sum, line) => sum + line.length + 1, 0) + point.character;
    const changes = edits.map(edit => ({ start: offset(edit.range.start), end: offset(edit.range.end), text: edit.newText }));
    changes.sort((first, second) => second.start - first.start);
    for (const change of changes) text = text.slice(0, change.start) + change.text + text.slice(change.end);
    return text;
}

async function answers(service, type, errors, extraMember = false) {
    for (let attempt = 0; attempt < 2; attempt++) {
        const hover = await service.request("textDocument/hover", params(main, "person.name", 8));
        check(JSON.stringify(hover).includes(type), `hover must report ${type}`);
        const completions = await service.request("textDocument/completion", params(main, "person.name", 7));
        const labels = (completions.items ?? completions).map(item => item.label);
        check(labels.includes("name"), "completion must contain name");
        check(labels.includes("age") === extraMember, "completion must reflect the updated dependency");
        const diagnostics = await service.request("textDocument/diagnostic", { textDocument: { uri } });
        check(diagnostics.items.length === errors, `expected ${errors} diagnostics, got ${JSON.stringify(diagnostics)}`);
        if (errors) check(diagnostics.items[0].code === 2322, "diagnostics must identify TS2322");
    }
}

async function rejects(operation, description) {
    try { await operation(); } catch { return; }
    throw new Error(`${description} must reject`);
}

export async function verifyLanguageService(create) {
    const service = await create(project());
    const isolated = await create(project('export const person = { name: 42, age: 36 };\n'));
    try {
        await answers(service, "string", 1);
        await answers(isolated, "number", 0, true);
        const definition = await service.request("textDocument/definition", params(main, "person.name", 2));
        check(JSON.stringify(definition).includes("file:///p/a.ts"), "definition must resolve across files");
        const references = await service.request("textDocument/references", { ...params(main, "person.name", 2), context: { includeDeclaration: true } });
        check(references.length >= 4, "references must include the declaration and uses");
        const localRename = await service.request("textDocument/rename", { ...params(main, "person.name", 2), newName: "localPerson" });
        check(JSON.stringify(localRename).includes("person as localPerson"), "renaming an imported binding must preserve the export");
        const rename = await service.request("textDocument/rename", { ...params(source, "person", 2, "file:///p/a.ts"), newName: "renamedPerson" });
        check(JSON.stringify(rename).includes("file:///p/a.ts") && JSON.stringify(rename).includes(uri), "rename must edit both files");
        const document = { textDocument: { uri } };
        const range = { start: { line: 0, character: 0 }, end: { line: 2, character: 12 } };
        const symbols = await service.request("textDocument/documentSymbol", document);
        check(Array.isArray(symbols) && symbols.length > 0, "document symbols must describe declarations");
        const tokens = await service.request("textDocument/semanticTokens/full", document);
        check(tokens.data.length > 0 && tokens.data.length % 5 === 0, "semantic tokens must use the requested legend");
        const rangedTokens = await service.request("textDocument/semanticTokens/range", { ...document, range });
        check(rangedTokens.data.length > 0, "range semantic tokens must work");
        const highlights = await service.request("textDocument/documentHighlight", params(main, "person.name", 2));
        check(highlights.length > 0, "document highlights must identify uses");
        await service.request("textDocument/typeDefinition", params(main, "person.name", 2));
        await service.request("textDocument/implementation", params(main, "person.name", 2));
        await service.request("textDocument/signatureHelp", params(main, "person.name", 2));
        await service.request("textDocument/codeAction", { ...document, range, context: { diagnostics: [] } });
        const formatted = await service.request("textDocument/formatting", { ...document, options: { tabSize: 2, insertSpaces: true } });
        check(Array.isArray(formatted), "formatting must return edits");
        await service.request("textDocument/rangeFormatting", { ...document, range, options: { tabSize: 2, insertSpaces: true } });
        const renamed = {};
        for (const [documentUri, edits] of Object.entries(rename.changes)) {
            const path = new URL(documentUri).pathname;
            renamed[path] = applyEdits(path === "/p/a.ts" ? source : main, edits);
        }
        await service.updateFiles(renamed);
        const renamedDiagnostics = await service.request("textDocument/diagnostic", document);
        check(renamedDiagnostics.items.length === 1 && renamedDiagnostics.items[0].code === 2322, "applied rename must preserve the existing type error without adding errors");
        await service.updateFiles({ "/p/a.ts": source, "/p/b.ts": main });
        await service.updateFiles({ "/p/a.ts": 'export const person = { name: 42, age: 36 };\n' });
        await answers(service, "number", 0, true);
        await service.updateFiles({ "/p/a.ts": source });
        await answers(service, "string", 1);
        await answers(isolated, "number", 0, true);

        const unicode = 'const emoji = "😀"; const café: number = "x";\r\ncafé;\r\n';
        await service.updateFiles({ "/p/a.ts": unicode });
        const diagnostics = await service.request("textDocument/diagnostic", { textDocument: { uri: "file:///p/a.ts" } });
        const error = diagnostics.items.find(item => item.code === 2322);
        check(error.range.start.line === 0 && error.range.start.character === unicode.indexOf("café"), "diagnostic positions must count UTF-16 units");
        const hover = await service.request("textDocument/hover", params(unicode, "café", 1, "file:///p/a.ts"));
        check(JSON.stringify(hover).includes("number"), "Unicode identifier hover must work");
        const edits = await service.request("textDocument/rename", { ...params(unicode, "café", 1, "file:///p/a.ts"), newName: "coffee" });
        check(JSON.stringify(edits).includes('"newText":"coffee"'), "Unicode rename must work");

        await rejects(() => service.request("textDocument/hover", { textDocument: { uri }, position: { line: -1, character: 0 } }), "negative position");
        await rejects(() => service.request("textDocument/hover", { textDocument: { uri }, position: { line: 900, character: 0 } }), "out-of-range position");
        const end = { textDocument: { uri }, position: { line: 2, character: 12 } };
        const clamped = { textDocument: { uri }, position: { line: 2, character: 1_000_000 } };
        check(JSON.stringify(await service.request("textDocument/hover", end)) === JSON.stringify(await service.request("textDocument/hover", clamped)), "overlong columns must clamp to line end");
        await rejects(() => service.request("textDocument/hover", { textDocument: { uri: "bad%" }, position: { line: 0, character: 0 } }), "invalid URI");
        await rejects(() => service.request("textDocument/hover", {}), "missing document");
        await rejects(() => service.request("unsupported", { textDocument: { uri } }), "unsupported request");
        await rejects(() => service.request("textDocument/codeAction", {
            textDocument: { uri }, range: { start: { line: 0, character: 0 }, end: { line: 0, character: 1 } }, context: { diagnostics: [null] },
        }), "null diagnostic");
        await rejects(() => service.request("textDocument/codeAction", {
            textDocument: { uri }, range: { start: { line: 0, character: 0 }, end: { line: 0, character: 1 } },
            context: { diagnostics: [{ code: 2322, message: "type error", range: { start: { line: 900, character: 0 }, end: { line: 900, character: 1 } } }] },
        }), "invalid diagnostic position");
        await rejects(() => service.request("textDocument/rangeFormatting", {
            textDocument: { uri }, range: { start: { line: 1, character: 0 }, end: { line: 0, character: 0 } }, options: { tabSize: 2, insertSpaces: true },
        }), "reversed range");
        await service.updateFiles({ "/p/a.ts": `/*${"x".repeat(150_000)}*/\n${source}` });
        await answers(service, "string", 1);
        await service.deleteFiles(["/p/a.ts"]);
        await rejects(() => service.request("textDocument/hover", { textDocument: { uri: "file:///p/a.ts" }, position: { line: 0, character: 0 } }), "deleted document");
        await service.updateFiles({ "/p/a.ts": source });
        await answers(service, "string", 1);
        await service.dispose();
        await service.dispose();
        await rejects(() => service.request("textDocument/diagnostic", { textDocument: { uri } }), "disposed service");
        await answers(isolated, "number", 0, true);
        await rejects(() => create({ args: ["a.ts"], files: { "a.ts": "const n = 1;" } }), "relative creation path");
        await rejects(() => create({ args: ["/a.ts"], files: { "/a.ts": 123 } }), "non-text creation file");
        await verifyResultShapes(create);
        return { updates: true, navigation: true, isolation: true, unicode: true, boundaries: true, disposal: true };
    } finally {
        await service.dispose();
        await isolated.dispose();
    }
}

async function verifyResultShapes(create) {
    const a = 'export interface Person { name: string }\nexport class User implements Person { name = "Ada"; }\nexport function greet(person: Person, greeting: string): string { return greeting + person.name; }\nexport const unused = 1;\n';
    const b = 'import { User } from "./a";\nimport { Person, greet, unused } from "./a";\nexport const person: Person = new User();\ngreet(person, "hello");\nperson;\n';
    const options = project();
    options.files["/p/a.ts"] = a;
    options.files["/p/b.ts"] = b;
    const service = await create(options);
    try {
        const types = await service.request("textDocument/typeDefinition", params(b, "person;", 2));
        check(JSON.stringify(types).includes("file:///p/a.ts"), "type definition must locate the interface");
        const implementations = await service.request("textDocument/implementation", params(a, "interface Person", 12, "file:///p/a.ts"));
        check(JSON.stringify(implementations).includes('"line":1'), "implementation must locate the implementing class");
        const signature = await service.request("textDocument/signatureHelp", params(b, 'greet(person, "hello")', 14));
        check(signature.signatures.length > 0 && signature.activeParameter === 1, "signature help must identify the second parameter");
        check(signature.signatures[0].parameters.length === 2, "signature help must return both parameters");
        const actions = await service.request("textDocument/codeAction", {
            textDocument: { uri }, range: { start: { line: 0, character: 0 }, end: { line: 4, character: 7 } },
            context: { diagnostics: [], only: ["source.removeUnusedImports.ts"] },
        });
        const action = actions.find(action => action.edit);
        check(action, "unused-import action must provide an edit");
        const changes = action.edit.changes ?? {};
        let updated = b;
        if (changes[uri]) updated = applyEdits(b, changes[uri]);
        for (const change of action.edit.documentChanges ?? []) {
            if (change.textDocument?.uri === uri) updated = applyEdits(b, change.edits);
        }
        check(!updated.includes("unused"), "the action must remove the unused import");
        await service.updateFiles({ "/p/b.ts": updated });
        const diagnostics = await service.request("textDocument/diagnostic", { textDocument: { uri } });
        check(!diagnostics.items.some(item => item.severity === 1), "the applied action must leave a valid program");
        const compact = 'export const compact={a:1,b:2};\nexport const sentinel = 7;\n';
        await service.updateFiles({ "/p/b.ts": compact });
        const edits = await service.request("textDocument/rangeFormatting", {
            textDocument: { uri }, range: { start: { line: 0, character: 0 }, end: { line: 0, character: 30 } }, options: { tabSize: 2, insertSpaces: true },
        });
        check(edits.length > 0, "range formatting must return edits for compact source");
        const formatted = applyEdits(compact, edits);
        check(formatted.includes("compact =") && formatted.endsWith("export const sentinel = 7;\n"), "range formatting must change the target and preserve surrounding text");
    } finally { await service.dispose(); }
}
