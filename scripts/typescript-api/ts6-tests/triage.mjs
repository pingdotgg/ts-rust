// Triage of a compiler-suite run against TS 7's own baselines (the Go harness at the tsc-rs pin).
// Entry point: `run.sh triage <out-dir>` (harness.mjs dispatches here). README.md "Triage" documents
// the classes and every normalization.
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const REPO = path.resolve(HERE, "../../..");
const CACHE = path.resolve(process.env.TS6_TESTS_CACHE || path.join(REPO, "target/typescript-api/ts60"));
// TS 7's baselines: the paths of microsoft/TypeScript at the tsc-rs pin that triage reads (loadTs7,
// ts7CaseFor, ts7Baseline), fetched into TS7_CHECKOUT on first use.
const TS7_PIN = {
    repo: "https://github.com/microsoft/TypeScript.git",
    commit: "fed0bf24149fb1ed36039212648bafdafc1ea10e",
    sparse: [
        "tsc/internal/testrunner",
        "tsc/testdata/baselines/reference/compiler",
        "tsc/testdata/baselines/reference/conformance",
        "tsc/testdata/tests/cases/compiler",
        "tsc/testdata/tests/cases/conformance",
    ],
};
const TS7_CHECKOUT = path.join(REPO, "target/typescript-api/ts71-baselines");
const DEFAULT_TS7 = process.env.TS6_TESTS_TS7 || path.join(TS7_CHECKOUT, "tsc");
const NO_CONTENT = null;

export const TRIAGE_USAGE = `  triage <out-dir> [--ts7 DIR] [--limit N]
      Classify every failing compiler-suite test of <out-dir> against TS 7's own baseline of the same
      name. DIR is the tsc/ directory of a microsoft/TypeScript checkout at the tsc-rs pin (default
      ${path.relative(REPO, TS7_CHECKOUT)}/tsc, a sparse shallow fetch of ${TS7_PIN.commit.slice(0, 12)} made on first
      use, about 300 MB; TS6_TESTS_TS7 overrides). Classes: A equals TS 7; B uses a setting TS 7
      removed; C no comparable TS 7 baseline (case removed or changed, variant dropped); D differs
      from TS 7 (a candidate bug). Also lists passing tests whose TS 7 baseline differs from TS 6.0's.
      Writes <out-dir>/triage.json and <out-dir>/triage.md, besides the default DIR; re-runnable.
      --limit caps the names listed per D cause in triage.md (default 12, 0 = all).`;

// TS 6.0: src/harness/harnessIO.ts optionRegex (directives of a test case).
const OPTION_REGEX = /^\/{2}\s*@(\w+)\s*:\s*([^\r\n]*)/gm;
const TITLE_REGEX = /^(compiler|conformance) tests \1 tests for (tests\/cases\/\S+?)(?: \((@.*)\))? Correct (errors|JS output) for \2$/;
const CLASS_TEXT = {
    A: "equals TS 7's baseline: expected TS 7 behavior",
    B: "uses a setting TS 7 removed",
    C: "no comparable TS 7 baseline",
    D: "differs from TS 7's baseline: candidate bug",
};

function readText(file) {
    try {
        return fs.readFileSync(file, "utf8");
    }
    catch (error) {
        if (error.code === "ENOENT" || error.code === "ENOTDIR") {
            return undefined;
        }
        throw error;
    }
}

function parseTriageArgs(args) {
    const options = { ts7: DEFAULT_TS7, fetchTs7: !process.env.TS6_TESTS_TS7, limit: 12 };
    const positional = [];
    for (let i = 0; i < args.length; i++) {
        const arg = args[i];
        if (arg === "--ts7") {
            options.ts7 = args[++i];
            options.fetchTs7 = false;
        }
        else if (arg === "--limit") {
            options.limit = Number(args[++i]);
        }
        else if (arg.startsWith("--")) {
            throw new Error(`unknown option ${arg}`);
        }
        else {
            positional.push(arg);
        }
    }
    if (positional.length !== 1 || !Number.isInteger(options.limit) || options.limit < 0) {
        throw new Error("usage: triage <out-dir> [--ts7 DIR] [--limit N]");
    }
    options.outDir = path.resolve(positional[0]);
    options.ts7 = path.resolve(options.ts7);
    return options;
}

// ---- test names ----

function parseVariation(description) {
    if (!description) {
        return [];
    }
    return description.split(/, (?=@)/).map(part => {
        const match = /^@(\w+): (.*)$/.exec(part);
        if (!match) {
            throw new Error(`cannot parse variation ${description}`);
        }
        return [match[1], match[2]];
    });
}

// TS 6.0: src/testRunner/compilerRunner.ts CompilerTest constructor (configuredName) and
// src/harness/harnessIO.ts doErrorBaseline / doJsEmitBaseline (the baseline file names).
// TS 7's Go harness names its baselines the same way.
function baselineName(caseFile, variation, check) {
    const justName = path.posix.basename(caseFile);
    let configuredName = justName;
    if (variation.length > 0) {
        const extname = path.posix.extname(justName);
        const basename = justName.slice(0, justName.length - extname.length);
        configuredName = `${basename}(${variation.map(([key, value]) => `${key.toLowerCase()}=${value.toLowerCase()}`).join(",")})${extname}`;
    }
    return check === "errors" ? configuredName.replace(/\.tsx?$/, ".errors.txt") : configuredName.replace(/\.tsx?/, ".js");
}

function parseTitle(title) {
    const match = TITLE_REGEX.exec(title);
    if (!match) {
        return undefined;
    }
    return {
        runner: match[1],
        caseFile: match[2],
        variation: parseVariation(match[3]),
        check: match[4] === "errors" ? "errors" : "js",
    };
}

// ---- test case settings and the settings TS 7 removed ----

// Test cases are read like TS 6.0's harness does (IO.readFile drops a byte order mark).
function readCase(file) {
    return readText(file)?.replace(/^\uFEFF/, "");
}

function directives(text) {
    const result = new Map();
    for (const match of text.matchAll(OPTION_REGEX)) {
        result.set(match[1].toLowerCase(), match[2].trim());
    }
    return result;
}

// The units of a case (TS 6.0: harnessIO.ts TestCaseParser.makeUnitsFromTest splits at @filename).
function caseUnits(text) {
    const units = [];
    let current;
    for (const line of text.split(/\r?\n/)) {
        const match = /^\/{2}\s*@(\w+)\s*:\s*([^\r\n]*)/.exec(line);
        if (match && match[1].toLowerCase() === "filename") {
            current = { name: match[2].trim(), lines: [] };
            units.push(current);
        }
        else if (current && !match) {
            current.lines.push(line);
        }
    }
    return units.map(unit => ({ name: unit.name, text: unit.lines.join("\n") }));
}

function stripJsonComments(text) {
    let result = "";
    for (let i = 0; i < text.length; i++) {
        const char = text[i];
        if (char === "\"") {
            const start = i;
            for (i++; i < text.length && text[i] !== "\""; i++) {
                if (text[i] === "\\") {
                    i++;
                }
            }
            result += text.slice(start, i + 1);
        }
        else if (char === "/" && text[i + 1] === "/") {
            while (i < text.length && text[i] !== "\n") {
                i++;
            }
            result += "\n";
        }
        else if (char === "/" && text[i + 1] === "*") {
            const end = text.indexOf("*/", i + 2);
            i = end < 0 ? text.length : end + 1;
        }
        else {
            result += char;
        }
    }
    return result.replace(/,(\s*[}\]])/g, "$1");
}

// compilerOptions of the case's tsconfig*.json / jsconfig*.json units (the harness compiles those
// projects with them), as lowercase name -> string value like directives.
function configSettings(text) {
    const settings = new Map();
    for (const unit of caseUnits(text)) {
        if (!/(^|\/)(tsconfig|jsconfig)[^/]*\.json$/i.test(unit.name)) {
            continue;
        }
        let compilerOptions;
        try {
            compilerOptions = JSON.parse(stripJsonComments(unit.text)).compilerOptions;
        }
        catch {
            continue;
        }
        for (const [key, value] of Object.entries(compilerOptions ?? {})) {
            if (typeof value === "string" || typeof value === "boolean" || typeof value === "number") {
                settings.set(key.toLowerCase(), String(value));
            }
        }
    }
    return settings;
}

function effectiveSettings(caseText, variation) {
    const settings = configSettings(caseText);
    for (const [key, value] of directives(caseText)) {
        settings.set(key, value);
    }
    for (const [key, value] of variation) {
        settings.set(key.toLowerCase(), value);
    }
    return settings;
}

// TS 7: tsc/internal/compiler/program.go verifyCompilerOptions ("Removed in TS7") plus the values
// TS 7's option parser no longer knows (target es3, module none, moduleResolution node).
function removedSettings(settings) {
    const removed = [];
    const value = key => settings.get(key)?.toLowerCase();
    if (["es3", "es5"].includes(value("target"))) {
        removed.push(`target=${value("target")}`);
    }
    if (["none", "amd", "umd", "system"].includes(value("module"))) {
        removed.push(`module=${value("module")}`);
    }
    if (["classic", "node10", "node"].includes(value("moduleresolution"))) {
        removed.push(`moduleResolution=${value("moduleresolution")}`);
    }
    for (const [key, name] of [["outfile", "outFile"], ["out", "out"], ["baseurl", "baseUrl"]]) {
        if (value(key)) {
            removed.push(name);
        }
    }
    if (value("downleveliteration") !== undefined) {
        removed.push("downlevelIteration");
    }
    for (const [key, name] of [["esmoduleinterop", "esModuleInterop"], ["allowsyntheticdefaultimports", "allowSyntheticDefaultImports"], ["alwaysstrict", "alwaysStrict"]]) {
        if (value(key) === "false") {
            removed.push(`${name}=false`);
        }
    }
    return removed;
}

// TS 7: tsc/internal/testutil/harnessutil/harnessutil.go SkipUnsupportedCompilerOptions and
// failOnUnsupportedCompilerOptions: configurations the Go harness never baselines.
function goSkipsSettings(settings) {
    const value = key => settings.get(key)?.toLowerCase();
    return ["amd", "umd", "system"].includes(value("module"))
        || ["classic", "node10", "node"].includes(value("moduleresolution"))
        || value("esmoduleinterop") === "false"
        || value("allowsyntheticdefaultimports") === "false"
        || Boolean(value("baseurl"))
        || Boolean(value("outfile"))
        || value("target") === "es5"
        || value("alwaysstrict") === "false";
}

// The case text without directive lines (a baseline shows the units without them).
function caseBody(text) {
    return text.split(/\r?\n/).filter(line => !/^\/{2}\s*@(\w+)\s*:/.test(line) || /^\/{2}\s*@filename\s*:/i.test(line)).join("\n");
}

// Whether the TS 7 case still produces this configuration (TS 7: harnessutil.go
// GetFileBasedTestConfigurations, splitOptionValues: an option varies when it lists several values or *).
function variantInCase(caseDirectives, variation) {
    for (const [key, value] of variation) {
        const listed = caseDirectives.get(key.toLowerCase());
        if (listed === undefined) {
            return false;
        }
        const entries = listed.split(",").map(entry => entry.trim().toLowerCase()).filter(Boolean);
        const wanted = value.toLowerCase();
        if (entries.length < 2 && !entries.includes("*")) {
            return false;
        }
        if (entries.includes(`-${wanted}`) || entries.includes(`!${wanted}`)) {
            return false;
        }
        if (!entries.includes(wanted) && !entries.includes("*")) {
            return false;
        }
    }
    return true;
}

// ---- TS 7 checkout ----

function git(cwd, args) {
    return execFileSync("git", args, { cwd, encoding: "utf8", stdio: ["ignore", "pipe", "inherit"] }).trim();
}

// TS7_CHECKOUT at the pin with the paths of TS7_PIN.sparse; a new checkout replaces a missing or
// wrong one only when it is complete.
function ensureTs7Checkout() {
    if (fs.existsSync(path.join(TS7_CHECKOUT, ".git")) && git(TS7_CHECKOUT, ["rev-parse", "HEAD"]) === TS7_PIN.commit) {
        if (git(TS7_CHECKOUT, ["sparse-checkout", "list"]) !== TS7_PIN.sparse.join("\n")) {
            git(TS7_CHECKOUT, ["sparse-checkout", "set", "--cone", ...TS7_PIN.sparse]);
        }
        return;
    }
    console.log(`triage: fetching TS 7's baselines (${TS7_PIN.repo} at ${TS7_PIN.commit.slice(0, 12)}) into ${path.relative(REPO, TS7_CHECKOUT)}`);
    const partial = `${TS7_CHECKOUT}.partial`;
    fs.rmSync(partial, { recursive: true, force: true });
    fs.mkdirSync(partial, { recursive: true });
    git(partial, ["init", "-q"]);
    git(partial, ["remote", "add", "origin", TS7_PIN.repo]);
    git(partial, ["sparse-checkout", "set", "--cone", ...TS7_PIN.sparse]);
    git(partial, ["fetch", "-q", "--depth", "1", "--filter=blob:none", "origin", TS7_PIN.commit]);
    git(partial, ["checkout", "-q", "FETCH_HEAD"]);
    fs.rmSync(TS7_CHECKOUT, { recursive: true, force: true });
    fs.renameSync(partial, TS7_CHECKOUT);
}

function gitCommit(dir) {
    const gitDir = path.join(dir, ".git");
    const head = readText(path.join(gitDir, "HEAD"));
    if (head === undefined) {
        return undefined;
    }
    const ref = /^ref: (.*)$/m.exec(head)?.[1];
    if (!ref) {
        return head.trim();
    }
    return readText(path.join(gitDir, ref))?.trim()
        ?? new RegExp(`^([0-9a-f]{40}) ${ref.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}$`, "m").exec(readText(path.join(gitDir, "packed-refs")) ?? "")?.[1];
}

function loadTs7(ts7) {
    const testdata = path.join(ts7, "testdata");
    if (!fs.existsSync(path.join(testdata, "baselines/reference/compiler"))) {
        throw new Error(`${ts7} is not the tsc/ directory of a TS 7 checkout (no testdata/baselines/reference/compiler)`);
    }
    // TS 7 renamed promoted TS 6.0 cases that collided with its own (promotedTestCollisions.txt).
    const promoted = new Map();
    for (const line of (readText(path.join(testdata, "promotedTestCollisions.txt")) ?? "").split("\n")) {
        const match = /^renamed-promoted\S* (\S+) -> (\S+)$/.exec(line.trim());
        if (match) {
            promoted.set(match[1], match[2]);
        }
    }
    const runner = readText(path.join(ts7, "internal/testrunner/compiler_runner.go")) ?? "";
    const skipped = new Set([...(/var skippedTests = \[\]string\{([\s\S]*?)\n\}/.exec(runner)?.[1] ?? "").matchAll(/"([^"]+)"/g)].map(match => match[1]));
    if (skipped.size === 0) {
        throw new Error(`no skippedTests list in ${ts7}/internal/testrunner/compiler_runner.go`);
    }
    // Cases whose JS output the Go harness does not baseline (nondeterministic parallel emit).
    const skippedEmit = new Set([...(/var skippedEmitTests = map\[string\]string\{([\s\S]*?)\n\}/.exec(runner)?.[1] ?? "").matchAll(/^\s*"([^"]+)":/gm)].map(match => match[1]));
    // Baselines where TS 7 knowingly differs from TS 6.0 (the former submodule diffs).
    const accepted = new Map();
    for (const list of ["submoduleAccepted.txt", "submoduleTriaged.txt"]) {
        for (const line of (readText(path.join(testdata, list)) ?? "").split("\n")) {
            if (/^(compiler|conformance)\//.test(line.trim())) {
                accepted.set(line.trim().replace(/\.diff$/, ""), list.replace(/^submodule|\.txt$/g, "").toLowerCase());
            }
        }
    }
    return { dir: ts7, testdata, promoted, skipped, skippedEmit, accepted, commit: gitCommit(path.dirname(ts7)) };
}

// ---- normalization ----

// TS 6.0's formatDiagnosticsWithColorAndContext (JS, kept by the package) against TS 7's Go
// diagnosticwriter: TS 7 puts a related message on its location line, ends every diagnostic block
// with an empty line, squiggles one character for an empty span, ends the summary with an empty
// line and writes CRLF in the "Errors  Files" table.
function normalizePretty(text) {
    let result = text.replace(/\r\n/g, "\n");
    result = result.replace(/^(  \u001b\[96m[^\n]*?\u001b\[0m:\u001b\[93m\d+\u001b\[0m:\u001b\[93m\d+\u001b\[0m) - ([^\n]*)\n((?:    [^\n]*\n)*)/gm, "$1\n$3    $2\n");
    result = result.replace(/\n\n(?=\u001b\[96m|\u001b\[91merror|==== )/g, "\n");
    result = result.replace(/^(    [^\u001b\n][^\n]*)\n\n(?=  \u001b\[96m)/gm, "$1\n");
    result = result.replace(/^((?:    )?\u001b\[7m \u001b\[0m \u001b\[9[16]m *)~(\u001b\[0m)$/gm, "$1$2");
    return result.replace(/\n+$/, "\n");
}

const ASTRAL = /[\u{10000}-\u{10FFFF}]/u;
const MARKER_LINE = /^    (\s*)(~+)$/;

// TS 6.0's harness draws error markers in UTF-16 units (harnessIO.ts iterateErrorBaseline), the Go
// harness in code points (tsbaseline/error_baseline.go): redraw TS 6.0's markers under source lines
// with astral characters in code points.
function markersInCodePoints(text) {
    const out = [];
    let source;
    for (const line of text.split("\n")) {
        const marker = source !== undefined ? MARKER_LINE.exec(line.replace(/\r$/, "")) : null;
        if (marker) {
            const start = marker[1].length;
            const prefix = [...source.slice(0, start)].map(char => (/\s/.test(char) ? char : " ")).join("");
            const count = [...source.slice(start, start + marker[2].length)].length;
            out.push(`    ${prefix}${"~".repeat(count)}${line.endsWith("\r") ? "\r" : ""}`);
            continue;
        }
        if (line.startsWith("    ") || !line.startsWith("!!! ")) {
            const body = line.replace(/\r$/, "").slice(4);
            source = line.startsWith("    ") && ASTRAL.test(body) ? body : undefined;
        }
        out.push(line);
    }
    return out.join("\n");
}

// Splits lines into entries: a line `startsEntry` accepts plus the lines after it that it does not.
function entries(block, startsEntry) {
    const result = [];
    for (const line of block) {
        if (startsEntry(line) || result.length === 0) {
            result.push([line]);
        }
        else {
            result[result.length - 1].push(line);
        }
    }
    return result;
}

const byCodeThenText = (a, b) => Number(/TS(\d+)/.exec(a[0])[1]) - Number(/TS(\d+)/.exec(b[0])[1]) || (a.join("\n") < b.join("\n") ? -1 : a.join("\n") > b.join("\n") ? 1 : 0);

// Diagnostics without a file come first in both harnesses, ordered by code in TS 6.0's
// compareDiagnostics (no start) but by their internal location first in TS 7's
// ast.CompareDiagnostics: order them by code (and text) on both sides, in the summary at the top
// and in the "!!! error" block before the first file.
function sortGlobalDiagnostics(text) {
    const all = text.split("\n");
    const isGlobal = line => /^error TS\d+: /.test(line.replace(/\r$/, ""));
    let end = 0;
    while (end < all.length && all[end].replace(/\r$/, "") !== "") {
        end++;
    }
    const top = entries(all.slice(0, end), line => /^(.*\(\d+,\d+\): )?error TS\d+: /.test(line));
    const globalCount = top.findIndex(entry => !isGlobal(entry[0]));
    const globals = globalCount < 0 ? top.length : globalCount;
    if (globals < 2) {
        return text;
    }
    const sortedTop = [...top.slice(0, globals)].sort(byCodeThenText).concat(top.slice(globals));
    let start = end;
    while (start < all.length && !all[start].startsWith("!!! error TS") && !all[start].startsWith("==== ")) {
        start++;
    }
    let stop = start;
    while (stop < all.length && all[stop].startsWith("!!! error TS")) {
        stop++;
    }
    const bangs = entries(all.slice(start, stop), line => /^!!! error TS\d+: (?! {2})/.test(line)).sort(byCodeThenText);
    return [...sortedTop.flat(), ...all.slice(end, start), ...bangs.flat(), ...all.slice(stop)].join("\n");
}

// Formatting that differs between the TS 6.0 JS harness ("ts6" side: our local baselines and TS
// 6.0's) and the Go harness ("ts7" side) only.
function normalize(text, side) {
    if (text === NO_CONTENT) {
        return NO_CONTENT;
    }
    // The Go harness drops "./" from unit names ("// @filename: ./a.ts") and joins "//" to "/".
    let result = text.replace(/^==== (.*) \((\d+) errors\) ====/gm, (_, name, count) => `==== ${name.replace(/^\.\//, "").replace(/\/{2,}/g, "/")} (${count} errors) ====`);
    if (result.includes("\u001b[")) {
        result = normalizePretty(result);
    }
    if (side === "ts6" && ASTRAL.test(result)) {
        result = markersInCodePoints(result);
    }
    return sortGlobalDiagnostics(result);
}

// ---- cause signatures of a D difference ----

function lines(text) {
    return text === NO_CONTENT ? [] : text.split(/\r?\n/);
}

// Top section of an .errors.txt: one entry per diagnostic, with its continuation lines.
function topDiagnostics(text) {
    const entries = [];
    for (const line of lines(text)) {
        if (line === "") {
            break;
        }
        const match = /^(?:(.*)\((\d+),(\d+)\): )?error (TS\d+): /.exec(line);
        if (match) {
            entries.push({ file: match[1] ?? "", pos: match[1] ? `${match[2]},${match[3]}` : "", code: match[4], text: line });
        }
        else if (entries.length > 0) {
            entries[entries.length - 1].text += "\n" + line;
        }
    }
    return entries;
}

function errorsFacts(local, ts7) {
    if (local !== NO_CONTENT && local.includes("\u001b[")) {
        return ["pretty output differs"];
    }
    const ours = topDiagnostics(local);
    const theirs = topDiagnostics(ts7);
    const facts = [];
    const key = entry => `${entry.file}(${entry.pos}) ${entry.code}`;
    const theirKeys = new Map(theirs.map(entry => [key(entry), entry]));
    const ourKeys = new Map(ours.map(entry => [key(entry), entry]));
    const extra = ours.filter(entry => !theirKeys.has(key(entry)));
    const missing = theirs.filter(entry => !ourKeys.has(key(entry)));
    const moved = new Set();
    for (const entry of extra) {
        const twin = missing.find(other => other.code === entry.code && !moved.has(other));
        if (twin) {
            moved.add(twin);
            moved.add(entry);
            facts.push(`${entry.code} at another position`);
        }
    }
    for (const entry of extra.filter(entry => !moved.has(entry))) {
        facts.push(`extra ${entry.code}${entry.file ? "" : " (global)"}`);
    }
    for (const entry of missing.filter(entry => !moved.has(entry))) {
        facts.push(`missing ${entry.code}${entry.file ? "" : " (global)"}`);
    }
    for (const entry of ours) {
        const twin = theirKeys.get(key(entry));
        if (twin && twin.text !== entry.text) {
            facts.push(`${entry.code} message text`);
        }
    }
    if (facts.length === 0 && ours.length !== theirs.length) {
        facts.push("diagnostic count");
    }
    if (facts.length === 0) {
        const related = text => lines(text).filter(line => line.startsWith("!!! related")).join("\n");
        facts.push(related(local) !== related(ts7) ? "related information" : "error body (file sections, markers)");
    }
    return [...new Set(facts)].sort();
}

// Sections of a .js baseline ("//// [name]" blocks; TS 6.0: harnessIO.ts doJsEmitBaseline).
function jsSections(text) {
    const sections = [];
    let current;
    for (const line of lines(text)) {
        const match = /^\/\/\/\/ \[(.*)\]( \/\/\/\/)?$/.exec(line);
        if (match) {
            current = { name: match[1], body: [] };
            sections.push(current);
        }
        else if (current) {
            current.body.push(line);
        }
    }
    return sections.map(section => ({ name: section.name, body: section.body.join("\n").trimEnd() }));
}

function outputKind(name) {
    if (name === "DtsFileErrors") {
        return "declaration diagnostics";
    }
    if (/\.d\.[cm]?ts$/.test(name)) {
        return "d.ts";
    }
    if (/\.map$/.test(name)) {
        return "source map";
    }
    if (/\.[cm]?jsx?$/.test(name)) {
        return "js";
    }
    return "other";
}

// The same "//// [name]" sections in another order: the Go harness lists some emitted files in
// another order than the tsc-rs CLI, the package and TS 6.0 write them.
function sameSectionsInOtherOrder(local, ts7) {
    if (local === NO_CONTENT || ts7 === NO_CONTENT || !local.startsWith("//// [")) {
        return false;
    }
    const key = section => `${section.name}\n${section.body}`;
    return jsSections(local).map(key).sort().join("\0") === jsSections(ts7).map(key).sort().join("\0");
}

function jsFacts(local, ts7) {
    if (local === NO_CONTENT || ts7 === NO_CONTENT) {
        return [local === NO_CONTENT ? "no JS baseline (TS 7 has one)" : "JS baseline where TS 7 has none"];
    }
    const ours = jsSections(local);
    const theirs = jsSections(ts7);
    const facts = [];
    const remaining = [...theirs];
    const unmatched = [];
    for (const section of ours) {
        const index = remaining.findIndex(other => other.name === section.name && other.body === section.body);
        if (index >= 0) {
            remaining.splice(index, 1);
        }
        else {
            unmatched.push(section);
        }
    }
    for (const section of unmatched) {
        const index = remaining.findIndex(other => other.name === section.name);
        if (index >= 0) {
            remaining.splice(index, 1);
            facts.push(`${outputKind(section.name)} content differs`);
        }
        else {
            facts.push(`extra ${outputKind(section.name)} output`);
        }
    }
    for (const section of remaining) {
        facts.push(`missing ${outputKind(section.name)} output`);
    }
    if (facts.length === 0) {
        facts.push("section order");
    }
    return [...new Set(facts)].sort();
}

// ---- classification ----

function localBaseline(shardDirs, name) {
    for (const dir of shardDirs) {
        const file = path.join(dir, name);
        const text = readText(file);
        if (text !== undefined) {
            return { file, text };
        }
        if (fs.existsSync(file + ".delete")) {
            return { file: file + ".delete", text: NO_CONTENT };
        }
    }
    return undefined;
}

function ts7CaseFor(ts7, caseFile) {
    const relative = caseFile.replace(/^tests\/cases\//, "");
    const renamed = ts7.promoted.get(relative) ?? relative;
    const ts7Case = { relative: renamed, renamed: renamed !== relative };
    return { ...ts7Case, text: undoPromotion(readCase(path.join(ts7.testdata, "tests/cases", renamed)), ts7Case, caseFile) };
}

// A promoted case that TS 7 renamed (X.ts -> X_promoted.ts) has the new name in its baselines, and
// the repository merge rewrote typescript-go issue links in promoted cases.
function undoPromotion(text, ts7Case, caseFile) {
    if (text === NO_CONTENT || text === undefined) {
        return text;
    }
    let result = text.replaceAll("github.com/microsoft/TypeScript/tsc/", "github.com/microsoft/typescript-go/");
    if (ts7Case.renamed) {
        const stem = name => path.posix.basename(name).replace(/\.[^.]+$/, "");
        result = result.replaceAll(stem(ts7Case.relative), stem(caseFile));
    }
    return result;
}

// What TS 7 baselines for this test: `ts7` is "same-case", "directives-changed" or "case-changed"
// when the Go harness runs the configuration (then `ts7Text` is its baseline or NO_CONTENT), else
// why it does not ("case-removed", "case-skipped", "variant-dropped", "variant-skipped",
// "emit-skipped").
function ts7Baseline(test, context) {
    const { ts7, caseTexts } = context;
    let ts6CaseText = caseTexts.get(test.caseFile);
    if (ts6CaseText === undefined) {
        ts6CaseText = readCase(path.join(CACHE, test.caseFile));
        if (ts6CaseText === undefined) {
            throw new Error(`TS 6.0 case ${test.caseFile} is missing from ${CACHE}`);
        }
        caseTexts.set(test.caseFile, ts6CaseText);
    }
    const removed = removedSettings(effectiveSettings(ts6CaseText, test.variation));
    const ts7Case = ts7CaseFor(ts7, test.caseFile);
    // TS 6.0's harness reads "@declaration: true;" as false, the Go harness as true.
    const odd = [...directives(ts6CaseText)].find(([, value]) => /;\s*$/.test(value));
    const harnessParse = odd ? `directive "@${odd[0]}: ${odd[1]}" ends with ";" (the harnesses read it differently)` : undefined;
    const result = { removed, ts7Case: ts7Case.relative, harnessParse };
    if (ts7Case.text === undefined) {
        return { ...result, ts7: "case-removed" };
    }
    if (ts7.skipped.has(path.posix.basename(ts7Case.relative))) {
        return { ...result, ts7: "case-skipped" };
    }
    if (test.check === "js" && ts7.skippedEmit.has(path.posix.basename(ts7Case.relative))) {
        return { ...result, ts7: "emit-skipped" };
    }
    if (test.variation.length > 0 && !variantInCase(directives(ts7Case.text), test.variation)) {
        return { ...result, ts7: "variant-dropped" };
    }
    if (goSkipsSettings(effectiveSettings(ts7Case.text, test.variation))) {
        return { ...result, ts7: "variant-skipped" };
    }
    const ts7Name = baselineName(ts7Case.relative, test.variation, test.check);
    const ts7File = path.join(ts7.testdata, "baselines/reference", test.runner, ts7Name);
    let ts7State = "case-changed";
    if (ts7Case.text === ts6CaseText) {
        ts7State = "same-case";
    }
    else if (caseBody(ts7Case.text) === caseBody(ts6CaseText)) {
        ts7State = "directives-changed";
    }
    return {
        ...result,
        ts7: ts7State,
        ts7Name,
        ts7File: fs.existsSync(ts7File) ? ts7File : undefined,
        ts7Text: undoPromotion(readText(ts7File) ?? NO_CONTENT, ts7Case, test.caseFile),
        accepted: ts7.accepted.get(`${test.runner}/${ts7Name}`),
    };
}

function classifyFailure(test, context) {
    const local = localBaseline(context.shardDirs, test.name);
    if (local === undefined) {
        throw new Error(`no local baseline ${test.name} for the failing test ${test.title}`);
    }
    const info = ts7Baseline(test, context);
    const entry = {
        name: test.name,
        runner: test.runner,
        check: test.check,
        case: test.caseFile,
        variation: Object.fromEntries(test.variation),
        local: path.relative(context.outDir, local.file),
        ts7: info.ts7,
        ts7Case: info.ts7Case,
        ts7Baseline: info.ts7File ? path.relative(context.ts7.dir, info.ts7File) : undefined,
        removed: info.removed.length > 0 ? info.removed : undefined,
        ts7VsTs6: info.accepted,
    };
    const ran = info.ts7Text !== undefined;
    const localText = normalize(local.text, "ts6");
    const ts7Text = ran ? normalize(info.ts7Text, "ts7") : undefined;
    if (ran && localText === ts7Text) {
        return { ...entry, class: "A", reason: info.ts7 === "same-case" ? "equals TS 7" : `equals TS 7 (${info.ts7})` };
    }
    if (ran && test.check === "js" && sameSectionsInOtherOrder(localText, ts7Text)) {
        return { ...entry, class: "A", reason: "equals TS 7 except the order of emitted files" };
    }
    if (info.removed.length > 0) {
        return { ...entry, class: "B", reason: info.removed.join(", "), ts6Difference: test.check === "errors" ? ts6Difference(test.name, local.text) : undefined };
    }
    if (info.harnessParse) {
        return { ...entry, class: "C", reason: info.harnessParse };
    }
    if (!ran || info.ts7 !== "same-case") {
        return { ...entry, class: "C", reason: info.ts7 };
    }
    const facts = test.check === "errors" ? errorsFacts(localText, ts7Text) : jsFacts(localText, ts7Text);
    if (local.text !== NO_CONTENT && (local.text.includes(REPO) || local.text.includes(context.outDir))) {
        facts.unshift("output shows the real working directory");
    }
    return { ...entry, class: "D", reason: `${test.check === "errors" ? "errors" : "JS"}: ${facts.join("; ")}` };
}

// TS 6.0 and TS 7 report removed and deprecated options with these codes.
const OPTION_REMOVAL_CODES = /\bTS(5101|5102|5107|5108)\b/;

// An .errors.txt without the removed/deprecated option diagnostics and the error counts.
function withoutOptionRemovals(text) {
    if (text === NO_CONTENT) {
        return "";
    }
    const out = [];
    let dropping = false;
    for (const line of text.split(/\r?\n/)) {
        const starts = /^(.*\(\d+,\d+\): )?error TS\d+: /.test(line) || /^!!! error TS\d+: (?! {2})/.test(line);
        if (starts) {
            dropping = OPTION_REMOVAL_CODES.test(line);
        }
        else if (dropping && (/^ {2}(?! {2})/.test(line) || /^!!! error TS\d+: {3}/.test(line))) {
            continue;
        }
        else {
            dropping = false;
        }
        if (!dropping) {
            out.push(line);
        }
    }
    return out.join("\n").replace(/\(\d+ errors\)/g, "(N errors)").replace(/\n{3,}/g, "\n\n").trim();
}

// For a B test: whether ours differs from TS 6.0's baseline only in removed/deprecated option diagnostics.
function ts6Difference(name, localText) {
    const ts6Text = readText(path.join(CACHE, "tests/baselines/reference", name)) ?? NO_CONTENT;
    return withoutOptionRemovals(localText) === withoutOptionRemovals(ts6Text) ? "option diagnostics only" : "more than option diagnostics";
}

// A passing test equals TS 6.0's baseline; it is suspicious when TS 7 baselines something else.
function checkPassing(test, context) {
    const info = ts7Baseline(test, context);
    if (info.ts7Text === undefined) {
        return undefined;
    }
    const ts6Text = normalize(readText(path.join(CACHE, "tests/baselines/reference", test.name)) ?? NO_CONTENT, "ts6");
    const ts7Text = normalize(info.ts7Text, "ts7");
    if (ts6Text === ts7Text || (test.check === "js" && sameSectionsInOtherOrder(ts6Text, ts7Text))) {
        return undefined;
    }
    let reason = "TS 7 baseline differs";
    if (info.removed.length > 0) {
        reason = `removed setting: ${info.removed.join(", ")}`;
    }
    else if (info.harnessParse) {
        reason = info.harnessParse;
    }
    else if (info.ts7 !== "same-case") {
        reason = info.ts7;
    }
    else if (info.accepted === "triaged") {
        reason = "TS 7 lists its baseline as a known TS 7 bug (submoduleTriaged.txt)";
    }
    return {
        name: test.name,
        runner: test.runner,
        check: test.check,
        ts7: info.ts7,
        reason,
        ts7VsTs6: info.accepted,
        ts7Baseline: info.ts7File ? path.relative(context.ts7.dir, info.ts7File) : undefined,
    };
}

function countBy(items, keyOf) {
    const counts = {};
    for (const item of items) {
        const key = keyOf(item);
        counts[key] = (counts[key] ?? 0) + 1;
    }
    return Object.fromEntries(Object.entries(counts).sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0])));
}

function renderTriage(triage, limit) {
    const out = [];
    const { failures, passingSuspicious } = triage;
    const listed = items => (limit === 0 ? items : items.slice(0, limit));
    const table = (title, counts) => {
        out.push(`### ${title}`, "", "| Reason | Tests |", "|---|---|");
        for (const [key, count] of Object.entries(counts)) {
            out.push(`| ${key} | ${count} |`);
        }
        out.push("");
    };
    out.push("# Compiler suite triage against TS 7 baselines", "");
    out.push(`- Run: \`${triage.run}\` (${triage.totals.tests} compiler tests, ${triage.totals.pass} pass, ${triage.totals.fail} fail)`);
    out.push(`- TS 7 baselines: \`${triage.ts7.dir}\` (commit ${triage.ts7.commit ?? "unknown"})`);
    out.push(`- TS 6.0 cases and baselines: \`${triage.ts60}\``);
    out.push("- Classes and normalizations: `scripts/typescript-api/ts6-tests/README.md` (Triage)", "");
    out.push("| Class | Meaning | Errors | JS | Total |", "|---|---|---|---|---|");
    for (const name of ["A", "B", "C", "D"]) {
        const counts = triage.classes[name];
        out.push(`| ${name} | ${CLASS_TEXT[name]} | ${counts.errors} | ${counts.js} | ${counts.errors + counts.js} |`);
    }
    out.push(`| all | failing tests | ${failures.filter(item => item.check === "errors").length} | ${failures.filter(item => item.check === "js").length} | ${failures.length} |`, "");
    table("A by TS 7 case state", countBy(failures.filter(item => item.class === "A"), item => item.reason));
    const b = failures.filter(item => item.class === "B");
    table("B by removed setting (first one listed)", countBy(b, item => item.removed[0]));
    table("B by what TS 7 does with the configuration", countBy(b, item => item.ts7));
    table("C by reason", countBy(failures.filter(item => item.class === "C"), item => item.reason));
    const d = failures.filter(item => item.class === "D");
    out.push(`## D: ${d.length} tests differ from TS 7, grouped by cause`, "");
    out.push("Cause: what differs between the normalized baselines. Errors: diagnostics extra, missing, at another position or with another text (by code), else related information or the file sections. JS: emitted files extra, missing or different (by kind). `ts7-vs-ts6` marks baselines where TS 7 itself differs from TS 6.0 (testdata/submoduleAccepted.txt, submoduleTriaged.txt).", "");
    for (const [reason, count] of Object.entries(countBy(d, item => item.reason))) {
        out.push(`### ${reason} (${count})`, "");
        const items = d.filter(item => item.reason === reason);
        for (const item of listed(items)) {
            out.push(`- \`${item.name}\` (${item.runner})${item.ts7VsTs6 ? ` ts7-vs-ts6 ${item.ts7VsTs6}` : ""}`);
        }
        if (items.length > listed(items).length) {
            out.push(`- ... ${items.length - limit} more in triage.json`);
        }
        out.push("");
    }
    out.push(`## Passing tests whose TS 7 baseline differs from TS 6.0's: ${passingSuspicious.length}`, "");
    out.push("A passing test equals TS 6.0's baseline. When TS 7 runs the same configuration and baselines something else, the package gave TS 6.0's answer where tsc-rs should give TS 7's.", "");
    table("By reason", countBy(passingSuspicious, item => item.reason));
    const plain = passingSuspicious.filter(item => item.reason === "TS 7 baseline differs");
    for (const item of listed(plain)) {
        out.push(`- \`${item.name}\` (${item.runner})${item.ts7VsTs6 ? ` ts7-vs-ts6 ${item.ts7VsTs6}` : ""}`);
    }
    if (plain.length > listed(plain).length) {
        out.push(`- ... ${plain.length - limit} more in triage.json`);
    }
    out.push("");
    return out.join("\n");
}

export function commandTriage(args) {
    const options = parseTriageArgs(args);
    const results = JSON.parse(fs.readFileSync(path.join(options.outDir, "results.json"), "utf8"));
    const shardDirs = fs.readdirSync(path.join(options.outDir, "ws"))
        .filter(name => name.startsWith("compiler"))
        .sort()
        .map(name => path.join(options.outDir, "ws", name, "tests/baselines/local"));
    if (options.fetchTs7) {
        ensureTs7Checkout();
    }
    const ts7 = loadTs7(options.ts7);
    const context = { ...options, ts7, shardDirs, caseTexts: new Map() };
    const failures = [];
    const passingSuspicious = [];
    const totals = { tests: 0, pass: 0, fail: 0 };
    for (const result of results) {
        if (result.suite !== "compiler") {
            continue;
        }
        const parsed = parseTitle(result.title);
        if (!parsed) {
            throw new Error(`cannot parse test title ${result.title}`);
        }
        const test = { ...parsed, name: baselineName(parsed.caseFile, parsed.variation, parsed.check), title: result.title };
        totals.tests++;
        if (result.state === "pass") {
            totals.pass++;
            const suspicious = checkPassing(test, context);
            if (suspicious) {
                passingSuspicious.push(suspicious);
            }
            continue;
        }
        totals.fail++;
        const reported = /tests\/baselines\/local\/(.*)$/.exec(result.error ?? "")?.[1];
        if (reported !== undefined && reported !== test.name) {
            throw new Error(`baseline name ${test.name} differs from the reported ${reported} (${result.title})`);
        }
        failures.push(classifyFailure(test, context));
    }
    if (totals.tests === 0) {
        throw new Error(`${options.outDir}/results.json has no compiler-suite tests`);
    }
    const triage = {
        run: options.outDir,
        ts60: CACHE,
        ts7: { dir: ts7.dir, commit: ts7.commit },
        totals,
        classes: Object.fromEntries(["A", "B", "C", "D"].map(name => [name, {
            errors: failures.filter(item => item.class === name && item.check === "errors").length,
            js: failures.filter(item => item.class === name && item.check === "js").length,
        }])),
        failures,
        passingSuspicious,
    };
    fs.writeFileSync(path.join(options.outDir, "triage.json"), JSON.stringify(triage, null, 1) + "\n");
    fs.writeFileSync(path.join(options.outDir, "triage.md"), renderTriage(triage, options.limit));
    const summary = ["A", "B", "C", "D"].map(name => `${name} ${triage.classes[name].errors + triage.classes[name].js}`).join(", ");
    console.log(`triage: ${totals.fail} failing compiler tests: ${summary}; ${passingSuspicious.length} passing tests with a TS 7 baseline that differs from TS 6.0's`);
    console.log(`wrote ${path.join(options.outDir, "triage.md")} and triage.json`);
    return 0;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
    try {
        process.exitCode = commandTriage(process.argv.slice(2));
    }
    catch (error) {
        console.error(`ts6-tests triage: ${error.message}`);
        process.exitCode = 2;
    }
}
