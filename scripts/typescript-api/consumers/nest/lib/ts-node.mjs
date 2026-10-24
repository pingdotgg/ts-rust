// (i) ts-node 10.9.2 (`node -r ts-node/register`) on ../fixtures/ts-node-script: type-check mode
// (ts.createLanguageService, getEmitOutput and diagnostics) and transpile-only mode
// (ts.transpileModule), each on a working script and on one with a type error, plus the TS 6.0
// rootDir option error (TS5011) that transpileModule reports when tsconfig has no rootDir.

import { minute, prepareFixture } from "./common.mjs";

const expectedMain = '{"greetings":["> Hello, Ada (happy)","> Hello, Linus (grumpy)"],"counts":[["a",2],["b",1]],"calls":["Shop.greetAll(1)"],"lazyMood":"grumpy","file":"main.ts"}';
const transpileOnly = { TS_NODE_TRANSPILE_ONLY: "true" };

const runs = [
    { name: "typecheck-main", file: "src/main.ts", env: {}, expectExit: 0, expect: expectedMain },
    { name: "typecheck-type-error", file: "src/type-error.ts", env: {}, expectExit: 1, expect: "src/type-error.ts(3,7): error TS2322" },
    { name: "transpile-main", file: "src/main.ts", env: transpileOnly, expectExit: 0, expect: expectedMain },
    { name: "transpile-type-error", file: "src/type-error.ts", env: transpileOnly, expectExit: 0, expect: "count is Hello, Ada (happy)" },
    // TS 6.0 behavior that TS 7 may not share, so it does not fail the scenario (compare still counts it).
    {
        name: "transpile-no-rootdir",
        file: "src/main.ts",
        env: { ...transpileOnly, TS_NODE_PROJECT: "tsconfig.no-rootdir.json" },
        expectExit: 1,
        expect: "error TS5011",
        required: false,
    },
];

// ts-node's stack frames name its own source lines, not the compiler's; the message is the result.
function withoutStackFrames(text) {
    return text.split("\n").filter((line) => !/^\s+at /.test(line)).join("\n");
}

export async function runTsNode(ctx) {
    if (!(await prepareFixture(ctx, "ts-node-script"))) {
        return;
    }
    for (const run of runs) {
        const required = run.required ?? true;
        const step = await ctx.step(run.name, "node", ["-r", "ts-node/register", run.file], {
            env: run.env,
            expectExit: run.expectExit,
            required,
            timeoutMs: 3 * minute,
        });
        const output = ctx.saveStepOutput(`out/${run.name}.txt`, step, withoutStackFrames);
        ctx.check(`${run.name} prints ${JSON.stringify(run.expect.slice(0, 60))}`, output.includes(run.expect), output.slice(0, 300), {
            severity: required ? "required" : "info",
        });
    }
}
