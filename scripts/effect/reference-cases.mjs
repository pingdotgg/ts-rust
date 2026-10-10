#!/usr/bin/env node
// Differential check of the native Effect diagnostics against the reference
// Effect-patched tsc, on the Effect-TS/tsgo test cases (testdata/tests/effect-v3, effect-v4).
// Each case becomes a small project (its units under .src, the pinned Effect
// packages linked into node_modules). Both compilers check it with the same
// tsconfig and flags; their `--pretty false` outputs are compared line by line.
//
// usage: node scripts/effect/reference-cases.mjs --ours <tsc> --ref <tsc> [--effect-repo DIR]
//          [--versions effect-v4,effect-v3] [--filter SUBSTR] [--out DIR] [--jobs N]
// The reference repo needs `pnpm install --filter ./testdata/tests/effect-v4 --filter ./testdata/tests/effect-v3`.
// Writes <out>/summary.json and <out>/<version>/<case>.diff for each mismatch. Exit 1 on any mismatch.
import { execFile } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { parseArgs, promisify } from "node:util";

const run = promisify(execFile);
const { values: args } = parseArgs({
  options: {
    ours: { type: "string" },
    ref: { type: "string" },
    "effect-repo": { type: "string", default: path.join(os.homedir(), ".cache/repo-explorer/Effect-TS-tsgo") },
    versions: { type: "string", default: "effect-v4,effect-v3" },
    filter: { type: "string", default: "" },
    out: { type: "string", default: "/tmp/effect-cases" },
    jobs: { type: "string", default: String(Math.max(1, os.availableParallelism() - 2)) },
    "keep-ts": { type: "boolean", default: false },
  },
});
if (!args.ours || !args.ref) {
  console.error("--ours and --ref are required");
  process.exit(2);
}

fs.mkdirSync(args.out, { recursive: true });
args.out = fs.realpathSync(args.out);

// Go effecttest DefaultTsConfig (runner.go): the 3 stability rules are off
// unless a case's own tsconfig turns them on.
const DEFAULT_TSCONFIG = {
  compilerOptions: {
    skipLibCheck: true,
    plugins: [
      {
        name: "@effect/language-service",
        ignoreEffectErrorsInTscExitCode: true,
        skipDisabledOptimization: true,
        diagnosticSeverity: {
          experimentalApiUsage: "off",
          unstableApiUsage: "off",
          apiStabilityLeak: "off",
        },
      },
    ],
  },
};

// Go effecttest.parseTestUnits
function parseUnits(content, defaultName) {
  const units = [];
  let name = "";
  let lines = [];
  for (const line of content.split(/\r?\n/)) {
    const m = /^\/{2}\s*@(\w+)\s*:\s*([^\r\n]*)/.exec(line);
    if (m && m[1].toLowerCase() === "filename") {
      if (name && lines.length) units.push({ name, content: lines.join("\n") });
      name = m[2].trim();
      lines = [];
      continue;
    }
    lines.push(line);
  }
  if (name) units.push({ name, content: lines.join("\n") });
  else if (lines.length) units.push({ name: defaultName, content: lines.join("\n") });
  return units;
}

// Lenient JSONC: strips comments and trailing commas.
function parseJsonc(text) {
  let out = "";
  for (let i = 0; i < text.length; i++) {
    const c = text[i];
    if (c === '"') {
      let j = i + 1;
      while (j < text.length && text[j] !== '"') j += text[j] === "\\" ? 2 : 1;
      out += text.slice(i, j + 1);
      i = j;
    } else if (c === "/" && text[i + 1] === "/") {
      while (i < text.length && text[i] !== "\n") i++;
      out += "\n";
    } else if (c === "/" && text[i + 1] === "*") {
      i = text.indexOf("*/", i + 2) + 1;
    } else out += c;
  }
  return JSON.parse(out.replace(/,(\s*[}\]])/g, "$1"));
}

function linkPackages(srcModules, dstModules) {
  fs.mkdirSync(dstModules, { recursive: true });
  for (const entry of fs.readdirSync(srcModules)) {
    if (entry.startsWith(".")) continue;
    const src = path.join(srcModules, entry);
    if (entry.startsWith("@")) {
      for (const sub of fs.readdirSync(src)) {
        fs.mkdirSync(path.join(dstModules, entry), { recursive: true });
        const dst = path.join(dstModules, entry, sub);
        if (!fs.existsSync(dst)) fs.symlinkSync(fs.realpathSync(path.join(src, sub)), dst);
      }
    } else {
      const dst = path.join(dstModules, entry);
      if (!fs.existsSync(dst)) fs.symlinkSync(fs.realpathSync(src), dst);
    }
  }
}

function prepareCase(version, file) {
  const base = path.basename(file, ".ts");
  const dir = path.join(args.out, "projects", version, base);
  fs.rmSync(dir, { recursive: true, force: true });
  const srcDir = path.join(dir, ".src");
  fs.mkdirSync(srcDir, { recursive: true });
  const units = parseUnits(fs.readFileSync(file, "utf8"), path.basename(file));
  let tsconfig = null;
  let tsconfigName = "tsconfig.json";
  for (const unit of units) {
    const abs = unit.name.startsWith("/") ? unit.name : "/.src/" + unit.name;
    const target = path.join(dir, path.normalize(abs));
    fs.mkdirSync(path.dirname(target), { recursive: true });
    if (/(^|\/)tsconfig[^/]*\.json$/.test(unit.name) && !abs.startsWith("/node_modules/")) {
      tsconfig = parseJsonc(unit.content);
      tsconfigName = path.relative(srcDir, target);
      continue;
    }
    fs.writeFileSync(target, unit.content);
  }
  const content = fs.readFileSync(file, "utf8");
  const modules = path.join(args["effect-repo"], "testdata/tests", version, "node_modules");
  linkPackages(modules, path.join(dir, "node_modules"));
  if (!content.includes("vitest")) {
    for (const p of ["vitest", "@vitest", "@effect/vitest"]) fs.rmSync(path.join(dir, "node_modules", p), { recursive: true, force: true });
  }
  const injected = tsconfig === null;
  tsconfig ??= structuredClone(DEFAULT_TSCONFIG);
  tsconfig.compilerOptions ??= {};
  const co = tsconfig.compilerOptions;
  Object.assign(co, { newLine: "lf", skipDefaultLibCheck: true, skipLibCheck: true, noErrorTruncation: true, noEmit: true });
  co.target ??= "esnext";
  co.module ??= "nodenext";
  co.moduleResolution ??= "nodenext";
  if (injected) Object.assign(co, { esModuleInterop: true, allowSyntheticDefaultImports: true });
  // The harness passes only the input units as roots.
  delete tsconfig.include;
  tsconfig.files = units
    .map((u) => (u.name.startsWith("/") ? u.name : "/.src/" + u.name))
    .filter((n) => !n.startsWith("/node_modules/") && !/tsconfig[^/]*\.json$/.test(n) && /\.(m|c)?tsx?$/.test(n))
    .map((n) => path.relative(path.dirname(path.join(srcDir, tsconfigName)), path.join(dir, n)));
  delete tsconfig.extends;
  const configPath = path.join(srcDir, tsconfigName);
  fs.mkdirSync(path.dirname(configPath), { recursive: true });
  fs.writeFileSync(configPath, JSON.stringify(tsconfig, null, 2));
  return { base, dir, srcDir, configPath };
}

async function check(bin, c) {
  try {
    const { stdout } = await run(bin, ["-p", c.configPath, "--pretty", "false"], { cwd: c.srcDir, maxBuffer: 64 << 20 });
    return { code: 0, out: stdout };
  } catch (e) {
    if (typeof e.code !== "number") throw e;
    return { code: e.code, out: e.stdout ?? "" };
  }
}

// One diagnostic per entry: the first line and its indented continuation lines.
function diagnostics(out) {
  const result = [];
  for (const line of out.split("\n")) {
    if (!line.trim()) continue;
    if (/^\s/.test(line) && result.length) result[result.length - 1] += "\n" + line;
    else result.push(line);
  }
  return result;
}

const isEffect = (d) => / TS377\d\d\d: /.test(d);

const summary = { cases: 0, match: 0, effectMismatch: 0, tsOnlyMismatch: 0, crashes: 0, mismatches: [] };
const work = [];
for (const version of args.versions.split(",")) {
  const casesDir = path.join(args["effect-repo"], "testdata/tests", version);
  for (const f of fs.readdirSync(casesDir).sort()) {
    if (!f.endsWith(".ts") || !f.includes(args.filter)) continue;
    work.push({ version, file: path.join(casesDir, f) });
  }
}

async function worker() {
  while (work.length) {
    const { version, file } = work.shift();
    const c = prepareCase(version, file);
    const [ours, ref] = await Promise.all([check(args.ours, c), check(args.ref, c)]);
    summary.cases++;
    const crashed = ours.code > 2 || /^panic: |unported/m.test(ours.out);
    const od = diagnostics(ours.out);
    const rd = diagnostics(ref.out);
    const oe = od.filter(isEffect).join("\n");
    const re = rd.filter(isEffect).join("\n");
    const effectSame = oe === re;
    // tsc-rs follows its pinned TypeScript Go revision, where --noEmit with errors
    // exits 2 (outputs generated); TS 7.0.2 exits 1. Only failure vs success counts.
    const allSame = od.join("\n") === rd.join("\n") && (ours.code === 0) === (ref.code === 0);
    if (crashed) summary.crashes++;
    if (allSame && !crashed) {
      summary.match++;
      continue;
    }
    if (!effectSame || crashed) summary.effectMismatch++;
    else summary.tsOnlyMismatch++;
    const kind = crashed ? "crash" : !effectSame ? "effect" : ours.code !== ref.code ? "exit" : "ts-only";
    summary.mismatches.push({ version, case: c.base, kind, exit: [ours.code, ref.code] });
    const diffDir = path.join(args.out, version);
    fs.mkdirSync(diffDir, { recursive: true });
    fs.writeFileSync(
      path.join(diffDir, c.base + ".diff"),
      `exit ours=${ours.code} ref=${ref.code}\n--- ours\n${ours.out}\n--- ref\n${ref.out}\n`,
    );
  }
}
fs.rmSync(path.join(args.out, "effect-v3"), { recursive: true, force: true });
fs.rmSync(path.join(args.out, "effect-v4"), { recursive: true, force: true });
await Promise.all(Array.from({ length: Number(args.jobs) }, worker));
summary.mismatches.sort((a, b) => (a.version + a.case).localeCompare(b.version + b.case));
fs.writeFileSync(path.join(args.out, "summary.json"), JSON.stringify(summary, null, 2));
console.log(
  `cases ${summary.cases}, identical ${summary.match}, Effect mismatches ${summary.effectMismatch}, TS-only mismatches ${summary.tsOnlyMismatch}, crashes ${summary.crashes}`,
);
const byKind = {};
for (const m of summary.mismatches) byKind[m.kind] = (byKind[m.kind] ?? 0) + 1;
console.log(JSON.stringify(byKind));
process.exit(summary.mismatches.length ? 1 : 0);
