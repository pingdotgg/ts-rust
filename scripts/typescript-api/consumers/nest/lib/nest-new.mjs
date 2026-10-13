// (h) Projects from `nest new` (../fixtures/nest-new-app, nest-new-cjs; nest.sh make-nest-new-fixture):
// the template's own test scripts, nest build, and the built app. The default template is ESM with
// vitest (vite transpiles, typescript is only loaded by nest build); the CommonJS template runs
// jest with ts-jest and reads jest.config.ts (which imports typescript) through ts-node. Both lint
// with oxlint --type-aware (oxlint-tsgolint, its own Go TypeScript), which never loads the
// typescript package, so lint is not run.

import { exists, minute, prepareFixture, saveBuildInfo } from "./common.mjs";

const nestStarted = /Nest application successfully started/;

export async function runNestNewEsm(ctx) {
    if (!(await prepareFixture(ctx, "nest-new-app"))) {
        return;
    }
    await ctx.vitest("unit", "npm", ["test", "--"], { timeoutMs: 10 * minute });
    await ctx.vitest("e2e", "npm", ["run", "test:e2e", "--"], { timeoutMs: 10 * minute });
    await buildAndRun(ctx);
}

export async function runNestNewCjs(ctx) {
    if (!(await prepareFixture(ctx, "nest-new-cjs"))) {
        return;
    }
    await ctx.jest("unit", "npm", ["test", "--"], { timeoutMs: 10 * minute });
    await ctx.jest("e2e", "npm", ["run", "test:e2e", "--"], { timeoutMs: 10 * minute });
    await buildAndRun(ctx);
}

async function buildAndRun(ctx) {
    const build = await ctx.step("nest-build", "npm", ["run", "build"], { timeoutMs: 10 * minute });
    ctx.saveStepOutput("build-output.txt", build);
    ctx.saveTree("dist", "dist");
    saveBuildInfo(ctx);
    if (!exists(ctx, "dist/main.js")) {
        ctx.check("dist/main.js exists", false);
        return;
    }
    await ctx.withService("app", "node", ["dist/main.js"], { ready: nestStarted, waitPort: true }, async (service, base) => {
        const root = await ctx.get(base, "/");
        ctx.saveHttp("root", root);
        ctx.check("GET / answers Hello World!", root.status === 200 && root.text === "Hello World!", `${root.status} ${root.text.slice(0, 200)}`);
    });
}
