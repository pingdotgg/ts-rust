// Tests for src/flags.js, src/names.js, src/options.js and src/diagnostics.js.
// Run after `node scripts/build.mjs`: `node --test test/translate.test.mjs`.
// The tests that drive tsc-rs send raw requests (TS7's API, crates/ts_goport/src/api) through this
// package's own session (src/session.js) and run tsc-rs's command line with bin/tsc, so they need
// the addon (scripts/build-native.mjs). The ones that compare with TypeScript 6.0.3 itself use the
// copy scripts/build.mjs unpacks (test/support.mjs). Each skips with a reason when its tool is
// missing.

import { after, test } from "node:test";
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { createRequire } from "node:module";

import { packageDir, repoDir, stockMain as ts6Main, stockSkip as ts6Skip } from "./support.mjs";

const require = createRequire(import.meta.url);
const ts = require(path.join(packageDir, "lib/typescript.js"));
const flags = require(path.join(packageDir, "src/flags.js"));
const names = require(path.join(packageDir, "src/names.js"));
const { toNativeOptions, toTsOptionDiagnostics, NATIVE_OPTION_NAMES } = require(path.join(packageDir, "src/options.js"));
const { toTsDiagnostic, toTsDiagnostics } = require(path.join(packageDir, "src/diagnostics.js"));
const { getSession } = require(path.join(packageDir, "src/session.js"));
const { addonPath } = require(path.join(packageDir, "src/native/addon.js"));

const rustSource = path.join(repoDir, "crates/ts_goport/src");
const tscPath = path.join(packageDir, "bin/tsc");
const nativeSkip = fs.existsSync(addonPath()) ? false : `no tsc-rs addon at ${addonPath()} (run scripts/build-native.mjs)`;
const ts6 = ts6Main === undefined ? undefined : require(ts6Main);
// With "/" and without links (macOS /var, Windows short names): the names tsc-rs reports.
const workDir = ts.normalizePath(fs.realpathSync.native(fs.mkdtempSync(path.join(os.tmpdir(), "tsrs-translate-"))));

after(() => {
    fs.rmSync(workDir, { recursive: true, force: true });
});

// A tsc-rs snapshot with one project. `request` adds the snapshot and the project to the params;
// `list` is a request whose answer is a list.
function openNative(params) {
    const session = getSession(ts, ts.sys.useCaseSensitiveFileNames, workDir);
    const { snapshot, projects } = session.request("createSnapshot", { fileNotifications: { invalidateAll: true }, ...params });
    assert.equal(projects.length, 1);
    const [project] = projects;
    const request = (method, args) => session.request(method, { snapshot, project: project.id, ...args });
    return {
        compilerOptions: project.compilerOptions,
        request,
        list: (method, args) => request(method, args) ?? [],
        dispose: () => session.request("release", { snapshot }),
    };
}

function openNativeProject(configPath) {
    return openNative({ openProjects: [ts.normalizePath(configPath)] });
}

function createNativeProgram(rootFiles, compilerOptions) {
    return openNative({ createPrograms: [{ rootFiles: rootFiles.map(file => ts.normalizePath(file)), compilerOptions }] });
}

function relativeName(dir, fileName) {
    return path.relative(dir, fileName).replaceAll("\\", "/");
}

function writeProject(name, files) {
    const dir = `${workDir}/${name}`;
    fs.rmSync(dir, { recursive: true, force: true });
    for (const [file, text] of Object.entries(files)) {
        fs.mkdirSync(path.dirname(path.join(dir, file)), { recursive: true });
        fs.writeFileSync(path.join(dir, file), typeof text === "string" ? text : JSON.stringify(text, undefined, 4));
    }
    return dir;
}

function parseConfig(typescript, configPath) {
    return typescript.getParsedCommandLineOfConfigFile(configPath, undefined, {
        ...typescript.sys,
        onUnRecoverableConfigFileDiagnostic: diagnostic => assert.fail(typescript.flattenDiagnosticMessageText(diagnostic.messageText, "\n")),
    });
}

function formatHost(dir) {
    return { getCurrentDirectory: () => dir, getCanonicalFileName: fileName => fileName, getNewLine: () => "\n" };
}

// --- flags.js ---------------------------------------------------------------------------------

// Members of a go_flags! enum in crates/ts_goport/src, keyed by lower-case name without
// underscores (Rust SCREAMING_SNAKE and TS PascalCase names meet there). Only single bits.
function readRustBits(file, enumName) {
    const text = fs.readFileSync(path.join(rustSource, file), "utf8");
    const start = text.indexOf(`go_flags!(${enumName},`);
    assert.notEqual(start, -1, `${enumName} in ${file}`);
    const body = text.slice(start, text.indexOf("});", start));
    const bits = new Map();
    for (const [, name, value] of body.matchAll(/^\s+(\w+) = ([^;]+);/gm)) {
        const shift = /^1 << (\d+)$/.exec(value.trim());
        const number = shift ? 2 ** Number(shift[1]) : Number(value.trim());
        if (Number.isInteger(number) && number > 0 && (number & (number - 1)) === 0 || number === 2 ** 31) {
            bits.set(normalizeName(name), number);
        }
    }
    return bits;
}

function normalizeName(name) {
    return name.replaceAll("_", "").toLowerCase();
}

function ts6Members(enumObject) {
    const members = new Map();
    for (const [name, value] of Object.entries(enumObject)) {
        if (typeof value === "number") {
            members.set(normalizeName(name), value);
        }
    }
    return members;
}

function isBit(value) {
    return value !== 0 && ((value >>> 0) & ((value >>> 0) - 1)) === 0;
}

// Each tsc-rs member bit must become the TS 6.0 member of the same name, or nothing when TS 6.0
// has no single-bit member of that name.
function assertBitsByName(rustBits, ts6Enum, convert, label, skip = new Set()) {
    const members = ts6Members(ts6Enum);
    let checked = 0;
    for (const [name, value] of rustBits) {
        if (skip.has(name)) {
            continue;
        }
        const ts6Value = members.get(name);
        const expected = ts6Value !== undefined && isBit(ts6Value) ? ts6Value : 0;
        assert.equal(convert(value) >>> 0, expected >>> 0, `${label}.${name}`);
        checked++;
    }
    return checked;
}

test("native SyntaxKind names match the tsc-rs syntax_kind.rs enum", () => {
    const text = fs.readFileSync(path.join(rustSource, "astdata/syntax_kind.rs"), "utf8");
    const body = text.slice(text.indexOf("pub enum SyntaxKind {"), text.indexOf("\n}", text.indexOf("pub enum SyntaxKind {")));
    const rustKinds = [...body.matchAll(/^\s+(\w+) = (\d+),/gm)].map(([, name, value]) => [normalizeName(name), Number(value)]);
    assert.ok(rustKinds.length > 350);
    for (const [name, value] of rustKinds) {
        assert.equal(normalizeName(flags.NATIVE_SYNTAX_KIND_NAMES[value]), name, `tsc-rs SyntaxKind ${value}`);
    }
    // Count, the one entry after the last Rust kind.
    assert.equal(flags.NATIVE_SYNTAX_KIND_NAMES.length, rustKinds.length + 1);
    assert.equal(flags.nativeSyntaxKind.Count, rustKinds.length);
});

test("TS 6.0 SyntaxKind names match the TS 6.0.3 runtime enum", () => {
    assert.equal(ts.version, "6.0.3");
    for (const [kind, name] of flags.TS6_SYNTAX_KIND_NAMES.entries()) {
        assert.equal(ts.SyntaxKind[name], kind, name);
    }
    assert.equal(flags.TS6_SYNTAX_KIND_NAMES.length, ts.SyntaxKind.Count + 1);
});

test("syntaxKindToTs6 maps every tsc-rs kind by name and only the documented kinds to undefined", () => {
    const renamed = { EndOfFile: "EndOfFileToken", JSDocUnknownTag: "JSDocTag" };
    const unmapped = new Set(["ImmediateKeyword", "SourceKeyword", "JSTypeAliasDeclaration", "JSImportDeclaration"]);
    for (const [kind, name] of flags.NATIVE_SYNTAX_KIND_NAMES.entries()) {
        const expected = unmapped.has(name) ? undefined : ts.SyntaxKind[renamed[name] ?? name];
        assert.equal(flags.syntaxKindToTs6(kind), expected, name);
    }
    assert.equal(flags.syntaxKindToTs6(79), ts.SyntaxKind.Identifier);
    assert.equal(flags.syntaxKindToTs6(9999), undefined);
});

test("syntaxKindToNative maps every TS 6.0 kind by name and only the documented kinds to undefined", () => {
    const renamed = {
        EndOfFileToken: "EndOfFile",
        JSDocTag: "JSDocUnknownTag",
        JSDocAuthorTag: "JSDocUnknownTag",
        JSDocClassTag: "JSDocUnknownTag",
        JSDocEnumTag: "JSDocUnknownTag",
        JSDocMemberName: "QualifiedName",
    };
    const unmapped = new Set(["ShebangTrivia", "ImportTypeAssertionContainer", "Bundle", "JSDocUnknownType", "JSDocFunctionType", "JSDocNamepathType", "CommaListExpression"]);
    for (const [kind, name] of flags.TS6_SYNTAX_KIND_NAMES.entries()) {
        const expected = unmapped.has(name) ? undefined : flags.nativeSyntaxKind[renamed[name] ?? name];
        assert.equal(flags.syntaxKindToNative(kind), expected, name);
    }
    for (const marker of ["FirstToken", "LastToken", "FirstKeyword", "FirstNode", "FirstJSDocNode", "LastTypeNode"]) {
        assert.equal(flags.syntaxKindToTs6(flags.syntaxKindToNative(ts.SyntaxKind[marker])), ts.SyntaxKind[marker], marker);
    }
});

test("object flags translate by member name for object types and for unions and intersections", () => {
    const bits = readRustBits("flags.rs", "ObjectFlags");
    const unionNames = new Set(["isgenerictypecomputed", "isgenericobjecttype", "isgenericindextype", "containsintersections", "isunknownlikeunioncomputed", "isunknownlikeunion", "isuniformenumcomputed", "isuniformenum", "isneverintersectioncomputed", "isneverintersection", "isconstrainedtypevariable"]);
    const objectNames = new Set([...bits.keys()].filter(name => !unionNames.has(name)));
    const objectChecked = assertBitsByName(bits, ts.ObjectFlags, value => flags.objectFlagsToTs6(value, ts.TypeFlags.Object), "ObjectFlags(object)", unionNames);
    const unionChecked = assertBitsByName(bits, ts.ObjectFlags, value => flags.objectFlagsToTs6(value, ts.TypeFlags.Union), "ObjectFlags(union)", objectNames);
    assert.ok(objectChecked > 25 && unionChecked > 8);
    assert.equal(flags.objectFlagsToTs6(bits.get("singlesignaturetype")), ts.ObjectFlags.SingleSignatureType);
});

test("check, signature, symbol, type and element flags translate by member name", () => {
    assertBitsByName(readRustBits("flags.rs", "CheckFlags"), ts.CheckFlags, flags.checkFlagsToTs6, "CheckFlags");
    assertBitsByName(readRustBits("flags.rs", "SignatureFlags"), ts.SignatureFlags, flags.signatureFlagsToTs6, "SignatureFlags");
    assertBitsByName(readRustBits("flags.rs", "SymbolFlags"), ts.SymbolFlags, flags.symbolFlagsToTs6, "SymbolFlags");
    assertBitsByName(readRustBits("flags.rs", "TypeFlags"), ts.TypeFlags, flags.typeFlagsToTs6, "TypeFlags");
    assertBitsByName(readRustBits("flags.rs", "ElementFlags"), ts.ElementFlags, flags.elementFlagsToTs6, "ElementFlags");
    for (const [name, value] of ts6Members(ts.SymbolFlags)) {
        if (isBit(value)) {
            assert.equal(flags.symbolFlagsToNative(value), readRustBits("flags.rs", "SymbolFlags").get(name), name);
        }
    }
    assert.equal(flags.symbolFlagsToNative(ts.SymbolFlags.All), 2 ** 28 - 1);
});

test("node flags translate by member name, with NestedNamespace on module declarations", () => {
    const bits = readRustBits("flags.rs", "NodeFlags");
    // OptionalChain and NestedNamespace share a tsc-rs bit; HasAsyncFunctions has an alias.
    const skip = new Set(["nestednamespace", "identifierisinjsdocnamespace", "identifierhasextendedunicodeescape"]);
    assertBitsByName(bits, ts.NodeFlags, value => flags.nodeFlagsToTs6(value, flags.nativeSyntaxKind.Identifier), "NodeFlags", skip);
    const moduleDeclaration = flags.nativeSyntaxKind.ModuleDeclaration;
    assert.equal(flags.nodeFlagsToTs6(bits.get("nestednamespace"), moduleDeclaration), ts.NodeFlags.NestedNamespace);
    assert.equal(flags.nodeFlagsToTs6(bits.get("optionalchain"), flags.nativeSyntaxKind.PropertyAccessExpression), ts.NodeFlags.OptionalChain);
    for (const name of ["Let", "Const", "OptionalChain", "Ambient", "JSDoc", "JsonFile", "Unreachable", "ThisNodeHasError"]) {
        assert.equal(flags.nodeFlagsToNative(ts.NodeFlags[name], ts.SyntaxKind.Identifier), bits.get(normalizeName(name)), name);
    }
    assert.equal(flags.nodeFlagsToNative(ts.NodeFlags.NestedNamespace, ts.SyntaxKind.ModuleDeclaration), bits.get("nestednamespace"));
});

test("node builder and type format flags keep every TS 6.0 bit and no tsc-rs-only bit", () => {
    const builderBits = readRustBits("checker/nodebuilder_types.rs", "NodeBuilderFlags");
    for (const [name, value] of ts6Members(ts.NodeBuilderFlags)) {
        if (isBit(value)) {
            assert.equal(flags.nodeBuilderFlagsToNative(value), builderBits.get(name), `NodeBuilderFlags.${name}`);
        }
    }
    const formatBits = readRustBits("flags.rs", "TypeFormatFlags");
    for (const [name, value] of ts6Members(ts.TypeFormatFlags)) {
        if (isBit(value)) {
            assert.equal(flags.typeFormatFlagsToNative(value), formatBits.get(name), `TypeFormatFlags.${name}`);
        }
    }
    const nativeOnly = builderBits.get("useinstantiationexpressions") | builderBits.get("writecallstylesignature");
    assert.equal(flags.nodeBuilderFlagsToNative(-1) & nativeOnly, 0);
    assert.equal(flags.typeFormatFlagsToNative(-1) & (formatBits.get("useinstantiationexpressions") | formatBits.get("writecallstylesignature")), 0);
});

test("option enums translate between TS 6.0 and tsc-rs numbering", () => {
    assert.equal(flags.newLineKindToNative(ts.NewLineKind.CarriageReturnLineFeed), 1);
    assert.equal(flags.newLineKindToNative(ts.NewLineKind.LineFeed), 2);
    assert.equal(flags.newLineKindToTs6(2), ts.NewLineKind.LineFeed);
    assert.equal(flags.newLineKindToTs6(0), undefined);
    assert.equal(flags.moduleDetectionKindToNative(ts.ModuleDetectionKind.Legacy), 2);
    assert.equal(flags.moduleDetectionKindToNative(ts.ModuleDetectionKind.Auto), 1);
    assert.equal(flags.moduleDetectionKindToNative(ts.ModuleDetectionKind.Force), 3);
    assert.equal(flags.moduleDetectionKindToTs6(1), ts.ModuleDetectionKind.Auto);
    assert.equal(flags.scriptTargetToNative(ts.ScriptTarget.ES3), undefined);
    assert.equal(flags.scriptTargetToNative(ts.ScriptTarget.ES2025), 12);
    assert.equal(flags.moduleKindToNative(ts.ModuleKind.None), undefined);
    assert.equal(flags.moduleKindToNative(ts.ModuleKind.NodeNext), 199);
    assert.equal(flags.scriptKindToNative(ts.ScriptKind.External), ts.ScriptKind.TS);
    assert.equal(flags.scriptKindToNative(ts.ScriptKind.Deferred), ts.ScriptKind.TS);
    assert.equal(flags.scriptKindToNative(ts.ScriptKind.TSX), 4);
});

// --- names.js ---------------------------------------------------------------------------------

test("escaped names pass through from tsc-rs and map back to binder names", () => {
    for (const name of ["x", "___foo", "__call", "__@iterator@60", "__#7@#priv", "export=", "default", '"foo"']) {
        assert.equal(names.escapedNameToTs6(name), name);
    }
    assert.equal(names.escapedNameToNative("x"), "x");
    assert.equal(names.escapedNameToNative("_x"), "_x");
    assert.equal(names.escapedNameToNative("___foo"), "__foo");
    assert.equal(names.escapedNameToNative("____bar"), "___bar");
    assert.equal(names.escapedNameToNative("export="), "export=");
    for (const internal of ["__call", "__new", "__index", "__type", "__object", "__@iterator@60", "__#7@#priv"]) {
        assert.equal(names.escapedNameToNative(internal), undefined, internal);
        assert.ok(names.isInternalEscapedName(internal));
    }
    assert.equal(names.symbolNameOfEscapedName("__#7@#priv"), "#priv");
    assert.equal(names.symbolNameOfEscapedName("___foo"), "__foo");
    assert.equal(names.symbolNameOfEscapedName("__call"), "__call");
    assert.equal(names.symbolNameOfEscapedName("__@iterator@60"), "__@iterator@60");
    for (const name of ["__foo", "___bar", "_x", "x", "__"]) {
        assert.equal(names.escapedNameToNative(ts.escapeLeadingUnderscores(name)), name);
    }
});

const namesSource = `declare const k: unique symbol;
export class C {
    __foo = 1;
    ___bar = 2;
    _single = 3;
    #priv = 4;
    #__privUnder = 5;
    static #sp = 6;
    [Symbol.iterator](): Iterator<number> { return null as any; }
    [Symbol.asyncIterator](): any { return null; }
    [k] = 7;
    ["lit"] = 8;
    constructor() {}
}
export interface I {
    (): void;
    new (): I;
    [x: string]: any;
    __x: number;
    "__proto__": 1;
    "___": 2;
    "__": 3;
}
export default function () {}
export namespace NS { export const a = 1; }
export type T = { a: 1 };
export const o = { a: 1 };
export const fe = function () {};
export const ce = class {};
export * from "./b";
`;

test("tsc-rs reports the escaped names TS 6.0.3 reports", { skip: nativeSkip || ts6Skip }, () => {
    const dir = writeProject("names", {
        "a.ts": namesSource,
        "b.ts": "export const fromB = 1;\n",
        "e.ts": "const x = 1;\nexport = x;\n",
        "tsconfig.json": { compilerOptions: { strict: true, target: "es2022", module: "commonjs", types: [] }, files: ["a.ts", "e.ts"] },
    });
    const ids = name => name.replace(/^__([@#])(.*?)@(\d+)$/, "__$1$2@N").replace(/^__#\d+@/, "__#N@");
    const native = collectNativeNames(dir);
    const reference = collectTs6Names(dir);
    assert.deepEqual(Object.keys(native).sort(), Object.keys(reference).sort());
    for (const [label, list] of Object.entries(reference)) {
        assert.deepEqual(native[label].map(name => ids(names.escapedNameToTs6(name))).sort(), list.map(ids).sort(), label);
    }
});

// A tsc-rs symbol's `name` is its escaped name (TS7's Symbol.escapedName).
function collectNativeNames(dir) {
    const native = openNativeProject(path.join(dir, "tsconfig.json"));
    try {
        const fileSymbol = name => native.request("getSymbolOfSourceFile", { file: `${dir}/${name}` });
        const escaped = symbols => symbols.map(symbol => symbol.name);
        const table = (symbol, method) => escaped(native.list(method, { symbol: symbol.reference }));
        const declaredType = symbol => native.request("getDeclaredTypeOfSymbol", { symbol: symbol.reference });
        const typeOf = symbol => native.request("getTypeOfSymbol", { symbol: symbol.reference });
        const symbolOf = type => native.request("getSymbolOfType", { objectId: type.id });
        const file = fileSymbol("a.ts");
        const exports = native.list("getExportsOfModule", { symbol: file.reference });
        const byName = name => exports.find(symbol => symbol.name === name);
        return {
            fileExports: table(file, "getExportsOfSymbol"),
            classMembers: table(byName("C"), "getMembersOfSymbol"),
            classStatics: table(byName("C"), "getExportsOfSymbol"),
            classProperties: escaped(native.list("getPropertiesOfType", { type: declaredType(byName("C")).id })),
            interfaceMembers: table(byName("I"), "getMembersOfSymbol"),
            typeLiteral: escaped([symbolOf(declaredType(byName("T")))]),
            objectLiteral: escaped([symbolOf(typeOf(byName("o")))]),
            functionExpression: escaped([symbolOf(typeOf(byName("fe")))]),
            classExpression: escaped([symbolOf(typeOf(byName("ce")))]),
            exportEquals: table(fileSymbol("e.ts"), "getExportsOfSymbol"),
        };
    }
    finally {
        native.dispose();
    }
}

function collectTs6Names(dir) {
    const parsed = parseConfig(ts6, path.join(dir, "tsconfig.json"));
    const program = ts6.createProgram(parsed.fileNames, parsed.options);
    const checker = program.getTypeChecker();
    const file = checker.getSymbolAtLocation(program.getSourceFile(path.join(dir, "a.ts")));
    const exports = checker.getExportsOfModule(file);
    const byName = name => exports.find(symbol => symbol.name === name);
    const escaped = symbols => [...symbols].map(symbol => symbol.escapedName);
    return {
        fileExports: [...file.exports.keys()],
        classMembers: [...byName("C").members.keys()],
        classStatics: [...byName("C").exports.keys()],
        classProperties: escaped(checker.getPropertiesOfType(checker.getDeclaredTypeOfSymbol(byName("C")))),
        interfaceMembers: [...byName("I").members.keys()],
        typeLiteral: escaped([checker.getDeclaredTypeOfSymbol(byName("T")).symbol]),
        objectLiteral: escaped([checker.getTypeOfSymbol(byName("o")).symbol]),
        functionExpression: escaped([checker.getTypeOfSymbol(byName("fe")).symbol]),
        classExpression: escaped([checker.getTypeOfSymbol(byName("ce")).symbol]),
        exportEquals: [...checker.getSymbolAtLocation(program.getSourceFile(path.join(dir, "e.ts"))).exports.keys()],
    };
}

// --- options.js: translation ------------------------------------------------------------------

test("enum options are renumbered for tsc-rs and string enum values are accepted", () => {
    const { options } = toNativeOptions(ts, {
        target: ts.ScriptTarget.ES2021,
        module: ts.ModuleKind.NodeNext,
        moduleResolution: ts.ModuleResolutionKind.NodeNext,
        jsx: ts.JsxEmit.ReactJSX,
        newLine: ts.NewLineKind.LineFeed,
        moduleDetection: ts.ModuleDetectionKind.Legacy,
    });
    assert.deepEqual(options, { target: 8, module: 199, moduleResolution: 99, jsx: 4, newLine: 2, moduleDetection: 2 });
    const fromStrings = toNativeOptions(ts, { target: "ES2020", module: "commonjs", newLine: "crlf", moduleDetection: "auto", jsx: "react-jsx", sourceMap: 1 }).options;
    assert.deepEqual(fromStrings, { jsx: 4, sourceMap: true, target: 7, module: 1, newLine: 1, moduleDetection: 1 });
    assert.deepEqual(toNativeOptions(ts, { target: "es3" }).diagnostics.map(d => d.code), [5108]);
    assert.deepEqual(toNativeOptions(ts, { target: "es1", module: {} }).options, { target: 12 });
});

test("an unset target becomes TS 6.0's default target and ES3 counts as unset", () => {
    assert.equal(toNativeOptions(ts, {}).options.target, ts.ScriptTarget.LatestStandard);
    assert.equal(toNativeOptions(ts, { module: ts.ModuleKind.NodeNext }).options.target, ts.getEmitScriptTarget({ module: ts.ModuleKind.NodeNext }));
    const es3 = toNativeOptions(ts, { target: ts.ScriptTarget.ES3 });
    assert.equal(es3.options.target, ts.ScriptTarget.LatestStandard);
    assert.deepEqual(es3.diagnostics.map(d => d.code), [5108]);
});

test("baseUrl becomes rooted paths substitutions and a last star pattern", () => {
    const { options, diagnostics } = toNativeOptions(ts, {
        baseUrl: "/proj/src",
        paths: { "@app/*": ["app/*", "../lib/*", "/abs/*"], "exact": ["exact.ts"] },
        pathsBasePath: "/proj",
    });
    assert.equal(options.baseUrl, undefined);
    assert.deepEqual(options.paths, {
        "@app/*": ["/proj/src/app/*", "/proj/lib/*", "/abs/*"],
        "exact": ["/proj/src/exact.ts"],
        "*": ["/proj/src/*"],
    });
    assert.deepEqual(diagnostics.map(d => d.code), [5101]);
    assert.deepEqual(toNativeOptions(ts, { baseUrl: "/p", paths: { "*": ["x/*"] } }).options.paths, { "*": ["/p/x/*"] });
    assert.deepEqual(toNativeOptions(ts, { baseUrl: "./src" }).options.paths, { "*": ["./src/*"] });
    assert.deepEqual(toNativeOptions(ts, { baseUrl: "./src" }, { currentDirectory: "/cwd" }).options.paths, { "*": ["/cwd/src/*"] });
});

test("paths substitutions are rooted at pathsBasePath and non-relative ones stay for tsc-rs to report", () => {
    const { options, diagnostics } = toNativeOptions(ts, { paths: { "@a/*": ["./src/a/*", "src/b/*"] }, pathsBasePath: "/cfg" });
    assert.deepEqual(options.paths, { "@a/*": ["/cfg/src/a/*", "src/b/*"] });
    assert.deepEqual(diagnostics, []);
    assert.deepEqual(toNativeOptions(ts, { paths: { "@a/*": ["./x/*"] } }, { currentDirectory: "/cwd" }).options.paths, { "@a/*": ["/cwd/x/*"] });
});

test("lib file names pass through except the esnext libs tsc-rs renamed", () => {
    const { options } = toNativeOptions(ts, { lib: ["lib.es2020.d.ts", "lib.dom.d.ts", "lib.esnext.array.d.ts", "lib.esnext.collection.d.ts", "lib.esnext.error.d.ts", "lib.esnext.typedarrays.d.ts", "lib.esnext.disposable.d.ts"] });
    assert.deepEqual(options.lib, ["lib.es2020.d.ts", "lib.dom.d.ts", "esnext.array", "esnext.collection", "esnext.error", "esnext.typedarrays", "lib.esnext.disposable.d.ts"]);
});

test("options tsc-rs removed or always enables are dropped with TS 6.0's deprecation diagnostics", () => {
    const { options, diagnostics } = toNativeOptions(ts, {
        module: ts.ModuleKind.CommonJS,
        moduleResolution: ts.ModuleResolutionKind.Node10,
        esModuleInterop: false,
        allowSyntheticDefaultImports: false,
        alwaysStrict: false,
        downlevelIteration: true,
    });
    assert.deepEqual(options, { target: 12, module: 1 });
    assert.deepEqual(diagnostics.map(d => [d.code, ts.flattenDiagnosticMessageText(d.messageText, "\n").split("\n")[0]]), [
        [5107, `Option 'alwaysStrict=false' is deprecated and will stop functioning in TypeScript 7.0. Specify compilerOption '"ignoreDeprecations": "6.0"' to silence this error.`],
        [5107, `Option 'moduleResolution=node10' is deprecated and will stop functioning in TypeScript 7.0. Specify compilerOption '"ignoreDeprecations": "6.0"' to silence this error.`],
        [5107, `Option 'esModuleInterop=false' is deprecated and will stop functioning in TypeScript 7.0. Specify compilerOption '"ignoreDeprecations": "6.0"' to silence this error.`],
        [5107, `Option 'allowSyntheticDefaultImports=false' is deprecated and will stop functioning in TypeScript 7.0. Specify compilerOption '"ignoreDeprecations": "6.0"' to silence this error.`],
        [5101, `Option 'downlevelIteration' is deprecated and will stop functioning in TypeScript 7.0. Specify compilerOption '"ignoreDeprecations": "6.0"' to silence this error.`],
    ]);
    assert.deepEqual(toNativeOptions(ts, { esModuleInterop: true, alwaysStrict: true }).options, { target: 12, esModuleInterop: true, alwaysStrict: true });
    // Options tsc-rs removed and reports itself pass through.
    assert.deepEqual(toNativeOptions(ts, { target: ts.ScriptTarget.ES5, module: ts.ModuleKind.AMD, outFile: "/o.js", moduleResolution: ts.ModuleResolutionKind.Classic }), {
        options: { outFile: "/o.js", target: 1, module: 2, moduleResolution: 1 },
        diagnostics: [],
    });
    assert.deepEqual(toNativeOptions(ts, { module: ts.ModuleKind.None }).diagnostics.map(d => ts.flattenDiagnosticMessageText(d.messageText, "\n")), [
        "Option 'module=None' has been removed. Please remove it from your configuration.",
    ]);
});

test("node10 resolution keeps TS 6.0's errors for options that need another resolution", () => {
    const codes = options => toNativeOptions(ts, { moduleResolution: ts.ModuleResolutionKind.Node10, ignoreDeprecations: "6.0", ...options }).diagnostics.map(d => d.code);
    assert.deepEqual(codes({ module: ts.ModuleKind.CommonJS, resolvePackageJsonExports: true, customConditions: [] }), [5098, 5098]);
    assert.deepEqual(codes({ module: ts.ModuleKind.NodeNext }), [5109]);
    assert.deepEqual(codes({ module: ts.ModuleKind.CommonJS }), []);
});

test("ignoreDeprecations 6.0 silences the deprecations and typeScriptVersion 7.0 turns them into removals", () => {
    const deprecated = { baseUrl: "/p", esModuleInterop: false, keyofStringsOnly: true };
    assert.deepEqual(toNativeOptions(ts, { ...deprecated, ignoreDeprecations: "6.0" }).diagnostics.map(d => d.code), [5102]);
    assert.deepEqual(toNativeOptions(ts, { ...deprecated, ignoreDeprecations: "5.5" }).diagnostics.map(d => d.code), [5103, 5102, 5101, 5107]);
    assert.deepEqual(toNativeOptions(ts, { ...deprecated, ignoreDeprecations: "6.0" }, { typeScriptVersion: "7.0" }).diagnostics.map(d => d.code), [5102, 5102, 5108]);
});

test("TS 6.0-only removed options are dropped with error 5102", () => {
    const removed = { keyofStringsOnly: true, suppressExcessPropertyErrors: true, suppressImplicitAnyIndexErrors: true, noStrictGenericChecks: true, noImplicitUseStrict: true, charset: "utf8", out: "x.js", importsNotUsedAsValues: ts.ImportsNotUsedAsValues.Error, preserveValueImports: true };
    const { options, diagnostics } = toNativeOptions(ts, removed);
    assert.deepEqual(options, { target: 12 });
    assert.deepEqual(diagnostics.map(d => d.code), Array(9).fill(5102));
    assert.equal(ts.flattenDiagnosticMessageText(diagnostics.at(-1).messageText, "\n"), "Option 'preserveValueImports' has been removed. Please remove it from your configuration.\n  Use 'verbatimModuleSyntax' instead.");
    assert.deepEqual(toNativeOptions(ts, { importsNotUsedAsValues: ts.ImportsNotUsedAsValues.Remove }).diagnostics, []);
});

test("relative path options are rooted at the given current directory and command line options stay in JS", () => {
    const { options } = toNativeOptions(ts, {
        outDir: "dist", rootDir: "./src", declarationDir: "types", tsBuildInfoFile: "cache/x", outFile: "o.js",
        rootDirs: ["a", "/b"], typeRoots: ["./t"], configFilePath: "tsconfig.json", configFile: {}, pathsBasePath: "/x",
        watch: true, listFiles: true, pretty: true, locale: "de", traceResolution: true, help: true, unknownOption: 1,
    }, { currentDirectory: "/cwd" });
    assert.deepEqual(options, {
        outDir: "/cwd/dist", rootDir: "/cwd/src", declarationDir: "/cwd/types", tsBuildInfoFile: "/cwd/cache/x", outFile: "/cwd/o.js",
        rootDirs: ["/cwd/a", "/b"], typeRoots: ["/cwd/t"], configFilePath: "/cwd/tsconfig.json", target: 12,
    });
    assert.equal(toNativeOptions(ts, { outDir: "dist" }).options.outDir, "dist");
});

test("native option names match the tsc-rs compiler option declarations", () => {
    const text = fs.readFileSync(path.join(rustSource, "frontend/tsoptions/decls.rs"), "utf8");
    const declared = new Set([...text.matchAll(/^\s+name: "(\w+)",/gm)].map(([, name]) => name));
    for (const name of NATIVE_OPTION_NAMES) {
        if (!["configFilePath", "allowNonTsExtensions", "suppressOutputPathCheck"].includes(name)) {
            assert.ok(declared.has(name), name);
        }
    }
    const ts6Names = new Set(ts.optionDeclarations.map(option => option.name));
    const dropped = [...ts6Names].filter(name => !NATIVE_OPTION_NAMES.has(name)).sort();
    assert.deepEqual(dropped, [
        "all", "charset", "diagnostics", "explainFiles", "extendedDiagnostics", "generateCpuProfile", "generateTrace",
        "help", "ignoreConfig", "importsNotUsedAsValues", "init", "keyofStringsOnly", "listEmittedFiles", "listFiles", "listFilesOnly", "locale",
        "noImplicitUseStrict", "noStrictGenericChecks", "out", "preserveValueImports", "preserveWatchOutput", "pretty", "project",
        "showConfig", "suppressExcessPropertyErrors", "suppressImplicitAnyIndexErrors", "traceResolution", "version", "watch",
    ]);
});

test("option diagnostics from tsc-rs are placed in tsconfig.json like TS 6.0 places them", () => {
    const dir = writeProject("placement", {
        "tsconfig.json": `{\n    "compilerOptions": {\n        "module": "amd",\n        "outFile": "./out.js",\n        "exactOptionalPropertyTypes": true,\n        "strictNullChecks": false,\n        "paths": { "a/*": ["./ok/*", "bad/*"] }\n    }\n}\n`,
    });
    const parsed = parseConfig(ts, path.join(dir, "tsconfig.json"));
    const response = (code, text, messageChain) => ({ pos: -1, end: -1, code, category: 1, text, messageChain });
    const diagnostics = toTsOptionDiagnostics(ts, parsed.options, [
        response(5095, "Option 'bundler' can only be used when 'module' is set to 'preserve', 'commonjs', or 'es2015' or later."),
        response(5108, "Option 'module=AMD' has been removed. Please remove it from your configuration."),
        response(5102, "Option 'outFile' has been removed. Please remove it from your configuration."),
        response(5052, "Option 'exactOptionalPropertyTypes' cannot be specified without specifying option 'strictNullChecks'."),
        response(5090, "Non-relative paths are not allowed. Did you forget a leading './'?"),
        response(5011, "The common source directory of 'tsconfig.json' is './src'. The 'rootDir' setting must be explicitly set to this or another path to adjust your output's file layout.", [response(5111, "Visit https://aka.ms/ts6 for migration information.")]),
        response(2318, "Cannot find global type 'Array'."),
    ], () => undefined);
    assert.equal(ts.formatDiagnostics(diagnostics, formatHost(dir)), [
        "tsconfig.json(3,19): error TS5108: Option 'module=AMD' has been removed. Please remove it from your configuration.",
        "tsconfig.json(4,9): error TS5102: Option 'outFile' has been removed. Please remove it from your configuration.",
        "tsconfig.json(5,9): error TS5052: Option 'exactOptionalPropertyTypes' cannot be specified without specifying option 'strictNullChecks'.",
        "tsconfig.json(6,9): error TS5052: Option 'exactOptionalPropertyTypes' cannot be specified without specifying option 'strictNullChecks'.",
        "tsconfig.json(7,38): error TS5090: Non-relative paths are not allowed when 'baseUrl' is not set. Did you forget a leading './'?",
        "tsconfig.json(4,9): error TS5011: The common source directory of 'tsconfig.json' is './src'. The 'rootDir' setting must be explicitly set to this or another path to adjust your output's file layout.",
        "  Visit https://aka.ms/ts6 for migration information.",
        "error TS2318: Cannot find global type 'Array'.",
        "",
    ].join("\n"));
    assert.equal(toTsOptionDiagnostics(ts, { module: ts.ModuleKind.AMD, moduleResolution: ts.ModuleResolutionKind.Bundler }, [response(5095, "x")], () => undefined).length, 1);
    assert.equal(toTsOptionDiagnostics(ts, { module: ts.ModuleKind.System }, [response(5095, "x")], () => undefined).length, 0);
});

test("every non-relative paths substitution gets its own TS5090, although tsc-rs's identical ones collapse into one", () => {
    const dir = writeProject("paths-5090", {
        "tsconfig.json": `{\n    "compilerOptions": {\n        "paths": {\n            "@interface/*": ["src/interface/*"],\n            "@blah": ["./ok", "blah"],\n            "@humbug/*": ["*/generated"]\n        }\n    }\n}\n`,
    });
    const parsed = parseConfig(ts, path.join(dir, "tsconfig.json"));
    const response = { pos: -1, end: -1, code: 5090, category: 1, text: "Non-relative paths are not allowed. Did you forget a leading './'?" };
    const diagnostics = toTsOptionDiagnostics(ts, parsed.options, [response, response], () => undefined);
    const message = "error TS5090: Non-relative paths are not allowed when 'baseUrl' is not set. Did you forget a leading './'?";
    assert.equal(ts.formatDiagnostics(diagnostics, formatHost(dir)), [
        `tsconfig.json(4,30): ${message}`,
        `tsconfig.json(5,31): ${message}`,
        `tsconfig.json(6,27): ${message}`,
        "",
    ].join("\n"));
});

// --- options.js: against tsc-rs and TS 6.0.3 --------------------------------------------------

const plainSource = "export const value: number = 1;\nexport function sum(xs: number[]) { let s = 0; for (const x of xs) s += x; return s; }\n";
const scriptSource = "const scriptValue = 1;\n";
const nestSource = `import { Injectable } from "./di";
@Injectable()
export class Service {
    constructor(private readonly dep: Dep) {}
    async get(): Promise<number> { for (const x of [1, 2]) { await x; } return 1; }
}
export class Dep {}
`;
const diSource = "export function Injectable(): ClassDecorator { return () => {}; }\n";

// Each case: tsconfig compilerOptions, files, the options tsc-rs must report for the program
// (`native`, given the project directory), the option diagnostics TS 6.0.3 reports (`ts6`) and
// the ones this package reports (`codes`, ours plus tsc-rs's after toTsOptionDiagnostics), and
// the emitted files that must match TS 6.0.3 byte for byte (`emit`). Where the codes are the
// same, the formatted diagnostics must be identical too.
const optionCases = [
    {
        name: "typical NestJS CommonJS config with baseUrl",
        compilerOptions: { module: "commonjs", declaration: true, removeComments: true, emitDecoratorMetadata: true, experimentalDecorators: true, allowSyntheticDefaultImports: true, target: "ES2021", sourceMap: true, outDir: "./dist", baseUrl: "./", incremental: true, skipLibCheck: true, strictNullChecks: false, noImplicitAny: false, strictBindCallApply: false, forceConsistentCasingInFileNames: false, noFallthroughCasesInSwitch: false },
        files: { "src/main.ts": nestSource, "src/di.ts": diSource },
        native: dir => ({ module: 1, target: 8, outDir: `${dir}/dist`, paths: { "*": [`${dir}/*`] } }),
        ts6: [5011, 5101],
        codes: [5011, 5101],
    },
    {
        name: "NestJS CommonJS config with rootDir emits like TS 6.0",
        compilerOptions: { module: "commonjs", declaration: true, removeComments: true, emitDecoratorMetadata: true, experimentalDecorators: true, target: "ES2021", sourceMap: true, outDir: "./dist", rootDir: "./src", incremental: true, skipLibCheck: true, strictNullChecks: false, noImplicitAny: false },
        files: { "src/main.ts": nestSource, "src/di.ts": diSource },
        native: dir => ({ module: 1, target: 8, rootDir: `${dir}/src`, outDir: `${dir}/dist` }),
        emit: ["dist/main.js", "dist/main.d.ts", "dist/di.js"],
    },
    {
        name: "ESM nodenext",
        compilerOptions: { module: "nodenext", moduleResolution: "nodenext", target: "es2022", strict: true, outDir: "dist", rootDir: "src", declaration: true, verbatimModuleSyntax: true },
        files: { "package.json": '{"type":"module"}', "src/main.ts": 'import { helper } from "./helper.js";\nexport const v = helper("x");\n', "src/helper.ts": "export function helper(x: string): string { return x.toUpperCase(); }\n" },
        native: () => ({ module: 199, moduleResolution: 99, target: 9 }),
        emit: ["dist/main.js", "dist/helper.d.ts"],
    },
    {
        name: "bundler with jsx, lib list and moduleDetection force",
        compilerOptions: { module: "esnext", moduleResolution: "bundler", target: "es2022", jsx: "react-jsx", lib: ["dom", "dom.iterable", "esnext"], noEmit: true, allowImportingTsExtensions: true, isolatedModules: true, moduleDetection: "force", strict: true },
        files: { "src/main.ts": 'import { helper } from "./helper.ts";\nhelper();\n', "src/helper.ts": "export function helper() {}\n" },
        native: () => ({ module: 99, moduleResolution: 100, jsx: 4, moduleDetection: 3, lib: ["lib.dom.d.ts", "lib.dom.iterable.d.ts", "lib.esnext.d.ts"] }),
    },
    {
        name: "legacy baseUrl and paths with node resolution",
        compilerOptions: { baseUrl: "./src", paths: { "@app/*": ["app/*"], "lib/*": ["../lib/*"] }, moduleResolution: "node", module: "commonjs", target: "es2017", outDir: "dist", rootDir: "." },
        files: { "src/a.ts": 'import { x } from "@app/x";\nimport { y } from "lib/y";\nimport { z } from "z";\nexport const all = x + y + z;\n', "src/app/x.ts": "export const x = 1;\n", "lib/y.ts": "export const y = 2;\n", "src/z.ts": "export const z = 3;\n" },
        native: dir => ({ module: 1, target: 4, paths: { "@app/*": [`${dir}/src/app/*`], "lib/*": [`${dir}/lib/*`], "*": [`${dir}/src/*`] } }),
        ts6: [5101, 5107],
        codes: [5101, 5107],
        semantic: true,
        emit: ["dist/src/a.js"],
    },
    {
        name: "legacy baseUrl with ignoreDeprecations 6.0",
        compilerOptions: { baseUrl: "./src", paths: { "@app/*": ["app/*"] }, moduleResolution: "node10", module: "commonjs", ignoreDeprecations: "6.0", rootDir: "." },
        files: { "src/a.ts": 'import { x } from "@app/x";\nimport { z } from "z";\nexport const all = x + z;\n', "src/app/x.ts": "export const x = 1;\n", "src/z.ts": "export const z = 3;\n" },
        native: () => ({ module: 1 }),
        semantic: true,
    },
    {
        name: "paths without baseUrl",
        compilerOptions: { paths: { "@app/*": ["./src/app/*"], "bad/*": ["src/*"] }, module: "esnext", moduleResolution: "bundler" },
        files: { "src/a.ts": 'import { x } from "@app/x";\nexport const all = x;\n', "src/app/x.ts": "export const x = 1;\n" },
        native: dir => ({ paths: { "@app/*": [`${dir}/src/app/*`], "bad/*": ["src/*"] } }),
        ts6: [5090],
        codes: [5090],
        semantic: true,
    },
    {
        name: "rootDirs",
        compilerOptions: { rootDirs: ["src", "generated"], module: "commonjs" },
        files: { "src/a.ts": 'import { g } from "./g";\nexport const v = g;\n', "generated/g.ts": "export const g = 1;\n" },
        native: dir => ({ rootDirs: [`${dir}/src`, `${dir}/generated`] }),
        semantic: true,
    },
    {
        name: "ES5 target with downlevelIteration",
        compilerOptions: { target: "es5", lib: ["es5", "dom"], downlevelIteration: true, module: "commonjs" },
        native: () => ({ target: 1, module: 1, lib: ["lib.es5.d.ts", "lib.dom.d.ts"] }),
        ts6: [5107, 5101],
        codes: [5108, 5101],
    },
    { name: "ES3 target", compilerOptions: { target: "es3", module: "commonjs" }, native: () => ({ target: 12, module: 1 }), ts6: [5108], codes: [5108] },
    {
        name: "AMD with outFile",
        compilerOptions: { module: "amd", outFile: "./out.js" },
        native: dir => ({ module: 2, outFile: `${dir}/out.js` }),
        ts6: [5107, 5011, 5101],
        codes: [5108, 5011, 5102],
    },
    { name: "UMD", compilerOptions: { module: "umd" }, native: () => ({ module: 3 }), ts6: [5107], codes: [5108] },
    { name: "System", compilerOptions: { module: "system" }, native: () => ({ module: 4 }), ts6: [5107], codes: [5108] },
    { name: "module none", compilerOptions: { module: "none" }, files: { "src/a.ts": scriptSource }, native: () => ({ target: 12 }), ts6: [5107], codes: [5108], emit: ["src/a.js"] },
    { name: "node resolution", compilerOptions: { moduleResolution: "node", module: "commonjs" }, native: () => ({ module: 1 }), ts6: [5107], codes: [5107] },
    { name: "node10 resolution with ignoreDeprecations", compilerOptions: { moduleResolution: "node10", module: "commonjs", ignoreDeprecations: "6.0" }, native: () => ({ module: 1 }) },
    { name: "classic resolution", compilerOptions: { moduleResolution: "classic", module: "commonjs" }, native: () => ({ moduleResolution: 1 }), ts6: [5107], codes: [5108] },
    {
        name: "lib list with renamed esnext libs",
        compilerOptions: { lib: ["es2020", "dom", "esnext.array", "esnext.collection", "esnext.error", "esnext.typedarrays", "esnext.disposable", "decorators.legacy"], target: "es2020" },
        native: () => ({ target: 7, lib: ["lib.es2020.d.ts", "lib.dom.d.ts", "esnext.array", "esnext.collection", "esnext.error", "esnext.typedarrays", "lib.esnext.disposable.d.ts", "lib.decorators.legacy.d.ts"] }),
        libFiles: ["lib.es2026.array.d.ts", "lib.es2026.collection.d.ts", "lib.es2026.error.d.ts", "lib.es2026.typedarrays.d.ts", "lib.esnext.disposable.d.ts", "lib.decorators.legacy.d.ts", "lib.dom.d.ts"],
    },
    ...["preserve", "react", "react-native", "react-jsx", "react-jsxdev"].map((jsx, index) => ({
        name: `jsx ${jsx}`,
        compilerOptions: { jsx, module: "esnext" },
        files: { "src/a.tsx": "declare const React: any;\nexport const el = <div className=\"x\">hi</div>;\ndeclare global { namespace JSX { interface IntrinsicElements { div: any } } }\n" },
        native: () => ({ jsx: index + 1 }),
        emit: [jsx === "preserve" ? "src/a.jsx" : "src/a.js"],
    })),
    { name: "newLine crlf", compilerOptions: { newLine: "crlf", module: "commonjs" }, native: () => ({ newLine: 1 }), emit: ["src/a.js"] },
    { name: "newLine lf", compilerOptions: { newLine: "lf", module: "commonjs" }, native: () => ({ newLine: 2 }), emit: ["src/a.js"] },
    ...[["force", 3], ["legacy", 2], ["auto", 1]].map(([moduleDetection, native]) => ({
        name: `moduleDetection ${moduleDetection}`,
        compilerOptions: { moduleDetection, module: "commonjs" },
        files: { "src/a.ts": scriptSource },
        native: () => ({ moduleDetection: native }),
        emit: ["src/a.js"],
    })),
    {
        name: "types and typeRoots",
        compilerOptions: { types: ["fake"], typeRoots: ["./types"], module: "commonjs" },
        files: { "src/a.ts": "export const v: FakeGlobal = 1;\n", "types/fake/index.d.ts": "declare type FakeGlobal = number;\n" },
        native: dir => ({ types: ["fake"], typeRoots: [`${dir}/types`] }),
        semantic: true,
    },
    {
        name: "rootDir, outDir and declarationDir",
        compilerOptions: { rootDir: "./src", outDir: "./out", declaration: true, declarationDir: "./types-out", module: "commonjs" },
        native: dir => ({ rootDir: `${dir}/src`, outDir: `${dir}/out`, declarationDir: `${dir}/types-out` }),
        emit: ["out/a.js", "types-out/a.d.ts"],
    },
    { name: "sourceMap with inlineSources", compilerOptions: { sourceMap: true, inlineSources: true, module: "commonjs", rootDir: "src", outDir: "out" }, native: () => ({ sourceMap: true, inlineSources: true }), emit: ["out/a.js"] },
    {
        name: "decorators with emitDecoratorMetadata",
        compilerOptions: { experimentalDecorators: true, emitDecoratorMetadata: true, module: "commonjs", target: "es2021" },
        files: { "src/a.ts": "declare function D(): any;\nexport class Dep {}\n@D()\nexport class S {\n    constructor(private d: Dep) {}\n    @D() m(x: string): number { return 1; }\n}\n" },
        native: () => ({ experimentalDecorators: true, emitDecoratorMetadata: true }),
        emit: ["src/a.js"],
    },
    { name: "incremental with tsBuildInfoFile", compilerOptions: { incremental: true, tsBuildInfoFile: "./cache/x.tsbuildinfo", module: "commonjs" }, native: dir => ({ incremental: true, tsBuildInfoFile: `${dir}/cache/x.tsbuildinfo` }) },
    { name: "composite", compilerOptions: { composite: true, module: "commonjs", outDir: "out" }, native: () => ({ composite: true }), emit: ["out/src/a.js", "out/src/a.d.ts"] },
    {
        name: "strict family",
        compilerOptions: { strict: true, strictNullChecks: false, noImplicitAny: true, useUnknownInCatchVariables: false, exactOptionalPropertyTypes: true, noUncheckedIndexedAccess: true, module: "commonjs" },
        files: { "src/a.ts": "export function f(x) { const a: number[] = []; const b: number = a[0]; try {} catch (e) { e.x; } return b; }\n" },
        native: () => ({ strict: true, strictNullChecks: false, exactOptionalPropertyTypes: true }),
        ts6: [5052, 5052],
        codes: [5052, 5052],
        semantic: true,
    },
    {
        name: "isolatedModules",
        compilerOptions: { isolatedModules: true, module: "esnext" },
        files: { "src/a.ts": 'import { T } from "./b";\nexport { T };\n', "src/b.ts": "export type T = number;\n" },
        native: () => ({ isolatedModules: true }),
        semantic: true,
    },
    {
        name: "verbatimModuleSyntax",
        compilerOptions: { verbatimModuleSyntax: true, module: "esnext" },
        files: { "src/a.ts": 'import { T } from "./b";\nexport const x: T = 1;\n', "src/b.ts": "export type T = number;\n" },
        native: () => ({ verbatimModuleSyntax: true }),
        semantic: true,
    },
    {
        name: "allowJs and checkJs",
        compilerOptions: { allowJs: true, checkJs: true, module: "commonjs", outDir: "out", rootDir: "src" },
        files: { "src/a.js": "/** @type {number} */\nconst n = \"s\";\nmodule.exports = { n };\n" },
        native: () => ({ allowJs: true, checkJs: true }),
        semantic: true,
        emit: ["out/a.js"],
    },
    {
        name: "options TS 6.0 still declares but removed",
        compilerOptions: { keyofStringsOnly: true, suppressExcessPropertyErrors: true, suppressImplicitAnyIndexErrors: true, noStrictGenericChecks: true, noImplicitUseStrict: true, charset: "utf8", out: "x.js", importsNotUsedAsValues: "error", preserveValueImports: true, module: "commonjs" },
        native: () => ({ module: 1 }),
        ts6: Array(9).fill(5102),
        codes: Array(9).fill(5102),
    },
    { name: "ignoreDeprecations 6.0", compilerOptions: { ignoreDeprecations: "6.0", esModuleInterop: false, allowSyntheticDefaultImports: false, alwaysStrict: false, downlevelIteration: false, baseUrl: ".", module: "commonjs" }, native: () => ({ module: 1 }) },
    { name: "invalid ignoreDeprecations", compilerOptions: { ignoreDeprecations: "5.5", esModuleInterop: false, module: "commonjs" }, native: () => ({ module: 1 }), ts6: [5103, 5107], codes: [5103, 5107] },
    {
        name: "esModuleInterop, allowSyntheticDefaultImports and alwaysStrict false",
        compilerOptions: { esModuleInterop: false, allowSyntheticDefaultImports: false, alwaysStrict: false, module: "commonjs" },
        native: () => ({ module: 1 }),
        ts6: [5107, 5107, 5107],
        codes: [5107, 5107, 5107],
    },
    { name: "deprecated options in an extended config", compilerOptions: undefined, tsconfig: { extends: "./base.json" }, files: { "src/a.ts": plainSource, "base.json": { compilerOptions: { baseUrl: ".", esModuleInterop: false, types: [] } } }, native: () => ({}), ts6: [5101, 5107], codes: [5101, 5107] },
];

test("tsconfig cases: tsc-rs program options, option diagnostics and emit follow TS 6.0.3", { skip: nativeSkip }, async t => {
    for (const optionCase of optionCases) {
        await t.test(optionCase.name, () => checkOptionCase(optionCase));
    }
});

function checkOptionCase(optionCase) {
    const tsconfig = optionCase.tsconfig ?? { compilerOptions: { types: [], ...optionCase.compilerOptions } };
    const dir = writeProject(optionCase.name.replace(/[^\w]+/g, "-"), { "tsconfig.json": tsconfig, ...(optionCase.files ?? { "src/a.ts": plainSource }) });
    const parsed = parseConfig(ts, path.join(dir, "tsconfig.json"));
    assert.deepEqual(parsed.errors, []);
    const translated = toNativeOptions(ts, parsed.options);
    const program = createNativeProgram(parsed.fileNames, translated.options);
    try {
        const nativeOptions = program.compilerOptions;
        for (const [name, value] of Object.entries(optionCase.native(dir))) {
            assert.deepEqual(nativeOptions[name], value, `tsc-rs option ${name}`);
        }
        const nativeFiles = program.list("getSourceFileNames");
        for (const lib of optionCase.libFiles ?? []) {
            assert.ok(nativeFiles.includes(`bundled:///libs/${lib}`), `lib ${lib}`);
        }
        const host = formatHost(dir);
        const optionDiagnostics = ts.sortAndDeduplicateDiagnostics([
            ...translated.diagnostics,
            ...toTsOptionDiagnostics(ts, parsed.options, program.list("getProgramDiagnostics"), () => undefined),
        ]);
        assert.deepEqual(optionDiagnostics.map(d => d.code), optionCase.codes ?? []);
        if (ts6 === undefined) {
            return;
        }
        const parsed6 = parseConfig(ts6, path.join(dir, "tsconfig.json"));
        const program6 = ts6.createProgram(parsed6.fileNames, parsed6.options, createTs6Host(parsed6.options));
        const diagnostics6 = ts6.sortAndDeduplicateDiagnostics([...program6.getOptionsDiagnostics(), ...program6.getGlobalDiagnostics()]);
        assert.deepEqual(diagnostics6.map(d => d.code), optionCase.ts6 ?? []);
        const sameCodes = diagnostics => diagnostics.filter(d => diagnostics6.some(d6 => d6.code === d.code) && optionDiagnostics.some(o => o.code === d.code));
        assert.equal(ts.formatDiagnostics(sameCodes(optionDiagnostics), host), ts6.formatDiagnostics(sameCodes(diagnostics6), host));
        if (optionCase.semantic) {
            assertSameSemanticDiagnostics(program, program6, host);
        }
        if (optionCase.emit) {
            assertSameEmit(program, program6, dir, optionCase.emit);
        }
    }
    finally {
        program.dispose();
    }
}

const ts6LibFiles = new Map();

// A TS 6.0.3 compiler host that parses each lib file once for all the cases.
function createTs6Host(options) {
    const host = ts6.createCompilerHost(options);
    const libDirectory = ts6.normalizePath(path.dirname(ts6Main));
    const getSourceFile = host.getSourceFile;
    host.getSourceFile = (fileName, languageVersionOrOptions, ...rest) => {
        if (path.dirname(fileName) !== libDirectory) {
            return getSourceFile.call(host, fileName, languageVersionOrOptions, ...rest);
        }
        const key = `${fileName}|${JSON.stringify(languageVersionOrOptions)}`;
        if (!ts6LibFiles.has(key)) {
            ts6LibFiles.set(key, getSourceFile.call(host, fileName, languageVersionOrOptions, ...rest));
        }
        return ts6LibFiles.get(key);
    };
    return host;
}

function assertSameSemanticDiagnostics(program, program6, host) {
    const files = program.list("getSourceFileNames").filter(fileName => !fileName.startsWith("bundled:"));
    const native = files.flatMap(fileName => [...program.list("getSyntacticDiagnostics", { files: [fileName] }), ...program.list("getSemanticDiagnostics", { files: [fileName] })]);
    const sourceFiles = new Map(program6.getSourceFiles().map(file => [file.fileName, file]));
    const diagnostics = ts.sortAndDeduplicateDiagnostics(toTsDiagnostics(ts, native, fileName => sourceFiles.get(fileName)));
    const diagnostics6 = ts6.sortAndDeduplicateDiagnostics(program6.getSourceFiles().flatMap(file => [...program6.getSyntacticDiagnostics(file), ...program6.getSemanticDiagnostics(file)]));
    assert.equal(ts.formatDiagnostics(diagnostics, host), ts6.formatDiagnostics(diagnostics6, host));
}

function assertSameEmit(program, program6, dir, files) {
    const outputs6 = new Map();
    program6.emit(undefined, (fileName, text) => outputs6.set(relativeName(dir, fileName), text));
    const outputs = new Map(program.request("emitToString", {}).outputFiles.map(output => [relativeName(dir, output.fileName), output.text]));
    for (const file of files) {
        assert.ok(outputs6.has(file), `TS 6.0 emits ${file}`);
        assert.equal(outputs.get(file), outputs6.get(file), `emit of ${file}`);
    }
}

test("every TS 6.0 lib loads in tsc-rs under its TS 6.0 file name or lib name", { skip: nativeSkip }, () => {
    const dir = writeProject("all-libs", { "a.ts": "export {};\n" });
    const libFiles = [...new Set(ts.libMap.values())];
    const { options } = toNativeOptions(ts, { lib: libFiles, types: [], noEmit: true });
    const program = createNativeProgram([`${dir}/a.ts`], options);
    try {
        const loaded = new Set(program.list("getSourceFileNames").filter(name => name.startsWith("bundled:///libs/")).map(name => name.slice("bundled:///libs/".length)));
        const missing = libFiles.filter(file => !loaded.has(file) && !loaded.has(file.replace("lib.esnext.", "lib.es2026.")));
        assert.deepEqual(missing, []);
    }
    finally {
        program.dispose();
    }
});

// --- diagnostics.js ---------------------------------------------------------------------------

test("a tsc-rs response becomes a TS 6.0 diagnostic with a message chain and related information", () => {
    const file = ts.createSourceFile("/p/a.ts", "const a: string = 1;\nconst a = 2;\n", ts.ScriptTarget.Latest);
    const getSourceFile = fileName => fileName === "/p/a.ts" ? file : undefined;
    const diagnostic = toTsDiagnostic(ts, {
        fileName: "/p/a.ts", pos: 6, end: 7, code: 2322, category: 1, text: "Type 'number' is not assignable to type 'string'.",
        messageChain: [{ pos: 6, end: 7, code: 2322, category: 1, text: "Inner.", messageChain: [{ pos: 0, end: 0, code: 1, category: 3, text: "Innermost." }] }],
        relatedInformation: [{ fileName: "/p/a.ts", pos: 27, end: 28, code: 6203, category: 3, text: "'a' was also declared here." }, { pos: -1, end: -1, code: 2, category: 3, text: "No file." }],
        reportsUnnecessary: false, source: "",
    }, getSourceFile);
    assert.deepEqual(diagnostic, {
        file, start: 6, length: 1, category: 1, code: 2322,
        messageText: {
            messageText: "Type 'number' is not assignable to type 'string'.", category: 1, code: 2322,
            next: [{ messageText: "Inner.", category: 1, code: 2322, next: [{ messageText: "Innermost.", category: 3, code: 1, next: undefined }] }],
        },
        relatedInformation: [
            { file, start: 27, length: 1, messageText: "'a' was also declared here.", category: 3, code: 6203 },
            { file: undefined, start: undefined, length: undefined, messageText: "No file.", category: 3, code: 2 },
        ],
    });
    assert.equal(ts.flattenDiagnosticMessageText(diagnostic.messageText, "\n"), "Type 'number' is not assignable to type 'string'.\n  Inner.\n    Innermost.");
    const suggestion = toTsDiagnostic(ts, { fileName: "/p/a.ts", pos: 6, end: 7, code: 6133, category: 2, text: "x", reportsUnnecessary: true, reportsDeprecated: true, source: "effect" }, getSourceFile);
    assert.equal(suggestion.reportsUnnecessary, true);
    assert.equal(suggestion.reportsDeprecated, true);
    assert.equal(suggestion.source, "effect");
    const global = toTsDiagnostic(ts, { pos: -1, end: -1, code: 5090, category: 1, text: "Non-relative paths are not allowed. Did you forget a leading './'?" }, getSourceFile);
    assert.deepEqual(global, { file: undefined, start: undefined, length: undefined, messageText: "Non-relative paths are not allowed when 'baseUrl' is not set. Did you forget a leading './'?", category: 1, code: 5090 });
});

const diagnosticsSource = {
    "a.ts": `type A = { x: { y: { z: string } } };
declare const a: { x: { y: { z: number } } };
const t: A = a;
interface Box { inner: { value: number } }
const b: Box = { inner: { value: "x" } };
class C {}
class C {}
declare var dup: number;
declare var dup: string;
function g(n: number) { return n; }
g("s");
const unused = 1;
/** @deprecated use h */
function old() {}
old();
const emoji = "😀é"; const bad: number = emoji;
export {};
`,
    "b.ts": `export function f(o: { p: { q: number } }) { return o; }
f({ p: { q: "no" } });
let r: Promise<number> = Promise.resolve("s");
`,
    "tsconfig.json": { compilerOptions: { strict: true, noUnusedLocals: true, target: "es2022", module: "esnext", types: [], noEmit: true }, files: ["a.ts", "b.ts"] },
};

// The diagnostics tsc-rs's command line reports for a project, as TS 6.0 diagnostics.
function nativeProjectDiagnostics(dir) {
    const program = openNativeProject(path.join(dir, "tsconfig.json"));
    const sourceFiles = new Map();
    const getSourceFile = fileName => {
        if (fileName.startsWith("bundled:")) {
            return undefined;
        }
        if (!sourceFiles.has(fileName)) {
            const file = ts.createSourceFile(fileName, fs.readFileSync(fileName, "utf8"), ts.ScriptTarget.Latest);
            file.path = fileName;
            sourceFiles.set(fileName, file);
        }
        return sourceFiles.get(fileName);
    };
    const files = program.list("getSourceFileNames").filter(fileName => !fileName.startsWith("bundled:"));
    const ofEachFile = method => files.flatMap(fileName => program.list(method, { files: [fileName] }));
    const responses = [...program.list("getConfigFileParsingDiagnostics"), ...program.list("getProgramDiagnostics"), ...ofEachFile("getSyntacticDiagnostics")];
    if (!responses.some(response => response.category === ts.DiagnosticCategory.Error)) {
        responses.push(...program.list("getGlobalDiagnostics"), ...ofEachFile("getSemanticDiagnostics"));
    }
    const suggestions = ofEachFile("getSuggestionDiagnostics");
    const diagnostics = ts.sortAndDeduplicateDiagnostics(toTsDiagnostics(ts, responses, getSourceFile));
    program.dispose();
    return { diagnostics, suggestions: toTsDiagnostics(ts, suggestions, getSourceFile) };
}

// The output of a command line, with "\n" for the system's new line (TS 6.0's tsc writes "\r\n" on
// Windows; the diagnostics it is compared with are formatted with "\n").
function runCli(command, args, cwd) {
    let output;
    try {
        output = execFileSync(command, args, { cwd, encoding: "utf8" });
    }
    catch (error) {
        output = error.stdout;
    }
    return output.replaceAll("\r\n", "\n");
}

// tsc-rs's own command line, in a child process through the addon.
function runTscRs(args, cwd) {
    return runCli(process.execPath, [tscPath, ...args], cwd);
}

test("diagnostics print like tsc-rs's command line with --pretty false", { skip: nativeSkip }, () => {
    for (const [name, files] of [["diagnostics-check", diagnosticsSource], ["diagnostics-global", { "a.ts": "const x: number = 1;\n", "tsconfig.json": { compilerOptions: { noLib: true, types: [], noEmit: true }, files: ["a.ts"] } }]]) {
        const dir = writeProject(name, files);
        const { diagnostics } = nativeProjectDiagnostics(dir);
        assert.ok(diagnostics.length >= 10, name);
        assert.equal(ts.formatDiagnostics(diagnostics, formatHost(dir)), runTscRs(["-p", ".", "--pretty", "false"], dir), name);
    }
});

test("pretty diagnostics print like TS 6.0.3's tsc, and like tsc-rs apart from its related information layout", { skip: nativeSkip || ts6Skip }, () => {
    const dir = writeProject("diagnostics-pretty", diagnosticsSource);
    const { diagnostics } = nativeProjectDiagnostics(dir);
    const host = formatHost(dir);
    // tsc writes each diagnostic with formatDiagnosticsWithColorAndContext and a new line.
    const ours = diagnostics.map(diagnostic => ts.formatDiagnosticsWithColorAndContext([diagnostic], host) + "\n").join("");
    const summary = /\n*(?:\x1b\[[0-9;]*m)*Found \d+ errors?[\s\S]*$/;
    const tsc6 = runCli(process.execPath, [path.join(path.dirname(ts6Main), "tsc.js"), "-p", ".", "--pretty", "true"], dir);
    const trimEnd = text => text.replace(summary, "").replace(/\n+$/, "");
    assert.equal(trimEnd(ours), trimEnd(tsc6));
    // tsc-rs puts a related message on its location line, before the code frame.
    const plain = text => text.replace(/\x1b\[[0-9;]*m/g, "");
    const tsgo = plain(trimEnd(runTscRs(["-p", ".", "--pretty", "true"], dir)));
    const ts6Layout = plain(trimEnd(ours)).replace(/^( {2}\S+:\d+:\d+)\n((?: {4}.*\n)*) {4}(.*)\n/gm, "$1 - $3\n$2");
    assert.equal(ts6Layout, tsgo);
});

test("suggestion diagnostics keep reportsUnnecessary and reportsDeprecated like TS 6.0.3's", { skip: nativeSkip || ts6Skip }, () => {
    const dir = writeProject("diagnostics-suggestions", {
        "a.ts": `import { f, g } from "./b";
const unused = 1;
class Unused {}
export function h(unusedParameter: number) {
    /** @deprecated use k */
    function old() {}
    old();
    let written = 1;
    written = 2;
}
/** @deprecated */
export const gone = 1;
export const use = gone;
`,
        "b.ts": "export function f() {}\nexport function g() {}\n",
        "tsconfig.json": { compilerOptions: { strict: true, target: "es2022", module: "esnext", types: [], noEmit: true }, files: ["a.ts", "b.ts"] },
    });
    const { suggestions } = nativeProjectDiagnostics(dir);
    const parsed = parseConfig(ts6, path.join(dir, "tsconfig.json"));
    const service = ts6.createLanguageService({
        getCompilationSettings: () => parsed.options,
        getScriptFileNames: () => parsed.fileNames,
        getScriptVersion: () => "1",
        getScriptSnapshot: fileName => fs.existsSync(fileName) ? ts6.ScriptSnapshot.fromString(fs.readFileSync(fileName, "utf8")) : undefined,
        getCurrentDirectory: () => dir,
        getDefaultLibFileName: options => ts6.getDefaultLibFilePath(options),
        fileExists: ts6.sys.fileExists,
        readFile: ts6.sys.readFile,
    });
    const checkerCodes = new Set([6133, 6138, 6192, 6196, 6198, 6199, 6205, 6385, 6387]);
    const summarize = diagnostics => diagnostics
        .filter(diagnostic => checkerCodes.has(diagnostic.code))
        .map(diagnostic => [path.basename(diagnostic.file.fileName), diagnostic.start, diagnostic.length, diagnostic.code, diagnostic.category, ts.flattenDiagnosticMessageText(diagnostic.messageText, "\n"), diagnostic.reportsUnnecessary, diagnostic.reportsDeprecated].join("|"))
        .sort();
    const reference = parsed.fileNames.flatMap(fileName => service.getSuggestionDiagnostics(fileName));
    service.dispose();
    assert.ok(suggestions.some(diagnostic => diagnostic.reportsUnnecessary) && suggestions.some(diagnostic => diagnostic.reportsDeprecated));
    assert.deepEqual(summarize(suggestions), summarize(reference));
});
