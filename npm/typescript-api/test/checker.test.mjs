// Tests for src/checker.js and src/objects.js: the TS 6.0 TypeChecker and object model backed by
// tsc-rs. Run after `node scripts/build.mjs`: `node --test test/checker.test.mjs`.
// Most tests are differential: the same small program is checked by TypeScript 6.0.3 itself (with
// `stableTypeOrdering: true`, the TS 7 ordering) and by this package, every node of the fixture
// files is asked the same questions, and the answers must agree except for the TS 7 differences
// listed with each fixture. TSRS_CHECKER_REPORT=1 prints every compared fact that differs.
// TypeScript 6.0.3 is the copy scripts/build.mjs unpacks, and the tools are the packages of
// test/consumers (test/support.mjs).

import { after, test } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import v8 from "node:v8";
import vm from "node:vm";
import { createRequire } from "node:module";
import { pathToFileURL } from "node:url";

import { consumersDir, consumersSkip as consumersMissing, packageDir, stockMain, stockSkip as ts6Skip } from "./support.mjs";

const require = createRequire(import.meta.url);
const ts = require(path.join(packageDir, "lib/typescript.js"));
const ts6 = stockMain === undefined ? undefined : require(stockMain);
const report = process.env.TSRS_CHECKER_REPORT === "1";
const workDir = fs.mkdtempSync(path.join(os.tmpdir(), "tsrs-checker-"));
const totals = { facts: 0, agree: 0 };

after(() => {
    if (report && totals.facts > 0) {
        console.log(`all fixtures: ${totals.agree}/${totals.facts} facts agree (${(100 * totals.agree / totals.facts).toFixed(2)}%)`);
    }
});

function writeFixture(name, files) {
    const dir = path.join(workDir, name);
    for (const [file, text] of Object.entries(files)) {
        fs.mkdirSync(path.dirname(path.join(dir, file)), { recursive: true });
        fs.writeFileSync(path.join(dir, file), text);
    }
    return dir;
}

const baseOptions = { strict: true, target: 11, module: 199, moduleResolution: 99, skipLibCheck: true, noEmit: true };

// The same program from TypeScript 6.0.3 and from this package.
function createPrograms(name, files, options = {}) {
    const dir = writeFixture(name, files);
    const rootNames = Object.keys(files).filter(file => /\.tsx?$/.test(file)).map(file => path.join(dir, file));
    const compilerOptions = { ...baseOptions, ...options };
    const reference = ts6.createProgram(rootNames, { ...compilerOptions, stableTypeOrdering: true });
    const native = ts.createProgram(rootNames, compilerOptions);
    return { dir, rootNames, reference, native };
}

function preorder(api, sourceFile) {
    const nodes = [];
    const visit = node => {
        nodes.push(node);
        api.forEachChild(node, visit);
    };
    visit(sourceFile);
    return nodes;
}

// Answers of one checker, as strings that compare across the two TypeScript copies.
class Describer {
    constructor(api, program, dir) {
        this.api = api;
        this.program = program;
        this.checker = program.getTypeChecker();
        this.dir = dir;
    }

    type(type) {
        return type === undefined ? "none" : this.checker.typeToString(type);
    }

    types(types) {
        return types === undefined ? "none" : `[${types.map(type => this.type(type)).join(", ")}]`;
    }

    location(node) {
        const file = node.getSourceFile();
        const inFixture = file.fileName.startsWith(this.dir);
        const name = path.basename(file.fileName);
        return inFixture ? `${name}:${node.pos}` : name;
    }

    declarations(symbol) {
        return (symbol.declarations ?? []).map(node => `${this.api.SyntaxKind[node.kind]}@${this.location(node)}`).join(",");
    }

    symbol(symbol) {
        if (symbol === undefined) {
            return "none";
        }
        return `${symbolName(symbol)} ${symbol.flags} ${this.declarations(symbol)}`;
    }

    signature(signature) {
        const { checker } = this;
        return [
            checker.signatureToString(signature),
            this.type(signature.getReturnType()),
            signature.getParameters().map(parameter => parameter.name).join(","),
            signature.minArgumentCount,
            signature.declaration === undefined ? "none" : this.api.SyntaxKind[signature.declaration.kind],
            signature.getTypeParameters()?.length ?? 0,
            this.api.displayPartsToString(signature.getDocumentationComment()),
            signature.getJsDocTags().map(tag => tag.name).join(","),
        ].join(" | ");
    }

    print(node) {
        if (node === undefined) {
            return "none";
        }
        const { api } = this;
        return api.createPrinter({ removeComments: true }).printNode(api.EmitHint.Unspecified, node, this.program.getSourceFiles().at(-1));
    }

    documentation(symbol) {
        if (symbol === undefined) {
            return "none";
        }
        const { api, checker } = this;
        const tags = symbol.getJsDocTags(checker).map(tag => `@${tag.name} ${api.displayPartsToString(tag.text)}`);
        return [api.displayPartsToString(symbol.getDocumentationComment(checker)), ...tags].join(" | ");
    }
}

// Every compared fact of a node: [name, describe(describer, node)].
const nodeFacts = [
    ["type", (d, node) => d.type(d.checker.getTypeAtLocation(node)), isTypeQuestion],
    ["type flags", (d, node) => d.checker.getTypeAtLocation(node).flags, isTypeQuestion],
    ["symbol", (d, node) => d.symbol(d.checker.getSymbolAtLocation(node)), isSymbolQuestion],
    ["contextual type", (d, node) => d.type(d.checker.getContextualType(node)), (api, node) => api.isExpression(node) && !api.isIdentifier(node.parent ?? node)],
    ["constant value", (d, node) => String(d.checker.getConstantValue(node)), (api, node) => api.isEnumMember(node) || api.isPropertyAccessExpression(node) || api.isElementAccessExpression(node)],
    ["resolved signature", (d, node) => {
        const signature = d.checker.getResolvedSignature(node);
        return signature === undefined ? "none" : d.signature(signature);
    }, (api, node) => api.isCallExpression(node) || api.isNewExpression(node) || api.isDecorator(node) || api.isTaggedTemplateExpression(node)],
    ["declared signature", (d, node) => {
        const signature = d.checker.getSignatureFromDeclaration(node);
        return signature === undefined ? "none" : d.signature(signature);
    }, (api, node) => api.isFunctionLike(node)],
    ["aliased symbol", (d, node) => {
        const symbol = d.checker.getSymbolAtLocation(node);
        if (symbol === undefined || !(symbol.flags & d.api.SymbolFlags.Alias)) {
            return "none";
        }
        return `${d.symbol(d.checker.getAliasedSymbol(symbol))} / ${d.symbol(d.checker.getImmediateAliasedSymbol(symbol))}`;
    }, (api, node) => api.isIdentifier(node)],
    ["symbol to string", (d, node) => {
        const symbol = d.checker.getSymbolAtLocation(node);
        if (symbol === undefined) {
            return "none";
        }
        return `${d.checker.symbolToString(symbol)} | ${d.checker.symbolToString(symbol, node)} | ${d.checker.getFullyQualifiedName(symbol)}`.replace(/@\d+/g, "@N");
    }, (api, node) => api.isIdentifier(node)],
    ["documentation", (d, node) => d.documentation(d.checker.getSymbolAtLocation(node)), (api, node) => api.isIdentifier(node)],
    ["related symbols", (d, node) => {
        const symbol = d.checker.getSymbolAtLocation(node);
        if (symbol === undefined) {
            return "none";
        }
        const { checker } = d;
        return [
            d.symbol(checker.getExportSymbolOfSymbol(symbol)),
            checker.getMergedSymbol(symbol) === symbol,
            symbol.parent === undefined ? "none" : symbolName(symbol.parent),
            d.type(checker.getTypeOfSymbolAtLocation(symbol, node)),
            checker.isUnknownSymbol(symbol),
            checker.isUndefinedSymbol(symbol),
            checker.isArgumentsSymbol(symbol),
        ].join(" | ");
    }, (api, node) => api.isIdentifier(node)],
];

// Facts of the type of a declaration name or an expression.
const typeFacts = [
    ["properties", (d, type) => d.checker.getPropertiesOfType(type).map(symbol => symbolName(symbol)).join(",")],
    ["call signatures", (d, type) => d.checker.getSignaturesOfType(type, d.api.SignatureKind.Call).map(signature => d.signature(signature)).join(" ; ")],
    ["construct signatures", (d, type) => d.checker.getSignaturesOfType(type, d.api.SignatureKind.Construct).length],
    ["index infos", (d, type) => d.checker.getIndexInfosOfType(type).map(info => `${d.type(info.keyType)}:${d.type(info.type)}:${info.isReadonly}`).join(",")],
    ["apparent type", (d, type) => d.type(d.checker.getApparentType(type))],
    ["awaited type", (d, type) => d.type(d.checker.getAwaitedType(type))],
    ["non-nullable type", (d, type) => d.type(d.checker.getNonNullableType(type))],
    ["widened type", (d, type) => d.type(d.checker.getWidenedType(type))],
    ["base type of literal", (d, type) => d.type(d.checker.getBaseTypeOfLiteralType(type))],
    ["array/tuple", (d, type) => `${d.checker.isArrayType(type)} ${d.checker.isTupleType(type)} ${d.checker.isArrayLikeType(type)}`],
    ["symbol of type", (d, type) => d.symbol(type.getSymbol())],
    ["alias", (d, type) => `${d.symbol(type.aliasSymbol)} ${d.types(type.aliasTypeArguments)}`],
    ["union members", (d, type) => type.isUnionOrIntersection() ? d.types(type.types) : "none"],
    ["literal value", (d, type) => type.isLiteral() ? JSON.stringify(type.value) : "none"],
    ["intrinsic name", (d, type) => String(type.intrinsicName)],
    ["object flags", (d, type) => type.flags & d.api.TypeFlags.Object ? type.objectFlags & publicObjectFlags(d.api) : "none"],
    ["type arguments", (d, type) => type.flags & d.api.TypeFlags.Object && type.objectFlags & d.api.ObjectFlags.Reference ? d.types(d.checker.getTypeArguments(type)) : "none"],
    ["tuple target", (d, type) => {
        if (!d.checker.isTupleType(type)) {
            return "none";
        }
        const target = type.target;
        return `${target.elementFlags} ${target.minLength} ${target.fixedLength} ${target.hasRestElement} ${target.combinedFlags} ${target.readonly}`;
    }],
    ["base types", (d, type) => type.isClassOrInterface() ? d.types(d.checker.getBaseTypes(type)) : "none"],
    ["base constraint", (d, type) => type.flags & d.api.TypeFlags.Instantiable ? d.type(d.checker.getBaseConstraintOfType(type)) : "none"],
    ["string index", (d, type) => `${d.type(type.getStringIndexType())} ${d.type(type.getNumberIndexType())}`],
    ["type node", (d, type) => d.print(d.checker.typeToTypeNode(type, undefined, d.api.NodeBuilderFlags.NoTruncation))],
    ["type node in type alias", (d, type) => d.print(d.checker.typeToTypeNode(type, undefined, d.api.NodeBuilderFlags.InTypeAlias | d.api.NodeBuilderFlags.MultilineObjectLiterals | d.api.NodeBuilderFlags.UseFullyQualifiedType))],
    ["fully qualified", (d, type) => d.checker.typeToString(type, undefined, d.api.TypeFormatFlags.UseFullyQualifiedType | d.api.TypeFormatFlags.NoTruncation)],
    ["formatted", (d, type) => d.checker.typeToString(type, undefined, d.api.TypeFormatFlags.UseTypeOfFunction | d.api.TypeFormatFlags.WriteArrayAsGenericType | d.api.TypeFormatFlags.UseSingleQuotesForStringLiteralType)],
    ["no format flags", (d, type) => d.checker.typeToString(type, undefined, d.api.TypeFormatFlags.None)],
    ["signature declarations", (d, type) => d.checker.getSignaturesOfType(type, d.api.SignatureKind.Call).map(signature => [
        d.print(d.checker.signatureToSignatureDeclaration(signature, d.api.SyntaxKind.MethodDeclaration, undefined, d.api.NodeBuilderFlags.NoTruncation)),
        d.print(d.checker.signatureToSignatureDeclaration(signature, d.api.SyntaxKind.FunctionType, undefined, undefined)),
        d.checker.signatureToString(signature, undefined, d.api.TypeFormatFlags.WriteArrowStyleSignature),
        d.checker.signatureToString(signature, undefined, undefined, d.api.SignatureKind.Construct),
    ].join(" / ")).join(" ; ")],
    ["type predicates", (d, type) => d.checker.getSignaturesOfType(type, d.api.SignatureKind.Call).map(signature => {
        const predicate = d.checker.getTypePredicateOfSignature(signature);
        return predicate === undefined ? "none" : `${predicate.kind} ${predicate.parameterName} ${predicate.parameterIndex} ${d.type(predicate.type)} ${d.checker.typePredicateToString(predicate)}`;
    }).join(" ; ")],
    ["index signature declarations", (d, type) => d.checker.getIndexInfosOfType(type).map(info => d.print(d.checker.indexInfoToIndexSignatureDeclaration(info, undefined, undefined))).join(" ; ")],
    ["type parameter declarations", (d, type) => (type.isClassOrInterface() ? type.localTypeParameters ?? [] : []).map(parameter => d.print(d.checker.typeParameterToDeclaration(parameter, undefined, undefined))).join(", ")],
];

// Symbol ids are opaque and differ between the two compilers (`__@iterator@11`).
function symbolName(symbol) {
    return symbol.name.replace(/@\d+$/, "@N");
}

function publicObjectFlags(api) {
    const { ObjectFlags } = api;
    return ObjectFlags.Class | ObjectFlags.Interface | ObjectFlags.Reference | ObjectFlags.Tuple | ObjectFlags.Anonymous | ObjectFlags.Mapped | ObjectFlags.Instantiated | ObjectFlags.ObjectLiteral | ObjectFlags.EvolvingArray | ObjectFlags.ReverseMapped | ObjectFlags.JsxAttributes | ObjectFlags.JSLiteral | ObjectFlags.FreshLiteral | ObjectFlags.ArrayLiteral;
}

function isTypeQuestion(api, node) {
    return api.isIdentifier(node) || api.isExpression(node) || api.isTypeNode(node) || api.isDeclaration(node);
}

function isSymbolQuestion(api, node) {
    return api.isIdentifier(node) || api.isPrivateIdentifier(node) || api.isStringLiteral(node) || api.isPropertyAccessExpression(node) || api.isQualifiedName(node);
}

function isTypeFactsQuestion(api, node) {
    return api.isIdentifier(node) && api.isDeclaration(node.parent) && node.parent.name === node || api.isCallExpression(node) || api.isAwaitExpression(node);
}

function guard(compute) {
    try {
        return String(compute());
    }
    catch (error) {
        return `throws ${error.message.split("\n")[0]}`;
    }
}

// Asks both checkers about every node of the fixture files; returns the facts that differ.
function compareCheckers({ dir, rootNames, reference, native }) {
    const ours = new Describer(ts, native, dir);
    const theirs = new Describer(ts6, reference, dir);
    const differences = [];
    let facts = 0;
    for (const fileName of rootNames) {
        const referenceNodes = preorder(ts6, reference.getSourceFile(fileName));
        const nativeNodes = preorder(ts, native.getSourceFile(fileName));
        assert.equal(nativeNodes.length, referenceNodes.length, `${fileName}: both parsers make the same tree`);
        referenceNodes.forEach((referenceNode, index) => {
            const nativeNode = nativeNodes[index];
            const where = `${path.basename(fileName)}:${referenceNode.pos} ${ts6.SyntaxKind[referenceNode.kind]} ${referenceNode.getText().slice(0, 40).replace(/\s+/g, " ")}`;
            const compare = (name, describe) => {
                facts++;
                const expected = guard(() => describe(theirs, referenceNode));
                const actual = guard(() => describe(ours, nativeNode));
                if (expected !== actual) {
                    differences.push({ where, name, expected, actual });
                }
            };
            for (const [name, describe, applies] of nodeFacts) {
                if (applies(ts6, referenceNode)) {
                    compare(name, describe);
                }
            }
            if (isTypeFactsQuestion(ts6, referenceNode)) {
                for (const [name, describe] of typeFacts) {
                    compare(name, (d, node) => describe(d, d.checker.getTypeAtLocation(node)));
                }
            }
        });
    }
    totals.facts += facts;
    totals.agree += facts - differences.length;
    return { facts, differences };
}

// The facts that differ must be exactly the listed TS 7 differences (a `where` substring, the fact
// name and the reason).
function assertAgreement(result, expected, label) {
    const unexplained = result.differences.filter(difference => !expected.some(([where, name]) => difference.where.includes(where) && difference.name === name));
    const unused = expected.filter(([where, name]) => !result.differences.some(difference => difference.where.includes(where) && difference.name === name));
    if (report) {
        console.log(`${label}: ${result.facts - result.differences.length}/${result.facts} facts agree`);
        for (const difference of result.differences) {
            console.log(`  ${difference.where} [${difference.name}]\n    ts6: ${difference.expected}\n    ours: ${difference.actual}`);
        }
    }
    assert.deepEqual(unexplained, [], `${label}: facts that differ from TypeScript 6.0.3`);
    assert.deepEqual(unused, [], `${label}: listed differences that no longer occur`);
}

// TS 7 differences that several fixtures show.
const bigintOrder = "TS 7 orders bigint literals by value (-2n before 1n); TS 6.0 with stableTypeOrdering does not";
const unionPropertyOrder = "TS 7 lists the common properties of a union of primitives in another order";
const libText = "tsc-rs bundles the TS 7 lib files, whose JSDoc text differs";

const basicFiles = {
    "b.ts": `export enum E { One = 1, Two = 2, Three = "three" }
export const enum K { A = 1, B = A << 2, C = "c" }
export class B { base = true; protected secret = 1; }
export function makeB(): B { return new B(); }
`,
    "a.ts": `import { B, E, K, makeB } from "./b";
export interface I { x: number; y?: string; [k: string]: unknown }
export class C<T> extends B implements I {
    [k: string]: unknown;
    x = 1;
    /** doc for m */
    m(p: T, q = "s"): Promise<T[]> { return Promise.resolve([p]); }
    static s: readonly [a: string, b?: number, ...rest: boolean[]] = ["a"];
}
export type U = "a" | "b" | 1n | -2n | E;
export const big = 10n;
export const tpl: \`x\${number}\` = "x1";
const e = E.Two;
const k = K.B;
const c = new C<string>();
const arr = [1, 2, 3].map(n => n * 2);
async function f(): Promise<number> { return 1; }
f();
const v = makeB();
let u: U | undefined;
type M = { readonly [P in keyof I]?: I[P] };
type Cond<X> = X extends string ? "s" : "n";
declare const cond: Cond<number>;
function isString(value: unknown): value is string { return typeof value === "string"; }
function overload(a: string): string;
function overload(a: number): number;
function overload(a: any) { return a; }
const tuple: [number, string?] = [1];
const obj = { a: 1, b: "two", nested: { c: true } };
let maybe: string | null = Math.random() > 0.5 ? "s" : null;
`,
};

test("checks a program with classes, generics, enums, tuples and literal types like TypeScript 6.0", { skip: ts6Skip }, () => {
    const result = compareCheckers(createPrograms("basic", basicFiles));
    assertAgreement(result, [
        ["a.ts:363 Identifier U", "union members", bigintOrder],
        ["a.ts:363 Identifier U", "type node in type alias", bigintOrder],
        ["a.ts:632 Identifier u", "union members", bigintOrder],
        ["a.ts:363 Identifier U", "properties", unionPropertyOrder],
        ["a.ts:672 Identifier P", "properties", unionPropertyOrder],
    ], "basic");
});

const moduleFiles = {
    "lib.ts": `export default class Widget { size = 1; }
export function helper(x: number): string { return String(x); }
export const value = 42;
export namespace NS { export const inner = "i"; export interface Shape { kind: string } }
export type Pair<A, B> = [A, B];
`,
    "reexport.ts": `export { helper as renamed, value } from "./lib";
export * from "./lib";
export * as all from "./lib";
`,
    "eq.ts": `const config = { port: 1 };
export = config;
`,
    "main.ts": `import Widget, { helper, NS } from "./lib";
import * as lib from "./lib";
import { renamed, all } from "./reexport";
import type { Pair } from "./lib";
import config = require("./eq");
const w = new Widget();
const h = helper(1);
const r = renamed(2);
const s: NS.Shape = { kind: "k" };
const p: Pair<string, number> = ["a", 1];
const port = config.port;
const v = lib.value + all.value + NS.inner.length;
export { w as widget, h, r, s, p, port, v };
`,
};

test("resolves aliases, namespaces and re-exports across modules like TypeScript 6.0", { skip: ts6Skip }, () => {
    const result = compareCheckers(createPrograms("modules", moduleFiles));
    const immediateTarget = "tsc-rs's getImmediateAliasedSymbol resolves a namespace import to the merged module symbol and an `import =` of an `export =` module to the `export=` alias";
    const singleQuotes = "tsc-rs writes the import type specifier with single quotes under UseSingleQuotesForStringLiteralType";
    assertAgreement(result, [
        ["main.ts:55 Identifier lib", "aliased symbol", immediateTarget],
        ["main.ts:364 Identifier lib", "aliased symbol", immediateTarget],
        ["main.ts:158 Identifier config", "aliased symbol", immediateTarget],
        ["main.ts:341 Identifier config", "aliased symbol", immediateTarget],
        ["main.ts:55 Identifier lib", "formatted", singleQuotes],
        ["main.ts:91 Identifier all", "formatted", singleQuotes],
        ["reexport.ts:84 Identifier all", "formatted", singleQuotes],
        ["main.ts:123 ImportClause", "type", "TypeScript 6.0 throws in getTypeAtLocation on a type-only import clause; tsc-rs answers the error type"],
        ["main.ts:123 ImportClause", "type flags", "TypeScript 6.0 throws in getTypeAtLocation on a type-only import clause; tsc-rs answers the error type"],
    ], "modules");
});

const contextualFiles = {
    "contextual.ts": `interface Options { name: string; onDone(result: number): void; flags?: { verbose: boolean } }
declare function run(options: Options): void;
declare function map<T, U>(items: T[], f: (item: T, index: number) => U): U[];
run({ name: "x", onDone(result) { result.toFixed(); }, flags: { verbose: true } });
const lengths = map(["a", "bb"], (s, i) => s.length + i);
const handler: (e: { x: number }) => void = e => e.x;
const pair: [string, (n: number) => string] = ["a", n => n.toString()];
const fromReturn = (): Options => ({ name: "y", onDone: r => r });
class K { m: (a: string) => number = a => a.length; }
const promise: Promise<string> = new Promise(resolve => resolve("v"));
const spread = Math.max(...[1, 2, 3]);
const tagged = String.raw\`a\${1}b\`;
export { lengths, handler, pair, fromReturn, K, promise, spread, tagged };
`,
};

test("answers contextual types and resolved signatures like TypeScript 6.0", { skip: ts6Skip }, () => {
    const result = compareCheckers(createPrograms("contextual", contextualFiles));
    const functionSymbol = "TypeScript 6.0 gives the widened type of an arrow function with a return type a transient copy of its symbol, so UseTypeOfFunction writes its signature; tsc-rs keeps the function's symbol and writes `typeof fromReturn`";
    assertAgreement(result, [
        ["contextual.ts:261 Identifier toFixed", "documentation", libText],
        ["contextual.ts:493 Identifier fromReturn", "symbol of type", functionSymbol],
        ["contextual.ts:786 Identifier fromReturn", "symbol of type", functionSymbol],
        ["contextual.ts:493 Identifier fromReturn", "formatted", functionSymbol],
        ["contextual.ts:786 Identifier fromReturn", "formatted", functionSymbol],
    ], "contextual");
});

const classFiles = {
    "classes.ts": `export abstract class Base<T> {
    protected abstract make(): T;
    private hidden = 1;
    #secret = "s";
    static count = 0;
    readonly id: string = "id";
    get size(): number { return this.#secret.length + this.hidden; }
    set size(value: number) { this.hidden = value; }
    self(): this { return this; }
    static create<U extends Base<string>>(this: new () => U): U { return new this(); }
}
export class Derived extends Base<string> {
    constructor(public name: string, private readonly age?: number) { super(); }
    protected make(): string { return this.name + (this.age ?? 0); }
    override self(): this { return this; }
}
export interface Callable { (x: number): string; new (x: string): Callable; readonly [index: number]: boolean; prop?: () => void }
declare const callable: Callable;
const fromCall = callable(1);
const fromIndex = callable[0];
const derived = new Derived("n", 2);
const size = derived.size;
const made = Derived.create();
export { fromCall, fromIndex, size, made };
`,
};

test("describes classes, accessors, private names, this types and call signatures like TypeScript 6.0", { skip: ts6Skip }, () => {
    const result = compareCheckers(createPrograms("classes", classFiles));
    assertAgreement(result, [], "classes");
});

const typeFiles = {
    "types.ts": `export type Mapped<T> = { -readonly [K in keyof T as \`get\${Capitalize<string & K>}\`]-?: () => T[K] };
export type Unwrap<T> = T extends Promise<infer U> ? U : T;
export type Keys = keyof { a: 1; b: 2 };
export type Access = { a: string; b: number }["a" | "b"];
export interface Shape { kind: "circle" | "square"; size: number }
export type Getters = Mapped<Shape>;
export const sym: unique symbol = Symbol("s");
export type WithSym = { [sym]: number };
declare function isShape(value: unknown): value is Shape;
declare function assertShape(value: unknown): asserts value is Shape;
declare function assertDefined(value: unknown): asserts value;
export const enum Flag { None = 0, A = 1 << 0, B = 1 << 1, AB = A | B, Name = "name".length }
const flag = Flag.AB;
const both: Shape & { extra: true } = { kind: "circle", size: 1, extra: true };
const never: never = undefined as never;
const unknownValue: unknown = 1;
const objectValue: object = {};
const fn: typeof isShape = isShape;
const record: Record<"x" | "y", Partial<Shape>> = { x: {}, y: { size: 1 } };
const unwrapped: Unwrap<Promise<number>> = 1;
const readonlyArray: readonly string[] = [];
const nested: Array<Array<number>> = [[1]];
function generic<T extends { length: number }, U = T[]>(value: T, other?: U): [T, U | undefined] { return [value, other]; }
const g = generic("abc");
let narrowed: string | number = Math.random() ? "a" : 1;
if (typeof narrowed === "string") { narrowed.toUpperCase(); }
export { flag, both, never, unknownValue, objectValue, fn, record, unwrapped, readonlyArray, nested, g, isShape, assertShape, assertDefined };
`,
};

test("describes mapped, conditional, template literal, indexed access and predicate types like TypeScript 6.0", { skip: ts6Skip }, () => {
    const result = compareCheckers(createPrograms("types", typeFiles));
    assertAgreement(result, [
        ["types.ts:37 Identifier K", "properties", unionPropertyOrder],
        ["types.ts:214 Identifier Access", "properties", unionPropertyOrder],
        ["types.ts:1347 Identifier narrowed", "properties", unionPropertyOrder],
    ], "types");
});

const docFiles = {
    "docs.ts": `/**
 * A base class.
 * @deprecated Use {@link Next} instead.
 */
export class Old {
    /**
     * Runs the thing.
     * @param times How many times.
     * @returns The result.
     */
    run(times: number): string { return String(times); }
    /** The label. */
    label = "old";
    /** Reads the width. */
    get width(): number { return 1; }
    /** Writes the width. */
    set width(value: number) { void value; }
}
/** The next one. @see Old */
export class Next extends Old {
    run(times: number): string { return super.run(times); }
}
const next = new Next();
const result = next.run(2);
export { next, result };
`,
};

test("reads documentation comments and JSDoc tags through the TS 6.0 symbol methods", { skip: ts6Skip }, () => {
    const result = compareCheckers(createPrograms("docs", docFiles));
    assertAgreement(result, [], "docs");
});

// Tools from other projects, run unmodified against both checkers.
const consumersSkip = ts6Skip || consumersMissing;
const consumersRequire = createRequire(path.join(consumersDir, "noop.js"));

const lintRules = [
    "await-thenable", "no-base-to-string", "no-confusing-void-expression", "no-floating-promises", "no-for-in-array",
    "no-implied-eval", "no-misused-promises", "no-redundant-type-constituents", "no-unnecessary-condition",
    "no-unnecessary-type-assertion", "no-unsafe-argument", "no-unsafe-assignment", "no-unsafe-call",
    "no-unsafe-enum-comparison", "no-unsafe-member-access", "no-unsafe-return", "prefer-nullish-coalescing",
    "prefer-optional-chain", "prefer-readonly", "prefer-reduce-type-parameter", "prefer-string-starts-ends-with",
    "require-await", "restrict-plus-operands", "restrict-template-expressions", "return-await",
    "switch-exhaustiveness-check", "unbound-method", "no-unnecessary-type-arguments", "prefer-promise-reject-errors",
    "only-throw-error", "no-array-delete", "no-deprecated", "no-misused-spread", "no-unnecessary-boolean-literal-compare",
];

const lintSource = `declare function fetchData(): Promise<string>;
declare const anyValue: any;
declare function takesString(s: string): void;
enum Fruit { Apple = "apple", Pear = "pear" }
type Shape = { kind: "circle"; r: number } | { kind: "square"; size: number };
/** @deprecated use other */
declare function oldApi(): void;
class Counter {
    private count = 0;
    private readonly step = 1;
    increment(): void { this.count += this.step; }
    log() { return this.count; }
}
async function main(shape: Shape, fruit: Fruit, maybe?: string | null): Promise<string> {
    fetchData();
    const x = anyValue;
    takesString(anyValue);
    const s = \`\${1} \${anyValue} \${[1, 2]} \${{ a: 1 }}\`;
    await 1;
    await fetchData();
    const list: any[] = [];
    const y: string[] = list;
    anyValue.foo.bar;
    anyValue();
    for (const key in [1, 2]) { void key; }
    if (fruit === "apple") { void 0; }
    switch (shape.kind) { case "circle": break; }
    const value = maybe !== null && maybe !== undefined ? maybe : "fallback";
    const len = maybe && maybe.length;
    const ok = (x as string) as string;
    const counter = new Counter();
    const unbound = counter.log;
    [1, 2].reduce((acc, n) => acc + n, 0 as number);
    const str = "abc";
    str.indexOf("a") === 0;
    if (true === (len === 3)) { void 0; }
    oldApi();
    const arr = [1, 2];
    delete arr[0];
    const spread = { ...fetchData() };
    void x; void s; void y; void value; void ok; void unbound; void spread;
    if (Math.random()) { throw "error"; }
    return Promise.reject("no");
}
function sum(a: number, b: string) { return a + b; }
main({ kind: "circle", r: 1 }, Fruit.Apple).then(() => {});
[1].forEach(async () => { await fetchData(); });
class Holder { private never = 2; read() { return this.never; } }
async function noAwait() { return 1; }
async function inTry(): Promise<string> { try { return fetchData(); } catch { return ""; } }
const always = "x";
if (always) { void 0; }
const voided = takesString("a");
type Redundant = string | "a";
setTimeout("code", 1);
function withDefault<T = number>(t?: T) { return t; }
withDefault<number>();
const added = [1] + 2;
const maybeNumber: number | undefined = Math.random() > 0.5 ? 1 : undefined;
const plus = maybeNumber + 1;
void sum; void Holder; void noAwait; void inTry; void voided; void added; void plus;
export type { Redundant };
`;

function lint(api, program, fileName) {
    const { Linter } = consumersRequire("eslint");
    const parser = consumersRequire("@typescript-eslint/parser");
    const plugin = consumersRequire("@typescript-eslint/eslint-plugin");
    const config = [{
        files: ["**/*.ts"],
        languageOptions: { parser, parserOptions: { programs: [program] } },
        plugins: { "@typescript-eslint": plugin },
        rules: Object.fromEntries(lintRules.map(rule => [`@typescript-eslint/${rule}`, "error"])),
    }];
    const linter = new Linter({ configType: "flat", cwd: path.dirname(fileName) });
    return linter.verify(fs.readFileSync(fileName, "utf8"), config, { filename: fileName }).map(message => `${message.line}:${message.column} ${message.ruleId} ${message.message}`);
}

test("typescript-eslint's type-aware rules report the same problems through this checker", { skip: consumersSkip }, () => {
    const { rootNames, reference, native } = createPrograms("lint", { "lint.ts": lintSource });
    const expected = lint(ts6, reference, rootNames[0]);
    const actual = lint(ts, native, rootNames[0]);
    if (report) {
        console.log(`lint: ${expected.length} reports from ${new Set(expected.map(line => line.split(" ")[1])).size} rules\n  ${expected.join("\n  ")}`);
    }
    assert.ok(expected.length > 20, `the fixture triggers the rules (${expected.length} reports)`);
    assert.deepEqual(actual, expected);
});

const nestFiles = {
    "src/owner.dto.ts": `export class OwnerDto {
    firstName: string;
    lastName?: string;
    pets: number;
}
`,
    "src/cat.dto.ts": `import { OwnerDto } from "./owner.dto";
export enum Color { Black = "black", White = "white" }
export enum Size { Small = 1, Large = 2 }
export class CreateCatDto {
    name: string;
    age?: number;
    color: Color;
    sizes: Size[];
    tags: string[];
    owner: OwnerDto;
    friends?: OwnerDto[];
    born: Date;
    alive: boolean = true;
    readonly id: number = 1;
    nickname: string | null;
    metadata: Record<string, unknown>;
    /** The weight in kg */
    weight: number;
    status: "new" | "old";
    matrix: number[][];
}
`,
    "src/cats.controller.ts": `import { Body, Controller, Get, Post, Param, Query } from "@nestjs/common";
import { CreateCatDto, Color } from "./cat.dto";
import { OwnerDto } from "./owner.dto";
@Controller("cats")
export class CatsController {
    @Post()
    async create(@Body() dto: CreateCatDto): Promise<CreateCatDto> { return dto; }
    @Get()
    findAll(@Query("limit") limit?: number): Promise<CreateCatDto[]> { return Promise.resolve([]); }
    @Get(":id")
    findOne(@Param("id") id: string): OwnerDto { return new OwnerDto(); }
    @Get("color")
    color(): Color { return Color.Black; }
    @Get("owners")
    owners(): Promise<Array<OwnerDto>> { return Promise.resolve([]); }
}
`,
    "src/cat.model.ts": `import { Field, ObjectType, Int } from "@nestjs/graphql";
import { Color } from "./cat.dto";
@ObjectType()
export class CatModel {
    /** The cat's name */
    name: string;
    age?: number;
    color: Color;
    tags: string[];
    @Field(() => Int)
    lives: number;
    friends: CatModel[];
    /** @deprecated use name */
    title: string;
}
`,
};

const nestOptions = { strict: false, strictPropertyInitialization: false, target: 9, module: 1, moduleResolution: 100, experimentalDecorators: true, emitDecoratorMetadata: true, types: [] };

// Runs a NestJS CLI plugin's `before` transformer over every source file and prints the result.
function runNestPlugin(api, program, before, options, rootNames) {
    const factory = before(options, program);
    return rootNames.map(fileName => {
        const result = ts6.transform(program.getSourceFile(fileName), [factory], program.getCompilerOptions());
        return `// ${path.basename(fileName)}\n${ts6.createPrinter().printFile(result.transformed[0])}`;
    }).join("\n");
}

function nestPrograms(name) {
    const dir = writeFixture(name, nestFiles);
    fs.symlinkSync(consumersDir, path.join(dir, "node_modules"), "junction");
    const rootNames = Object.keys(nestFiles).map(file => path.join(dir, file));
    const options = { ...nestOptions, outDir: path.join(dir, "dist"), rootDir: path.join(dir, "src") };
    return {
        rootNames,
        reference: ts6.createProgram(rootNames, { ...options, stableTypeOrdering: true }),
        native: ts.createProgram(rootNames, options),
    };
}

test("the NestJS swagger plugin writes the same OpenAPI metadata through this checker", { skip: consumersSkip }, async () => {
    const { before } = await import(pathToFileURL(path.join(consumersDir, "@nestjs/swagger/dist/plugin/compiler-plugin.js")).href);
    const { rootNames, reference, native } = nestPrograms("swagger");
    const options = { introspectComments: true, classValidatorShim: true };
    const expected = runNestPlugin(ts6, reference, before, options, rootNames);
    const actual = runNestPlugin(ts, native, before, options, rootNames);
    assert.match(expected, /_OPENAPI_METADATA_FACTORY/);
    assert.match(expected, /openapi\.ApiResponse/);
    assert.equal(actual, expected);
});

test("the NestJS graphql plugin writes the same field metadata through this checker", { skip: consumersSkip }, async () => {
    const { before } = await import(pathToFileURL(path.join(consumersDir, "@nestjs/graphql/dist/plugin/compiler-plugin.js")).href);
    const { rootNames, reference, native } = nestPrograms("graphql");
    const options = { introspectComments: true };
    const expected = runNestPlugin(ts6, reference, before, options, rootNames);
    const actual = runNestPlugin(ts, native, before, options, rootNames);
    assert.match(expected, /_GRAPHQL_METADATA_FACTORY/);
    assert.equal(actual, expected);
});

test("typeToString with the swagger plugin's flags writes import types like TypeScript 6.0", { skip: consumersSkip }, () => {
    const { reference, native, rootNames } = nestPrograms("swagger-strings");
    const flags = ts6.TypeFormatFlags.UseFullyQualifiedType | ts6.TypeFormatFlags.NoTruncation;
    const controller = rootNames.find(file => file.endsWith("cats.controller.ts"));
    const describe = (api, program) => {
        const checker = program.getTypeChecker();
        const results = [];
        const visit = node => {
            if (api.isMethodDeclaration(node)) {
                const signature = checker.getSignatureFromDeclaration(node);
                results.push(checker.typeToString(checker.getReturnTypeOfSignature(signature), undefined, flags));
                for (const parameter of node.parameters) {
                    results.push(checker.typeToString(checker.getTypeAtLocation(parameter), undefined, flags));
                }
            }
            api.forEachChild(node, visit);
        };
        visit(program.getSourceFile(controller));
        return results;
    };
    const expected = describe(ts6, reference);
    assert.ok(expected.some(text => /^Promise<import\(".*\/src\/cat\.dto"\)\.CreateCatDto>$/.test(text)), expected.join("\n"));
    assert.deepEqual(describe(ts, native), expected);
});

// TS 7 difference: without an enclosing declaration TS 6.0 adds `{ with: { "resolution-mode": "import" } }`
// to an import type of an ES module; tsc-rs's node builder adds it only when there is a context file
// (tsc/internal/checker/nodebuilderimpl.go:673). With an enclosing declaration both agree.
test("typeToString writes import types of ES modules like TypeScript 6.0 except the resolution-mode attribute", { skip: ts6Skip }, () => {
    const files = {
        "package.json": JSON.stringify({ type: "module" }),
        "entity.ts": "export class Cat { name = \"c\"; }\nexport interface Owner { cats: Cat[] }\n",
        "use.cts": "import type { Cat, Owner } from \"./entity.js\";\nexport declare function get(): Promise<Cat>;\nexport declare const owner: Owner | undefined;\n",
        "main.ts": "import { Cat } from \"./entity.js\";\nexport async function load(): Promise<Cat[]> { return [new Cat()]; }\n",
    };
    const dir = writeFixture("esm-strings", files);
    const rootNames = ["entity.ts", "use.cts", "main.ts"].map(file => path.join(dir, file));
    const options = { ...baseOptions, module: 199, moduleResolution: 99 };
    const flags = ts6.TypeFormatFlags.UseFullyQualifiedType | ts6.TypeFormatFlags.NoTruncation;
    const describe = (api, program) => {
        const checker = program.getTypeChecker();
        const results = [];
        for (const fileName of rootNames.slice(1)) {
            const visit = node => {
                if ((api.isFunctionDeclaration(node) || api.isVariableDeclaration(node)) && node.name !== undefined) {
                    const type = checker.getTypeAtLocation(node.name);
                    results.push(checker.typeToString(type, undefined, flags), checker.typeToString(type, node, flags));
                }
                api.forEachChild(node, visit);
            };
            visit(program.getSourceFile(fileName));
        }
        return results;
    };
    const expected = describe(ts6, ts6.createProgram(rootNames, { ...options, stableTypeOrdering: true }));
    assert.ok(expected.some(text => text.includes("resolution-mode")), expected.join("\n"));
    const withoutAttribute = expected.map(text => text.replaceAll(', { with: { "resolution-mode": "import" } }', ""));
    assert.deepEqual(describe(ts, ts.createProgram(rootNames, options)), withoutAttribute);
});

// The object model

const modelFiles = {
    "model.ts": `export interface Box<T> { value: T; map<U>(f: (value: T) => U): Box<U> }
export class Impl<T> implements Box<T> {
    constructor(public value: T) {}
    map<U>(f: (value: T) => U): Box<U> { return new Impl(f(this.value)); }
    static make(): Impl<number> { return new Impl(1); }
}
export type Cond<T> = T extends string ? { s: T } : { n: T };
export type Mapped<T> = { readonly [K in keyof T]: T[K] };
export const key = Symbol("key");
export const withKey = { [key]: 1, plain: 2 };
export const big = -12345678901234567890n;
export const box: Box<string> = new Impl("v");
export const tuple: [first: number, second?: string, ...rest: boolean[]] = [1];
export const flag = true;
export const nothing = undefined;
declare function check<T>(value: T): Cond<T>;
export const checked = check(1);
export const mapped: Mapped<{ a: number }> = { a: 1 };
export const item = box.map(value => value.length);
`,
};

function modelProgram(name = "model") {
    const dir = writeFixture(name, modelFiles);
    const fileName = path.join(dir, "model.ts");
    const program = ts.createProgram([fileName], baseOptions);
    const sourceFile = program.getSourceFile(fileName);
    const checker = program.getTypeChecker();
    const declaration = name => {
        let found;
        const visit = node => {
            if (found === undefined && (ts.isVariableDeclaration(node) || ts.isClassDeclaration(node) || ts.isInterfaceDeclaration(node) || ts.isTypeAliasDeclaration(node)) && node.name?.text === name) {
                found = node;
            }
            ts.forEachChild(node, visit);
        };
        visit(sourceFile);
        return found;
    };
    const typeOf = name => checker.getTypeAtLocation(declaration(name).name);
    return { dir, fileName, program, sourceFile, checker, declaration, typeOf };
}

test("one object per type, symbol and signature, so === and Map keys work", () => {
    const { checker, typeOf, declaration } = modelProgram();
    const box = typeOf("box");
    assert.equal(typeOf("box"), box);
    assert.equal(checker.getTypeOfSymbol(checker.getSymbolAtLocation(declaration("box").name)), box);
    const seen = new Map([[box, "box"]]);
    assert.equal(seen.get(checker.getTypeAtLocation(declaration("box").name)), "box");
    const value = box.getProperty("value");
    assert.equal(box.getProperties().find(property => property.name === "value"), value);
    const [map] = checker.getTypeOfSymbol(box.getProperty("map")).getCallSignatures();
    assert.equal(checker.getTypeOfSymbol(box.getProperty("map")).getCallSignatures()[0], map);
    assert.equal(box.target, typeOf("item").target);
    assert.equal(checker.getDeclaredTypeOfSymbol(box.symbol), box.target);
});

test("objects come from TS 6.0's constructors and their prototype methods run through this checker", () => {
    const { checker, typeOf, declaration } = modelProgram();
    const box = typeOf("box");
    assert.ok(box instanceof ts.objectAllocator.getTypeConstructor());
    assert.equal(box.constructor.name, "TypeObject");
    assert.equal(box.checker, checker);
    const symbol = checker.getSymbolAtLocation(declaration("box").name);
    assert.ok(symbol instanceof ts.objectAllocator.getSymbolConstructor());
    assert.equal(symbol.getName(), "box");
    assert.equal(symbol.getEscapedName(), "box");
    assert.equal(symbol.getFlags(), ts.SymbolFlags.BlockScopedVariable);
    assert.equal(symbol.getDeclarations()[0], declaration("box"));
    assert.ok(symbol.id > 0);
    const [signature] = checker.getTypeOfSymbol(box.getProperty("map")).getCallSignatures();
    assert.ok(signature instanceof ts.objectAllocator.getSignatureConstructor());
    assert.equal(checker.typeToString(signature.getReturnType()), "Box<U>");
    assert.equal(signature.getTypeParameters().length, 1);
    assert.deepEqual(signature.getParameters().map(parameter => parameter.name), ["f"]);
    assert.equal(signature.getDeclaration().kind, ts.SyntaxKind.MethodSignature);
    assert.ok(box.isClassOrInterface() === false && box.typeArguments.length === 1);
    assert.equal(checker.typeToString(box.typeArguments[0]), "string");
    assert.equal(box.getSymbol().name, "Box");
    assert.ok(box.target.isClassOrInterface());
    assert.equal(box.target.getBaseTypes().length, 0);
});

test("type fields carry TS 6.0's values: literals, bigints, tuples, intrinsics and booleans", () => {
    const { checker, typeOf } = modelProgram();
    const big = typeOf("big");
    assert.ok(big.isLiteral());
    assert.deepEqual(big.value, { negative: true, base10Value: "12345678901234567890" });
    assert.equal(big.freshType, big);
    assert.notEqual(big.regularType, big);
    const tuple = typeOf("tuple");
    assert.ok(checker.isTupleType(tuple));
    const target = tuple.target;
    assert.deepEqual(target.elementFlags, [ts.ElementFlags.Required, ts.ElementFlags.Optional, ts.ElementFlags.Rest]);
    assert.equal(target.minLength, 1);
    assert.equal(target.fixedLength, 2);
    assert.equal(target.hasRestElement, true);
    assert.equal(target.combinedFlags, ts.ElementFlags.Required | ts.ElementFlags.Optional | ts.ElementFlags.Rest);
    assert.equal(target.readonly, false);
    assert.deepEqual(target.labeledElementDeclarations.map(node => node.name.text), ["first", "second", "rest"]);
    const flag = typeOf("flag");
    assert.equal(flag.intrinsicName, "true");
    assert.equal(flag.value, undefined);
    assert.equal(checker.getTrueType(), flag.regularType ?? flag);
    assert.equal(checker.getBooleanType().intrinsicName, "boolean");
    assert.equal(checker.getBooleanType().types.length, 2);
    assert.equal(typeOf("nothing").intrinsicName, "undefined");
    assert.equal(checker.getAnyType().intrinsicName, "any");
    assert.equal(checker.getNeverType().flags, ts.TypeFlags.Never);
});

test("the error type is one shared any type for nodes the checker cannot answer", () => {
    const { checker, program, declaration } = modelProgram();
    const errorType = checker.getTypeAtLocation(undefined);
    assert.equal(errorType.flags, ts.TypeFlags.Any);
    assert.equal(errorType.intrinsicName, "error");
    assert.equal(checker.getTypeAtLocation(ts.factory.createIdentifier("synthesized")), errorType);
    const other = modelProgram("model-other");
    assert.equal(checker.getTypeAtLocation(other.declaration("box").name), errorType);
    assert.equal(checker.getTypeFromTypeNode(declaration("box").name), errorType);
    assert.equal(checker.getSymbolAtLocation(undefined), undefined);
    assert.equal(checker.typeToString(errorType), "any");
    assert.equal(program.getTypeChecker(), checker);
});

test("symbol fields: parent, members and exports tables, value declaration and transient links", () => {
    const { checker, typeOf, declaration } = modelProgram();
    const impl = checker.getSymbolAtLocation(declaration("Impl").name);
    assert.ok(impl.members instanceof Map);
    assert.deepEqual([...impl.members.keys()].sort(), ["T", "__constructor", "map", "value"]);
    assert.ok(impl.exports.has("make") && impl.exports.has("prototype"));
    const map = impl.members.get("map");
    assert.equal(map.parent, impl);
    assert.equal(map.valueDeclaration.kind, ts.SyntaxKind.MethodDeclaration);
    assert.equal(map.links, undefined);
    assert.ok("links" in map);
    const instantiated = typeOf("box").getProperty("value");
    assert.ok(instantiated.flags & ts.SymbolFlags.Transient);
    assert.equal(instantiated.links.checkFlags & ts.CheckFlags.Instantiated, ts.CheckFlags.Instantiated);
    assert.equal(checker.typeToString(instantiated.links.type), "string");
    assert.equal(checker.getRootSymbols(instantiated)[0], checker.getSymbolAtLocation(declaration("Box").name).members.get("value"));
    const file = checker.getSymbolAtLocation(declaration("box").name).parent;
    assert.ok(file.exports.has("box"));
    assert.equal(checker.getExportsOfModule(file).length, file.exports.size);
});

test("type shapes answer `in` like TS 6.0 objects: mapped, conditional and unique symbol types", () => {
    const { checker, typeOf, declaration } = modelProgram();
    const mapped = typeOf("mapped");
    assert.ok("typeParameter" in mapped && !("types" in mapped));
    assert.equal(checker.typeToString(mapped.typeParameter), "K");
    assert.equal(checker.typeToString(mapped.modifiersType), "{ a: number; }");
    assert.equal(mapped.declaration.kind, ts.SyntaxKind.MappedType);
    const conditional = checker.getTypeFromTypeNode(declaration("Cond").type);
    assert.ok(conditional.flags & ts.TypeFlags.Conditional);
    assert.equal(conditional.root.node, declaration("Cond").type);
    assert.equal(checker.typeToString(checker.getTypeFromTypeNode(conditional.root.node.trueType)), "{ s: T; }");
    assert.equal(checker.typeToString(conditional.checkType), "T");
    const keyType = typeOf("key");
    assert.ok(keyType.flags & ts.TypeFlags.UniqueESSymbol);
    const property = typeOf("withKey").getProperties().find(symbol => symbol.escapedName.startsWith("__@"));
    assert.equal(keyType.escapedName, property.escapedName);
    assert.ok(!("typeParameter" in typeOf("box")));
});

test("typeToTypeNode and signatureToSignatureDeclaration return TS 6.0 factory nodes that TS 6.0 prints", () => {
    const { checker, typeOf, sourceFile } = modelProgram();
    const printer = ts.createPrinter();
    const node = checker.typeToTypeNode(typeOf("withKey"), undefined, undefined);
    assert.equal(node.kind, ts.SyntaxKind.TypeLiteral);
    assert.equal(node.pos, -1);
    assert.equal(node.end, -1);
    assert.ok(node.flags & ts.NodeFlags.Synthesized);
    assert.equal(printer.printNode(ts.EmitHint.Unspecified, node, sourceFile), "{ [key]: number; plain: number; }");
    const multiline = checker.typeToTypeNode(typeOf("withKey"), undefined, ts.NodeBuilderFlags.MultilineObjectLiterals);
    assert.equal(printer.printNode(ts.EmitHint.Unspecified, multiline, sourceFile), "{\n    [key]: number;\n    plain: number;\n}");
    const [map] = checker.getTypeOfSymbol(typeOf("box").getProperty("map")).getCallSignatures();
    const method = checker.signatureToSignatureDeclaration(map, ts.SyntaxKind.MethodDeclaration, undefined, undefined);
    assert.equal(method.kind, ts.SyntaxKind.MethodDeclaration);
    assert.equal(printer.printNode(ts.EmitHint.Unspecified, method, sourceFile), "<U>(f: (value: string) => U): Box<U>;");
    assert.equal(checker.signatureToString(map), "<U>(f: (value: string) => U): Box<U>");
});

test("members tools call beyond the public interface answer like TypeScript 6.0", { skip: ts6Skip }, () => {
    const { rootNames, reference, native } = createPrograms("internal", modelFiles);
    const describe = (api, program) => {
        const checker = program.getTypeChecker();
        const sourceFile = program.getSourceFile(rootNames[0]);
        const results = [];
        const visit = node => {
            if (api.isCallExpression(node)) {
                for (let index = 0; index < node.arguments.length + 1; index++) {
                    results.push(`argument ${index}: ${checker.typeToString(checker.getContextualTypeForArgumentAtIndex(node, index) ?? checker.getAnyType())}`);
                }
            }
            if (api.isVariableDeclaration(node)) {
                const type = checker.getTypeAtLocation(node.name);
                for (const name of ["value", "map", "length", "a", "plain", "__proto__", "missing"]) {
                    const property = checker.getTypeOfPropertyOfType(type, name);
                    results.push(`${node.name.text}.${name}: ${property === undefined ? "none" : checker.typeToString(property)}`);
                }
                results.push(`${node.name.text} nullable: ${type.isNullableType()} ${checker.typeToString(type.getNonOptionalType())}`);
                results.push(`${node.name.text} callable: ${checker.typeHasCallOrConstructSignatures(type)} ${checker.typeToString(checker.getElementTypeOfArrayType(type) ?? checker.getNeverType())}`);
            }
            api.forEachChild(node, visit);
        };
        visit(sourceFile);
        results.push(checker.typeToString(checker.getPromiseType()), checker.typeToString(checker.getPromiseLikeType()));
        for (const statement of sourceFile.statements) {
            if (api.isClassDeclaration(statement) || api.isInterfaceDeclaration(statement) || api.isTypeAliasDeclaration(statement)) {
                const symbol = checker.getSymbolAtLocation(statement.name);
                results.push(`${statement.name.text}: ${checker.getLocalTypeParametersOfClassOrInterfaceOrTypeAlias(symbol)?.map(type => checker.typeToString(type))}`);
            }
        }
        return results;
    };
    assert.deepEqual(describe(ts, native), describe(ts6, reference));
});

test("isTypeAssignableTo agrees with TypeScript 6.0 for every pair of declared types", { skip: ts6Skip }, () => {
    const files = {
        "assign.ts": `declare const a: string; declare const b: "lit"; declare const c: number; declare const d: 1 | 2;
declare const e: string | number; declare const f: unknown; declare const g: any; declare const h: never;
declare const i: { x: number }; declare const j: { x: number; y: string }; declare const k: { readonly x: number };
declare const l: number[]; declare const m: readonly number[]; declare const n: [number, number];
declare const o: () => void; declare const p: (x: string) => number; declare const q: Promise<string>;
declare const r: PromiseLike<string>; declare const s: object; declare const t: {}; declare const u: undefined; declare const v: null;
declare const w: Record<string, number>; declare const x: Map<string, number>; declare const y: symbol; declare const z: bigint;
`,
    };
    const { rootNames, reference, native } = createPrograms("assign", files);
    const matrix = (api, program) => {
        const checker = program.getTypeChecker();
        const types = [];
        const visit = node => {
            if (api.isVariableDeclaration(node)) {
                types.push([node.name.text, checker.getTypeAtLocation(node.name)]);
            }
            api.forEachChild(node, visit);
        };
        visit(program.getSourceFile(rootNames[0]));
        return types.flatMap(([sourceName, source]) => types.map(([targetName, target]) => `${sourceName}->${targetName}:${checker.isTypeAssignableTo(source, target)}`));
    };
    const expected = matrix(ts6, reference);
    assert.equal(expected.length, 26 * 26);
    assert.deepEqual(matrix(ts, native), expected);
});

test("members tsc-rs cannot answer throw a not-supported error instead of a wrong answer", () => {
    const { checker, typeOf } = modelProgram();
    for (const [name, call] of [
        ["getStringLiteralType", () => checker.getStringLiteralType("a")],
        ["getNumberLiteralType", () => checker.getNumberLiteralType(1)],
        ["getBigIntLiteralType", () => checker.getBigIntLiteralType({ negative: false, base10Value: "1" })],
        ["getSymbolOfExpando", () => checker.getSymbolOfExpando(typeOf("box").symbol.declarations[0], false)],
        ["getNullableType", () => checker.getNullableType(checker.getStringType(), ts.TypeFlags.Undefined)],
        ["getIndexType", () => checker.getIndexType(typeOf("box"))],
    ]) {
        assert.throws(call, new RegExp(`^Error: tsc-rs: ${name} is not supported: `), name);
    }
    assert.equal(checker.getNullableType(typeOf("nothing"), ts.TypeFlags.Undefined), typeOf("nothing"));
    assert.throws(() => checker.getAliasedSymbol(typeOf("box").symbol), /Should only get Alias here/);
});

test("every member of TS 6.0's public TypeChecker and the internal ones tools call is a function", () => {
    const declarations = fs.readFileSync(path.join(packageDir, "lib/typescript.d.ts"), "utf8");
    const start = declarations.indexOf("interface TypeChecker {");
    const body = declarations.slice(start, declarations.indexOf("\n    }\n", start));
    const publicMembers = [...new Set([...body.matchAll(/^ {8}(\w+)[(:<]/gm)].map(match => match[1]))];
    assert.equal(publicMembers.length, 90);
    const internalMembers = [
        "getTypeOfPropertyOfType", "getContextualTypeForArgumentAtIndex", "isNullableType", "getNonOptionalType", "getParameterType",
        "getConstraintOfTypeParameter", "isContextSensitive", "getDiagnostics", "getGlobalDiagnostics", "getSuggestionDiagnostics",
        "getNodeCount", "getIdentifierCount", "getSymbolCount", "getTypeCount", "getInstantiationCount", "getRelationCacheSizes",
    ];
    const { checker } = modelProgram("members");
    const missing = [...publicMembers, ...internalMembers].filter(name => typeof checker[name] !== "function");
    assert.deepEqual(missing, []);
});

test("getAmbientModules and declarations in lib files answer like TypeScript 6.0", { skip: ts6Skip }, () => {
    const files = {
        "ambient.d.ts": "declare module \"virtual:config\" { export const value: number; }\ndeclare module \"other-*\" { const x: string; export default x; }\n",
        "use.ts": "import { value } from \"virtual:config\";\nexport const doubled = [value].map(n => n * 2);\n",
    };
    const { rootNames, reference, native } = createPrograms("ambient", files);
    const describe = (api, program) => {
        const checker = program.getTypeChecker();
        const modules = checker.getAmbientModules().map(symbol => symbol.name).sort();
        const doubled = program.getSourceFile(rootNames[1]).statements[1].declarationList.declarations[0];
        const map = checker.getSymbolAtLocation(doubled.initializer.expression.name);
        const declaration = map.declarations[0];
        return [
            ...modules,
            path.basename(declaration.getSourceFile().fileName),
            api.SyntaxKind[declaration.kind],
            checker.typeToString(checker.getTypeAtLocation(declaration.name)),
            checker.symbolToString(checker.getSymbolAtLocation(declaration.parent.name)),
            program.isSourceFileDefaultLibrary(declaration.getSourceFile()),
        ];
    };
    assert.deepEqual(describe(ts, native), describe(ts6, reference));
});

// TSRS_CHECKER_PROJECT=<tsconfig.json> compares the checkers on a whole project and reports the
// agreement (TSRS_CHECKER_FILES limits the number of files). It asserts only that both answer.
test("compares the checkers on the project in TSRS_CHECKER_PROJECT", { skip: ts6Skip || (process.env.TSRS_CHECKER_PROJECT ? false : "set TSRS_CHECKER_PROJECT to a tsconfig.json") }, () => {
    const configPath = path.resolve(process.env.TSRS_CHECKER_PROJECT);
    const parsed = ts6.getParsedCommandLineOfConfigFile(configPath, {}, { ...ts6.sys, onUnRecoverableConfigFileDiagnostic: diagnostic => assert.fail(ts6.flattenDiagnosticMessageText(diagnostic.messageText, "\n")) });
    const limit = Number(process.env.TSRS_CHECKER_FILES ?? Infinity);
    const options = { ...parsed.options, noEmit: true };
    const reference = ts6.createProgram(parsed.fileNames, { ...options, stableTypeOrdering: true });
    const native = ts.createProgram(parsed.fileNames, options);
    const rootNames = parsed.fileNames.filter(fileName => !fileName.endsWith(".d.ts")).slice(0, limit);
    const started = performance.now();
    const result = compareCheckers({ dir: path.dirname(configPath), rootNames, reference, native });
    const elapsed = performance.now() - started;
    const byFact = new Map();
    for (const difference of result.differences) {
        byFact.set(difference.name, (byFact.get(difference.name) ?? 0) + 1);
    }
    console.log(`${configPath}: ${rootNames.length} files, ${result.facts - result.differences.length}/${result.facts} facts agree (${(100 * (1 - result.differences.length / result.facts)).toFixed(2)}%) in ${(elapsed / 1000).toFixed(1)} s`);
    console.log([...byFact].sort((left, right) => right[1] - left[1]).map(([name, count]) => `  ${name}: ${count}`).join("\n"));
    if (report) {
        for (const difference of result.differences) {
            console.log(`  ${difference.where} [${difference.name}]\n    ts6: ${difference.expected}\n    ours: ${difference.actual}`);
        }
    }
});

const emulatedFiles = {
    "emulated.ts": `export class Base { protected prot = 1; private priv = 2; pub = 3; #hash = 4; static stat = 5; readHash() { return this.#hash; } }
export class Sub extends Base {
    constructor(public label: string, readonly count = 0) { super(); }
    look(other: Base, sub: Sub) { return [this.prot, other.pub, sub.prot]; }
}
export function over(a: string): string;
export function over(a: number): number;
export function over(a: any) { return a; }
export function single(a = 1, b?: string, ...rest: number[]) { return [a, b, rest]; }
export function trailing(a = 1, b: string) { return [a, b]; }
export const iife = (function (x, y) { return [x, y]; })(1);
export interface Indexed { [key: string]: number; [index: number]: 1 | 2; readonly [sym: symbol]: string }
export type Union = { kind: "a"; value: string } | { kind: "b"; value: number };
declare const union: Union;
export const value = union.value;
let left = 0, right = "";
({ left, right } = { left: 1, right: "r" });
export { left, right };
export namespace Space { export const inner = 1; export namespace Deep { export type T = string; } }
export type Deep = Space.Deep.T;
export function generic<const T extends string, in out U = number>(t: T, u?: U): [T, U | undefined] { return [t, u]; }
`,
};

test("members emulated in JS answer like TypeScript 6.0", { skip: ts6Skip }, () => {
    const { rootNames, reference, native } = createPrograms("emulated", emulatedFiles);
    const describe = (api, program) => {
        const checker = program.getTypeChecker();
        const sourceFile = program.getSourceFile(rootNames[0]);
        const printer = api.createPrinter({ removeComments: true });
        const print = node => node === undefined ? "none" : printer.printNode(api.EmitHint.Unspecified, node, sourceFile);
        const results = [];
        const visit = node => {
            if (api.isFunctionLike(node)) {
                results.push(`overload ${node.pos}: ${checker.isImplementationOfOverload(node)}`);
                for (const parameter of node.parameters) {
                    results.push(`optional ${parameter.name.getText()}: ${checker.isOptionalParameter(parameter)}`);
                    if (api.isParameterPropertyDeclaration(parameter, node)) {
                        results.push(`parameter property ${checker.getSymbolsOfParameterPropertyDeclaration(parameter, parameter.name.text).map(symbol => `${symbol.name}:${symbol.flags}`)}`);
                    }
                    const symbol = checker.getSymbolAtLocation(parameter.name);
                    if (symbol !== undefined && api.isIdentifier(parameter.name)) {
                        results.push(`parameter declaration ${print(checker.symbolToParameterDeclaration(symbol, undefined, undefined))}`);
                    }
                }
            }
            if (api.isPropertyAccessExpression(node)) {
                for (const name of ["prot", "priv", "pub", "missing", "value", "kind"]) {
                    results.push(`valid ${node.getText()} ${name}: ${checker.isValidPropertyAccess(node, name)}`);
                }
                const symbol = checker.getSymbolAtLocation(node.name);
                if (symbol !== undefined) {
                    results.push(`roots ${node.getText()}: ${checker.getRootSymbols(symbol).map(root => `${root.name}@${root.declarations?.[0]?.pos}`).join(",")}`);
                }
            }
            if (api.isShorthandPropertyAssignment(node) && api.isObjectLiteralExpression(node.parent) && api.isBinaryExpression(node.parent.parent)) {
                const property = checker.getPropertySymbolOfDestructuringAssignment(node.name);
                results.push(`destructured ${node.name.text}: ${property?.name} ${checker.typeToString(checker.getTypeOfAssignmentPattern(node.parent))}`);
            }
            if (api.isPrivateIdentifier(node) && api.isPropertyAccessExpression(node.parent)) {
                const type = checker.getTypeAtLocation(node.parent.expression);
                results.push(`private ${node.text}: ${checker.getPrivateIdentifierPropertyOfType(type, node.text, node)?.flags}`);
            }
            if (api.isClassDeclaration(node) || api.isInterfaceDeclaration(node) || api.isTypeAliasDeclaration(node) || api.isModuleDeclaration(node) || api.isFunctionDeclaration(node)) {
                const symbol = checker.getSymbolAtLocation(node.name);
                results.push(`entity ${node.name.text}: ${print(checker.symbolToEntityName(symbol, api.SymbolFlags.Type, node, undefined))} ${print(checker.symbolToEntityName(symbol, api.SymbolFlags.Value, node, undefined))}`);
                results.push(`expression ${node.name.text}: ${print(checker.symbolToExpression(symbol, api.SymbolFlags.Value, node, undefined))}`);
                results.push(`type parameters ${node.name.text}: ${checker.symbolToTypeParameterDeclarations(symbol, undefined, undefined)?.map(print).join(", ")}`);
                const indexSymbol = symbol.members?.get("__index");
                if (indexSymbol !== undefined) {
                    results.push(`index infos of index symbol: ${checker.getIndexInfosOfIndexSymbol(indexSymbol).map(info => checker.typeToString(info.keyType)).join(",")}`);
                }
            }
            if (api.isFunctionDeclaration(node) && node.typeParameters !== undefined) {
                const signature = checker.getSignatureFromDeclaration(node);
                results.push(`type parameter declarations: ${signature.typeParameters.map(parameter => print(checker.typeParameterToDeclaration(parameter, undefined, undefined))).join(", ")}`);
            }
            if (api.isQualifiedName(node) || api.isIdentifier(node) && api.isTypeReferenceNode(node.parent)) {
                results.push(`type at ${node.getText()}: ${checker.typeToString(checker.getTypeAtLocation(node))}`);
            }
            api.forEachChild(node, visit);
        };
        visit(sourceFile);
        results.push(`ambient: ${checker.getAmbientModules().length}`);
        results.push(`jsx: ${checker.getJsxIntrinsicTagNamesAt(sourceFile).length}`);
        results.push(`scope: ${checker.getSymbolsInScope(sourceFile.statements[1], api.SymbolFlags.Value).filter(symbol => symbol.declarations?.[0]?.getSourceFile() === sourceFile).map(symbol => symbol.name).sort().join(",")}`);
        return results;
    };
    const expected = describe(ts6, reference);
    const actual = describe(ts, native);
    if (report) {
        expected.forEach((line, index) => line !== actual[index] && console.log(`  ts6: ${line}\n  ours: ${actual[index]}`));
    }
    assert.deepEqual(actual, expected);
});

test("contextual documentation of accessors comes from the getter or the setter", { skip: ts6Skip }, () => {
    const { rootNames, reference, native } = createPrograms("accessor-docs", docFiles);
    const describe = (api, program) => {
        const checker = program.getTypeChecker();
        const sourceFile = program.getSourceFile(rootNames[0]);
        const results = [];
        const visit = node => {
            if (api.isGetAccessor(node) || api.isSetAccessor(node)) {
                const symbol = checker.getSymbolAtLocation(node.name);
                results.push(api.displayPartsToString(symbol.getContextualDocumentationComment(node, checker)));
                results.push(symbol.getContextualJsDocTags(node, checker).length);
            }
            api.forEachChild(node, visit);
        };
        visit(sourceFile);
        return results;
    };
    const expected = describe(ts6, reference);
    assert.deepEqual(expected, ["Reads the width.", 0, "Writes the width.", 0]);
    assert.deepEqual(describe(ts, native), expected);
});

test("signatureToSignatureDeclaration builds every declaration kind like TypeScript 6.0", { skip: ts6Skip }, () => {
    const files = {
        "kinds.ts": `export declare function plain<T extends object = {}>(this: Window, a: T, b?: string, ...rest: [x: number, y?: boolean]): asserts a is T & { tag: 1 };
export declare function guard(value: unknown, { a, b: [c] }: { a: string; b: number[] }): value is readonly string[];
export declare class Shape { constructor(size: number); get area(): number; method<K extends keyof this>(key: K): this[K]; }
`,
    };
    const { rootNames, reference, native } = createPrograms("kinds", files);
    const kinds = ["FunctionDeclaration", "MethodDeclaration", "MethodSignature", "Constructor", "GetAccessor", "SetAccessor", "CallSignature", "ConstructSignature", "IndexSignature", "FunctionType", "ConstructorType", "ArrowFunction", "FunctionExpression"];
    const describe = (api, program) => {
        const checker = program.getTypeChecker();
        const sourceFile = program.getSourceFile(rootNames[0]);
        const printer = api.createPrinter({ removeComments: true });
        const results = [];
        const visit = node => {
            if (api.isFunctionLike(node) && node.parent !== undefined) {
                const signature = checker.getSignatureFromDeclaration(node);
                for (const kind of kinds) {
                    for (const flags of [undefined, api.NodeBuilderFlags.NoTruncation | api.NodeBuilderFlags.WriteTypeArgumentsOfSignature | api.NodeBuilderFlags.OmitThisParameter]) {
                        const declaration = checker.signatureToSignatureDeclaration(signature, api.SyntaxKind[kind], undefined, flags);
                        results.push(`${kind}: ${declaration === undefined ? "none" : printer.printNode(api.EmitHint.Unspecified, declaration, sourceFile)}`);
                    }
                }
            }
            api.forEachChild(node, visit);
        };
        visit(sourceFile);
        return results;
    };
    const expected = describe(ts6, reference);
    const actual = describe(ts, native);
    if (report) {
        expected.forEach((line, index) => line !== actual[index] && console.log(`  ts6: ${line}\n  ours: ${actual[index]}`));
    }
    assert.deepEqual(actual, expected);
});

test("object fields are fetched once, on first read", () => {
    const { program, checker, typeOf } = modelProgram("lazy");
    let requests = 0;
    const request = program.request.bind(program);
    program.request = (method, params) => {
        requests++;
        return request(method, params);
    };
    const count = read => {
        const before = requests;
        read();
        return requests - before;
    };
    const box = typeOf("box");
    assert.equal(count(() => [box.flags, box.objectFlags, box.id, box.checker]), 0);
    assert.equal(count(() => box.typeArguments), 1);
    assert.equal(count(() => box.typeArguments), 0);
    assert.equal(count(() => box.getProperties()), 1);
    assert.equal(count(() => box.getProperties()), 0);
    const property = box.getProperty("value");
    assert.equal(count(() => [property.name, property.escapedName, property.flags]), 0);
    assert.equal(count(() => checker.typeToString(box)), 1);
    assert.equal(count(() => checker.typeToString(box)), 0);
});

test("answers the checker already holds need no request: apparent types, index infos by kind, repeated questions", () => {
    const dir = writeFixture("local-answers", {
        "local.ts": "export const o = { a: 1 };\nexport const u: string | number[] = \"\";\nexport const r: Record<string, number> = {};\nexport const f = true as boolean;\n",
    });
    const fileName = path.join(dir, "local.ts");
    const program = ts.createProgram([fileName], baseOptions);
    const checker = program.getTypeChecker();
    let requests = 0;
    const request = program.request.bind(program);
    program.request = (method, params) => {
        requests++;
        return request(method, params);
    };
    const count = read => {
        const before = requests;
        const value = read();
        return [value, requests - before];
    };
    const names = Object.fromEntries(program.getSourceFile(fileName).statements.map(statement => statement.declarationList.declarations[0]).map(declaration => [declaration.name.text, declaration.name]));
    const [o] = count(() => checker.getTypeAtLocation(names.o));
    assert.deepEqual(count(() => checker.getApparentType(o)), [o, 0]);
    const u = checker.getTypeAtLocation(names.u);
    assert.deepEqual(count(() => checker.getApparentType(u)), [u, 0]);
    // `boolean` is a union too, but its apparent type is the global Boolean interface.
    const [apparentBoolean, booleanRequests] = count(() => checker.getApparentType(checker.getTypeAtLocation(names.f)));
    assert.equal(checker.typeToString(apparentBoolean), "Boolean");
    assert.ok(booleanRequests > 0);
    const r = checker.getTypeAtLocation(names.r);
    checker.getIndexInfosOfType(r);
    const [stringIndex, indexRequests] = count(() => checker.getIndexInfoOfType(r, ts.IndexKind.String));
    assert.equal(indexRequests, 0);
    assert.equal(checker.typeToString(stringIndex.type), "number");
    assert.deepEqual(count(() => checker.getIndexInfoOfType(r, ts.IndexKind.Number)), [undefined, 0]);
    const [assignable] = count(() => checker.isTypeAssignableTo(o, r));
    assert.deepEqual(count(() => checker.isTypeAssignableTo(o, r)), [assignable, 0]);
    assert.deepEqual(count(() => checker.getTypeAtLocation(names.o)), [o, 0]);
});

// typescript-eslint's project service (packages/project-service/src/createProjectService.ts).
function projectService() {
    const noop = () => {};
    const host = { ...ts.sys, clearImmediate, clearTimeout, setImmediate, setTimeout, watchDirectory: () => ({ close: noop }), watchFile: () => ({ close: noop }) };
    const logger = { close: noop, endGroup: noop, getLogFileName: () => undefined, hasLevel: () => false, info: noop, loggingEnabled: () => false, msg: noop, perftrc: noop, startGroup: noop };
    return new ts.server.ProjectService({
        cancellationToken: ts.server.nullCancellationToken,
        host,
        logger,
        session: undefined,
        useInferredProjectPerProjectRoot: false,
        useSingleInferredProject: false,
        jsDocParsingMode: ts.JSDocParsingMode.ParseAll,
    });
}

test("declarations in files programs share answer `symbol` from the newest program, and old programs can be collected", async () => {
    v8.setFlagsFromString("--expose-gc");
    const gc = vm.runInNewContext("gc");
    const dir = writeFixture("shared-declarations", {
        "tsconfig.json": JSON.stringify({ compilerOptions: { strict: true, target: "es2022", module: "commonjs", types: [] }, include: ["src"] }),
        "src/dep.ts": "export interface Dep { a: number }\nexport function make(a: number): Dep { return { a }; }\nexport class Box { value = 1; }\n",
        "src/file.ts": "",
    });
    const service = projectService();
    const file = path.join(dir, "src/file.ts");
    const programs = [];
    let depFile;
    for (let index = 0; index < 30; index++) {
        service.openClientFile(file, `import { make, Box } from "./dep";\nconst x${index} = make(${index});\nconst box = new Box();\n`, undefined, dir);
        const program = service.getDefaultProjectForFile(service.getScriptInfo(file).fileName, true).getLanguageService(true).getProgram();
        const checker = program.getTypeChecker();
        const [, made, box] = program.getSourceFile(file).statements.map(statement => statement.declarationList?.declarations[0]);
        const declaration = checker.getResolvedSignature(made.initializer).declaration;
        assert.equal(checker.typeToString(checker.getTypeOfSymbol(declaration.symbol)), "(a: number) => Dep");
        const [value] = checker.getTypeAtLocation(box.name).getProperties();
        assert.equal(value.declarations[0].symbol, checker.getSymbolAtLocation(value.declarations[0].name));
        depFile ??= ts.getSourceFileOfNode(declaration);
        assert.equal(ts.getSourceFileOfNode(declaration), depFile, "dep.ts is one SourceFile shared by every program");
        programs.push(new WeakRef(program));
    }
    for (let round = 0; round < 2; round++) {
        await new Promise(resolve => setImmediate(resolve));
        gc();
    }
    const alive = programs.filter(reference => reference.deref() !== undefined).length;
    assert.ok(alive <= 2, `${alive} of ${programs.length} programs are still alive`);
});

// typescript-eslint no-unnecessary-type-assertion typeContains: an unbounded walk through return
// types, which never repeats a type here (each level instantiates a new one).
function typeContains(checker, type, predicate, seen = new Set()) {
    if (seen.has(type)) {
        return false;
    }
    seen.add(type);
    if (predicate(type)) {
        return true;
    }
    if (type.isUnionOrIntersection()) {
        return type.types.some(member => typeContains(checker, member, predicate, seen));
    }
    const typeArguments = type.aliasTypeArguments ?? (type.objectFlags & ts.ObjectFlags.Reference ? checker.getTypeArguments(type) : []);
    const nested = [
        ...typeArguments,
        ...type.getCallSignatures().flatMap(signature => [signature.getReturnType(), ...signature.getParameters().map(parameter => checker.getTypeOfSymbol(parameter))]),
    ];
    return nested.some(member => typeContains(checker, member, predicate, seen));
}

test("an unbounded walk over instantiated types ends near the end of the stack, as TS 6.0's second walk does", () => {
    const dir = writeFixture("recursive-method", {
        "file.ts": "type RecursiveMethod<Options = {}> = (<NewOptions = {}>(options: NewOptions) => RecursiveMethod<Options & NewOptions>) & ((command: string) => Promise<void>);\ndeclare const recursiveMethod: RecursiveMethod;\ndeclare const value: object;\nvalue as unknown as typeof recursiveMethod;\n",
    });
    const program = ts.createProgram([path.join(dir, "file.ts")], { ...baseOptions, lib: ["lib.es2015.d.ts"], types: [] });
    const checker = program.getTypeChecker();
    const cast = checker.getTypeAtLocation(program.getSourceFile(path.join(dir, "file.ts")).statements[3].expression);
    // TS 6.0 throws "Maximum call stack size exceeded" in the first walk and leaves the return type
    // it was resolving on its resolution stack, so the second walk stops there and finds the type
    // parameter. tsc-rs answers that circularity (errorType, no members) once less than 64 KB of
    // stack is left, so neither walk throws; the first may end on the error type, which is `any`.
    const containsAny = typeContains(checker, cast, type => (type.flags & ts.TypeFlags.Any) !== 0);
    assert.equal(typeof containsAny, "boolean");
    assert.equal(typeContains(checker, cast, type => (type.flags & (ts.TypeFlags.TypeVariable | ts.TypeFlags.Index)) !== 0), true);
    const [signature] = cast.types[0].getCallSignatures();
    assert.equal(checker.typeToString(signature.getReturnType()), "RecursiveMethod<{} & NewOptions>");
    assert.equal(signature.getParameters().length, 1);
});

// src/index.js installs the node `symbol` field when lib/typescript.js loads; installing it again
// is a no-op, and here it makes the tests independent of that line.
require(path.join(packageDir, "src/objects.js")).install(ts);

test("a parsed declaration keeps TS 6.0's own keys and answers `symbol` from its program on first read", () => {
    const text = "type Foo = { a: number };\nfunction f(x: number) { return x; }\nf(1);\n";
    const parsed = ts.createSourceFile("a.ts", text, ts.ScriptTarget.Latest, true);
    const reference = ts6?.createSourceFile("a.ts", text, ts6.ScriptTarget.Latest, true);
    for (const [index, statement] of parsed.statements.slice(0, 2).entries()) {
        assert.equal(statement.symbol, undefined, "a file no program has answers nothing");
        assert.ok(Object.keys(statement).includes("symbol"));
        if (reference !== undefined) {
            assert.deepEqual(Object.keys(statement), Object.keys(reference.statements[index]));
        }
    }
    const dir = writeFixture("node-symbols", { "main.ts": `export ${text}`, "script.js": "function foo() { let a = 10; }\n" });
    const program = ts.createProgram([path.join(dir, "main.ts"), path.join(dir, "script.js")], { ...baseOptions, allowJs: true });
    const main = program.getSourceFile(path.join(dir, "main.ts"));
    const [alias, func] = main.statements;
    // No checker exists yet: the first read makes one.
    assert.equal(func.symbol.name, "f");
    assert.equal(func.parameters[0].symbol.name, "x");
    assert.equal(alias.type.symbol.flags & ts.SymbolFlags.TypeLiteral, ts.SymbolFlags.TypeLiteral);
    assert.equal(main.symbol, program.getTypeChecker().getSymbolAtLocation(main), "a module's SourceFile has its module symbol");
    assert.equal(program.getSourceFile(path.join(dir, "script.js")).statements[0].symbol.name, "foo", "a JS function, as suggestion diagnostics read it");
    assert.equal(func.body.symbol, undefined, "a block is no declaration");
    // TS 6.0's binder and transformers assign the field; the assignment wins.
    func.symbol = "assigned";
    assert.equal(func.symbol, "assigned");
});

function languageServiceHost(api, files, options) {
    return {
        getScriptFileNames: () => Object.keys(files),
        getScriptVersion: () => "1",
        getScriptSnapshot: name => {
            const text = files[name] ?? api.sys.readFile(name);
            return text === undefined ? undefined : api.ScriptSnapshot.fromString(text);
        },
        getCurrentDirectory: () => path.dirname(Object.keys(files)[0]),
        getCompilationSettings: () => options,
        getDefaultLibFileName: settings => api.getDefaultLibFilePath(settings),
        fileExists: name => files[name] !== undefined || api.sys.fileExists(name),
        readFile: name => files[name] ?? api.sys.readFile(name),
    };
}

// Quick info, signature help, definitions, references and completion details of every identifier
// and call of the files, as text.
function languageServiceFacts(api, files, options) {
    const service = api.createLanguageService(languageServiceHost(api, files, options));
    const parts = displayParts => displayParts === undefined ? "" : api.displayPartsToString(displayParts);
    const facts = new Map();
    for (const [fileName, text] of Object.entries(files)) {
        const base = path.basename(fileName);
        for (const match of text.matchAll(/[A-Za-z_]\w*|[(,]/g)) {
            const at = `${base}:${match.index} ${match[0]}`;
            if (match[0] === "(" || match[0] === ",") {
                facts.set(`signature help ${at}`, guard(() => {
                    const help = service.getSignatureHelpItems(fileName, match.index + 1, {});
                    return help && `${help.selectedItemIndex}/${help.argumentIndex}: ${help.items.map(item => [parts(item.prefixDisplayParts), item.parameters.map(parameter => parts(parameter.displayParts)).join(parts(item.separatorDisplayParts)), parts(item.suffixDisplayParts)].join("")).join(" | ")}`;
                }));
                continue;
            }
            facts.set(`quick info ${at}`, guard(() => {
                const info = service.getQuickInfoAtPosition(fileName, match.index);
                return info && `${info.kind} ${info.kindModifiers}: ${parts(info.displayParts)}`;
            }));
            // tsc-rs's lib files are TS 7's, so positions in them differ.
            const where = definition => path.basename(definition.fileName).startsWith("lib.") ? path.basename(definition.fileName) : `${path.basename(definition.fileName)}:${definition.textSpan.start}`;
            facts.set(`definition ${at}`, guard(() => service.getDefinitionAtPosition(fileName, match.index)?.map(definition => `${definition.kind} ${definition.containerName}.${definition.name} ${where(definition)}`).join(", ")));
        }
        for (const match of text.matchAll(/\b(?:class|function|const|let|interface|type) (\w+)/g)) {
            const position = match.index + match[0].length - match[1].length;
            facts.set(`references ${base}:${position} ${match[1]}`, guard(() => service.findReferences(fileName, position)?.map(symbol => `${parts(symbol.definition.displayParts)}: ${symbol.references.map(reference => `${path.basename(reference.fileName)}:${reference.textSpan.start}`).join(",")}`).join(" | ")));
        }
        for (const match of text.matchAll(/\b(\w+)\.(\w+)/g)) {
            const position = match.index + match[1].length + 1;
            facts.set(`completion details ${base}:${position} ${match[2]}`, guard(() => {
                const details = service.getCompletionEntryDetails(fileName, position, match[2], {}, undefined, {}, undefined);
                return details && `${details.kind}: ${parts(details.displayParts)}`;
            }));
        }
    }
    return facts;
}

test("quick info, signature help, definitions, references and completion details read like TypeScript 6.0", { skip: ts6Skip }, () => {
    const dir = writeFixture("language-service", {});
    const files = {
        [path.join(dir, "main.ts")]: `import { helper, Shape } from "./lib";
function overloaded(): void;
function overloaded<T>(x: T): T;
function overloaded<T>(x?: T): T | void {}
overloaded("s");
namespace N { export class C<T extends string = "a", U = number> { m(n: number) { return this; } static s(this: void) {} p = 1; private q = 2; } }
const c = new N.C<"z">();
c.m(1);
N.C.s();
const shape: Shape = { kind: "circle", radius: 1 };
helper(shape, 2);
function isText(value: unknown): value is string { return typeof value === "string"; }
function assertNumber(value: unknown): asserts value is number {}
const literal = { method() { return this; }, field: 1 };
literal.method();
export default class {}
const arrow = (x: number, y = 2, ...rest: string[]) => x + y + rest.length;
arrow(1, 2, "a");
enum Color { Red, Green }
const red = Color.Red;
`,
        [path.join(dir, "lib.ts")]: `export interface Shape { kind: "circle" | "square"; radius?: number }
/** Area of a shape. */
export function helper(shape: Shape, scale: number): number { return (shape.radius ?? 1) * scale; }
`,
    };
    const options = { strict: true, target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext, moduleResolution: ts.ModuleResolutionKind.Bundler, declaration: true, types: [] };
    const expected = languageServiceFacts(ts6, files, { ...options, stableTypeOrdering: true });
    const actual = languageServiceFacts(ts, files, options);
    // tsc-rs does not send the type arguments of the type an instantiated member was read from, so
    // its container is written without them (TS 6.0: `N.C<"z", number>.m`).
    const known = [
        ["quick info main.ts:", " m", "(method) N.C<\"z\", number>.m(n: number): N.C<\"z\", number>", "(method) N.C.m(n: number): N.C<\"z\", number>"],
        ["completion details main.ts:", " m", "(method) N.C<\"z\", number>.m(n: number): N.C<\"z\", number>", "(method) N.C.m(n: number): N.C<\"z\", number>"],
        ["quick info main.ts:", " length", "(property) Array<string>.length: number", "(property) Array.length: number"],
        ["completion details main.ts:", " length", "(property) Array<string>.length: number", "(property) Array.length: number"],
    ];
    const differences = [];
    for (const [key, value] of expected) {
        const ours = actual.get(key);
        if (ours === value) {
            continue;
        }
        const explained = known.some(([prefix, suffix, theirs, mine]) => key.includes(prefix) && key.endsWith(suffix) && value.endsWith(theirs) && ours?.endsWith(mine));
        if (!explained) {
            differences.push(`${key}\n    ts6:  ${value}\n    ours: ${ours}`);
        }
    }
    assert.deepEqual(differences, []);
    assert.ok([...actual.values()].filter(value => value.startsWith("throws")).length === 0);
});

test("tryGetThisTypeAt and isDeclarationVisible answer like TypeScript 6.0 at every node", { skip: ts6Skip }, () => {
    const { dir, rootNames, reference, native } = createPrograms("this-and-visibility", {
        "main.ts": `export class Box<T> {
    value!: T;
    get(this: Box<T>, n: number) { return this.value; }
    static make() { return this; }
    private hidden = 1;
    protected shared() {}
}
export function standalone(this: { tag: string }) { return this.tag; }
function plain() { return 1; }
const literal = { field: 1, method() { return this.field; }, nested: { inner() { return 2; } } };
interface Handlers { onClick(this: HTMLLike, n: number): void; name?: string }
interface HTMLLike { id: string }
const typed: Handlers = { onClick(n) { return this.id; }, name: "x" };
const target = { count: 0, bump: undefined as undefined | (() => void) };
target.bump = function () { this.count++; };
export namespace Space { export const inner = 1; const local = 2; }
const local = 1;
export { local };
interface Shape { a: number }
export type { Shape };
declare module "ambient" { export const x: number; }
`,
    }, { declaration: true, noImplicitThis: true });
    reference.getSemanticDiagnostics();
    const theirs = reference.getTypeChecker();
    const ours = native.getTypeChecker();
    const differences = [];
    for (const fileName of rootNames) {
        const referenceNodes = preorder(ts6, reference.getSourceFile(fileName));
        const nativeNodes = preorder(ts, native.getSourceFile(fileName));
        referenceNodes.forEach((referenceNode, index) => {
            const nativeNode = nativeNodes[index];
            const where = `${referenceNode.pos} ${ts6.SyntaxKind[referenceNode.kind]} ${referenceNode.getText().slice(0, 30)}`;
            const thisType = (api, checker, node, includeGlobalThis) => {
                const type = checker.tryGetThisTypeAt(node, includeGlobalThis);
                return type === undefined ? "none" : checker.typeToString(type);
            };
            for (const includeGlobalThis of [true, false]) {
                const expected = guard(() => thisType(ts6, theirs, referenceNode, includeGlobalThis));
                const actual = guard(() => thisType(ts, ours, nativeNode, includeGlobalThis));
                if (expected !== actual) {
                    differences.push(`${where} tryGetThisTypeAt(${includeGlobalThis}): ts6 ${expected}, ours ${actual}`);
                }
            }
            const visible = guard(() => theirs.isDeclarationVisible(referenceNode));
            const ourVisible = guard(() => ours.isDeclarationVisible(nativeNode));
            if (visible !== ourVisible) {
                differences.push(`${where} isDeclarationVisible: ts6 ${visible}, ours ${ourVisible}`);
            }
        });
    }
    assert.deepEqual(differences, []);
    void dir;
});

test("signature help writes candidates in TS 6.0's order, the chosen one instantiated, with display parts", { skip: ts6Skip }, () => {
    const { rootNames, reference, native } = createPrograms("signature-help", {
        "main.ts": `declare function pick(a: "x"): 1;
declare function pick(a: string): 2;
declare function pick<T>(a: T[]): T;
pick([true]);
declare function spread(...args: [a: string, b?: number]): void;
`,
    });
    const call = program => program.getSourceFile(rootNames[0]).statements[3].expression;
    const describe = (api, program) => {
        const checker = program.getTypeChecker();
        const candidates = [];
        const resolved = checker.getResolvedSignatureForSignatureHelp(call(program), candidates, 1);
        return {
            candidates: candidates.map(signature => checker.signatureToString(signature)),
            chosen: candidates.indexOf(resolved),
            parameters: candidates.map(signature => checker.getExpandedParameters(signature).map(list => list.map(parameter => api.createPrinter().printNode(api.EmitHint.Unspecified, checker.symbolToParameterDeclaration(parameter, undefined, api.NodeBuilderFlags.None), program.getSourceFile(rootNames[0]))).join(", "))),
            rest: candidates.map(signature => checker.hasEffectiveRestParameter(signature)),
            returnType: api.displayPartsToString(api.mapToDisplayParts(writer => checker.writeType(checker.getReturnTypeOfSignature(resolved), undefined, undefined, writer))),
            kinds: api.mapToDisplayParts(writer => checker.writeSymbol(resolved.declaration.symbol, undefined, undefined, undefined, writer)).map(part => part.kind),
        };
    };
    assert.deepEqual(describe(ts, native), describe(ts6, reference));
    const spread = native.getSourceFile(rootNames[0]).statements[4];
    const nativeChecker = native.getTypeChecker();
    assert.throws(() => nativeChecker.getExpandedParameters(nativeChecker.getSignatureFromDeclaration(spread)), /^Error: tsc-rs: getExpandedParameters is not supported: /);
});

// A language service over the files of `dir` with these root files, sharing `registry`.
function serviceOfRoots(api, dir, roots, options, registry) {
    const host = {
        getScriptFileNames: () => roots.map(root => path.join(dir, root)),
        getScriptVersion: () => "1",
        getScriptSnapshot: name => fs.existsSync(name) ? api.ScriptSnapshot.fromString(fs.readFileSync(name, "utf8")) : undefined,
        getCurrentDirectory: () => dir,
        getCompilationSettings: () => options,
        getDefaultLibFileName: settings => api.getDefaultLibFilePath(settings),
        fileExists: name => api.sys.fileExists(name),
        readFile: name => api.sys.readFile(name),
        readDirectory: (...args) => api.sys.readDirectory(...args),
        directoryExists: name => api.sys.directoryExists(name),
        getDirectories: name => api.sys.getDirectories(name),
    };
    return api.createLanguageService(host, registry);
}

function completionNames(service, fileName, position, preferences = {}) {
    return service.getCompletionsAtPosition(fileName, position, preferences)?.entries.map(entry => entry.source ? `${entry.name}@${entry.source}` : entry.name) ?? [];
}

test("the symbol of a declaration in a SourceFile that two language services share works in either program's checker", { skip: ts6Skip }, () => {
    const files = {
        "shared.ts": "export const sharedValue = 1;\nexport function helper() {}\n",
        "eq.d.ts": "declare const box: { boxed: number };\nexport = box;\n",
        "a.ts": "import { sharedValue } from './shared';\nsharedValue;\nhel",
        "b.ts": "import { helper } from './shared';\nhelper();\nbox",
    };
    const describe = (api, name) => {
        const dir = writeFixture(name, files);
        const registry = api.createDocumentRegistry();
        const options = { strict: true, target: api.ScriptTarget.ES2022, module: api.ModuleKind.ESNext, moduleResolution: api.ModuleResolutionKind.Bundler, types: [] };
        const serviceA = serviceOfRoots(api, dir, ["a.ts", "shared.ts"], options, registry);
        const serviceB = serviceOfRoots(api, dir, ["b.ts", "shared.ts", "eq.d.ts"], options, registry);
        const programA = serviceA.getProgram();
        const programB = serviceB.getProgram();
        const shared = programA.getSourceFile(path.join(dir, "shared.ts"));
        assert.equal(programB.getSourceFile(path.join(dir, "shared.ts")), shared, "one SourceFile for both programs");
        const checkerA = programA.getTypeChecker();
        const checkerB = programB.getTypeChecker();
        const moduleSymbol = checkerA.getMergedSymbol(shared.symbol);
        assert.equal(moduleSymbol, checkerA.getSymbolAtLocation(shared), "the module symbol of checker A");
        const helper = shared.statements[1];
        const eq = programB.getSourceFile(path.join(dir, "eq.d.ts"));
        return {
            exports: checkerA.getExportsOfModule(moduleSymbol).map(symbol => symbol.name).sort(),
            typeInA: checkerA.typeToString(checkerA.getTypeOfSymbol(helper.symbol)),
            typeInB: checkerB.typeToString(checkerB.getTypeOfSymbol(helper.symbol)),
            exportEquals: checkerB.resolveExternalModuleSymbol(checkerB.getSymbolAtLocation(eq)).name,
            exportEqualsProperty: checkerB.tryGetMemberInModuleExportsAndProperties("boxed", checkerB.getSymbolAtLocation(eq))?.name,
            autoImportsA: completionNames(serviceA, path.join(dir, "a.ts"), files["a.ts"].length, { includeCompletionsForModuleExports: true }).filter(name => name.includes("@")).sort(),
            autoImportsB: completionNames(serviceB, path.join(dir, "b.ts"), files["b.ts"].length, { includeCompletionsForModuleExports: true }).filter(name => name.includes("@")).sort(),
        };
    };
    const expected = describe(ts6, "shared-registry-ts6");
    assert.deepEqual(expected.autoImportsA.includes("helper@./shared"), true);
    assert.deepEqual(describe(ts, "shared-registry"), expected);
});

test("property accessibility follows TS 6.0: parameter properties, fields through super, this parameters and binding patterns", { skip: ts6Skip }, () => {
    const text = `class S { run() {} }
class Base { field = 1; method() {} protected pm() {} private priv() {} static protected sp = 1; #own = 1; own() { return this.#own; } }
abstract class Shape { abstract area: number; read = this.area; }
class Ctl extends Base {
    constructor(private readonly service: S, protected other: S) { super(); this.service; }
    go() { this.service.run(); this.other.run(); super.field; super.method; Base.sp; const { service } = this; this./*this*/; super./*super*/; }
}
class Sub extends Ctl { more() { this.other; this./*sub*/; } }
function withThis(this: Ctl) { this.other; this.pm; this./*thisParameter*/; }
function outside(ctl: Ctl, sub: Sub) { ctl.field; ctl.pm; ctl.service; Base.sp; sub.other; }
`;
    const describe = (api, name) => {
        const dir = writeFixture(name, { "main.ts": text });
        const fileName = path.join(dir, "main.ts");
        const service = serviceOfRoots(api, dir, ["main.ts"], { ...baseOptions, types: [] });
        const program = service.getProgram();
        const checker = program.getTypeChecker();
        const results = [];
        const visit = node => {
            if (api.isPropertyAccessExpression(node) && node.name.text !== "") {
                const enclosing = api.findAncestor(node, ancestor => api.isFunctionLike(ancestor) && ancestor.name !== undefined)?.name?.getText() ?? "class";
                results.push(`${enclosing} ${node.getText()}: ${checker.isValidPropertyAccess(node, node.name.text)}`);
            }
            if (api.isObjectBindingPattern(node)) {
                const type = checker.getTypeAtLocation(node.parent.initializer);
                results.push(`binding: ${checker.getPropertiesOfType(type).filter(property => checker.isPropertyAccessible(node, false, false, type, property)).map(property => property.name)}`);
            }
            api.forEachChild(node, visit);
        };
        visit(program.getSourceFile(fileName));
        for (const match of text.matchAll(/\/\*(\w+)\*\//g)) {
            results.push(`${match[1]}: ${completionNames(service, fileName, match.index).filter(entry => !/^(constructor|run)$/.test(entry)).sort()}`);
        }
        return results;
    };
    const expected = describe(ts6, "accessibility-ts6");
    assert.ok(expected.includes("go this.service: true") && expected.includes("go super.field: false") && expected.includes("withThis this.pm: true"));
    assert.deepEqual(describe(ts, "accessibility"), expected);
});

test("getNonOptionalType removes only the optional chain marker, so member completions work with exactOptionalPropertyTypes", { skip: ts6Skip }, () => {
    const text = `declare const opt: { p?: { q: number } };
declare const chain: { b: { c: number } } | undefined;
declare const maybe: { m?: number | undefined };
const missing = opt.p;
const marked = chain?.b.c;
const explicit = maybe.m;
opt.p./*member*/;
opt.p?./*optionalChain*/;
`;
    const describe = (api, name, options) => {
        const dir = writeFixture(name, { "main.ts": text });
        const fileName = path.join(dir, "main.ts");
        const service = serviceOfRoots(api, dir, ["main.ts"], { ...baseOptions, types: [], ...options });
        const checker = service.getProgram().getTypeChecker();
        const sourceFile = service.getProgram().getSourceFile(fileName);
        const results = [];
        const visit = node => {
            if (api.isPropertyAccessExpression(node) && node.name.text !== "") {
                const type = checker.getTypeAtLocation(node);
                results.push(`${node.getText()}: ${checker.typeToString(type)} -> ${checker.typeToString(checker.getNonOptionalType(type))}`);
            }
            api.forEachChild(node, visit);
        };
        visit(sourceFile);
        for (const match of text.matchAll(/\/\*(\w+)\*\//g)) {
            results.push(`${match[1]}: ${completionNames(service, fileName, match.index)}`);
        }
        return results;
    };
    for (const [label, options] of [["exact", { exactOptionalPropertyTypes: true }], ["strict", {}], ["loose", { strict: false }]]) {
        const expected = describe(ts6, `non-optional-${label}-ts6`, options);
        assert.ok(expected.includes("member: q"), label);
        assert.deepEqual(describe(ts, `non-optional-${label}`, options), expected, label);
    }
});

test("isNullableType answers TS 6.0's type facts: any, unknown, void and unconstrained type parameters are not nullable", { skip: ts6Skip }, () => {
    const text = `declare const any: any;
declare const unknownValue: unknown;
declare const voidValue: void;
declare const nullValue: null;
declare const undefinedValue: undefined;
declare const optional: string | undefined;
declare const neverValue: never;
declare const nonNull: (string | undefined) & {};
declare const nullOrObject: { x: 1 } & null;
declare const branded: string & { __brand: 1 };
function generic<T, U extends string | null, W extends unknown, Z extends {}, V extends T, A extends any>(t: T, u: U, w: W, z: Z, v: V, a: A, tu: T & U, un: U & null, tz: T & {}) {}
declare const a: { b: any; c: unknown; d: string | undefined } | undefined;
const b = a ? a.b : undefined;
const c = a ? a.c : undefined;
const d = a ? a.d : undefined;
`;
    const describe = (api, name) => {
        const dir = writeFixture(name, { "main.ts": text });
        const fileName = path.join(dir, "main.ts");
        const service = serviceOfRoots(api, dir, ["main.ts"], { ...baseOptions, types: [] });
        const checker = service.getProgram().getTypeChecker();
        const results = [];
        const visit = node => {
            if ((api.isVariableDeclaration(node) || api.isParameter(node)) && api.isIdentifier(node.name)) {
                const type = checker.getTypeAtLocation(node.name);
                results.push(`${node.name.text}: ${checker.typeToString(type)} ${checker.isNullableType(type)} ${type.isNullableType()}`);
            }
            if (api.isConditionalExpression(node)) {
                const refactors = service.getApplicableRefactors(fileName, { pos: node.getStart(), end: node.end }, {}).flatMap(refactor => refactor.actions.map(action => action.description));
                results.push(`${node.getText()}: ${refactors.includes("Convert to optional chain expression")}`);
            }
            api.forEachChild(node, visit);
        };
        visit(service.getProgram().getSourceFile(fileName));
        return results;
    };
    const expected = describe(ts6, "nullable-ts6");
    assert.ok(expected.includes("any: any false false") && expected.includes("a ? a.c : undefined: true"));
    assert.deepEqual(describe(ts, "nullable"), expected);
});

// React 19's shape: no global JSX; React.JSX, re-exported by react/jsx-runtime.
const jsxPackages = {
    "node_modules/react/package.json": JSON.stringify({ name: "react", version: "19.0.0", types: "index.d.ts", exports: { ".": { types: "./index.d.ts" }, "./jsx-runtime": { types: "./jsx-runtime.d.ts" } } }),
    "node_modules/react/index.d.ts": "export = React;\nexport as namespace React;\ndeclare namespace React { namespace JSX { interface Element {} interface IntrinsicElements { div: {}; span: {} } } }\n",
    "node_modules/react/jsx-runtime.d.ts": "export { JSX } from \"./\";\nexport function jsx(): any;\nexport function jsxs(): any;\n",
    "node_modules/preact/package.json": JSON.stringify({ name: "preact", version: "10.0.0", exports: { "./jsx-runtime": { types: "./jsx-runtime.d.ts" } } }),
    "node_modules/preact/jsx-runtime.d.ts": "export namespace JSX { interface Element {} interface IntrinsicElements { pre: {} } }\nexport function jsx(): any;\n",
    "h.d.ts": "export declare function h(): any;\nexport declare namespace h { namespace JSX { interface Element {} interface IntrinsicElements { p: {} } } }\n",
};

test("getJsxIntrinsicTagNamesAt reads the JSX namespace of the jsx-runtime module, of the factory's namespace or the global one", { skip: ts6Skip }, () => {
    const jsx = { module: 99, moduleResolution: 100, types: [] };
    const cases = [
        ["react-jsx", { ...jsxPackages, "main.tsx": "export const el = <div />;\n" }, { ...jsx, jsx: 4 }],
        ["import source pragma", { ...jsxPackages, "main.tsx": "/** @jsxImportSource preact */\nexport const el = <pre />;\n" }, { ...jsx, jsx: 4 }],
        ["React namespace", { ...jsxPackages, "main.tsx": "import * as React from \"react\";\nexport const el = <div />;\nexport const fragment = <></>;\n" }, { ...jsx, jsx: 2 }],
        ["jsx pragma", { ...jsxPackages, "main.tsx": "/** @jsx h */\nimport { h } from \"./h\";\nexport const el = <p />;\n" }, { ...jsx, jsx: 2 }],
        ["jsxFactory", { ...jsxPackages, "main.tsx": "import { h } from \"./h\";\nexport const el = <p />;\n" }, { ...jsx, jsx: 2, jsxFactory: "h" }],
        ["global", { "main.tsx": "export const el = <g />;\ndeclare global { namespace JSX { interface Element {} interface IntrinsicElements { g: {} } } }\n" }, { ...jsx, jsx: 1 }],
    ];
    for (const [label, files, options] of cases) {
        const { rootNames, reference, native } = createPrograms(`jsx-${label.replaceAll(" ", "-")}`, files, options);
        const describe = (api, program) => {
            const checker = program.getTypeChecker();
            const sourceFile = program.getSourceFile(rootNames.find(name => name.endsWith("main.tsx")));
            const names = [];
            const visit = node => {
                if (api.isJsxSelfClosingElement(node) || api.isJsxOpeningFragment(node)) {
                    names.push(checker.getJsxIntrinsicTagNamesAt(node).map(symbol => symbol.name).join(","));
                }
                api.forEachChild(node, visit);
            };
            visit(sourceFile);
            return names;
        };
        const expected = describe(ts6, reference);
        assert.ok(expected.every(names => names !== ""), label);
        assert.deepEqual(describe(ts, native), expected, label);
    }
    const dir = writeFixture("jsx-completions", { ...jsxPackages, "main.tsx": "export const el = <d" });
    const complete = api => completionNames(serviceOfRoots(api, dir, ["main.tsx"], { ...baseOptions, ...jsx, jsx: 4 }), path.join(dir, "main.tsx"), "export const el = <d".length).filter(name => /^(div|span)$/.test(name));
    assert.deepEqual(complete(ts), complete(ts6));
    assert.deepEqual(complete(ts), ["div", "span"]);
});

test("symbolToString, symbolToEntityName, symbolToExpression and display parts write late-bound and quoted member names like TS 6.0", { skip: ts6Skip }, () => {
    const { rootNames, reference, native } = createPrograms("member-names", {
        "main.ts": `declare const arr: string[];
const it = arr[Symbol.iterator];
const s1: unique symbol = Symbol();
class K { [s1] = 1; [Symbol.asyncIterator]() { return null as any; } "a.b" = 2; 0 = 3; 1.5 = 4; "it's" = 5; "\\u00e9" = 6; plain = 7; }
declare const k: K;
enum E { "a b" = 1, c = 2 }
namespace N { export const inner = { [s1]: 1 }; }
`,
    });
    const describe = (api, program) => {
        const checker = program.getTypeChecker();
        const sourceFile = program.getSourceFile(rootNames[0]);
        const printer = api.createPrinter({ removeComments: true });
        const print = node => node === undefined ? "none" : printer.printNode(api.EmitHint.Unspecified, node, sourceFile);
        const parts = (symbol, enclosing, flags) => api.symbolToDisplayParts(checker, symbol, enclosing, undefined, flags).map(part => `${part.text}:${part.kind}`).join("|");
        const typeOf = name => checker.getTypeAtLocation(sourceFile.statements.find(statement => api.isVariableStatement(statement) && statement.declarationList.declarations[0].name.text === name).declarationList.declarations[0].name);
        const enumSymbol = checker.getSymbolAtLocation(sourceFile.statements[5].name);
        const inner = checker.getSymbolAtLocation(sourceFile.statements[6].name).exports.get("inner");
        const symbols = [
            checker.getPropertiesOfType(typeOf("arr")).find(property => property.escapedName.startsWith("__@iterator")),
            ...checker.getPropertiesOfType(typeOf("k")),
            ...enumSymbol.exports.values(),
            inner,
            ...checker.getPropertiesOfType(checker.getTypeOfSymbol(inner)),
        ];
        const enclosing = sourceFile.statements[1];
        return symbols.map(symbol => [
            checker.symbolToString(symbol),
            checker.symbolToString(symbol, enclosing),
            checker.symbolToString(symbol, sourceFile),
            checker.symbolToString(symbol, enclosing, undefined, api.SymbolFormatFlags.None),
            checker.getFullyQualifiedName(symbol),
            print(checker.symbolToEntityName(symbol, api.SymbolFlags.Value, enclosing)),
            print(checker.symbolToEntityName(symbol, api.SymbolFlags.Value, undefined)),
            print(checker.symbolToExpression(symbol, api.SymbolFlags.Value, enclosing)),
            parts(symbol, enclosing),
            parts(symbol, enclosing, api.SymbolFormatFlags.AllowAnyNodeKind),
        ].join("  ").replace(/@\d+/g, "@N"));
    };
    const expected = describe(ts6, reference);
    assert.ok(expected[0].startsWith("[Symbol.iterator]  Array[Symbol.iterator]"), expected[0]);
    assert.deepEqual(describe(ts, native), expected);
});
