#!/usr/bin/env node
// Incremental and watch invalidation of Effect diagnostics. A small project
// goes through edits to a source file, an imported type, the tsconfig plugin
// options and a directive. After each edit:
//   - an `--incremental` run (reusing its .tsbuildinfo) must print what a
//     clean run of the same compiler prints, and
//   - a `--watch` session must report the same diagnostics.
// With --ref, the reference compiler's clean run must match too.
// With --plain (a compiler without Effect, such as the tsgo oracle), ours and
// plain take turns on one .tsbuildinfo: plain must print what its clean run
// prints (no "Unknown diagnostic message" panic on Effect build info), and
// ours must still print what its clean run prints.
//
// usage: node scripts/effect/incremental-check.mjs --ours BIN [--ref BIN] [--plain BIN] --modules <node_modules with effect> [--work DIR]
import { execFile, spawn } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { parseArgs, promisify } from "node:util";

const run = promisify(execFile);
const { values: args } = parseArgs({
  options: {
    ours: { type: "string" },
    ref: { type: "string" },
    plain: { type: "string" },
    modules: { type: "string" },
    work: { type: "string", default: "/tmp/effect-incremental" },
  },
});

// `plugin` adds plugin options (the allow-list steps).
const config = (dateSeverity, plugin = {}) =>
  JSON.stringify({
    compilerOptions: {
      strict: true, noEmit: true, allowImportingTsExtensions: true, module: "NodeNext", moduleResolution: "NodeNext", target: "ESNext", skipLibCheck: true,
      plugins: [{ name: "@effect/language-service", diagnosticSeverity: { globalDate: dateSeverity, floatingEffect: "error" }, ...plugin }],
    },
    include: ["src"],
  });
const cliOverride = (apis) => ({ overrides: [{ include: ["src/cli.ts"], options: { allowedUnstableApis: apis } }] });

const steps = [
  {
    name: "initial",
    files: {
      "tsconfig.json": config("error"),
      "package.json": '{ "type": "module" }',
      "src/make.ts": 'import { Effect } from "effect";\n\nexport const make = () => Effect.succeed(1);\n',
      "src/index.ts": 'import { make } from "./make.ts";\n\nmake();\nexport const when = new Date();\n',
    },
  },
  { name: "no change", files: {} },
  { name: "edit source", files: { "src/index.ts": 'import { make } from "./make.ts";\n\nmake();\nexport const when = new Date();\nexport const later = new Date();\n' } },
  { name: "edit imported type", files: { "src/make.ts": "export const make = () => 1;\n" } },
  { name: "restore imported type", files: { "src/make.ts": 'import { Effect } from "effect";\n\nexport const make = () => Effect.succeed(1);\n' } },
  { name: "edit config", files: { "tsconfig.json": config("off") } },
  { name: "restore config", files: { "tsconfig.json": config("error") } },
  {
    name: "add directive",
    files: { "src/index.ts": 'import { make } from "./make.ts";\n\n// @effect-diagnostics-next-line floatingEffect:off\nmake();\n// @effect-diagnostics-next-line globalDate:off\nexport const when = new Date();\nexport const later = new Date();\n' },
  },
  // unstableApiUsage and its allow lists. With an effect that has no
  // @stability tags (before 4.0.0) these steps report nothing new.
  { name: "use unstable API", files: { "src/cli.ts": 'import { Command } from "effect/cli";\n\nexport const cmd = Command.make("x");\n' } },
  { name: "allow at root", files: { "tsconfig.json": config("error", { allowedUnstableApis: ["effect/cli/Command#make"] }) } },
  { name: "allow in override", files: { "tsconfig.json": config("error", cliOverride(["effect/cli/Command"])) } },
  { name: "empty override list", files: { "tsconfig.json": config("error", { allowedUnstableApis: ["effect/cli"], ...cliOverride([]) }) } },
  { name: "restore allow lists", files: { "tsconfig.json": config("error") } },
];

const work = path.resolve(args.work);
fs.rmSync(work, { recursive: true, force: true });
const dirs = { inc: path.join(work, "incremental"), watch: path.join(work, "watch"), clean: path.join(work, "clean") };
if (args.plain) dirs.mixed = path.join(work, "mixed");
for (const dir of Object.values(dirs)) {
  fs.mkdirSync(path.join(dir, "node_modules"), { recursive: true });
  fs.symlinkSync(fs.realpathSync(path.join(args.modules, "effect")), path.join(dir, "node_modules/effect"));
}

async function tsc(bin, cwd, extra = []) {
  try {
    const { stdout } = await run(bin, ["-p", "tsconfig.json", "--pretty", "false", ...extra], { cwd });
    return { code: 0, out: stdout, err: "" };
  } catch (e) {
    return { code: e.code, out: e.stdout ?? "", err: e.stderr ?? "" };
  }
}
const lines = (out) => out.split("\n").filter((l) => /\): (error|warning|suggestion|message) TS/.test(l)).sort().join("\n");

// Watch session: collect output and split it at each "Watching for file changes" report.
const watch = spawn(args.ours, ["-p", "tsconfig.json", "--pretty", "false", "--watch", "--preserveWatchOutput"], { cwd: dirs.watch });
let watchBuffer = "";
const reports = [];
let waiter = null;
watch.stdout.on("data", (chunk) => {
  watchBuffer += chunk;
  let i;
  while ((i = watchBuffer.indexOf("Watching for file changes")) !== -1) {
    const end = watchBuffer.indexOf("\n", i);
    reports.push(watchBuffer.slice(0, end === -1 ? undefined : end));
    watchBuffer = end === -1 ? "" : watchBuffer.slice(end + 1);
    waiter?.();
  }
});
const nextReport = (count) =>
  new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error("watch report timeout")), 60000);
    waiter = () => {
      if (reports.length >= count) {
        clearTimeout(timer);
        resolve(reports[count - 1]);
      }
    };
    waiter();
  });

let failures = 0;
let watchReports = 0;
for (const step of steps) {
  for (const dir of Object.values(dirs)) {
    for (const [file, content] of Object.entries(step.files)) {
      fs.mkdirSync(path.dirname(path.join(dir, file)), { recursive: true });
      fs.writeFileSync(path.join(dir, file), content);
    }
  }
  fs.rmSync(path.join(dirs.clean, "tsconfig.tsbuildinfo"), { force: true });
  const clean = await tsc(args.ours, dirs.clean);
  const inc = await tsc(args.ours, dirs.inc, ["--incremental", "--tsBuildInfoFile", "inc.tsbuildinfo"]);
  const ref = args.ref ? await tsc(args.ref, dirs.clean) : null;
  let watched = null;
  if (step.name === "initial" || Object.keys(step.files).length) {
    watchReports++;
    watched = await nextReport(watchReports);
  }
  const problems = [];
  if (lines(inc.out) !== lines(clean.out)) problems.push(`incremental differs from clean:\n${inc.out}\n--- clean\n${clean.out}`);
  if ((inc.code === 0) !== (clean.code === 0)) problems.push(`incremental exit ${inc.code}, clean ${clean.code}`);
  if (watched !== null && lines(watched) !== lines(clean.out)) problems.push(`watch differs from clean:\n${watched}\n--- clean\n${clean.out}`);
  if (ref && lines(ref.out) !== lines(clean.out)) problems.push(`reference differs:\n${ref.out}\n--- ours\n${clean.out}`);
  if (args.plain) {
    const shared = ["--incremental", "--tsBuildInfoFile", "mixed.tsbuildinfo"];
    const mixedOurs = await tsc(args.ours, dirs.mixed, shared);
    const mixedPlain = await tsc(args.plain, dirs.mixed, shared);
    const plainClean = await tsc(args.plain, dirs.clean);
    if (lines(mixedOurs.out) !== lines(clean.out) || (mixedOurs.code === 0) !== (clean.code === 0))
      problems.push(`ours after plain differs from clean (exit ${mixedOurs.code}):\n${mixedOurs.out}${mixedOurs.err}`);
    if (mixedPlain.out !== plainClean.out || mixedPlain.code !== plainClean.code || mixedPlain.err)
      problems.push(`plain after ours differs from a clean plain run (exit ${mixedPlain.code}, clean ${plainClean.code}):\n${mixedPlain.out}${mixedPlain.err}`);
  }
  const count = lines(clean.out).split("\n").filter(Boolean).length;
  if (problems.length) {
    failures++;
    console.log(`FAIL ${step.name}\n  ${problems.join("\n  ")}`);
  } else console.log(`ok   ${step.name} (${count} diagnostics)`);
}
watch.kill();
console.log(failures ? `${failures} step(s) failed` : "all steps passed");
process.exit(failures ? 1 : 0);
