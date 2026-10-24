// Helpers shared by the scenario modules: app paths, installs, the CJS fixture, swagger schema
// checks and build-info files.

import fs from "node:fs";
import path from "node:path";

import { pins, prepareAppDir } from "./sources.mjs";
import { fixturesDir, harnessDir, listFiles } from "./util.mjs";

export const minute = 60_000;
export const preloadPort = path.join(harnessDir, "preload-port.cjs");
export const appRunning = /Application is running on: /;
export const cjsFixture = path.join(fixturesDir, "nest-cjs-paths");

export function appFile(ctx, rel) {
    return path.join(ctx.appDir, rel);
}

export function exists(ctx, rel) {
    return fs.existsSync(appFile(ctx, rel));
}

export function sourceFiles(ctx) {
    return new Set(listFiles(ctx.appDir, (rel, entry) => entry.name === "node_modules" || entry.name === ".git"));
}

export async function npmInstall(ctx, { lockKey, legacyPeerDeps = false }) {
    const args = ["install", `--before=${pins.npmBefore}`];
    if (legacyPeerDeps) {
        args.push("--legacy-peer-deps");
    }
    const step = await ctx.step("npm-install", "npm", args, { install: true, timeoutMs: 20 * minute });
    if (step.ok && lockKey !== undefined) {
        ctx.saveLock(lockKey);
    }
    return step.ok && ctx.verifyInstall();
}

// A committed fixture app (with its package-lock.json) installed with the package under test.
export async function prepareFixture(ctx, name, { lockKey, extraDevDependencies, edit } = {}) {
    prepareAppDir(path.join(fixturesDir, name), ctx.appDir);
    edit?.();
    ctx.useTarball({ lockKey, extraDevDependencies });
    return npmInstall(ctx, { lockKey });
}

export function prepareCjsFixture(ctx, options) {
    return prepareFixture(ctx, "nest-cjs-paths", options);
}

// The plugin is the only source of the DTO schema properties: the checked classes have no @ApiProperty.
export function checkSchema(ctx, text, schemaName, { required = [], properties = [] }) {
    let schema;
    try {
        schema = JSON.parse(text).components?.schemas?.[schemaName];
    } catch {
        schema = undefined;
    }
    const names = Object.keys(schema?.properties ?? {});
    const missing = [...required, ...properties].filter((name) => !names.includes(name));
    const missingRequired = required.filter((name) => !(schema?.required ?? []).includes(name));
    ctx.check(`api-json schema ${schemaName} has the plugin properties`, schema !== undefined && missing.length === 0 && missingRequired.length === 0,
        `properties [${names.join(", ")}] required [${(schema?.required ?? []).join(", ")}]`);
}

// The .tsbuildinfo records the compiler's own file versions (lib file hashes, format), so it is
// kept for inspection and diffed, not counted.
export function saveBuildInfo(ctx) {
    for (const name of fs.readdirSync(ctx.appDir)) {
        if (name.endsWith(".tsbuildinfo")) {
            ctx.saveFile(name, name, "json", { informational: true });
        }
    }
}

// Module specifiers that tsconfig-paths.hook.ts (or webpack's resolver) must have replaced.
// Declarations keep the aliases in a webpack build (ts-loader emits them as tsc would without
// nest-cli's hook), so that check passes /\.js$/.
export function findAliasedSpecifiers(ctx, files = /\.(js|d\.ts)$/) {
    const hits = [];
    const dist = appFile(ctx, "dist");
    if (!fs.existsSync(dist)) {
        return hits;
    }
    for (const rel of listFiles(dist)) {
        if (!files.test(rel)) {
            continue;
        }
        const text = fs.readFileSync(path.join(dist, rel), "utf8");
        for (const match of text.matchAll(/(?:require\(|from |import\()\s*["']((?:@app\/|@common|src\/)[^"']*)["']/g)) {
            hits.push(`${rel}: ${match[1]}`);
        }
    }
    return hits;
}

// The six requests every run of the CJS fixture app answers; the swagger schema check needs the
// plugin, so callers without it pass checkPluginSchema: false.
export async function exerciseCjsApp(ctx, base, { checkPluginSchema = true } = {}) {
    const requests = [
        ["root", "/", {}],
        ["cats-list", "/cats?offset=0&limit=5", {}],
        ["cats-one", "/cats/1", {}],
        ["cats-missing", "/cats/99", {}],
        ["cats-create", "/cats", { method: "POST", body: { name: "Tom", age: 4, breed: "Siamese", color: "black", size: 1, tags: ["x"] } }],
        ["cats-invalid", "/cats", { method: "POST", body: { name: "Tom", age: "old", color: "green" } }],
    ];
    const results = {};
    for (const [name, urlPath, options] of requests) {
        results[name] = await ctx.get(base, urlPath, options);
        ctx.saveHttp(name, results[name]);
    }
    ctx.check("GET / answers the v1 greeting", results.root.status === 200 && results.root.text === "Hello from nest-cjs-paths v1", results.root.text.slice(0, 200));
    ctx.check("GET /cats answers 200", results["cats-list"].status === 200, `status ${results["cats-list"].status}`);
    ctx.check("POST /cats answers 201", results["cats-create"].status === 201, `status ${results["cats-create"].status}`);
    ctx.check("POST /cats with a bad body answers 400", results["cats-invalid"].status === 400, `status ${results["cats-invalid"].status}`);
    const apiJson = await ctx.get(base, "/api-json");
    ctx.saveText("api-json.json", apiJson.text, "json");
    ctx.check("GET /api-json answers 200", apiJson.status === 200, `status ${apiJson.status ?? apiJson.error}`);
    if (checkPluginSchema) {
        checkSchema(ctx, apiJson.text, "CreateCatDto", { required: ["name", "age", "breed", "color", "tags"], properties: ["size"] });
        checkSchema(ctx, apiJson.text, "Cat", { required: ["id", "name", "age", "breed", "color", "tags", "createdAt"], properties: ["size", "nickname"] });
    }
}
