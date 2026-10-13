// nest.sh compare <base-out> <new-out> [--out <dir>] [--max-diff-lines N]
//
// Diffs two run outputs: scenario status, step and check changes, per-test pass/fail changes,
// artifacts byte by byte (dist trees, api-json, metadata.ts, schema.gql, HTTP answers, build
// output) with unified diff excerpts, and the watch event order. Timings are reported, never
// counted as differences.

import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

import { formatMs, hashTree, readJson, readJsonIfExists, writeJson } from "./lib/util.mjs";

function usage() {
    return "usage: nest.sh compare <base-out> <new-out> [--out <dir>] [--max-diff-lines N]";
}

function parseArgs(argv) {
    const options = { positional: [], out: undefined, maxDiffLines: 40 };
    for (let i = 0; i < argv.length; i++) {
        const arg = argv[i];
        if (arg === "--out") {
            options.out = argv[++i];
        } else if (arg === "--max-diff-lines") {
            options.maxDiffLines = Number(argv[++i]);
        } else if (arg.startsWith("--")) {
            throw new Error(`unknown option ${arg}`);
        } else {
            options.positional.push(arg);
        }
    }
    if (options.positional.length !== 2 || !(options.maxDiffLines > 0)) {
        throw new Error("expected <base-out> and <new-out>");
    }
    return options;
}

function canonicalJson(value) {
    if (Array.isArray(value)) {
        return value.map(canonicalJson);
    }
    if (value !== null && typeof value === "object") {
        return Object.fromEntries(Object.keys(value).sort().map((key) => [key, canonicalJson(value[key])]));
    }
    return value;
}

function parseJsonFile(file) {
    try {
        return JSON.parse(fs.readFileSync(file, "utf8"));
    } catch {
        return undefined;
    }
}

let tempDir;

function prettyJsonFile(file, label) {
    tempDir ??= fs.mkdtempSync(path.join(os.tmpdir(), "nest-compare-"));
    const target = path.join(tempDir, `${label}-${path.basename(file)}`);
    const value = parseJsonFile(file);
    fs.writeFileSync(target, value === undefined ? fs.readFileSync(file) : `${JSON.stringify(canonicalJson(value), null, 2)}\n`);
    return target;
}

function unifiedDiff(baseFile, newFile, label) {
    const result = spawnSync("diff", ["-u", "--label", `base/${label}`, "--label", `new/${label}`, baseFile, newFile], {
        encoding: "utf8",
        maxBuffer: 256 * 1024 * 1024,
    });
    return result.stdout || result.stderr || "";
}

function excerpt(diff, maxLines) {
    const lines = diff.split("\n");
    if (lines.length <= maxLines) {
        return diff.trimEnd();
    }
    return `${lines.slice(0, maxLines).join("\n")}\n... (${lines.length - maxLines} more diff lines)`;
}

// A .tsbuildinfo records the compiler's own file versions (lib file hashes, format): diffed, never
// counted, also inside a tree (outDir holds it when rootDir is the project directory).
function isBuildInfo(rel) {
    return rel.endsWith(".tsbuildinfo");
}

// One artifact (a tree, a file or a JSON file) of one scenario in both runs.
function compareArtifact(name, baseInfo, newInfo, baseDir, newDir, options) {
    const kind = (baseInfo ?? newInfo).kind;
    const informational = Boolean(baseInfo?.informational || newInfo?.informational);
    const result = { name, kind, informational, differs: false, files: [] };
    const baseMissing = baseInfo === undefined || baseInfo.missing === true || !fs.existsSync(path.join(baseDir, baseInfo.path));
    const newMissing = newInfo === undefined || newInfo.missing === true || !fs.existsSync(path.join(newDir, newInfo.path));
    if (baseMissing || newMissing) {
        result.differs = baseMissing !== newMissing;
        result.presence = { base: !baseMissing, new: !newMissing };
        return result;
    }
    if (kind === "tree") {
        const baseManifest = hashTree(path.join(baseDir, baseInfo.path));
        const newManifest = hashTree(path.join(newDir, newInfo.path));
        const all = [...new Set([...Object.keys(baseManifest), ...Object.keys(newManifest)])].sort();
        result.total = all.length;
        for (const rel of all) {
            const file = { path: rel };
            if (baseManifest[rel] === undefined) {
                file.change = "added";
            } else if (newManifest[rel] === undefined) {
                file.change = "removed";
            } else if (baseManifest[rel] !== newManifest[rel]) {
                file.change = "differs";
                file.diff = unifiedDiff(path.join(baseDir, baseInfo.path, rel), path.join(newDir, newInfo.path, rel), `${name}/${rel}`);
            } else {
                continue;
            }
            if (isBuildInfo(rel)) {
                file.informational = true;
            }
            result.files.push(file);
        }
        result.same = all.length - result.files.length;
        result.differs = result.files.some((file) => !file.informational);
        result.informationalFiles = result.files.filter((file) => file.informational).length;
        return result;
    }
    const baseFile = path.join(baseDir, baseInfo.path);
    const newFile = path.join(newDir, newInfo.path);
    if (Buffer.compare(fs.readFileSync(baseFile), fs.readFileSync(newFile)) === 0) {
        return result;
    }
    result.differs = true;
    const file = { path: name, change: "differs" };
    if (kind === "json") {
        const baseValue = parseJsonFile(baseFile);
        const newValue = parseJsonFile(newFile);
        file.jsonEqual = baseValue !== undefined && newValue !== undefined
            && JSON.stringify(canonicalJson(baseValue)) === JSON.stringify(canonicalJson(newValue));
        file.diff = unifiedDiff(prettyJsonFile(baseFile, "base"), prettyJsonFile(newFile, "new"), `${name} (JSON, keys sorted)`);
        if (file.jsonEqual) {
            file.diff = unifiedDiff(baseFile, newFile, name);
        }
    } else {
        file.diff = unifiedDiff(baseFile, newFile, name);
    }
    result.files.push(file);
    return result;
}

function loadTests(dir, scenario, suite) {
    return readJsonIfExists(path.join(dir, scenario, "tests", `${suite}.json`));
}

// Per-test pass/fail changes of one suite.
function compareTests(baseTests, newTests) {
    const result = { lost: [], recovered: [], absent: [], added: [], stillFailing: 0, retained: 0 };
    const base = baseTests?.tests ?? {};
    const next = newTests?.tests ?? {};
    for (const [name, status] of Object.entries(base)) {
        const now = next[name];
        if (now === undefined) {
            result.absent.push(`${name} (was ${status})`);
        } else if (status === "passed" && now !== "passed") {
            result.lost.push(`${name} (${now})`);
        } else if (status !== "passed" && now === "passed") {
            result.recovered.push(name);
        } else if (status === "passed") {
            result.retained++;
        } else {
            result.stillFailing++;
        }
    }
    for (const [name, status] of Object.entries(next)) {
        if (base[name] === undefined) {
            result.added.push(`${name} (${status})`);
        }
    }
    result.baseCounts = baseTests?.counts;
    result.newCounts = newTests?.counts;
    result.reportMissing = { base: baseTests === undefined || baseTests.missing === true, new: newTests === undefined || newTests.missing === true };
    result.baseFailedFiles = (baseTests?.failedFiles ?? []).map((file) => file.file);
    result.newFailedFiles = (newTests?.failedFiles ?? []).map((file) => file.file);
    result.differs = result.lost.length + result.recovered.length + result.absent.length + result.added.length > 0
        || result.reportMissing.base !== result.reportMissing.new
        || JSON.stringify(result.baseFailedFiles) !== JSON.stringify(result.newFailedFiles);
    return result;
}

function byName(list) {
    return new Map((list ?? []).map((item) => [item.name, item]));
}

function compareScenario(id, baseRecord, newRecord, baseOut, newOut, options) {
    const result = { id, letter: (baseRecord ?? newRecord).letter, differences: 0 };
    result.status = { base: baseRecord?.status ?? "absent", new: newRecord?.status ?? "absent" };
    result.ms = { base: baseRecord?.ms, new: newRecord?.ms };
    if (result.status.base !== result.status.new) {
        result.differences++;
    }
    if (baseRecord === undefined || newRecord === undefined) {
        return result;
    }
    const baseDir = path.join(baseOut, id);
    const newDir = path.join(newOut, id);

    const baseSteps = byName(baseRecord.steps);
    const newSteps = byName(newRecord.steps);
    result.steps = [];
    for (const name of new Set([...baseSteps.keys(), ...newSteps.keys()])) {
        const a = baseSteps.get(name);
        const b = newSteps.get(name);
        const describe = (step) => (step === undefined ? "absent" : `${step.ok ? "ok" : "FAILED"} exit=${step.exitCode ?? step.signal ?? "-"}${step.timedOut ? " timeout" : ""}`);
        const changed = a?.ok !== b?.ok || ((a?.exitCode ?? null) !== (b?.exitCode ?? null) && !(a?.service || b?.service));
        result.steps.push({ name, base: describe(a), new: describe(b), changed, baseMs: a?.ms, newMs: b?.ms });
        if (changed) {
            result.differences++;
        }
    }

    const baseChecks = byName(baseRecord.checks);
    const newChecks = byName(newRecord.checks);
    result.checks = [];
    for (const name of new Set([...baseChecks.keys(), ...newChecks.keys()])) {
        const a = baseChecks.get(name);
        const b = newChecks.get(name);
        if (a?.ok === b?.ok) {
            continue;
        }
        result.checks.push({ name, base: a === undefined ? "absent" : a.ok, new: b === undefined ? "absent" : b.ok, detail: b?.detail ?? a?.detail, severity: (b ?? a).severity });
        result.differences++;
    }

    result.tests = {};
    for (const suite of new Set([...Object.keys(baseRecord.tests ?? {}), ...Object.keys(newRecord.tests ?? {})])) {
        const tests = compareTests(loadTests(baseOut, id, suite), loadTests(newOut, id, suite));
        result.tests[suite] = tests;
        if (tests.differs) {
            result.differences++;
        }
    }

    result.artifacts = [];
    const names = new Set([...Object.keys(baseRecord.artifacts ?? {}), ...Object.keys(newRecord.artifacts ?? {})]);
    for (const name of [...names].sort()) {
        const artifact = compareArtifact(name, baseRecord.artifacts?.[name], newRecord.artifacts?.[name], baseDir, newDir, options);
        result.artifacts.push(artifact);
        if (artifact.differs && !artifact.informational) {
            result.differences++;
        }
    }

    const baseEvents = baseRecord.metrics?.watchEvents;
    const newEvents = newRecord.metrics?.watchEvents;
    if (baseEvents !== undefined || newEvents !== undefined) {
        result.watchEvents = { base: baseEvents, new: newEvents, differs: JSON.stringify(baseEvents) !== JSON.stringify(newEvents) };
        if (result.watchEvents.differs) {
            result.differences++;
        }
    }
    result.metrics = { base: baseRecord.metrics, new: newRecord.metrics };
    return result;
}

function list(items, max = 30) {
    const shown = items.slice(0, max).map((item) => `  - ${item}`);
    if (items.length > max) {
        shown.push(`  - ... ${items.length - max} more`);
    }
    return shown;
}

function markdown(report, options) {
    const lines = [
        "# NestJS harness compare",
        "",
        `- base: ${report.base.dir} (${report.base.package})`,
        `- new: ${report.new.dir} (${report.new.package})`,
        `- differences: ${report.differences}`,
        "",
        "| | scenario | base | new | base time | new time | differences |",
        "|---|---|---|---|---|---|---|",
    ];
    for (const scenario of report.scenarios) {
        lines.push(`| ${scenario.letter} | ${scenario.id} | ${scenario.status.base} | ${scenario.status.new} | ${formatMs(scenario.ms.base)} | ${formatMs(scenario.ms.new)} | ${scenario.differences} |`);
    }
    for (const scenario of report.scenarios) {
        lines.push("", `## ${scenario.letter} ${scenario.id}: ${scenario.status.base} -> ${scenario.status.new}`);
        if (scenario.steps === undefined) {
            continue;
        }
        for (const step of scenario.steps) {
            const timing = `${formatMs(step.baseMs)} -> ${formatMs(step.newMs)}`;
            lines.push(`- step ${step.name}: ${step.changed ? `${step.base} -> ${step.new}` : step.new} (${timing})`);
        }
        for (const check of scenario.checks) {
            lines.push(`- check "${check.name}" (${check.severity}): ${check.base} -> ${check.new}${check.detail ? ` [${String(check.detail).slice(0, 300)}]` : ""}`);
        }
        for (const [suite, tests] of Object.entries(scenario.tests)) {
            const counts = (c) => (c === undefined ? "no report" : `${c.passed}/${c.total} passed, ${c.failed} failed, ${c.skipped} skipped`);
            lines.push(`- tests ${suite}: ${counts(tests.baseCounts)} -> ${counts(tests.newCounts)}; retained ${tests.retained}, lost ${tests.lost.length}, recovered ${tests.recovered.length}, absent ${tests.absent.length}, added ${tests.added.length}, still failing ${tests.stillFailing}`);
            if (tests.lost.length > 0) {
                lines.push("  lost (passed in base):", ...list(tests.lost));
            }
            if (tests.recovered.length > 0) {
                lines.push("  recovered:", ...list(tests.recovered));
            }
            if (tests.absent.length > 0) {
                lines.push("  absent in new:", ...list(tests.absent));
            }
            if (tests.added.length > 0) {
                lines.push("  added in new:", ...list(tests.added));
            }
            if (JSON.stringify(tests.baseFailedFiles) !== JSON.stringify(tests.newFailedFiles)) {
                lines.push(`  files that failed to load: [${tests.baseFailedFiles.join(", ")}] -> [${tests.newFailedFiles.join(", ")}]`);
            }
        }
        if (scenario.watchEvents?.differs) {
            lines.push(`- watch events: ${JSON.stringify(scenario.watchEvents.base)} -> ${JSON.stringify(scenario.watchEvents.new)}`);
        }
        for (const artifact of scenario.artifacts) {
            if (artifact.presence !== undefined) {
                lines.push(`- ${artifact.name}: ${artifact.differs ? `present in base ${artifact.presence.base}, in new ${artifact.presence.new}` : "absent in both"}`);
                continue;
            }
            const infoFiles = (artifact.files ?? []).filter((file) => file.informational);
            const infoNote = infoFiles.length > 0 ? `; .tsbuildinfo differs (informational, not counted): ${infoFiles.map((file) => `${file.change} ${file.path}`).join(", ")}` : "";
            if (!artifact.differs) {
                lines.push(`- ${artifact.name}: identical${artifact.kind === "tree" ? ` (${artifact.total - infoFiles.length}${infoFiles.length > 0 ? ` of ${artifact.total}` : ""} files)` : ""}${infoNote}`);
                continue;
            }
            if (artifact.informational) {
                lines.push(`- ${artifact.name}: differs (informational, not counted)`);
                continue;
            }
            if (artifact.kind === "tree") {
                const counted = artifact.files.filter((file) => !file.informational);
                const count = (change) => counted.filter((file) => file.change === change).length;
                lines.push(`- ${artifact.name}: ${artifact.same}/${artifact.total} files identical, ${count("differs")} differ, ${count("added")} only in new, ${count("removed")} only in base${infoNote}`);
                for (const file of counted.filter((f) => f.change !== "differs")) {
                    lines.push(`  - ${file.change}: ${file.path}`);
                }
            } else {
                const json = artifact.files[0]?.jsonEqual === undefined ? "" : artifact.files[0].jsonEqual ? " (bytes differ, JSON equal)" : " (JSON differs)";
                lines.push(`- ${artifact.name}: differs${json}`);
            }
            const diffed = artifact.files.filter((f) => f.diff !== undefined && !f.informational);
            for (const file of diffed.slice(0, 15)) {
                lines.push("", "```diff", excerpt(file.diff, options.maxDiffLines), "```");
            }
            const more = diffed.length - 15;
            if (more > 0) {
                lines.push(`  ... ${more} more differing files`);
            }
        }
    }
    return `${lines.join("\n")}\n`;
}

function packageLabel(results) {
    const pkg = results.package;
    return pkg === undefined ? "no package" : `${pkg.name}@${pkg.version} tree ${pkg.treeHash.slice(0, 12)}`;
}

function writeFullDiffs(report, outDir) {
    for (const scenario of report.scenarios) {
        for (const artifact of scenario.artifacts ?? []) {
            for (const file of artifact.files) {
                if (file.diff === undefined) {
                    continue;
                }
                const target = path.join(outDir, "diffs", scenario.id, artifact.kind === "tree" ? `${artifact.name}/${file.path}.diff` : `${file.path}.diff`);
                fs.mkdirSync(path.dirname(target), { recursive: true });
                fs.writeFileSync(target, file.diff);
            }
        }
    }
}

function main() {
    const argv = process.argv.slice(2);
    if (argv[0] === "help" || argv.includes("--help") || argv.includes("-h")) {
        console.log(usage());
        return 0;
    }
    let options;
    try {
        options = parseArgs(argv);
    } catch (error) {
        console.error(`nest.sh compare: ${error.message}\n${usage()}`);
        return 2;
    }
    const [baseOut, newOut] = options.positional.map((dir) => path.resolve(dir));
    for (const dir of [baseOut, newOut]) {
        if (!fs.existsSync(path.join(dir, "results.json"))) {
            console.error(`nest.sh compare: ${dir} has no results.json`);
            return 2;
        }
    }
    const baseResults = readJson(path.join(baseOut, "results.json"));
    const newResults = readJson(path.join(newOut, "results.json"));
    const baseById = new Map(baseResults.scenarios.map((record) => [record.id, record]));
    const newById = new Map(newResults.scenarios.map((record) => [record.id, record]));
    const ids = [...new Set([...baseById.keys(), ...newById.keys()])];
    const scenarios = ids.map((id) => compareScenario(id, baseById.get(id), newById.get(id), baseOut, newOut, options));
    const report = {
        base: { dir: baseOut, package: packageLabel(baseResults) },
        new: { dir: newOut, package: packageLabel(newResults) },
        differences: scenarios.reduce((sum, scenario) => sum + scenario.differences, 0),
        scenarios,
    };
    const text = markdown(report, options);
    process.stdout.write(text);
    if (options.out !== undefined) {
        const outDir = path.resolve(options.out);
        fs.mkdirSync(outDir, { recursive: true });
        fs.writeFileSync(path.join(outDir, "compare.md"), markdown(report, { ...options, maxDiffLines: 400 }));
        writeFullDiffs(report, outDir);
        writeJson(path.join(outDir, "compare.json"), stripDiffs(report));
    }
    if (tempDir !== undefined) {
        fs.rmSync(tempDir, { recursive: true, force: true });
    }
    return report.differences === 0 ? 0 : 1;
}

// compare.json keeps which files differ; the diffs themselves are under diffs/.
function stripDiffs(report) {
    return JSON.parse(JSON.stringify(report, (key, value) => (key === "diff" ? undefined : value)));
}

process.exitCode = main();
