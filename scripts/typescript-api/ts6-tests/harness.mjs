// Runs TypeScript 6.0.3's own tests, unmodified, against a `typescript` library directory.
// Entry point: run.sh (it adds the systemd memory cap). `node harness.mjs help` prints the usage.
import { spawn, spawnSync } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { commandTriage, TRIAGE_USAGE } from "./triage.mjs";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const REPO = path.resolve(HERE, "../../..");
const CACHE = path.resolve(process.env.TS6_TESTS_CACHE || path.join(REPO, "target/typescript-api/ts60"));
const STATE = path.join(CACHE, ".ts6-tests");
const TS60_GIT = process.env.TS6_TESTS_GIT || "https://github.com/microsoft/TypeScript";
const TS60_TAG = "v6.0.3";
const TS60_COMMIT = "050880ce59e30b356b686bd3144efe24f875ebc8";
const SWAPPED_BUNDLE = "run-swapped.js";
const SUITES = JSON.parse(fs.readFileSync(path.join(HERE, "suites.json"), "utf8"));
const SUITE_ORDER = ["apisamples", "tier1", "compiler"];
const MAX_JOBS = 4;
const MOCHA_TIMEOUT_MS = 40000;
// Mocha's per-test timeout cannot stop a blocked synchronous call (a hung native request).
const PROCESS_TIMEOUT_S = Number(process.env.TS6_TESTS_PROCESS_TIMEOUT || 3600);
const HARNESS_OUTPUTS = ["ws", "raw", "apisamples", "results.json", "summary.md", "meta.json", "harness.log"];

const USAGE = `usage: run.sh <command> ...

  help
      Print this text.
  setup
      Clone microsoft/TypeScript ${TS60_TAG} into ${path.relative(REPO, CACHE)}, npm ci, build its tests
      (hereby tests, dts, generate-types-map) and the swapped bundle. Built once; run does it when needed.
  run <library-dir> <out-dir> [--suite tier1|compiler|apisamples|all] [--jobs N] [--force] [--detach]
      Run a suite (default all) against <library-dir>/lib/typescript.js and write results.json,
      summary.md, meta.json, harness.log and raw logs to <out-dir>. --jobs shards the compiler suite
      over N mocha processes (default ${MAX_JOBS}, at most ${MAX_JOBS}). <out-dir> must be empty unless --force, which
      replaces the harness outputs in it. --detach starts the run in its own systemd unit and returns;
      <out-dir>/harness.log ends with DONE or FAIL rc=N. Exit status 1 when a mocha process crashed;
      test failures alone exit 0.
  compare <base-out> <new-out> [--limit N] [--json FILE]
      Per suite: retained, lost, gained, absent, still-failing and added tests. Lists up to N names per
      category (default 30, 0 = all); --json writes every name. Exits 1 when a base pass is lost or absent.
      Tests that pass settings TS 7 removed (ts7-removed.json, tier1 only) are marked [TS7-removed: ...].
      Only suites present in both runs are compared; the others are listed as unrun.
${TRIAGE_USAGE}

Environment: TS6_TESTS_CACHE (cache checkout), TS6_TESTS_GIT (clone source),
TS6_TESTS_MEMORY_MAX (systemd cap, default 12G), TS6_TESTS_SYSTEMD=0 (no systemd-run wrapper),
TS6_TESTS_PROCESS_TIMEOUT (seconds per mocha process, default 3600; a killed process counts as crashed),
TS6_TESTS_TS7 (triage: the TS 7 tsc/ directory).
The systemd unit gets PATH, HOME and every TS6_TESTS_* and TSRS_* variable, nothing else.`;

let harnessLog;

function fail(message) {
    console.error(`ts6-tests: ${message}`);
    if (harnessLog !== undefined) {
        fs.appendFileSync(harnessLog, `${message}\nFAIL rc=2\n`);
    }
    process.exit(2);
}

function log(message) {
    const line = `[ts6-tests] ${message}`;
    console.log(line);
    if (harnessLog !== undefined) {
        fs.appendFileSync(harnessLog, line + "\n");
    }
}

function run(command, args, options = {}) {
    log(`$ ${command} ${args.join(" ")}${options.cwd ? `   (in ${options.cwd})` : ""}`);
    const result = spawnSync(command, args, { stdio: "inherit", ...options });
    if (result.status !== 0) {
        fail(`${command} ${args.join(" ")} failed with ${result.status ?? result.signal}`);
    }
}

function sha256(...parts) {
    const hash = crypto.createHash("sha256");
    for (const part of parts) {
        hash.update(part);
        hash.update("\0");
    }
    return hash.digest("hex");
}

function readText(file) {
    return fs.existsSync(file) ? fs.readFileSync(file, "utf8") : undefined;
}

function writeJson(file, value) {
    fs.writeFileSync(file, JSON.stringify(value, null, 1) + "\n");
}

// ---- cache: the TS 6.0.3 checkout and its test build ----

function withCacheLock(action) {
    const lock = `${CACHE}.lock`;
    fs.mkdirSync(path.dirname(CACHE), { recursive: true });
    for (;;) {
        try {
            fs.mkdirSync(lock);
            fs.writeFileSync(path.join(lock, "pid"), String(process.pid));
            break;
        }
        catch (error) {
            if (error.code !== "EEXIST") {
                throw error;
            }
            const owner = Number(readText(path.join(lock, "pid")));
            if (owner && !isAlive(owner)) {
                fs.rmSync(lock, { recursive: true, force: true });
                continue;
            }
            log(`waiting for the cache lock ${lock} (pid ${owner || "?"})`);
            Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 5000);
        }
    }
    try {
        return action();
    }
    finally {
        fs.rmSync(lock, { recursive: true, force: true });
    }
}

function isAlive(pid) {
    try {
        process.kill(pid, 0);
        return true;
    }
    catch (error) {
        return error.code === "EPERM";
    }
}

function stepDone(name, key) {
    return readText(path.join(STATE, name)) === key;
}

function markStep(name, key) {
    fs.mkdirSync(STATE, { recursive: true });
    fs.writeFileSync(path.join(STATE, name), key);
}

function ensureCache() {
    withCacheLock(() => {
        if (!fs.existsSync(path.join(CACHE, ".git"))) {
            const partial = `${CACHE}.partial`;
            fs.rmSync(partial, { recursive: true, force: true });
            run("git", ["clone", "--quiet", "--branch", TS60_TAG, "--depth", "1", TS60_GIT, partial]);
            fs.renameSync(partial, CACHE);
        }
        const head = spawnSync("git", ["rev-parse", "HEAD"], { cwd: CACHE, encoding: "utf8" }).stdout.trim();
        if (head !== TS60_COMMIT) {
            fail(`${CACHE} is at ${head}, expected ${TS60_TAG} ${TS60_COMMIT}`);
        }
        const lockHash = sha256(fs.readFileSync(path.join(CACHE, "package-lock.json")));
        if (!stepDone("npm-ci", lockHash)) {
            run("npm", ["ci", "--ignore-scripts", "--no-audit", "--no-fund"], { cwd: CACHE });
            markStep("npm-ci", lockHash);
        }
        const hereby = path.join(CACHE, "node_modules/hereby/bin/hereby.js");
        // TS 6.0: Herebyfile.mjs:687 runtests depends on tests, lib, generate-types-map and dts.
        if (!stepDone("build", TS60_COMMIT)) {
            run(process.execPath, [hereby, "tests", "--no-typecheck"], { cwd: CACHE });
            run(process.execPath, [hereby, "dts"], { cwd: CACHE });
            run(process.execPath, [hereby, "generate-types-map"], { cwd: CACHE });
            markStep("build", TS60_COMMIT);
        }
        const shim = path.join(HERE, "shim.cjs");
        const builder = path.join(HERE, "build-swapped.mjs");
        const swapKey = sha256(fs.readFileSync(builder), shim);
        if (!stepDone("swapped", swapKey) || !fs.existsSync(path.join(CACHE, "built/local", SWAPPED_BUNDLE))) {
            run(process.execPath, [builder, CACHE, path.join(CACHE, "built/local", SWAPPED_BUNDLE), shim]);
            markStep("swapped", swapKey);
        }
    });
}

// ---- workspaces: one overlay of the cache per mocha process ----

// The harness resolves tests/, built/local/ and test.config against the working directory and the
// directory of the bundle (findUpRoot), so each process gets its own root of symlinks into the cache
// with its own test.config, tests/baselines/local and built/local/typescript.d.ts.
function makeWorkspace(ws, libraryDir, testConfig) {
    fs.mkdirSync(ws, { recursive: true });
    linkEntries(CACHE, ws, new Set([".git", ".ts6-tests", "built", "tests", "test.config", "mytest.config"]));
    fs.mkdirSync(path.join(ws, "tests/baselines/local"), { recursive: true });
    linkEntries(path.join(CACHE, "tests"), path.join(ws, "tests"), new Set(["baselines"]));
    linkEntries(path.join(CACHE, "tests/baselines"), path.join(ws, "tests/baselines"), new Set(["local"]));
    const builtLocal = path.join(ws, "built/local");
    fs.mkdirSync(builtLocal, { recursive: true });
    const own = new Set(["run.js", "run.js.map", SWAPPED_BUNDLE, `${SWAPPED_BUNDLE}.map`, "typescript.d.ts"]);
    linkEntries(path.join(CACHE, "built/local"), builtLocal, own);
    // A real file (not a symlink), so that the bundle's __dirname is the workspace.
    for (const file of [SWAPPED_BUNDLE, `${SWAPPED_BUNDLE}.map`]) {
        linkOrCopy(path.join(CACHE, "built/local", file), path.join(builtLocal, file));
    }
    // TS 6.0's Public APIs test and the APISample cases read the library's own typescript.d.ts.
    const libraryDts = path.join(libraryDir, "lib/typescript.d.ts");
    const dts = fs.existsSync(libraryDts) ? libraryDts : path.join(CACHE, "built/local/typescript.d.ts");
    fs.copyFileSync(dts, path.join(builtLocal, "typescript.d.ts"));
    writeJson(path.join(ws, "test.config"), testConfig);
}

function linkOrCopy(from, to) {
    try {
        fs.linkSync(from, to);
    }
    catch {
        fs.copyFileSync(from, to);
    }
}

function linkEntries(fromDir, toDir, skip) {
    for (const name of fs.readdirSync(fromDir)) {
        if (!skip.has(name)) {
            fs.symlinkSync(path.join(fromDir, name), path.join(toDir, name));
        }
    }
}

// ---- mocha suites ----

function mochaJob({ name, suite, ws, rawDir, libraryDir }) {
    const spec = SUITES[suite];
    const jsonl = path.join(rawDir, `${name}.jsonl`);
    const logFile = path.join(rawDir, `${name}.log`);
    const args = [
        path.join(ws, "node_modules/mocha/bin/_mocha"),
        "-R", path.join(HERE, "reporter.cjs"),
        "-O", `output=${jsonl}`,
        "-t", String(MOCHA_TIMEOUT_MS),
        "--no-colors",
        "-g", spec.grep,
        `built/local/${SWAPPED_BUNDLE}`,
    ];
    const env = { ...process.env, NODE_ENV: "development", TS6_TESTS_LIBRARY: libraryDir };
    return new Promise(resolve => {
        const out = fs.openSync(logFile, "w");
        const started = Date.now();
        const child = spawn(process.execPath, args, { cwd: ws, env, stdio: ["ignore", out, out] });
        const timer = setTimeout(() => {
            fs.writeSync(out, `\n[ts6-tests] killed after ${PROCESS_TIMEOUT_S} s (TS6_TESTS_PROCESS_TIMEOUT)\n`);
            child.kill("SIGKILL");
        }, PROCESS_TIMEOUT_S * 1000);
        child.on("close", (code, signal) => {
            clearTimeout(timer);
            fs.closeSync(out);
            resolve({ name, suite, jsonl, log: logFile, exitCode: code, signal, seconds: (Date.now() - started) / 1000 });
        });
    });
}

async function runMochaSuite(suite, context) {
    const spec = SUITES[suite];
    const shards = spec.shardable ? context.jobs : 1;
    const jobs = [];
    for (let shardId = 1; shardId <= shards; shardId++) {
        const name = shards === 1 ? suite : `${suite}-shard${shardId}`;
        const ws = path.join(context.outDir, "ws", name);
        const testConfig = { runners: spec.runners, light: true };
        if (shards > 1) {
            Object.assign(testConfig, { shards, shardId });
        }
        makeWorkspace(ws, context.libraryDir, testConfig);
        jobs.push({ name, suite, ws, rawDir: context.rawDir, libraryDir: context.libraryDir });
    }
    log(`${suite}: ${jobs.length} mocha process(es)`);
    const started = Date.now();
    const finished = await Promise.all(jobs.map(mochaJob));
    const seconds = (Date.now() - started) / 1000;
    const tests = [];
    const runs = [];
    for (const job of finished) {
        const { records, ended } = readJsonl(job.jsonl);
        for (const record of records) {
            tests.push({ suite, title: record.title, top: record.top, state: record.state, error: record.error, hook: record.hook });
        }
        const localBaselines = countFiles(path.join(context.outDir, "ws", job.name, "tests/baselines/local"));
        runs.push({ name: job.name, exitCode: job.exitCode, signal: job.signal, seconds: job.seconds, crashed: !ended, records: records.length, localBaselines });
        log(`${job.name}: exit ${job.exitCode ?? job.signal}, ${records.length} results, ${job.seconds.toFixed(1)} s${ended ? "" : ", CRASHED (no end record)"}`);
    }
    return { suite, seconds, runs, tests };
}

function readJsonl(file) {
    const records = [];
    let ended = false;
    for (const line of (readText(file) ?? "").split("\n")) {
        if (line === "") {
            continue;
        }
        let record;
        try {
            record = JSON.parse(line);
        }
        catch {
            continue; // the half-written last line of a killed process
        }
        if (record.event === "end") {
            ended = true;
            continue;
        }
        records.push(record);
    }
    return { records, ended };
}

function countFiles(dir) {
    if (!fs.existsSync(dir)) {
        return 0;
    }
    let count = 0;
    for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
        count += entry.isDirectory() ? countFiles(path.join(dir, entry.name)) : 1;
    }
    return count;
}

// ---- APISample programs ----

const SAMPLE_NAMES = ["compile", "linter", "transform", "parseConfig", "jsdoc", "Watch", "WatchWithDefaults", "WatchWithOwnWatchHost", "watcher"];
const WATCH_SAMPLES = new Set(["Watch", "WatchWithDefaults", "WatchWithOwnWatchHost", "watcher"]);

// The samples are the emitted JS that TS 6.0 records in tests/baselines/reference/APISample_*.js.
function extractSamples(samplesDir) {
    fs.mkdirSync(samplesDir, { recursive: true });
    const reference = path.join(CACHE, "tests/baselines/reference");
    for (const name of SAMPLE_NAMES) {
        const file = `APISample_${name}.js`;
        const text = fs.readFileSync(path.join(reference, file), "utf8");
        const marker = `//// [${file}]`;
        const start = text.lastIndexOf(marker);
        if (start < 0) {
            fail(`${file} has no ${marker} section`);
        }
        fs.writeFileSync(path.join(samplesDir, file), text.slice(start + marker.length).replace(/^\r?\n/, ""));
    }
}

async function runApiSamples(context) {
    const dir = path.join(context.outDir, "apisamples");
    const samplesDir = path.join(dir, "samples");
    const outputDir = path.join(dir, "out");
    extractSamples(samplesDir);
    const logFile = path.join(context.rawDir, "apisamples.log");
    const started = Date.now();
    const exitCode = await new Promise(resolve => {
        const out = fs.openSync(logFile, "w");
        const child = spawn("bash", [path.join(HERE, "apisamples/run-samples.sh"), context.libraryDir, outputDir, samplesDir], { stdio: ["ignore", out, out] });
        child.on("close", code => {
            fs.closeSync(out);
            resolve(code);
        });
    });
    const seconds = (Date.now() - started) / 1000;
    const expectedDir = path.join(HERE, "apisamples/expected");
    const tests = SAMPLE_NAMES.map(name => {
        const files = WATCH_SAMPLES.has(name) ? [`${name}.out`, `${name}.files`] : [`${name}.out`];
        const error = files.map(file => diffFirstLine(path.join(expectedDir, file), path.join(outputDir, file), file)).find(Boolean);
        return { suite: "apisamples", title: `APISample_${name}`, top: "APISample programs", state: error ? "fail" : "pass", error };
    });
    log(`apisamples: exit ${exitCode}, ${tests.filter(test => test.state === "pass").length}/${tests.length} match typescript@6.0.3, ${seconds.toFixed(1)} s`);
    return { suite: "apisamples", seconds, runs: [{ name: "apisamples", exitCode, seconds, crashed: false, records: tests.length }], tests };
}

function diffFirstLine(expectedFile, actualFile, label) {
    const expected = readText(expectedFile);
    const actual = readText(actualFile);
    if (expected === undefined) {
        return `${label}: no expected file`;
    }
    if (actual === undefined) {
        return `${label}: no output`;
    }
    if (expected === actual) {
        return undefined;
    }
    const want = expected.split("\n");
    const got = actual.split("\n");
    const line = want.findIndex((text, index) => text !== got[index]);
    const at = line < 0 ? want.length : line;
    return `${label}:${at + 1}: expected ${JSON.stringify(want[at] ?? "<end>")} got ${JSON.stringify(got[at] ?? "<end>")}`;
}

// ---- run ----

function parseRunArgs(args) {
    const positional = [];
    const options = { suite: "all", jobs: MAX_JOBS, force: false };
    for (let index = 0; index < args.length; index++) {
        const arg = args[index];
        if (arg === "--suite") {
            options.suite = args[++index];
        }
        else if (arg === "--jobs") {
            options.jobs = Number(args[++index]);
        }
        else if (arg === "--force") {
            options.force = true;
        }
        else if (arg === "--detach") {
            fail("--detach needs run.sh with systemd-run (TS6_TESTS_SYSTEMD unset)");
        }
        else if (arg.startsWith("--")) {
            fail(`unknown option ${arg}`);
        }
        else {
            positional.push(arg);
        }
    }
    if (positional.length !== 2) {
        fail("run needs <library-dir> <out-dir>");
    }
    if (!["all", ...SUITE_ORDER].includes(options.suite)) {
        fail(`unknown suite ${options.suite}`);
    }
    if (!Number.isInteger(options.jobs) || options.jobs < 1 || options.jobs > MAX_JOBS) {
        fail(`--jobs must be 1..${MAX_JOBS}`);
    }
    return { libraryDir: path.resolve(positional[0]), outDir: path.resolve(positional[1]), ...options };
}

function prepareOutDir(outDir, force) {
    fs.mkdirSync(outDir, { recursive: true });
    const existing = fs.readdirSync(outDir);
    if (existing.length === 0) {
        return;
    }
    if (!force) {
        fail(`${outDir} is not empty (use --force to replace the harness outputs in it)`);
    }
    for (const name of HARNESS_OUTPUTS) {
        fs.rmSync(path.join(outDir, name), { recursive: true, force: true });
    }
}

function fingerprint(libraryDir) {
    const files = [];
    for (const rel of ["package.json", "lib/typescript.js", "lib/typescript.d.ts"]) {
        if (fs.existsSync(path.join(libraryDir, rel))) {
            files.push(rel);
        }
    }
    for (const dir of ["src", "native"]) {
        listFiles(libraryDir, dir, files);
    }
    const hashes = Object.fromEntries(files.map(rel => [rel, sha256(fs.readFileSync(path.join(libraryDir, rel))).slice(0, 16)]));
    const pkg = JSON.parse(readText(path.join(libraryDir, "package.json")) ?? "{}");
    return { dir: libraryDir, realpath: fs.realpathSync(libraryDir), name: pkg.name, version: pkg.version, combined: sha256(JSON.stringify(hashes)).slice(0, 16), files: hashes };
}

function listFiles(root, rel, out) {
    const dir = path.join(root, rel);
    if (!fs.existsSync(dir)) {
        return;
    }
    for (const entry of fs.readdirSync(dir, { withFileTypes: true }).sort((a, b) => a.name.localeCompare(b.name))) {
        const child = path.join(rel, entry.name);
        if (entry.isDirectory()) {
            listFiles(root, child, out);
        }
        else if (entry.isFile()) {
            out.push(child);
        }
    }
}

// Peak memory of this run's own systemd unit (run.sh starts one); undefined in a shared cgroup.
function unitMemoryPeak() {
    const cgroup = (readText("/proc/self/cgroup") ?? "").trim().split("\n").find(line => line.startsWith("0::"));
    if (!cgroup || !/\/(run|ts6-tests)-[^/]+\.service$/.test(cgroup)) {
        return undefined;
    }
    const peak = readText(path.join("/sys/fs/cgroup", cgroup.slice(3), "memory.peak"));
    return peak === undefined ? undefined : Number(peak);
}

function withKeys(tests) {
    const seen = new Map();
    return tests.map(test => {
        const base = `${test.suite}\u0000${test.title}`;
        const count = (seen.get(base) ?? 0) + 1;
        seen.set(base, count);
        return count === 1 ? test : { ...test, title: `${test.title} [duplicate ${count}]` };
    });
}

async function commandRun(args) {
    const options = parseRunArgs(args);
    if (!fs.existsSync(path.join(options.libraryDir, "lib/typescript.js"))) {
        fail(`${options.libraryDir}/lib/typescript.js does not exist`);
    }
    prepareOutDir(options.outDir, options.force);
    harnessLog = path.join(options.outDir, "harness.log");
    ensureCache();
    const rawDir = path.join(options.outDir, "raw");
    fs.mkdirSync(rawDir, { recursive: true });
    const context = { ...options, rawDir };
    const suites = options.suite === "all" ? SUITE_ORDER : [options.suite];
    const library = fingerprint(options.libraryDir);
    const startedAt = new Date().toISOString();
    log(`library ${library.dir} (${library.name}@${library.version}, fingerprint ${library.combined})`);
    const results = [];
    for (const suite of suites) {
        results.push(suite === "apisamples" ? await runApiSamples(context) : await runMochaSuite(suite, context));
    }
    const tests = withKeys(results.flatMap(result => result.tests));
    fs.writeFileSync(path.join(options.outDir, "results.json"), "[\n" + tests.map(test => JSON.stringify(test)).join(",\n") + "\n]\n");
    const after = fingerprint(options.libraryDir);
    const changedFiles = [...new Set([...Object.keys(library.files), ...Object.keys(after.files)])].filter(file => library.files[file] !== after.files[file]);
    const meta = {
        library,
        libraryChangedDuringRun: changedFiles,
        ts60: { tag: TS60_TAG, commit: TS60_COMMIT, cache: CACHE },
        env: Object.fromEntries(Object.entries(process.env).filter(([name]) => /^(TSRS_|TS6_TESTS_)/.test(name))),
        node: process.version,
        host: os.hostname(),
        startedAt,
        finishedAt: new Date().toISOString(),
        jobs: options.jobs,
        unitMemoryPeakBytes: unitMemoryPeak(),
        suites: results.map(({ suite, seconds, runs }) => ({ suite, seconds, runs })),
    };
    writeJson(path.join(options.outDir, "meta.json"), meta);
    fs.writeFileSync(path.join(options.outDir, "summary.md"), renderSummary(meta, tests));
    log(`wrote ${path.join(options.outDir, "summary.md")}`);
    if (changedFiles.length > 0) {
        log(`WARNING: library files changed during the run, so the results may mix two sources: ${changedFiles.join(", ")}`);
    }
    const crashed = results.some(result => result.runs.some(entry => entry.crashed));
    fs.appendFileSync(harnessLog, crashed ? "FAIL rc=1 (a mocha process crashed)\n" : "DONE\n");
    return crashed ? 1 : 0;
}

// ---- summary ----

function countStates(tests) {
    const counts = { total: tests.length, pass: 0, fail: 0, pending: 0 };
    for (const test of tests) {
        counts[test.state]++;
    }
    return counts;
}

function groupBy(tests, keyOf) {
    const groups = new Map();
    for (const test of tests) {
        const key = keyOf(test);
        if (!groups.has(key)) {
            groups.set(key, []);
        }
        groups.get(key).push(test);
    }
    return [...groups.entries()].sort((a, b) => a[0].localeCompare(b[0]));
}

function countsTable(header, rows) {
    const lines = [`| ${header} | Total | Pass | Fail | Pending |`, "|---|---|---|---|---|"];
    for (const [name, tests] of rows) {
        const counts = countStates(tests);
        lines.push(`| ${name.replace(/\|/g, "\\|")} | ${counts.total} | ${counts.pass} | ${counts.fail} | ${counts.pending} |`);
    }
    return lines.join("\n");
}

function renderSummary(meta, tests) {
    const out = [];
    out.push("# TS 6.0.3 tests against a library", "");
    out.push(`- Library: \`${meta.library.dir}\` (${meta.library.name}@${meta.library.version}, fingerprint \`${meta.library.combined}\`)`);
    out.push(`- TS 6.0 tests: ${meta.ts60.tag} ${meta.ts60.commit.slice(0, 12)}; node ${meta.node}; ${meta.startedAt} to ${meta.finishedAt}`);
    const envText = Object.entries(meta.env).map(([name, value]) => `${name}=${value}`).join(" ") || "none";
    const peak = meta.unitMemoryPeakBytes === undefined ? "not measured" : `${(meta.unitMemoryPeakBytes / 2 ** 30).toFixed(2)} GiB`;
    out.push(`- Environment: ${envText}; jobs ${meta.jobs}; unit memory peak ${peak}`);
    if (meta.libraryChangedDuringRun.length > 0) {
        out.push(`- WARNING: library files changed during the run: ${meta.libraryChangedDuringRun.join(", ")}`);
    }
    out.push("");
    out.push("| Suite | Total | Pass | Fail | Pending | Wall (s) | Processes | Exit codes | Crashed |", "|---|---|---|---|---|---|---|---|---|");
    for (const entry of meta.suites) {
        const counts = countStates(tests.filter(test => test.suite === entry.suite));
        const exits = entry.runs.map(run => run.exitCode ?? run.signal).join(",");
        const crashed = entry.runs.filter(run => run.crashed).map(run => run.name).join(",") || "no";
        out.push(`| ${entry.suite} | ${counts.total} | ${counts.pass} | ${counts.fail} | ${counts.pending} | ${entry.seconds.toFixed(1)} | ${entry.runs.length} | ${exits} | ${crashed} |`);
    }
    for (const entry of meta.suites) {
        const suiteTests = tests.filter(test => test.suite === entry.suite);
        out.push("", `## ${entry.suite}`, "", countsTable("Top-level describe", groupBy(suiteTests, test => test.top)));
        if (entry.suite === "compiler") {
            out.push("", countsTable("Check", groupBy(suiteTests, test => test.title.match(/(Correct [^]*?) for /)?.[1] ?? "(other)")));
        }
        const failures = suiteTests.filter(test => test.state === "fail");
        if (failures.length > 0) {
            out.push("", `First failures (${Math.min(failures.length, 40)} of ${failures.length}):`, "");
            for (const test of failures.slice(0, 40)) {
                out.push(`- ${test.title}: ${test.error ?? ""}`);
            }
        }
    }
    return out.join("\n") + "\n";
}

// ---- compare ----

function loadResults(outDir) {
    const file = path.join(path.resolve(outDir), "results.json");
    if (!fs.existsSync(file)) {
        fail(`${file} does not exist`);
    }
    return JSON.parse(fs.readFileSync(file, "utf8"));
}

// Settings TS 7 removed that a test passes into the library (ts7-removed.json), so a loss under
// tsc-rs semantics can be told from a regression.
function loadRemovedSettings() {
    const file = path.join(HERE, "ts7-removed.json");
    const data = JSON.parse(readText(file) ?? "{}");
    return new Map(Object.entries(data).filter(([key]) => key !== "source").flatMap(([suite, titles]) => Object.entries(titles).map(([title, settings]) => [`${suite}\u0000${title}`, settings])));
}

function commandCompare(args) {
    const positional = [];
    let limit = 30;
    let jsonFile;
    for (let index = 0; index < args.length; index++) {
        if (args[index] === "--limit") {
            limit = Number(args[++index]);
        }
        else if (args[index] === "--json") {
            jsonFile = args[++index];
        }
        else {
            positional.push(args[index]);
        }
    }
    if (positional.length !== 2) {
        fail("compare needs <base-out> <new-out>");
    }
    const base = loadResults(positional[0]);
    const next = loadResults(positional[1]);
    const removed = loadRemovedSettings();
    const removedNote = (suite, title) => {
        const settings = removed.get(`${suite}\u0000${title}`);
        return settings ? ` [TS7-removed: ${settings.join(",")}]` : "";
    };
    const baseSuites = new Set(base.map(test => test.suite));
    const newSuites = new Set(next.map(test => test.suite));
    const bySuiteOrder = (a, b) => SUITE_ORDER.indexOf(a) - SUITE_ORDER.indexOf(b);
    const suites = [...baseSuites].filter(suite => newSuites.has(suite)).sort(bySuiteOrder);
    const unrun = [...baseSuites].filter(suite => !newSuites.has(suite)).sort(bySuiteOrder);
    const onlyNew = [...newSuites].filter(suite => !baseSuites.has(suite)).sort(bySuiteOrder);
    const report = {};
    let blocking = 0;
    const lines = [`base ${path.resolve(positional[0])}`, `new  ${path.resolve(positional[1])}`, ""];
    if (unrun.length > 0) {
        lines.push(`unrun suites (in base, not in new; not compared): ${unrun.map(suite => `${suite} (${base.filter(test => test.suite === suite).length} tests)`).join(", ")}`);
    }
    if (onlyNew.length > 0) {
        lines.push(`suites only in new (not compared): ${onlyNew.join(", ")}`);
    }
    if (unrun.length + onlyNew.length > 0) {
        lines.push("");
    }
    lines.push("| Suite | Base pass/total | New pass/total | Retained | Lost (TS7-removed) | Gained | Absent (base pass) | Still failing | Added |", "|---|---|---|---|---|---|---|---|---|");
    for (const suite of suites) {
        const baseTests = new Map(base.filter(test => test.suite === suite).map(test => [test.title, test]));
        const newTests = new Map(next.filter(test => test.suite === suite).map(test => [test.title, test]));
        const entry = { retained: [], lost: [], gained: [], absent: [], absentBasePass: [], stillFailing: [], added: [], otherChanges: [] };
        for (const [title, old] of baseTests) {
            const now = newTests.get(title);
            if (!now) {
                entry.absent.push(title);
                if (old.state === "pass") {
                    entry.absentBasePass.push(title);
                }
            }
            else if (old.state === "pass" && now.state === "pass") {
                entry.retained.push(title);
            }
            else if (old.state === "pass") {
                entry.lost.push({ title, state: now.state, error: now.error });
            }
            else if (now.state === "pass") {
                entry.gained.push(title);
            }
            else if (old.state === "fail" && now.state === "fail") {
                entry.stillFailing.push({ title, error: now.error });
            }
            else {
                entry.otherChanges.push({ title, from: old.state, to: now.state });
            }
        }
        for (const [title, now] of newTests) {
            if (!baseTests.has(title)) {
                entry.added.push({ title, state: now.state });
            }
        }
        const basePass = [...baseTests.values()].filter(test => test.state === "pass").length;
        const newPass = [...newTests.values()].filter(test => test.state === "pass").length;
        const lostRemoved = entry.lost.filter(item => removed.has(`${suite}\u0000${item.title}`)).length;
        lines.push(`| ${suite} | ${basePass}/${baseTests.size} | ${newPass}/${newTests.size} | ${entry.retained.length} | ${entry.lost.length} (${lostRemoved}) | ${entry.gained.length} | ${entry.absent.length} (${entry.absentBasePass.length}) | ${entry.stillFailing.length} | ${entry.added.length} |`);
        blocking += entry.lost.length + entry.absentBasePass.length;
        report[suite] = entry;
    }
    for (const suite of suites) {
        const entry = report[suite];
        const sections = [
            ["lost", entry.lost.map(item => `${item.title} [${item.state}]${removedNote(suite, item.title)} ${item.error ?? ""}`)],
            ["absent", entry.absent],
            ["gained", entry.gained],
            ["still failing", entry.stillFailing.map(item => `${item.title}${removedNote(suite, item.title)}: ${item.error ?? ""}`)],
            ["added", entry.added.map(item => `${item.title} [${item.state}]`)],
            ["other changes", entry.otherChanges.map(item => `${item.title} [${item.from} -> ${item.to}]`)],
        ];
        for (const [label, items] of sections) {
            if (items.length === 0) {
                continue;
            }
            const shown = limit === 0 ? items : items.slice(0, limit);
            lines.push("", `${suite}: ${label} (${items.length}${shown.length < items.length ? `, first ${shown.length}` : ""})`);
            for (const item of shown) {
                lines.push(`  ${item}`);
            }
        }
    }
    console.log(lines.join("\n"));
    if (jsonFile) {
        writeJson(jsonFile, report);
    }
    return blocking > 0 ? 1 : 0;
}

// ---- main ----

const [command, ...rest] = process.argv.slice(2);
switch (command) {
    case undefined:
    case "help":
    case "--help":
    case "-h":
        console.log(USAGE);
        break;
    case "setup":
        ensureCache();
        log(`cache ready: ${CACHE}`);
        break;
    case "run":
        process.exitCode = await commandRun(rest);
        break;
    case "compare":
        process.exitCode = commandCompare(rest);
        break;
    case "triage":
        try {
            process.exitCode = commandTriage(rest);
        }
        catch (error) {
            fail(`triage: ${error.message}`);
        }
        break;
    default:
        fail(`unknown command ${command}\n\n${USAGE}`);
}
