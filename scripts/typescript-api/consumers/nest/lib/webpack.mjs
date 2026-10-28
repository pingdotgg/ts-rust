// (g) nest build --webpack on the CommonJS fixture. nest-cli's webpack defaults
// (lib/compiler/defaults/webpack-defaults.ts) pick ts-loader with transpileOnly plus
// fork-ts-checker-webpack-plugin when no compiler plugin is registered (g1), and ts-loader in full
// mode (language service getEmitOutput, getCustomTransformers(program)) without fork-ts-checker
// when one is (g2, the swagger plugin).

import fs from "node:fs";

import { appFile, appRunning, exerciseCjsApp, exists, findAliasedSpecifiers, minute, prepareCjsFixture, saveBuildInfo } from "./common.mjs";
import { pins } from "./sources.mjs";
import { readJson, writeJson } from "./util.mjs";

const lockKey = "cjs-paths-webpack";

export async function runWebpack(ctx) {
    const edit = () => {
        const file = appFile(ctx, "nest-cli.json");
        const config = readJson(file);
        delete config.compilerOptions.plugins;
        writeJson(file, config);
    };
    if (!(await prepareCjsFixture(ctx, { lockKey, extraDevDependencies: pins.webpack, edit }))) {
        return;
    }
    ctx.note("nest-cli.json: compilerOptions.plugins removed (no plugin: ts-loader transpileOnly plus fork-ts-checker-webpack-plugin)");
    await buildRunAndBreak(ctx, { checkPluginSchema: false, typeErrorFrom: "fork-ts-checker" });
}

export async function runWebpackSwagger(ctx) {
    if (!(await prepareCjsFixture(ctx, { lockKey, extraDevDependencies: pins.webpack }))) {
        return;
    }
    await buildRunAndBreak(ctx, { checkPluginSchema: true, typeErrorFrom: "ts-loader" });
}

// webpack prints asset and module sizes; the bundle itself is an artifact, so a size change is
// not counted twice.
function normalizeWebpack(text) {
    return text.replace(/\b\d+(\.\d+)? (bytes|KiB|MiB)\b/g, "N $2");
}

async function buildRunAndBreak(ctx, { checkPluginSchema, typeErrorFrom }) {
    const build = await ctx.step("nest-build-webpack", "node_modules/.bin/nest", ["build", "--webpack"], { timeoutMs: 10 * minute });
    const output = ctx.saveStepOutput("build-output.txt", build, normalizeWebpack);
    ctx.saveTree("dist", "dist");
    saveBuildInfo(ctx);
    ctx.check("the build reports no TypeScript error", !/\bTS\d{4,5}\b/.test(output), output.match(/.*\bTS\d{4,5}\b.*/)?.[0]);
    const aliased = findAliasedSpecifiers(ctx, /\.js$/);
    ctx.check("the bundle has no unresolved @app/@common/src aliases", exists(ctx, "dist/main.js") && aliased.length === 0, aliased.slice(0, 20).join("; "));
    if (!exists(ctx, "dist/main.js")) {
        ctx.check("dist/main.js exists", false);
        return;
    }
    await ctx.withService("app", "node", ["dist/main.js"], { ready: appRunning }, async (service, base) => {
        await exerciseCjsApp(ctx, base, { checkPluginSchema });
    });

    // A type error the transpiler alone does not see: the checker (fork-ts-checker in g1, ts-loader's
    // own diagnostics in g2) must fail the build with TS2322.
    const servicePath = appFile(ctx, "src/app.service.ts");
    fs.appendFileSync(servicePath, "\nexport const TYPE_ERROR: number = BUILD_MARKER;\n");
    const broken = await ctx.step("nest-build-webpack-type-error", "node_modules/.bin/nest", ["build", "--webpack"], { timeoutMs: 10 * minute, expectExit: "nonzero" });
    const brokenOutput = ctx.saveStepOutput("build-type-error-output.txt", broken, normalizeWebpack);
    ctx.check(`${typeErrorFrom} reports TS2322 for the type error`, /TS2322/.test(brokenOutput), brokenOutput.match(/.*TS2322.*/)?.[0]);
}
