// nest.sh run <typescript-package-dir|tgz> <out-dir> [--scenario a,b,...] [--env NAME=VALUE]... [--force]
//
// Packs the package under test, installs it into each scenario's app copy the way users do
// (file:<tgz> plus an npm override) and records statuses, logs and outputs under <out-dir>.

import { execFileSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

import { ScenarioContext } from "./lib/context.mjs";
import { selectScenarios } from "./lib/scenarios.mjs";
import { pins } from "./lib/sources.mjs";
import { formatMs, hashTree, isoNow, manifestHash, readJson, removePath, sha256File, workDir, writeJson } from "./lib/util.mjs";

function usage() {
    return [
        "usage: nest.sh run <typescript-package-dir|tgz> <out-dir> [options]",
        "  --scenario a,b,...   letters or ids (default: all; g = g1,g2): a=nest-cli-unit b=swagger-sample",
        "                       c=cjs-paths d=cjs-paths-watch e=swagger-swc f=graphql-sample g1=cjs-paths-webpack",
        "                       g2=cjs-paths-webpack-swagger h1=nest-new-esm h2=nest-new-cjs i=ts-node",
        "  --env NAME=VALUE     extra environment for builds, tests and apps (not for npm); repeatable",
        "  --force              replace an existing <out-dir>",
    ].join("\n");
}

function parseArgs(argv) {
    const options = { positional: [], userEnv: {}, scenario: undefined, force: false };
    for (let i = 0; i < argv.length; i++) {
        const arg = argv[i];
        const value = () => {
            const next = arg.includes("=") && arg.startsWith("--scenario=") ? arg.slice(arg.indexOf("=") + 1) : argv[++i];
            if (next === undefined) {
                throw new Error(`${arg} needs a value`);
            }
            return next;
        };
        if (arg === "--scenario" || arg.startsWith("--scenario=")) {
            options.scenario = value();
        } else if (arg === "--env") {
            const pair = argv[++i] ?? "";
            const eq = pair.indexOf("=");
            if (eq <= 0) {
                throw new Error(`--env needs NAME=VALUE, got "${pair}"`);
            }
            options.userEnv[pair.slice(0, eq)] = pair.slice(eq + 1);
        } else if (arg === "--force") {
            options.force = true;
        } else if (arg.startsWith("--")) {
            throw new Error(`unknown option ${arg}`);
        } else {
            options.positional.push(arg);
        }
    }
    if (options.positional.length !== 2) {
        throw new Error("expected <typescript-package-dir|tgz> and <out-dir>");
    }
    return options;
}

// One run at a time: the app copies under target/typescript-api/consumers/nest are shared.
function takeLock() {
    fs.mkdirSync(workDir, { recursive: true });
    const lockFile = path.join(workDir, ".lock");
    for (let attempt = 0; attempt < 2; attempt++) {
        try {
            fs.writeFileSync(lockFile, `${process.pid}\n`, { flag: "wx" });
            return () => fs.rmSync(lockFile, { force: true });
        } catch (error) {
            if (error.code !== "EEXIST") {
                throw error;
            }
            const holder = Number(fs.readFileSync(lockFile, "utf8"));
            if (processAlive(holder)) {
                throw new Error(`another nest harness run (pid ${holder}) holds ${lockFile}`);
            }
            fs.rmSync(lockFile, { force: true });
        }
    }
    throw new Error(`could not take ${lockFile}`);
}

function processAlive(pid) {
    if (!Number.isInteger(pid) || pid <= 0) {
        return false;
    }
    try {
        process.kill(pid, 0);
        return true;
    } catch (error) {
        return error.code === "EPERM";
    }
}

function gitInfo(dir) {
    try {
        const head = execFileSync("git", ["-C", dir, "rev-parse", "HEAD"], { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] }).trim();
        const status = execFileSync("git", ["-C", dir, "status", "--porcelain", "--", "."], { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] });
        return { head, dirtyFiles: status.split("\n").filter(Boolean).length };
    } catch {
        return undefined;
    }
}

// npm pack of a package directory (or a copy of a ready .tgz) into <out>/package, unpacked next
// to it so that every installed copy can be checked against the same file manifest.
function packLibrary(input, outDir) {
    const packDir = path.join(outDir, "package");
    fs.mkdirSync(packDir, { recursive: true });
    const abs = path.resolve(input);
    let tgz;
    let source;
    if (fs.statSync(abs).isFile()) {
        tgz = path.join(packDir, path.basename(abs));
        fs.copyFileSync(abs, tgz);
        source = { kind: "tgz", path: abs };
    } else {
        if (!fs.existsSync(path.join(abs, "package.json"))) {
            throw new Error(`${abs} has no package.json`);
        }
        const json = execFileSync("npm", ["pack", abs, "--ignore-scripts", "--pack-destination", packDir, "--json"], {
            encoding: "utf8",
            stdio: ["ignore", "pipe", "pipe"],
            maxBuffer: 64 * 1024 * 1024,
        });
        const [packed] = JSON.parse(json.slice(json.indexOf("[")));
        tgz = path.join(packDir, packed.filename);
        source = { kind: "dir", path: abs, git: gitInfo(abs) };
    }
    const contents = path.join(packDir, "contents");
    removePath(contents);
    fs.mkdirSync(contents);
    execFileSync("tar", ["-xzf", tgz, "-C", contents]);
    const root = path.join(contents, "package");
    const manifest = hashTree(root);
    writeJson(path.join(packDir, "manifest.json"), manifest);
    const pkg = readJson(path.join(root, "package.json"));
    return {
        source,
        tgz,
        tgzSha256: sha256File(tgz),
        name: pkg.name,
        version: pkg.version,
        files: Object.keys(manifest).length,
        treeHash: manifestHash(manifest),
        hasNative: Object.keys(manifest).some((file) => file.startsWith("native/")),
        hasBinTsc: manifest["bin/tsc"] !== undefined,
    };
}

function toolVersion(cmd, args) {
    try {
        return execFileSync(cmd, args, { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] }).trim();
    } catch {
        return undefined;
    }
}

function tsrsEnv() {
    return Object.fromEntries(Object.entries(process.env).filter(([name]) => name.startsWith("TSRS_")));
}

function summaryMarkdown(results) {
    const lines = [
        `# NestJS harness run`,
        "",
        `- package: ${results.package.name}@${results.package.version} (${results.package.source.kind} ${results.package.source.path}), tree ${results.package.treeHash.slice(0, 12)}, ${results.package.files} files, native/ ${results.package.hasNative ? "yes" : "no"}, bin/tsc ${results.package.hasBinTsc ? "yes" : "no"}`,
        `- started ${results.startedAt}, total ${formatMs(results.ms)}, node ${results.host.node}, npm ${results.host.npm}`,
        `- env: ${JSON.stringify({ ...results.tsrsEnv, ...results.userEnv })}`,
        "",
        "| | scenario | status | time | tests | failed steps / checks |",
        "|---|---|---|---|---|---|",
    ];
    for (const scenario of results.scenarios) {
        const tests = Object.entries(scenario.tests ?? {})
            .map(([suite, counts]) => `${suite} ${counts.passed}/${counts.total}${counts.failed ? ` (${counts.failed} failed)` : ""}${counts.failedFiles ? ` +${counts.failedFiles} files failed` : ""}`)
            .join(", ");
        const failed = [...(scenario.failedSteps ?? []), ...(scenario.failedChecks ?? [])].join("; ");
        lines.push(`| ${scenario.letter} | ${scenario.id} | ${scenario.status} | ${formatMs(scenario.ms)} | ${tests || "-"} | ${failed || "-"} |`);
    }
    lines.push("");
    for (const scenario of results.scenarios) {
        const steps = (scenario.steps ?? []).map((step) => `${step.name} ${step.ok ? "ok" : `FAILED (exit ${step.exitCode ?? step.signal ?? "-"}${step.timedOut ? ", timeout" : ""}${step.error ? `, ${step.error}` : ""})`} ${formatMs(step.ms)}`);
        lines.push(`## ${scenario.letter} ${scenario.id}: ${scenario.status}`, "", scenario.title, "", ...steps.map((step) => `- ${step}`));
        if (Object.keys(scenario.metrics ?? {}).length > 0) {
            lines.push(`- metrics: ${JSON.stringify(scenario.metrics)}`);
        }
        for (const note of scenario.notes ?? []) {
            lines.push(`- note: ${note.split("\n")[0]}`);
        }
        lines.push("");
    }
    return `${lines.join("\n")}\n`;
}

async function main() {
    const argv = process.argv.slice(2);
    if (argv[0] === "help" || argv.includes("--help") || argv.includes("-h")) {
        console.log(usage());
        return 0;
    }
    let options;
    let selected;
    try {
        options = parseArgs(argv);
        selected = selectScenarios(options.scenario);
    } catch (error) {
        console.error(`nest.sh run: ${error.message}\n${usage()}`);
        return 2;
    }
    const [input, outArg] = options.positional;
    if (!fs.existsSync(input)) {
        console.error(`nest.sh run: ${input} does not exist`);
        return 2;
    }
    const outDir = path.resolve(outArg);
    if (fs.existsSync(outDir) && fs.readdirSync(outDir).length > 0) {
        if (!options.force) {
            console.error(`nest.sh run: ${outDir} exists and is not empty (use --force to replace it)`);
            return 2;
        }
        removePath(outDir);
    }
    fs.mkdirSync(outDir, { recursive: true });
    const releaseLock = takeLock();
    const started = Date.now();
    const results = {
        harness: "nest",
        startedAt: isoNow(),
        outDir,
        host: { node: process.version, npm: toolVersion("npm", ["--version"]), platform: `${os.platform()} ${os.release()}`, cpus: os.cpus().length },
        pins,
        userEnv: options.userEnv,
        tsrsEnv: tsrsEnv(),
        scenarios: [],
    };
    try {
        try {
            results.package = packLibrary(input, outDir);
        } catch (error) {
            console.error(`nest.sh run: packing ${input} failed: ${error.stderr ?? error.message}`);
            return 2;
        }
        console.log(`[nest] package ${results.package.name}@${results.package.version} tree ${results.package.treeHash.slice(0, 12)} -> ${path.relative(process.cwd(), results.package.tgz)}`);
        for (const scenario of selected) {
            const ctx = new ScenarioContext({
                scenario,
                outDir: path.join(outDir, scenario.id),
                appDir: path.join(workDir, scenario.id),
                pack: results.package,
                baseEnv: process.env,
                userEnv: options.userEnv,
            });
            console.log(`[nest] ${scenario.letter} ${scenario.id}: running`);
            let error;
            try {
                await scenario.run(ctx);
            } catch (caught) {
                error = caught;
            }
            const record = ctx.finish(error);
            results.scenarios.push(record);
            console.log(`[nest] ${scenario.letter} ${scenario.id}: ${record.status} (${formatMs(record.ms)})${record.failedSteps.length + record.failedChecks.length > 0 ? ` failed: ${[...record.failedSteps, ...record.failedChecks].join("; ")}` : ""}`);
            if (error !== undefined) {
                console.log(`[nest]   harness error: ${error.message}`);
            }
            writeResults(outDir, results, started);
        }
    } finally {
        writeResults(outDir, results, started);
        releaseLock();
    }
    console.log(`[nest] results: ${path.join(outDir, "summary.md")}`);
    return results.scenarios.every((scenario) => scenario.status === "pass") ? 0 : 1;
}

function writeResults(outDir, results, started) {
    results.finishedAt = isoNow();
    results.ms = Date.now() - started;
    writeJson(path.join(outDir, "results.json"), results);
    if (results.package !== undefined) {
        fs.writeFileSync(path.join(outDir, "summary.md"), summaryMarkdown(results));
    }
}

process.exitCode = await main();
