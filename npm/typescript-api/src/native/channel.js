"use strict";

// createChannel(args) opens a tsc-rs API session and returns an object with the interface of TS7's
// SyncRpcChannel (src/api/syncChannel.ts): requestSync, requestBinarySync, registerCallback,
// unregisterCallback, close, lastBytesSent and lastBytesReceived. `args` are the `tsgo --api`
// arguments. The session runs in this process (crates/ts_napi); TSRS_API_CHANNEL=process runs it in
// a `tsgo --api` child process instead (TSRS_TSGO names the binary).
//
// tsc-rs reads a failed read callback, and a filesystem answer of a kind or value it does not take,
// as a Go panic, and inside program loading that ends the whole process
// (crates/ts_goport/src/api/callbackfs.rs). So a filesystem callback that throws or gives such an
// answer gets a harmless answer, and its exception is thrown again when the request returns. The
// channel sends each valid answer in the form tsc-rs reads.
//
// tsc-rs's JSON decoder refuses a lone UTF-16 surrogate, which JSON.stringify writes as an escape
// (`\ud83d`); the channel sends U+FFFD in its place, in requests and answers. Both are one UTF-16
// unit, so positions stay.

const fs = require("node:fs");
const path = require("node:path");
const { loadAddon } = require("./addon.js");
const { ProcessTransport } = require("./process.js");

// crates/ts_napi/src/channel.rs read_answer answers the same when JS cannot run.
const readAnswers = new Map([
    ["readFile", JSON.stringify({ kind: "missing" })],
    ["fileExists", JSON.stringify({ kind: "value", value: false })],
    ["directoryExists", JSON.stringify({ kind: "value", value: false })],
    ["getAccessibleEntries", JSON.stringify({ kind: "value", value: { files: [], directories: [] } })],
    ["realpath", JSON.stringify({ kind: "identity" })],
    ["stat", JSON.stringify({ kind: "missing" })],
]);
// The answer kinds each filesystem callback takes, and the value of a "value" answer
// (crates/ts_goport/src/api/callbackfs.rs, `impl Fs for CallbackFS`). A value function returns the
// value to send or throws.
const answerShapes = new Map([
    ["readFile", { kinds: ["value", "missing", "useOS"], value: stringValue }],
    ["fileExists", { kinds: ["value", "useOS"], value: booleanValue }],
    ["directoryExists", { kinds: ["value", "useOS"], value: booleanValue }],
    ["getAccessibleEntries", { kinds: ["value", "useOS"], value: entriesValue }],
    ["realpath", { kinds: ["value", "identity", "useOS"], value: pathValue }],
    ["stat", { kinds: ["value", "missing", "fakeStat", "useOS"], value: statValue }],
    ["writeFile", { kinds: ["value", "noop", "useOS"], value: () => undefined }],
    ["removeFile", { kinds: ["value", "noop", "useOS"], value: () => undefined }],
]);
const fileSystemCallbacks = new Set(answerShapes.keys());
// The answers of each callback that are sent as they are.
const plainAnswers = new Map([...answerShapes].map(([name, { kinds, value }]) => [name, new Set([
    ...kinds.filter(kind => kind !== "value").map(kind => JSON.stringify({ kind })),
    ...(value === booleanValue ? [true, false].map(result => JSON.stringify({ kind: "value", value: result })) : []),
    ...(name === "writeFile" || name === "removeFile" ? [JSON.stringify({ kind: "value" })] : []),
])]));
const stringifiedPrefix = '{"kind":"value","value":';
const surrogateEscape = /\\u[dD][89a-fA-F][0-9a-fA-F]{2}/;
const surrogateEscapes = /(\\*)\\u([dD][89abAB][0-9a-fA-F]{2})(\\u[dD][c-fC-F][0-9a-fA-F]{2})?|(\\*)\\u[dD][c-fC-F][0-9a-fA-F]{2}/g;
const repoTsgo = path.resolve(__dirname, "..", "..", "..", "..", "target", "release", "tsgo");

class NativeTransport {
    constructor(args) {
        const { NativeChannel } = loadAddon();
        this.native = new NativeChannel(args);
    }

    requestSync(method, payload, onCall) {
        try {
            return this.native.requestSync(method, payload, onCall);
        }
        catch (error) {
            throw plainError(error);
        }
    }

    requestBinarySync(method, payload, onCall) {
        const bytes = typeof payload === "string" ? Buffer.from(payload, "utf8") : payload;
        try {
            return this.native.requestBinarySync(method, bytes, onCall);
        }
        catch (error) {
            throw plainError(error);
        }
    }

    close() {
        this.native.close();
    }
}

class SyncChannel {
    constructor(transport, collectTiming, sentinels) {
        this.transport = transport;
        this.collectTiming = collectTiming;
        this.callbacks = new Map();
        this.sentinels = sentinels;
        this.callbackFailure = undefined;
        this.onCall = (name, payload) => this.handleCall(name, payload);
        this.lastBytesSent = 0;
        this.lastBytesReceived = 0;
    }

    requestSync(method, payload) {
        const result = this.request(this.transport.requestSync, method, payload);
        if (this.collectTiming) {
            this.lastBytesReceived = Buffer.byteLength(result, "utf8");
        }
        return result;
    }

    requestBinarySync(method, payload) {
        const result = this.request(this.transport.requestBinarySync, method, payload);
        if (this.collectTiming) {
            this.lastBytesReceived = result.length;
        }
        return result;
    }

    registerCallback(name, callback) {
        this.callbacks.set(name, callback);
    }

    unregisterCallback(name) {
        this.callbacks.delete(name);
    }

    close() {
        this.transport.close();
    }

    request(send, method, payload) {
        if (typeof payload === "string") {
            payload = wellFormedJson(payload);
        }
        else if (ArrayBuffer.isView(payload)) {
            payload = wellFormedJsonBytes(payload);
        }
        if (this.collectTiming) {
            this.lastBytesSent = typeof payload === "string" ? Buffer.byteLength(payload, "utf8") : payload.length;
            this.lastBytesReceived = 0;
        }
        // A callback may make requests of its own; each request keeps the failures of its callbacks.
        const outer = this.callbackFailure;
        this.callbackFailure = undefined;
        let result;
        let failure;
        try {
            result = send.call(this.transport, method, payload, this.onCall);
        }
        catch (error) {
            failure = { error };
        }
        const callbackFailure = this.callbackFailure;
        this.callbackFailure = outer;
        if (callbackFailure !== undefined) {
            throw callbackFailure.error;
        }
        if (failure !== undefined) {
            throw failure.error;
        }
        return result;
    }

    handleCall(name, payload) {
        const callback = this.callbacks.get(name) ?? this.sentinels.get(name);
        let answer;
        try {
            if (callback === undefined) {
                throw new Error(`no callback named \`${name}\` found`);
            }
            answer = callback(name, payload);
        }
        catch (error) {
            return this.failCall(name, error);
        }
        if (typeof answer !== "string") {
            return this.failCall(name, new TypeError(`callback \`${name}\` returned ${typeof answer}, not a string`));
        }
        if (!fileSystemCallbacks.has(name)) {
            return wellFormedJson(answer);
        }
        try {
            return fileSystemAnswer(name, answer);
        }
        catch (error) {
            return this.failCall(name, error);
        }
    }

    // A failed read callback answers as if the file were missing; any other callback gets
    // CALL_ERROR (tsc-rs returns that as an error). The request throws the exception at its end.
    failCall(name, error) {
        this.callbackFailure ??= { error };
        const answer = readAnswers.get(name);
        if (answer === undefined) {
            throw error;
        }
        return answer;
    }
}

// The answer of filesystem callback `name` in the form tsc-rs reads, or an exception.
function fileSystemAnswer(name, answer) {
    if (plainAnswers.get(name).has(answer)) {
        return answer;
    }
    const stringified = stringifiedValue(answer);
    const parsed = stringified ?? parseAnswer(name, answer);
    const kind = parsed?.kind;
    if (kind === "error") {
        throw new Error(`filesystem callback returned serverFS.error: ${name}`);
    }
    const { kinds, value } = answerShapes.get(name);
    if (!kinds.includes(kind)) {
        throw invalidAnswer(name, answer, typeof kind === "string" ? `${name} takes no kind "${kind}"` : "it has no kind");
    }
    if (kind !== "value") {
        return JSON.stringify({ kind });
    }
    let result;
    try {
        result = value(parsed.value);
    }
    catch (error) {
        throw invalidAnswer(name, answer, `its value ${error.message}`);
    }
    // A file's text is most of the answers' bytes; sending it as it is saves a second JSON.stringify.
    if (stringified !== undefined && result === stringified.value) {
        return wellFormedJson(answer);
    }
    return JSON.stringify({ kind, value: result });
}

// The answer as JSON.stringify({ kind: "value", value }) writes it, when it is that: its value text
// is one JSON value, so the answer has no other members (tsc-rs refuses a duplicate name).
function stringifiedValue(answer) {
    if (!answer.startsWith(stringifiedPrefix) || !answer.endsWith("}")) {
        return undefined;
    }
    try {
        return { kind: "value", value: JSON.parse(answer.slice(stringifiedPrefix.length, -1)) };
    }
    catch {
        return undefined;
    }
}

function parseAnswer(name, answer) {
    try {
        return JSON.parse(answer);
    }
    catch {
        throw invalidAnswer(name, answer, "it is not JSON");
    }
}

function invalidAnswer(name, answer, reason) {
    const shown = answer.length > 80 ? `${answer.slice(0, 80)}...` : answer;
    return new TypeError(`filesystem callback \`${name}\` gave an invalid answer (${reason}): ${shown}`);
}

function stringValue(value) {
    if (typeof value !== "string") {
        throw new TypeError(`is ${describe(value)}, not a string`);
    }
    return value.toWellFormed();
}

// tsc-rs roots the path against the asked path's directory, and an empty one is a Go panic.
function pathValue(value) {
    const result = stringValue(value);
    if (result === "") {
        throw new TypeError("is an empty path");
    }
    return result;
}

function booleanValue(value) {
    if (typeof value !== "boolean") {
        throw new TypeError(`is ${describe(value)}, not a boolean`);
    }
    return value;
}

// An absent or null list is empty, as Go reads a nil slice; null entries are a Go nil dereference.
function entriesValue(value) {
    if (!isObject(value)) {
        throw new TypeError(`is ${describe(value)}, not {files, directories}`);
    }
    const entries = { files: names(value.files, "files"), directories: names(value.directories, "directories") };
    if (value.symlinks !== undefined && value.symlinks !== null) {
        entries.symlinks = names(value.symlinks, "symlinks");
    }
    return entries;
}

function names(list, field) {
    if (list === undefined || list === null) {
        return [];
    }
    if (!Array.isArray(list) || !list.every(name => typeof name === "string")) {
        throw new TypeError(`has ${field} that are not a list of strings`);
    }
    return list.map(name => name.toWellFormed());
}

// Go reads mode as uint32, size as int64 and mtime as RFC 3339 (callbackfs.rs parse_rfc3339_nano).
function statValue(value) {
    if (!isObject(value)) {
        throw new TypeError(`is ${describe(value)}, not {mode, size, mtime}`);
    }
    const mode = value.mode ?? 0;
    const size = value.size ?? 0;
    if (!Number.isInteger(mode) || mode < 0 || mode > 0xFFFFFFFF) {
        throw new TypeError("has a mode that is not a uint32");
    }
    if (!Number.isSafeInteger(size)) {
        throw new TypeError("has a size that is not an integer");
    }
    if (typeof value.mtime !== "string" || !isRfc3339(value.mtime)) {
        throw new TypeError("has an mtime that is not an RFC 3339 time");
    }
    return { mode, size, mtime: value.mtime };
}

function isRfc3339(text) {
    const match = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2}):(\d{2})(?:\.\d+)?(?:Z|[+-](\d{2}):(\d{2}))$/.exec(text);
    if (!match) {
        return false;
    }
    const [year, month, day, hour, minute, second, zoneHour, zoneMinute] = match.slice(1).map(part => Number(part ?? 0));
    const leap = year % 4 === 0 && (year % 100 !== 0 || year % 400 === 0);
    const days = month === 2 ? (leap ? 29 : 28) : [4, 6, 9, 11].includes(month) ? 30 : 31;
    return month >= 1 && month <= 12 && day >= 1 && day <= days && hour <= 23 && minute <= 59 && second <= 59 && zoneHour <= 23 && zoneMinute <= 59;
}

function isObject(value) {
    return typeof value === "object" && value !== null && !Array.isArray(value);
}

function describe(value) {
    if (value === null) {
        return "null";
    }
    return Array.isArray(value) ? "an array" : typeof value;
}

// JSON text with U+FFFD for each lone surrogate escape. An escape is a `\u` after an even number of
// backslashes; a high surrogate escape followed by a low one is a pair.
function wellFormedJson(text) {
    if (!surrogateEscape.test(text)) {
        return text;
    }
    return text.replace(surrogateEscapes, (match, highSlashes, high, low, lowSlashes) => {
        const slashes = highSlashes ?? lowSlashes;
        if (slashes.length % 2 === 1) {
            // Not an escape: an escaped backslash, then the text `u`. A low surrogate escape after it is one.
            return high === undefined ? match : `${slashes}\\u${high}${low === undefined ? "" : "\\ufffd"}`;
        }
        if (high !== undefined && low !== undefined) {
            return match;
        }
        return `${slashes}\\ufffd`;
    });
}

function wellFormedJsonBytes(bytes) {
    const buffer = Buffer.from(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    if (buffer.indexOf("\\ud") === -1 && buffer.indexOf("\\uD") === -1) {
        return bytes;
    }
    const text = buffer.toString("utf8");
    const result = wellFormedJson(text);
    return result === text ? bytes : Buffer.from(result, "utf8");
}

function createChannel(args, options = {}) {
    const { args: sessionArgs, sentinels } = takeErrorSentinels(args);
    const collectTiming = options.collectTiming ?? sessionArgs.includes("--timing");
    const transport = process.env.TSRS_API_CHANNEL === "process"
        ? new ProcessTransport(tsgoPath(options.tsgo), withApiFlag(sessionArgs))
        : new NativeTransport(sessionArgs);
    return new SyncChannel(transport, collectTiming, sentinels);
}

// `--callbacks=<name>:error` makes tsc-rs panic when it reaches that operation (serverFS.error), and
// a panic inside program loading ends the process. The channel answers those operations itself and
// fails the request instead.
function takeErrorSentinels(args) {
    const sentinels = new Map();
    const sessionArgs = args.map(arg => {
        const match = /^(--?callbacks=)(.*)$/.exec(arg);
        if (!match) {
            return arg;
        }
        const names = match[2].split(",").map(entry => {
            const [name, sentinel] = entry.split(":");
            if (sentinel !== "error" || !fileSystemCallbacks.has(name)) {
                return entry;
            }
            sentinels.set(name, () => {
                throw new Error(`filesystem operation configured with serverFS.error: ${name}`);
            });
            return name;
        });
        return match[1] + names.join(",");
    });
    return { args: sessionArgs, sentinels };
}

function withApiFlag(args) {
    return args[0] === "--api" ? args : ["--api", ...args];
}

function tsgoPath(fallback) {
    const exe = process.env.TSRS_TSGO || fallback || (fs.existsSync(repoTsgo) ? repoTsgo : undefined);
    if (exe === undefined) {
        throw new Error("TSRS_API_CHANNEL=process needs a tsgo binary: set TSRS_TSGO, or build target/release/tsgo.");
    }
    return exe;
}

// The native layer's errors carry a napi status code; the channel throws plain errors, as TS7's does.
function plainError(error) {
    if (error instanceof Error && error.code === "GenericFailure") {
        return new Error(error.message);
    }
    return error;
}

module.exports = { createChannel };
