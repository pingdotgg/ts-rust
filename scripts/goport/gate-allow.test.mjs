import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const GATE = fileURLToPath(new URL("./gate.sh", import.meta.url));
const ALLOW = fileURLToPath(new URL("./gate-allow.txt", import.meta.url));

// Runs the allow step of gate.sh (load_allow and apply_allow of its Python helper) on rows, with each condition
// stubbed to hold. Takes an allow list text, or null for the real gate-allow.txt. Returns {entries, rows}, or
// {error} when the list does not load.
function allowStep(text, rows = []) {
  const dir = mkdtempSync(join(tmpdir(), "gate-allow-"));
  try {
    const allow = text === null ? ALLOW : join(dir, "gate-allow.txt");
    if (text !== null) writeFileSync(allow, text);
    const driver = `
import json, re, sys
ns = {'__name__': 'gate'}
exec(re.search(r"read -r -d '' PY <<'PYEOF'\\n(.*?)\\nPYEOF\\n", open(sys.argv[1]).read(), re.S)[1], ns)
for k in ns['CONDITIONS']:
    ns['CONDITIONS'][k] = lambda row, file, evidence: True
entries = ns['load_allow'](sys.argv[2])
rows = json.loads(sys.argv[3])
for row in rows:
    ns['apply_allow'](row, entries, None)
print(json.dumps({'entries': list(entries.values()), 'rows': rows}))`;
    const run = spawnSync("python3", ["-c", driver, GATE, allow, JSON.stringify(rows)],
      { env: { ...process.env, GOPORT_MAX_EXIT: "1" }, encoding: "utf8" });
    return run.status === 0 ? JSON.parse(run.stdout) : { error: run.stderr.trim() };
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

const ST = "single-threaded-equal";
const row = (id, detail) => ({ stage: id.split("/")[0], id, status: "FAIL", detail });

test("a corpus allow entry applies only to the item of its id with its case path", () => {
  const text = `corpus-diag/00001 | a.ts | ${ST} | trace order\ncorpus-emit/00003 | e.ts | ${ST} | trace order\n`;
  const { entries, rows } = allowStep(text, [
    row("corpus-diag/00001", "MISMATCH a.ts"),
    row("corpus-emit/00003", 'MISMATCH exit 0/0 e.ts first={"file": "x.js"}'),
  ]);
  assert.deepEqual(rows.map(r => [r.status, r.allowedBy?.[0].path]), [["ALLOWED", "a.ts"], ["ALLOWED", "e.ts"]]);
  assert.deepEqual(entries.map(e => e.used), [true, true]);
  // The same id is another case (another pin): the item stays FAIL and the entry is unused.
  const other = allowStep(text, [row("corpus-diag/00001", "MISMATCH w.ts"), row("corpus-emit/00003", "MISMATCH exit 0/0 w.ts")]);
  assert.deepEqual(other.rows.map(r => [r.status, r.allowedBy]), [["FAIL", undefined], ["FAIL", undefined]]);
  assert.deepEqual(other.entries.map(e => e.used), [false, false]);
  // Other families keep the old form, and a glob still applies.
  const syms = allowStep(`typesyms/effect/src!* | same-chars | order\n`,
    [{ ...row("typesyms/effect", "diffFiles=1"), ctx: { exits: [0, 0], diffFiles: ["src!a.d.ts"] } }]);
  assert.deepEqual(syms.rows.map(r => [r.status, r.allowedBy[0].id, "path" in r.allowedBy[0]]), [["ALLOWED", "typesyms/effect/src!*", false]]);
});

test("a corpus allow entry must name its case path", () => {
  for (const text of [`corpus-diag/00001 | ${ST} | trace order\n`, `f1/007 | ${ST} | trace order\n`,
    `corpus-diag/00001 | a.ts | ${ST} | x\ncorpus-diag/00001 | a.ts | ${ST} | y\n`]) {
    assert.match(allowStep(text).error, /gate-allow\.txt:[12]: (need "<id> \| <case path> \||corpus-diag\/00001 a\.ts is in two lines)/, text);
  }
});

test("gate-allow.txt: each corpus entry names the case of its reason", () => {
  const { entries, error } = allowStep(null);
  assert.equal(error, undefined);
  const corpus = entries.filter(e => e.id.startsWith("corpus-"));
  assert.equal(corpus.length, 70); // 14 traceResolution cases at each of 5 pins (dc37b5249ab6, 52168999f3dc, B, N, N')
  for (const e of corpus) {
    const name = /^traceResolution case (\S+?)[: ]/.exec(e.reason)[1];
    assert.equal(e.path.split("/").at(-1).replace(/\.tsx?$/, ""), name, e.id);
  }
});
