// A vitest 4 reporter that appends one JSON line per finished test file, so a killed or partial run keeps
// every file that finished. typescript-eslint.sh passes it with --reporter and these environment variables:
// TSAPI_TSE_JSONL (output file), TSAPI_TSE_PACKAGE (package name), TSAPI_TSE_PACKAGE_DIR (for relative paths).
import fs from "node:fs";
import path from "node:path";

const output = process.env.TSAPI_TSE_JSONL;
const packageName = process.env.TSAPI_TSE_PACKAGE ?? "";
const packageDir = process.env.TSAPI_TSE_PACKAGE_DIR ?? process.cwd();
const detailsPerFile = 5;
const ansi = /\u001b\[[0-9;]*m/g;

function write(record) {
    if (!output) {
        return;
    }
    fs.appendFileSync(output, `${JSON.stringify(record)}\n`);
}

function firstLine(text) {
    const lines = String(text ?? "").replace(ansi, "").split("\n");
    const line = lines.find((each) => each.trim() !== "") ?? "";
    return line.trim().slice(0, 300);
}

function errorText(error) {
    if (!error) {
        return "";
    }
    if (typeof error !== "object") {
        return String(error);
    }
    return error.message ?? error.name ?? JSON.stringify(error);
}

function errorDetail(error) {
    const message = errorText(error).replace(ansi, "");
    const stack = String(error?.stack ?? "").replace(ansi, "");
    const body = stack.includes(message) ? stack : `${message}\n${stack}`;
    return body.slice(0, 4000);
}

function relative(moduleId) {
    return path.relative(packageDir, moduleId).split(path.sep).join("/");
}

function testState(test) {
    if (test.options.mode === "todo") {
        return "todo";
    }
    return test.result().state;
}

export default class TsapiTypescriptEslintReporter {
    onInit(vitest) {
        this.vitest = vitest;
        this.counts = { files: 0, tests: 0, passed: 0, failed: 0, skipped: 0 };
        write({ type: "start", package: packageName, pid: process.pid, time: new Date().toISOString() });
    }

    // The files this run will run. vitest reports them before --shard picks its part (the pool shards them
    // with the configured sequencer), and `vitest list` ignores --shard, so shard them the same way here.
    async onTestRunStart(specifications) {
        let selected = [...specifications];
        const shard = this.vitest?.config.shard;
        if (shard) {
            const Sequencer = this.vitest.config.sequence.sequencer;
            selected = await new Sequencer(this.vitest).shard(selected);
        }
        const files = [...new Set(selected.map((each) => relative(each.moduleId)))].sort();
        write({ type: "planned", files, shard: shard ? `${shard.index}/${shard.count}` : null });
    }

    onTestModuleEnd(testModule) {
        const file = relative(testModule.moduleId);
        const tests = [];
        let details = 0;
        const count = { passed: 0, failed: 0, skipped: 0 };
        for (const test of testModule.children.allTests()) {
            const state = testState(test);
            const entry = { name: test.fullName, state };
            const errors = test.result().errors ?? [];
            if (errors.length > 0) {
                entry.error = firstLine(errorText(errors[0]));
                if (state === "failed" && details < detailsPerFile) {
                    entry.detail = errorDetail(errors[0]);
                    details += 1;
                }
            }
            entry.ms = Math.round(test.diagnostic()?.duration ?? 0);
            tests.push(entry);
            if (state === "passed") {
                count.passed += 1;
            } else if (state === "failed") {
                count.failed += 1;
            } else {
                count.skipped += 1;
            }
        }
        const moduleErrors = testModule.errors();
        const record = {
            type: "file",
            file,
            state: testModule.state(),
            ms: Math.round(testModule.diagnostic().duration ?? 0),
            tests,
        };
        if (moduleErrors.length > 0) {
            record.error = firstLine(errorText(moduleErrors[0]));
            record.detail = errorDetail(moduleErrors[0]);
        }
        write(record);
        this.counts.files += 1;
        this.counts.tests += tests.length;
        this.counts.passed += count.passed;
        this.counts.failed += count.failed;
        this.counts.skipped += count.skipped;
        const label = record.state === "passed" ? "PASS" : record.state.toUpperCase();
        const suffix = record.error ? ` (${record.error})` : "";
        process.stdout.write(
            `${label} ${file} ${count.passed} passed, ${count.failed} failed, ${count.skipped} skipped, ${record.ms} ms${suffix}\n`,
        );
    }

    onTestRunEnd(testModules, unhandledErrors, reason) {
        const unhandled = unhandledErrors.map((error) => ({
            error: firstLine(errorText(error)),
            detail: errorDetail(error),
        }));
        write({ type: "end", reason, files: testModules.length, unhandled, time: new Date().toISOString() });
        for (const each of unhandled) {
            process.stdout.write(`UNHANDLED ${each.detail}\n`);
        }
        const c = this.counts;
        process.stdout.write(
            `END ${reason}: ${c.files} files, ${c.tests} tests, ${c.passed} passed, ${c.failed} failed, ${c.skipped} skipped, ${unhandled.length} unhandled errors\n`,
        );
    }
}
