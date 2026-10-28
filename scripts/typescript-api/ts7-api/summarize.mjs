// usage: node summarize.mjs <results.jsonl> [shim.log]
// Prints the summary of a TS7 API test run as JSON: tests by status, split into the sync suite
// (test/sync), the async suite (test/async, always a tsgo child process) and the others, plus the
// failing test names and the channels the shim opened.

import fs from "node:fs";

const [resultsFile, shimLog] = process.argv.slice(2);
if (!resultsFile || resultsFile === "help") {
    console.log("usage: node summarize.mjs <results.jsonl> [shim.log]");
    process.exit(resultsFile ? 0 : 2);
}

const groups = {};
const failures = [];
for (const line of fs.readFileSync(resultsFile, "utf8").split("\n")) {
    if (!line) continue;
    const result = JSON.parse(line);
    if (result.kind !== "test") continue;
    const file = result.file ?? "";
    const group = file.includes("test/sync/") ? "sync" : file.includes("test/async/") ? "async" : "other";
    groups[group] ??= { total: 0, pass: 0, fail: 0, skip: 0, todo: 0 };
    groups.total ??= { total: 0, pass: 0, fail: 0, skip: 0, todo: 0 };
    for (const counts of [groups[group], groups.total]) {
        counts.total++;
        counts[result.status]++;
    }
    if (result.status === "fail") {
        failures.push({ file, name: result.name, message: result.message?.slice(0, 300) });
    }
}
const channels = {};
if (shimLog && fs.existsSync(shimLog)) {
    for (const line of fs.readFileSync(shimLog, "utf8").split("\n")) {
        const kind = line.split(" ")[0];
        if (kind) channels[kind] = (channels[kind] ?? 0) + 1;
    }
}
console.log(JSON.stringify({ ...groups, channels, failures }, null, 2));
