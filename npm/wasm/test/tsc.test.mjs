// Tests of the package's Node entry. Build the module first:
// scripts/wasm/build.sh (WASM_PROFILE=release for a fast build).

import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { once } from "node:events";
import {
    chmodSync,
    lstatSync,
    mkdirSync,
    mkdtempSync,
    readFileSync,
    rmSync,
    symlinkSync,
    unlinkSync,
    writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { loadModule, memoryFileSystem, runTscAsync, tsc } from "../node.js";

const cli = fileURLToPath(new URL("../bin/tsc-wasm.js", import.meta.url));

const project = {
    "/p/tsconfig.json": JSON.stringify({
        compilerOptions: { strict: true, target: "es2022", module: "esnext", outDir: "out", declaration: true },
        files: ["a.ts"],
    }),
    "/p/a.ts": [
        'const x: number = "hello";',
        "export function f(a: string) { return a.length; }",
        "f(1);",
        "",
    ].join("\n"),
};

test("prints the diagnostics and emits, in memory", async () => {
    const result = await tsc(["-p", "tsconfig.json"], { files: project, cwd: "/p" });
    assert.equal(result.exitCode, 2);
    assert.equal(
        result.stdout,
        "a.ts(1,7): error TS2322: Type 'string' is not assignable to type 'number'.\n"
            + "a.ts(3,3): error TS2345: Argument of type 'number' is not assignable to parameter of type 'string'.\n",
    );
    // The same output as the native tsgo.
    assert.equal(
        result.files.get("/p/out/a.js"),
        'const x = "hello";\nexport function f(a) { return a.length; }\nf(1);\n',
    );
    assert.equal(result.files.get("/p/out/a.d.ts"), "export declare function f(a: string): number;\n");
});

test("returns the diagnostics as JSON", async () => {
    const result = await tsc(["-p", "/p", "--noEmit"], { files: project, diagnostics: "json" });
    assert.equal(result.exitCode, 2);
    assert.equal(result.stdout, "");
    assert.deepEqual(
        result.diagnostics.map(d => [d.fileName, d.code, d.category, d.startPosition, d.text]),
        [
            ["/p/a.ts", 2322, 1, { line: 0, character: 6 }, "Type 'string' is not assignable to type 'number'."],
            [
                "/p/a.ts",
                2345,
                1,
                { line: 2, character: 2 },
                "Argument of type 'number' is not assignable to parameter of type 'string'.",
            ],
        ],
    );
    assert.equal(result.files.has("/p/out/a.js"), false);
});

test("reports config errors without a file", async () => {
    const result = await tsc(["-p", "/p/missing.json"], { files: project, diagnostics: "json" });
    assert.equal(result.exitCode, 1);
    assert.deepEqual(result.diagnostics, [
        { pos: -1, end: -1, code: 5058, category: 1, text: "The specified path does not exist: '/p/missing.json'." },
    ]);
});

test("checks the DOM lib and a clean program", async () => {
    const result = await tsc(["--noEmit", "--lib", "es2022,dom", "--strict", "/m.ts"], {
        files: { "/m.ts": "const el: HTMLElement | null = document.querySelector('div');\nexport const tag = el?.tagName;\n" },
    });
    assert.equal(result.stdout, "");
    assert.equal(result.exitCode, 0);
});

// Issue #83: the wasm checker panicked (flow_p1.rs, index out of bounds) on
// a narrowing callback whose contextual signature has a type predicate.
// Native builds were fine. CI runs this file on a wasm build (ci.yml).
test("checks callbacks typed by a type predicate (#83)", async () => {
    const run = source => tsc(["--noEmit", "--strict", "a.ts"], { files: { "/r/a.ts": source }, cwd: "/r" });
    const apply = await run(
        "declare function apply<S extends number>(f: (o: number) => o is S): void;\napply(o => o !== 1);\n",
    );
    assert.equal(apply.stderr, "");
    assert.equal(apply.exitCode, 2);
    assert.equal(
        apply.stdout,
        "a.ts(2,7): error TS2345: Argument of type '(o: number) => boolean' is not assignable to parameter of type '(o: number) => o is number'.\n"
            + "  Signature '(o: number): boolean' must be a type predicate.\n",
    );
    const filter = await run("const xs: number[] = [1, 2, 3];\nexport const ys = xs.filter(o => o !== 1);\n");
    assert.equal(filter.stderr, "");
    assert.equal(filter.exitCode, 0);
    assert.equal(filter.stdout, "");
});

// Issue #99: a chain of 5,000 binary operators binds and checks in a worker
// with a 1 MB stack (V8 gets 832 KiB of it). Go recurses for each level, and
// so did the port (about 465 B for each level in check, release build). Its
// binder and checker now keep a work stack on the heap (binder_p3.rs
// BindBinaryFrame, checker_p14.rs CheckBinaryFrame). The logical operators
// have calls as operands, so the test does not depend on the flow analysis
// of a reference, which still recurses (up to Go's limit of 2000, TS2563).
// The output is tsgo-oracle-3e9236361fa9's (typescript@7.1.0-beta) on the
// same files, one program for each.
test("checks long binary chains on a 1 MB stack (#99)", async () => {
    const n = 5000;
    const chain = (op, special = {}) => Array.from({ length: n }, (_, i) => special[i] ?? "v").join(op);
    const check = (name, text) =>
        tsc(["-p", "/s"], {
            files: {
                "/s/tsconfig.json": JSON.stringify({
                    compilerOptions: {
                        strict: true,
                        types: [],
                        lib: ["es5"],
                        target: "es2022",
                        noEmit: true,
                        skipLibCheck: true,
                    },
                    files: [name],
                }),
                [`/s/${name}`]: text,
            },
            cwd: "/s",
            stackSizeMb: 1,
        });
    const expect = async (name, text, exitCode, stdout) => {
        const result = await check(name, text);
        assert.equal(result.stderr, "", name);
        assert.equal(result.stdout, stdout, name);
        assert.equal(result.exitCode, exitCode, name);
    };
    const v = "declare const v: number;\n";
    await expect("plus.ts", `${v}export const total = ${chain(" + ")};\n`, 0, "");
    for (const [name, op] of [["and.ts", " && "], ["or.ts", " || "], ["nullish.ts", " ?? "]]) {
        const calls = Array(n).fill("f()").join(op);
        await expect(name, `declare function f(): number;\nexport const total = ${calls};\n`, 0, "");
    }
    await expect(
        "comma.ts",
        `${v}export const total = (${chain(",\n")});\n`,
        2,
        "comma.ts(2,23): error TS2695: Left side of comma operator is unused and has no side effects.\n".repeat(n - 1),
    );
    // A `null` in each of 4 chains, and 4 calls with an argument too many in
    // one chain: one diagnostic for each, at Go's positions.
    const marks = [2, n / 4, n / 2, n - 1];
    const nulls = marks.map((mark, i) => `export const bad${i} = ${chain(" + ", { [mark]: "null" })};\n`);
    await expect(
        "null.ts",
        v + nulls.join(""),
        2,
        ["(2,29)", "(3,5021)", "(4,10021)", "(5,20017)"]
            .map(at => `null.ts${at}: error TS18050: The value 'null' cannot be used here.\n`)
            .join(""),
    );
    const calls = Object.fromEntries(marks.map(mark => [mark, "Math.abs(v, 0)"]));
    await expect(
        "call.ts",
        `${v}export const bad = ${chain(" + ", calls)};\n`,
        2,
        ["(2,40)", "(2,5045)", "(2,10058)", "(2,20067)"]
            .map(at => `call.ts${at}: error TS2554: Expected 1 arguments, but got 2.\n`)
            .join(""),
    );
});

test("its types need no lib dom", async () => {
    // A consumer of the package, checked with and without lib dom, the
    // package's .d.ts files included.
    const files = {
        "/c/package.json": '{ "type": "module" }',
        "/c/a.ts": [
            'import { loadModule, tsc } from "ts-rust-wasm";',
            'import { memoryFileSystem, runTsc } from "ts-rust-wasm/core";',
            "const module = await loadModule(new Uint8Array(8));",
            "runTsc(module, { fs: memoryFileSystem() });",
            'await tsc([], { wasm: "ts_rust.wasm" });',
            "",
        ].join("\n"),
    };
    for (const name of ["package.json", "index.d.ts", "core.d.ts"]) {
        files[`/c/node_modules/ts-rust-wasm/${name}`] = readFileSync(new URL(`../${name}`, import.meta.url), "utf8");
    }
    for (const lib of ["es2022", "es2022,dom"]) {
        const args = ["--noEmit", "--strict", "--module", "nodenext", "--types", "", "--lib", lib, "a.ts"];
        const result = await tsc(args, { files, cwd: "/c" });
        assert.equal(result.stdout, "", lib);
        assert.equal(result.exitCode, 0, lib);
    }
});

test("reads and writes the real file system", async () => {
    const dir = mkdtempSync(join(tmpdir(), "ts-rust-wasm-"));
    try {
        writeFileSync(join(dir, "tsconfig.json"), JSON.stringify({ compilerOptions: { outDir: "out" } }));
        writeFileSync(join(dir, "b.ts"), "export const answer: number = 42;\n");
        const result = await tsc([], { cwd: dir });
        assert.equal(result.stdout, "");
        assert.equal(result.exitCode, 0);
        assert.equal(readFileSync(join(dir, "out", "b.js"), "utf8"), "export const answer = 42;\n");
    } finally {
        rmSync(dir, { recursive: true, force: true });
    }
});

test("keeps a symlinked current directory, as tsgo does", () => {
    const real = mkdtempSync(join(tmpdir(), "ts-rust-wasm-"));
    const link = `${real}-link`;
    try {
        symlinkSync(real, link);
        writeFileSync(join(real, "a.ts"), "export {};\n");
        // A shell that entered the link sets PWD to it.
        const run = spawnSync(process.execPath, [cli, "--listFilesOnly", "--types", "", "--lib", "es5", "a.ts"], {
            cwd: link,
            env: { ...process.env, PWD: link },
            encoding: "utf8",
        });
        assert.equal(run.status, 0);
        assert.equal(run.stdout.trim().split("\n").at(-1), `${link}/a.ts`);
    } finally {
        // Not rmSync: on Node 24.13 it throws ERR_FS_EISDIR for a link to a directory.
        if (lstatSync(link, { throwIfNoEntry: false })) unlinkSync(link);
        rmSync(real, { recursive: true, force: true });
    }
});

test("writes all output to a slow non-blocking pipe", async () => {
    const dir = mkdtempSync(join(tmpdir(), "ts-rust-wasm-"));
    try {
        // About 230 KB of diagnostics, more than a pipe buffer holds.
        const lines = Array.from({ length: 3000 }, (_, i) => i + 1);
        writeFileSync(join(dir, "a.ts"), lines.map(i => `export const x${i}: number = "";\n`).join(""));
        const want = lines
            .map(i => `a.ts(${i},14): error TS2322: Type 'string' is not assignable to type 'number'.\n`)
            .join("");
        // The preload opens process.stdout, which makes the pipe non-blocking,
        // as a Node parent that shares the pipe does.
        const preload = ["--import", "data:text/javascript,process.stdout.isTTY"];
        const args = ["--types", "", "--lib", "es5", "a.ts", "--noEmit"];
        const child = spawn(process.execPath, [...preload, cli, ...args], { cwd: dir });
        const closed = once(child, "close");
        const chunks = [];
        child.stdout.on("data", chunk => chunks.push(chunk));
        // Read nothing for a second, so the pipe fills up.
        child.stdout.pause();
        await new Promise(resolve => setTimeout(resolve, 1000));
        child.stdout.resume();
        const [code] = await closed;
        const stdout = Buffer.concat(chunks).toString();
        assert.equal(code, 2);
        assert.equal(stdout.length, want.length);
        assert.ok(stdout === want, "stdout differs");
    } finally {
        rmSync(dir, { recursive: true, force: true });
    }
});

test("reports a failed write with the OS error, as tsgo does", async () => {
    const dir = mkdtempSync(join(tmpdir(), "ts-rust-wasm-"));
    try {
        writeFileSync(join(dir, "tsconfig.json"), JSON.stringify({ compilerOptions: { outDir: "out" }, files: ["a.ts"] }));
        writeFileSync(join(dir, "a.ts"), "export {};\n");
        mkdirSync(join(dir, "out"));
        writeFileSync(join(dir, "out", "a.js"), "");
        chmodSync(join(dir, "out", "a.js"), 0o444);
        const result = await tsc(["-p", "."], { cwd: dir });
        assert.equal(result.exitCode, 2);
        assert.equal(
            result.stdout,
            `error TS5033: Could not write file '${dir}/out/a.js': open ${dir}/out/a.js: permission denied.\n`,
        );
    } finally {
        rmSync(dir, { recursive: true, force: true });
    }
});

test("reports a failed mkdir with Go's error, as tsgo does", async () => {
    const dir = mkdtempSync(join(tmpdir(), "ts-rust-wasm-"));
    try {
        writeFileSync(join(dir, "a.ts"), "export {};\n");
        writeFileSync(join(dir, "f"), "");
        // Go's os.MkdirAll names the file in the path, with ENOTDIR's text.
        for (const outDir of ["f", "f/x/y"]) {
            const result = await tsc(["--outDir", outDir, "--types", "", "a.ts"], { cwd: dir });
            assert.equal(result.exitCode, 2, outDir);
            assert.equal(
                result.stdout,
                `error TS5033: Could not write file '${dir}/${outDir}/a.js': mkdir ${dir}/f: not a directory.\n`,
            );
        }
    } finally {
        rmSync(dir, { recursive: true, force: true });
    }
});

test("runs through runTscAsync", async () => {
    const fs = memoryFileSystem({ "/q/a.ts": "export const s: string = 1;\n" });
    const { exitCode, diagnostics } = await runTscAsync(await loadModule(), {
        args: ["--noEmit", "/q/a.ts"],
        fs,
        diagnosticsJson: true,
    });
    assert.equal(exitCode, 2);
    assert.deepEqual(diagnostics.map(d => d.code), [2322]);
});

test("keeps the BOM of an emitted file in memory (--emitBOM)", async () => {
    const result = await tsc(["--emitBOM", "--declaration", "--outDir", "out", "a.ts"], {
        files: { "/b/a.ts": "export const n = 1;\n" },
        cwd: "/b",
    });
    assert.equal(result.exitCode, 0);
    assert.equal(result.files.get("/b/out/a.js"), "\uFEFFexport const n = 1;\n");
    assert.equal(result.files.get("/b/out/a.d.ts"), "\uFEFFexport declare const n = 1;\n");
});

test("refuses watch mode, also from a response file", async () => {
    const files = { ...project, "/p/args.txt": "--watch\n" };
    for (const args of [["--watch"], ["-w"], ["-watch"], ["--WATCH"], ["@args.txt"], ["-b", "--watch"]]) {
        const result = await tsc(args, { files, cwd: "/p" });
        assert.equal(result.exitCode, 1, args.join(" "));
        assert.match(result.stderr, /--watch is not supported/, args.join(" "));
    }
    const result = await tsc(["--watch", "false", "--noEmit"], { files, cwd: "/p" });
    assert.equal(result.exitCode, 2);
});

test("refuses JSON diagnostics for --build", async () => {
    const result = await tsc(["-b"], { files: project, cwd: "/p", diagnostics: "json" });
    assert.equal(result.exitCode, 1);
    assert.match(result.stderr, /not supported with --build/);
});

test("builds projects after one with a syntax error (tsc -b)", async () => {
    // A syntax error stops the check, so the emit makes only the checkers of
    // p1's files. Before p2 counts on from p1's symbol ids, the build makes
    // p1's other checkers (it panicked there before). Go and the native
    // tsgo give the same output.
    const config = files => JSON.stringify({
        compilerOptions: { strict: true, outDir: "out", types: [], lib: ["es2022"], target: "es2022" },
        files,
    });
    const files = {
        "/w/p1/tsconfig.json": config(["a.ts", "b.ts"]),
        "/w/p1/a.ts": "export const a: number = 1;\nconst x = (;\n",
        "/w/p1/b.ts": "export const b = 2;\n",
        "/w/p2/tsconfig.json": config(["c.ts"]),
        "/w/p2/c.ts": "export class C { static readonly #c = Symbol(); static t() { const n: number = C.#c; } }\n",
    };
    const result = await tsc(["-b", "p1", "p2", "--pretty", "false"], { files, cwd: "/w" });
    assert.equal(result.stderr, "");
    assert.equal(
        result.stdout,
        "p1/a.ts(2,12): error TS1109: Expression expected.\n"
            + "p2/c.ts(1,68): error TS2322: Type 'typeof import(\"/w/p2/c\").C.�#1@#c'"
            + " is not assignable to type 'number'.\n",
    );
    assert.equal(result.exitCode, 2);
    assert.equal(result.files.get("/w/p1/out/a.js"), "export const a = 1;\nconst x = ();\n");
    assert.equal(result.files.get("/w/p1/out/b.js"), "export const b = 2;\n");
    assert.equal(
        result.files.get("/w/p2/out/c.js"),
        "export class C {\n    static #c = Symbol();\n    static t() { const n = C.#c; }\n}\n",
    );
});
