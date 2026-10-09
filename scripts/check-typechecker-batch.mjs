import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdtempSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { gunzipSync } from "node:zlib";
import { loadState, verifyAppendOnly } from "./state.mjs";

// This checks saved evidence. It does not run Cargo or authenticate agent identities.
export const CHECKPOINT_SHA256 = "60a372581586cb3c8d0046ba6e4ab0af65b515267485a0a01bdfe90695f7a538";
export const BASELINE_SHA256 = "f562cd3ca338de7203c6ae12693dfc4a726a6478b3da7f1393374c36194bdcba";
// Theo's opt-in crate rule (2026-09-25) pins the inherited losses to the R96 full result.
export const INHERITED_PIN = { sha256: "e7838ed863c42bc2271981c680d2fc46bb6a98f3171c8554b2d040ca9277d6b7", originalAccepted: 290, laterPasses: 15 };
export const CARRY_FORWARD_RULE = "goport-only-roster-carry-forward";
// Theo's standing rule (2026-09-28) that makes goport's own tests the protected set for a
// batch with protectedSet "goport". See checkGoport.
export const GOPORT_RULE = "goport-protected-set";
// The first goport test baseline (docs/goport-protected/tests-r131.json.gz, the sha256 of the
// stored gzip bytes). The rule must name it.
export const GOPORT_BASELINE_SHA256 = "d1b90114690033ea0d3450182b7340c7f87df27d7e3c45af07192506a2476b7a";
// Unit test suites of the kept legacy crates. A name map may remove their tests when goport's Go
// port replaces a crate (legacy removal stages 5 and 6); other removals need a Go pin change.
const KEPT_CRATE_SUITE = /^ts_(scanner|ast|diagnostics|path|core|jsnum)_lib$/;
// The only suite whose "ignored" names a removal line may keep (stale Go reference files; compare-tests.py).
const STALE_REFERENCE_SUITE = "go_baselines_reference";
// The wires an API rebase run may use: api_oracle.py check --wire 3 (protocol 3 base bins at a protocol 4 pin,
// bump C reviewer ruling 1 item 1) and --wire 4 (protocol 4 base bins at a protocol 5 pin, bump D; ruling 3 item 6).
const API_REBASE_WIRES = [3, 4];
// R132 is the last revision under the legacy cargo roster (rule goport-protected-set). It was opened
// in batch port-18 under the legacy rules before stage 1 merged. A later revision needs protectedSet
// "goport": the legacy check, and with it the roster carry-forward, is only for revisions up to this one.
export const LAST_LEGACY_REVISION = 132;
// A Go pin (commit hash) as upstreamPin.to names it: 7 to 64 hex characters.
const GO_PIN = /^[0-9a-f]{7,64}$/i;
const ROOT = fileURLToPath(new URL("../", import.meta.url));
const HASH = /^[a-f0-9]{64}$/;
const STAGES = ["checker", "compiler", "fixture"];
const SCOPE = "Original 6055 and later 6330 PASS names only. Later added passes and corpus parity need independent review.";
const GOPORT_SCOPE = "goport protected set: every base ok test name and every base gate item, plus bound runs, LSP and API oracles "
  + "and quality. Name maps, gate id maps, oracle rebase runs and known diffs, oracle answer sets, gate noise and allow-list "
  + "conditions need independent review.";
const TEST_STATUSES = new Set(["ok", "failed", "ignored", "unrun"]);

function requireValue(condition, message) {
  if (!condition) throw new Error(message);
}

function text(value) {
  return typeof value === "string" && value.trim().length > 0;
}

// An ISO date (2026-09-28) or date-time (2026-09-28T20:57:30Z).
function isoDate(value) {
  return text(value) && /^\d{4}-\d{2}-\d{2}(T|$)/.test(value) && Number.isFinite(Date.parse(value));
}

// Two abbreviated or full git hashes (commits, Go pins) name the same object.
function sameHash(a, b) {
  const hex = /^[0-9a-f]{7,64}$/;
  if (typeof a !== "string" || typeof b !== "string") return false;
  const [x, y] = [a.toLowerCase(), b.toLowerCase()];
  return hex.test(x) && hex.test(y) && (x.startsWith(y) || y.startsWith(x));
}

function samePath(a, b) {
  return text(a) && text(b) && resolve(ROOT, a) === resolve(ROOT, b);
}

// A count, or a list whose length is the count.
function size(value, label) {
  const result = Array.isArray(value) ? value.length : value;
  requireValue(Number.isInteger(result) && result >= 0, `${label} needs a count or a list.`);
  return result;
}

function key(row) {
  requireValue(text(row?.harness) && text(row?.name), "Missing exact harness or test name.");
  return JSON.stringify([row.harness, row.name]);
}

function rowsByKey(rows, label) {
  requireValue(Array.isArray(rows), `${label}: missing exact ledger.`);
  const result = new Map();
  for (const row of rows) {
    const id = key(row);
    requireValue(!result.has(id), `${label}: duplicate ${row.harness}::${row.name}.`);
    result.set(id, row);
  }
  return result;
}

function counts(rows) {
  return {
    tests: rows.length,
    PASS: rows.filter(row => row.status === "PASS").length,
    FAIL: rows.filter(row => row.status === "FAIL").length,
    harnesses: new Set(rows.map(row => row.harness)).size,
  };
}

function matchCounts(actual, claimed, label) {
  for (const field of ["tests", "PASS", "FAIL", "harnesses"]) {
    requireValue(actual[field] === claimed?.[field], `${label}: ${field} count mismatch.`);
  }
}

// Reads an evidence file {path, sha256} relative to the repository root. The hash must match
// unless the caller passes pinned false (for records the state saves without a hash, such as
// the quality record). A path that ends in .gz is gzip: the hash is of the stored bytes. json
// false returns the text; gunzip false returns the stored bytes (to check only the hash of a big file).
export function readEvidenceFile(reference, { pinned = true, json = true, gunzip = true } = {}) {
  requireValue(text(reference?.path) && (!pinned || HASH.test(reference?.sha256)), "Missing evidence path or SHA-256.");
  let bytes;
  try {
    bytes = readFileSync(resolve(ROOT, reference.path));
  } catch {
    throw new Error(`Missing evidence: ${reference.path}. Do not regenerate a baseline.`);
  }
  requireValue(!pinned || createHash("sha256").update(bytes).digest("hex") === reference.sha256,
    `Evidence hash mismatch: ${reference.path}.`);
  if (!gunzip) return bytes;
  if (reference.path.endsWith(".gz")) {
    try {
      bytes = gunzipSync(bytes);
    } catch {
      throw new Error(`Invalid gzip evidence: ${reference.path}.`);
    }
  }
  if (!json) return bytes.toString("utf8");
  try {
    return JSON.parse(bytes.toString("utf8"));
  } catch {
    throw new Error(`Invalid evidence JSON: ${reference.path}.`);
  }
}

// Theo-approved rule changes live in state.acceptanceRuleChanges. Each entry is bound
// to one batch id. Rules that do not name this batch have no effect. Only the standing
// rules (standingRule) use batchId "*", so "*" never matches here.
function approvedRule(state, id) {
  const rules = Array.isArray(state.acceptanceRuleChanges) ? state.acceptanceRuleChanges : [];
  const rule = rules.find(item => item?.id === id && item.batchId === state.batch?.id && item.batchId !== "*");
  if (!rule) return null;
  requireValue(rule.approvedBy === "Theo" && text(rule.instruction) && text(rule.date) && Number.isFinite(Date.parse(rule.date)),
    `Rule ${id} needs Theo's saved approval, instruction and date.`);
  return rule;
}

// A standing rule applies to every batch that asks for it: batchId "*", standing true,
// approvedBy Theo, instruction, scope and an ISO date. Used by the roster carry-forward
// and the goport protected set.
function standingRule(state, id, label) {
  const rules = Array.isArray(state.acceptanceRuleChanges) ? state.acceptanceRuleChanges : [];
  const rule = rules.find(item => item?.id === id && item.batchId === "*");
  requireValue(rule?.standing === true && rule.approvedBy === "Theo" && text(rule.instruction) && text(rule.scope) && isoDate(rule.date),
    `${label} needs Theo's standing ${id} rule with batchId "*", standing true, approvedBy Theo, instruction, scope and `
    + "an ISO date (2026-09-28 or 2026-09-28T20:57:30Z).");
  return rule;
}

// batch.protectedSet selects the protected set. Absent means the legacy cargo roster.
// "goport" needs the standing goport-protected-set rule, which pins the first goport
// baseline, cites Theo's saved approval note and may list unbound history revisions.
function goportRule(state) {
  const set = state?.batch?.protectedSet;
  if (set === undefined) return null;
  requireValue(set === "goport", `Unknown protectedSet ${JSON.stringify(set)}. Use "goport" or leave it out.`);
  const rule = standingRule(state, GOPORT_RULE, "The goport protected set");
  requireValue(rule.protectedSet === "goport" && text(rule.baseline?.path) && HASH.test(rule.baseline?.sha256),
    `${GOPORT_RULE} needs protectedSet "goport" and a baseline path and SHA-256.`);
  requireValue(rule.baseline.sha256 === GOPORT_BASELINE_SHA256,
    `${GOPORT_RULE} baseline must be the pinned first goport baseline (sha256 ${GOPORT_BASELINE_SHA256}).`);
  requireValue(text(rule.approvalNote) && state[rule.approvalNote] != null, `${GOPORT_RULE} must cite a saved approval note.`);
  requireValue(rule.unboundRevisions === undefined || (Array.isArray(rule.unboundRevisions) && rule.unboundRevisions.every(Number.isInteger)),
    `${GOPORT_RULE} unboundRevisions must be a list of revision numbers.`);
  return rule;
}

// The canonical JSON text of a value: object keys sorted, no spaces (Python json.dumps(value,
// sort_keys=True, separators=(",", ":"), ensure_ascii=False)). oracleRebaseSha256 is its sha256.
export function canonicalJson(value) {
  if (Array.isArray(value)) return `[${value.map(canonicalJson).join(",")}]`;
  if (value !== null && typeof value === "object") {
    return `{${Object.keys(value).sort().map(name => `${JSON.stringify(name)}:${canonicalJson(value[name])}`).join(",")}}`;
  }
  return JSON.stringify(value);
}

// The evidence hashes of the batch oracles that the history row and both verdicts carry:
// oracleRebaseSha256 (the sha256 of the canonical JSON of batch.oracleRebase, or null) and
// oracleAnswersSha256 (the sorted sha256 values of batch.oracleAnswers).
function oracleHashes(batch) {
  return { oracleRebaseSha256: batch.oracleRebase == null ? null : createHash("sha256").update(canonicalJson(batch.oracleRebase)).digest("hex"),
    oracleAnswersSha256: (Array.isArray(batch.oracleAnswers) ? batch.oracleAnswers : []).map(ref => ref?.sha256).sort() };
}

// goport is the goport-protected-set rule for a goport batch, else null. A goport batch has
// no full result: its current history row and verdicts carry goportTestsSha256, gateSha256,
// nameMapSha256 (the goportTests name map sha256), gateIdMapSha256 (the batch.gateIdMap
// sha256), oracleRebaseSha256 and oracleAnswersSha256 (oracleHashes); a map or rebase sha256 is
// absent or null when the batch has none, and the answer list absent or empty without answer sets.
function validateBatch(state, goport = null) {
  requireValue(state?.schemaVersion === 1, "Unsupported state schema.");
  const continuation = state.phase === "recovery-continuation";
  requireValue(state.phase === "initial-recovery" || continuation, "Unsupported recovery phase.");
  requireValue(state.status === "ready", "State is not ready. Paused or missing state means STOP.");
  requireValue(state.decision === "REVIEW" || state.decision === "PASS", "Ready state cannot keep a STOP or missing decision.");
  requireValue(text(state.goalAuthorization) || (state.goalAuthorization !== null && typeof state.goalAuthorization === "object"
    && !Array.isArray(state.goalAuthorization) && Object.keys(state.goalAuthorization).length > 0), "Missing goal authorization.");
  if (continuation) {
    const authorization = state.continuationAuthorization;
    requireValue(authorization?.authorized === true && text(authorization.instruction) && text(authorization.scope) && isoDate(authorization.date),
      "Continuation requires explicit saved authorization, instruction, scope, and a valid date.");
  }
  requireValue(HASH.test(state.preservedCandidateSourceFingerprint), "Missing preserved candidate fingerprint.");
  const batch = state.batch;
  requireValue(batch && text(batch.id) && text(batch.implementer) && text(batch.hypothesis), "Missing authorized batch, implementer, or hypothesis.");
  requireValue(HASH.test(batch.sourceFingerprint), "Missing batch source fingerprint.");
  if (!goport) requireValue(text(batch.fullResult?.path) && HASH.test(batch.fullResult?.sha256), "Missing completed full result.");
  const history = batch.recoveryHistory;
  requireValue(Array.isArray(history) && (continuation ? history.length >= 5 : history.length >= 1 && history.length <= 4),
    continuation ? "Continuation history must retain all four initial revisions and each later measured revision."
      : "Recovery history must contain 1 to 4 measured revisions.");
  requireValue(batch.recoveryRevision === history.length, "Recovery revision must equal the retained history length.");
  if (!continuation) {
    requireValue(batch.maxRecoveryRevisions === undefined || batch.maxRecoveryRevisions === 4, "The recovery limit is fixed at 4.");
  }
  const unbound = approvedRule(state, "unbound-history-rows");
  const unboundRevisions = new Set([...(Array.isArray(unbound?.revisions) ? unbound.revisions : []), ...(goport?.unboundRevisions ?? [])]);
  const hypotheses = new Map();
  for (const [index, row] of history.entries()) {
    // An approved unbound row keeps a null source and a null result. It can never be the current row.
    const sourceOk = HASH.test(row?.sourceFingerprint)
      || (row?.sourceFingerprint === null && row.fullResultSha256 === null && unboundRevisions.has(row.revision) && index < history.length - 1);
    requireValue(row?.revision === index + 1 && text(row.hypothesis) && sourceOk,
      `Invalid or reset recovery history.${goport ? ` A goport batch lists unbound rows in ${GOPORT_RULE} unboundRevisions.` : ""}`);
    requireValue(row.fullResultSha256 === null || HASH.test(row.fullResultSha256), "History needs a result hash or explicit null.");
    // Later authorization does not change the initial four-revision trial.
    if (index < 4) hypotheses.set(row.hypothesis, (hypotheses.get(row.hypothesis) ?? 0) + 1);
  }
  requireValue(hypotheses.size <= 2 && [...hypotheses.values()].every(value => value <= 2), "Limit: two hypotheses, two revisions per hypothesis.");
  const last = history.at(-1);
  // A goport row and verdict bind to the goport test results, the gate manifest, the name map, the
  // gate id map, the oracle rebase runs and the oracle answer sets instead. checkGoport checks that the
  // gate id map and answer set files have their sha256.
  const oracle = goport ? oracleHashes(batch) : null;
  const evidence = item => goport ? item.goportTestsSha256 === batch.goportTests?.sha256 && item.gateSha256 === batch.gate?.sha256
    && (item.nameMapSha256 ?? null) === (batch.goportTests?.nameMap?.sha256 ?? null)
    && (item.gateIdMapSha256 ?? null) === (batch.gateIdMap?.sha256 ?? null)
    && (item.oracleRebaseSha256 ?? null) === oracle.oracleRebaseSha256
    && JSON.stringify([...(item.oracleAnswersSha256 ?? [])].sort()) === JSON.stringify(oracle.oracleAnswersSha256)
    : item.fullResultSha256 === batch.fullResult.sha256;
  const bound = goport ? "goport test, gate, name map and gate id map hashes, and the oracle rebase and answer set hashes" : "full result";
  requireValue(last.hypothesis === batch.hypothesis && last.sourceFingerprint === batch.sourceFingerprint && evidence(last),
    `Current history row does not match the batch source, hypothesis, and ${bound}.`);
  const verdicts = [batch.auditor, batch.reviewer];
  requireValue(batch.auditor?.role === "audit_accepted_roster", "Missing audit_accepted_roster verdict.");
  for (const verdict of verdicts) {
    requireValue(text(verdict?.role) && text(verdict?.agent) && verdict.verdict === "PASS", "Missing independent PASS verdict. STOP is the default.");
    requireValue(verdict.batchId === batch.id && verdict.sourceFingerprint === batch.sourceFingerprint && evidence(verdict),
      `Verdict is not bound to this batch, source, and ${bound}.`);
    requireValue(verdict.agent !== batch.implementer && verdict.role !== batch.implementer, "The implementer cannot supply an independent verdict.");
  }
  requireValue(batch.auditor.agent !== batch.reviewer.agent && batch.auditor.role !== batch.reviewer.role, "Auditor and reviewer must have distinct roles and agent identities.");
  return batch;
}

// Theo's standing goport-only roster carry-forward rule (2026-09-28). When no file outside
// crates/ts_goport changed (equal scripts/goport/roster_fp.py hashes), batch.fullResult may
// be the saved result of an earlier measured revision. Returns null without
// batch.rosterCarryForward, else the carry record, whose fromSourceFingerprint the full
// result must name. Verdicts and the current history row stay bound to this batch.
function rosterCarry(state, batch) {
  const carry = batch.rosterCarryForward;
  if (carry == null) return null;
  standingRule(state, CARRY_FORWARD_RULE, "Roster carry-forward");
  requireValue(HASH.test(batch.rosterFingerprint) && HASH.test(carry.rosterFingerprint) && HASH.test(carry.fromSourceFingerprint),
    "Roster carry-forward needs SHA-256 roster and source fingerprints.");
  requireValue(carry.rosterFingerprint === batch.rosterFingerprint, "Roster carry-forward fingerprint differs from the batch roster fingerprint.");
  requireValue(Number.isInteger(carry.fromRevision) && carry.fromRevision < batch.recoveryRevision, "Roster carry-forward must name an earlier revision.");
  const from = batch.recoveryHistory.find(row => row.revision === carry.fromRevision);
  // A carried row never ran the roster, so it cannot be a source for another carry.
  requireValue(typeof from?.status === "string" && from.status.startsWith("full_measured") && from.rosterCarryForward == null,
    `Roster carry-forward source R${carry.fromRevision} is not a full_measured revision.`);
  requireValue(from.sourceFingerprint === carry.fromSourceFingerprint && from.rosterFingerprint === batch.rosterFingerprint
    && from.fullResultSha256 === batch.fullResult.sha256,
  `R${carry.fromRevision} source, roster fingerprint or full result differs from the carry-forward record.`);
  return carry;
}

function laterPasses(report) {
  requireValue(report?.closure?.normalClosure === true && report.closure.sourceUnchanged === true, "Later baseline checker is not closed.");
  const rows = report.versusPreviousFullBaseline?.exactLedger?.map(row => ({ ...row, status: row.current?.status }));
  requireValue(Array.isArray(rows), "Later baseline checker ledger is missing.");
  for (const stage of ["compiler", "fixture"]) {
    const result = report.closedStages?.[stage];
    requireValue(result?.closure?.normalClosure === true && result.closure.sourceUnchanged === true && result.closure.sourceFingerprint === report.closure.sourceFingerprint,
      `Later baseline ${stage} is missing or has a different source.`);
    requireValue(Array.isArray(result.outcomes), `Later baseline ${stage} ledger is missing.`);
    rows.push(...result.outcomes);
  }
  rowsByKey(rows, "Later baseline");
  requireValue(rows.every(row => row.status === "PASS" || row.status === "FAIL"), "Later baseline has incomplete outcomes.");
  matchCounts(counts(rows), report.summary, "Later baseline");
  const passes = rows.filter(row => row.status === "PASS");
  requireValue(passes.length === 6330, "Later baseline must contain exactly 6330 PASS names.");
  return passes;
}

function currentResults(report, sourceFingerprint) {
  requireValue(report?.schemaVersion === 1 && report.status === "ALL_STAGES_CLOSED", "Missing full ALL_STAGES_CLOSED result.");
  requireValue(report.sourceFingerprint === sourceFingerprint && report.closure?.sourceFingerprint === sourceFingerprint,
    "Full result source mismatch.");
  requireValue(report.closure.normalClosures === true && report.closure.sourceUnchangedThroughAllStages === true,
    "Full result lacks unchanged-source normal closures.");
  requireValue(report.expectationChangesApplied === false && report.waiversApplied === false,
    "Expectation changes or waivers need human review and cannot pass this guard.");
  requireValue(Array.isArray(report.versusFullBaseline?.exactLedger) && Array.isArray(report.addedNames), "Full exact result ledger is missing.");
  const rows = report.versusFullBaseline.exactLedger.map(row => ({ ...row, ...row.current }));
  for (const row of report.addedNames) {
    const stage = report.stages?.find(item => item.path === row.path);
    rows.push({ ...row, logStage: stage?.stage });
  }
  const map = rowsByKey(rows, "Current result");
  requireValue(rows.every(row => row.status === "PASS" || row.status === "FAIL"), "Current ledger contains missing or unrun outcomes.");
  requireValue(Array.isArray(report.stages) && report.stages.length === 3 && new Set(report.stages.map(stage => stage.stage)).size === 3,
    "Need exactly one checker, compiler, and fixture stage.");
  for (const name of STAGES) {
    const stage = report.stages.find(item => item.stage === name);
    requireValue(stage?.normalClosure === true && text(stage.closureReceipt) && stage.sourceFingerprint === sourceFingerprint,
      `${name}: missing closure or source mismatch.`);
    const measured = counts(rows.filter(row => row.logStage === name));
    matchCounts(measured, stage.counts, name);
    requireValue(stage.exitCode === (measured.FAIL > 0 ? 101 : 0), `${name}: unexpected exit code.`);
  }
  requireValue(rows.every(row => STAGES.includes(row.logStage)), "Outcome has no closed stage.");
  matchCounts(counts(rows), report.summary, "Full summary");
  matchCounts(counts(rows), report.current, "Full current counts");
  requireValue(report.current.ABSENT === 0 && report.current.UNRUN === 0, "Full result reports absent or unrun selected outcomes.");
  return map;
}

// Exact outcomes of a saved full result, for the inherited-loss comparison.
function ledgerStatuses(report) {
  requireValue(Array.isArray(report?.versusFullBaseline?.exactLedger) && Array.isArray(report.addedNames), "Inherited result ledger is missing.");
  const rows = [...report.versusFullBaseline.exactLedger.map(row => ({ harness: row.harness, name: row.name, status: row.current?.status })),
    ...report.addedNames.map(row => ({ harness: row.harness, name: row.name, status: row.status }))];
  return rowsByKey(rows, "Inherited result");
}

function losses(baseline, current) {
  return baseline.flatMap(row => {
    const status = current.get(key(row))?.status ?? "ABSENT";
    return status === "PASS" ? [] : [{ harness: row.harness, name: row.name, status }];
  });
}

// Name map TSV (compare-tests.py --name-map): oldSuite, oldName, newSuite, newName and evidence
// per line. The evidence cell (Go evidence at the new pin, or why a test moved) must not be
// empty; more tab cells belong to it. "-" in both new columns marks a removed base name.
// Blank lines, "#" lines and a header row that starts with "oldSuite" are skipped. Returns
// Map(old key -> {line, to: [suite, name] | null, evidence}).
function parseNameMap(source) {
  const map = new Map();
  for (const [index, line] of source.split("\n").entries()) {
    const cells = line.replace(/\r$/, "").split("\t");
    if (!line.trim() || line.startsWith("#") || (index === 0 && cells[0] === "oldSuite")) continue;
    requireValue(cells.length >= 5 && cells.slice(0, 5).every(text),
      `Name map line ${index + 1} needs oldSuite, oldName, newSuite, newName and evidence.`);
    const id = JSON.stringify(cells.slice(0, 2));
    requireValue(!map.has(id), `Name map line ${index + 1} maps ${cells[0]} ${cells[1]} twice.`);
    map.set(id, { line: index + 1, to: cells[2] === "-" && cells[3] === "-" ? null : cells.slice(2, 4), evidence: cells.slice(4).join("\t") });
  }
  return map;
}

// results.json of goport-tests.sh: {source, pin, suites: {suite: {name: status}}, incomplete: [suite]}.
function testSuites(results, label) {
  const suites = results?.suites;
  requireValue(suites && typeof suites === "object" && !Array.isArray(suites), `${label}: missing suites.`);
  for (const [suite, names] of Object.entries(suites)) {
    requireValue(names && typeof names === "object" && !Array.isArray(names), `${label}: suite ${suite} is not a name map.`);
    for (const [name, status] of Object.entries(names)) {
      requireValue(TEST_STATUSES.has(status), `${label}: ${suite} ${name} has status ${JSON.stringify(status)}.`);
    }
  }
  return suites;
}

// Every base "ok" name must be "ok" at its own name or its mapped name. As in
// compare-tests.py: failed or ignored is lost; unrun, or a name missing from a missing or
// incomplete suite, is unrun; a name missing from a complete suite is absent. A map line that
// moves or removes a name needs the old name gone from the new results, and a new name that is
// not a base name (so a map cannot swap a lost name for a passing one). A removal needs a Go
// pin change (pinChanged: the base batch pin differs from batch.upstreamPin.to), or a kept-crate suite.
// A removal line may keep its old name in the new results only when it is a go_baselines_reference name that
// is "ignored" there (a stale Go reference file that no Go test at the new pin writes; bump C reviewer ruling 1
// item 5): removedIgnored lists it.
function compareTests(baseResults, newResults, map, pinChanged) {
  const before = testSuites(baseResults, "Base goport results"), after = testSuites(newResults, "goportTests results");
  requireValue(newResults.incomplete === undefined || (Array.isArray(newResults.incomplete) && newResults.incomplete.every(text)),
    "goportTests results: incomplete must be a list of suites.");
  const incomplete = new Set(newResults.incomplete ?? []);
  const has = (suites, suite, name) => Object.hasOwn(suites, suite) && Object.hasOwn(suites[suite], name);
  const removedIgnored = [];
  for (const [id, { line, to, evidence }] of map ?? []) {
    const [suite, name] = JSON.parse(id);
    if (to && to[0] === suite && to[1] === name) continue;
    const ignored = !to && suite === STALE_REFERENCE_SUITE && has(after, suite, name) && after[suite][name] === "ignored";
    if (ignored) removedIgnored.push({ suite, name, evidence });
    requireValue(ignored || !has(after, suite, name), `Name map line ${line}: ${suite} ${name} is still in the new results.`);
    requireValue(!to || !has(before, ...to), `Name map line ${line}: the new name ${to?.[0]} ${to?.[1]} is a base name.`);
    requireValue(to || pinChanged || KEPT_CRATE_SUITE.test(suite),
      `Name map line ${line} removes ${suite} ${name}, but the Go pin did not change and ${suite} is not a kept-crate suite.`);
  }
  const result = { baseOk: 0, retained: 0, recovered: 0, newNames: 0, removed: [], removedIgnored, lost: [], absent: [], unrun: [] };
  const targets = new Set();
  for (const [suite, names] of Object.entries(before)) {
    for (const [name, status] of Object.entries(names)) {
      const id = JSON.stringify([suite, name]);
      const entry = map?.get(id);
      const target = entry ? entry.to : [suite, name];
      if (status === "ok") result.baseOk++;
      if (target === null) {
        if (status === "ok") result.removed.push({ suite, name, evidence: entry.evidence });
        continue;
      }
      const targetId = JSON.stringify(target);
      requireValue(!targets.has(targetId), `Two base names map to ${target[0]} ${target[1]}.`);
      targets.add(targetId);
      const [toSuite, toName] = target;
      const now = has(after, toSuite, toName) ? after[toSuite][toName]
        : !Object.hasOwn(after, toSuite) || incomplete.has(toSuite) ? "unrun" : "absent";
      if (status !== "ok") {
        if (now === "ok") result.recovered++;
      } else if (now === "ok") {
        result.retained++;
      } else {
        const row = { suite, name, status: now, ...(targetId !== id && { mappedTo: { suite: toSuite, name: toName } }) };
        result[now === "absent" || now === "unrun" ? now : "lost"].push(row);
      }
    }
  }
  for (const [suite, names] of Object.entries(after)) {
    for (const name of Object.keys(names)) if (!targets.has(JSON.stringify([suite, name]))) result.newNames++;
  }
  return result;
}

// Runs python3 scripts/goport/<script> <args>. Exit 0 (pass) and exit 1 (a loss or regression)
// print one JSON object. Any other exit is bad input, which is STOP.
function runPython(script, args) {
  const run = spawnSync("python3", [resolve(ROOT, "scripts/goport", script), ...args], { encoding: "utf8", maxBuffer: 256 << 20 });
  const last = (run.stderr ?? "").trim().split("\n").at(-1);
  requireValue(run.status === 0 || run.status === 1, `${script} failed (exit ${run.status ?? run.error?.code}): ${last}`);
  try {
    return { exit: run.status, output: JSON.parse(run.stdout) };
  } catch {
    throw new Error(`${script} exit ${run.status} printed no JSON: ${last}`);
  }
}

// Runs scripts/goport/gate-compare.py, the one implementation of the gate item rules (removed
// ids, MATCH stays MATCH, ALLOWED needs allowedBy, no new FAIL, the open editor-long-growth
// noise rule), on two gate manifest files (absolute paths, already hash-checked) and the
// batch's openDefects, gateToolChanges (the tool changes the batch lists for the reviewer) and
// gateIdMap ({path, sha256} with an absolute path, or null: the id map of a pin bump).
// It reads files next to the base manifest (runs/editor/result.json), so it gets the real paths.
function runGateCompare(basePath, newPath, openDefects, gateToolChanges, gateIdMap = null) {
  const dir = mkdtempSync(join(tmpdir(), "check-gate-"));
  try {
    const state = join(dir, "state.json");
    writeFileSync(state, JSON.stringify({ batch: { openDefects: openDefects ?? [], gateToolChanges: gateToolChanges ?? [],
      ...(gateIdMap && { gateIdMap }) } }));
    const { exit, output } = runPython("gate-compare.py", [basePath, newPath, "--state", state]);
    requireValue(Array.isArray(output?.regressions) && Array.isArray(output.knownOpen) && (exit === 1) === (output.regressions.length > 0),
      "gate-compare.py output lacks its regressions and knownOpen lists.");
    return output;
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

// Runs scripts/goport/oracle-compare.py on LSP or API oracle results dirs (absolute paths): the base
// dirs (one, or the runs of batch.oracleRebase, where a request is protected when it is protected in
// any run) and the new dir. Every protected base request (same or oracle_error_same) must be protected
// again. Each answer set of answers ({path, sha256} with an absolute path, of this kind) goes as
// --answers path@sha256: a base flaky_oracle request in a set must keep goport's answer in the set at
// the set's pin. parity passes --parity (the new run matches Go at its pin) with --known-diff for each
// key of knownDiffs, and identity passes --identity (resultsSha256, goportSha256 and oracleSha256 of
// each dir). With any option it passes --kind. Exit 1 is a loss or a parity problem.
function runOracleCompare(baseDirs, newDir, { kind = null, answers = [], parity = false, knownDiffs = [], identity = false } = {}) {
  const args = [...answers.flatMap(ref => ["--answers", `${ref.path}@${ref.sha256}`]), ...(parity ? ["--parity"] : []),
    ...knownDiffs.flatMap(diff => ["--known-diff", diff]), ...(identity ? ["--identity"] : [])];
  const { exit, output } = runPython("oracle-compare.py", [...baseDirs, newDir, ...args, ...(args.length ? ["--kind", kind] : [])]);
  const total = output?.total, bad = parity ? output?.parity?.bad : 0;
  requireValue(total && ["lost", "unrun", "absent"].every(field => Number.isInteger(total[field])) && Number.isInteger(bad)
    && (exit === 1) === (total.lost + total.unrun + total.absent + bad > 0), "oracle-compare.py output lacks its totals.");
  return output;
}

// The committed build inputs of a commit: its crates tree and its Cargo.toml and Cargo.lock
// blobs (the committed part of candidate.sh's evidence key). candidate.sh reuses test and gate
// evidence across commits with the same inputs, so the check compares inputs, not commits.
function gitInputs(commit) {
  requireValue(typeof commit === "string" && /^[0-9a-f]{7,40}$/i.test(commit), `Not a commit hash: ${JSON.stringify(commit)}.`);
  const run = spawnSync("git", ["-C", ROOT, "rev-parse", ...["crates", "Cargo.toml", "Cargo.lock"].map(path => `${commit}:${path}`)],
    { encoding: "utf8" });
  const [crates, toml, lock] = run.status === 0 ? run.stdout.trim().split("\n") : [];
  requireValue(crates && toml && lock, `git has no crates tree, Cargo.toml and Cargo.lock for commit ${commit}.`);
  return { crates, toml, lock };
}

// The failed gate runs that candidate.sh side kept in an evidence dir (relative to the repository
// root): the paths of their gate-compare-fail-<label>.json files, in name order.
function keptGateRuns(dir) {
  let names;
  try {
    names = readdirSync(resolve(ROOT, dir));
  } catch {
    throw new Error(`Cannot list the gate evidence dir ${dir}.`);
  }
  return names.filter(name => /^gate-compare-fail-.+\.json$/.test(name)).sort().map(name => join(dir, name));
}

// batch.gateIdMap {path, sha256}: the gate id map of a pin bump (gate-compare.py uses it only when the
// base and new gates are at different Go pins). The file must have that sha256. Returns the
// reference with an absolute path for gate-compare.py, which checks the hash again, or null.
function gateIdMapRef(ref, readEvidence) {
  if (ref == null) return null;
  readEvidence(ref, { json: false });
  return { path: resolve(ROOT, ref.path), sha256: ref.sha256 };
}

// A list of oracle answer sets [{kind, path, sha256, pin}] (goport-oracle-answers/1 files: the recorded Go
// answers of the flake requests of a pin bump). Each file must have its sha256. Returns the list with
// absolute paths.
function answerSetRefs(list, label, readEvidence) {
  if (list == null) return [];
  requireValue(Array.isArray(list) && list.every(ref => (ref?.kind === "lsp" || ref?.kind === "api") && text(ref.path)
    && HASH.test(ref.sha256) && typeof ref.pin === "string" && GO_PIN.test(ref.pin)),
  `${label} must list answer sets {kind (lsp or api), path, sha256, pin}.`);
  for (const ref of list) readEvidence(ref, { json: false, gunzip: false });
  return list.map(ref => ({ ...ref, path: resolve(ROOT, ref.path) }));
}

// One key per answer set (kind, absolute path, sha256 and pin), sorted: two lists name the same sets when
// their keys are equal.
function answerKeys(list) {
  return JSON.stringify(list.map(ref => [ref.kind, resolve(ROOT, ref.path), ref.sha256, ref.pin].join(" ")).sort());
}

// batch.oracleRebase.<kind> (reviewer ruling 10) {runs [{label, dir, resultsSha256}], binsSha256,
// oracleSha256, knownDiffs? [{key, reason}] (API only), wire and toolSha256 (API only)}: runs of the base
// batch's bins (binsSha256, the tsgo sha256 of its gate manifest) with the oracle of the batch pin
// (oracleSha256, upstreamPin.oracleSha256). Returns the runs, the base of the compare.
function rebaseRuns(kind, entry, tsgo, oracle) {
  const field = `batch.oracleRebase.${kind}`;
  requireValue(Array.isArray(entry?.runs) && entry.runs.length > 0
    && entry.runs.every(run => text(run?.label) && text(run.dir) && HASH.test(run.resultsSha256))
    && new Set(entry.runs.map(run => resolve(ROOT, run.dir))).size === entry.runs.length,
  `${field} needs runs [{label, dir, resultsSha256}], each dir once.`);
  requireValue(entry.binsSha256 === tsgo, `${field}.binsSha256 must be the tsgo sha256 of the base batch's gate manifest (${tsgo}).`);
  requireValue(entry.oracleSha256 === oracle, `${field}.oracleSha256 must be the oracle of the batch pin, upstreamPin.oracleSha256 (${oracle}).`);
  const known = entry.knownDiffs ?? [];
  requireValue(Array.isArray(known) && known.every(diff => text(diff?.key) && text(diff.reason)) && new Set(known.map(diff => diff.key)).size === known.length
    && (kind === "api" || known.length === 0), `${field}.knownDiffs must list each {key, reason} once${kind === "lsp" ? "; the LSP has none" : ""}.`);
  if (kind === "api") {
    requireValue(API_REBASE_WIRES.includes(entry.wire) && HASH.test(entry.toolSha256),
      `${field} needs "wire": ${API_REBASE_WIRES.join(" or ")} and toolSha256, the api_oracle.py sha256 of its runs (bump C ruling 1 item 1).`);
  }
  return entry.runs;
}

// batch.oracleRebase.api (bump C reviewer ruling 1 item 1): the base bins speak an older API protocol, so each run
// is `api_oracle.py check --wire <entry.wire>` (3 or 4) with one API tool. Every battery of each run's manifest.json
// must have that wire and toolSha256 as its scriptSha.
function checkRebaseWire(entry, readEvidence) {
  for (const run of entry.runs) {
    const manifest = readEvidence({ path: join(run.dir, "manifest.json") }, { pinned: false });
    const batteries = Object.entries(manifest?.batteries ?? {});
    const other = batteries.filter(([, record]) => record?.wire !== entry.wire || record.scriptSha !== entry.toolSha256).map(([name]) => name);
    requireValue(batteries.length > 0 && other.length === 0,
      `batch.oracleRebase.api: rebase run ${run.label} (${run.dir}) has batteries without "wire": ${entry.wire} or api_oracle.py ${entry.toolSha256}${other.length ? `: ${other.join(", ")}` : ""}.`);
  }
}

// The external tools of the goport check. Tests may replace them.
export const TOOLS = { gitInputs, gateCompare: runGateCompare, oracleCompare: runOracleCompare, keptGateRuns };

function sameInputs(a, b) {
  return a.crates === b.crates && a.toml === b.toml && a.lock === b.lock;
}

// {label, dir} of the LSP or API oracle run that a batch field or the rule's apiBaseline names,
// as open_revision.py oracle_base does: its dir, or the dir of an older record's summary path.
function oracleRun(record) {
  const dir = text(record?.dir) ? record.dir : text(record?.summary) ? dirname(record.summary) : null;
  return dir ? { label: text(record.label) ? record.label : basename(dir), dir } : null;
}

function sameRun(a, b) {
  return (a == null && b == null) || samePath(a?.dir, b?.dir);
}

// An LSP or API oracle record of accept_revision.py ({label, dir, base {label, dir}, bases?, compare,
// output {path, sha256}}). Its base is the base batch's run, or the runs of batch.oracleRebase (rebase, in
// order; bases lists them all, base is the first). Its pinned oracle-compare.py output compares those base
// runs with its dir, names the answer sets (answers, of this kind) and shows no lost, unrun or absent
// request. The check also runs oracle-compare.py again on the dirs with the same answer sets. With rebase
// (reviewer ruling 10) it also passes --parity with the known diffs and --identity: each base run must
// have its resultsSha256, the base batch's tsgo (binsSha256) and the batch pin's oracle (oracleSha256), the
// new run that oracle, and the new run no parity problem (condition 3). Returns the new totals.
function checkOracle(field, kind, record, baseRuns, answers, rebase, pin, tools, readEvidence, reasons) {
  requireValue(baseRuns.length > 0 && baseRuns.every(Boolean), `${field}: the base batch names no oracle run to compare with.`);
  const dirs = baseRuns.map(run => run.dir).join(", ");
  const sameRuns = list => Array.isArray(list) && list.length === baseRuns.length && list.every((head, i) => samePath(head?.dir, baseRuns[i].dir));
  requireValue(text(record?.dir) && sameRuns(record.bases ?? [record.base]),
    `${field} needs its results dir and the base run${baseRuns.length > 1 ? "s" : ""} ${dirs}.`);
  const output = readEvidence(record.output);
  requireValue(sameRuns(output?.bases ?? [output?.base]) && samePath(output.new?.dir, record.dir),
    `${field}.output must be the oracle-compare.py output of ${dirs} and ${record.dir}.`);
  const shas = list => JSON.stringify((list ?? []).map(ref => ref?.sha256).sort());
  requireValue(shas(output.answers) === shas(answers),
    `${field}.output must use the answer sets ${answers.map(ref => ref.sha256).join(", ") || "(none)"}.`);
  const knownDiffs = (rebase?.knownDiffs ?? []).map(diff => diff.key);
  if (rebase) {
    requireValue(output.parity && output.parity.knownDiffs === knownDiffs.length && output.new?.oracleSha256,
      `${field}.output must be a compare with --parity, the ${knownDiffs.length} known diffs and --identity (batch.oracleRebase).`);
  }
  const saved = ["lost", "unrun", "absent"]
    .filter(name => size(record.compare?.[name], `${field}.compare.${name}`) > 0 || size(output.total?.[name], `${field}.output total ${name}`) > 0);
  if (saved.length) reasons.push(`${field} compare reports ${saved.join(", ")} requests.`);
  const again = tools.oracleCompare(baseRuns.map(run => resolve(ROOT, run.dir)), resolve(ROOT, record.dir),
    { kind, answers, parity: Boolean(rebase), knownDiffs, identity: Boolean(rebase) });
  const total = again.total;
  requireValue(shas(again.answers) === shas(answers), `${field}: oracle-compare.py did not use the answer sets.`);
  const lost = ["lost", "unrun", "absent"].filter(name => total[name] > 0);
  if (lost.length) reasons.push(`${field}: ${lost.map(name => `${total[name]} ${name}`).join(", ")} protected base requests.`);
  if (rebase) {
    (again.bases ?? [again.base]).forEach((head, i) => {
      const run = baseRuns[i], one = value => JSON.stringify([value]);
      requireValue(head?.resultsSha256 === run.resultsSha256,
        `${field}: rebase run ${run.label} (${run.dir}) has resultsSha256 ${head?.resultsSha256}, batch.oracleRebase says ${run.resultsSha256}.`);
      requireValue(JSON.stringify(head.goportSha256) === one(rebase.binsSha256),
        `${field}: rebase run ${run.label} ran tsgo ${(head.goportSha256 ?? []).join(", ") || "(none)"}, not the base batch's ${rebase.binsSha256}.`);
      requireValue(JSON.stringify(head.oracleSha256) === one(rebase.oracleSha256),
        `${field}: rebase run ${run.label} used the oracle ${(head.oracleSha256 ?? []).join(", ") || "(none)"}, not the batch pin's ${rebase.oracleSha256}.`);
    });
    requireValue(JSON.stringify(again.new?.oracleSha256) === JSON.stringify([rebase.oracleSha256]),
      `${field}: the new run ${record.dir} did not use the batch pin's oracle ${rebase.oracleSha256} only.`);
    // The answer sets of the batch pin are at its oracle, so parity checked each of their requests.
    const atPin = (again.answers ?? []).filter(set => sameHash(set.pin, pin));
    requireValue(atPin.every(set => set.oracleSha256 === rebase.oracleSha256)
      && again.parity.answerRequests + again.parity.badFirst.filter(row => row.class === "absent").length
        >= atPin.reduce((sum, set) => sum + set.requests, 0),
    `${field}: an answer set of the batch pin ${pin} is not at its oracle ${rebase.oracleSha256}, so parity did not check its requests.`);
    const bad = run => run.parity.bad ? `${run.parity.bad} (first: ${run.parity.badFirst.slice(0, 5)
      .map(row => `${row.battery}/${row.trace}#${row.event} ${row.class}: ${row.why}`).join("; ")})` : null;
    if (bad(output) || bad(again)) {
      reasons.push(`${field}: the new run does not match Go at the batch pin (ruling 10 condition 3): ${bad(again) ?? bad(output)} problems.`);
    }
  }
  // The dirs are not pinned. The same counts as the pinned output show that they did not change.
  const counts = run => ({ ...run.total, bases: (run.bases ?? [run.base]).map(head => [head?.traces, head?.requests]),
    new: [run.new?.traces, run.new?.requests], parity: run.parity?.bad ?? null });
  requireValue(JSON.stringify(counts(again)) === JSON.stringify(counts(output)),
    `${field}: oracle-compare.py gives other counts now than its pinned output ${record.output.path}. A results dir changed.`);
  return { label: record.label, base: baseRuns.map(run => run.label).join(","), traces: again.new.traces, requests: again.new.requests, ...total };
}

// The LSP run's summary.json (lsp_oracle.py, in its dir): it has the traces and requests that
// the compare counted (checked is checkOracle's result), ran the gate's tsgo, and has 0 diff,
// goport_error, timeout and crash (crash includes crash exits).
function checkLspClean(record, checked, gateManifest, readEvidence, reasons) {
  const summary = readEvidence({ path: join(record.dir, "summary.json") }, { pinned: false });
  requireValue(summary?.format === "goport-lsp-summary/1" && summary.batteries && typeof summary.batteries === "object",
    `${record.dir}/summary.json is not an lsp_oracle.py summary.`);
  const sum = name => Object.values(summary.batteries).reduce((total, battery) => total + (battery?.[name] ?? 0), 0);
  requireValue(sum("traces") === checked.traces && sum("requests") === checked.requests,
    `${record.dir}/summary.json counts other traces or requests than the oracle compare.`);
  const tsgo = gateManifest.binaries?.tsgo?.sha256;
  requireValue(HASH.test(tsgo) && Array.isArray(summary.goport) && summary.goport.length > 0 && summary.goport.every(bin => bin?.sha256 === tsgo),
    "languageServerOracle ran another goport binary than the gate's tsgo.");
  const classes = {};
  for (const battery of Object.values(summary.batteries)) {
    for (const [name, count] of Object.entries({ ...battery.classes, crash: (battery.classes?.crash ?? 0) + (battery.crashExits ?? 0) })) {
      classes[name] = (classes[name] ?? 0) + count;
    }
  }
  const bad = ["diff", "goport_error", "timeout", "crash"].filter(name => classes[name] > 0);
  if (bad.length) reasons.push(`LSP oracle has ${bad.map(name => `${classes[name]} ${name}`).join(", ")}.`);
}

// The API run's manifest.json (api_oracle.py check, in its dir) names the goport binary
// (goportSha) of each battery it ran. Every battery ran the gate manifest's tsgo, and the run has
// every battery of the base runs' manifest.json.
function checkApiRun(record, baseRuns, gateManifest, readEvidence, reasons) {
  const batteries = run => {
    const manifest = readEvidence({ path: join(run.dir, "manifest.json") }, { pinned: false });
    requireValue(manifest?.batteries && typeof manifest.batteries === "object" && !Array.isArray(manifest.batteries),
      `${run.dir}/manifest.json is not an api_oracle.py manifest.`);
    return manifest.batteries;
  };
  const tsgo = gateManifest.binaries?.tsgo?.sha256;
  const now = batteries(record);
  const other = Object.keys(now).filter(name => now[name]?.goportSha !== tsgo);
  requireValue(HASH.test(tsgo) && Object.keys(now).length > 0 && other.length === 0,
    `apiOracle ran another goport binary than the gate's tsgo${other.length ? ` (batteries ${other.join(", ")})` : ""}.`);
  const missing = [...new Set(baseRuns.flatMap(run => Object.keys(batteries(run))))].filter(name => !Object.hasOwn(now, name));
  if (missing.length) reasons.push(`apiOracle did not run the base batteries ${missing.join(", ")}.`);
}

// True when a note's JSON text names name as a whole word, as accept_revision.py named() does: an
// item id does not match a longer id, and a run label may also be one segment of a path.
function namesWord(value, name, path = false) {
  const [before, after] = path ? ["(?<![\\w.-])", "(?![\\w.-])"] : ["(?<![\\w./-])", "(?![\\w.-]|/\\w)"];
  return new RegExp(before + name.replace(/[.*+?^${}()|[\]\\]/g, "\\$&") + after).test(JSON.stringify(value));
}

// Every gate run of the batch source (repeat-run rule). batch.gateRuns (accept_revision.py) lists
// them oldest first, the batch gate last: {label, manifest, sha256, compare {path, sha256},
// regressions [{id, base, new, why, flake}]}. candidate.sh side keeps each failed run's compare
// as gate-compare-fail-<label>.json next to gateCompare.output, and each of them must be in the
// list. The check runs gate-compare.py again on each earlier run. Each regressed item needs a
// saved flake note flake-r<revision>-<name> whose text names the item id and the run label, or it
// is a loss. Returns the flakes and the number of runs.
function checkGateRuns(state, batch, baseManifest, idMap, inputs, pin, tools, readEvidence, reasons) {
  const runs = batch.gateRuns, last = Array.isArray(runs) ? runs.at(-1) : undefined;
  requireValue(samePath(last?.manifest, batch.gate.manifest) && last.sha256 === batch.gate.sha256,
    "gateRuns must list every gate run of the source, oldest first, with batch.gate last.");
  requireValue(runs.every(run => text(run?.label) && Array.isArray(run.regressions)) && new Set(runs.map(run => run.label)).size === runs.length,
    "Each gateRuns entry needs its own label and a regressions list.");
  for (const path of tools.keptGateRuns(dirname(batch.gateCompare.output.path))) {
    const kept = readEvidence({ path }, { pinned: false });
    requireValue(runs.some(run => run.sha256 === kept?.new?.sha256), `The failed gate run ${path} of the source is not in gateRuns.`);
  }
  const prefix = `flake-r${batch.recoveryRevision}-`, notes = Object.keys(state).filter(name => name.startsWith(prefix)).sort();
  const flakes = [];
  for (const run of runs.slice(0, -1)) {
    const manifest = readEvidence({ path: run.manifest, sha256: run.sha256 });
    requireValue(sameInputs(tools.gitInputs(manifest.commit), inputs) && sameHash(manifest.upstreamPin, pin),
      `Gate run ${run.label} is not a run of the batch source at the batch Go pin.`);
    const compare = readEvidence(run.compare);
    requireValue(compare?.new?.sha256 === run.sha256, `gateRuns ${run.label}: compare is not the gate-compare.py output of its manifest.`);
    const { regressions } = tools.gateCompare(baseManifest, resolve(ROOT, run.manifest), batch.openDefects, batch.gateToolChanges, idMap);
    const ids = list => JSON.stringify(list.map(item => item?.id).sort());
    requireValue(ids(run.regressions) === ids(regressions), `gateRuns ${run.label}: its regressions differ from gate-compare.py now.`);
    for (const item of regressions) {
      const note = notes.find(name => namesWord(state[name], item.id) && namesWord(state[name], run.label, true));
      if (note) flakes.push({ run: run.label, id: item.id, note });
      else reasons.push(`Gate run ${run.label}: ${item.id} ${item.base} -> ${item.new} (${item.why}) has no flake note ${prefix}<name> `
        + "that names the item and the run.");
    }
  }
  return { runs: runs.length, flakes };
}

// The Query core and Hono bound runs and the quality record of the batch source.
function checkRunEvidence(batch, readEvidence) {
  for (const label of ["ordinaryQuery", "latestHono"]) {
    const run = batch[label];
    requireValue(run?.complete === true && run.exitCode === 0 && run.matchesOracle === true && run.sourceFingerprint === batch.sourceFingerprint
      && Array.isArray(run.runs) && run.runs.length > 0,
    `${label}: bound run is missing, incomplete, differs from the oracle or names another source.`);
    for (const ref of run.runs) {
      requireValue(readEvidence({ path: ref?.manifest, sha256: ref?.sha256 }).sourceFingerprint === batch.sourceFingerprint,
        `${label}: bound run ${ref.manifest} names another source.`);
    }
  }
  requireValue(text(batch.quality?.record) && batch.qualityEvidence?.sourceFingerprint === batch.sourceFingerprint,
    "Quality needs a record and qualityEvidence for the batch source.");
  const quality = readEvidence({ path: batch.quality.record }, { pinned: false });
  requireValue(quality.sourceFingerprint === batch.sourceFingerprint && quality.rustfmtExit === 0 && quality.clippyExit === 0
    && quality.tsGoportWarnings === 0 && (quality.keptCrateWarnings ?? 0) === 0 && quality.fingerprintUnchanged === true,
  "Quality record is for another source or has rustfmt, clippy or ts_goport warning findings.");
}

// Theo's goport protected set (rule goport-protected-set, 2026-09-28). The base is the last
// accepted batch: its goportTests results when it was a goport batch, else the rule's pinned
// baseline (docs/goport-protected/tests-r131.json.gz); the gate base is always its gate manifest;
// the oracle bases are its LSP and API runs (the rule's apiBaseline after a legacy batch). The
// check recomputes the test comparison and runs gate-compare.py and oracle-compare.py itself,
// and also requires the saved compare records to show no loss or regression.
function checkGoport(state, rule, readEvidence, tools) {
  const batch = validateBatch(state, rule);
  requireValue(batch.rosterCarryForward == null, "A goport batch has no roster carry-forward.");
  requireValue(text(batch.commit), "A goport batch needs its commit.");
  // The Go pin of the batch. The results, the gate and the pin change of a name map removal use it.
  const pin = batch.upstreamPin?.to;
  requireValue(typeof pin === "string" && GO_PIN.test(pin),
    `A goport batch needs its Go pin: upstreamPin.to of 7 to 64 hex characters, not ${JSON.stringify(pin)}.`);
  // open_revision.py --new-batch archives the accepted batch as the last batchRecords entry and
  // names that entry, so an older accepted batch can never be the base.
  const archive = batch.previousBatch?.archive;
  const last = Array.isArray(state.batchRecords) ? state.batchRecords.at(-1) : undefined;
  requireValue(samePath(archive?.path, last?.path) && archive.sha256 === last.sha256,
    "previousBatch.archive must be the last saved batch record (batchRecords.at(-1)).");
  const previous = readEvidence(archive);
  requireValue(previous?.id === batch.previousBatch.id && previous.compilerAccepted === true,
    "previousBatch.archive is not the saved record of an accepted batch.");
  const previousGoport = previous.protectedSet === "goport";
  const baseRef = previousGoport ? { path: previous.goportTests?.results, sha256: previous.goportTests?.sha256 } : rule.baseline;
  const basePin = previous.upstreamPin?.to;
  requireValue(typeof basePin === "string" && GO_PIN.test(basePin),
    `Accepted batch ${previous.id} needs its Go pin: upstreamPin.to of 7 to 64 hex characters.`);
  // The oracle base runs. A legacy base batch had no API run: the rule's apiBaseline is its base.
  const lspBase = oracleRun(previous.languageServerOracle);
  const apiBase = oracleRun(previousGoport ? previous.apiOracle : rule.apiBaseline);
  // The answer sets of the base batch (oracleAnswers): they keep the flake requests of a pin bump protected.
  const baseAnswers = answerSetRefs(previous.oracleAnswers, `Accepted batch ${previous.id} oracleAnswers`, readEvidence);
  // open_revision.py saves the same base as batch.protectedBase when it opens the batch.
  const saved = batch.protectedBase;
  requireValue(saved == null || (saved.batch === previous.id && samePath(saved.tests?.path, baseRef.path) && saved.tests.sha256 === baseRef.sha256
    && samePath(saved.gate?.path, previous.gate?.manifest) && saved.gate.sha256 === previous.gate.sha256
    && sameRun(saved.lsp, lspBase) && sameRun(saved.api, apiBase) && answerKeys(saved.oracleAnswers ?? []) === answerKeys(baseAnswers)),
  `batch.protectedBase differs from the base that accepted batch ${previous.id} gives.`);
  const tests = batch.goportTests;
  requireValue(samePath(tests?.base, baseRef.path) && tests.baseSha256 === baseRef.sha256,
    previousGoport ? `goportTests base must be the results of accepted batch ${previous.id}.`
      : `goportTests base must be the protected baseline ${rule.baseline.path}.`);
  const inputs = tools.gitInputs(batch.commit);
  const results = readEvidence({ path: tests.results, sha256: tests.sha256 });
  requireValue(sameInputs(tools.gitInputs(results.source?.commit), inputs) && sameHash(results.source?.tree, inputs.crates),
    `goportTests results come from commit ${results.source?.commit}, whose crates tree, Cargo.toml or Cargo.lock differ from batch commit ${batch.commit}.`);
  requireValue(sameHash(results.pin, pin), `goportTests results are not at the batch Go pin ${pin}.`);
  const baseResults = readEvidence(baseRef);
  requireValue(sameHash(baseResults.pin, basePin), `The base goport results are not at the Go pin ${basePin} of accepted batch ${previous.id}.`);
  const map = tests.nameMap == null ? null : parseNameMap(readEvidence(tests.nameMap, { json: false }));
  const compared = compareTests(baseResults, results, map, !sameHash(basePin, pin));
  const reasons = [];
  const recorded = ["lost", "absent", "unrun"].filter(field => size(tests.compare?.[field], `goportTests.compare.${field}`) > 0);
  if (recorded.length) reasons.push(`goportTests.compare reports ${recorded.join(", ")} names.`);
  for (const field of ["lost", "absent", "unrun"]) {
    if (compared[field].length) reasons.push(`${compared[field].length} base ok goport test names are ${field}.`);
  }

  const gateCompare = batch.gateCompare;
  requireValue(text(previous.gate?.manifest) && samePath(gateCompare?.base, previous.gate.manifest)
    && (gateCompare.baseSha256 === undefined || gateCompare.baseSha256 === previous.gate.sha256),
  `gateCompare base must be the gate manifest of accepted batch ${previous.id}.`);
  requireValue(samePath(gateCompare.new, batch.gate?.manifest) && gateCompare.sha256 === batch.gate.sha256,
    "gateCompare new and sha256 must be batch.gate.");
  const newGate = readEvidence({ path: gateCompare.new, sha256: gateCompare.sha256 });
  requireValue(sameInputs(tools.gitInputs(newGate.commit), inputs),
    `Gate manifest comes from commit ${newGate.commit}, whose crates tree, Cargo.toml or Cargo.lock differ from batch commit ${batch.commit}.`);
  requireValue(sameHash(newGate.upstreamPin, pin), `Gate manifest is not at the batch Go pin ${pin}.`);
  const baseGate = readEvidence({ path: previous.gate.manifest, sha256: previous.gate.sha256 });
  const idMap = gateIdMapRef(batch.gateIdMap, readEvidence);
  const gate = tools.gateCompare(resolve(ROOT, previous.gate.manifest), resolve(ROOT, gateCompare.new), batch.openDefects, batch.gateToolChanges, idMap);
  const output = readEvidence(gateCompare.output);
  requireValue(output?.base?.sha256 === previous.gate.sha256 && output.new?.sha256 === batch.gate.sha256
    && (output.idMap?.sha256 ?? null) === (idMap?.sha256 ?? null),
  "gateCompare.output must be the gate-compare.py output for the base gate, batch.gate and batch.gateIdMap.");
  if (size(gateCompare.regressions, "gateCompare.regressions") > 0 || !Array.isArray(output.regressions) || output.regressions.length) {
    reasons.push("gateCompare reports gate regressions.");
  }
  if (gate.regressions.length) reasons.push(`${gate.regressions.length} gate items regressed against ${previous.gate.manifest}.`);
  const gateRuns = checkGateRuns(state, batch, resolve(ROOT, previous.gate.manifest), idMap, inputs, pin, tools, readEvidence, reasons);

  checkRunEvidence(batch, readEvidence);
  // batch.oracleAnswers: the answer sets of the base at the batch pin (accept_revision.py keeps them), and in a
  // pin-bump batch new answer sets at the batch pin (reviewer ruling 10 condition 5: the flake requests of the bump).
  const pinBump = !sameHash(basePin, pin);
  const answers = answerSetRefs(batch.oracleAnswers, "batch.oracleAnswers", readEvidence);
  const setKey = ref => `${ref.kind} ${ref.sha256}`;
  const inBase = new Set(baseAnswers.map(setKey)), own = new Set(answers.map(setKey));
  requireValue(own.size === answers.length, "batch.oracleAnswers names an answer set twice.");
  const dropped = baseAnswers.filter(ref => sameHash(ref.pin, pin) && !own.has(setKey(ref)));
  requireValue(dropped.length === 0,
    `batch.oracleAnswers must keep the answer sets of the base at the batch pin ${pin}: ${dropped.map(ref => ref.path).join(", ")}.`);
  for (const ref of answers) {
    requireValue(sameHash(ref.pin, pin), `batch.oracleAnswers ${ref.path} is at the Go pin ${ref.pin}, not the batch pin ${pin}.`);
    requireValue(inBase.has(setKey(ref)) || pinBump, `batch.oracleAnswers ${ref.path} is not an answer set of the base: only a pin-bump batch can add one.`);
  }
  // Each compare uses the answer sets of the base and of the batch of its kind, each once.
  const compareSets = [...new Map([...baseAnswers, ...answers].map(ref => [setKey(ref), ref])).values()];
  const ofKind = kind => compareSets.filter(ref => ref.kind === kind);
  // batch.oracleRebase (reviewer ruling 10): at a pin bump the oracle base is the base batch's bins measured
  // again at the batch pin, instead of the base batch's own runs.
  const rebase = batch.oracleRebase ?? null;
  let [lspRuns, apiRuns] = [[lspBase], [apiBase]];
  if (rebase != null) {
    requireValue(pinBump, `batch.oracleRebase is only for a pin-bump batch; base batch ${previous.id} is at the batch pin ${pin}.`);
    const tsgo = baseGate.binaries?.tsgo?.sha256, oracle = batch.upstreamPin?.oracleSha256;
    requireValue(HASH.test(tsgo) && HASH.test(oracle), "batch.oracleRebase needs the tsgo sha256 of the base gate manifest and upstreamPin.oracleSha256.");
    [lspRuns, apiRuns] = [rebaseRuns("lsp", rebase.lsp, tsgo, oracle), rebaseRuns("api", rebase.api, tsgo, oracle)];
    checkRebaseWire(rebase.api, readEvidence);
  }
  const lsp = checkOracle("languageServerOracle", "lsp", batch.languageServerOracle, lspRuns, ofKind("lsp"), rebase?.lsp ?? null, pin,
    tools, readEvidence, reasons);
  checkLspClean(batch.languageServerOracle, lsp, newGate, readEvidence, reasons);
  const api = checkOracle("apiOracle", "api", batch.apiOracle, apiRuns, ofKind("api"), rebase?.api ?? null, pin, tools, readEvidence, reasons);
  checkApiRun(batch.apiOracle, apiRuns, newGate, readEvidence, reasons);
  const { lost, absent, unrun, removed, removedIgnored, ...counts } = compared;
  return { verdict: reasons.length ? "STOP" : "PASS", scope: GOPORT_SCOPE, protectedSet: "goport", rule: GOPORT_RULE, reasons,
    base: { batch: previous.id, tests: baseRef.path, gate: previous.gate.manifest },
    counts: { goportTests: { ...counts, removedByMap: removed.length, removedIgnored: removedIgnored.length, lost: lost.length,
      absent: absent.length, unrun: unrun.length },
      gate: { baseItems: gate.counts?.baseItems, items: gate.counts?.items, regressions: gate.regressions.length, knownOpen: gate.knownOpen.length,
        runs: gateRuns.runs, flakes: gateRuns.flakes.length } },
    oracles: { lsp, api },
    knownOpenGateItems: gate.knownOpen,
    gateFlakes: gateRuns.flakes,
    nameMapRemoved: removed,
    nameMapRemovedIgnored: removedIgnored,
    losses: { goportTests: [...lost, ...absent, ...unrun], gate: gate.regressions.map(item => ({ id: item.id, base: item.base, now: item.new, why: item.why })) } };
}

// Tests may supply parsed synthetic evidence, a synthetic pin and a synthetic gitInputs. The
// CLI always uses readEvidence, INHERITED_PIN and TOOLS.
export function checkBatch(state, readEvidence = readEvidenceFile, inheritedPin = INHERITED_PIN, tools = TOOLS) {
  try {
    const goport = goportRule(state);
    if (goport) return checkGoport(state, goport, readEvidence, { ...TOOLS, ...tools });
    const batch = validateBatch(state);
    requireValue(batch.recoveryRevision <= LAST_LEGACY_REVISION,
      `R${batch.recoveryRevision} is after R${LAST_LEGACY_REVISION}, the last revision under the legacy cargo roster. `
      + `It needs protectedSet "goport" (rule ${GOPORT_RULE}).`);
    const carry = rosterCarry(state, batch);
    requireValue(state.acceptedBaseline?.sha256 === CHECKPOINT_SHA256, "Accepted checkpoint identity changed.");
    requireValue(state.originalAccepted?.sha256 === BASELINE_SHA256 && state.originalAccepted.expectedNames === 6055,
      "Original accepted baseline identity or count changed.");
    requireValue(state.laterPassBaseline?.sha256 === BASELINE_SHA256 && state.laterPassBaseline.expectedPasses === 6330,
      "Later PASS baseline identity or count changed.");
    readEvidence(state.acceptedBaseline);
    const original = readEvidence(state.originalAccepted);
    const accepted = original.originalAccepted6055?.exactLedger;
    const acceptedMap = rowsByKey(accepted, "Original accepted baseline");
    requireValue(acceptedMap.size === 6055 && accepted.every(row => row.accepted?.status === "PASS"), "Original baseline must contain 6055 exact PASS names.");
    const later = laterPasses(readEvidence(state.laterPassBaseline));
    const candidate = readEvidence(batch.fullResult);
    const current = currentResults(candidate, carry?.fromSourceFingerprint ?? batch.sourceFingerprint);
    const candidateAccepted = rowsByKey(candidate.originalAccepted6055?.exactLedger, "Candidate accepted ledger");
    requireValue(candidateAccepted.size === acceptedMap.size, "Candidate accepted ledger changed its name count.");
    for (const [id] of acceptedMap) {
      const row = candidateAccepted.get(id);
      requireValue(row && row.accepted?.status === "PASS", "Candidate accepted ledger renamed or omitted an original name.");
      requireValue(row.current?.status === (current.get(id)?.status ?? "ABSENT"), "Candidate accepted ledger disagrees with exact current outcomes.");
    }
    const originalLosses = losses(accepted, current), laterLosses = losses(later, current);
    // Opt-in crate rule: losses already present in the pinned inherited result are reported,
    // not blocking. The inherited counts must equal the approved counts exactly.
    const optIn = approvedRule(state, "opt-in-crate-no-new-loss");
    let inherited = null;
    if (optIn) {
      requireValue(optIn.inheritedResult?.sha256 === inheritedPin.sha256
        && optIn.inheritedLosses?.originalAccepted === inheritedPin.originalAccepted
        && optIn.inheritedLosses?.laterPasses === inheritedPin.laterPasses, "Inherited loss rule differs from the pinned constants.");
      const statuses = ledgerStatuses(readEvidence(optIn.inheritedResult));
      inherited = { originalAccepted: losses(accepted, statuses), laterPasses: losses(later, statuses) };
      requireValue(inherited.originalAccepted.length === optIn.inheritedLosses?.originalAccepted
        && inherited.laterPasses.length === optIn.inheritedLosses?.laterPasses, "Inherited loss counts differ from the approved rule.");
    }
    const isNew = (loss, list) => !list || !list.some(row => key(row) === key(loss));
    const newOriginal = originalLosses.filter(loss => isNew(loss, inherited?.originalAccepted));
    const newLater = laterLosses.filter(loss => isNew(loss, inherited?.laterPasses));
    const reasons = [];
    if (newOriginal.length) reasons.push(`${newOriginal.length} original accepted PASS names are FAIL or ABSENT${inherited ? " and not inherited" : ""}.`);
    if (newLater.length) reasons.push(`${newLater.length} later baseline PASS names are FAIL or ABSENT${inherited ? " and not inherited" : ""}.`);
    return { verdict: reasons.length ? "STOP" : "PASS", scope: SCOPE, reasons, rule: optIn ? optIn.id : null,
      ...(carry && { rosterCarryForward: { rule: CARRY_FORWARD_RULE, fromRevision: carry.fromRevision, fromSourceFingerprint: carry.fromSourceFingerprint } }),
      counts: { originalAccepted: 6055, originalRetained: 6055 - originalLosses.length, laterPasses: 6330, laterRetained: 6330 - laterLosses.length,
        inheritedOriginal: inherited?.originalAccepted.length ?? null, inheritedLater: inherited?.laterPasses.length ?? null },
      losses: { originalAccepted: originalLosses, laterPasses: laterLosses } };
  } catch (error) {
    return { verdict: "STOP", scope: state?.batch?.protectedSet === "goport" ? GOPORT_SCOPE : SCOPE, reasons: [error.message], counts: null };
  }
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  if (process.argv.length === 3 && process.argv[2] === "--help") {
    console.log(`Usage: node scripts/check-typechecker-batch.mjs <state-dir | legacy-state.json>

A state directory holds current.json and the append-only history.jsonl. The
check rebuilds the full state from both and stops if committed history lines
were changed or removed.

Read-only pre-acceptance check. Exit 0 means the protected-name and review
prerequisites pass. Exit 1 means STOP. This is not a Cargo wrapper.

batch.protectedSet selects the protected set. Without it the batch uses the
legacy cargo roster (below). R${LAST_LEGACY_REVISION} is the last revision under it:
a legacy batch with a later recoveryRevision is STOP, with or without the
roster carry-forward. protectedSet "goport" uses goport's own tests and gate
(see "Goport protected set" at the end).

State requires schemaVersion 1, phase initial-recovery or recovery-continuation, status ready,
decision REVIEW or PASS, goalAuthorization, and a preserved candidate hash.
Legacy roster: acceptedBaseline, originalAccepted, and laterPassBaseline need pinned path/SHA-256
references. Original expectedNames is 6055. Later expectedPasses is 6330.

batch needs id, implementer, hypothesis, sourceFingerprint, fullResult path/hash,
recoveryRevision, recoveryHistory, auditor, and reviewer. History retains all
measured revisions, including failures. Initial recovery limits are 4 revisions,
2 hypotheses, and 2 revisions per hypothesis. Continuation requires a saved
continuationAuthorization with authorized true, instruction, scope, and a valid
ISO date. Its history must keep all four initial revisions and each later
revision, numbered from 1 without gaps or resets. Initial limits still apply
to the first four rows. Later revisions have no fixed count or hypothesis limit.
Each history row needs revision, hypothesis,
sourceFingerprint, and fullResultSha256. A past result hash may be null.
The final row must match this completed full result.

Both verdicts need distinct role/agent identities, batchId, PASS,
sourceFingerprint, and fullResultSha256. Neither may be the implementer.
The auditor role is audit_accepted_roster. Missing evidence, STOP, source
mismatch, renamed/missing protected names, or expectation exceptions stop.

Theo-approved rules in acceptanceRuleChanges apply only to the named batch id:
- unbound-history-rows: listed past revisions may keep a null source and result.
- opt-in-crate-no-new-loss: losses already in the pinned inheritedResult are
  reported, not blocking. Inherited counts must equal the approved counts.
A rule with batchId "*" is ignored, except this standing rule:
- goport-only-roster-carry-forward (batchId "*", standing true, scope): used
  only when batch.rosterCarryForward {fromRevision, fromSourceFingerprint,
  rosterFingerprint} is present. batch.rosterFingerprint is the
  scripts/goport/roster_fp.py hash (all files except crates/ts_goport). The
  history row of the earlier fromRevision must be full_measured, not carried
  itself, with that sourceFingerprint, the same rosterFingerprint, and
  fullResultSha256 equal to batch.fullResult. The full result must then name
  fromSourceFingerprint. Protected names, inherited losses, the current history
  row and both verdicts stay bound to this batch source and full result. The
  output then has rosterCarryForward.

${SCOPE}

Goport protected set (batch.protectedSet "goport"):
The state needs Theo's standing rule ${GOPORT_RULE}: batchId "*", standing
true, approvedBy Theo, instruction, scope, an ISO date (2026-09-28 or
2026-09-28T20:57:30Z), protectedSet "goport", baseline {path, sha256} (the
first goport test baseline, docs/goport-protected/tests-r131.json.gz; its sha256
must be ${GOPORT_BASELINE_SHA256}),
approvalNote, the key of a saved note that holds Theo's approval, and
apiBaseline {label?, dir} (the API oracle run that is the API base after a
legacy base batch, which had no API run). Optional unboundRevisions lists
history rows that may keep a null source (as unbound-history-rows does, for
every batch). The batch has no fullResult, corpus, roster baselines or
rosterCarryForward. The history rules above still apply. The current history
row and both verdicts (PASS, batchId, sourceFingerprint) carry
goportTestsSha256 = goportTests.sha256, gateSha256 = gate.sha256,
nameMapSha256 = goportTests.nameMap.sha256, gateIdMapSha256 =
gateIdMap.sha256 (each map sha256 absent or null without that map),
oracleRebaseSha256 = the sha256 of the canonical JSON (keys sorted, no
spaces) of oracleRebase (absent or null without it) and oracleAnswersSha256 =
the sorted sha256 values of oracleAnswers (absent or empty without answer
sets) instead of fullResultSha256.

The base is batch.previousBatch.archive {path, sha256}, the saved record of the
last accepted batch: it must be the last batchRecords entry (open_revision.py
--new-batch writes it so) and an accepted batch. When it is a goport batch,
the test base is its goportTests results; else it is the rule baseline. The
gate base is its gate.
batch.upstreamPin.to is the batch Go pin (7 to 64 hex characters). The
upstreamPin.to of the base batch record is the base pin, and the base goport
results must be at it. batch.commit is the batch commit. Evidence from
another commit counts when that commit has the same build inputs (git crates
tree, Cargo.toml and Cargo.lock), as candidate.sh reuses it by that key.
- goportTests {results, sha256, base, baseSha256, compare, nameMap?}: results
  is the results.json of scripts/goport/goport-tests.sh
  ({source {commit, tree, testbinSha256}, pin, suites {suite {name: ok |
  failed | ignored | unrun}}, incomplete [suite]}). Its source.commit has the
  build inputs of batch.commit, source.tree is that crates tree, and its pin is
  batch.upstreamPin.to. compare.lost, absent and unrun (counts or lists) must
  be 0. The check also compares the files itself, as compare-tests.py does:
  each base ok name must be ok. failed or ignored is lost; unrun, or missing
  from a missing or incomplete suite, is unrun; missing from a complete suite
  is absent.
- nameMap {path, sha256}: a TSV of oldSuite, oldName, newSuite, newName and a
  non-empty evidence cell, for pin bumps and moved tests. A mapped old name
  must be gone from the new results, and a new name must not be a base name.
  A removal line may keep a go_baselines_reference old name that is "ignored"
  in the new results (a stale Go reference file that no Go test at the new pin
  writes); nameMapRemovedIgnored lists those lines with their evidence for the
  reviewer. An ignored name of another suite is still in the new results.
  "-" "-" removes a name: only when the base pin and batch.upstreamPin.to
  differ (the pins in the results files do not count), or for a kept-crate
  suite (ts_scanner, ts_ast, ts_diagnostics, ts_path, ts_core, ts_jsnum). The
  output lists the removed names with their evidence in nameMapRemoved for the
  reviewer.
- gateCompare {base, baseSha256?, new, sha256, regressions, output}: base is
  the previous gate manifest, new and sha256 equal batch.gate {manifest,
  sha256}, regressions (count or list) is 0, and output {path, sha256} is the
  gate-compare.py output for those two manifests, with no regressions. The new
  manifest has the build inputs of batch.commit and the batch pin. The check
  runs scripts/goport/gate-compare.py itself on the pinned manifests and
  batch.openDefects, so the gate rules (removed ids, MATCH stays MATCH,
  ALLOWED needs allowedBy, no new FAIL, the open editor-long-growth noise rule)
  have one implementation. Its known-open items are in knownOpenGateItems.
- gateIdMap {path, sha256} (optional): the gate id map of a pin bump, a TSV
  of old id, new id and case path (format in gate-compare.py). A line moves
  an id. A removal line (new id "-", and a note that names the Go commit that
  deletes the case; gate-compare.py checks only that the note has a word of
  7 to 40 hex digits) removes a case only when gate-compare.py checks the
  removal against Go at both pins (accountability rules, "Pin bumps"): the
  line's case path is the base item's, the case file is in the base pin's Go
  checkout, the new pin's Go checkout has neither that path nor its moved
  path, and no new run item of the line's family has either path. Else its
  base id is a removed id.
  gate-compare.py lists the removed cases in idMap.removed. The file must
  have that sha256, and the history row and both verdicts carry it as
  gateIdMapSha256. The check passes it and batch.gateToolChanges to
  gate-compare.py for the batch gate and each gateRuns run. gate-compare.py
  uses it only when the base and new manifests are at different Go pins: a
  mapped id is the same item, and a base allow entry moves only with its own
  case. A line's case path must equal the case path of the base item and of
  the new item. At a new pin of layout "typescript" (microsoft/TypeScript,
  tsc/) the new item's case path is the line's path moved to
  testdata/tests/cases, with the renames of the pin's
  testdata/promotedTestCollisions.txt. An unmapped base id of a mapped
  family, a missing new id and a line that names two cases give a removed id.
  gateCompare.output must name the same map sha256 in idMap (no idMap without
  a map).
- gateRuns [{label, manifest, sha256, compare {path, sha256}, regressions
  [{id, base, new, why, flake}]}]: every gate run of the source, oldest
  first, with batch.gate last (accept_revision.py). Each failed run that
  candidate.sh side kept next to gateCompare.output
  (gate-compare-fail-<label>.json) must be in the list. Each earlier run has
  the build inputs of batch.commit and the batch pin, and the check runs
  gate-compare.py on it again. Each regressed item needs a saved flake note
  flake-r<recoveryRevision>-<name> whose text names the item id and the run
  label (repeat-run rule), or the batch is STOP. The output lists the flakes
  in gateFlakes for the reviewer.
- ordinaryQuery and latestHono: complete, exitCode 0, matchesOracle, the batch
  sourceFingerprint, and runs [{manifest, sha256}] whose manifests name it.
- languageServerOracle and apiOracle {label, dir, base {label, dir}, bases?,
  compare, output {path, sha256}} (accept_revision.py): dir is the results
  dir of lsp_oracle.py or api_oracle.py check. The base run is the base
  batch's run (its dir, or the dir of its summary path); after a legacy base
  batch the API base is the rule's apiBaseline {label?, dir}. With
  oracleRebase the base runs are its runs of that kind, in order: bases lists
  them and base is the first. base.dir (bases) is that run, output is the
  oracle-compare.py output of the base and new dirs with the answer sets of
  that kind (below), and compare and output show 0 lost, unrun and absent
  requests. The check runs oracle-compare.py again on the dirs: every base
  same or oracle_error_same request must be so again (in any base run), and
  the counts must equal the pinned output. The LSP run's summary.json has the
  traces and requests of that compare, the gate manifest's tsgo and 0 diff,
  goport_error, timeout and crash. The API run's manifest.json gives the gate
  manifest's tsgo as the goportSha of every battery, and has every battery of
  the base runs' manifest.json.
- oracleAnswers [{kind, path, sha256, pin}]: answer sets
  (goport-oracle-answers/1: the recorded Go answers of the flake requests of a
  pin bump). Each file must have its sha256 and each set the batch pin. It
  holds every answer set of the base batch's oracleAnswers at the batch pin
  (accept_revision.py adds them), and only a pin-bump batch can add others.
  batch.protectedBase.oracleAnswers must equal the base batch's oracleAnswers.
  The compares use the base's and the batch's answer sets of their kind (each
  once, oracle-compare.py --answers path@sha256), and both pinned compare
  outputs must name them. A base flaky_oracle request in a set counts as
  retainedByAnswers when goport's new answer is in the set at the set's
  oracle, else as lost; with a set of another oracle only it is protected like
  a same request.
- oracleRebase {lsp, api} (optional, reviewer ruling 10): each
  {runs [{label, dir, resultsSha256}], binsSha256, oracleSha256, knownDiffs?
  [{key, reason}] (API only)}, the runs of the base batch's bins at the batch
  pin, which replace the base batch's runs as the oracle base. Only a
  pin-bump batch (the base batch pin differs from upstreamPin.to) can have it.
  binsSha256 must be the tsgo sha256 of the base batch's gate manifest and
  oracleSha256 upstreamPin.oracleSha256. The api entry also has "wire" (3,
  or 4 for protocol 4 base bins at a protocol 5 pin) and toolSha256 (bump C
  reviewer ruling 1 item 1): every battery of each
  run's manifest.json must have that wire and that api_oracle.py sha256 as
  its scriptSha (scripts/goport/oracle-rebase.sh writes the fragment). The check runs oracle-compare.py
  with every run as a base (protected in any run), --identity and --parity
  with the known diff keys: each run must have its resultsSha256, binsSha256
  as its only tsgo and oracleSha256 as its only oracle, and the new run that
  oracle. Parity: the new LSP run has no diff, goport_error,
  oracle_error_diff, timeout, crash or crash exit, and the new API run no
  goport_error, crash or timeout and no diff, id_only or oracle_error_diff
  outside knownDiffs; a request in an answer set of its oracle must have
  goport's answer in the set, and is then allowed. An unused known diff is a
  problem. Both pinned compare outputs must be such compares.
- quality {record} and qualityEvidence {sourceFingerprint}: the record names
  the batch source, rustfmtExit 0, clippyExit 0, tsGoportWarnings 0,
  keptCrateWarnings 0 or absent, fingerprintUnchanged true.

${GOPORT_SCOPE}

Independent review must compare retained history against saved batchRecords.
This check cannot prevent arbitrary direct commands or edits to state history.`);
  } else {
  let result;
  try {
    requireValue(process.argv.length === 3, "Usage: node scripts/check-typechecker-batch.mjs <state-dir | legacy-state.json>");
    const path = resolve(process.argv[2]);
    let state;
    if (statSync(path).isDirectory()) {
      verifyAppendOnly(path);
      state = loadState(path);
    } else {
      state = JSON.parse(readFileSync(path, "utf8"));
    }
    result = checkBatch(state);
  } catch (error) {
    result = { verdict: "STOP", scope: SCOPE, reasons: [`Missing or invalid state: ${error.message}`] };
  }
  const first20 = rows => ({ total: rows.length, first20: rows.slice(0, 20) });
  if (result.losses) result.losses = Object.fromEntries(Object.entries(result.losses).map(([name, rows]) => [name, first20(rows)]));
  if (result.nameMapRemoved) result.nameMapRemoved = first20(result.nameMapRemoved);
  console.log(JSON.stringify(result, null, 2));
  process.exitCode = result.verdict === "PASS" ? 0 : 1;
  }
}
