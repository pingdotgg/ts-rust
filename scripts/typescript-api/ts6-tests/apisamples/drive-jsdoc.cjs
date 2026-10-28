"use strict";

// APISample_jsdoc only defines functions: evaluate it unmodified in a vm context, then call them on
// every node of a fixture program.
const fs = require("node:fs");
const vm = require("node:vm");
const ts = require("typescript");

const context = vm.createContext({ require, exports: {}, console });
vm.runInContext(fs.readFileSync(process.argv[2], "utf8"), context, { filename: process.argv[2] });

const source = [
    "/** Adds.",
    " * @param a first",
    " * @param b second",
    " * @returns sum",
    " * @deprecated use plus",
    " */",
    "function add(a: number, b: number) { return a + b; }",
    "/** @type {(x: string) => number} */",
    "const f = (x) => 1;",
    "/** @class @augments Base */",
    "function C() {}",
    "",
].join("\n");
const host = ts.createCompilerHost({});
const getSourceFile = host.getSourceFile;
host.getSourceFile = (name, languageVersion) => name === "/fx.ts" ? ts.createSourceFile(name, source, languageVersion, true) : getSourceFile(name, languageVersion);
const program = ts.createProgram(["/fx.ts"], { noLib: true }, host);
const checker = program.getTypeChecker();
const sourceFile = program.getSourceFile("/fx.ts");

function describe(node) {
    if (!node) {
        return String(node);
    }
    if (Array.isArray(node)) {
        return node.map(describe);
    }
    return ts.SyntaxKind[node.kind] + ":" + node.getText(sourceFile).replace(/\s+/g, " ").slice(0, 60);
}

const lines = [];
ts.forEachChild(sourceFile, function visit(node) {
    lines.push([ts.SyntaxKind[node.kind], describe(context.parseSpecificTags(node)), describe(context.getReturnTypeFromJSDoc(node)), describe(context.getSomeOtherTags(node))].join(" | "));
    const symbol = checker.getSymbolAtLocation(node.name || node);
    if (symbol) {
        const definition = {};
        context.parseCommentsIntoDefinition.call({ checker, userValidationKeywords: {}, parseValue: value => value }, symbol, definition, {});
        lines.push("  def " + JSON.stringify(definition));
    }
    ts.forEachChild(node, visit);
});
console.log(lines.join("\n"));
