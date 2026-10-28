// Result files of typescript-eslint.sh runs.
// usage: node results.mjs meta <out-dir> --clone DIR --tag TAG --commit SHA --package-arg ARG --package-dir DIR
//                              --shard I/N --workers N --packages p1,p2
//        node results.mjs build <out-dir>
//        node results.mjs compare <base-out> <new-out> [--limit N] [--json FILE]
//        node results.mjs merge <out-dir> <in-dir>...
import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

function die(message) {
    console.error(`results.mjs: ${message}`);
    process.exit(2);
}

function readJson(file, fallback) {
    if (!fs.existsSync(file)) {
        if (fallback !== undefined) {
            return fallback;
        }
        die(`${file} is missing`);
    }
    return JSON.parse(fs.readFileSync(file, "utf8"));
}

function writeJson(file, value, indent) {
    fs.writeFileSync(file, `${JSON.stringify(value, null, indent)}\n`);
}

function readJsonl(file) {
    if (!fs.existsSync(file)) {
        return [];
    }
    const records = [];
    for (const line of fs.readFileSync(file, "utf8").split("\n")) {
        if (line.trim() === "") {
            continue;
        }
        try {
            records.push(JSON.parse(line));
        } catch {
            // A run killed in the middle of a write leaves one partial last line.
        }
    }
    return records;
}

function parseFlags(args) {
    const flags = {};
    const rest = [];
    for (let i = 0; i < args.length; i += 1) {
        const arg = args[i];
        if (!arg.startsWith("--")) {
            rest.push(arg);
            continue;
        }
        const eq = arg.indexOf("=");
        if (eq > 0) {
            flags[arg.slice(2, eq)] = arg.slice(eq + 1);
            continue;
        }
        flags[arg.slice(2)] = args[i + 1];
        i += 1;
    }
    return { flags, rest };
}

function sha256File(file) {
    return createHash("sha256").update(fs.readFileSync(file)).digest("hex");
}

function sha256Tree(dir) {
    if (!fs.existsSync(dir)) {
        return null;
    }
    const hash = createHash("sha256");
    const walk = (current) => {
        const entries = fs.readdirSync(current, { withFileTypes: true }).sort((a, b) => a.name.localeCompare(b.name));
        for (const entry of entries) {
            const full = path.join(current, entry.name);
            if (entry.isDirectory()) {
                walk(full);
                continue;
            }
            if (entry.isFile()) {
                hash.update(`${path.relative(dir, full)}\0`);
                hash.update(fs.readFileSync(full));
            }
        }
    };
    walk(dir);
    return hash.digest("hex");
}

function git(dir, args) {
    try {
        return execFileSync("git", ["-C", dir, ...args], { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] }).trim();
    } catch {
        return null;
    }
}

// With --copy, `dir` is the copy and `source` the directory it came from (git facts come from the source).
function describePackage(arg, dir, source) {
    if (!dir) {
        return { arg, dir: null, kind: "stock" };
    }
    const manifest = readJson(path.join(dir, "package.json"));
    const head = git(source, ["rev-parse", "HEAD"]);
    const changed = head ? git(source, ["status", "--porcelain", "--", "."]) : null;
    return {
        arg,
        dir,
        source,
        kind: "swapped",
        name: manifest.name,
        version: manifest.version,
        libTypescriptSha256: sha256File(path.join(dir, "lib", "typescript.js")),
        srcSha256: sha256Tree(path.join(dir, "src")),
        gitHead: head,
        gitChangedFiles: changed === null ? null : changed.split("\n").filter(Boolean).length,
    };
}

function samePackage(a, b) {
    return a.kind === b.kind && a.libTypescriptSha256 === b.libTypescriptSha256 && a.srcSha256 === b.srcSha256;
}

function cmdMeta(args) {
    const { flags, rest } = parseFlags(args);
    const out = rest[0];
    if (!out) {
        die("meta: need <out-dir>");
    }
    const runFile = path.join(out, "run.json");
    const previous = readJson(runFile, null);
    const run = {
        consumer: "typescript-eslint",
        clone: { dir: flags.clone, tag: flags.tag, commit: flags.commit },
        package: describePackage(flags["package-arg"], flags["package-dir"] || null, flags["package-source"] || flags["package-dir"]),
        probe: readJson(path.join(out, "probe.json")),
        packages: flags.packages.split(",").filter(Boolean),
        shard: flags.shard || null,
        files: flags.files ? flags.files.split(",").filter(Boolean) : null,
        workers: Number(flags.workers),
        start: new Date().toISOString(),
        host: {
            name: os.hostname(),
            cpus: os.cpus().length,
            memoryGiB: Math.round(os.totalmem() / 2 ** 30),
            loadAverage: os.loadavg().map((each) => Math.round(each * 100) / 100),
            node: process.version,
        },
    };
    if (previous) {
        if (!samePackage(previous.package, run.package)) {
            console.error("results.mjs: WARNING --resume with a package that differs from the first part of this run");
        }
        run.resumes = [...(previous.resumes ?? []), { time: run.start, package: run.package, host: run.host }];
        run.start = previous.start;
        run.package = previous.package;
        run.packages = [...new Set([...previous.packages, ...run.packages])];
    }
    writeJson(runFile, run, 4);
}

function plannedFiles(records) {
    const files = new Set();
    for (const record of records) {
        if (record.type === "planned") {
            for (const file of record.files) {
                files.add(file);
            }
        }
    }
    return [...files];
}

function emptySummary(packageName, runs, planned, fileRecords, ends) {
    return {
        package: packageName,
        ran: runs.length > 0,
        rc: runs.length > 0 ? Math.max(...runs.map((each) => each.rc)) : null,
        wallSeconds: Math.round(runs.reduce((sum, each) => sum + each.wallSeconds, 0) * 10) / 10,
        runs: runs.length,
        filesPlanned: planned.length,
        filesRun: fileRecords.size,
        filesFailed: 0,
        fileErrors: 0,
        tests: 0,
        passed: 0,
        failed: 0,
        skipped: 0,
        todo: 0,
        unhandled: ends.reduce((sum, each) => sum + each.unhandled.length, 0),
        unhandledErrors: ends.flatMap((each) => each.unhandled.map((u) => u.error)).slice(0, 5),
        endReasons: ends.map((each) => each.reason),
    };
}

function collectPackage(out, run, packageName, packageRuns) {
    const runs = packageRuns.filter((each) => each.package === packageName);
    const fileRecords = new Map();
    const ends = [];
    const records = readJsonl(path.join(out, "raw", `${packageName}.jsonl`));
    for (const record of records) {
        if (record.type === "file") {
            fileRecords.set(record.file, record);
        } else if (record.type === "end") {
            ends.push(record);
        }
    }
    const planned = plannedFiles(records);
    const summary = emptySummary(packageName, runs, planned, fileRecords, ends);
    const files = [];
    const tests = [];
    for (const record of fileRecords.values()) {
        const count = { passed: 0, failed: 0, skipped: 0 };
        for (const test of record.tests) {
            const entry = { package: packageName, file: record.file, name: test.name, state: test.state };
            if (test.error) {
                entry.error = test.error;
            }
            tests.push(entry);
            summary.tests += 1;
            if (test.state === "passed" || test.state === "failed") {
                count[test.state] += 1;
                summary[test.state] += 1;
            } else {
                count.skipped += 1;
                summary[test.state === "todo" ? "todo" : "skipped"] += 1;
            }
        }
        const fileEntry = { package: packageName, file: record.file, state: record.state, ms: record.ms, tests: record.tests.length, ...count };
        if (record.error) {
            fileEntry.error = record.error;
            summary.fileErrors += 1;
        }
        if (record.state === "failed") {
            summary.filesFailed += 1;
        }
        files.push(fileEntry);
    }
    for (const file of planned) {
        if (!fileRecords.has(file)) {
            files.push({ package: packageName, file, state: "unrun", ms: 0, tests: 0, passed: 0, failed: 0, skipped: 0 });
        }
    }
    return { summary, files, tests };
}

function collect(out) {
    const run = readJson(path.join(out, "run.json"));
    const packageRuns = readJsonl(path.join(out, "raw", "packages.jsonl"));
    const packages = [];
    const files = [];
    const tests = [];
    for (const packageName of run.packages) {
        const collected = collectPackage(out, run, packageName, packageRuns);
        packages.push(collected.summary);
        files.push(...collected.files);
        tests.push(...collected.tests);
    }
    return { run, packages, files, tests };
}

function failingFileLines(files, tests, packageName, limit) {
    const failing = files
        .filter((each) => each.package === packageName && (each.failed > 0 || each.error))
        .sort((a, b) => b.failed - a.failed || a.file.localeCompare(b.file));
    const lines = failing.slice(0, limit).map((file) => {
        const firstError = file.error ?? tests.find((test) => test.package === packageName && test.file === file.file && test.state === "failed")?.error;
        return `- \`${file.file}\`: ${file.failed} of ${file.tests} failed${file.error ? " (file error)" : ""}: ${firstError ?? ""}`;
    });
    if (failing.length > limit) {
        lines.push(`- ... ${failing.length - limit} more failing files (files.json)`);
    }
    return lines;
}

function headerLines(run) {
    const pkg = run.package;
    const lines = [];
    lines.push(`# typescript-eslint ${run.clone.tag} against ${pkg.kind === "stock" ? "stock typescript@6.0.3" : pkg.dir}`);
    lines.push("");
    lines.push(`- typescript ${run.probe.version} loaded from \`${run.probe.loadedFrom}\` (${run.probe.exports} exports)`);
    if (pkg.kind !== "stock") {
        lines.push(
            `- package ${pkg.name}@${pkg.version}, lib/typescript.js sha256 ${pkg.libTypescriptSha256.slice(0, 16)}, src sha256 ${pkg.srcSha256?.slice(0, 16) ?? "none"}, git ${pkg.gitHead?.slice(0, 12) ?? "none"} (${pkg.gitChangedFiles ?? "?"} changed files under the package)`,
        );
    }
    if (run.merged) {
        lines.push(`- merged from ${run.merged.length} runs: ${run.merged.map((each) => `\`${each.dir}\` (shard ${each.shard ?? "none"})`).join(", ")}`);
    }
    lines.push(`- clone \`${run.clone.dir}\` at ${run.clone.tag} (${run.clone.commit.slice(0, 12)}); vitest --maxWorkers=${run.workers}, ${runScope(run)}`);
    lines.push(`- started ${run.start} on ${run.host.name} (${run.host.cpus} cores, ${run.host.memoryGiB} GiB, load ${run.host.loadAverage.join(" ")}), node ${run.host.node}`);
    return lines;
}

function summaryMarkdown({ run, packages, files, tests }) {
    const lines = headerLines(run);
    lines.push("");
    lines.push("| Package | Files run/planned | Tests | Passed | Failed | Skipped | Todo | File errors | Unhandled | vitest rc | Wall s |");
    lines.push("|---|---|---|---|---|---|---|---|---|---|---|");
    const total = { filesRun: 0, filesPlanned: 0, tests: 0, passed: 0, failed: 0, skipped: 0, todo: 0, fileErrors: 0, unhandled: 0, wallSeconds: 0 };
    for (const each of packages) {
        const rc = each.ran ? String(each.rc) : each.filesRun > 0 ? "stopped" : "not run";
        lines.push(
            `| ${each.package} | ${each.filesRun}/${each.filesPlanned} | ${each.tests} | ${each.passed} | ${each.failed} | ${each.skipped} | ${each.todo} | ${each.fileErrors} | ${each.unhandled} | ${rc} | ${each.wallSeconds} |`,
        );
        for (const key of Object.keys(total)) {
            total[key] += each[key];
        }
    }
    lines.push(
        `| **total** | ${total.filesRun}/${total.filesPlanned} | ${total.tests} | ${total.passed} | ${total.failed} | ${total.skipped} | ${total.todo} | ${total.fileErrors} | ${total.unhandled} | | ${Math.round(total.wallSeconds * 10) / 10} |`,
    );
    const failing = packages.filter((each) => each.failed > 0 || each.fileErrors > 0 || each.unhandled > 0 || each.filesRun < each.filesPlanned);
    if (failing.length > 0) {
        lines.push("");
        lines.push("## Failures");
        for (const each of failing) {
            lines.push("");
            lines.push(`### ${each.package}`);
            lines.push("");
            lines.push(...failingFileLines(files, tests, each.package, 15));
            for (const error of each.unhandledErrors) {
                lines.push(`- unhandled: ${error}`);
            }
            const unrun = files.filter((file) => file.package === each.package && file.state === "unrun");
            if (unrun.length > 0) {
                const shown = unrun.slice(0, 10).map((file) => `\`${file.file}\``).join(", ");
                lines.push(`- ${unrun.length} planned files did not run: ${shown}${unrun.length > 10 ? ", ..." : ""}`);
            }
        }
    }
    lines.push("");
    lines.push("Logs: `logs/<package>.log` (one line per file); error details of the first failures per file: `raw/<package>.jsonl`.");
    return `${lines.join("\n")}\n`;
}

function cmdBuild(args) {
    const out = args[0];
    if (!out) {
        die("build: need <out-dir>");
    }
    const collected = collect(out);
    const run = { ...collected.run, end: new Date().toISOString(), summary: collected.packages };
    writeJson(path.join(out, "run.json"), run, 4);
    writeJson(path.join(out, "results.json"), collected.tests);
    writeJson(path.join(out, "files.json"), collected.files, 1);
    fs.writeFileSync(path.join(out, "summary.md"), summaryMarkdown({ ...collected, run }));
    const total = collected.packages.reduce(
        (sum, each) => ({ tests: sum.tests + each.tests, passed: sum.passed + each.passed, failed: sum.failed + each.failed }),
        { tests: 0, passed: 0, failed: 0 },
    );
    console.log(`results: ${total.tests} tests, ${total.passed} passed, ${total.failed} failed in ${collected.packages.length} packages`);
}

function mergedShard(runs) {
    const shards = runs.map(({ run }) => run.shard);
    if (shards.some((each) => !each)) {
        return null;
    }
    const counts = new Set(shards.map((each) => each.split("/")[1]));
    const indexes = new Set(shards.map((each) => each.split("/")[0]));
    if (counts.size === 1 && indexes.size === Number([...counts][0])) {
        return null;
    }
    return shards.join(",");
}

function cmdMerge(args) {
    const [out, ...inputs] = args;
    if (!out || inputs.length === 0) {
        die("merge: need <out-dir> <in-dir>...");
    }
    if (fs.existsSync(out) && fs.readdirSync(out).length > 0) {
        die(`merge: ${out} is not empty`);
    }
    fs.mkdirSync(path.join(out, "raw"), { recursive: true });
    const runs = inputs.map((dir) => ({ dir: path.resolve(dir), run: readJson(path.join(dir, "run.json")) }));
    const first = runs[0].run;
    for (const { dir, run } of runs) {
        if (!samePackage(run.package, first.package)) {
            die(`merge: ${dir} tested a different package than ${runs[0].dir}`);
        }
        if (run.clone.commit !== first.clone.commit) {
            die(`merge: ${dir} ran another typescript-eslint commit`);
        }
    }
    const packageNames = [...new Set(runs.flatMap(({ run }) => run.packages))];
    const appendRaw = (name, text) => fs.appendFileSync(path.join(out, "raw", name), text);
    for (const { dir } of runs) {
        const packagesFile = path.join(dir, "raw", "packages.jsonl");
        if (fs.existsSync(packagesFile)) {
            appendRaw("packages.jsonl", fs.readFileSync(packagesFile, "utf8"));
        }
    }
    for (const name of packageNames) {
        for (const { dir } of runs) {
            const jsonl = path.join(dir, "raw", `${name}.jsonl`);
            if (fs.existsSync(jsonl)) {
                appendRaw(`${name}.jsonl`, fs.readFileSync(jsonl, "utf8"));
            }
        }
    }
    fs.copyFileSync(path.join(runs[0].dir, "probe.json"), path.join(out, "probe.json"));
    const merged = {
        ...first,
        packages: packageNames,
        shard: mergedShard(runs),
        merged: runs.map(({ dir, run }) => ({ dir, shard: run.shard, start: run.start, end: run.end })),
    };
    delete merged.summary;
    writeJson(path.join(out, "run.json"), merged, 4);
    cmdBuild([out]);
}

function load(dir) {
    const run = readJson(path.join(dir, "run.json"));
    const tests = readJson(path.join(dir, "results.json"));
    const files = readJson(path.join(dir, "files.json"));
    const byPackage = new Map(run.packages.map((name) => [name, { tests: new Map(), files: new Map() }]));
    for (const file of files) {
        byPackage.get(file.package)?.files.set(file.file, file);
    }
    for (const test of tests) {
        const entry = byPackage.get(test.package);
        if (!entry) {
            continue;
        }
        const key = `${test.file}\u0000${test.name}`;
        const list = entry.tests.get(key) ?? [];
        list.push(test);
        entry.tests.set(key, list);
    }
    return { dir, run, byPackage };
}

function label(test) {
    return `${test.file} > ${test.name}`;
}

function bucket(state) {
    if (state === "passed" || state === "failed") {
        return state;
    }
    return "skipped";
}

function countStates(packageTests) {
    const counts = { passed: 0, failed: 0, skipped: 0 };
    for (const list of packageTests?.tests.values() ?? []) {
        for (const test of list) {
            counts[bucket(test.state)] += 1;
        }
    }
    return counts;
}

function partialRun(run) {
    return Boolean(run.shard || run.files?.length);
}

function runScope(run) {
    const parts = [`shard ${run.shard ?? "none"}`];
    if (run.files?.length) {
        parts.push(`files ${run.files.join(",")}`);
    }
    return parts.join(", ");
}

// Tests are keyed by file and full name; repeated names in one file pair up in order.
function comparePackage(base, next, name) {
    const basePackage = base.byPackage.get(name);
    const nextPackage = next.byPackage.get(name);
    const result = {
        package: name,
        base: countStates(basePackage),
        new: countStates(nextPackage),
        retained: 0,
        lost: [],
        gained: [],
        absent: [],
        unrun: [],
        added: [],
        uncovered: 0,
        uncoveredPassed: 0,
    };
    if (!basePackage) {
        return result;
    }
    // vitest of this package stopped before it planned any file (config or startup error, or never ran).
    const notStarted = nextPackage !== undefined && nextPackage.files.size === 0;
    for (const [key, baseList] of basePackage.tests) {
        const nextFile = nextPackage?.files.get(baseList[0].file);
        const nextList = nextPackage?.tests.get(key) ?? [];
        baseList.forEach((baseTest, index) => {
            const passed = baseTest.state === "passed";
            if (!nextFile && notStarted) {
                if (passed) {
                    result.unrun.push(baseTest);
                }
                return;
            }
            if (!nextFile) {
                if (nextPackage && !partialRun(next.run) && passed) {
                    result.absent.push({ ...baseTest, newError: "file not planned" });
                } else if (!nextPackage || partialRun(next.run)) {
                    result.uncovered += 1;
                    result.uncoveredPassed += passed ? 1 : 0;
                }
                return;
            }
            if (nextFile.state === "unrun") {
                if (passed) {
                    result.unrun.push(baseTest);
                }
                return;
            }
            const nextTest = nextList[index];
            if (!nextTest) {
                if (passed) {
                    result.absent.push({ ...baseTest, newError: nextFile.error ?? "" });
                }
                return;
            }
            if (passed && nextTest.state === "passed") {
                result.retained += 1;
            } else if (passed) {
                result.lost.push({ ...nextTest, baseState: baseTest.state });
            } else if (nextTest.state === "passed") {
                result.gained.push({ ...nextTest, baseState: baseTest.state });
            }
        });
    }
    for (const [key, nextList] of nextPackage?.tests ?? []) {
        const baseFile = basePackage.files.get(nextList[0].file);
        if (!baseFile || baseFile.state === "unrun") {
            continue;
        }
        const baseCount = basePackage.tests.get(key)?.length ?? 0;
        result.added.push(...nextList.slice(baseCount));
    }
    return result;
}

function describeRun(run) {
    if (run.package.kind === "stock") {
        return "stock typescript@6.0.3";
    }
    return `${run.package.dir} (lib ${run.package.libTypescriptSha256.slice(0, 12)}, src ${run.package.srcSha256?.slice(0, 12) ?? "none"})`;
}

function compareMarkdown(baseDir, nextDir, base, next, results, limit) {
    const lines = [];
    lines.push("# typescript-eslint compare");
    lines.push("");
    lines.push(`- base: \`${path.resolve(baseDir)}\`: ${describeRun(base.run)}, ${runScope(base.run)}`);
    lines.push(`- new: \`${path.resolve(nextDir)}\`: ${describeRun(next.run)}, ${runScope(next.run)}`);
    lines.push(
        "- lost: passed in base, failed or skipped in new. absent: passed in base, missing from a file the new run ran. unrun: passed in base, its file did not finish in new. gained: not passed in base, passed in new. added: only in new. uncovered: base tests in files or packages the new run did not plan (other shards, other packages).",
    );
    lines.push("");
    lines.push("| Package | Base pass/fail/skip | New pass/fail/skip | Retained | Lost | Gained | Absent | Unrun | Added | Uncovered (passed) |");
    lines.push("|---|---|---|---|---|---|---|---|---|---|");
    for (const each of results) {
        lines.push(
            `| ${each.package} | ${each.base.passed}/${each.base.failed}/${each.base.skipped} | ${each.new.passed}/${each.new.failed}/${each.new.skipped} | ${each.retained} | ${each.lost.length} | ${each.gained.length} | ${each.absent.length} | ${each.unrun.length} | ${each.added.length} | ${each.uncovered} (${each.uncoveredPassed}) |`,
        );
    }
    for (const each of results) {
        const sections = [
            ["lost", each.lost, (test) => `${label(test)}: ${test.error ?? ""}`],
            ["absent", each.absent, (test) => `${label(test)}${test.newError ? `: ${test.newError}` : ""}`],
            ["unrun", each.unrun, (test) => label(test)],
            ["gained", each.gained, (test) => `${label(test)} (was ${test.baseState})`],
            ["added", each.added, (test) => `${label(test)} (${test.state})`],
        ].filter(([, list]) => list.length > 0);
        if (sections.length === 0) {
            continue;
        }
        lines.push("");
        lines.push(`## ${each.package}`);
        for (const [title, list, format] of sections) {
            lines.push("");
            lines.push(`${title} (${list.length}):`);
            for (const test of list.slice(0, limit)) {
                lines.push(`- ${format(test)}`);
            }
            if (list.length > limit) {
                lines.push(`- ... ${list.length - limit} more (--limit N, or --json FILE for all)`);
            }
        }
    }
    return lines.join("\n");
}

function cmdCompare(args) {
    const { flags, rest } = parseFlags(args);
    const [baseDir, nextDir] = rest;
    if (!baseDir || !nextDir) {
        die("compare: need <base-out> <new-out>");
    }
    const limit = Number(flags.limit ?? 25);
    const base = load(baseDir);
    const next = load(nextDir);
    const results = next.run.packages.map((name) => comparePackage(base, next, name));
    const notRun = base.run.packages.filter((name) => !next.run.packages.includes(name));
    console.log(compareMarkdown(baseDir, nextDir, base, next, results, limit));
    if (notRun.length > 0) {
        console.log(`\nBase packages the new run did not run: ${notRun.join(", ")}`);
    }
    if (flags.json) {
        writeJson(flags.json, { base: path.resolve(baseDir), new: path.resolve(nextDir), packages: results }, 1);
    }
    const bad = results.some((each) => each.lost.length > 0 || each.absent.length > 0 || each.unrun.length > 0);
    process.exit(bad ? 1 : 0);
}

const [command, ...args] = process.argv.slice(2);
switch (command) {
    case "meta":
        cmdMeta(args);
        break;
    case "build":
        cmdBuild(args);
        break;
    case "compare":
        cmdCompare(args);
        break;
    case "merge":
        cmdMerge(args);
        break;
    default:
        die(`unknown command ${command ?? ""} (meta, build, compare, merge)`);
}
