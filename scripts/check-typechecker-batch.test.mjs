import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, dirname, join, relative, resolve } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { gunzipSync, gzipSync } from "node:zlib";
import { BASELINE_SHA256, CARRY_FORWARD_RULE, CHECKPOINT_SHA256, GOPORT_BASELINE_SHA256, GOPORT_RULE, INHERITED_PIN, LAST_LEGACY_REVISION,
  TOOLS, canonicalJson, checkBatch, readEvidenceFile } from "./check-typechecker-batch.mjs";

const SOURCE = "a".repeat(64);
const RESULT = "b".repeat(64);
const tally = rows => ({ tests: rows.length, PASS: rows.filter(row => row.status === "PASS").length,
  FAIL: rows.filter(row => row.status === "FAIL").length, harnesses: new Set(rows.map(row => row.harness)).size });

function fixture() {
  const rows = Array.from({ length: 6330 }, (_, index) => ({ harness: index < 6328 ? "checker" : index === 6328 ? "compiler" : "fixture",
    name: `case_${index}`, status: "PASS", logStage: index < 6328 ? "checker" : index === 6328 ? "compiler" : "fixture" }));
  rows.push({ harness: "checker", name: "older_unprotected_failure", status: "FAIL", logStage: "checker" });
  const accepted = rows.slice(0, 6055).map(row => ({ harness: row.harness, name: row.name, accepted: { status: "PASS" } }));
  const closure = { normalClosure: true, sourceUnchanged: true, sourceFingerprint: SOURCE };
  const baseline = { closure, summary: tally(rows), originalAccepted6055: { exactLedger: accepted },
    versusPreviousFullBaseline: { exactLedger: rows.filter(row => row.logStage === "checker").map(row => ({ ...row, current: { status: row.status } })) },
    closedStages: Object.fromEntries(["compiler", "fixture"].map(stage => [stage, { closure, outcomes: rows.filter(row => row.logStage === stage) }])) };
  const candidate = { schemaVersion: 1, status: "ALL_STAGES_CLOSED", sourceFingerprint: SOURCE,
    closure: { normalClosures: true, sourceUnchangedThroughAllStages: true, sourceFingerprint: SOURCE },
    expectationChangesApplied: false, waiversApplied: false, summary: tally(rows), current: { ...tally(rows), ABSENT: 0, UNRUN: 0 },
    versusFullBaseline: { exactLedger: rows.map(row => ({ harness: row.harness, name: row.name, current: { status: row.status, logStage: row.logStage } })) }, addedNames: [],
    stages: ["checker", "compiler", "fixture"].map(stage => ({ stage, path: `${stage}.log`, sourceFingerprint: SOURCE,
      normalClosure: true, closureReceipt: `closed-${stage}`, counts: tally(rows.filter(row => row.logStage === stage)), exitCode: stage === "checker" ? 101 : 0 })),
    originalAccepted6055: { exactLedger: accepted.map(row => ({ ...row, current: { status: "PASS" } })) } };
  const verdict = (role, agent) => ({ role, agent, batchId: "batch-1", verdict: "PASS", sourceFingerprint: SOURCE, fullResultSha256: RESULT });
  const state = { schemaVersion: 1, phase: "initial-recovery", status: "ready", decision: "REVIEW", goalAuthorization: "Theo authorized initial recovery.", preservedCandidateSourceFingerprint: SOURCE,
    acceptedBaseline: { path: "checkpoint.json", sha256: CHECKPOINT_SHA256 },
    originalAccepted: { path: "baseline.json", sha256: BASELINE_SHA256, expectedNames: 6055 },
    laterPassBaseline: { path: "baseline.json", sha256: BASELINE_SHA256, expectedPasses: 6330 },
    batch: { id: "batch-1", implementer: "writer", hypothesis: "hypothesis-1", sourceFingerprint: SOURCE,
      fullResult: { path: "candidate.json", sha256: RESULT }, recoveryRevision: 1,
      recoveryHistory: [{ revision: 1, hypothesis: "hypothesis-1", sourceFingerprint: SOURCE, fullResultSha256: RESULT }],
      auditor: verdict("audit_accepted_roster", "auditor"), reviewer: verdict("production-review", "reviewer") } };
  return { state, baseline, candidate, read: ref => {
    if (ref.path === "checkpoint.json") return {};
    if (ref.path === "baseline.json") return baseline;
    if (ref.path === "candidate.json") return candidate;
    throw new Error("Missing synthetic evidence.");
  } };
}

function continuationFixture() {
  const f = fixture();
  f.state.phase = "recovery-continuation";
  f.state.continuationAuthorization = { authorized: true, date: "2026-09-08T03:26:02Z",
    instruction: "Theo authorized continued Query core and Hono recovery.",
    scope: "Preserve all revision history, protected passes and independent review." };
  const hypotheses = ["hypothesis-1", "hypothesis-1", "hypothesis-2", "hypothesis-2", "hypothesis-3", "hypothesis-3", "hypothesis-3"];
  f.state.batch.recoveryHistory = hypotheses.map((hypothesis, index) => ({ revision: index + 1, hypothesis,
    sourceFingerprint: String(index + 1).repeat(64), fullResultSha256: index === 3 ? "c".repeat(64) : null }));
  Object.assign(f.state.batch.recoveryHistory.at(-1), { sourceFingerprint: SOURCE, fullResultSha256: RESULT });
  f.state.batch.recoveryRevision = hypotheses.length;
  f.state.batch.hypothesis = hypotheses.at(-1);
  return f;
}

function refresh(candidate) {
  const rows = candidate.versusFullBaseline.exactLedger.map(row => ({ ...row, ...row.current }));
  candidate.summary = tally(rows);
  candidate.current = { ...tally(rows), ABSENT: 0, UNRUN: 0 };
  for (const stage of candidate.stages) {
    stage.counts = tally(rows.filter(row => row.logStage === stage.stage));
    stage.exitCode = stage.counts.FAIL ? 101 : 0;
  }
}

function stopped(f, pattern) {
  const result = checkBatch(f.state, f.read, undefined, f.tools);
  assert.equal(result.verdict, "STOP");
  if (pattern) assert.match(result.reasons.join(" "), pattern);
  return result;
}

test("complete pinned prerequisite passes, without claiming corpus parity", () => {
  const f = fixture(), result = checkBatch(f.state, f.read);
  assert.equal(result.verdict, "PASS");
  assert.deepEqual(result.counts, { originalAccepted: 6055, originalRetained: 6055, laterPasses: 6330, laterRetained: 6330,
    inheritedOriginal: null, inheritedLater: null });
  assert.match(result.scope, /corpus parity need independent review/);
});

test("paused state and absent batch fail closed before evidence reads", () => {
  const f = fixture(); f.state.status = "paused"; f.state.batch = null;
  assert.equal(checkBatch(f.state, () => assert.fail("No evidence read needed")).verdict, "STOP");
});

test("missing evidence and different same-size baseline identity stop", () => {
  const f = fixture();
  assert.equal(checkBatch(f.state, () => { throw new Error("Missing evidence"); }).verdict, "STOP");
  f.state.originalAccepted.sha256 = "c".repeat(64);
  stopped(f, /baseline identity/);
  f.state.originalAccepted.sha256 = BASELINE_SHA256;
  f.state.acceptedBaseline.sha256 = "c".repeat(64);
  stopped(f, /checkpoint identity/);
});

test("real evidence loader rejects missing files and hash mismatches", () => {
  assert.throws(() => readEvidenceFile({ path: "scripts/does-not-exist-guard-fixture.json", sha256: RESULT }), /Missing evidence/);
  assert.throws(() => readEvidenceFile({ path: "scripts/check-typechecker-batch.mjs", sha256: RESULT }), /hash mismatch/);
});

test("one exact accepted PASS loss is counted in both overlapping baselines", () => {
  const f = fixture();
  f.candidate.versusFullBaseline.exactLedger[0].current.status = "FAIL";
  f.candidate.originalAccepted6055.exactLedger[0].current.status = "FAIL";
  refresh(f.candidate);
  const result = stopped(f);
  assert.equal(result.losses.originalAccepted.length, 1);
  assert.equal(result.losses.laterPasses.length, 1);
  assert.equal(result.losses.originalAccepted[0].name, "case_0");
});

test("later nonaccepted PASS loss is protected independently", () => {
  const f = fixture(); f.candidate.versusFullBaseline.exactLedger[6100].current.status = "FAIL"; refresh(f.candidate);
  const result = stopped(f);
  assert.equal(result.losses.originalAccepted.length, 0);
  assert.equal(result.losses.laterPasses[0].name, "case_6100");
});

test("missing exact name is ABSENT, never a renamed-name waiver", () => {
  const f = fixture(); f.candidate.versusFullBaseline.exactLedger[0].name = "renamed_case_0";
  f.candidate.originalAccepted6055.exactLedger[0].current.status = "ABSENT";
  const result = stopped(f);
  assert.deepEqual(result.losses.originalAccepted[0], { harness: "checker", name: "case_0", status: "ABSENT" });
});

test("candidate accepted ledger cannot replace or hide an original name", () => {
  const f = fixture(); f.candidate.originalAccepted6055.exactLedger[0].name = "renamed";
  stopped(f, /renamed or omitted/);
});

test("source mismatch, partial stages, duplicate names and wrong totals stop", () => {
  for (const change of [
    f => { f.candidate.sourceFingerprint = RESULT; },
    f => { f.candidate.stages[0].sourceFingerprint = RESULT; },
    f => { f.candidate.stages.pop(); },
    f => { f.candidate.versusFullBaseline.exactLedger.push(f.candidate.versusFullBaseline.exactLedger[0]); },
    f => { f.candidate.summary.PASS++; },
    f => { f.candidate.current.UNRUN = 1; },
    f => { f.candidate.status = "CHECKER_CLOSED"; },
    f => { f.candidate.stages[0].normalClosure = false; },
  ]) { const f = fixture(); change(f); stopped(f); }
});

test("missing, STOP, stale, or non-independent verdicts stop", () => {
  for (const change of [
    f => { delete f.state.batch.reviewer; },
    f => { f.state.batch.reviewer.verdict = "STOP"; },
    f => { f.state.batch.reviewer.batchId = "older-batch"; },
    f => { f.state.batch.auditor.sourceFingerprint = RESULT; },
    f => { f.state.batch.auditor.fullResultSha256 = SOURCE; },
    f => { f.state.batch.reviewer.agent = "auditor"; },
    f => { f.state.batch.reviewer.role = "audit_accepted_roster"; },
    f => { f.state.batch.reviewer.agent = "writer"; },
  ]) { const f = fixture(); change(f); stopped(f); }
});

test("failed revisions remain counted and may have no full result", () => {
  const f = fixture(); f.state.batch.recoveryRevision = 2;
  f.state.batch.recoveryHistory.unshift({ revision: 1, hypothesis: "hypothesis-1", sourceFingerprint: RESULT, fullResultSha256: null });
  f.state.batch.recoveryHistory[1].revision = 2;
  assert.equal(checkBatch(f.state, f.read).verdict, "PASS");
  f.state.batch.recoveryHistory[1].fullResultSha256 = null;
  stopped(f, /Current history row/);
});

test("fixed limits reject extra revisions, hypotheses, and counter resets", () => {
  const base = fixture();
  for (const hypotheses of [["a", "a", "a"], ["a", "b", "c"], ["a", "a", "b", "b", "b"]]) {
    const f = fixture();
    f.state.batch.recoveryHistory = hypotheses.map((hypothesis, i) => ({ revision: i + 1, hypothesis, sourceFingerprint: SOURCE, fullResultSha256: RESULT }));
    f.state.batch.recoveryRevision = hypotheses.length;
    f.state.batch.hypothesis = hypotheses.at(-1);
    stopped(f, /[Ll]imit|1 to 4/);
  }
  base.state.batch.maxRecoveryRevisions = 99; stopped(base, /fixed at 4/);
  delete base.state.batch.maxRecoveryRevisions;
  base.state.batch.recoveryRevision = 0; stopped(base, /history length/);
  base.state.batch.recoveryRevision = 1;
  base.state.phase = "new-phase"; stopped(base, /Unsupported recovery phase/);
});

test("waivers, expectation exceptions, and missing hypotheses stop", () => {
  for (const field of ["waiversApplied", "expectationChangesApplied"]) {
    const f = fixture(); f.candidate[field] = true; stopped(f, /human review/);
  }
  const f = fixture(); f.state.batch.hypothesis = ""; stopped(f, /hypothesis/);
});

test("ready state must remove STOP and include goal authorization", () => {
  const f = fixture(); f.state.decision = "STOP"; stopped(f, /decision/);
  f.state.decision = "REVIEW";
  for (const value of [null, undefined, "", {}]) {
    f.state.goalAuthorization = value; stopped(f, /authorization/);
  }
});

test("authorized continuation retains the initial trial and permits later revisions", () => {
  const f = continuationFixture(), before = structuredClone(f.state);
  const result = checkBatch(f.state, f.read);
  assert.equal(result.verdict, "PASS");
  assert.deepEqual(result.counts, { originalAccepted: 6055, originalRetained: 6055, laterPasses: 6330, laterRetained: 6330,
    inheritedOriginal: null, inheritedLater: null });
  assert.match(result.scope, /Later added passes and corpus parity need independent review/);
  assert.deepEqual(f.state, before);
});

test("continuation requires explicit saved authorization before evidence reads", () => {
  for (const change of [
    f => { delete f.state.continuationAuthorization; },
    f => { f.state.continuationAuthorization = null; },
    f => { f.state.continuationAuthorization = "Theo said keep going"; },
    f => { f.state.continuationAuthorization = {}; },
    f => { f.state.continuationAuthorization.authorized = false; },
    f => { f.state.continuationAuthorization.authorized = "true"; },
    f => { f.state.continuationAuthorization.instruction = " "; },
    f => { f.state.continuationAuthorization.scope = ""; },
    f => { delete f.state.continuationAuthorization.date; },
    f => { f.state.continuationAuthorization.date = "invalid"; },
    f => { f.state.continuationAuthorization.date = "2026-99-99T00:00:00Z"; },
  ]) {
    const f = continuationFixture(); change(f);
    const result = checkBatch(f.state, () => assert.fail("Unauthorized continuation must not read evidence"));
    assert.equal(result.verdict, "STOP");
    assert.match(result.reasons.join(" "), /Continuation requires explicit saved authorization/);
  }
});

test("saved continuation authorization does not change initial recovery limits", () => {
  const f = continuationFixture(); f.state.phase = "initial-recovery";
  stopped(f, /1 to 4 measured revisions/);
  f.state.batch.recoveryHistory = f.state.batch.recoveryHistory.slice(0, 3);
  f.state.batch.recoveryRevision = 3;
  for (const row of f.state.batch.recoveryHistory) row.hypothesis = "hypothesis-1";
  stopped(f, /two revisions per hypothesis/);
  f.state.batch.recoveryHistory[2].hypothesis = "hypothesis-2";
  f.state.batch.maxRecoveryRevisions = 99;
  stopped(f, /fixed at 4/);
});

test("continuation rejects missing, reordered, duplicated and reset history", () => {
  for (const change of [
    f => { delete f.state.batch.recoveryHistory; },
    f => { f.state.batch.recoveryHistory = []; f.state.batch.recoveryRevision = 0; },
    f => { f.state.batch.recoveryHistory = f.state.batch.recoveryHistory.slice(0, 4); f.state.batch.recoveryRevision = 4; },
    f => { f.state.batch.recoveryRevision = 1; },
    f => { f.state.batch.recoveryHistory.shift(); f.state.batch.recoveryRevision--; },
    f => { [f.state.batch.recoveryHistory[0], f.state.batch.recoveryHistory[1]] = [f.state.batch.recoveryHistory[1], f.state.batch.recoveryHistory[0]]; },
    f => { f.state.batch.recoveryHistory[2] = { ...f.state.batch.recoveryHistory[1] }; },
    f => { f.state.batch.recoveryHistory[4].revision = 1; },
    f => { f.state.batch.recoveryHistory[5].revision = 7; },
    f => { f.state.batch.recoveryHistory[0].sourceFingerprint = "invalid"; },
    f => { delete f.state.batch.recoveryHistory[0].fullResultSha256; },
  ]) { const f = continuationFixture(); change(f); stopped(f, /history|History/); }
});

test("continuation cannot rewrite initial history to exceed its hypothesis limits", () => {
  for (const change of [
    f => { f.state.batch.recoveryHistory[2].hypothesis = "hypothesis-1"; },
    f => { f.state.batch.recoveryHistory[3].hypothesis = "hypothesis-3"; },
  ]) { const f = continuationFixture(); change(f); stopped(f, /two hypotheses, two revisions per hypothesis/); }
});

test("continuation binds the latest history row to the current source, hypothesis and full result", () => {
  for (const change of [
    row => { row.sourceFingerprint = "d".repeat(64); },
    row => { row.hypothesis = "older-hypothesis"; },
    row => { row.fullResultSha256 = "d".repeat(64); },
    row => { row.fullResultSha256 = null; },
  ]) {
    const f = continuationFixture(); change(f.state.batch.recoveryHistory.at(-1));
    stopped(f, /Current history row does not match/);
  }
});

test("continuation rejects protected lost passes even when another test recovers", () => {
  for (const index of [0, 6100]) {
    const f = continuationFixture();
    f.candidate.versusFullBaseline.exactLedger[index].current.status = "FAIL";
    if (index < 6055) f.candidate.originalAccepted6055.exactLedger[index].current.status = "FAIL";
    f.candidate.versusFullBaseline.exactLedger.at(-1).current.status = "PASS";
    refresh(f.candidate);
    const result = stopped(f, /PASS names are FAIL or ABSENT/);
    assert.equal(result.losses.originalAccepted.length, index < 6055 ? 1 : 0);
    assert.deepEqual(result.losses.laterPasses, [{ harness: "checker", name: `case_${index}`, status: "FAIL" }]);
  }
});

test("continuation rejects missing protected names, incomplete evidence and non-independent verdicts", () => {
  for (const change of [
    f => { f.candidate.versusFullBaseline.exactLedger[0].name = "renamed_case_0"; f.candidate.originalAccepted6055.exactLedger[0].current.status = "ABSENT"; },
    f => { f.candidate.closure.normalClosures = false; },
    f => { f.candidate.stages[1].sourceFingerprint = RESULT; },
    f => { f.state.batch.fullResult.path = "missing.json"; },
    f => { f.state.acceptedBaseline.sha256 = RESULT; },
    f => { f.candidate.versusFullBaseline.exactLedger[0].current.status = "UNRUN"; },
    f => { f.candidate.waiversApplied = true; },
    f => { f.candidate.expectationChangesApplied = true; },
    f => { delete f.state.batch.auditor; },
    f => { delete f.state.batch.reviewer; },
    f => { f.state.batch.auditor.verdict = "STOP"; },
    f => { f.state.batch.reviewer.verdict = "STOP"; },
    f => { f.state.batch.reviewer.sourceFingerprint = RESULT; },
    f => { f.state.batch.reviewer.batchId = "older-batch"; },
    f => { f.state.batch.reviewer.fullResultSha256 = SOURCE; },
    f => { f.state.batch.reviewer.agent = "writer"; },
    f => { f.state.batch.reviewer.agent = "auditor"; },
    f => { f.state.batch.reviewer.role = "audit_accepted_roster"; },
    f => { f.state.decision = "STOP"; },
    f => { f.state.status = "active"; },
  ]) { const f = continuationFixture(); change(f); stopped(f); }
});

// A candidate with inherited losses (case_0, and later-only case_6100) and an inherited
// result that already had them. PIN stands in for the real INHERITED_PIN.
const PIN = { sha256: "d".repeat(64), originalAccepted: 1, laterPasses: 2 };
function optInFixture() {
  const f = continuationFixture();
  f.candidate.versusFullBaseline.exactLedger[0].current.status = "FAIL";
  f.candidate.originalAccepted6055.exactLedger[0].current.status = "FAIL";
  f.candidate.versusFullBaseline.exactLedger[6100].current.status = "FAIL";
  refresh(f.candidate);
  const inherited = { versusFullBaseline: { exactLedger: f.candidate.versusFullBaseline.exactLedger.map(row => ({ ...row, current: { ...row.current } })) }, addedNames: [] };
  const read = f.read;
  f.read = ref => ref.path === "inherited.json" ? inherited : read(ref);
  f.state.acceptanceRuleChanges = [{ id: "opt-in-crate-no-new-loss", batchId: "batch-1", approvedBy: "Theo", date: "2026-09-25T06:00:00Z",
    instruction: "Opt-in crate rule", inheritedResult: { path: "inherited.json", sha256: PIN.sha256 },
    inheritedLosses: { originalAccepted: 1, laterPasses: 2 } }];
  return { ...f, inherited };
}

test("opt-in crate rule passes inherited losses but reports them", () => {
  const f = optInFixture(), result = checkBatch(f.state, f.read, PIN);
  assert.equal(result.verdict, "PASS");
  assert.equal(result.rule, "opt-in-crate-no-new-loss");
  assert.equal(result.counts.inheritedOriginal, 1);
  assert.equal(result.counts.inheritedLater, 2);
  assert.equal(result.losses.originalAccepted.length, 1);
  // The real pin (290/15, R96 hash) does not match this rule, so the CLI default stops.
  assert.equal(checkBatch(f.state, f.read).verdict, "STOP");
  assert.deepEqual(INHERITED_PIN, { sha256: "e7838ed863c42bc2271981c680d2fc46bb6a98f3171c8554b2d040ca9277d6b7", originalAccepted: 290, laterPasses: 15 });
});

test("opt-in crate rule still stops on a new loss, a count drift, or another batch", () => {
  let f = optInFixture();
  const stop = (g, pattern) => { const r = checkBatch(g.state, g.read, PIN); assert.equal(r.verdict, "STOP"); assert.match(r.reasons.join(" "), pattern); };
  f.candidate.versusFullBaseline.exactLedger[1].current.status = "FAIL";
  f.candidate.originalAccepted6055.exactLedger[1].current.status = "FAIL";
  refresh(f.candidate);
  stop(f, /not inherited/);
  f = optInFixture(); f.state.acceptanceRuleChanges[0].inheritedLosses.originalAccepted = 2;
  stop(f, /pinned constants/);
  f = optInFixture(); f.state.acceptanceRuleChanges[0].inheritedResult.sha256 = "e".repeat(64);
  stop(f, /pinned constants/);
  f = optInFixture(); f.inherited.versusFullBaseline.exactLedger[0].current.status = "PASS";
  stop(f, /Inherited loss counts/);
  f = optInFixture(); f.state.acceptanceRuleChanges[0].batchId = "other";
  stop(f, /original accepted PASS names/);
  f = optInFixture(); delete f.state.acceptanceRuleChanges[0].approvedBy;
  stop(f, /Theo's saved approval/);
});

test("approved unbound rows may keep a null source, never the current row", () => {
  const f = continuationFixture();
  Object.assign(f.state.batch.recoveryHistory[5], { sourceFingerprint: null, fullResultSha256: null });
  stopped(f, /Invalid or reset recovery history/);
  f.state.acceptanceRuleChanges = [{ id: "unbound-history-rows", batchId: "batch-1", approvedBy: "Theo", date: "2026-09-25T06:00:00Z",
    instruction: "Accept as recorded gaps", revisions: [6, 7] }];
  assert.equal(checkBatch(f.state, f.read).verdict, "PASS");
  Object.assign(f.state.batch.recoveryHistory[6], { sourceFingerprint: null });
  stopped(f, /Invalid or reset recovery history/);
});

// Revision 7 carries the full result of measured revision 6 (source FROM). Only
// crates/ts_goport changed, so both rows have the same roster fingerprint.
const FROM = "e".repeat(64), ROSTER = "f".repeat(64);
function carryFixture() {
  const f = continuationFixture();
  for (const item of [f.candidate, f.candidate.closure, ...f.candidate.stages]) item.sourceFingerprint = FROM;
  Object.assign(f.state.batch.recoveryHistory[5], { sourceFingerprint: FROM, fullResultSha256: RESULT, status: "full_measured", rosterFingerprint: ROSTER });
  f.state.batch.rosterFingerprint = ROSTER;
  f.state.batch.rosterCarryForward = { fromRevision: 6, fromSourceFingerprint: FROM, rosterFingerprint: ROSTER };
  f.state.acceptanceRuleChanges = [{ id: CARRY_FORWARD_RULE, batchId: "*", standing: true, approvedBy: "Theo", date: "2026-09-28T09:00:00Z",
    instruction: "Skip the roster run when only crates/ts_goport changed.", scope: "Every batch. Equal roster_fp.py hashes." }];
  return f;
}

test("roster carry-forward checks the earlier measured result and reports it", () => {
  const f = carryFixture(), result = checkBatch(f.state, f.read);
  assert.equal(result.verdict, "PASS");
  assert.deepEqual(result.rosterCarryForward, { rule: CARRY_FORWARD_RULE, fromRevision: 6, fromSourceFingerprint: FROM });
  const plain = fixture();
  assert.equal("rosterCarryForward" in checkBatch(plain.state, plain.read), false);
  delete f.state.batch.rosterCarryForward;
  stopped(f, /Full result source mismatch/);
});

test("roster carry-forward stops without the rule, equal fingerprints or an earlier measured row", () => {
  const from = f => f.state.batch.recoveryHistory[5];
  for (const [change, pattern] of [
    [f => { f.state.acceptanceRuleChanges = []; }, /standing goport-only-roster-carry-forward rule/],
    [f => { f.state.acceptanceRuleChanges[0].standing = false; }, /standing/],
    [f => { f.state.acceptanceRuleChanges[0].batchId = "batch-1"; }, /standing/],
    [f => { delete f.state.acceptanceRuleChanges[0].approvedBy; }, /standing/],
    [f => { f.state.batch.rosterFingerprint = "d".repeat(64); }, /differs from the batch roster fingerprint/],
    [f => { f.state.batch.rosterFingerprint = f.state.batch.rosterCarryForward.rosterFingerprint = "roster"; }, /SHA-256/],
    [f => { from(f).rosterFingerprint = "d".repeat(64); }, /differs from the carry-forward record/],
    [f => { from(f).fullResultSha256 = "d".repeat(64); }, /differs from the carry-forward record/],
    [f => { from(f).sourceFingerprint = "d".repeat(64); }, /differs from the carry-forward record/],
    [f => { from(f).status = "focused_only"; }, /not a full_measured revision/],
    [f => { from(f).rosterCarryForward = { fromRevision: 5 }; }, /not a full_measured revision/],
    [f => { f.state.batch.rosterCarryForward.fromRevision = 7; }, /earlier revision/],
    [f => { f.state.batch.auditor.sourceFingerprint = FROM; }, /Verdict is not bound/],
  ]) { const f = carryFixture(); change(f); stopped(f, pattern); }
});

test("only the carry-forward rule may use batchId *", () => {
  const f = optInFixture(); f.state.acceptanceRuleChanges[0].batchId = "*";
  assert.match(checkBatch(f.state, f.read, PIN).reasons.join(" "), /original accepted PASS names are FAIL or ABSENT\./);
});

// Adds measured rows before the current row, so the current row becomes revision n.
function atRevision(f, n) {
  const { batch } = f.state, current = batch.recoveryHistory.pop();
  while (batch.recoveryHistory.length < n - 1) {
    const revision = batch.recoveryHistory.length + 1;
    batch.recoveryHistory.push({ revision, hypothesis: "hypothesis-3", sourceFingerprint: revision.toString(16).padStart(64, "0"), fullResultSha256: null });
  }
  batch.recoveryHistory.push({ ...current, revision: n });
  batch.recoveryRevision = n;
  return f;
}

test("the legacy roster check, with or without the carry-forward, ends at R132", () => {
  assert.equal(LAST_LEGACY_REVISION, 132);
  for (const make of [continuationFixture, carryFixture]) {
    assert.equal(checkBatch(atRevision(make(), 132).state, make().read).verdict, "PASS");
    const f = atRevision(make(), 133);
    stopped(f, /R133 is after R132, the last revision under the legacy cargo roster\. It needs protectedSet "goport"/);
  }
});

// A goport batch (protectedSet "goport") after an accepted legacy batch-0. The test base is the
// rule baseline, the gate base is batch-0's gate, the LSP base is batch-0's lsp-r131 run (an
// older record with only its summary path) and the API base is the rule's apiBaseline api-r131.
// case_* roster evidence is not present. gitInputs is synthetic. gate-compare.py and
// oracle-compare.py are real: they run on copies of the fixture files in a temp dir.
const GO_PIN = "52168999f3dc", NEW_PIN = "16c25522e123", COMMIT = "0123456789abcdef0123456789abcdef01234567";
const SAME_INPUTS = "50b0593b54de8a2e44eccd0261670e3ada289887", OTHER_INPUTS = "fedcba9876543210fedcba9876543210fedcba98";
const NOTE = "legacy-removal-rule-approval-2026-09-28";
const BASELINE_PATH = "docs/goport-protected/tests-r131.json.gz";
const TSGO = "0".repeat(64);
const INPUTS = { crates: "a4aae8d62022eac88cee7fbdbdaa2fcd7c9e56e0", toml: "1".repeat(40), lock: "2".repeat(40) };
const COMMITS = { [COMMIT]: INPUTS, [SAME_INPUTS]: INPUTS, [OTHER_INPUTS]: { ...INPUTS, lock: "3".repeat(40) } };
const ROOT = fileURLToPath(new URL("../", import.meta.url));
const gitInputs = commit => {
  const full = Object.keys(COMMITS).find(key => typeof commit === "string" && commit.length >= 7 && key.startsWith(commit));
  if (!full) throw new Error(`git has no crates tree, Cargo.toml and Cargo.lock for commit ${commit}.`);
  return COMMITS[full];
};
const growth = value => `growth ${value} MiB/edit (limit 1.00, Go -0.04)`;
// One oracle trace: its requests (events) with their classes.
const trace = (battery, name, classes) => ({ battery, trace: name, events: classes.map((cls, i) => ({ i, method: "m", class: cls })) });
const check = f => checkBatch(f.state, f.read, undefined, f.tools);

function inTemp(run) {
  const dir = mkdtempSync(join(tmpdir(), "check-test-"));
  try {
    return run(dir);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

function writeJson(path, value) {
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, JSON.stringify(value));
  return path;
}

// Writes JSON lines as a gzip file.
function writeGz(path, lines) {
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, gzipSync(lines.map(line => JSON.stringify(line)).join("\n") + "\n"));
  return path;
}

// The real gate-compare.py and oracle-compare.py on temp copies of the fixture files: a gate
// manifest with the runs/ files next to it, the gate id map, and per oracle results dir its traces,
// summary.json, manifest.json and goport's responses (f.responses). The compare output names the
// fixture dirs and labels, as a real output names the real ones.
function fixtureTools(f) {
  return {
    gitInputs,
    gateCompare: (base, now, defects, toolChanges, idMap) => inTemp(dir => {
      const [from, to] = [base, now].map(path => relative(ROOT, path));
      for (const [path, file] of Object.entries(f.files)) {
        if (path.startsWith(`${dirname(from)}/runs/`)) writeJson(join(dir, "base", relative(dirname(from), path)), file.value);
      }
      const map = idMap && join(dir, "gate-id-map.tsv");
      if (map) writeFileSync(map, f.files[relative(ROOT, idMap.path)].value);
      return TOOLS.gateCompare(writeJson(join(dir, "base/manifest.json"), f.files[from].value),
        writeJson(join(dir, "new/manifest.json"), f.files[to].value), defects, toolChanges, map && { path: map, sha256: idMap.sha256 });
    }),
    oracleCompare: (bases, now, options) => inTemp(dir => {
      const sides = [...bases.map((path, i) => [`base${i}`, relative(ROOT, path)]), ["new", relative(ROOT, now)]];
      for (const [side, from] of sides) {
        const run = join(dir, "results", side);
        for (const item of f.oracle[from] ?? []) writeJson(join(run, "traces", item.battery, `${item.trace}.json`), item);
        for (const name of ["summary.json", "manifest.json"]) if (f.files[`${from}/${name}`]) writeJson(join(run, name), f.files[`${from}/${name}`].value);
        for (const [name, lines] of Object.entries(f.responses?.[from] ?? {})) writeGz(join(run, "responses", `${name}.jsonl.gz`), lines);
      }
      const output = TOOLS.oracleCompare(sides.slice(0, -1).map(([side]) => join(dir, "results", side)), join(dir, "results/new"), options);
      for (const [head, [, from]] of [...(output.bases ?? [output.base]).map((head, i) => [head, sides[i]]), [output.new, sides.at(-1)]]) {
        Object.assign(head, { dir: from, label: basename(from) });
      }
      return output;
    }),
    keptGateRuns: dir => Object.keys(f.files).filter(path => dirname(path) === dir && /^gate-compare-fail-.+\.json$/.test(basename(path))).sort(),
  };
}

// lsp_oracle.py summary.json of the traces of one run, with its goldens (oracle sha256 values) when given.
function lspSummary(traces, oracleSha256) {
  const batteries = {};
  for (const item of traces) {
    const battery = batteries[item.battery] ??= { traces: 0, requests: 0, crashExits: 0, classes: {} };
    battery.traces++;
    battery.requests += item.events.length;
    for (const event of item.events) battery.classes[event.class] = (battery.classes[event.class] ?? 0) + 1;
  }
  return { format: "goport-lsp-summary/1", goport: [{ path: "bins/tsgo", sha256: TSGO }], batteries, ...(oracleSha256 && { oracleSha256 }) };
}

// Writes the LSP run's summary.json and the pinned oracle-compare.py output (and compare) of both
// oracle records from the current traces, as candidate.sh and accept_revision.py do: with the answer
// sets of protectedBase and of the batch, and with batch.oracleRebase its runs as the base (bases),
// --parity with its known diffs and --identity.
function pinOracles(f) {
  const { batch } = f.state, lsp = batch.languageServerOracle.dir;
  f.put(`${lsp}/summary.json`, "0", lspSummary(f.oracle[lsp], f.lspGoldens));
  const sets = [...new Map([...(batch.protectedBase?.oracleAnswers ?? []), ...(batch.oracleAnswers ?? [])]
    .map(ref => [`${ref.kind} ${ref.sha256}`, ref])).values()];
  for (const [field, sha, kind] of [["languageServerOracle", "6", "lsp"], ["apiOracle", "f", "api"]]) {
    const record = batch[field], rebase = batch.oracleRebase?.[kind];
    const bases = rebase ? rebase.runs : [record.base];
    if (rebase) Object.assign(record, { base: { label: bases[0].label, dir: bases[0].dir }, bases: bases.map(({ label, dir }) => ({ label, dir })) });
    const answers = sets.filter(ref => ref.kind === kind).map(ref => ({ ...ref, path: resolve(ROOT, ref.path) }));
    const output = f.tools.oracleCompare(bases.map(run => resolve(ROOT, run.dir)), resolve(ROOT, record.dir),
      { kind, answers, parity: Boolean(rebase), knownDiffs: (rebase?.knownDiffs ?? []).map(diff => diff.key), identity: Boolean(rebase) });
    record.output = f.put(`${field}-compare.json`, sha, output);
    record.compare = structuredClone(output.total);
  }
}

function goportFixture() {
  const f = continuationFixture();
  const { state } = f;
  for (const field of ["acceptedBaseline", "originalAccepted", "laterPassBaseline"]) delete state[field];
  state[NOTE] = { quote: "i approve any rule changes that allow removing the legacy code" };
  state.acceptanceRuleChanges = [{ id: GOPORT_RULE, batchId: "*", standing: true, approvedBy: "Theo", date: "2026-09-28T22:00:00Z",
    instruction: "i approve any rule changes that allow removing the legacy code", scope: "Every batch with protectedSet goport.",
    protectedSet: "goport", baseline: { path: BASELINE_PATH, sha256: GOPORT_BASELINE_SHA256 }, approvalNote: NOTE,
    apiBaseline: { label: "api-r131", dir: "api/api-r131" } }];
  const batch = state.batch;
  delete batch.fullResult;
  const bound = { goportTestsSha256: "2".repeat(64), gateSha256: "7".repeat(64) };
  Object.assign(batch.recoveryHistory.at(-1), { fullResultSha256: null, status: "full_measured", ...bound });
  for (const verdict of [batch.auditor, batch.reviewer]) {
    delete verdict.fullResultSha256;
    Object.assign(verdict, bound);
  }
  const files = {};
  const put = (path, sha, value) => {
    const sha256 = sha.length === 64 ? sha : sha.repeat(64);
    files[path] = { sha256, value };
    return { path, sha256 };
  };
  const base = { source: { commit: "50b0593b5", tree: "a4aae8d62022", testbinSha256: "9".repeat(64) }, pin: GO_PIN, suites: {
    ts_goport_lib: { "api::t1": "ok", "api::t2": "ok", "api::slow": "ignored" },
    go_baselines: { "b::one": "ok", "b::two": "failed" },
    ts_scanner_lib: { "scan::a": "ok" } } };
  const results = structuredClone(base);
  results.source = { commit: COMMIT.slice(0, 9), tree: INPUTS.crates, testbinSha256: "a".repeat(64) };
  results.suites.go_baselines["b::two"] = "ok";
  results.suites.ts_goport_lib["api::t3"] = "ok";
  const allow = { id: "corpus-diag/00001/single", condition: "single-threaded-equal" };
  const gateItems = [
    { stage: "measure", id: "measure/query", status: "MATCH", detail: "query exit=0 diags=0" },
    { stage: "typesyms", id: "typesyms/effect", status: "ALLOWED", detail: "diffFiles=2", allowedBy: [{ id: "typesyms/effect/x", condition: "same-chars" }] },
    { stage: "editor", id: "editor/query-core/long", status: "FAIL", detail: growth("1.40") },
    { stage: "editor", id: "editor/hono/long", status: "MATCH", detail: "growth, rss, answers ok" },
    { stage: "corpus-diag", id: "corpus-diag/00001", status: "MATCH", detail: "MATCH a.ts" }];
  const gateBase = { commit: "b2b7dca1fc694b34dcf2303c8b013d544b043374", upstreamPin: GO_PIN, binsDir: "bins-r131",
    allowList: { sha256: "a".repeat(64), entries: [allow, { id: "typesyms/effect/x", condition: "same-chars" }] }, results: gateItems };
  const gateNew = { commit: COMMIT, upstreamPin: GO_PIN, binsDir: "bins", binaries: { tsgo: { path: "bins/tsgo", sha256: TSGO } },
    allowList: gateBase.allowList, results: [...structuredClone(gateItems), { stage: "sweep", id: "sweep/new", status: "MATCH", detail: "new" }] };
  // editor/hono/long is flaky on the same bins: MATCH in the base gate at the same Rust growth
  // (the base gate's editor run records it), FAIL in the new gate.
  Object.assign(gateNew.results.find(item => item.id === "editor/hono/long"), { status: "FAIL", detail: growth("1.13") });
  put(BASELINE_PATH, GOPORT_BASELINE_SHA256, base);
  const manifest = put("measure/r40/manifest.json", "4", { sourceFingerprint: SOURCE });
  put("quality.json", "5", { sourceFingerprint: SOURCE, rustfmtExit: 0, clippyExit: 0, tsGoportWarnings: 0, keptCrateWarnings: 0,
    fingerprintUnchanged: true });
  const run = { complete: true, exitCode: 0, diagnostics: 0, matchesOracle: true, sourceFingerprint: SOURCE,
    runs: [{ manifest: manifest.path, sha256: manifest.sha256 }] };
  const tests = put("tests-new.json", "2", results);
  const gate = put("gate/r132/manifest.json", "7", gateNew);
  const previousGate = put("gate/r131/manifest.json", "8", gateBase);
  put("gate/r131/runs/editor/result.json", "1", { sessions: [{ project: "hono", scenario: "long",
    rust: { cand: { binary: "bins-r131/tsgo", limits: { growth: [true, "1.13 MiB/edit"] } } } }] });
  const gateOutput = { base: { sha256: previousGate.sha256 }, new: { sha256: gate.sha256 }, regressions: [], knownOpen: [] };
  const output = put("gate-compare.json", "d", gateOutput);
  const archive = put("docs/typechecker-batches/batch-0.json", "3", { id: "batch-0", compilerAccepted: true,
    upstreamPin: { from: GO_PIN, to: GO_PIN }, gate: { manifest: previousGate.path, sha256: previousGate.sha256 },
    languageServerOracle: { summary: "lsp/lsp-r131/summary.md", result: "0 diff, 0 crash" } });
  const oracle = {
    "lsp/lsp-r131": [trace("b1", "t1", ["same", "same", "not_run"]), trace("b1", "t2", ["same", "oracle_error_same"])],
    "lsp/lsp-r132": [trace("b1", "t1", ["same", "same", "not_run"]), trace("b1", "t2", ["same", "oracle_error_same"]),
      trace("b2", "t3", ["same"])],
    "api/api-r131": [trace("qc", "a1", ["same", "diff"])],
    "api/api-r132": [trace("qc", "a1", ["same", "same"])],
  };
  // api_oracle.py manifest.json of each API run: the goport binary of each battery. The base ran R131's tsgo.
  put("api/api-r131/manifest.json", "0", { batteries: { qc: { goportSha: "b".repeat(64), traces: 1 } } });
  put("api/api-r132/manifest.json", "0", { batteries: { qc: { goportSha: TSGO, traces: 1 } } });
  state.batchRecords = ["docs/typechecker-batches/old.json", { ...archive, bytes: 1 }];
  const lspBase = { label: "lsp-r131", dir: "lsp/lsp-r131" }, apiBase = { label: "api-r131", dir: "api/api-r131" };
  Object.assign(batch, { protectedSet: "goport", commit: COMMIT.slice(0, 9), upstreamPin: { from: GO_PIN, to: GO_PIN },
    previousBatch: { id: "batch-0", archive: { ...archive, bytes: 1 } },
    protectedBase: { batch: "batch-0", revision: 6, tests: { path: BASELINE_PATH, sha256: GOPORT_BASELINE_SHA256 }, gate: previousGate,
      lsp: lspBase, api: apiBase },
    goportTests: { results: tests.path, sha256: tests.sha256, base: BASELINE_PATH, baseSha256: GOPORT_BASELINE_SHA256,
      compare: { lost: 0, absent: [], unrun: 0, retained: 4, recovered: 1 } },
    gate: { manifest: gate.path, sha256: gate.sha256 },
    gateCompare: { base: previousGate.path, baseSha256: previousGate.sha256, new: gate.path, sha256: gate.sha256, regressions: 0, output },
    gateRuns: [{ label: "r132", manifest: gate.path, sha256: gate.sha256, compare: output, regressions: [] }],
    ordinaryQuery: run, latestHono: structuredClone(run),
    languageServerOracle: { label: "lsp-r132", summary: "lsp/lsp-r132/summary.md", dir: "lsp/lsp-r132", base: { ...lspBase } },
    apiOracle: { label: "api-r132", summary: "api/api-r132/summary.md", dir: "api/api-r132", base: { ...apiBase } },
    quality: { record: "quality.json", result: "rustfmt 0, clippy 0" }, qualityEvidence: { sourceFingerprint: SOURCE },
    openDefects: [{ id: "editor-long-growth", status: "open; lsshells M2 running" }] });
  const read = (ref, { pinned = true } = {}) => {
    const file = files[ref?.path];
    if (!file) throw new Error(`Missing evidence: ${ref?.path}.`);
    if (pinned && ref.sha256 !== file.sha256) throw new Error(`Evidence hash mismatch: ${ref.path}.`);
    return file.value;
  };
  const result = { state, files, put, results, gateNew, gateOutput, oracle, read };
  result.tools = fixtureTools(result);
  pinOracles(result);
  return result;
}

// Binds the current history row and both verdicts to the batch goport test, gate and map hashes again.
function rebind(f) {
  const { batch } = f.state;
  for (const item of [batch.recoveryHistory.at(-1), batch.auditor, batch.reviewer]) {
    Object.assign(item, { goportTestsSha256: batch.goportTests.sha256, gateSha256: batch.gate.sha256,
      nameMapSha256: batch.goportTests.nameMap?.sha256 ?? null, gateIdMapSha256: batch.gateIdMap?.sha256 ?? null,
      oracleRebaseSha256: batch.oracleRebase ? createHash("sha256").update(canonicalJson(batch.oracleRebase)).digest("hex") : null,
      oracleAnswersSha256: (batch.oracleAnswers ?? []).map(ref => ref.sha256).sort() });
  }
}

// Makes batch-0 a goport batch: its results, LSP run and API run api-r131b are the base.
function goportPrevious(f) {
  const archive = f.files["docs/typechecker-batches/batch-0.json"].value;
  archive.protectedSet = "goport";
  const results = f.put("tests-prev.json", "a", structuredClone(f.files[BASELINE_PATH].value));
  archive.goportTests = { results: results.path, sha256: results.sha256 };
  archive.languageServerOracle = { label: "lsp-r131", dir: "lsp/lsp-r131" };
  archive.apiOracle = { label: "api-r131b", dir: "api/api-r131b" };
  f.oracle["api/api-r131b"] = [trace("qc", "a1", ["same", "same"])];
  f.put("api/api-r131b/manifest.json", "0", { batteries: { qc: { goportSha: "c".repeat(64), traces: 1 } } });
  const { batch } = f.state;
  batch.protectedBase.tests = { path: "tests-prev.json", sha256: "a".repeat(64) };
  batch.protectedBase.api = { label: "api-r131b", dir: "api/api-r131b" };
  batch.apiOracle.base = { label: "api-r131b", dir: "api/api-r131b" };
  Object.assign(batch.goportTests, { base: "tests-prev.json", baseSha256: "a".repeat(64) });
  pinOracles(f);
  return archive;
}

// The name map evidence file and its binding in the history row and verdicts.
function withMap(f, text) {
  f.state.batch.goportTests.nameMap = f.put("map.tsv", "c", text);
  rebind(f);
}

test("goport batch passes on its own tests, gate and oracles, without roster evidence", () => {
  const f = goportFixture(), result = check(f);
  assert.equal(result.verdict, "PASS", result.reasons.join(" "));
  assert.equal(result.protectedSet, "goport");
  assert.equal(result.rule, GOPORT_RULE);
  assert.deepEqual(result.counts, {
    goportTests: { baseOk: 4, retained: 4, recovered: 1, removedByMap: 0, removedIgnored: 0, newNames: 1, lost: 0, absent: 0, unrun: 0 },
    gate: { baseItems: 5, items: 6, regressions: 0, knownOpen: 2, runs: 1, flakes: 0 } });
  assert.deepEqual(result.gateFlakes, []);
  assert.deepEqual(result.base, { batch: "batch-0", tests: BASELINE_PATH, gate: "gate/r131/manifest.json" });
  // editor/hono/long is judged by its growth, not its status: base MATCH at 1.13, now FAIL at 1.13.
  assert.deepEqual(result.knownOpenGateItems.map(item => [item.id, item.growth, item.baseGrowth, item.baseStatus]),
    [["editor/query-core/long", 1.4, 1.4, "FAIL"], ["editor/hono/long", 1.13, 1.13, "MATCH"]]);
  assert.deepEqual(result.oracles, {
    lsp: { label: "lsp-r132", base: "lsp-r131", traces: 3, requests: 6, retained: 4, recovered: 0, lost: 0, unrun: 0, absent: 0, newRequests: 1 },
    api: { label: "api-r132", base: "api-r131", traces: 1, requests: 2, retained: 1, recovered: 1, lost: 0, unrun: 0, absent: 0, newRequests: 0 } });
  assert.match(result.scope, /goport protected set/);
});

test("the committed goport baseline has the pinned sha256 and reads as gzip JSON", () => {
  const baseline = readEvidenceFile({ path: BASELINE_PATH, sha256: GOPORT_BASELINE_SHA256 });
  const names = Object.values(baseline.suites).flatMap(Object.values);
  assert.deepEqual([baseline.pin, Object.keys(baseline.suites).length, names.length, names.filter(status => status === "ok").length],
    ["52168999f3dc", 19, 169555, 165544]);
});

test("a legacy batch ignores the goport rule and keeps the roster check", () => {
  const f = fixture(), g = goportFixture();
  f.state.acceptanceRuleChanges = g.state.acceptanceRuleChanges;
  const result = checkBatch(f.state, f.read);
  assert.equal(result.verdict, "PASS");
  assert.equal(result.counts.originalRetained, 6055);
  f.candidate.versusFullBaseline.exactLedger[0].current.status = "FAIL";
  f.candidate.originalAccepted6055.exactLedger[0].current.status = "FAIL";
  refresh(f.candidate);
  stopped(f, /original accepted PASS names/);
});

test("a planted lost goport name stops even when the saved compare shows none", () => {
  const f = goportFixture();
  f.results.suites.ts_goport_lib["api::t1"] = "failed";
  const result = stopped(f, /1 base ok goport test names are lost/);
  assert.deepEqual(result.losses.goportTests, [{ suite: "ts_goport_lib", name: "api::t1", status: "failed" }]);
  assert.equal(result.counts.goportTests.lost, 1);
});

test("an absent goport name stops", () => {
  const f = goportFixture();
  delete f.results.suites.go_baselines["b::one"];
  const result = stopped(f, /are absent/);
  assert.deepEqual(result.losses.goportTests, [{ suite: "go_baselines", name: "b::one", status: "absent" }]);
});

test("an unrun suite, an unrun name or an incomplete suite stops; ignored is lost", () => {
  let f = goportFixture();
  delete f.results.suites.ts_scanner_lib;
  assert.deepEqual(stopped(f, /are unrun/).losses.goportTests, [{ suite: "ts_scanner_lib", name: "scan::a", status: "unrun" }]);
  f = goportFixture();
  f.results.suites.ts_goport_lib["api::t2"] = "unrun";
  assert.deepEqual(stopped(f, /are unrun/).losses.goportTests, [{ suite: "ts_goport_lib", name: "api::t2", status: "unrun" }]);
  f = goportFixture();
  delete f.results.suites.ts_scanner_lib["scan::a"];
  f.results.incomplete = ["ts_scanner_lib"];
  assert.deepEqual(stopped(f, /are unrun/).losses.goportTests, [{ suite: "ts_scanner_lib", name: "scan::a", status: "unrun" }]);
  f = goportFixture();
  f.results.suites.ts_goport_lib["api::t2"] = "ignored";
  assert.deepEqual(stopped(f, /are lost/).losses.goportTests, [{ suite: "ts_goport_lib", name: "api::t2", status: "ignored" }]);
});

test("a saved compare that reports a loss stops", () => {
  for (const [field, value] of [["lost", 1], ["absent", ["b::one"]], ["unrun", 2]]) {
    const f = goportFixture();
    f.state.batch.goportTests.compare[field] = value;
    stopped(f, new RegExp(`goportTests.compare reports ${field}`));
  }
  const f = goportFixture(); delete f.state.batch.goportTests.compare.unrun;
  stopped(f, /compare.unrun needs a count/);
});

test("a wrong sha256 or a base that is not the pinned baseline stops", () => {
  for (const [change, pattern] of [
    [f => { f.state.batch.goportTests.sha256 = "e".repeat(64); }, /Current history row does not match/],
    [f => { f.state.batch.goportTests.sha256 = "e".repeat(64); rebind(f); }, /hash mismatch: tests-new.json/],
    [f => { f.state.batch.goportTests.baseSha256 = "e".repeat(64); }, /base must be the protected baseline/],
    [f => { f.state.batch.goportTests.base = "tests-new.json"; f.state.batch.goportTests.baseSha256 = "2".repeat(64); }, /base must be the protected baseline/],
    [f => { f.state.batch.protectedBase.tests.sha256 = "e".repeat(64); }, /protectedBase differs/],
    [f => { f.state.batch.protectedBase.lsp.dir = "lsp/lsp-r130"; }, /protectedBase differs/],
    [f => { delete f.state.batch.protectedBase.api; }, /protectedBase differs/],
    [f => { f.state.acceptanceRuleChanges[0].baseline.sha256 = "e".repeat(64); }, /baseline must be the pinned first goport baseline/],
    [f => { f.state.batch.gateCompare.sha256 = "e".repeat(64); }, /must be batch.gate/],
    [f => { f.state.batch.gate.sha256 = f.state.batch.gateCompare.sha256 = "e".repeat(64); rebind(f); }, /hash mismatch: gate\/r132/],
    [f => { f.state.batch.previousBatch.archive.sha256 = "e".repeat(64); }, /must be the last saved batch record/],
    [f => { f.state.batch.previousBatch.archive.sha256 = f.state.batchRecords[1].sha256 = "e".repeat(64); }, /hash mismatch: docs\/typechecker-batches/],
    [f => { f.state.batch.ordinaryQuery.runs[0].sha256 = "e".repeat(64); }, /hash mismatch: measure/],
  ]) { const f = goportFixture(); change(f); stopped(f, pattern); }
});

test("a state edit cannot swap in a weaker first baseline", () => {
  const f = goportFixture();
  f.results.suites.ts_goport_lib["api::t1"] = "failed";
  const weak = structuredClone(f.files[BASELINE_PATH].value);
  delete weak.suites.ts_goport_lib["api::t1"];
  const ref = f.put("weak.json", "e", weak);
  f.state.acceptanceRuleChanges[0].baseline = ref;
  f.state.batch.protectedBase.tests = ref;
  Object.assign(f.state.batch.goportTests, { base: ref.path, baseSha256: ref.sha256 });
  stopped(f, /baseline must be the pinned first goport baseline/);
});

test("gate items are judged by gate-compare.py: removed, MATCH lost, new FAIL, noise and allowedBy", () => {
  const item = (f, id) => f.gateNew.results.find(row => row.id === id);
  const fresh = [{ id: "corpus-diag/00001/new", condition: "single-threaded-equal" }];
  for (const [change, regression] of [
    [f => { item(f, "measure/query").status = "FAIL"; }, { id: "measure/query", base: "MATCH", now: "FAIL" }],
    [f => { item(f, "corpus-diag/00001").status = "ALLOWED"; }, { id: "corpus-diag/00001", base: "MATCH", now: "ALLOWED" }],
    [f => { Object.assign(item(f, "corpus-diag/00001"), { status: "ALLOWED", allowedBy: fresh }); }, { id: "corpus-diag/00001", base: "MATCH", now: "ALLOWED" }],
    [f => { item(f, "typesyms/effect").status = "FAIL"; }, { id: "typesyms/effect", base: "ALLOWED", now: "FAIL" }],
    [f => { delete item(f, "typesyms/effect").allowedBy; }, { id: "typesyms/effect", base: "ALLOWED", now: "ALLOWED" }],
    [f => { f.gateNew.results = f.gateNew.results.filter(row => row.id !== "measure/query"); }, { id: "measure/query", base: "MATCH", now: "REMOVED" }],
    [f => { item(f, "sweep/new").status = "FAIL"; }, { id: "sweep/new", base: "NEW", now: "FAIL" }],
    [f => { item(f, "editor/query-core/long").detail = growth("3.00"); }, { id: "editor/query-core/long", base: "FAIL", now: "FAIL" }],
    [f => { item(f, "editor/query-core/long").detail = `${growth("1.40")}; rss 3000 MiB`; }, { id: "editor/query-core/long", base: "FAIL", now: "FAIL" }],
    [f => { item(f, "editor/hono/long").detail = growth("1.40"); }, { id: "editor/hono/long", base: "MATCH", now: "FAIL" }],
    [f => { delete f.files["gate/r131/runs/editor/result.json"]; }, { id: "editor/hono/long", base: "MATCH", now: "FAIL" }],
  ]) {
    const f = goportFixture(); change(f);
    const result = stopped(f, /1 gate items regressed/);
    assert.deepEqual(result.losses.gate.map(({ id, base, now }) => ({ id, base, now })), [regression]);
    assert.ok(result.losses.gate[0].why);
  }
  // A flaky single-threaded-equal item: MATCH to ALLOWED by an entry of the base allow list (same id, condition and case path).
  const f = goportFixture();
  f.files["gate/r131/manifest.json"].value.allowList.entries.push({ id: "corpus-diag/00001", path: "a.ts", condition: "single-threaded-equal" });
  Object.assign(item(f, "corpus-diag/00001"), { status: "ALLOWED",
    allowedBy: [{ id: "corpus-diag/00001", path: "a.ts", condition: "single-threaded-equal" }] });
  assert.equal(check(f).verdict, "PASS");
});

// The base allow list has one entry for corpus-diag/00001 (entry), and the new run (same pin) has corpus-diag/00001,
// the case a.ts, ALLOWED by allowedBy. Returns the check result.
function allowCase(entry, allowedBy) {
  const f = goportFixture();
  f.files["gate/r131/manifest.json"].value.allowList.entries.push({ id: "corpus-diag/00001", condition: "single-threaded-equal", ...entry });
  Object.assign(f.gateNew.results.find(row => row.id === "corpus-diag/00001"),
    { status: "ALLOWED", allowedBy: [{ id: "corpus-diag/00001", condition: "single-threaded-equal", ...allowedBy }] });
  return check(f);
}

test("an allow entry with the right id and another case path does not allow (auditor: cross-pin id reuse)", () => {
  // The base allow list has the entry of w.ts, a case that had the id corpus-diag/00001 at another pin. Here the id is a.ts,
  // so after the first acceptance at this pin the entry cannot reallow a.ts: not with its own path, not with a.ts, and not
  // in the old form without a path (gate.sh before case paths).
  for (const [allowedBy, why] of [
    [{ path: "w.ts" }, "ALLOWED by an allow entry of another case than a.ts: corpus-diag/00001 (w.ts)"],
    [{ path: "a.ts" }, "base MATCH is ALLOWED by an allow entry the base did not have: corpus-diag/00001 (single-threaded-equal, a.ts)"],
    [{}, "base MATCH is ALLOWED by an allow entry the base did not have: corpus-diag/00001 (single-threaded-equal, a.ts)"],
  ]) {
    const result = allowCase({ path: "w.ts" }, allowedBy);
    assert.equal(result.verdict, "STOP");
    assert.deepEqual(result.losses.gate.map(({ id, base, now, why }) => [id, base, now, why]), [["corpus-diag/00001", "MATCH", "ALLOWED", why]]);
  }
  // The entry of a.ts reallows a.ts. An old-form base entry (r137-full) is the entry of the case of its id in the base run.
  for (const entry of [{ path: "a.ts" }, {}]) {
    for (const allowedBy of [{ path: "a.ts" }, {}]) {
      const result = allowCase(entry, allowedBy);
      assert.equal(result.verdict, "PASS", result.reasons.join(" "));
      assert.equal(result.counts.gate.regressions, 0);
    }
  }
});

test("a gate tool change passes only when batch.gateToolChanges lists it exactly (R133)", () => {
  const withTool = (f, sha) => { f.files["gate/r131/manifest.json"].value.gate = { path: "scripts/goport/gate.sh", sha256: "a".repeat(64) };
    f.gateNew.gate = { path: "scripts/goport/gate.sh", sha256: sha }; };
  let f = goportFixture(); withTool(f, "b".repeat(64));
  const result = stopped(f, /1 gate items regressed/);
  assert.deepEqual(result.losses.gate.map(({ id }) => id), ["tool/gate"]);
  f = goportFixture(); withTool(f, "b".repeat(64));
  f.state.batch.gateToolChanges = [{ key: "gate", from: "a".repeat(64), to: "c".repeat(64), reason: "wrong to" }];
  stopped(f, /1 gate items regressed/);
  f = goportFixture(); withTool(f, "b".repeat(64));
  f.state.batch.gateToolChanges = [{ key: "gate", from: "a".repeat(64), to: "b".repeat(64), reason: "the reviewed stage 1 gate.sh" }];
  assert.equal(check(f).verdict, "PASS");
});

// A pin bump that renumbers the corpus cases: at pin, the base case corpus-diag/00001 (a.ts) is
// corpus-diag/00002, and extra new corpus items can follow. The batch, its tests and its gate are at pin.
function renumbered(f, extra = [], pin = NEW_PIN) {
  f.gateNew.results.find(row => row.id === "corpus-diag/00001").id = "corpus-diag/00002";
  f.gateNew.results.push(...extra.map(([id, detail]) => ({ stage: "corpus-diag", id, status: "MATCH", detail })));
  f.gateNew.upstreamPin = f.results.pin = pin;
  f.state.batch.upstreamPin = { from: GO_PIN, to: pin };
}

// The gate id map in batch.gateIdMap, with its real sha256 (gate-compare.py checks it), in the saved
// gate compare output, and bound in the history row and both verdicts (bind false leaves them unbound).
function withIdMap(f, text, { bind = true } = {}) {
  const sha256 = createHash("sha256").update(text).digest("hex");
  f.state.batch.gateIdMap = f.put("gate-id-map.tsv", sha256, text);
  f.gateOutput.idMap = { path: "gate-id-map.tsv", sha256 };
  if (bind) rebind(f);
}

test("a gate id map moves a renumbered id at a pin bump: with it PASS, without it STOP", () => {
  let f = goportFixture(); renumbered(f);
  let result = stopped(f, /1 gate items regressed/);
  assert.deepEqual(result.losses.gate.map(({ id, now, why }) => [id, now, why]), [["corpus-diag/00001", "REMOVED", "removed id"]]);
  withIdMap(f, "# bump\noldId\tnewId\tsource\ncorpus-diag/00001\tcorpus-diag/00002\ta.ts\n");
  result = check(f);
  assert.equal(result.verdict, "PASS", result.reasons.join(" "));
  assert.deepEqual([result.counts.gate.baseItems, result.counts.gate.items, result.counts.gate.regressions], [5, 6, 0]);
  // A base allow entry moves with its id: MATCH to ALLOWED by the moved entry is reallowed, not a new entry.
  f.files["gate/r131/manifest.json"].value.allowList.entries.push({ id: "corpus-diag/00001", condition: "single-threaded-equal" });
  Object.assign(f.gateNew.results.find(row => row.id === "corpus-diag/00002"),
    { status: "ALLOWED", allowedBy: [{ id: "corpus-diag/00002", condition: "single-threaded-equal" }] });
  assert.equal(check(f).verdict, "PASS");
  // The saved compare must name the map, and the map must have its sha256.
  delete f.gateOutput.idMap;
  stopped(f, /gateCompare.output must be the gate-compare.py output/);
  f.gateOutput.idMap = { sha256: f.state.batch.gateIdMap.sha256 };
  f.state.batch.gateIdMap.sha256 = "e".repeat(64);
  rebind(f);
  stopped(f, /hash mismatch: gate-id-map.tsv/);
  // At one Go pin the map has no effect.
  f = goportFixture(); renumbered(f, [], GO_PIN);
  withIdMap(f, "corpus-diag/00001\tcorpus-diag/00002\ta.ts\n");
  result = stopped(f, /1 gate items regressed/);
  assert.deepEqual(result.losses.gate.map(({ id, now }) => [id, now]), [["corpus-diag/00001", "REMOVED"]]);
});

test("a gate id map cannot hide a real removal", () => {
  // a.ts is gone at the new pin, and a new case c.ts has corpus-diag/00002 (the renamed item is dropped).
  const gone = f => {
    renumbered(f, [["corpus-diag/00003", "MATCH c.ts"]]);
    f.gateNew.results = f.gateNew.results.filter(row => row.id !== "corpus-diag/00002");
    f.gateNew.results.find(row => row.id === "corpus-diag/00003").id = "corpus-diag/00002";
  };
  for (const [change, text, why] of [
    // The map pairs a.ts with the new case: the new item is the case c.ts.
    [gone, "corpus-diag/00001\tcorpus-diag/00002\ta.ts\n", /id map line 1: corpus-diag\/00002 is the case c.ts, not a.ts/],
    // The map names the new case's path: it is not the case path of the base item.
    [gone, "corpus-diag/00001\tcorpus-diag/00002\tc.ts\n", /id map line 1: c.ts is not the case path of corpus-diag\/00001, a.ts/],
    // The map points at an id that is not in the new run.
    [f => renumbered(f), "corpus-diag/00001\tcorpus-diag/00007\ta.ts\n", /its new id corpus-diag\/00007 is not in the new run/],
    // A new case c.ts takes the old id, and the map has no line for it: never compared with c.ts.
    [f => renumbered(f, [["corpus-diag/00001", "MATCH c.ts"]]), "corpus-diag/00009\tcorpus-diag/00002\ta.ts\n", /family corpus-diag is mapped/],
  ]) {
    const f = goportFixture(); change(f); withIdMap(f, text);
    const result = stopped(f, /1 gate items regressed/);
    assert.deepEqual(result.losses.gate.map(({ id, now }) => [id, now]), [["corpus-diag/00001", "REMOVED"]]);
    assert.match(result.losses.gate[0].why, why);
  }
  // Malformed maps are bad input: two old ids for one new id, a missing source, "-" as the case path.
  for (const [text, why] of [
    ["corpus-diag/00001\tcorpus-diag/00002\ta.ts\ncorpus-diag/00009\tcorpus-diag/00002\ta.ts\n", /line 2: corpus-diag\/00002 is in two lines/],
    ["corpus-diag/00001\tcorpus-diag/00002\n", /line 1: need/],
    ["corpus-diag/00001\tcorpus-diag/00002\t-\n", /line 1: need/],
    ["corpus-diag/00001\tcorpus-emit/00002\ta.ts\n", /are not ids of one corpus family/],
  ]) {
    const f = goportFixture(); renumbered(f); withIdMap(f, text);
    stopped(f, why);
  }
});

// The removed-id regressions of a STOP result, as [id, why].
const removedIds = result => result.losses.gate.filter(item => item.now === "REMOVED").map(({ id, why }) => [id, why]);

test("a gate id map line must name the case path of both items, never a word they share (skeptic: generic word)", () => {
  // a.ts is gone at the new pin, and a different case c.ts has corpus-diag/00002. Both details hold the word MATCH.
  const gone = f => {
    renumbered(f, [["corpus-diag/00003", "MATCH c.ts"]]);
    f.gateNew.results = f.gateNew.results.filter(row => row.id !== "corpus-diag/00002");
    f.gateNew.results.find(row => row.id === "corpus-diag/00003").id = "corpus-diag/00002";
  };
  let f = goportFixture(); gone(f); withIdMap(f, "corpus-diag/00001\tcorpus-diag/00002\tMATCH\n");
  assert.deepEqual(removedIds(stopped(f, /1 gate items regressed/)),
    [["corpus-diag/00001", "removed id (id map line 1: MATCH is not the case path of corpus-diag/00001, a.ts)"]]);
  // A note after the case path (an allow condition that did not hold) does not change the case path.
  f = goportFixture(); renumbered(f);
  Object.assign(f.files["gate/r131/manifest.json"].value.results.find(row => row.id === "corpus-diag/00001"),
    { status: "FAIL", detail: "MISMATCH a.ts (allow entry corpus-diag/00001 condition single-threaded-equal did not hold for None)" });
  withIdMap(f, "corpus-diag/00001\tcorpus-diag/00002\ta.ts\n");
  const result = check(f);
  assert.equal(result.verdict, "PASS", result.reasons.join(" "));
  assert.equal(result.counts.gate.regressions, 0);
  // Only the corpus families have a case path, so no other family can have a line.
  f = goportFixture(); renumbered(f); withIdMap(f, "measure/query\tmeasure/query2\tquery\n");
  stopped(f, /line 1: measure\/query and measure\/query2 are not ids of one corpus family/);
});

// Base: corpus-diag/00001 MATCH a.ts and corpus-diag/00002 ALLOWED w.ts by the base allow entry corpus-diag/00002.
// New pin: a.ts is corpus-diag/00011 and now ALLOWED by a new entry, w.ts is corpus-diag/00012 and MATCH.
function swapped(f) {
  const base = f.files["gate/r131/manifest.json"].value, st = "single-threaded-equal";
  base.results.push({ stage: "corpus-diag", id: "corpus-diag/00002", status: "ALLOWED", detail: "MISMATCH w.ts",
    allowedBy: [{ id: "corpus-diag/00002", condition: st }] });
  base.allowList.entries.push({ id: "corpus-diag/00002", condition: st });
  f.gateNew.results = f.gateNew.results.filter(row => row.id !== "corpus-diag/00001");
  f.gateNew.results.push({ stage: "corpus-diag", id: "corpus-diag/00011", status: "ALLOWED", detail: "MISMATCH a.ts",
    allowedBy: [{ id: "corpus-diag/00011", condition: st }] },
  { stage: "corpus-diag", id: "corpus-diag/00012", status: "MATCH", detail: "MATCH w.ts" });
  f.gateNew.upstreamPin = f.results.pin = NEW_PIN;
  f.state.batch.upstreamPin = { from: GO_PIN, to: NEW_PIN };
}

test("a swapped gate id map cannot move an allow entry to another case (skeptic: swap)", () => {
  // The skeptic's swap: each line names a word of both details and pairs a.ts with w.ts, so the base allow
  // entry of w.ts would cover a.ts. Both lines are broken and both base ids are removed ids.
  for (const text of ["corpus-diag/00001\tcorpus-diag/00012\tMATCH\ncorpus-diag/00002\tcorpus-diag/00011\tMISMATCH\n",
    "corpus-diag/00001\tcorpus-diag/00012\ta.ts\ncorpus-diag/00002\tcorpus-diag/00011\tw.ts\n"]) {
    const f = goportFixture(); swapped(f); withIdMap(f, text);
    const result = stopped(f, /2 gate items regressed/);
    assert.deepEqual(removedIds(result).map(([id]) => id), ["corpus-diag/00001", "corpus-diag/00002"]);
  }
  // The honest map moves the entry of w.ts with w.ts only: a.ts, MATCH in the base, is ALLOWED by an entry the base did not have.
  const f = goportFixture(); swapped(f);
  withIdMap(f, "corpus-diag/00001\tcorpus-diag/00011\ta.ts\ncorpus-diag/00002\tcorpus-diag/00012\tw.ts\n");
  const result = stopped(f, /1 gate items regressed/);
  assert.deepEqual(result.losses.gate.map(({ id, base, now, why }) => [id, base, now, why]), [["corpus-diag/00011", "MATCH", "ALLOWED",
    "base MATCH is ALLOWED by an allow entry the base did not have: corpus-diag/00011 (single-threaded-equal, a.ts)"]]);
  // One case path in two lines of one family is bad input.
  withIdMap(f, "corpus-diag/00001\tcorpus-diag/00011\ta.ts\ncorpus-diag/00002\tcorpus-diag/00012\ta.ts\n");
  stopped(f, /line 2: a.ts is in two lines/);
});

test("a gate id map moves a base allow entry only with its own case path (auditor: cross-pin id reuse)", () => {
  // At the new pin a.ts is corpus-diag/00002 and ALLOWED. The base entry of corpus-diag/00001 moves to corpus-diag/00002
  // with its path: the entry of a.ts reallows a.ts, and the entry of w.ts (an old pin's case at that id) does not.
  const run = (entryPath, allowedPath) => {
    const f = goportFixture(); renumbered(f);
    f.files["gate/r131/manifest.json"].value.allowList.entries.push({ id: "corpus-diag/00001", path: entryPath, condition: "single-threaded-equal" });
    Object.assign(f.gateNew.results.find(row => row.id === "corpus-diag/00002"),
      { status: "ALLOWED", allowedBy: [{ id: "corpus-diag/00002", path: allowedPath, condition: "single-threaded-equal" }] });
    withIdMap(f, "corpus-diag/00001\tcorpus-diag/00002\ta.ts\n");
    return check(f);
  };
  const result = run("a.ts", "a.ts");
  assert.equal(result.verdict, "PASS", result.reasons.join(" "));
  for (const [allowedPath, why] of [
    ["a.ts", "base MATCH is ALLOWED by an allow entry the base did not have: corpus-diag/00002 (single-threaded-equal, a.ts)"],
    ["w.ts", "ALLOWED by an allow entry of another case than a.ts: corpus-diag/00002 (w.ts)"]]) {
    assert.deepEqual(run("w.ts", allowedPath).losses.gate.map(({ id, why }) => [id, why]), [["corpus-diag/00002", why]]);
  }
});

test("the history row and both verdicts bind the gate id map (skeptic: unbound map)", () => {
  const text = "corpus-diag/00001\tcorpus-diag/00002\ta.ts\n", sha = createHash("sha256").update(text).digest("hex");
  const f = goportFixture(); renumbered(f); withIdMap(f, text, { bind: false });
  const { batch } = f.state, row = batch.recoveryHistory.at(-1);
  stopped(f, /Current history row does not match the batch source, hypothesis, and goport test, gate, name map and gate id map hashes/);
  row.gateIdMapSha256 = sha;
  stopped(f, /Verdict is not bound to this batch, source, and goport test, gate, name map and gate id map hashes/);
  rebind(f);
  assert.equal(check(f).verdict, "PASS");
  // A bound sha256 that is not the batch map, and a bound map when the batch has none.
  batch.reviewer.gateIdMapSha256 = "e".repeat(64);
  stopped(f, /Verdict is not bound/);
  const g = goportFixture(); g.state.batch.recoveryHistory.at(-1).gateIdMapSha256 = sha;
  stopped(g, /Current history row does not match/);
  // The bound sha256 is the batch map's, and the file must have it.
  rebind(f);
  f.files["gate-id-map.tsv"].value = "corpus-diag/00001\tcorpus-diag/00002\tb.ts\n";
  stopped(f, /gate id map .* does not have the sha256/);
});

test("a removal line needs Go evidence at both pins, whatever commit its note cites (skeptic: made-up commit)", () => {
  // The new run does not hold a.ts: Go removed it, or it left the gate sample. Either way its base id is a removed id.
  const gone = (f, pin) => {
    renumbered(f, [], pin);
    f.gateNew.results = f.gateNew.results.filter(row => row.id !== "corpus-diag/00002");
  };
  let f = goportFixture(); gone(f);
  assert.deepEqual(removedIds(stopped(f, /1 gate items regressed/)), [["corpus-diag/00001", "removed id"]]);
  // A removal line needs a note that names the Go commit that deletes the case (bump C reviewer ruling 2 item 3), but
  // the note is no evidence. With a made-up commit, the pin B commit or tsgo #4407 the removal check (Go at both pins)
  // decides: a.ts is in no Go checkout, so it is a removed id. At one pin the map has no effect.
  const noBase = /^removed id \(id map line \d removes it, but a\.ts is not in the base pin Go checkout /;
  for (const commit of ["deadbeef", "16c25522e123", "bbdf7a24b"]) {
    f = goportFixture(); gone(f, NEW_PIN); withIdMap(f, `corpus-diag/00001\t-\ta.ts\tdeleted by ${commit}\n`);
    assert.match(removedIds(stopped(f, /1 gate items regressed/))[0][1], noBase);
    f = goportFixture(); gone(f, GO_PIN); withIdMap(f, `corpus-diag/00001\t-\ta.ts\tdeleted by ${commit}\n`);
    assert.deepEqual(removedIds(stopped(f, /1 gate items regressed/)), [["corpus-diag/00001", "removed id"]]);
  }
  // The skeptic's map: an honest line for each case the new run holds, then a removal line for a.ts.
  f = goportFixture(); gone(f, NEW_PIN);
  f.files["gate/r131/manifest.json"].value.results.push({ stage: "corpus-diag", id: "corpus-diag/00005", status: "MATCH", detail: "MATCH b.ts" });
  f.gateNew.results.push({ stage: "corpus-diag", id: "corpus-diag/00006", status: "MATCH", detail: "MATCH b.ts" });
  withIdMap(f, "corpus-diag/00005\tcorpus-diag/00006\tb.ts\n");
  assert.deepEqual(removedIds(stopped(f, /1 gate items regressed/)),
    [["corpus-diag/00001", "removed id (the id map has no line for it, and its family corpus-diag is mapped)"]]);
  withIdMap(f, "corpus-diag/00005\tcorpus-diag/00006\tb.ts\ncorpus-diag/00001\t-\ta.ts\tdeleted by deadbeef\n");
  assert.match(removedIds(stopped(f, /1 gate items regressed/))[0][1], noBase);
  // Other forms are bad input: a removal line without a note or whose note names no commit, "-" as the old id or the
  // case path, a fourth cell on a move.
  const need = /line 1: need "<old id> TAB <new id> TAB <case path>" or "<old id> TAB - TAB <case path> TAB <note naming the Go commit that deletes the case>"/;
  for (const text of ["corpus-diag/00001\t-\ta.ts\n", "corpus-diag/00001\t-\ta.ts\tts#64122\n",
    "corpus-diag/00001\tcorpus-diag/00002\ta.ts\tdeadbeef\n", "corpus-diag/00001\tcorpus-diag/00002\t-\n", "-\tcorpus-diag/00002\ta.ts\n"]) {
    f = goportFixture(); gone(f); withIdMap(f, text);
    stopped(f, need);
  }
});

test("the saved gate compare must show no regression and name the pinned manifests", () => {
  let f = goportFixture(); f.state.batch.gateCompare.regressions = [{ id: "editor/query-core/long" }];
  stopped(f, /gateCompare reports gate regressions/);
  f = goportFixture(); f.gateOutput.regressions = [{ id: "measure/query" }];
  stopped(f, /gateCompare reports gate regressions/);
  f = goportFixture(); f.gateOutput.base.sha256 = "e".repeat(64);
  stopped(f, /gateCompare.output must be the gate-compare.py output/);
  f = goportFixture(); f.state.batch.gateCompare.output.sha256 = "e".repeat(64);
  stopped(f, /hash mismatch: gate-compare.json/);
  f = goportFixture(); delete f.state.batch.gateCompare.output;
  stopped(f, /Missing evidence/);
});

// A failed earlier gate run r132-fail of the same source (measure/query MATCH to FAIL), kept by
// candidate.sh side as gate-compare-fail-r132-fail.json next to gate-compare.json. listed puts it
// in gateRuns, as accept_revision.py does.
function failedGateRun(f, { listed = true, commit = COMMIT } = {}) {
  const manifest = structuredClone(f.gateNew);
  manifest.commit = commit;
  Object.assign(manifest.results.find(item => item.id === "measure/query"), { status: "FAIL", detail: "query exit=1" });
  const gate = f.put("gate/r132-fail/manifest.json", "1234".repeat(16), manifest);
  const regressions = [{ id: "measure/query", base: "MATCH", new: "FAIL", why: "base MATCH is not MATCH", flake: null }];
  const compare = f.put("gate-compare-fail-r132-fail.json", "5678".repeat(16), { new: { manifest: gate.path, sha256: gate.sha256 }, regressions });
  if (listed) f.state.batch.gateRuns.unshift({ label: "r132-fail", manifest: gate.path, sha256: gate.sha256, compare, regressions });
}

const flakeNote = (item, run) => ({ item, source: "r132", runs: [{ label: run, result: "FAIL" }, { label: "r132", result: "MATCH" }],
  evidence: "timeout at load 14 on zbook, MATCH on dbook-lan" });

test("every failed gate run of the source needs a flake note for each regressed item (repeat-run rule)", () => {
  // Without a flake note the failed run is a loss, although batch.gate passes.
  let f = goportFixture();
  failedGateRun(f);
  let result = stopped(f, /Gate run r132-fail: measure\/query MATCH -> FAIL \(base MATCH is not MATCH\) has no flake note flake-r7-<name>/);
  assert.equal(result.reasons.length, 1);
  // A note that names another run, another item, a longer id or another revision does not count.
  for (const [name, note] of [["flake-r7-query", flakeNote("measure/query", "r132-fail-2")],
    ["flake-r7-query", flakeNote("measure/hono", "r132-fail")], ["flake-r7-query", flakeNote("measure/query/x", "r132-fail")],
    ["flake-r6-query", flakeNote("measure/query", "r132-fail")]]) {
    f = goportFixture(); failedGateRun(f); f.state[name] = note;
    stopped(f, /Gate run r132-fail: measure\/query .* has no flake note/);
  }
  // A saved note that names the item and the run: PASS, and the output lists the flake.
  f = goportFixture(); failedGateRun(f); f.state["flake-r7-query"] = flakeNote("measure/query", "r132-fail");
  result = check(f);
  assert.equal(result.verdict, "PASS", result.reasons.join(" "));
  assert.deepEqual(result.gateFlakes, [{ run: "r132-fail", id: "measure/query", note: "flake-r7-query" }]);
  assert.deepEqual([result.counts.gate.runs, result.counts.gate.flakes], [2, 1]);
});

test("gateRuns must list every kept gate run, the batch gate last, with its true regressions", () => {
  const note = f => { f.state["flake-r7-query"] = flakeNote("measure/query", "r132-fail"); };
  for (const [change, pattern] of [
    // The kept failed run is left out of gateRuns (the accept refusal bypassed and the record dropped).
    [f => { failedGateRun(f, { listed: false }); }, /failed gate run gate-compare-fail-r132-fail.json of the source is not in gateRuns/],
    [f => { failedGateRun(f, { listed: false }); note(f); }, /is not in gateRuns/],
    [f => { delete f.state.batch.gateRuns; }, /gateRuns must list every gate run/],
    [f => { f.state.batch.gateRuns = []; }, /gateRuns must list every gate run/],
    [f => { failedGateRun(f); note(f); f.state.batch.gateRuns.reverse(); }, /batch.gate last/],
    [f => { failedGateRun(f); note(f); f.state.batch.gateRuns[0].label = "r132"; }, /its own label/],
    [f => { failedGateRun(f); note(f); f.state.batch.gateRuns[0].regressions = []; }, /its regressions differ from gate-compare.py now/],
    [f => { failedGateRun(f); note(f); f.state.batch.gateRuns[0].sha256 = "e".repeat(64); }, /is not in gateRuns/],
    [f => { failedGateRun(f); note(f); f.state.batch.gateRuns[0].compare = f.state.batch.gateCompare.output; }, /compare is not the gate-compare.py output/],
    [f => { failedGateRun(f, { commit: OTHER_INPUTS }); note(f); }, /Gate run r132-fail is not a run of the batch source/],
  ]) { const f = goportFixture(); change(f); stopped(f, pattern); }
  // A run compared again after a state fix: the batch gate is the kept run, and the check judges it as batch.gate.
  const f = goportFixture();
  f.put("gate-compare-fail-r132.json", "9abc".repeat(16), { new: { sha256: f.state.batch.gate.sha256 }, regressions: [{ id: "editor/hono/long" }] });
  assert.equal(check(f).verdict, "PASS");
});

test("the editor long-growth FAIL passes only while its open defect is recorded", () => {
  for (const defects of [[], [{ id: "editor-long-growth", status: "closed" }], undefined]) {
    const f = goportFixture(); f.state.batch.openDefects = defects;
    const result = stopped(f, /2 gate items regressed/);
    assert.deepEqual(result.losses.gate.map(({ id, base, now }) => ({ id, base, now })),
      [{ id: "editor/query-core/long", base: "FAIL", now: "FAIL" }, { id: "editor/hono/long", base: "MATCH", now: "FAIL" }]);
  }
});

test("the base is the last accepted batch: its goport results and oracle runs when it had them", () => {
  let f = goportFixture();
  f.files["docs/typechecker-batches/batch-0.json"].value.compilerAccepted = false;
  stopped(f, /accepted batch/);
  f = goportFixture();
  goportPrevious(f);
  const result = check(f);
  assert.equal(result.verdict, "PASS", result.reasons.join(" "));
  assert.deepEqual(result.oracles.api, { label: "api-r132", base: "api-r131b", traces: 1, requests: 2, retained: 2, recovered: 0,
    lost: 0, unrun: 0, absent: 0, newRequests: 0 });
  f.state.batch.protectedBase.tests = { path: BASELINE_PATH, sha256: GOPORT_BASELINE_SHA256 };
  stopped(f, /protectedBase differs from the base that accepted batch batch-0 gives/);
  delete f.state.batch.protectedBase;
  assert.equal(check(f).verdict, "PASS");
  Object.assign(f.state.batch.goportTests, { base: BASELINE_PATH, baseSha256: GOPORT_BASELINE_SHA256 });
  stopped(f, /base must be the results of accepted batch batch-0/);
  // After a goport base batch, the rule's apiBaseline is not the API base.
  f = goportFixture();
  goportPrevious(f);
  f.state.batch.apiOracle.base = { label: "api-r131", dir: "api/api-r131" };
  delete f.state.batch.protectedBase;
  pinOracles(f);
  stopped(f, /apiOracle needs its results dir and the base run api\/api-r131b/);
  f = goportFixture();
  f.state.batch.gateCompare.base = "gate/r132/manifest.json";
  stopped(f, /gateCompare base must be the gate manifest of accepted batch batch-0/);
});

test("the base cannot roll back to an older accepted batch", () => {
  let f = goportFixture();
  f.state.batchRecords.push(f.put("docs/typechecker-batches/batch-1.json", "9", { id: "batch-1", compilerAccepted: true }));
  stopped(f, /previousBatch.archive must be the last saved batch record/);
  f = goportFixture();
  f.state.batchRecords.reverse();
  stopped(f, /previousBatch.archive must be the last saved batch record/);
  f = goportFixture();
  f.state.batchRecords = [];
  stopped(f, /previousBatch.archive must be the last saved batch record/);
});

test("the API oracle is required and keeps every base same request", () => {
  for (const [change, pattern] of [
    [f => { delete f.state.batch.apiOracle; }, /apiOracle needs its results dir/],
    [f => { delete f.state.acceptanceRuleChanges[0].apiBaseline; delete f.state.batch.protectedBase.api; },
      /apiOracle: the base batch names no oracle run to compare with/],
    [f => { f.state.batch.apiOracle.base.dir = "api/api-r130"; }, /apiOracle needs its results dir and the base run api\/api-r131/],
    [f => { f.oracle["api/api-r132"] = [trace("qc", "a1", ["diff", "same"])]; pinOracles(f); },
      /apiOracle compare reports lost requests\. .*apiOracle: 1 lost protected base requests/],
    [f => { f.oracle["api/api-r132"] = [trace("qc", "a1", ["not_run", "same"])]; pinOracles(f); }, /apiOracle: 1 unrun protected base requests/],
    [f => { f.oracle["api/api-r132"] = [trace("qc", "a2", ["same"])]; pinOracles(f); }, /apiOracle: 1 absent protected base requests/],
    // The results dir changed after its pinned compare.
    [f => { f.oracle["api/api-r132"][0].events[0].class = "diff"; }, /apiOracle: oracle-compare.py gives other counts now than its pinned output/],
    [f => { f.oracle["api/api-r132"] = []; }, /oracle-compare.py failed \(exit 2\)/],
    [f => { f.state.batch.apiOracle.compare.lost = 1; }, /apiOracle compare reports lost requests/],
    [f => { delete f.state.batch.apiOracle.compare; }, /apiOracle.compare.lost needs a count/],
    [f => { f.files["apiOracle-compare.json"].value.new.dir = "api/api-r131"; }, /apiOracle.output must be the oracle-compare.py output/],
    [f => { f.state.batch.apiOracle.output.sha256 = "e".repeat(64); }, /hash mismatch: apiOracle-compare.json/],
  ]) { const f = goportFixture(); change(f); stopped(f, pattern); }
});

test("the API run used the gate's tsgo for every battery and ran every base battery", () => {
  const manifest = (f, dir = "api/api-r132") => f.files[`${dir}/manifest.json`].value;
  for (const [change, pattern] of [
    [f => { manifest(f).batteries.qc.goportSha = "e".repeat(64); }, /apiOracle ran another goport binary than the gate's tsgo \(batteries qc\)/],
    [f => { delete manifest(f).batteries.qc.goportSha; }, /apiOracle ran another goport binary than the gate's tsgo \(batteries qc\)/],
    [f => { manifest(f).batteries.zod = { goportSha: "e".repeat(64), traces: 0 }; }, /another goport binary than the gate's tsgo \(batteries zod\)/],
    [f => { manifest(f).batteries = {}; }, /apiOracle ran another goport binary than the gate's tsgo\./],
    [f => { delete f.files["api/api-r132/manifest.json"]; }, /Missing evidence: api\/api-r132\/manifest.json/],
    [f => { delete f.files["api/api-r131/manifest.json"]; }, /Missing evidence: api\/api-r131\/manifest.json/],
    [f => { f.files["api/api-r132/manifest.json"].value = { qc: {} }; }, /api\/api-r132\/manifest.json is not an api_oracle.py manifest/],
    [f => { Object.assign(manifest(f, "api/api-r131").batteries, { zod: { traces: 3 }, hono: { traces: 2 } }); },
      /apiOracle did not run the base batteries zod, hono\./],
  ]) { const f = goportFixture(); change(f); stopped(f, pattern); }
  // More batteries than the base is fine when they ran the gate's tsgo.
  const f = goportFixture();
  manifest(f).batteries.zod = { goportSha: TSGO, traces: 0 };
  assert.equal(check(f).verdict, "PASS");
  // After a goport base batch, the base batteries come from its API run.
  goportPrevious(f);
  manifest(f, "api/api-r131b").batteries.effect = { traces: 1 };
  stopped(f, /apiOracle did not run the base batteries effect\./);
});

test("the LSP oracle is clean, run with the gate's tsgo and keeps every base same request", () => {
  const b1 = f => f.files["lsp/lsp-r132/summary.json"].value.batteries.b1;
  for (const [change, pattern] of [
    [f => { b1(f).classes.diff = 3; }, /LSP oracle has 3 diff\./],
    [f => { b1(f).classes.goport_error = 1; b1(f).classes.timeout = 2; }, /LSP oracle has 1 goport_error, 2 timeout\./],
    [f => { b1(f).crashExits = 1; }, /LSP oracle has 1 crash\./],
    [f => { b1(f).requests += 1; }, /summary.json counts other traces or requests than the oracle compare/],
    [f => { f.files["lsp/lsp-r132/summary.json"].value.goport[0].sha256 = "e".repeat(64); }, /another goport binary than the gate's tsgo/],
    [f => { f.files["lsp/lsp-r132/summary.json"].value.format = "other"; }, /is not an lsp_oracle.py summary/],
    [f => { f.oracle["lsp/lsp-r132"][1].events[1].class = "flaky_oracle"; pinOracles(f); }, /languageServerOracle: 1 lost protected base requests/],
    [f => { f.oracle["lsp/lsp-r132"].splice(1, 1); pinOracles(f); }, /languageServerOracle: 2 absent protected base requests/],
    [f => { f.state.batch.languageServerOracle.base = { label: "lsp-r130", dir: "lsp/lsp-r130" }; }, /needs its results dir and the base run lsp\/lsp-r131/],
    [f => { delete f.state.batch.languageServerOracle; }, /languageServerOracle needs its results dir/],
  ]) { const f = goportFixture(); change(f); stopped(f, pattern); }
});

test("a checked name map covers renamed names, never two names in one", () => {
  const f = goportFixture();
  const suite = f.results.suites.go_baselines;
  suite["b::uno"] = suite["b::one"]; delete suite["b::one"];
  stopped(f, /are absent/);
  withMap(f, "oldSuite\toldName\tnewSuite\tnewName\tevidence\ngo_baselines\tb::one\tgo_baselines\tb::uno\tGo renamed it: go/x.go:12\n");
  const result = check(f);
  assert.equal(result.verdict, "PASS", result.reasons.join(" "));
  assert.equal(result.counts.goportTests.newNames, 1);
  withMap(f, "go_baselines\tb::one\tgo_baselines\tb::uno\n");
  stopped(f, /Name map line 1 needs oldSuite, oldName, newSuite, newName and evidence/);
  withMap(f, "go_baselines\tb::one\tgo_baselines\tb::uno\t\n");
  stopped(f, /Name map line 1 needs/);
  delete suite["b::two"];
  withMap(f, "go_baselines\tb::one\tgo_baselines\tb::uno\te\ngo_baselines\tb::two\tgo_baselines\tb::uno\te\n");
  stopped(f, /Two base names map to go_baselines b::uno/);
  withMap(f, "go_baselines\tb::one\tgo_baselines\tb::uno\te\n");
  f.state.batch.goportTests.nameMap.sha256 = "e".repeat(64);
  rebind(f);
  stopped(f, /hash mismatch: map.tsv/);
});

test("a name map cannot hide a lost name", () => {
  // The old name is still in the new results (and fails): the map points at a passing name.
  let f = goportFixture();
  f.results.suites.ts_goport_lib["api::t1"] = "failed";
  withMap(f, "ts_goport_lib\tapi::t1\tts_goport_lib\tapi::t3\tmoved\n");
  stopped(f, /Name map line 1: ts_goport_lib api::t1 is still in the new results/);
  // A swap: the lost base ok name maps to a base name that passes now.
  f = goportFixture();
  f.results.suites.ts_goport_lib["api::t1"] = "failed";
  withMap(f, "ts_goport_lib\tapi::t1\tgo_baselines\tb::two\tswap\ngo_baselines\tb::two\tts_goport_lib\tapi::t1\tswap\n");
  stopped(f, /is still in the new results|is a base name/);
  f = goportFixture();
  delete f.results.suites.ts_goport_lib["api::t1"];
  withMap(f, "ts_goport_lib\tapi::t1\tgo_baselines\tb::two\tswap\n");
  stopped(f, /Name map line 1: the new name go_baselines b::two is a base name/);
  // A removal of a lost name at the same pin, and a removal of a name that still runs.
  f = goportFixture();
  delete f.results.suites.ts_goport_lib["api::t1"];
  withMap(f, "ts_goport_lib\tapi::t1\t-\t-\tgone\n");
  stopped(f, /removes ts_goport_lib api::t1, but the Go pin did not change/);
  f = goportFixture();
  f.results.suites.ts_goport_lib["api::t1"] = "failed";
  withMap(f, "ts_goport_lib\tapi::t1\t-\t-\tgone\n");
  stopped(f, /api::t1 is still in the new results/);
});

test("a removal needs a Go pin change or a kept-crate suite, and is listed for the reviewer", () => {
  let f = goportFixture();
  delete f.results.suites.go_baselines["b::one"];
  f.results.pin = NEW_PIN;
  f.gateNew.upstreamPin = NEW_PIN;
  f.state.batch.upstreamPin = { from: GO_PIN, to: NEW_PIN };
  withMap(f, "# pin bump\ngo_baselines\tb::one\t-\t-\tremoved at 16c25522e123: go/testdata/x.ts deleted\n");
  let result = check(f);
  assert.equal(result.verdict, "PASS", result.reasons.join(" "));
  assert.equal(result.counts.goportTests.removedByMap, 1);
  assert.deepEqual(result.nameMapRemoved, [{ suite: "go_baselines", name: "b::one", evidence: "removed at 16c25522e123: go/testdata/x.ts deleted" }]);
  // A removal line may keep a go_baselines_reference name that is ignored in the new results (bump C ruling 1
  // item 5), not an ok or failed one, and not an ignored name of another suite (a libtest #[ignore]).
  const REF = "tsc/commandLine/adds-color.js";
  for (const [suite, name, status, pass] of [["go_baselines_reference", REF, "ignored", true], ["go_baselines_reference", REF, "ok", false],
    ["go_baselines_reference", REF, "failed", false], ["go_baselines", "b::one", "ignored", false]]) {
    f = goportFixture();
    f.files[BASELINE_PATH].value.suites.go_baselines_reference = { [REF]: "ok" };
    f.results.suites.go_baselines_reference = { [REF]: "ok" };
    f.results.suites[suite][name] = status;
    f.results.pin = NEW_PIN;
    f.gateNew.upstreamPin = NEW_PIN;
    f.state.batch.upstreamPin = { from: GO_PIN, to: NEW_PIN };
    withMap(f, `${suite}\t${name}\t-\t-\tGo test removed at 16c25522e123, its baseline file left behind\n`);
    if (pass) {
      result = check(f);
      assert.equal(result.verdict, "PASS", result.reasons.join(" "));
      assert.deepEqual([result.counts.goportTests.removedByMap, result.counts.goportTests.removedIgnored], [1, 1]);
      assert.deepEqual(result.nameMapRemovedIgnored.map(r => r.name), [REF]);
    } else {
      stopped(f, new RegExp(`Name map line 1: ${suite} ${name.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")} is still in the new results`));
    }
  }
  f = goportFixture();
  delete f.results.suites.ts_scanner_lib["scan::a"];
  withMap(f, "ts_scanner_lib\tscan::a\t-\t-\tstage 5: goport's scanner replaces ts_scanner\n");
  result = check(f);
  assert.equal(result.verdict, "PASS", result.reasons.join(" "));
  assert.equal(result.nameMapRemoved.length, 1);
});

test("the history row and both verdicts bind the name map", () => {
  const f = goportFixture();
  const suite = f.results.suites.go_baselines;
  suite["b::uno"] = suite["b::one"]; delete suite["b::one"];
  f.state.batch.goportTests.nameMap = f.put("map.tsv", "c", "go_baselines\tb::one\tgo_baselines\tb::uno\tmoved\n");
  stopped(f, /Current history row does not match the batch source, hypothesis, and goport test, gate, name map and gate id map hashes/);
  f.state.batch.recoveryHistory.at(-1).nameMapSha256 = "c".repeat(64);
  stopped(f, /Verdict is not bound to this batch, source, and goport test, gate, name map and gate id map hashes/);
  rebind(f);
  assert.equal(check(f).verdict, "PASS");
});

test("goport results and gate come from the batch build inputs and Go pin", () => {
  for (const [change, pattern] of [
    [f => { f.results.source.commit = OTHER_INPUTS.slice(0, 9); }, /goportTests results come from commit fedcba987, whose crates tree, Cargo.toml or Cargo.lock differ/],
    [f => { f.results.source.tree = "c".repeat(40); }, /goportTests results come from commit/],
    [f => { f.results.source.commit = "abcabcabc"; }, /git has no crates tree, Cargo.toml and Cargo.lock for commit abcabcabc/],
    [f => { f.results.pin = "dc37b5249ab6"; }, /goportTests results are not at the batch Go pin/],
    [f => { delete f.results.pin; }, /goportTests results are not at the batch Go pin/],
    [f => { f.gateNew.commit = OTHER_INPUTS; }, /Gate manifest comes from commit fedcba98/],
    [f => { f.gateNew.upstreamPin = "dc37b5249ab6"; }, /Gate manifest is not at the batch Go pin/],
    [f => { delete f.state.batch.commit; }, /needs its commit/],
    [f => { f.results.suites.ts_goport_lib["api::t1"] = "FAILED"; }, /has status "FAILED"/],
  ]) { const f = goportFixture(); change(f); stopped(f, pattern); }
  // candidate.sh reuses evidence of an earlier commit with the same build inputs.
  const f = goportFixture();
  f.results.source.commit = SAME_INPUTS;
  f.gateNew.commit = SAME_INPUTS.slice(0, 9);
  assert.equal(check(f).verdict, "PASS");
});

test("a goport batch needs a hex Go pin, and a removal needs a change of the batch pins", () => {
  for (const [change, pattern] of [
    [f => { delete f.state.batch.upstreamPin; }, /A goport batch needs its Go pin: upstreamPin.to of 7 to 64 hex characters, not undefined/],
    [f => { f.state.batch.upstreamPin.to = null; }, /needs its Go pin/],
    [f => { f.state.batch.upstreamPin.to = "main"; }, /needs its Go pin: upstreamPin.to of 7 to 64 hex characters, not "main"/],
    [f => { f.state.batch.upstreamPin.to = "521689"; }, /needs its Go pin/],
    [f => { f.state.batch.upstreamPin.to = `${GO_PIN}x`; }, /needs its Go pin/],
    [f => { delete f.files["docs/typechecker-batches/batch-0.json"].value.upstreamPin; }, /Accepted batch batch-0 needs its Go pin/],
    [f => { f.files["docs/typechecker-batches/batch-0.json"].value.upstreamPin.to = "HEAD"; }, /Accepted batch batch-0 needs its Go pin/],
    [f => { f.files[BASELINE_PATH].value.pin = NEW_PIN; }, /The base goport results are not at the Go pin 52168999f3dc of accepted batch batch-0/],
    [f => { delete f.files[BASELINE_PATH].value.pin; }, /The base goport results are not at the Go pin/],
  ]) { const f = goportFixture(); change(f); stopped(f, pattern); }
  // An abbreviated pin names the same commit.
  let f = goportFixture();
  f.state.batch.upstreamPin.to = GO_PIN.slice(0, 7);
  assert.equal(check(f).verdict, "PASS");
  // A fake results pin cannot claim a pin change: the results must be at upstreamPin.to.
  f = goportFixture();
  delete f.results.suites.go_baselines["b::one"];
  f.results.pin = NEW_PIN;
  withMap(f, "go_baselines\tb::one\t-\t-\tfake pin bump\n");
  stopped(f, /goportTests results are not at the batch Go pin 52168999f3dc/);
  // Without a batch pin, the fake results pin is not trusted either.
  delete f.state.batch.upstreamPin;
  stopped(f, /A goport batch needs its Go pin/);
  // upstreamPin.from does not count: the base pin is the accepted batch's upstreamPin.to.
  f = goportFixture();
  delete f.results.suites.go_baselines["b::one"];
  f.state.batch.upstreamPin.from = NEW_PIN;
  withMap(f, "go_baselines\tb::one\t-\t-\tfake pin bump\n");
  stopped(f, /removes go_baselines b::one, but the Go pin did not change/);
});

test("goport batch needs bound runs, quality and bound verdicts", () => {
  for (const [change, pattern] of [
    [f => { delete f.state.batch.ordinaryQuery; }, /ordinaryQuery: bound run/],
    [f => { f.state.batch.latestHono.matchesOracle = false; }, /latestHono: bound run/],
    [f => { f.state.batch.latestHono.sourceFingerprint = RESULT; }, /latestHono: bound run/],
    [f => { f.files["measure/r40/manifest.json"].value.sourceFingerprint = RESULT; }, /names another source/],
    [f => { f.state.batch.qualityEvidence.sourceFingerprint = RESULT; }, /Quality needs/],
    [f => { f.files["quality.json"].value.clippyExit = 1; }, /Quality record/],
    [f => { f.files["quality.json"].value.tsGoportWarnings = 2; }, /ts_goport warning/],
    [f => { delete f.files["quality.json"].value.tsGoportWarnings; }, /ts_goport warning/],
    [f => { f.files["quality.json"].value.keptCrateWarnings = 1; }, /ts_goport warning/],
    [f => { f.state.batch.reviewer.verdict = "STOP"; }, /Missing independent PASS verdict/],
    [f => { f.state.batch.reviewer.sourceFingerprint = RESULT; }, /Verdict is not bound/],
    [f => { f.state.batch.auditor.batchId = "batch-0"; }, /Verdict is not bound/],
    [f => { f.state.batch.reviewer.agent = "writer"; }, /implementer/],
    [f => { f.state.batch.recoveryHistory.at(-1).sourceFingerprint = RESULT; }, /Current history row does not match/],
    [f => { f.state.batch.recoveryHistory.at(-1).goportTestsSha256 = RESULT; }, /Current history row does not match .* goport test, gate, name map and gate id map hashes/],
    [f => { delete f.state.batch.recoveryHistory.at(-1).gateSha256; }, /Current history row does not match/],
    [f => { f.state.batch.auditor.gateSha256 = RESULT; }, /Verdict is not bound to this batch, source, and goport test, gate, name map and gate id map hashes/],
    [f => { delete f.state.batch.reviewer.goportTestsSha256; }, /Verdict is not bound/],
    [f => { f.state.batch.gateCompare.baseSha256 = RESULT; }, /gateCompare base must be/],
    [f => { f.state.batch.rosterCarryForward = { fromRevision: 6 }; }, /no roster carry-forward/],
  ]) { const f = goportFixture(); change(f); stopped(f, pattern); }
});

test("goport mode needs Theo's standing rule that cites a saved approval note", () => {
  for (const [change, pattern] of [
    [f => { f.state.acceptanceRuleChanges = []; }, /standing goport-protected-set rule/],
    [f => { f.state.acceptanceRuleChanges[0].batchId = "batch-1"; }, /standing goport-protected-set rule/],
    [f => { f.state.acceptanceRuleChanges[0].standing = false; }, /standing goport-protected-set rule/],
    [f => { delete f.state.acceptanceRuleChanges[0].approvedBy; }, /standing goport-protected-set rule/],
    [f => { f.state.acceptanceRuleChanges[0].date = "28 Sep 2026"; }, /an ISO date \(2026-09-28 or 2026-09-28T20:57:30Z\)/],
    [f => { delete f.state.acceptanceRuleChanges[0].baseline; }, /baseline path and SHA-256/],
    [f => { f.state.acceptanceRuleChanges[0].protectedSet = "roster"; }, /protectedSet "goport"/],
    [f => { delete f.state[NOTE]; }, /saved approval note/],
    [f => { f.state.batch.protectedSet = "goport2"; }, /Unknown protectedSet/],
  ]) { const f = goportFixture(); change(f); stopped(f, pattern); }
  // A plain ISO date is enough.
  const f = goportFixture();
  f.state.acceptanceRuleChanges[0].date = "2026-09-28";
  assert.equal(check(f).verdict, "PASS");
});

test("goport unbound history rows come from the standing rule list, never the current row", () => {
  const f = goportFixture();
  Object.assign(f.state.batch.recoveryHistory[5], { sourceFingerprint: null, fullResultSha256: null });
  stopped(f, /Invalid or reset recovery history. A goport batch lists unbound rows in goport-protected-set unboundRevisions/);
  f.state.acceptanceRuleChanges[0].unboundRevisions = [6, 7];
  assert.equal(check(f).verdict, "PASS");
  f.state.batch.recoveryHistory[6].sourceFingerprint = null;
  stopped(f, /Invalid or reset recovery history/);
});

// Oracle answer sets and rebase runs (reviewer ruling 10, r139-diag/reviewer-ruling.md). GOLDEN_A and GOLDEN_B
// are the oracles of GO_PIN and NEW_PIN; BASE_TSGO is the tsgo of the base batch's gate (batch-0).
const [GOLDEN_A, GOLDEN_B, BASE_TSGO, API_TOOL] = ["a1".repeat(32), "b2".repeat(32), "b".repeat(64), "c3".repeat(32)];

// An answer set file (goport-oracle-answers/1) in dir, named in the fixture by its absolute path: each request key
// gets its Go answers, each from one Go golden at the set's oracle. The answers are JSON objects with one key, so
// JSON.stringify is canon().
function answerSet(f, dir, name, requests, { kind = "lsp", oracle = GOLDEN_A, pin = GO_PIN } = {}) {
  const doc = { format: "goport-oracle-answers/1", kind, pin, oracleSha256: oracle, goldenSha12: oracle.slice(0, 12), requests: {} };
  for (const [key, answers] of Object.entries(requests)) {
    const battery = key.slice(0, key.indexOf("/")), traceName = key.slice(key.indexOf("/") + 1, key.lastIndexOf("#"));
    doc.requests[key] = { method: "m", multiset: [], answers: answers.map((answer, n) => ({ answer,
      sha256: createHash("sha256").update(JSON.stringify(answer)).digest("hex"),
      sources: [`target/rerun/r${n + 1}/golden/${oracle.slice(0, 12)}/${battery}/${traceName}.golden.jsonl.gz`] })) };
  }
  const bytes = gzipSync(JSON.stringify(doc)), path = join(dir, name);
  writeFileSync(path, bytes);
  const ref = { kind, path, sha256: createHash("sha256").update(bytes).digest("hex"), pin };
  f.put(path, ref.sha256, null);
  return ref;
}

// goport answers {items: [answer]} to the LSP request b1/t1#1 in the new run.
function lspAnswer(f, answer) {
  f.responses = { "lsp/lsp-r132": { "b1/t1": [{ i: 1, method: "m", status: "ok", response: { result: { items: [answer] } } }] } };
}

// batch-0 keeps b1/t1#1 flaky with the answer set set (condition 5 after its acceptance): the base and the new
// LSP runs have it flaky_oracle, the new run at the oracle golden, and goport answers "b".
function withBaseAnswers(f, set, { golden = GOLDEN_A } = {}) {
  f.files["docs/typechecker-batches/batch-0.json"].value.oracleAnswers = [set];
  f.state.batch.protectedBase.oracleAnswers = [set];
  f.state.batch.oracleAnswers = [set];
  f.oracle["lsp/lsp-r131"][0].events[1].class = "flaky_oracle";
  f.oracle["lsp/lsp-r132"][0].events[1].class = "flaky_oracle";
  f.lspGoldens = [golden];
  lspAnswer(f, "b");
  rebind(f);
  pinOracles(f);
}

test("answer sets keep a base flake request protected at their pin (ruling 10 condition 5)", () => inTemp(dir => {
  const f = goportFixture(), set = answerSet(f, dir, "base.json.gz", { "b1/t1#1": [{ items: ["a"] }, { items: ["b"] }] });
  withBaseAnswers(f, set);
  const result = check(f);
  assert.equal(result.verdict, "PASS", result.reasons.join(" "));
  assert.deepEqual([result.oracles.lsp.retainedByAnswers, result.oracles.lsp.retained], [1, 3]);
  // An answer outside the set is lost.
  lspAnswer(f, "c");
  pinOracles(f);
  stopped(f, /languageServerOracle: 1 lost protected base requests/);
  // At another oracle only (a later pin bump) the set does not apply and the request is protected like a same request.
  const g = goportFixture();
  withBaseAnswers(g, answerSet(g, dir, "base.json.gz", { "b1/t1#1": [{ items: ["a"] }, { items: ["b"] }] }), { golden: GOLDEN_B });
  stopped(g, /languageServerOracle: 1 lost protected base requests/);
  const saved = g.files["languageServerOracle-compare.json"].value;
  assert.deepEqual([saved.answers[0].notApplied, saved.lostFirst[0].carried], [1, true]);
  // protectedBase, batch.oracleAnswers and the pinned compares must name the base batch's answer sets.
  for (const [change, pattern] of [
    [h => { h.state.batch.protectedBase.oracleAnswers = []; }, /batch.protectedBase differs from the base that accepted batch batch-0 gives/],
    [h => { h.state.batch.oracleAnswers = []; rebind(h); }, /batch.oracleAnswers must keep the answer sets of the base at the batch pin/],
    [h => { h.files["languageServerOracle-compare.json"].value.answers = []; }, /languageServerOracle.output must use the answer sets/],
    [h => { h.state.batch.reviewer.oracleAnswersSha256 = []; }, /Verdict is not bound to this batch, source, and goport test/]]) {
    const h = goportFixture();
    withBaseAnswers(h, answerSet(h, dir, "base.json.gz", { "b1/t1#1": [{ items: ["a"] }, { items: ["b"] }] }));
    change(h);
    stopped(h, pattern);
  }
}));

test("an answer set with another sha256 or a bad answer is refused", () => inTemp(dir => {
  const f = goportFixture(), set = answerSet(f, dir, "base.json.gz", { "b1/t1#1": [{ items: ["a"] }, { items: ["b"] }] });
  withBaseAnswers(f, set);
  f.files["docs/typechecker-batches/batch-0.json"].value.oracleAnswers = [{ ...set, sha256: "e".repeat(64) }];
  stopped(f, /Evidence hash mismatch/);
  // oracle-compare.py itself refuses it (exit 2), and a set whose answer sha256 is not that of its text.
  const run = sha => f.tools.oracleCompare([resolve(ROOT, "lsp/lsp-r131")], resolve(ROOT, "lsp/lsp-r132"),
    { kind: "lsp", answers: [{ ...set, sha256: sha }] });
  assert.throws(() => run("e".repeat(64)), /oracle-compare.py failed \(exit 2\): .*does not have the sha256 e{64}/);
  const doc = JSON.parse(gunzipSync(readFileSync(set.path)));
  doc.requests["b1/t1#1"].answers[0].sha256 = "0".repeat(64);
  const bytes = gzipSync(JSON.stringify(doc));
  writeFileSync(set.path, bytes);
  assert.throws(() => run(createHash("sha256").update(bytes).digest("hex")), /exit 2\): .*has an answer whose sha256 is not that of its canon\(\) text/);
}));

test("only a pin-bump batch adds answer sets, at the batch pin", () => inTemp(dir => {
  const added = (f, pin) => {
    f.state.batch.oracleAnswers = [answerSet(f, dir, `${pin}.json.gz`, { "b1/t1#1": [{ items: ["a"] }, { items: ["b"] }] },
      { pin, oracle: pin === GO_PIN ? GOLDEN_A : GOLDEN_B })];
    rebind(f);
    pinOracles(f);
  };
  let f = goportFixture();
  added(f, GO_PIN);
  stopped(f, /batch.oracleAnswers .* is not an answer set of the base: only a pin-bump batch can add one/);
  f = goportFixture();
  f.results.pin = f.gateNew.upstreamPin = NEW_PIN;
  f.state.batch.upstreamPin = { from: GO_PIN, to: NEW_PIN };
  added(f, GO_PIN);
  stopped(f, /batch.oracleAnswers .* is at the Go pin 52168999f3dc, not the batch pin 16c25522e123/);
  added(f, NEW_PIN);
  const result = check(f);
  assert.equal(result.verdict, "PASS", result.reasons.join(" "));
}));

// A pin bump with batch.oracleRebase: batch-0's bins (BASE_TSGO) ran again at NEW_PIN (GOLDEN_B) as lsp/lsp-r131-atB
// (and lsp/lsp-r131-atB2 when two) and api/api-r131-atB. At NEW_PIN Go's answer to b1/t1#1 varies: flaky_oracle in the
// rebase runs and in the new run, so the rebase base does not protect it, while the base batch's own run (GOLDEN_A) does.
// lsp: the classes of b1/t1 in each LSP rebase run.
function withRebase(f, { lsp = [["same", "flaky_oracle", "not_run"]], knownDiffs = [], answers = [] } = {}) {
  f.results.pin = f.gateNew.upstreamPin = NEW_PIN;
  f.state.batch.upstreamPin = { from: GO_PIN, to: NEW_PIN, oracleSha256: GOLDEN_B };
  f.files["gate/r131/manifest.json"].value.binaries = { tsgo: { path: "bins-r131/tsgo", sha256: BASE_TSGO } };
  f.oracle["lsp/lsp-r132"][0].events[1].class = "flaky_oracle";
  f.lspGoldens = [GOLDEN_B];
  f.files["api/api-r132/manifest.json"].value.batteries.qc.oracleSha = GOLDEN_B;
  const lspRuns = lsp.map((classes, i) => {
    const dir = `lsp/lsp-r131-atB${i ? i + 1 : ""}`;
    f.oracle[dir] = [trace("b1", "t1", classes), trace("b1", "t2", ["same", "oracle_error_same"]), trace("b2", "t3", ["diff"])];
    f.put(`${dir}/summary.json`, "0", { ...lspSummary(f.oracle[dir], [GOLDEN_B]), goport: [{ path: "bins-r131/tsgo", sha256: BASE_TSGO }] });
    return { label: basename(dir), dir };
  });
  f.oracle["api/api-r131-atB"] = [trace("qc", "a1", ["same", "goport_error"])];
  f.put("api/api-r131-atB/manifest.json", "0", { batteries: { qc: { goportSha: BASE_TSGO, oracleSha: GOLDEN_B, traces: 1, wire: 3, scriptSha: API_TOOL } } });
  f.state.batch.oracleRebase = {
    lsp: { runs: lspRuns, binsSha256: BASE_TSGO, oracleSha256: GOLDEN_B },
    api: { runs: [{ label: "api-r131-atB", dir: "api/api-r131-atB" }], binsSha256: BASE_TSGO, oracleSha256: GOLDEN_B, wire: 3, toolSha256: API_TOOL,
      knownDiffs } };
  f.state.batch.oracleAnswers = answers;
  resultsShas(f);
}

// Sets the resultsSha256 of each rebase run from oracle-compare.py --identity, then rebinds and pins the compares.
function resultsShas(f) {
  for (const kind of ["lsp", "api"]) {
    const { runs } = f.state.batch.oracleRebase[kind];
    const record = f.state.batch[kind === "lsp" ? "languageServerOracle" : "apiOracle"];
    const out = f.tools.oracleCompare(runs.map(run => resolve(ROOT, run.dir)), resolve(ROOT, record.dir), { kind, identity: true });
    (out.bases ?? [out.base]).forEach((head, i) => { runs[i].resultsSha256 = head.resultsSha256; });
  }
  rebind(f);
  pinOracles(f);
}

test("the oracle base of a pin bump is the base bins measured again at the new pin (ruling 10 conditions 1 and 2)", () => {
  // Without oracleRebase the Go-side flake of b1/t1#1 is a loss against batch-0's run at the old pin.
  let f = goportFixture();
  f.results.pin = f.gateNew.upstreamPin = NEW_PIN;
  f.state.batch.upstreamPin = { from: GO_PIN, to: NEW_PIN };
  f.oracle["lsp/lsp-r132"][0].events[1].class = "flaky_oracle";
  pinOracles(f);
  stopped(f, /languageServerOracle: 1 lost protected base requests/);
  f = goportFixture();
  withRebase(f);
  let result = check(f);
  assert.equal(result.verdict, "PASS", result.reasons.join(" "));
  assert.deepEqual(result.oracles.lsp, { label: "lsp-r132", base: "lsp-r131-atB", traces: 3, requests: 6, retained: 3, recovered: 1,
    lost: 0, unrun: 0, absent: 0, newRequests: 0 });
  assert.equal(f.state.batch.languageServerOracle.output.path, "languageServerOracle-compare.json");
  // The runs, their tsgo and oracle, and their hashes are bound; each mismatch stops.
  const oracleRebaseSha = () => createHash("sha256").update(canonicalJson(f.state.batch.oracleRebase)).digest("hex");
  assert.equal(f.state.batch.reviewer.oracleRebaseSha256, oracleRebaseSha());
  for (const [change, pattern] of [
    [g => { g.state.batch.upstreamPin = { from: GO_PIN, to: GO_PIN, oracleSha256: GOLDEN_B }; g.results.pin = g.gateNew.upstreamPin = GO_PIN; rebind(g); },
      /batch.oracleRebase is only for a pin-bump batch; base batch batch-0 is at the batch pin 52168999f3dc/],
    [g => { g.state.batch.oracleRebase.lsp.binsSha256 = TSGO; rebind(g); }, /batch.oracleRebase.lsp.binsSha256 must be the tsgo sha256 of the base batch's gate manifest/],
    [g => { g.state.batch.oracleRebase.api.oracleSha256 = GOLDEN_A; rebind(g); }, /batch.oracleRebase.api.oracleSha256 must be the oracle of the batch pin/],
    [g => { g.files["lsp/lsp-r131-atB/summary.json"].value.goport[0].sha256 = TSGO; resultsShas(g); }, /rebase run lsp-r131-atB ran tsgo 0{64}, not the base batch's b{64}/],
    [g => { g.files["api/api-r131-atB/manifest.json"].value.batteries.qc.oracleSha = GOLDEN_A; resultsShas(g); }, /rebase run api-r131-atB used the oracle a1a1.*, not the batch pin's/],
    [g => { g.state.batch.oracleRebase.lsp.runs[0].resultsSha256 = "e".repeat(64); rebind(g); }, /rebase run lsp-r131-atB \(lsp\/lsp-r131-atB\) has resultsSha256 /],
    [g => { g.state.batch.oracleRebase.lsp.knownDiffs = [{ key: "b1/t1#1", reason: "r" }]; rebind(g); }, /knownDiffs must list each \{key, reason\} once; the LSP has none/],
    // bump C ruling 1 item 1: the API rebase runs are --wire 3 (or, at a protocol 5 pin, --wire 4) runs of one API
    // tool, and the entry says so.
    [g => { delete g.state.batch.oracleRebase.api.wire; rebind(g); }, /batch.oracleRebase.api needs "wire": 3 or 4 and toolSha256, the api_oracle.py sha256 of its runs/],
    [g => { g.state.batch.oracleRebase.api.toolSha256 = "c3"; rebind(g); }, /batch.oracleRebase.api needs "wire": 3 or 4 and toolSha256/],
    [g => { g.state.batch.oracleRebase.api.wire = 5; g.files["api/api-r131-atB/manifest.json"].value.batteries.qc.wire = 5; resultsShas(g); },
      /batch.oracleRebase.api needs "wire": 3 or 4 and toolSha256/],
    [g => { g.state.batch.oracleRebase.api.wire = "4"; g.files["api/api-r131-atB/manifest.json"].value.batteries.qc.wire = "4"; resultsShas(g); },
      /batch.oracleRebase.api needs "wire": 3 or 4 and toolSha256/],
    [g => { g.state.batch.oracleRebase.api.wire = 4; resultsShas(g); },
      /rebase run api-r131-atB \(api\/api-r131-atB\) has batteries without "wire": 4 or api_oracle.py c3c3.*: qc/],
    [g => { delete g.files["api/api-r131-atB/manifest.json"].value.batteries.qc.wire; resultsShas(g); },
      /rebase run api-r131-atB \(api\/api-r131-atB\) has batteries without "wire": 3 or api_oracle.py c3c3.*: qc/],
    [g => { g.files["api/api-r131-atB/manifest.json"].value.batteries.qc.scriptSha = "d".repeat(64); resultsShas(g); },
      /rebase run api-r131-atB .* has batteries without "wire": 3 or api_oracle.py c3c3.*: qc/],
    [g => { g.state.batch.auditor.oracleRebaseSha256 = null; }, /Verdict is not bound to this batch/],
    [g => { g.state.batch.languageServerOracle.bases = g.state.batch.languageServerOracle.bases.slice(0, 0); },
      /languageServerOracle needs its results dir and the base run lsp\/lsp-r131-atB/]]) {
    const g = goportFixture();
    withRebase(g);
    change(g);
    stopped(g, pattern);
  }
  // Every rebase run counts: b1/t1#1 is same in a second run, so the new run's flaky_oracle is a loss.
  f = goportFixture();
  withRebase(f, { lsp: [["same", "flaky_oracle", "not_run"], ["same", "same", "not_run"]] });
  result = stopped(f, /languageServerOracle: 1 lost protected base requests/);
  const saved = f.files["languageServerOracle-compare.json"].value;
  assert.deepEqual([saved.bases.map(head => head.label), saved.lostFirst[0].event], [["lsp-r131-atB", "lsp-r131-atB2"], "1"]);
  // b1/t1#0 is same in the first run only: still protected, and retained.
  withRebase(f, { lsp: [["same", "flaky_oracle", "not_run"], ["diff", "flaky_oracle", "not_run"]] });
  result = check(f);
  assert.equal(result.verdict, "PASS", result.reasons.join(" "));
  assert.deepEqual([result.oracles.lsp.base, result.oracles.lsp.retained], ["lsp-r131-atB,lsp-r131-atB2", 3]);
  // bump D: protocol 4 base bins at a protocol 5 pin use --wire 4. The entry and every manifest battery say 4.
  f = goportFixture();
  withRebase(f);
  f.state.batch.oracleRebase.api.wire = 4;
  f.files["api/api-r131-atB/manifest.json"].value.batteries.qc.wire = 4;
  resultsShas(f);
  result = check(f);
  assert.equal(result.verdict, "PASS", result.reasons.join(" "));
  f.files["api/api-r131-atB/manifest.json"].value.batteries.qc.wire = 3;
  resultsShas(f);
  stopped(f, /rebase run api-r131-atB \(api\/api-r131-atB\) has batteries without "wire": 4 or api_oracle.py c3c3.*: qc/);
});

test("with oracleRebase the new runs must match Go at the batch pin (ruling 10 condition 3)", () => inTemp(dir => {
  // LSP: an oracle_error_diff of a new request is a parity problem, though it is no loss.
  let f = goportFixture();
  withRebase(f);
  f.oracle["lsp/lsp-r132"][2].events[0].class = "oracle_error_diff";
  resultsShas(f);
  stopped(f, /languageServerOracle: the new run does not match Go at the batch pin .*b2\/t3#0 oracle_error_diff: oracle_error_diff in the new run/);
  // API: a diff needs a known diff with a reason; a known diff must be a diff in the new run; goport_error is never allowed.
  f = goportFixture();
  withRebase(f);
  f.oracle["api/api-r132"][0].events[1].class = "diff";
  resultsShas(f);
  stopped(f, /apiOracle: the new run does not match Go at the batch pin .*qc\/a1#1 diff: diff in the new run, not a known diff/);
  withRebase(f, { knownDiffs: [{ key: "qc/a1#1", reason: "Go-side: the oracle answers another order" }] });
  let result = check(f);
  assert.equal(result.verdict, "PASS", result.reasons.join(" "));
  f.oracle["api/api-r132"][0].events[1].class = "same";
  resultsShas(f);
  stopped(f, /apiOracle: the new run does not match Go at the batch pin .*qc\/a1#1 same: known diff is not a diff in the new run/);
  f.oracle["api/api-r132"][0].events[1].class = "goport_error";
  withRebase(f, { knownDiffs: [{ key: "qc/a1#1", reason: "r" }] });
  stopped(f, /qc\/a1#1 goport_error: goport_error in the new run/);
  // A request of the batch's answer set: goport's answer must be in the set, whatever its class.
  f = goportFixture();
  const set = answerSet(f, dir, "b.json.gz", { "b1/t1#1": [{ items: ["a"] }, { items: ["b"] }] }, { oracle: GOLDEN_B, pin: NEW_PIN });
  lspAnswer(f, "b");
  withRebase(f, { answers: [set] });
  result = check(f);
  assert.equal(result.verdict, "PASS", result.reasons.join(" "));
  assert.equal(result.oracles.lsp.retainedByAnswers, 1);
  assert.deepEqual(f.state.batch.reviewer.oracleAnswersSha256, [set.sha256]);
  lspAnswer(f, "c");
  resultsShas(f);
  stopped(f, /languageServerOracle: 1 lost protected base requests.*b1\/t1#1 flaky_oracle: goport's answer \(sha256 [0-9a-f]{12}\) is not in the answer set/);
  // A set of the batch pin at another oracle would apply to nothing.
  f = goportFixture();
  lspAnswer(f, "b");
  withRebase(f, { answers: [answerSet(f, dir, "a.json.gz", { "b1/t1#1": [{ items: ["a"] }, { items: ["b"] }] }, { oracle: GOLDEN_A, pin: NEW_PIN })] });
  stopped(f, /languageServerOracle: an answer set of the batch pin 16c25522e123 is not at its oracle/);
  // The pinned compare must be a parity compare.
  f = goportFixture();
  withRebase(f);
  delete f.files["apiOracle-compare.json"].value.parity;
  stopped(f, /apiOracle.output must be a compare with --parity, the 0 known diffs and --identity/);
}));

test("oracle-compare.py without options prints the output of before the options", () => {
  const f = goportFixture();
  const out = f.tools.oracleCompare([resolve(ROOT, "lsp/lsp-r131")], resolve(ROOT, "lsp/lsp-r132"), {});
  assert.deepEqual([Object.keys(out), Object.keys(out.base), Object.keys(out.total)],
    [["base", "new", "protectedClasses", "total", "batteries", "lostFirst"], ["dir", "label", "traces", "requests"],
      ["retained", "recovered", "lost", "unrun", "absent", "newRequests"]]);
});
