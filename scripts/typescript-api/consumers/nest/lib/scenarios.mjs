// The NestJS scenarios. Each run(ctx) prepares its app copy, installs the package under test, runs
// the commands a user runs and keeps the outputs that compare.mjs diffs.

import fs from "node:fs";
import path from "node:path";

import {
    appFile,
    appRunning,
    checkSchema,
    exerciseCjsApp,
    exists,
    findAliasedSpecifiers,
    minute,
    npmInstall,
    prepareCjsFixture,
    preloadPort,
    saveBuildInfo,
    sourceFiles,
} from "./common.mjs";
import { runNestNewCjs, runNestNewEsm } from "./nest-new.mjs";
import { ensureSource, pins, prepareAppDir } from "./sources.mjs";
import { runTsNode } from "./ts-node.mjs";
import { readJson, writeJson } from "./util.mjs";
import { runWebpack, runWebpackSwagger } from "./webpack.mjs";

const foundZeroErrors = /Found 0 errors\. Watching for file changes\./;

export const scenarios = [
    {
        id: "nest-cli-unit",
        letter: "a",
        title: "nestjs/nest-cli 12.0.8: npm run build (tsc) and the vitest unit tests",
        run: runNestCliUnit,
    },
    {
        id: "swagger-sample",
        letter: "b",
        title: "nestjs/nest sample/11-swagger: nest build (swagger plugin), unit and e2e tests, GET /api-json",
        run: runSwaggerSample,
    },
    {
        id: "cjs-paths",
        letter: "c",
        title: "CommonJS app with baseUrl/paths aliases and the swagger plugin: nest build, run, HTTP checks",
        run: runCjsPaths,
    },
    {
        id: "cjs-paths-watch",
        letter: "d",
        title: "nest start --watch on the CommonJS app: start, edit, rebuild, restart, stop",
        run: runCjsPathsWatch,
    },
    {
        id: "swagger-swc",
        letter: "e",
        title: "sample/11-swagger with nest build --builder swc --type-check (src/metadata.ts)",
        run: runSwaggerSwc,
    },
    {
        id: "graphql-sample",
        letter: "f",
        title: "nestjs/nest sample/23-graphql-code-first: nest build (graphql plugin), run, schema.gql",
        run: runGraphqlSample,
    },
    {
        id: "cjs-paths-webpack",
        letter: "g1",
        title: "nest build --webpack on the CommonJS app without plugins: ts-loader transpileOnly + fork-ts-checker, run the bundle, a type error",
        run: runWebpack,
    },
    {
        id: "cjs-paths-webpack-swagger",
        letter: "g2",
        title: "nest build --webpack on the CommonJS app with the swagger plugin: ts-loader full mode (language service), run the bundle, a type error",
        run: runWebpackSwagger,
    },
    {
        id: "nest-new-esm",
        letter: "h1",
        title: "nest new (default ESM template, vitest): npm test, npm run test:e2e, nest build, run the app",
        run: runNestNewEsm,
    },
    {
        id: "nest-new-cjs",
        letter: "h2",
        title: "nest new, CommonJS template (jest + ts-jest, jest.config.ts through ts-node): npm test, npm run test:e2e, nest build, run the app",
        run: runNestNewCjs,
    },
    {
        id: "ts-node",
        letter: "i",
        title: "node -r ts-node/register on a script that imports TS modules: type-check and transpile-only modes, type and option errors",
        run: runTsNode,
    },
];

// --scenario takes letters or ids; a bare letter also takes its numbered scenarios (g = g1,g2).
export function selectScenarios(list) {
    if (list === undefined) {
        return scenarios;
    }
    const selected = list.split(",").filter(Boolean).flatMap((key) => {
        const matches = scenarios.filter((candidate) => candidate.letter === key || candidate.id === key
            || (/^[a-z]$/.test(key) && new RegExp(`^${key}\\d+$`).test(candidate.letter)));
        if (matches.length === 0) {
            throw new Error(`unknown scenario "${key}" (known: ${scenarios.map((s) => `${s.letter}=${s.id}`).join(", ")})`);
        }
        return matches;
    });
    return [...new Set(selected)];
}

// (a) nest-cli's own build and unit tests, the way its CI runs them (.circleci/config.yml:
// install, build, clean, test). The package goes in with --no-save so that
// test/package-json.spec.ts still sees "typescript": "~6.0.2".
async function runNestCliUnit(ctx) {
    prepareAppDir(ensureSource("nest-cli"), ctx.appDir);
    const ci = await ctx.step("npm-ci", "npm", ["ci", "--ignore-scripts", "--legacy-peer-deps"], { install: true, timeoutMs: 20 * minute });
    if (!ci.ok) {
        return;
    }
    const swap = await ctx.step("npm-install-typescript", "npm", [
        "install",
        "--no-save",
        "--ignore-scripts",
        "--legacy-peer-deps",
        `typescript@file:${ctx.pack.tgz}`,
    ], { install: true, timeoutMs: 10 * minute });
    if (!swap.ok || !ctx.verifyInstall()) {
        return;
    }
    const before = sourceFiles(ctx);
    const build = await ctx.step("build", "npm", ["run", "build"], { timeoutMs: 10 * minute });
    ctx.saveStepOutput("build-output.txt", build);
    saveNewFiles(ctx, "build", before);
    await ctx.step("clean", "npm", ["run", "clean"], { timeoutMs: 2 * minute });
    await ctx.vitest("unit", "npm", ["test", "--"], { timeoutMs: 20 * minute });
}

// Files a step created (nest-cli compiles next to its sources), copied into <out>/<name>/.
function saveNewFiles(ctx, name, before) {
    const created = [...sourceFiles(ctx)].filter((rel) => !before.has(rel));
    const target = path.join(ctx.outDir, name);
    for (const rel of created) {
        fs.mkdirSync(path.dirname(path.join(target, rel)), { recursive: true });
        fs.copyFileSync(appFile(ctx, rel), path.join(target, rel));
    }
    ctx.saveTreeFromOut(name, created.length);
}

// (b) sample/11-swagger with the tsc builder and @nestjs/swagger (introspectComments).
async function runSwaggerSample(ctx) {
    prepareAppDir(path.join(ensureSource("nest"), "sample/11-swagger"), ctx.appDir);
    ctx.useTarball({ lockKey: "swagger-sample" });
    if (!(await npmInstall(ctx, { lockKey: "swagger-sample" }))) {
        return;
    }
    const build = await ctx.step("nest-build", "npm", ["run", "build"], { timeoutMs: 10 * minute });
    ctx.saveStepOutput("build-output.txt", build);
    ctx.saveTree("dist", "dist");
    saveBuildInfo(ctx);
    await ctx.vitest("unit", "npm", ["test", "--"], { timeoutMs: 10 * minute });
    await ctx.vitest("e2e", "npm", ["run", "test:e2e", "--"], { timeoutMs: 10 * minute });
    if (!exists(ctx, "dist/main.js")) {
        ctx.check("dist/main.js exists", false);
        return;
    }
    await ctx.withService("app", "node", ["-r", preloadPort, "dist/main.js"], { ready: appRunning }, async (service, base) => {
        const apiJson = await ctx.get(base, "/api-json");
        ctx.saveText("api-json.json", apiJson.text, "json");
        ctx.check("GET /api-json answers 200", apiJson.status === 200, `status ${apiJson.status ?? apiJson.error}`);
        checkSchema(ctx, apiJson.text, "CreateCatDto", { required: ["name", "age", "breed"] });
    });
}

// (c) The committed CommonJS fixture: baseUrl + paths, ignoreDeprecations 6.0, swagger plugin.
async function runCjsPaths(ctx) {
    if (!(await prepareCjsFixture(ctx))) {
        return;
    }
    const build = await ctx.step("nest-build", "npm", ["run", "build"], { timeoutMs: 10 * minute });
    ctx.saveStepOutput("build-output.txt", build);
    ctx.saveTree("dist", "dist");
    saveBuildInfo(ctx);
    const aliased = findAliasedSpecifiers(ctx);
    ctx.check("dist has no unrewritten @app/@common/src aliases", exists(ctx, "dist/main.js") && aliased.length === 0, aliased.slice(0, 20).join("; "));
    if (!exists(ctx, "dist/main.js")) {
        ctx.check("dist/main.js exists", false);
        return;
    }
    await ctx.withService("app", "node", ["dist/main.js"], { ready: appRunning }, async (service, base) => {
        await exerciseCjsApp(ctx, base);
    });
}

// (d) nest start --watch (tsc watch through createWatchCompilerHost, restart on "Found 0 errors").
async function runCjsPathsWatch(ctx) {
    if (!(await prepareCjsFixture(ctx))) {
        return;
    }
    const servicePath = "src/app.service.ts";
    await ctx.withService("watch", "npm", ["run", "start:dev"], { ready: watchReport, readyTimeoutMs: 3 * minute }, async (service, base) => {
        ctx.metrics.firstBuildMs = Date.now() - service.startedAt;
        expectZeroErrors(service, 1);
        await service.waitFor(appRunning, { timeoutMs: minute });
        ctx.metrics.firstStartMs = Date.now() - service.startedAt;
        const first = await ctx.get(base, "/");
        ctx.saveHttp("root-before-edit", first);
        ctx.check("first build serves the v1 greeting", first.text === "Hello from nest-cjs-paths v1", first.text.slice(0, 200));
        ctx.saveTree("dist-initial", "dist");

        const source = fs.readFileSync(appFile(ctx, servicePath), "utf8");
        fs.writeFileSync(appFile(ctx, servicePath), source.replace("BUILD_MARKER = 'v1'", "BUILD_MARKER = 'v2'"));
        const edited = Date.now();
        await service.waitFor(/File change detected\. Starting incremental compilation\.\.\./, { timeoutMs: minute });
        await service.waitFor(watchReport, { times: 2, timeoutMs: 2 * minute });
        ctx.metrics.rebuildMs = Date.now() - edited;
        expectZeroErrors(service, 2);
        await service.waitFor(appRunning, { times: 2, timeoutMs: minute });
        ctx.metrics.restartMs = Date.now() - edited;
        const second = await ctx.get(base, "/");
        ctx.saveHttp("root-after-edit", second);
        ctx.check("rebuilt app serves the v2 greeting", second.text === "Hello from nest-cjs-paths v2", second.text.slice(0, 200));
        ctx.saveTree("dist-after-edit", "dist");
    });
    const watch = ctx.steps.at(-1);
    ctx.metrics.watchEvents = watchEvents(ctx.lastServiceOutput ?? "");
    ctx.check("watch reported Found 0 errors twice and restarted the app", watch.ok, watch.error);
}

const watchReport = /Found \d+ errors?\. Watching for file changes\./;

function expectZeroErrors(service, times) {
    if (service.count(foundZeroErrors) < times) {
        throw new Error(`watch report ${times} was not "Found 0 errors"`);
    }
}

// The order of the watch and restart messages, without times (kept in metrics, compared).
function watchEvents(output) {
    const events = [];
    const patterns = [
        ["watch-start", /Starting compilation in watch mode\.\.\./],
        ["file-change", /File change detected\. Starting incremental compilation\.\.\./],
        ["found-errors", /Found \d+ errors?\. Watching for file changes\./],
        ["app-running", appRunning],
    ];
    for (const line of output.split("\n")) {
        for (const [name, pattern] of patterns) {
            const match = line.match(pattern);
            if (match !== null) {
                events.push(name === "found-errors" ? match[0].replace(/\. Watching.*/, "") : name);
            }
        }
    }
    return events;
}

// (e) swc builder with --type-check: the TS program feeds PluginMetadataGenerator, which prints
// src/metadata.ts with the TS printer.
async function runSwaggerSwc(ctx) {
    prepareAppDir(path.join(ensureSource("nest"), "sample/11-swagger"), ctx.appDir);
    ctx.useTarball({ lockKey: "swagger-swc", extraDevDependencies: pins.swc });
    if (!(await npmInstall(ctx, { lockKey: "swagger-swc" }))) {
        return;
    }
    const build = await ctx.step("nest-build-swc", "node_modules/.bin/nest", ["build", "--builder", "swc", "--type-check"], { timeoutMs: 10 * minute });
    ctx.saveStepOutput("build-output.txt", build);
    ctx.saveTree("dist", "dist");
    ctx.saveFile("metadata.ts", "src/metadata.ts");
    const metadata = exists(ctx, "src/metadata.ts") ? fs.readFileSync(appFile(ctx, "src/metadata.ts"), "utf8") : "";
    ctx.check("src/metadata.ts was generated with the CreateCatDto metadata", metadata.includes("CreateCatDto"), `${metadata.length} bytes`);
}

// (f) sample/23-graphql-code-first with the graphql plugin. TS 6.0 turns strict on by default and
// the sample then has 4 errors, so the copy sets strict: false.
async function runGraphqlSample(ctx) {
    prepareAppDir(path.join(ensureSource("nest"), "sample/23-graphql-code-first"), ctx.appDir);
    const tsconfigFile = appFile(ctx, "tsconfig.json");
    const tsconfig = readJson(tsconfigFile);
    tsconfig.compilerOptions.strict = false;
    writeJson(tsconfigFile, tsconfig);
    ctx.note("tsconfig.json: compilerOptions.strict set to false (TS 6.0 default strict gives 4 errors)");
    // The sample misses the express 5 integration that @nestjs/apollo loads at startup.
    ctx.useTarball({ lockKey: "graphql-sample", extraDependencies: pins.graphqlSampleExtraDependencies });
    ctx.note(`package.json: added ${JSON.stringify(pins.graphqlSampleExtraDependencies)} (missing from the sample; the app does not start without it)`);
    if (!(await npmInstall(ctx, { lockKey: "graphql-sample", legacyPeerDeps: true }))) {
        return;
    }
    const build = await ctx.step("nest-build", "npm", ["run", "build"], { timeoutMs: 10 * minute });
    ctx.saveStepOutput("build-output.txt", build);
    ctx.saveTree("dist", "dist");
    saveBuildInfo(ctx);
    if (!exists(ctx, "dist/main.js")) {
        ctx.check("dist/main.js exists", false);
        return;
    }
    const upstreamSchema = fs.readFileSync(appFile(ctx, "schema.gql"), "utf8");
    fs.rmSync(appFile(ctx, "schema.gql"));
    await ctx.withService("app", "node", ["-r", preloadPort, "dist/main.js"], { ready: appRunning }, async (service, base) => {
        const query = { query: "{ recipes(skip: 0, take: 5) { id title description creationDate ingredients } }" };
        const recipes = await ctx.get(base, "/graphql", { method: "POST", body: query });
        ctx.saveHttp("graphql-recipes", recipes);
        ctx.check("POST /graphql answers 200 without errors", recipes.status === 200 && !recipes.text.includes("\"errors\""), recipes.text.slice(0, 300));
    });
    const generated = exists(ctx, "schema.gql");
    ctx.check("the app generated schema.gql", generated);
    if (generated) {
        ctx.saveFile("schema.gql", "schema.gql");
        const schema = fs.readFileSync(appFile(ctx, "schema.gql"), "utf8");
        ctx.check("generated schema.gql equals the sample's committed one", schema === upstreamSchema, undefined, { severity: "info" });
        ctx.check("schema.gql has the plugin-only field Recipe.title", /type Recipe \{[^}]*\btitle: String!/.test(schema));
    }
}
