"use strict";

// Mocha reporter for the TS 6.0 test runs: one JSON line per test as it finishes, so a crashed run
// still leaves every finished result. Tests that a failed "before" hook skips are recorded as failed,
// not left out. The last line is {"event":"end",...}; a file without it comes from a crashed run.
const fs = require("node:fs");

const PROGRESS_EVERY = 1000;
const MAX_ERROR_LENGTH = 400;

function firstLine(err) {
    const message = err && typeof err === "object" ? String(err.message ?? err) : String(err);
    const line = message.split("\n").find(part => part.trim() !== "") ?? "";
    return line.trim().slice(0, MAX_ERROR_LENGTH);
}

function topTitle(runnable) {
    const path = runnable.titlePath();
    return path.length > 0 ? path[0] : "";
}

function collectTests(suite, out) {
    for (const test of suite.tests) {
        out.push(test);
    }
    for (const child of suite.suites) {
        collectTests(child, out);
    }
    return out;
}

function ResultsReporter(runner, options) {
    const output = options.reporterOptions && options.reporterOptions.output;
    if (!output) {
        throw new Error("reporter.cjs needs -O output=<file>");
    }
    const fd = fs.openSync(output, "w");
    const seen = new Set();
    const counts = { pass: 0, fail: 0, pending: 0, hookFailures: 0 };
    const grep = runner._grep;
    const invert = runner._invert;

    function write(record) {
        fs.writeSync(fd, JSON.stringify(record) + "\n");
    }

    function isSelected(test) {
        if (!grep) {
            return true;
        }
        const matches = grep.test(test.fullTitle());
        return invert ? !matches : matches;
    }

    function report(test, state, error) {
        if (seen.has(test)) {
            return;
        }
        seen.add(test);
        counts[state]++;
        const record = { title: test.fullTitle(), top: topTitle(test), state, ms: test.duration ?? 0 };
        if (error !== undefined) {
            record.error = error;
        }
        write(record);
        const done = counts.pass + counts.fail + counts.pending;
        if (done % PROGRESS_EVERY === 0) {
            console.log(`[ts6-tests] ${done} tests: pass=${counts.pass} fail=${counts.fail} pending=${counts.pending}`);
        }
    }

    function reportHookFailure(hook, err) {
        counts.hookFailures++;
        const error = firstLine(err);
        const isBefore = /"before (all|each)" hook/.test(hook.title);
        if (!isBefore) {
            write({ title: hook.fullTitle(), top: topTitle(hook), state: "fail", hook: true, error });
            return;
        }
        // Mocha skips every test under a failed "before" hook without reporting it.
        for (const test of collectTests(hook.parent, [])) {
            if (isSelected(test)) {
                report(test, "fail", `${hook.originalTitle ?? hook.title} failed: ${error}`);
            }
        }
    }

    runner.on("pass", test => report(test, "pass"));
    runner.on("pending", test => report(test, "pending"));
    runner.on("fail", (runnable, err) => {
        if (runnable.type === "hook") {
            reportHookFailure(runnable, err);
            return;
        }
        report(runnable, "fail", firstLine(err));
    });
    runner.once("end", () => {
        write({ event: "end", ...counts });
        fs.closeSync(fd);
        console.log(`[ts6-tests] end: pass=${counts.pass} fail=${counts.fail} pending=${counts.pending} hookFailures=${counts.hookFailures}`);
    });
}

module.exports = ResultsReporter;
