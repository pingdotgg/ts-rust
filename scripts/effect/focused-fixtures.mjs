#!/usr/bin/env node
// Focused Effect diagnostic fixtures (crates/ts_goport/tests/effect_fixtures).
// Each fixture is a small project with an expect.json:
//   { "fails": bool, "has": [substrings that must appear], "lacks": [substrings that must not] }
// Every fixture runs with our tsc and, when --ref is given, the reference
// Effect-patched tsc. Both must meet the expectations, and their Effect
// diagnostic lines (TS377xxx) must be identical. An optional "ref" object
// replaces fails/has/lacks for the reference, where it has a known bug
// ("refNote" says which). An optional "effect" names the effect version the
// fixture needs; with another version in --modules the fixture is skipped.
//
// usage: node scripts/effect/focused-fixtures.mjs --ours <tsc> [--ref <tsc>] --modules <node_modules dir with effect>
//          [--filter NAME] [--work DIR]
import { execFile } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs, promisify } from "node:util";

const run = promisify(execFile);
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const { values: args } = parseArgs({
  options: {
    ours: { type: "string" },
    ref: { type: "string" },
    modules: { type: "string" },
    filter: { type: "string", default: "" },
    work: { type: "string", default: "/tmp/effect-focused" },
  },
});
if (!args.ours || !args.modules) {
  console.error("--ours and --modules are required");
  process.exit(2);
}

const fixtures = path.join(root, "crates/ts_goport/tests/effect_fixtures");
fs.rmSync(args.work, { recursive: true, force: true });
fs.cpSync(fixtures, args.work, { recursive: true });
const work = fs.realpathSync(args.work);
fs.mkdirSync(path.join(work, "node_modules/@types"), { recursive: true });
fs.symlinkSync(fs.realpathSync(path.join(args.modules, "effect")), path.join(work, "node_modules/effect"));
fs.symlinkSync(fs.realpathSync(path.join(args.modules, "@types/node")), path.join(work, "node_modules/@types/node"));

async function check(bin, dir) {
  try {
    const { stdout } = await run(bin, ["-p", "tsconfig.json", "--pretty", "false"], { cwd: dir, maxBuffer: 1 << 26 });
    return { code: 0, out: stdout };
  } catch (e) {
    if (typeof e.code !== "number") throw e;
    return { code: e.code, out: e.stdout ?? "" };
  }
}

const effectVersion = JSON.parse(fs.readFileSync(path.join(work, "node_modules/effect/package.json"), "utf8")).version;
const effectLines = (out) => out.split("\n").filter((l) => / TS377\d\d\d: /.test(l)).join("\n");
let failures = 0;
for (const name of fs.readdirSync(work).sort()) {
  const dir = path.join(work, name);
  if (name.startsWith("_") || name === "node_modules" || !fs.statSync(path.join(work, name)).isDirectory() || !name.includes(args.filter)) continue;
  const expect = JSON.parse(fs.readFileSync(path.join(dir, "expect.json"), "utf8"));
  if (expect.effect && expect.effect !== effectVersion) {
    console.log(`skip ${name} (needs effect ${expect.effect}, --modules has ${effectVersion})`);
    continue;
  }
  const results = { ours: await check(args.ours, dir) };
  if (args.ref) results.ref = await check(args.ref, dir);
  const problems = [];
  for (const [who, r] of Object.entries(results)) {
    const exp = who === "ref" && expect.ref ? { ...expect, ...expect.ref } : expect;
    if ((r.code !== 0) !== exp.fails) problems.push(`${who}: exit ${r.code}, expected ${exp.fails ? "failure" : "success"}`);
    for (const s of exp.has) if (!r.out.includes(s)) problems.push(`${who}: missing ${JSON.stringify(s)}`);
    for (const s of exp.lacks) if (r.out.includes(s)) problems.push(`${who}: unexpected ${JSON.stringify(s)}`);
  }
  if (results.ref && effectLines(results.ours.out) !== effectLines(results.ref.out)) problems.push("ours and ref Effect diagnostics differ");
  if (problems.length) {
    failures++;
    console.log(`FAIL ${name}\n  ${problems.join("\n  ")}`);
    for (const [who, r] of Object.entries(results)) console.log(`  --- ${who} (exit ${r.code})\n${r.out.replace(/^/gm, "    ")}`);
  } else console.log(`ok   ${name}`);
}
console.log(failures ? `${failures} fixture(s) failed` : "all fixtures passed");
process.exit(failures ? 1 : 0);
