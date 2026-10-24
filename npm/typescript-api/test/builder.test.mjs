// Builder programs, incremental programs, TS 6.0's watch API and tsc -b (src/builder.js) over tsc-rs,
// checked against stock typescript@6.0.3: the copy scripts/build.mjs unpacks (TS6_STOCK names
// another, test/support.mjs).
// Run: cd npm/typescript-api && node --test test/builder.test.mjs
// TSRS_API_CHANNEL=process runs tsc-rs as a `tsgo --api` child process when the addon is not built.

import assert from "node:assert/strict";
import fs from "node:fs";
import { createRequire } from "node:module";
import os from "node:os";
import path from "node:path";
import { after, test } from "node:test";

import { stockMain, stockSkip } from "./support.mjs";

const require = createRequire(import.meta.url);
const ts = require("../lib/typescript.js");
const { NativeProgram } = require("../src/program.js");
const ts6 = stockMain === undefined ? undefined : require(stockMain);
const needsStock = { skip: stockSkip };

const temporaryDirectories = [];
after(() => {
    for (const dir of temporaryDirectories) {
        fs.rmSync(dir, { recursive: true, force: true });
    }
});

// A project in a new temporary directory. Names are relative to it and use "/".
function project(files) {
    const dir = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "tsrs-builder-"))).replaceAll("\\", "/");
    temporaryDirectories.push(dir);
    const file = name => `${dir}/${name}`;
    // A null text deletes the file.
    const write = (name, text) => {
        if (text === null) {
            fs.rmSync(file(name));
            return;
        }
        fs.mkdirSync(path.dirname(file(name)), { recursive: true });
        fs.writeFileSync(file(name), text);
    };
    for (const [name, text] of Object.entries(files)) {
        write(name, text);
    }
    return {
        dir,
        file,
        write,
        read: name => fs.readFileSync(file(name), "utf8"),
        exists: name => fs.existsSync(file(name)),
        // Default library files of either package compare by name.
        relative: fileName => fileName.startsWith(`${dir}/`) ? fileName.slice(dir.length + 1) : `lib/${path.basename(fileName)}`,
        outputs: (folder = "out") => listFiles(file(folder)).map(name => name.slice(dir.length + 1)).sort(),
    };
}

function listFiles(dir) {
    if (!fs.existsSync(dir)) {
        return [];
    }
    return fs.readdirSync(dir, { withFileTypes: true }).flatMap(entry =>
        entry.isDirectory() ? listFiles(`${dir}/${entry.name}`) : [`${dir}/${entry.name}`]);
}

// Polls `condition` until it holds; fails after `timeout` ms.
async function waitFor(condition, description, timeout = 5000) {
    const start = Date.now();
    while (!condition()) {
        if (Date.now() - start > timeout) {
            assert.fail(`timed out after ${timeout} ms waiting for ${description}`);
        }
        await new Promise(resolve => setTimeout(resolve, 20));
    }
}

const chain = {
    "src/a.ts": "import { b } from \"./b\";\nexport const a: number = b + 1;\n",
    "src/b.ts": "import { c } from \"./c\";\nexport const b = c;\n",
    "src/c.ts": "export const c = 1;\n",
    "src/d.ts": "export const d: string = \"d\";\n",
};
const chainRoots = ["src/a.ts", "src/b.ts", "src/c.ts", "src/d.ts"];

function chainOptions(lib, p, extra = {}) {
    return {
        outDir: p.file("out"),
        rootDir: p.file("src"),
        declaration: true,
        strict: true,
        target: lib.ScriptTarget.ES2022,
        module: lib.ModuleKind.CommonJS,
        ...extra,
    };
}

// One build step per edit: a builder made from the last one, its affected files in order (with
// their semantic diagnostic codes) and the outputs that its emit wrote.
function builderSteps(lib, edits, { kind = "emit", extra, files = chain, roots = chainRoots, freshHosts = false } = {}) {
    const p = project(files);
    const options = chainOptions(lib, p, extra);
    let host = lib.createIncrementalCompilerHost(options);
    const create = kind === "emit" ? lib.createEmitAndSemanticDiagnosticsBuilderProgram : lib.createSemanticDiagnosticsBuilderProgram;
    const steps = [];
    let builder;
    for (const edit of [{}, ...edits]) {
        for (const [name, text] of Object.entries(edit)) {
            p.write(name, text);
        }
        // An incremental compiler host caches declaration and JSON files.
        host = freshHosts ? lib.createIncrementalCompilerHost(options) : host;
        builder = create(roots.filter(p.exists).map(p.file), options, host, builder);
        const affected = [];
        let next;
        while ((next = builder.getSemanticDiagnosticsOfNextAffectedFile())) {
            affected.push(`${p.relative(next.affected.fileName)} ${next.result.map(d => d.code).join(",")}`.trim());
        }
        const written = [];
        const result = builder.emit(undefined, fileName => written.push(p.relative(fileName)));
        steps.push({
            affected,
            written: written.sort(),
            emitSkipped: result.emitSkipped,
            all: builder.getSemanticDiagnostics().map(d => `${p.relative(d.file.fileName)} ${d.code}`),
            changedEmitSignature: builder.hasChangedEmitSignature?.(),
        });
    }
    return steps;
}

test("a semantic diagnostics builder reports each affected file once and then undefined, as TS 6.0 does", needsStock, () => {
    const edits = [{ "src/c.ts": "export const c = \"x\";\n" }];
    assert.deepEqual(builderSteps(ts, edits, { kind: "semantic" }), builderSteps(ts6, edits, { kind: "semantic" }));
});

test("an edit that changes a file's declarations affects the files that import it, as in TS 6.0", needsStock, () => {
    const edits = [{ "src/c.ts": "export const c = \"x\";\n" }, { "src/c.ts": "export const c = 1;\n" }];
    const steps = builderSteps(ts, edits);
    assert.deepEqual(steps[1].affected, ["src/c.ts", "src/b.ts", "src/a.ts 2322"]);
    assert.deepEqual(steps, builderSteps(ts6, edits));
});

test("an edit that keeps a file's declarations affects only that file, as in TS 6.0", needsStock, () => {
    const edits = [{ "src/c.ts": "export const c = 1;\n" }, { "src/c.ts": "export const c = 1;\n// a comment\n" }];
    const steps = builderSteps(ts, edits);
    assert.deepEqual(steps[2].affected, ["src/c.ts"]);
    assert.deepEqual(steps[2].written, ["out/c.d.ts", "out/c.js"]);
    assert.deepEqual(steps, builderSteps(ts6, edits));
});

test("composite builders say whether declaration output changed, as TS 6.0 does", needsStock, () => {
    const edits = [{ "src/c.ts": "export const c = 1;\n// a comment\n" }, { "src/c.ts": "export const c = \"x\";\n" }];
    const steps = builderSteps(ts, edits, { extra: { composite: true } });
    assert.deepEqual(steps.map(step => step.changedEmitSignature), [true, false, true]);
    assert.deepEqual(steps.map(step => step.changedEmitSignature), builderSteps(ts6, edits, { extra: { composite: true } }).map(step => step.changedEmitSignature));
});

test("deleting an imported file affects the files that imported it, as in TS 6.0", needsStock, () => {
    const files = { "src/a.ts": "import { b } from \"./b\";\nexport const a = b;\n", "src/b.ts": "export const b = 1;\n", "src/c.ts": "export const c = 1;\n" };
    const options = { files, roots: ["src/a.ts", "src/b.ts", "src/c.ts"] };
    const steps = builderSteps(ts, [{ "src/b.ts": null }], options);
    assert.deepEqual(steps[1].affected.filter(entry => entry.startsWith("src/")), ["src/a.ts 2307"]);
    assert.deepEqual(steps, builderSteps(ts6, [{ "src/b.ts": null }], options));
});

test("a change to a global script or a global augmentation affects every file, as in TS 6.0", needsStock, () => {
    const files = {
        "src/a.ts": "export const a: number = g + gg;\n",
        "src/g.ts": "declare var g: number;\n",
        "src/h.ts": "export {};\ndeclare global { var gg: number; }\n",
        "src/c.ts": "export const c: number = 1;\n",
    };
    const options = { files, roots: ["src/a.ts", "src/g.ts", "src/h.ts", "src/c.ts"] };
    const edits = [{ "src/g.ts": "declare var g: string;\n" }, { "src/h.ts": "export {};\ndeclare global { var gg: number; var hh: string; }\n" }];
    const steps = builderSteps(ts, edits, options);
    assert.deepEqual(steps[1].affected, ["src/g.ts", "src/a.ts 2322", "src/h.ts", "src/c.ts"]);
    assert.deepEqual(steps, builderSteps(ts6, edits, options));
});

test("a const enum change re-emits the JavaScript of the files that import it, as in TS 6.0", needsStock, () => {
    const files = { "src/a.ts": "import { E } from \"./e\";\nexport const a = E.X;\n", "src/e.ts": "export const enum E { X = 1 }\n", "src/c.ts": "export const c = 1;\n" };
    const options = { files, roots: ["src/a.ts", "src/e.ts", "src/c.ts"] };
    const steps = builderSteps(ts, [{ "src/e.ts": "export const enum E { X = 2 }\n" }], options);
    assert.deepEqual(steps[1].written, ["out/a.d.ts", "out/a.js", "out/e.d.ts", "out/e.js"]);
    assert.deepEqual(steps, builderSteps(ts6, [{ "src/e.ts": "export const enum E { X = 2 }\n" }], options));
});

test("JSON modules and dynamic imports count as references, as in TS 6.0", needsStock, () => {
    const files = {
        "src/a.ts": "import data from \"./d.json\";\nexport const a: number = data.x;\n",
        "src/d.json": "{ \"x\": 1 }\n",
        "src/b.ts": "export async function f() { const m = await import(\"./c\"); const n: number = m.c; return n; }\n",
        "src/c.ts": "export const c = 1;\n",
    };
    const options = {
        files,
        roots: ["src/a.ts", "src/b.ts", "src/c.ts"],
        extra: { resolveJsonModule: true, esModuleInterop: true, module: 99, moduleResolution: 100 },
        freshHosts: true,
    };
    const edits = [{ "src/d.json": "{ \"x\": \"s\" }\n", "src/c.ts": "export const c = \"x\";\n" }];
    const steps = builderSteps(ts, edits, options);
    assert.deepEqual(steps[1].affected.filter(entry => entry.startsWith("src/")).sort(), ["src/a.ts 2322", "src/b.ts 2322", "src/c.ts", "src/d.json"]);
    assert.deepEqual(steps, builderSteps(ts6, edits, options));
});

test("emitNextAffectedFile emits one affected file per call and then returns undefined", needsStock, () => {
    const run = lib => {
        const p = project(chain);
        const options = chainOptions(lib, p);
        const host = lib.createIncrementalCompilerHost(options);
        const roots = chainRoots.map(p.file);
        const first = lib.createEmitAndSemanticDiagnosticsBuilderProgram(roots, options, host);
        first.emit();
        p.write("src/c.ts", "export const c = \"x\";\n");
        const builder = lib.createEmitAndSemanticDiagnosticsBuilderProgram(roots, options, host, first);
        const results = [];
        let next;
        while ((next = builder.emitNextAffectedFile(() => {}))) {
            results.push(`${next.affected === builder.getProgram() ? "program" : p.relative(next.affected.fileName)} ${next.result.emitSkipped}`);
        }
        return results;
    };
    const results = run(ts);
    // TS 6.0 ends with the build info step, for the program.
    assert.deepEqual(results, ["src/c.ts false", "src/b.ts false", "src/a.ts false", "program false"]);
    assert.deepEqual(results, run(ts6));
});

test("the first emit of a builder emits the whole program in one tsc-rs request", () => {
    const p = project(chain);
    const options = chainOptions(ts, p);
    const builder = ts.createEmitAndSemanticDiagnosticsBuilderProgram(chainRoots.map(p.file), options, ts.createIncrementalCompilerHost(options));
    const program = builder.getProgram();
    const emitCalls = [];
    const emit = program.emit.bind(program);
    program.emit = (targetSourceFile, ...rest) => {
        emitCalls.push(targetSourceFile === undefined ? "program" : p.relative(targetSourceFile.fileName));
        return emit(targetSourceFile, ...rest);
    };
    const result = builder.emit();
    assert.deepEqual(emitCalls, ["program"]);
    assert.equal(result.emitSkipped, false);
    assert.deepEqual(p.outputs(), ["out/a.d.ts", "out/a.js", "out/b.d.ts", "out/b.js", "out/c.d.ts", "out/c.js", "out/d.d.ts", "out/d.js"]);
    assert.equal(builder.emitNextAffectedFile(), undefined);
});

test("builder emit hands custom transformers to the program's emit", () => {
    const p = project(chain);
    const options = chainOptions(ts, p);
    const builder = ts.createEmitAndSemanticDiagnosticsBuilderProgram(chainRoots.map(p.file), options, ts.createIncrementalCompilerHost(options));
    const transformers = { before: [() => sourceFile => sourceFile], after: [], afterDeclarations: [] };
    const received = [];
    const program = builder.getProgram();
    const emit = program.emit.bind(program);
    program.emit = (targetSourceFile, writeFile, cancellationToken, emitOnlyDtsFiles, customTransformers) => {
        received.push(customTransformers);
        return emit(targetSourceFile, writeFile, cancellationToken, emitOnlyDtsFiles, customTransformers);
    };
    builder.emit(undefined, undefined, undefined, undefined, transformers);
    assert.ok(received.length > 0);
    assert.ok(received.every(customTransformers => customTransformers === transformers));
});

test("a builder without declaration output writes no declaration files", () => {
    const p = project(chain);
    const options = chainOptions(ts, p, { declaration: false });
    const host = ts.createIncrementalCompilerHost(options);
    const roots = chainRoots.map(p.file);
    const first = ts.createEmitAndSemanticDiagnosticsBuilderProgram(roots, options, host);
    first.emit();
    p.write("src/c.ts", "export const c = \"x\";\n");
    const written = [];
    ts.createEmitAndSemanticDiagnosticsBuilderProgram(roots, options, host, first).emit(undefined, fileName => written.push(p.relative(fileName)));
    assert.deepEqual(written.sort(), ["out/a.js", "out/b.js", "out/c.js"]);
    assert.deepEqual(p.outputs(), ["out/a.js", "out/b.js", "out/c.js", "out/d.js"]);
});

test("noEmit makes a builder write nothing while it goes through the affected files, as in TS 6.0", needsStock, () => {
    const run = lib => {
        const p = project(chain);
        const options = chainOptions(lib, p, { noEmit: true });
        const builder = lib.createEmitAndSemanticDiagnosticsBuilderProgram(chainRoots.map(p.file), options, lib.createIncrementalCompilerHost(options));
        const written = [];
        const result = builder.emit(undefined, fileName => written.push(fileName));
        const steps = [];
        let next;
        while ((next = builder.emitNextAffectedFile(fileName => written.push(fileName)))) {
            steps.push(`${next.affected === builder.getProgram() ? "program" : p.relative(next.affected.fileName)} ${next.result.emitSkipped}`);
        }
        return { emitSkipped: result.emitSkipped, diagnostics: result.diagnostics.length, steps, written, outputs: p.outputs() };
    };
    const result = run(ts);
    assert.deepEqual(result.written, []);
    assert.deepEqual(result.steps.slice(0, 4), ["src/c.ts true", "src/b.ts true", "src/a.ts true", "src/d.ts true"]);
    assert.deepEqual(result, run(ts6));
});

test("getAllDependencies lists a file and what it imports, transitively, as TS 6.0 does", needsStock, () => {
    const run = lib => {
        const p = project({
            ...chain,
            "src/e.ts": "export type E = import(\"./d\").D;\n",
            "src/d.ts": "export interface D { x: number }\n",
            "src/f.ts": "/// <reference path=\"./g.d.ts\" />\nexport * from \"./a\";\n",
            "src/g.d.ts": "declare const g: number;\n",
        });
        const options = chainOptions(lib, p);
        const roots = [...chainRoots, "src/e.ts", "src/f.ts"].map(p.file);
        const builder = lib.createSemanticDiagnosticsBuilderProgram(roots, options, lib.createIncrementalCompilerHost(options));
        return ["src/a.ts", "src/e.ts", "src/f.ts"].map(name => builder.getAllDependencies(builder.getSourceFile(p.file(name))).map(p.relative).sort());
    };
    const dependencies = run(ts);
    assert.deepEqual(dependencies[0], ["src/a.ts", "src/b.ts", "src/c.ts"]);
    assert.deepEqual(dependencies, run(ts6));
});

test("imports of ambient modules count as references to the files that declare them, as in TS 6.0", needsStock, () => {
    const run = lib => {
        const p = project({
            "src/a.ts": "import { f } from \"foo\";\nexport const a: number = f;\n",
            "src/c.ts": "export const c = 1;\n",
            "types/foo.d.ts": "declare module \"foo\" {\n    import { B } from \"bar\";\n    export const f: B;\n}\n",
            "types/bar.d.ts": "declare module \"bar\" {\n    export type B = number;\n}\ndeclare module \"bar\" {\n    export const extra: string;\n}\n",
        });
        const options = chainOptions(lib, p);
        const roots = ["src/a.ts", "src/c.ts", "types/foo.d.ts", "types/bar.d.ts"].map(p.file);
        const builder = lib.createSemanticDiagnosticsBuilderProgram(roots, options, lib.createIncrementalCompilerHost(options));
        return ["src/a.ts", "src/c.ts"].map(name => builder.getAllDependencies(builder.getSourceFile(p.file(name))).map(p.relative).sort());
    };
    const dependencies = run(ts);
    assert.deepEqual(dependencies[0], ["src/a.ts", "types/bar.d.ts", "types/foo.d.ts"]);
    assert.deepEqual(dependencies, run(ts6));
});

test("getPreEmitDiagnostics accepts a builder program, as NestJS's tsc build passes one", needsStock, () => {
    const run = lib => {
        const p = project({ ...chain, "src/c.ts": "export const c = \"x\";\n" });
        const options = chainOptions(lib, p);
        const builder = lib.createIncrementalProgram({ rootNames: chainRoots.map(p.file), options });
        builder.emit(undefined, undefined, undefined, undefined, { before: [], after: [], afterDeclarations: [] });
        return lib.getPreEmitDiagnostics(builder).map(d => `${p.relative(d.file.fileName)}:${d.start} ${d.code}`);
    };
    const diagnostics = run(ts);
    assert.deepEqual(diagnostics, ["src/a.ts:38 2322"]);
    assert.deepEqual(diagnostics, run(ts6));
});

test("createIncrementalProgram builds on a tsc-rs program and writes no .tsbuildinfo", () => {
    const p = project({ ...chain, "tsconfig.json": "{}" });
    const options = chainOptions(ts, p, { incremental: true, configFilePath: p.file("tsconfig.json") });
    const builder = ts.createIncrementalProgram({ rootNames: chainRoots.map(p.file), options });
    assert.ok(builder.getProgram() instanceof NativeProgram);
    assert.equal(ts.getTsBuildInfoEmitOutputFilePath(options), p.file("tsconfig.tsbuildinfo"));
    const result = builder.emit();
    assert.equal(result.emitSkipped, false);
    assert.equal(p.exists("out/a.js"), true);
    assert.equal(p.exists("tsconfig.tsbuildinfo"), false);
    assert.equal(builder.emitBuildInfo(), ts.emitSkippedWithNoDiagnostics);
});

test("readBuilderProgram ignores a TS 6.0 .tsbuildinfo, so tsc-rs rebuilds everything", needsStock, () => {
    const p = project({ ...chain, "tsconfig.json": "{}" });
    const options6 = chainOptions(ts6, p, { incremental: true, configFilePath: p.file("tsconfig.json") });
    ts6.createIncrementalProgram({ rootNames: chainRoots.map(p.file), options: options6 }).emit();
    assert.equal(p.exists("tsconfig.tsbuildinfo"), true);
    assert.ok(ts6.readBuilderProgram(options6, ts6.createIncrementalCompilerHost(options6)) !== undefined);

    const options = chainOptions(ts, p, { incremental: true, configFilePath: p.file("tsconfig.json") });
    assert.equal(ts.readBuilderProgram(options, ts.createIncrementalCompilerHost(options)), undefined);
    const written = [];
    ts.createIncrementalProgram({ rootNames: chainRoots.map(p.file), options }).emit(undefined, fileName => written.push(p.relative(fileName)));
    assert.equal(written.length, 8);
});

// Runs a watch program until `steps` are done. Each step edits files after the status message
// before it and waits for the next one; returns the status messages and diagnostics in order.
async function watchScenario(lib, p, createHost, steps, { configure } = {}) {
    const events = [];
    const statuses = () => events.filter(event => event.startsWith("status "));
    const host = createHost(
        diagnostic => events.push(`diagnostic ${diagnostic.file ? p.relative(diagnostic.file.fileName) : ""} ${diagnostic.code}`),
        diagnostic => events.push(`status ${diagnostic.code} ${diagnostic.messageText}`),
    );
    configure?.(host);
    const watch = lib.createWatchProgram(host);
    try {
        let seen = 0;
        for (const step of [{}, ...steps]) {
            for (const [name, text] of Object.entries(step)) {
                p.write(name, text);
            }
            await waitFor(() => statuses().filter(status => / (6193|6194) /.test(status)).length > seen, `report ${seen + 1}`);
            seen++;
        }
    }
    finally {
        watch.close();
    }
    return events;
}

const watchConfig = JSON.stringify({ compilerOptions: { outDir: "out", rootDir: "src", strict: true, target: "es2022", module: "commonjs" }, include: ["src"] });

test("a config file watch reports TS 6.0's status messages and rebuilds after an edit", needsStock, async () => {
    const run = async lib => {
        const p = project({ ...chain, "tsconfig.json": watchConfig });
        const events = await watchScenario(
            lib,
            p,
            (reportDiagnostic, reportStatus) => lib.createWatchCompilerHost(p.file("tsconfig.json"), {}, lib.sys, lib.createEmitAndSemanticDiagnosticsBuilderProgram, reportDiagnostic, reportStatus),
            [{ "src/c.ts": "export const c = \"x\";\n" }, { "src/c.ts": "export const c = 2;\n" }],
        );
        return { events, outputs: p.outputs(), c: p.read("out/c.js") };
    };
    const result = await run(ts);
    assert.deepEqual(result.events, [
        "status 6031 Starting compilation in watch mode...",
        "status 6194 Found 0 errors. Watching for file changes.",
        "status 6032 File change detected. Starting incremental compilation...",
        "diagnostic src/a.ts 2322",
        "status 6193 Found 1 error. Watching for file changes.",
        "status 6032 File change detected. Starting incremental compilation...",
        "status 6194 Found 0 errors. Watching for file changes.",
    ]);
    assert.match(result.c, /exports\.c = 2;/);
    assert.deepEqual(result, await run(ts6));
});

test("a root files watch compiles those files and recompiles an edited one", needsStock, async () => {
    const run = async lib => {
        const p = project(chain);
        const written = [];
        const events = await watchScenario(
            lib,
            p,
            (reportDiagnostic, reportStatus) => lib.createWatchCompilerHost(chainRoots.map(p.file), chainOptions(lib, p), lib.sys, lib.createEmitAndSemanticDiagnosticsBuilderProgram, reportDiagnostic, reportStatus),
            [{ "src/d.ts": "export const d: string = \"e\";\n" }],
            {
                configure: host => {
                    const writeFile = host.writeFile;
                    host.writeFile = (fileName, ...rest) => {
                        written.push(p.relative(fileName));
                        return writeFile(fileName, ...rest);
                    };
                },
            },
        );
        return { events, written, d: p.read("out/d.js") };
    };
    const result = await run(ts);
    assert.deepEqual(result.written.slice(8).sort(), ["out/d.d.ts", "out/d.js"]);
    assert.match(result.d, /"e"/);
    const stock = await run(ts6);
    assert.deepEqual({ ...result, written: result.written.sort() }, { ...stock, written: stock.written.sort() });
});

test("NestJS's watch compiler can replace host.createProgram and the builder's emit", async () => {
    const p = project({ ...chain, "tsconfig.json": watchConfig });
    const transformed = [];
    const emits = [];
    const events = await watchScenario(
        ts,
        p,
        (reportDiagnostic, reportStatus) => ts.createWatchCompilerHost(p.file("tsconfig.json"), { preserveWatchOutput: true }, ts.sys, ts.createEmitAndSemanticDiagnosticsBuilderProgram, reportDiagnostic, reportStatus),
        [{ "src/d.ts": "export const d: string = \"e\";\n" }],
        {
            // nest-cli lib/compiler/watch-compiler.ts:106-167
            configure: host => {
                const origCreateProgram = host.createProgram;
                host.createProgram = (rootNames, options, compilerHost, oldProgram) => {
                    const program = origCreateProgram(rootNames, options, compilerHost, oldProgram, undefined, undefined);
                    const origProgramEmit = program.emit;
                    program.emit = (targetSourceFile, writeFile, cancellationToken, emitOnlyDtsFiles, customTransformers) => {
                        emits.push(program.getProgram() instanceof NativeProgram);
                        const before = [() => sourceFile => {
                            transformed.push(p.relative(sourceFile.fileName));
                            return sourceFile;
                        }];
                        return origProgramEmit(targetSourceFile, writeFile, cancellationToken, emitOnlyDtsFiles, {
                            before: [...before, ...(customTransformers?.before ?? [])],
                            after: customTransformers?.after ?? [],
                            afterDeclarations: customTransformers?.afterDeclarations ?? [],
                        });
                    };
                    return program;
                };
            },
        },
    );
    const statuses = events.filter(event => event.startsWith("status "));
    // nest-cli restarts the app when a status message includes "Found 0 errors".
    assert.equal(statuses.filter(status => status.includes("Found 0 errors")).length, 2);
    assert.deepEqual(emits, [true, true]);
    assert.deepEqual(transformed.sort(), ["src/a.ts", "src/b.ts", "src/c.ts", "src/d.ts", "src/d.ts"]);
    assert.match(p.read("out/d.js"), /"e"/);
});

test("typescript-eslint's watch program updates synchronously without timers", () => {
    const p = project({ ...chain, "tsconfig.json": watchConfig });
    const fileCallbacks = new Map();
    const code = new Map();
    // typescript-estree create-program/getWatchProgramsForProjects.js createWatchProgram
    const host = ts.createWatchCompilerHost(p.file("tsconfig.json"), { noEmit: true }, ts.sys, ts.createAbstractBuilder, diagnostic => {
        throw new Error(ts.flattenDiagnosticMessageText(diagnostic.messageText, "\n"));
    }, () => {});
    const readFile = host.readFile;
    host.readFile = (fileName, encoding) => code.get(fileName) ?? readFile(fileName, encoding);
    host.afterProgramCreate = program => {
        const errors = program.getConfigFileParsingDiagnostics().filter(diagnostic => diagnostic.category === ts.DiagnosticCategory.Error && diagnostic.code !== 18003);
        assert.deepEqual(errors, []);
    };
    host.watchFile = (fileName, callback) => {
        fileCallbacks.set(fileName, callback);
        return { close: () => fileCallbacks.delete(fileName) };
    };
    host.watchDirectory = () => ({ close() {} });
    host.setTimeout = undefined;
    host.clearTimeout = undefined;
    const watch = ts.createWatchProgram(host);
    try {
        const first = watch.getProgram().getProgram();
        assert.ok(first instanceof NativeProgram);
        assert.equal(first.getSourceFile(p.file("src/c.ts")).text, chain["src/c.ts"]);
        assert.deepEqual(first.getRootFileNames().map(p.relative).sort(), chainRoots);

        code.set(p.file("src/c.ts"), "export const c = \"linted\";\n");
        fileCallbacks.get(p.file("src/c.ts"))(p.file("src/c.ts"), ts.FileWatcherEventKind.Changed);
        const second = watch.getProgram().getProgram();
        assert.notEqual(second, first);
        assert.equal(second.getSourceFile(p.file("src/c.ts")).text, "export const c = \"linted\";\n");
        assert.deepEqual(ts.getPreEmitDiagnostics(second).map(d => `${p.relative(d.file.fileName)} ${d.code}`), ["src/a.ts 2322"]);
        assert.equal(watch.getProgram().getProgram(), second);
    }
    finally {
        watch.close();
    }
});

test("createDiagnosticReporter and createWatchStatusReporter print what TS 6.0 prints", needsStock, async () => {
    const run = async lib => {
        const p = project({ ...chain, "tsconfig.json": watchConfig });
        let output = "";
        const system = { ...lib.sys, write: text => output += text, clearScreen() {} };
        // nest-cli lib/compiler/watch-compiler.ts:55-59 uses these two internal reporters.
        const reportDiagnostic = lib.createDiagnosticReporter(system, true);
        const reportStatus = lib.createWatchStatusReporter(system, true);
        await watchScenario(
            lib,
            p,
            (onDiagnostic, onStatus) => lib.createWatchCompilerHost(
                p.file("tsconfig.json"),
                { preserveWatchOutput: true },
                system,
                lib.createEmitAndSemanticDiagnosticsBuilderProgram,
                diagnostic => {
                    onDiagnostic(diagnostic);
                    reportDiagnostic(diagnostic);
                },
                (diagnostic, ...rest) => {
                    onStatus(diagnostic);
                    reportStatus(diagnostic, ...rest);
                },
            ),
            [{ "src/c.ts": "export const c = \"x\";\n" }],
        );
        return output.replaceAll(p.dir, "<dir>").replace(/\[\S*?\d{1,2}:\d{2}:\d{2}( [AP]M)?\S*?\]/g, "[time]");
    };
    const output = await run(ts);
    assert.match(output, /Found 1 error\. Watching for file changes\./);
    assert.match(output, /error.* TS2322: .*Type 'string' is not assignable to type 'number'\./);
    assert.equal(output, await run(ts6));
});

function referencesProject() {
    return project({
        "core/tsconfig.json": JSON.stringify({ compilerOptions: { composite: true, outDir: "out", rootDir: "src", module: "commonjs", target: "es2022", strict: true }, include: ["src"] }),
        "core/src/index.ts": "export const x = 1;\n",
        "app/tsconfig.json": JSON.stringify({ compilerOptions: { outDir: "out", rootDir: "src", module: "commonjs", target: "es2022", strict: true }, references: [{ path: "../core" }], include: ["src"] }),
        "app/src/main.ts": "import { x } from \"../../core/src/index\";\nexport const y: number = x;\n",
    });
}

test("createSolutionBuilder builds a project and the project it references", needsStock, () => {
    const run = lib => {
        const p = referencesProject();
        const events = [];
        const host = lib.createSolutionBuilderHost(lib.sys, undefined, diagnostic => events.push(`diagnostic ${diagnostic.code}`), () => {}, errors => events.push(`errors ${errors}`));
        const status = lib.createSolutionBuilder(host, [p.file("app/tsconfig.json")], {}).build();
        p.write("core/src/index.ts", "export const x = \"x\";\n");
        const host2 = lib.createSolutionBuilderHost(lib.sys, undefined, diagnostic => events.push(`diagnostic ${diagnostic.code}`), () => {}, errors => events.push(`errors ${errors}`));
        const status2 = lib.createSolutionBuilder(host2, [p.file("app/tsconfig.json")], {}).build();
        return { status, status2, events, outputs: [...p.outputs("core/out"), ...p.outputs("app/out")], declaration: p.read("core/out/index.d.ts") };
    };
    const result = run(ts);
    assert.equal(result.status, ts.ExitStatus.Success);
    assert.equal(result.status2, ts.ExitStatus.DiagnosticsPresent_OutputsGenerated);
    assert.deepEqual(result.events, ["errors 0", "diagnostic 2322", "errors 1"]);
    assert.deepEqual(result.outputs, ["core/out/index.d.ts", "core/out/index.js", "app/out/main.js"]);
    assert.equal(result.declaration, "export declare const x = \"x\";\n");
    assert.deepEqual(result, run(ts6));
});

test("createSolutionBuilderWithWatch rebuilds the referencing project after an edit", needsStock, async () => {
    const run = async lib => {
        const p = referencesProject();
        const events = [];
        const host = lib.createSolutionBuilderWithWatchHost(
            lib.sys,
            lib.createSemanticDiagnosticsBuilderProgram,
            diagnostic => events.push(`diagnostic ${p.relative(diagnostic.file?.fileName ?? "")} ${diagnostic.code}`),
            () => {},
            diagnostic => events.push(`status ${diagnostic.code}`),
        );
        const builder = lib.createSolutionBuilderWithWatch(host, [p.file("app/tsconfig.json")], {});
        builder.build();
        try {
            await waitFor(() => events.includes("status 6194"), "the first build");
            p.write("core/src/index.ts", "export const x = \"x\";\n");
            await waitFor(() => events.includes("status 6193"), "the rebuild");
        }
        finally {
            builder.close();
        }
        return events;
    };
    const events = await run(ts);
    assert.deepEqual(events, ["status 6194", "status 6032", "diagnostic app/src/main.ts 2322", "status 6193"]);
    assert.deepEqual(events, await run(ts6));
});
