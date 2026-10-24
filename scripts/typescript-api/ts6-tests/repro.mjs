// Minimal repros of the triage clusters (README.md "Triage"): each case runs one tiny program through
// the package, through stock typescript@6.0.3 and through tsc-rs's own command line (tsgo).
// usage: node repro.mjs help | list | <case>... | all [--pkg DIR] [--ts6 DIR] [--tsgo BIN]
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import { createRequire } from "node:module";
import path from "node:path";
import { fileURLToPath } from "node:url";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const REPO = path.resolve(HERE, "../../..");
const FIXTURES = path.join(REPO, "target/typescript-api/ts6-repro");
const require = createRequire(import.meta.url);

const USAGE = `usage: node scripts/typescript-api/ts6-tests/repro.mjs help | list | <case>... | all
           [--pkg DIR] [--ts6 DIR] [--tsgo BIN]

Runs each case through the package (--pkg, default npm/typescript-api), stock typescript@6.0.3
(--ts6, default npm/typescript-api/.cache/typescript-6.0.3, which npm/typescript-api/scripts/build.mjs
unpacks) and the tsgo command line (--tsgo, default target/release/tsgo), and prints the
diagnostics and declaration files of each. Fixture files are written to
${path.relative(REPO, FIXTURES)}/<case>/. "list" prints the cases with the triage cluster and the
expected (TS 7) answer.`;

// host: "disk" (ts.createCompilerHost), "memory" (an in-memory CompilerHost under /ts6-repro-virtual/,
// paths that do not exist on disk, like the TS 6.0 harness's virtual file system), "other-cwd" (disk,
// relative names resolved against host.getCurrentDirectory(), which is the fixture directory, while
// the process runs elsewhere). `emitFirst` emits before asking for diagnostics, as
// both test harnesses do.
const CASES = {
    "cwd": {
        cluster: "host current directory ignored (program)",
        expect: "no error; usage.d.ts has import(\"url\").Url (TS 7 baseline referenceTypesPreferedToPathIfPossible)",
        host: "other-cwd",
        files: {
            "node_modules/@types/node/index.d.ts": "declare module \"url\" {\n    export class Url {}\n    export function parse(): Url;\n}\n",
            "usage.ts": "import { parse } from \"url\";\nexport const thing = () => parse();\n",
        },
        roots: ["usage.ts"],
        options: { types: ["*"], declaration: true, module: "commonjs", target: "es2015" },
        tsgo: ["usage.ts", "--types", "*", "--declaration", "--module", "commonjs", "--target", "es2015"],
    },
    "case-insensitive": {
        cluster: "host useCaseSensitiveFileNames ignored (program)",
        expect: "no error, outputs A:/bar.js and A:/baz.js (TS 7 baseline commonSourceDir3)",
        host: "memory",
        caseSensitive: false,
        files: { "A:/foo/bar.ts": "var x: number;\n", "a:/foo/baz.ts": "var y: number;\n" },
        roots: ["A:/foo/bar.ts", "a:/foo/baz.ts"],
        options: { target: "es2015", outDir: "A:/" },
        showJs: true,
    },
    "specifier-exports": {
        cluster: "declaration emit ignores package.json of a host file system (program/emit)",
        expect: "TS2307 for inner/other.js; index.d.ts has import(\"inner/other\").Thing (TS 7 baseline nodeModulesExportsSpecifierGenerationConditions)",
        host: "memory",
        files: {
            "index.ts": "import { Thing } from \"inner/other.js\";\nexport const a = (await import(\"inner\")).x();\n",
            "node_modules/inner/index.d.ts": "export { x } from \"./other.js\";\n",
            "node_modules/inner/other.d.ts": "export interface Thing {}\nexport const x: () => Thing;\n",
            "package.json": "{ \"name\": \"package\", \"private\": true, \"type\": \"module\", \"exports\": \"./index.js\" }\n",
            "node_modules/inner/package.json": "{ \"name\": \"inner\", \"private\": true, \"type\": \"module\", \"exports\": { \".\": { \"default\": \"./index.js\" }, \"./other\": { \"default\": \"./other.js\" } } }\n",
        },
        roots: ["index.ts"],
        options: { target: "es2022", module: "node16", declaration: true },
        tsgo: ["index.ts", "--target", "es2022", "--module", "node16", "--declaration"],
        alsoOnDisk: true,
    },
    "dts-despite-errors": {
        cluster: "declaration emit ignores package.json of a host file system (program/emit)",
        expect: "TS2883 twice and no src/index.d.ts (TS 7 baseline declarationEmitUsingTypeAlias1)",
        host: "memory",
        files: {
            "node_modules/some-dep/dist/inner.d.ts": "export declare type Other = { other: string };\nexport declare type SomeType = { arg: Other };\n",
            "node_modules/some-dep/dist/index.d.ts": "export type OtherType = import('./inner').Other;\nexport type SomeType = import('./inner').SomeType;\n",
            "node_modules/some-dep/package.json": "{ \"name\": \"some-dep\", \"exports\": { \".\": \"./dist/index.js\" } }\n",
            "src/index.ts": "import { SomeType } from \"some-dep\";\n\nexport const foo = (thing: SomeType) => {\n  return thing;\n};\n\nexport const bar = (thing: SomeType) => {\n  return thing.arg;\n};\n",
        },
        roots: ["src/index.ts"],
        options: { target: "es2015", module: "nodenext", strict: true, declaration: true },
        tsgo: ["src/index.ts", "--target", "es2015", "--module", "nodenext", "--strict", "--declaration"],
        alsoOnDisk: true,
    },
    "circularity-location": {
        cluster: "check-order dependent circularity diagnostics (checker)",
        expect: "TS5114 on `this.a` in m2, line 13 col 9 here (TS 7 baseline mutuallyRecursiveInference, line 12 there)",
        host: "disk",
        emitFirst: true,
        files: {
            "a.ts": "// The test case without its directive line.\nclass T<A> {\n    a: A;\n    b: any\n}\nclass L<RT extends { a: 'a' | 'b', b: any }> extends T<RT[RT['a']]> {\n    m() { this.a }\n}\nclass X extends L<X> {\n    a: 'a' | 'b'\n    b: number\n    m2() {\n        this.a\n    }\n}\n",
        },
        roots: ["a.ts"],
        options: { target: "es2015", strict: true },
        tsgo: ["a.ts", "--target", "es2015", "--strict"],
    },
    "paths-5090": {
        cluster: "TS5090 diagnostics collapse to one, TS 6.0 text (options-diagnostics)",
        expect: "three TS5090 'Non-relative paths are not allowed. Did you forget a leading './'?' in tsconfig.json (TS 7 baseline pathsValidation5)",
        host: "disk",
        project: "tsconfig.json",
        files: {
            "tsconfig.json": "{\n  \"compilerOptions\": {\n    \"noEmit\": true,\n    \"paths\": {\n      \"@interface/*\": [\"src/interface/*\"],\n      \"@blah\": [\"blah\"],\n      \"@humbug/*\": [\"*/generated\"]\n    }\n  }\n}\n",
            "src/main.ts": "export {};\n",
        },
        tsgo: ["-p", "tsconfig.json"],
    },
    "related-2751": {
        cluster: "check-order dependent circularity diagnostics (checker)",
        expect: "TS2313 with related TS2751 'Circularity originates in type at this location.' at 4:17 (TS 7 baseline typeParameterWithInvalidConstraintType)",
        host: "disk",
        emitFirst: true,
        files: {
            "a.ts": "class A<T extends T> {\n    foo() {\n        var x!: T;\n        var a = x.foo();\n        var b = new x(123);\n        var c = x[1];\n        var d = x();\n    }\n}\n",
        },
        roots: ["a.ts"],
        options: { target: "es2015", strict: false },
        tsgo: ["a.ts", "--target", "es2015", "--strict", "false", "--pretty", "true"],
    },
};

function parseArgs(args) {
    const options = {
        pkg: path.join(REPO, "npm/typescript-api"),
        ts6: path.join(REPO, "npm/typescript-api/.cache/typescript-6.0.3"),
        tsgo: path.join(REPO, "target/release/tsgo"),
        cases: [],
    };
    for (let i = 0; i < args.length; i++) {
        if (["--pkg", "--ts6", "--tsgo"].includes(args[i])) {
            options[args[i].slice(2)] = path.resolve(args[++i]);
        }
        else {
            options.cases.push(args[i]);
        }
    }
    return options;
}

function writeFixture(name, testCase) {
    const dir = path.join(FIXTURES, name);
    fs.rmSync(dir, { recursive: true, force: true });
    for (const [file, text] of Object.entries(testCase.files)) {
        if (/^[A-Za-z]:\//.test(file)) {
            continue;
        }
        fs.mkdirSync(path.dirname(path.join(dir, file)), { recursive: true });
        fs.writeFileSync(path.join(dir, file), text);
    }
    fs.mkdirSync(dir, { recursive: true });
    return dir;
}

// ---- child: one library on one case ----

function runChild(library, name, host) {
    const ts = require(path.join(library, "lib/typescript.js"));
    const testCase = CASES[name];
    const dir = host === "memory" ? `/ts6-repro-virtual/${name}` : path.join(FIXTURES, name);
    const show = text => text.split(dir).join("<dir>");
    const written = {};
    const memory = new Map(Object.entries(testCase.files).map(([file, text]) => [/^[A-Za-z]:\//.test(file) ? file : path.join(dir, file), text]));
    const key = file => (testCase.caseSensitive === false ? file.toLowerCase() : file);
    const memoryKeys = new Map([...memory.keys()].map(file => [key(file), file]));
    const read = file => {
        const own = memoryKeys.get(key(file));
        return own !== undefined ? memory.get(own) : file.includes("/node_modules/typescript/") || file.includes("/lib/lib.") ? ts.sys.readFile(file) : undefined;
    };
    let compilerHost;
    let parsed = { options: {}, fileNames: [], errors: [] };
    if (testCase.project) {
        const configPath = path.join(dir, testCase.project);
        const configFile = ts.readJsonConfigFile(configPath, ts.sys.readFile);
        parsed = ts.parseJsonSourceFileConfigFileContent(configFile, ts.sys, dir, undefined, configPath);
    }
    else {
        parsed.options = ts.convertCompilerOptionsFromJson(testCase.options, dir).options;
        parsed.fileNames = testCase.roots.map(root => (/^[A-Za-z]:\//.test(root) ? root : path.join(dir, root)));
    }
    if (host === "memory") {
        compilerHost = {
            getSourceFile: (file, languageVersion) => {
                const text = read(file);
                return text === undefined ? undefined : ts.createSourceFile(file, text, languageVersion);
            },
            getDefaultLibFileName: options => ts.getDefaultLibFilePath(options),
            writeFile: (file, text) => {
                written[file] = text;
            },
            getCurrentDirectory: () => (testCase.caseSensitive === false ? "A:/" : dir),
            getCanonicalFileName: key,
            useCaseSensitiveFileNames: () => testCase.caseSensitive !== false,
            getNewLine: () => "\n",
            fileExists: file => read(file) !== undefined,
            readFile: read,
            directoryExists: directory => [...memoryKeys.keys()].some(file => file.startsWith(key(directory.replace(/\/?$/, "/")))) || ts.sys.directoryExists(directory),
            getDirectories: () => [],
        };
        parsed.options.outDir ??= path.join(dir, "out");
    }
    else {
        compilerHost = ts.createCompilerHost(parsed.options);
        compilerHost.writeFile = (file, text) => {
            written[file] = text;
        };
        if (host === "other-cwd") {
            // Like the TS 6.0 harness's file system: relative names are relative to the host's directory.
            const absolute = file => path.resolve(dir, file);
            for (const method of ["getSourceFile", "fileExists", "readFile", "directoryExists", "getDirectories", "realpath"]) {
                const original = compilerHost[method];
                compilerHost[method] = (file, ...rest) => original(absolute(file), ...rest);
            }
            compilerHost.getCurrentDirectory = () => dir;
            parsed.fileNames = testCase.roots;
        }
        parsed.options.outDir ??= path.join(dir, "out");
    }
    const program = ts.createProgram({ rootNames: parsed.fileNames, options: parsed.options, host: compilerHost, configFileParsingDiagnostics: parsed.errors });
    if (testCase.emitFirst) {
        program.emit();
    }
    const where = diagnostic => {
        if (!diagnostic.file) {
            return "";
        }
        const { line, character } = diagnostic.file.getLineAndCharacterOfPosition(diagnostic.start);
        return `${show(diagnostic.file.fileName)}(${line + 1},${character + 1}): `;
    };
    for (const diagnostic of ts.getPreEmitDiagnostics(program)) {
        console.log(`${where(diagnostic)}error TS${diagnostic.code}: ${ts.flattenDiagnosticMessageText(diagnostic.messageText, " ")}`);
        for (const related of diagnostic.relatedInformation ?? []) {
            console.log(`  related ${where(related)}TS${related.code}: ${ts.flattenDiagnosticMessageText(related.messageText, " ")}`);
        }
    }
    if (!testCase.emitFirst) {
        program.emit();
    }
    for (const [file, text] of Object.entries(written)) {
        if (/\.d\.[cm]?ts$/.test(file) || testCase.showJs) {
            console.log(`--- ${show(file)}\n${text.trimEnd()}`);
        }
    }
}

// ---- parent ----

function runEngine(label, command, args, cwd) {
    console.log(`  == ${label}`);
    const result = spawnSync(command, args, { cwd, encoding: "utf8", timeout: 120000 });
    const output = `${result.stdout ?? ""}${result.stderr ?? ""}`.replace(/\u001b\[[0-9;]*m/g, "").trimEnd();
    console.log(output.split("\n").map(line => `  ${line}`).join("\n") || "  (no output)");
    if (result.status !== 0 && result.status !== 1 && result.status !== 2) {
        console.log(`  (exit ${result.status ?? result.signal})`);
    }
}

function runCase(name, options) {
    const testCase = CASES[name];
    const dir = writeFixture(name, testCase);
    console.log(`### ${name}: ${testCase.cluster}\n  TS 7: ${testCase.expect}`);
    const self = fileURLToPath(import.meta.url);
    const otherCwd = path.dirname(FIXTURES);
    runEngine(`package ${path.relative(REPO, options.pkg) || options.pkg} (${testCase.host} host)`, process.execPath, [self, "--child", options.pkg, name, testCase.host], otherCwd);
    if (testCase.alsoOnDisk) {
        runEngine(`package ${path.relative(REPO, options.pkg) || options.pkg} (disk host, same files)`, process.execPath, [self, "--child", options.pkg, name, "disk"], otherCwd);
    }
    runEngine(`typescript@6.0.3 (${testCase.host} host)`, process.execPath, [self, "--child", options.ts6, name, testCase.host], otherCwd);
    if (testCase.tsgo === undefined) {
        console.log("  == tsgo: not applicable (needs a case-insensitive file system)\n");
        return;
    }
    const out = path.join(dir, "out-tsgo");
    const args = testCase.project ? testCase.tsgo : [...testCase.tsgo, "--outDir", out];
    runEngine(`tsgo ${args.join(" ")}`, options.tsgo, args, dir);
    if (fs.existsSync(out)) {
        for (const file of fs.readdirSync(out, { recursive: true })) {
            if (/\.d\.[cm]?ts$/.test(file)) {
                console.log(`  --- <dir>/out-tsgo/${file}\n${fs.readFileSync(path.join(out, file), "utf8").trimEnd().split("\n").map(line => `  ${line}`).join("\n")}`);
            }
        }
    }
    console.log("");
}

const args = process.argv.slice(2);
if (args[0] === "--child") {
    runChild(args[1], args[2], args[3]);
}
else {
    const options = parseArgs(args);
    if (options.cases.length === 0 || options.cases[0] === "help") {
        console.log(USAGE);
    }
    else if (options.cases[0] === "list") {
        for (const [name, testCase] of Object.entries(CASES)) {
            console.log(`${name}\n    ${testCase.cluster}\n    TS 7: ${testCase.expect}`);
        }
    }
    else {
        const names = options.cases[0] === "all" ? Object.keys(CASES) : options.cases;
        for (const name of names) {
            if (!CASES[name]) {
                console.error(`unknown case ${name}; run "list"`);
                process.exit(2);
            }
            runCase(name, options);
        }
    }
}
