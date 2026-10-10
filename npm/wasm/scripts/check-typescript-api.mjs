import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const reference = process.argv[2];
if (!reference) {
    console.error("Usage: node scripts/check-typescript-api.mjs <classic-typescript-package-directory> [candidate-module]");
    process.exit(2);
}
const require = createRequire(import.meta.url);
const root = resolve(reference);
const ts = require(join(root, "lib/typescript.js"));
const declaration = join(root, "lib/typescript.d.ts");
const packageRoot = dirname(dirname(fileURLToPath(import.meta.url)));
const candidate = process.argv[3] ? resolve(process.argv[3]) : join(packageRoot, "index.js");
const fileName = join(packageRoot, "__typescript_api_contract__.mts");
const text = `
import type * as ts from ${JSON.stringify(declaration)};
import { createLanguageService } from ${JSON.stringify(candidate)};
const factory: typeof ts.createLanguageService = createLanguageService;
declare const service: Awaited<ReturnType<typeof createLanguageService>>;
const replacement: ts.LanguageService = service;
`;
const options = {
    strict: true, noEmit: true, skipLibCheck: true,
    target: ts.ScriptTarget.ES2022,
    module: ts.ModuleKind.NodeNext, moduleResolution: ts.ModuleResolutionKind.NodeNext,
};
const host = ts.createCompilerHost(options);
const getSourceFile = host.getSourceFile.bind(host);
host.getSourceFile = (name, languageVersion, onError, shouldCreateNewSourceFile) => name === fileName
    ? ts.createSourceFile(name, text, languageVersion, true)
    : getSourceFile(name, languageVersion, onError, shouldCreateNewSourceFile);
const program = ts.createProgram([fileName], options, host);
const diagnostics = ts.getPreEmitDiagnostics(program).map(diagnostic => ({
    code: diagnostic.code,
    message: ts.flattenDiagnosticMessageText(diagnostic.messageText, "\n"),
}));
const source = ts.createSourceFile(declaration, readFileSync(declaration, "utf8"), ts.ScriptTarget.Latest, true);
const members = new Set();
function visit(node) {
    if (ts.isInterfaceDeclaration(node) && node.name.text === "LanguageService") {
        for (const member of node.members) members.add(member.name.getText(source));
    }
    ts.forEachChild(node, visit);
}
visit(source);
console.log(JSON.stringify({ referenceVersion: ts.version, requiredMembers: Array.from(members), diagnostics }, null, 2));
process.exitCode = diagnostics.length ? 1 : 0;
